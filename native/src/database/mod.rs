use crate::error::AppError;
use rusqlite::{Connection, OptionalExtension};
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
pub mod import;
mod pricing_seed;

macro_rules! lock_conn {
    ($mutex:expr) => {
        $mutex
            .lock()
            .map_err(|e| crate::error::AppError::Database(format!("Database lock failed: {e}")))?
    };
}
pub(crate) use lock_conn;

/// Bookkeeping tables that do not affect any statistics result: the collection cursors, the
/// deduplication ledger and the activity log. The first two are written frequently by scans, but a
/// real data change always comes with detail/daily rollup writes; the activity log is unrelated to
/// the statistics. Counting them separately would only blow the cache for nothing.
const EPOCH_IGNORED_TABLES: [&str; 3] = ["session_log_sync", "session_usage_dedup", "activity_log"];

/// The tables that end up in the local LAN export (`storage::export_local_usage`): the detail
/// records, the daily rollups, the provider names and the settings table holding the sharing
/// scope and the device name. Only writes to them advance [`Database::export_epoch`]. The
/// `lan_usage_*` copies synced in from other devices are not part of the export; counting them
/// would make two mutually paired devices change their own fingerprint because they stored the
/// other's data, and they would then download everything from each other forever.
const EXPORT_EPOCH_TABLES: [&str; 4] = [
    "proxy_request_logs",
    "usage_daily_rollups",
    "providers",
    "settings",
];

/// Shared state of the write epoch hooks.
#[derive(Default)]
pub(crate) struct EpochState {
    /// The result-related write epoch, see [`Database::write_epoch`].
    pub(crate) epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// The write epoch of the exported content, see [`Database::export_epoch`].
    export: std::sync::atomic::AtomicU64,
    /// A row-level write that counts towards the epoch happened in the current transaction; it is incremented once more on commit.
    pending: std::sync::atomic::AtomicBool,
    /// Same, for the export epoch.
    export_pending: std::sync::atomic::AtomicBool,
    /// The update_hook fired at least once in the current transaction (whether or not it counts towards the epoch).
    saw_rows: std::sync::atomic::AtomicBool,
    /// The lock holder is performing a bookkeeping write that does not affect statistics results ([`Database::write_quietly`]).
    quiet: std::sync::atomic::AtomicBool,
}

/// update_hook callback: a row-level write that counts towards the epoch increments it by 1 and
/// marks the transaction as pending commit. Written as a separate function so the closure can be
/// inferred through the higher-ranked lifetime.
fn write_epoch_bumper(
    state: std::sync::Arc<EpochState>,
) -> impl FnMut(rusqlite::hooks::Action, &str, &str, i64) + Send + 'static {
    use std::sync::atomic::Ordering::Relaxed;
    move |_action, _db, table, _rowid| {
        state.saw_rows.store(true, Relaxed);
        if state.quiet.load(Relaxed) || EPOCH_IGNORED_TABLES.contains(&table) {
            return;
        }
        state.epoch.fetch_add(1, Relaxed);
        state.pending.store(true, Relaxed);
        if EXPORT_EPOCH_TABLES.contains(&table) {
            state.export.fetch_add(1, Relaxed);
            state.export_pending.store(true, Relaxed);
        }
    }
}

/// Commit hook (only fires when a write transaction commits):
/// - When the transaction contained a row-level write that counts towards the epoch, it is
///   incremented once more at the moment of the commit. The row-level hook fires before the data
///   is visible, so a read connection may take an old snapshot between "the epoch changed and the
///   data is not committed"; this increment after the commit makes the cache keys of such old
///   snapshots invalid right away.
/// - A write transaction in which not a single row change was observed is also incremented
///   conservatively: `DELETE FROM t;` (without a WHERE) hits SQLite's truncate optimization and
///   does not fire the update hook, so it can only be caught here.
/// - Transactions that only touch bookkeeping tables or that run inside
///   [`Database::write_quietly`] do not count.
fn write_commit_bumper(state: std::sync::Arc<EpochState>) -> impl FnMut() -> bool + Send + 'static {
    use std::sync::atomic::Ordering::Relaxed;
    move || {
        let pending = state.pending.swap(false, Relaxed);
        let export_pending = state.export_pending.swap(false, Relaxed);
        let saw_rows = state.saw_rows.swap(false, Relaxed);
        if !state.quiet.load(Relaxed) {
            if pending || !saw_rows {
                state.epoch.fetch_add(1, Relaxed);
            }
            // A write transaction in which no row change was observed (truncate) is conservatively counted towards the export epoch as well.
            if export_pending || !saw_rows {
                state.export.fetch_add(1, Relaxed);
            }
        }
        false // false = commit as usual, do not roll back
    }
}

