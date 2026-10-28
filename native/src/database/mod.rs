use crate::error::AppError;
use rusqlite::{Connection, OptionalExtension};
use std::{path::Path, sync::Mutex, time::Duration};
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

pub struct Database {
    pub(crate) conn: Mutex<Connection>,
    pub(crate) log_count_cache: Mutex<Option<crate::services::usage_stats::LogCountCache>>,
}
impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent).map_err(|e| AppError::io(parent, e))?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
        )?;
        Self::from_connection(conn)
    }
    pub fn memory() -> Result<Self, AppError> {
        Self::from_connection(Connection::open_in_memory()?)
    }
    fn from_connection(conn: Connection) -> Result<Self, AppError> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > 1 {
            return Err(AppError::Database(format!(
                "观测台数据库版本 {version} 高于当前支持版本，请升级应用。"
            )));
        }
        conn.execute_batch(include_str!("schema.sql"))?;
        Self::create_lan_usage_tables(&conn)?;
        let db = Self {
            conn: Mutex::new(conn),
            log_count_cache: Mutex::new(None),
        };
        db.ensure_model_pricing_seeded()?;
        lock_conn!(db.conn).execute_batch("PRAGMA user_version=1;")?;
        Ok(db)
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
}
