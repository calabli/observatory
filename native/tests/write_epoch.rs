// The write epoch only advances with writes that affect the aggregated
// statistics; the statistics queries go through a separate read-only connection.

use observatory_usage_core::database::Database;
use observatory_usage_core::error::AppError;
use observatory_usage_core::lan_usage::storage;

fn open() -> (Database, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(temp.path().join("epoch.db")).unwrap();
    (db, temp)
}

fn insert_log(db: &Database, id: &str) {
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, latency_ms, status_code, created_at)
             VALUES (?1, 'p', 'claude', 'm', 0, 200, 1)",
            [id],
        )?;
        Ok::<_, AppError>(())
    })
    .unwrap();
}

fn count_via_reader(db: &Database) -> i64 {
    db.with_read_connection(|conn| {
        Ok::<_, AppError>(
            conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| r.get(0))?,
        )
    })
    .unwrap()
}

#[test]
fn bookkeeping_writes_do_not_advance_epoch() {
    let (db, _temp) = open();
    storage::register_device(&db, "peer", "Peer", "127.0.0.1", 17891).unwrap();

    let before = db.data_epoch();
    storage::record_sync_result(&db, "peer", 3, None).unwrap();
    storage::record_sync_result(&db, "peer", 0, Some("timeout")).unwrap();
    db.set_setting_quietly("lan_last_snapshot_epoch:peer", "42")
        .unwrap();
    db.with_connection(|conn| {
        conn.execute(
            "INSERT INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
             VALUES ('f', 1, 0, 1)",
            [],
        )?;
        Ok::<_, AppError>(())
    })
    .unwrap();
    assert_eq!(db.data_epoch(), before, "记账写入不应推进写入代数");

    // The bookkeeping fields really landed in the database.
    let error: Option<String> = db
        .with_connection(|conn| {
            Ok::<_, AppError>(conn.query_row(
                "SELECT last_error FROM lan_devices WHERE device_id = 'peer'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(error.as_deref(), Some("timeout"));
    assert_eq!(
        db.get_setting("lan_last_snapshot_epoch:peer")
            .unwrap()
            .as_deref(),
        Some("42")
    );
}

#[test]
fn data_writes_advance_epoch_and_quiet_flag_resets() {
    let (db, _temp) = open();
    let start = db.data_epoch();
    insert_log(&db, "a");
    let after_insert = db.data_epoch();
    assert!(after_insert > start, "明细写入必须推进代数");

    // After a bookkeeping write fails the quiet flag has to be reset as well,
    // so later data writes are counted as usual.
    let failed = db.write_quietly(|_| Err::<(), _>(AppError::Message("boom".into())));
    assert!(failed.is_err());
    insert_log(&db, "b");
    assert!(db.data_epoch() > after_insert);

    db.set_setting("usage_refresh_interval", "60").unwrap();
    assert!(db.data_epoch() > after_insert, "普通设置写入仍计入代数");

    // A DELETE without a WHERE clause takes the truncate optimization and does
    // not trigger the row-level hook, so the commit hook has to catch it.
    let before_truncate = db.data_epoch();
    db.execute_batch("DELETE FROM proxy_request_logs;").unwrap();
    assert!(db.data_epoch() > before_truncate, "清空明细必须推进代数");
}

#[test]
fn read_connection_sees_committed_writes() {
    let (db, _temp) = open();
    assert_eq!(count_via_reader(&db), 0);
    insert_log(&db, "a");
    insert_log(&db, "b");
    assert_eq!(count_via_reader(&db), 2);

    // The read connection is read-only: using a write statement by mistake
    // raises an error instead of silently modifying the database.
    let write = db.with_read_connection(|conn| {
        conn.execute("DELETE FROM proxy_request_logs WHERE request_id = 'a'", [])
            .map_err(AppError::from)
    });
    assert!(write.is_err());
    assert_eq!(count_via_reader(&db), 2);
}

#[test]
fn reader_does_not_wait_for_writer_lock() {
    let (db, _temp) = open();
    insert_log(&db, "a");
    let db = std::sync::Arc::new(db);
    // While the write connection is held for a long time (simulating a big scan
    // transaction), the read connection still returns results.
    let count = db
        .with_connection(|_writer_held| {
            let db = db.clone();
            Ok::<_, AppError>(
                std::thread::spawn(move || count_via_reader(&db))
                    .join()
                    .unwrap(),
            )
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn dashboard_result_cache_follows_data_changes() {
    use observatory_usage_core::api::UsageBackend;
    let (db, _temp) = open();
    insert_log(&db, "a");
    let backend = UsageBackend::with_database(std::sync::Arc::new(db));
    let total = |v: serde_json::Value| v["summary"]["totalRequests"].as_u64().unwrap();
    let args = serde_json::json!({"page": 1, "pageSize": 20});

    let first = backend.call("dashboard", args.clone()).await.unwrap();
    let repeat = backend.call("dashboard", args.clone()).await.unwrap();
    assert_eq!(
        first, repeat,
        "unchanged data must return the same (cached) result"
    );
    assert_eq!(total(first), 1);

    // After new detail rows are written the old result must no longer be hit.
    insert_log(&backend.db, "b");
    assert_eq!(
        total(backend.call("dashboard", args.clone()).await.unwrap()),
        2
    );
    let tray = serde_json::json!({});
    assert_eq!(
        total(backend.call("tray_dashboard", tray.clone()).await.unwrap()),
        2
    );
    insert_log(&backend.db, "c");
    assert_eq!(
        total(backend.call("tray_dashboard", tray).await.unwrap()),
        3
    );
}