fn write_rollback_clearer(state: std::sync::Arc<EpochState>) -> impl FnMut() + Send + 'static {
    use std::sync::atomic::Ordering::Relaxed;
    move || {
        state.pending.store(false, Relaxed);
        state.export_pending.store(false, Relaxed);
        state.saw_rows.store(false, Relaxed);
    }
}

/// Set the quiet flag during bookkeeping writes; a guard makes sure it is reset on error or panic as well.
struct QuietGuard<'a>(&'a std::sync::atomic::AtomicBool);
impl Drop for QuietGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
    /// A read-only connection on the same database file (under WAL it can run concurrently with the write
    /// connection). Statistics queries use it, so they no longer queue behind each other while a background
    /// scan holds the write connection. It is None for in-memory/projection databases, which fall back to the
    /// write connection.
    pub(crate) read_conn: Option<Mutex<Connection>>,
    /// Path of the database file (None for in-memory/projection databases), used to open an independent read-only connection on demand.
    path: Option<PathBuf>,
    /// Process-unique instance number: every rebuilt projection is a new instance and can serve as part of
    /// a result cache key together with the write epoch (a pointer address may be reused after the old
    /// instance is freed and must not be used as a key).
    pub(crate) instance_id: u64,
    pub(crate) log_count_cache: Mutex<crate::services::usage_stats::LogCountCache>,
    /// The write epoch that affects statistics results (maintained by update_hook / commit_hook). It is
    /// the invalidation key for the expensive caches of status, projections, detail counts and the like,
    /// and the basis for frontend refreshes; data synced in from a peer counts as well. The collection
    /// cursors, the deduplication ledger and the bookkeeping writes inside
    /// [`Database::write_quietly`] do not count. The fingerprint compared by LAN peers is the narrower
    /// [`Database::export_epoch`].
    pub(crate) write_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub(crate) epoch_state: std::sync::Arc<EpochState>,
}
impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let conn = Connection::open(path.as_ref())?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // cache_size uses a 64MB page cache: aggregation queries scan the detail table over and over, the
        // 2MB default would keep paging in from the OS; desktop machines have plenty of memory, so give it
        // the full amount (the read connection is configured the same way).
        // journal_size_limit lets the WAL shrink back after a checkpoint instead of lingering at the high
        // water mark left by a large transaction.
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-65536; PRAGMA journal_size_limit=67108864;
             PRAGMA analysis_limit=1000;",
        )?;
        let mut db = Self::from_connection(conn)?;
        // Table creation and migrations are done on the write connection before the read connection is opened.
        let read = Connection::open_with_flags(
            path.as_ref(),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        read.busy_timeout(Duration::from_secs(5))?;
        read.execute_batch("PRAGMA query_only=1; PRAGMA cache_size=-65536;")?;
        db.read_conn = Some(Mutex::new(read));
        db.path = Some(path.as_ref().to_path_buf());
        Ok(db)
    }
    pub fn memory() -> Result<Self, AppError> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    /// A bare database without hooks and without a read connection, for in-memory projections (read-only, epoch always 0).
    pub(crate) fn detached(conn: Connection) -> Self {
        let epoch_state = std::sync::Arc::new(EpochState::default());
        static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            conn: Mutex::new(conn),
            read_conn: None,
            path: None,
            instance_id: NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            log_count_cache: Mutex::new(Default::default()),
            write_epoch: epoch_state.epoch.clone(),
            epoch_state,
        }
    }
    fn from_connection(conn: Connection) -> Result<Self, AppError> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 1 {
            return Err(AppError::Database(format!(
                "观测台数据库版本 {version} 高于当前支持版本，请升级应用。"
            )));
        }
        conn.execute_batch(include_str!("schema.sql"))?;
        Self::migrate_model_pricing_case_insensitive(&conn)?;
        Self::create_lan_usage_tables(&conn)?;
        let db = Self::detached(conn);
        {
            let conn = lock_conn!(db.conn);
            conn.update_hook(Some(write_epoch_bumper(db.epoch_state.clone())));
            conn.commit_hook(Some(write_commit_bumper(db.epoch_state.clone())));
            conn.rollback_hook(Some(write_rollback_clearer(db.epoch_state.clone())));
        }
        db.ensure_model_pricing_seeded()?;
        lock_conn!(db.conn).execute_batch("PRAGMA user_version=1;")?;
        Ok(db)
    }

    /// The model pricing table treats the model id case-insensitively.
    ///
    /// The built-in prices, the ids models.dev publishes and the ids the user
    /// types by hand spell the same model in different cases (`minimax-m3` and
    /// `MiniMax-M3` are the same model). They have to address one row,
    /// otherwise the price list shows the same model twice and an exact
    /// lookup can hit an arbitrary one of them.
    ///
    /// Databases created before this were built with a case-sensitive primary
    /// key and can hold several rows for the same model. The migration
    /// rebuilds the table with a case-insensitive key and folds the duplicates
    /// together. It runs before the seed is applied, so the seed cannot add
    /// a second variant of a model that is already there.
    fn migrate_model_pricing_case_insensitive(conn: &Connection) -> Result<usize, AppError> {
        let table_sql: Option<String> = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='model_pricing'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let Some(table_sql) = table_sql else {
            return Ok(0);
        };
        if table_sql.to_ascii_lowercase().contains("collate nocase") {
            return Ok(0);
        }

        type PricingRow = (String, String, String, String, String, String);
        let mut statement = conn.prepare(
            "SELECT model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
             FROM model_pricing ORDER BY model_id",
        )?;
        let rows: Vec<PricingRow> = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);

        // Fold the rows that only differ in case together. The canonical
        // lower-case id wins; when several rows share it, the first one in the
        // order above is kept. The prices of the local pricing file are
        // written back afterwards and overwrite the result, so this only
        // decides what happens to the rows the file does not cover.
        let mut folded: std::collections::BTreeMap<String, PricingRow> =
            std::collections::BTreeMap::new();
        let mut folded_away = 0usize;
        for row in rows {
            let key = row.0.to_ascii_lowercase();
            let keep_existing = folded.get(&key).is_some_and(|existing| {
                // The canonical lower-case id wins over a mixed-case variant.
                existing.0 == existing.0.to_ascii_lowercase()
            });
            if keep_existing {
                folded_away += 1;
                continue;
            }
            folded.insert(key, row);
        }
        let duplicates = folded_away;

        let transaction = conn.unchecked_transaction()?;
        transaction.execute_batch(
            "CREATE TABLE model_pricing_case_migrated (
                 model_id TEXT PRIMARY KEY COLLATE NOCASE, display_name TEXT NOT NULL,
                 input_cost_per_million TEXT NOT NULL, output_cost_per_million TEXT NOT NULL,
                 cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
                 cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
             );
             DROP TABLE model_pricing;",
        )?;
        for (model_id, row) in &folded {
            transaction.execute(
                "INSERT INTO model_pricing_case_migrated (
                     model_id, display_name, input_cost_per_million, output_cost_per_million,
                     cache_read_cost_per_million, cache_creation_cost_per_million
                 ) VALUES (?1,?2,?3,?4,?5,?6)",
                rusqlite::params![model_id, row.1, row.2, row.3, row.4, row.5],
            )?;
        }
        transaction.execute_batch("ALTER TABLE model_pricing_case_migrated RENAME TO model_pricing;")?;
        transaction.commit()?;

        if duplicates > 0 {
            log::info!("已合并 {duplicates} 条仅大小写不同的模型定价");
        }
        Ok(duplicates)
    }
    pub fn get_setting(&self, key: &str) -> Result<Option<String>, AppError> {
        Ok(lock_conn!(self.conn)
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), AppError> {
        lock_conn!(self.conn).execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[key,value])?;
        Ok(())
    }
    pub fn delete_setting(&self, key: &str) -> Result<(), AppError> {
        lock_conn!(self.conn).execute("DELETE FROM settings WHERE key=?1", [key])?;
        Ok(())
    }
    pub fn execute_batch(&self, sql: &str) -> Result<(), AppError> {
        lock_conn!(self.conn).execute_batch(sql)?;
        Ok(())
    }

    /// Low-frequency maintenance (triggered by the background scheduler in the gap after a scan, once an
    /// hour): TRUNCATE reclaims the WAL file, `PRAGMA optimize` updates the query statistics (with the
    /// analysis_limit sampling restriction from open). Holding the lock usually takes milliseconds;
    /// failures (including lock poisoning) are only logged and do not block the scheduler.
    pub fn maintenance_checkpoint(&self) {
        let Ok(conn) = self.conn.lock() else {
            return;
        };
        if let Err(e) = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;") {
            log::warn!("数据库维护（checkpoint/optimize）失败: {e}");
        }
    }

    /// Run a group of read-only queries under the same connection lock.
    ///
    /// All reads and writes go through this mutex, so there are no concurrent writes while the lock is
    /// held and the multiple queries inside the closure necessarily see the same data. Connection-level
    /// functions of the aggregation layer (such as
    /// [`crate::services::usage_stats::daily_usage_rows`]) must be called through this entry point,
    /// otherwise a background scan could slip between two queries and make the totals and the
    /// groupings contradict each other.
    ///
    /// Inside the closure do **not** call methods on `Database` that take the lock themselves:
    /// `std::sync::Mutex` is not reentrant, so a second `lock_conn!` while the outer lock is held
    /// deadlocks.
    pub fn with_connection<T, E>(&self, f: impl FnOnce(&Connection) -> Result<T, E>) -> Result<T, E>
    where
        E: From<AppError>,
    {
        let conn = lock_conn!(self.conn);
        f(&conn)
    }

    /// Run a group of queries in a read transaction on the read-only connection: the WAL snapshot
    /// guarantees that the multiple queries inside the closure see the same data, and it does not hold
    /// the write connection lock, so a background scan can write at the same time.
    ///
    /// Callers that use the write epoch as a cache key must read the epoch **before** the call: if the
    /// snapshot comes after the epoch was read, the worst case is "new data with an old key" and the
    /// next query recomputes because the key does not match; the other way round could cache old data
    /// under a new key.
    ///
    /// Without a read connection (in-memory and projection databases) it falls back to
    /// [`Self::with_connection`].
    pub fn with_read_connection<T, E>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<AppError>,
    {
        let Some(read) = &self.read_conn else {
            return self.with_connection(f);
        };
        let conn = lock_conn!(read);
        conn.execute_batch("BEGIN")
            .map_err(|e| AppError::Database(e.to_string()))?;
        let result = f(&conn);
        // A read-only transaction, where COMMIT and ROLLBACK are equivalent; on failure the connection may
        // keep an open transaction, so issue another ROLLBACK to avoid an error on the next BEGIN.
        if conn.execute_batch("COMMIT").is_err() {
            let _ = conn.execute_batch("ROLLBACK");
        }
        result
    }

    /// Run a group of queries in a read snapshot on an **independent** read-only connection, closed again
    /// right afterwards.
    ///
    /// Meant for long full-database reads (such as building the multi-device projection): the shared
    /// read-only connection is a mutex as well, and holding it for a long time makes other queries
    /// queue up; an independent connection only pays the cost of opening it once (about 1 ms) and runs
    /// in parallel with both the write and the shared read connection. An in-memory database has no file
    /// path and falls back to [`Self::with_connection`]. The read order required for epoch cache keys is
    /// the same as for [`Self::with_read_connection`].
    pub(crate) fn with_private_read_snapshot<T, E>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, E>,
    ) -> Result<T, E>
    where
        E: From<AppError>,
    {
        let Some(path) = &self.path else {
            return self.with_connection(f);
        };
        let open = || -> rusqlite::Result<Connection> {
            let conn = Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                    | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?;
            conn.busy_timeout(Duration::from_secs(5))?;
            conn.execute_batch("PRAGMA query_only=1; PRAGMA cache_size=-65536; BEGIN")?;
            Ok(conn)
        };
        let conn = open().map_err(|e| AppError::Database(e.to_string()))?;
        let result = f(&conn);
        let _ = conn.execute_batch("COMMIT");
        result
    }

    /// Perform a bookkeeping write that does not affect statistics results (sync time, error messages,
    /// peer fingerprints, ...): it does not advance the write epoch, which avoids having this state that
    /// is written every round blow away all aggregation caches and the frontend dashboard, and also
    /// avoids the two LAN sides re-downloading from each other because of the other's bookkeeping writes.
    pub fn write_quietly<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let conn = lock_conn!(self.conn);
        self.epoch_state
            .quiet
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let _guard = QuietGuard(&self.epoch_state.quiet);
        f(&conn)
    }

    /// The write epoch of the local exported content: it only advances with writes to `EXPORT_EPOCH_TABLES`.
    /// LAN peers use it to tell whether the local export changed (`/api/lan/usage/fingerprint`).
    pub fn export_epoch(&self) -> u64 {
        self.epoch_state
            .export
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The current write epoch (read snapshot), see [`Self::write_epoch`].
    pub fn data_epoch(&self) -> u64 {
        self.write_epoch.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The bookkeeping version of [`Self::set_setting`], see [`Self::write_quietly`].
    pub fn set_setting_quietly(&self, key: &str, value: &str) -> Result<(), AppError> {
        self.write_quietly(|conn| {
            conn.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [key, value])?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model id is a case-insensitive key. Databases created before that
    /// were built with a case-sensitive primary key and can hold several rows
    /// for the same model; opening them folds the duplicates together and
    /// rebuilds the table with the case-insensitive key.
    #[test]
    fn opening_an_old_database_folds_the_case_duplicate_pricing_rows() {
        let temp = tempfile::tempdir().expect("tempdir");
        let previous = std::env::var_os("OBSERVATORY_TEST_HOME");
        std::env::set_var("OBSERVATORY_TEST_HOME", temp.path());
        let path = temp.path().join("observatory.db");
        {
            let conn = Connection::open(&path).expect("create database");
            conn.execute_batch(
                "CREATE TABLE model_pricing (
                     model_id TEXT PRIMARY KEY, display_name TEXT NOT NULL,
                     input_cost_per_million TEXT NOT NULL, output_cost_per_million TEXT NOT NULL,
                     cache_read_cost_per_million TEXT NOT NULL DEFAULT '0',
                     cache_creation_cost_per_million TEXT NOT NULL DEFAULT '0'
                 );
                 INSERT INTO model_pricing VALUES ('zz-case-test', 'ZZ Case Test', '1', '2', '0', '0');
                 INSERT INTO model_pricing VALUES ('ZZ-CASE-TEST', 'ZZ Case Test', '3', '4', '0', '0');",
            )
            .expect("create the old pricing table");
        }

        let db = Database::open(&path).expect("open the migrated database");
        {
            let conn = db.conn.lock().expect("lock test database");
            let rows: Vec<(String, String)> = conn
                .prepare(
                    "SELECT model_id, input_cost_per_million FROM model_pricing
                     WHERE model_id = ?1 COLLATE NOCASE",
                )
                .expect("prepare query")
                .query_map(rusqlite::params!["zz-case-test"], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .expect("query rows")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect rows");
            assert_eq!(rows.len(), 1, "the case variants must share one row");
            assert_eq!(rows[0].0, "zz-case-test");
            // The canonical lower-case id wins, so its price is the one kept.
            assert_eq!(rows[0].1, "1");

            // The key is case-insensitive now: another spelling updates the
            // same row instead of adding a second one.
            conn.execute(
                "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million,
                     output_cost_per_million, cache_read_cost_per_million,
                     cache_creation_cost_per_million)
                 VALUES ('ZZ-CASE-TEST', 'ZZ Case Test', '5', '6', '0', '0')
                 ON CONFLICT(model_id) DO UPDATE SET
                     input_cost_per_million = excluded.input_cost_per_million",
                [],
            )
            .expect("insert a differently spelled id");
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM model_pricing WHERE model_id = ?1 COLLATE NOCASE",
                    rusqlite::params!["zz-case-test"],
                    |row| row.get(0),
                )
                .expect("count rows");
            assert_eq!(count, 1);
        }

        match previous {
            Some(value) => std::env::set_var("OBSERVATORY_TEST_HOME", value),
            None => std::env::remove_var("OBSERVATORY_TEST_HOME"),
        }
    }
}
