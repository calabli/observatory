// Storage behavior coverage migrated from the retired LAN aggregator.

struct TestDb {
    db: Database,
    read: rusqlite::Connection,
    _temp: tempfile::TempDir,
}
impl std::ops::Deref for TestDb {
    type Target = Database;
    fn deref(&self) -> &Database {
        &self.db
    }
}
fn test_db() -> TestDb {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("storage.db");
    let db = Database::open(&path).unwrap();
    let read = rusqlite::Connection::open(&path).unwrap();
    TestDb {
        db,
        read,
        _temp: temp,
    }
}
struct StoredTotals {
    total_requests: u64,
    total_input_tokens: u64,
    success_count: u64,
    total_cost: String,
}
// Check persisted storage invariants directly, independently of dashboard aggregation.
fn stored_totals(db: &TestDb, id: &str) -> rusqlite::Result<StoredTotals> {
    db.read.query_row("SELECT COALESCE(SUM(requests),0),COALESCE(SUM(input),0),COALESCE(SUM(success),0),CAST(COALESCE(SUM(cost),0) AS TEXT) FROM (
        SELECT 1 AS requests,input_tokens AS input,CASE WHEN status_code BETWEEN 200 AND 299 THEN 1 ELSE 0 END AS success,CAST(total_cost_usd AS REAL) AS cost FROM lan_usage_events WHERE device_id=?1
        UNION ALL SELECT request_count,input_tokens,success_count,CAST(total_cost_usd AS REAL) FROM lan_usage_daily WHERE device_id=?1
    )", [id], |row| Ok(StoredTotals {total_requests:row.get(0)?,total_input_tokens:row.get(1)?,success_count:row.get(2)?,total_cost:row.get(3)?}))
}

use observatory_usage_core::lan_usage::{DailyRollupRow, DeviceInfo, UsageEventRow};
use observatory_usage_core::{
    database::Database,
    lan_usage::{storage::*, UsageExport},
};
use rusqlite::params;

fn device(id: &str) -> DeviceInfo {
    DeviceInfo {
        device_id: id.to_string(),
        device_name: format!("device-{id}"),
        app_version: "4.0.19".to_string(),
        platform: "linux".to_string(),
        started_at: 1_700_000_000,
    }
}

fn event(id: &str, created_at: i64, input: i64) -> UsageEventRow {
    UsageEventRow {
        metadata: Default::default(),
        request_id: id.to_string(),
        data_source: "dsh_session".to_string(),
        app_type: "dsh".to_string(),
        model: "fixture".to_string(),
        session_id: Some("s1".to_string()),
        input_tokens: input,
        output_tokens: 10,
        cache_read_tokens: 5,
        cache_creation_tokens: 2,
        input_token_semantics: 2,
        input_cost_usd: "1.5".to_string(),
        output_cost_usd: "0.5".to_string(),
        cache_read_cost_usd: "0.25".to_string(),
        cache_creation_cost_usd: "0.125".to_string(),
        total_cost_usd: "2.375".to_string(),
        status_code: 200,
        latency_ms: 100,
        created_at,
    }
}

fn daily(date: &str, requests: i64, input: i64) -> DailyRollupRow {
    DailyRollupRow {
        metadata: Default::default(),
        date: date.to_string(),
        app_type: "dsh".to_string(),
        model: "fixture".to_string(),
        request_count: requests,
        success_count: requests,
        input_tokens: input,
        output_tokens: 10,
        cache_read_tokens: 5,
        cache_creation_tokens: 2,
        input_token_semantics: 2,
        total_cost_usd: "3".to_string(),
        avg_latency_ms: 100,
    }
}

/// 构造某个本地日期 12:00 的 unix 秒（按本机时区算，避免写死时刻）
fn ts_for(date: &str) -> i64 {
    use chrono::TimeZone;
    let naive = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap();
    chrono::Local
        .from_local_datetime(&naive)
        .single()
        .unwrap()
        .timestamp()
}

#[test]
fn stores_events_and_summarizes_per_device() {
    let db = test_db();
    register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
    let ts = ts_for("2024-05-01");
    let export = UsageExport {
        snapshot_version: 0,
        device: device("dev-a"),
        shared_app_types: None,
        events: vec![event("r1", ts, 100), event("r2", ts + 60, 200)],
        daily: vec![],
    };
    assert_eq!(store_export(&db, &export).unwrap(), 2);

    let summary = stored_totals(&db, "dev-a").unwrap();
    assert_eq!(summary.total_requests, 2);
    assert_eq!(summary.total_input_tokens, 300);
    assert_eq!(summary.success_count, 2);
    assert!((summary.total_cost.parse::<f64>().unwrap() - 4.75).abs() < 1e-9);
}

/// 对端把某天收进按天汇总表后，本地存的明细行必须作废，
/// 否则同一天会被算两遍。
#[test]
fn daily_rollup_replaces_detail_rows_for_the_same_day() {
    let db = test_db();
    register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
    let ts = ts_for("2024-05-01");

    let detail = UsageExport {
        snapshot_version: 0,
        device: device("dev-a"),
        shared_app_types: None,
        events: vec![event("r1", ts, 100), event("r2", ts + 60, 200)],
        daily: vec![],
    };
    store_export(&db, &detail).unwrap();
    assert_eq!(stored_totals(&db, "dev-a").unwrap().total_requests, 2);

    let rolled = UsageExport {
        snapshot_version: 0,
        device: device("dev-a"),
        shared_app_types: None,
        events: vec![],
        daily: vec![daily("2024-05-01", 7, 999)],
    };
    store_export(&db, &rolled).unwrap();

    let summary = stored_totals(&db, "dev-a").unwrap();
    // 汇总行接管了那一天：请求数是 7 而不是 2+7
    assert_eq!(summary.total_requests, 7);
    assert_eq!(summary.total_input_tokens, 999);
}

/// 反向：对端把历史重新导回成明细时，已有的按天汇总行为准。
#[test]
fn detail_rows_are_ignored_when_the_day_already_has_a_rollup() {
    let db = test_db();
    register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
    let ts = ts_for("2024-05-01");

    let rolled = UsageExport {
        snapshot_version: 0,
        device: device("dev-a"),
        shared_app_types: None,
        events: vec![],
        daily: vec![daily("2024-05-01", 7, 999)],
    };
    store_export(&db, &rolled).unwrap();

    let detail = UsageExport {
        snapshot_version: 0,
        device: device("dev-a"),
        shared_app_types: None,
        events: vec![event("r1", ts, 100)],
        daily: vec![],
    };
    store_export(&db, &detail).unwrap();

    let summary = stored_totals(&db, "dev-a").unwrap();
    assert_eq!(summary.total_requests, 7);
    assert_eq!(summary.total_input_tokens, 999);
}

#[test]
fn disconnecting_a_device_clears_its_rows() {
    let db = test_db();
    register_device(&db, "dev-a", "A", "203.0.113.2", 17890).unwrap();
    let ts = ts_for("2024-05-01");
    store_export(
        &db,
        &UsageExport {
            snapshot_version: 0,
            device: device("dev-a"),
            shared_app_types: None,
            events: vec![event("a1", ts, 100)],
            daily: vec![],
        },
    )
    .unwrap();
    remove_device(&db, "dev-a").unwrap();
    assert!(list_devices(&db).unwrap().is_empty());
    assert_eq!(stored_totals(&db, "dev-a").unwrap().total_requests, 0);
}

fn seed_local_usage(db: &TestDb) {
    let conn = &db.read;
    for app in observatory_usage_core::lan_usage::SHARED_APP_TYPES {
        // Include proxy-origin statistics as well as locally imported sessions.
        conn.execute(
            "INSERT INTO proxy_request_logs (request_id, provider_id, app_type, model, data_source, latency_ms, status_code, created_at, input_tokens)
             VALUES (?1, 'fixture', ?1, 'fixture', 'proxy', 100, 200, ?2, 100)",
            params![app, ts_for("2024-05-02")],
        ).unwrap();
        for provider in ["first", "second"] {
            conn.execute(
                "INSERT INTO usage_daily_rollups (date, app_type, provider_id, model, request_count, success_count, input_tokens, total_cost_usd)
                 VALUES ('2024-05-01', ?1, ?2, 'fixture', 2, 2, 200, '1.25')",
                params![app, provider],
            ).unwrap();
        }
    }
}

#[test]
fn export_filters_details_and_rollups_and_merges_provider_dimensions() {
    let db = test_db();
    seed_local_usage(&db);
    let all = export_local_usage(&db).unwrap();
    assert_eq!(
        all.events.len(),
        observatory_usage_core::lan_usage::SHARED_APP_TYPES.len()
    );
    assert_eq!(
        all.daily.len(),
        observatory_usage_core::lan_usage::SHARED_APP_TYPES.len() * 2
    );
    let mut settings = observatory_usage_core::lan_usage::identity::share_settings(&db);
    settings.shared_app_types = vec!["mcode".into()];
    observatory_usage_core::lan_usage::identity::save_share_settings(&db, &settings).unwrap();
    let selected = export_local_usage(&db).unwrap();
    assert_eq!(selected.shared_app_types, Some(vec!["mcode".into()]));
    assert_eq!(selected.events.len(), 1);
    assert_eq!(selected.events[0].app_type, "mcode");
    assert_eq!(selected.daily.len(), 2);
    assert_eq!(selected.daily[0].app_type, "mcode");
    assert_eq!(
        selected.daily.iter().map(|r| r.request_count).sum::<i64>(),
        4
    );
    assert_eq!(
        selected.daily.iter().map(|r| r.input_tokens).sum::<i64>(),
        400
    );
    assert_eq!(selected.daily[0].input_token_semantics, 2);
    assert_eq!(
        selected
            .daily
            .iter()
            .map(|r| r.total_cost_usd.parse::<f64>().unwrap())
            .sum::<f64>(),
        2.5
    );
    settings.shared_app_types.clear();
    observatory_usage_core::lan_usage::identity::save_share_settings(&db, &settings).unwrap();
    let none = export_local_usage(&db).unwrap();
    assert_eq!(none.shared_app_types, Some(vec![]));
    assert!(none.events.is_empty());
    assert!(none.daily.is_empty());
    // Selecting none changes exports, never deletes original local records.
    let conn = &db.read;
    let local: i64 = conn
        .query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(
        local as usize,
        observatory_usage_core::lan_usage::SHARED_APP_TYPES.len()
    );
}

#[test]
fn source_revocation_clears_only_that_peers_unselected_copies() {
    let db = test_db();
    register_device(&db, "a", "A", "203.0.113.2", 17890).unwrap();
    register_device(&db, "b", "B", "203.0.113.3", 17890).unwrap();
    let ts = ts_for("2024-05-02");
    let mut zcode = event("zcode", ts, 10);
    zcode.app_type = "zcode".into();
    zcode.data_source = "zcode_session".into();
    let mut export = UsageExport {
        snapshot_version: 0,
        device: device("a"),
        shared_app_types: None,
        events: vec![event("dsh", ts, 20), zcode],
        daily: vec![daily("2024-05-01", 3, 30)],
    };
    store_export(&db, &export).unwrap();
    export.device = device("b");
    store_export(&db, &export).unwrap();
    export.device = device("a");
    export.shared_app_types = Some(vec!["zcode".into()]);
    // Rows outside the declared selection are ignored even if present in the payload.
    store_export(&db, &export).unwrap();
    assert_eq!(stored_totals(&db, "a").unwrap().total_requests, 1);
    assert_eq!(stored_totals(&db, "b").unwrap().total_requests, 5);
    export.shared_app_types = Some(vec![]);
    store_export(&db, &export).unwrap();
    assert_eq!(stored_totals(&db, "a").unwrap().total_requests, 0);
    assert_eq!(stored_totals(&db, "b").unwrap().total_requests, 5);
}

#[test]
fn one_apps_rollup_does_not_remove_another_apps_same_day_details() {
    let db = test_db();
    register_device(&db, "a", "A", "203.0.113.2", 17890).unwrap();
    let ts = ts_for("2024-05-01");
    let mut zcode = event("zcode", ts, 10);
    zcode.app_type = "zcode".into();
    let mut export = UsageExport {
        snapshot_version: 0,
        device: device("a"),
        shared_app_types: None,
        events: vec![event("dsh", ts, 20), zcode.clone()],
        daily: vec![],
    };
    store_export(&db, &export).unwrap();
    export.events.clear();
    export.daily.push(daily("2024-05-01", 3, 30));
    store_export(&db, &export).unwrap();
    assert_eq!(stored_totals(&db, "a").unwrap().total_requests, 4);
    // A rollup for DSH must not suppress a new ZCode detail, either.
    zcode.request_id = "zcode-new".into();
    export.events.push(zcode);
    store_export(&db, &export).unwrap();
    assert_eq!(stored_totals(&db, "a").unwrap().total_requests, 5);
}

#[test]
fn legacy_export_without_scope_remains_readable() {
    let value = serde_json::json!({"device": device("old"), "events": [], "daily": []});
    let export: UsageExport = serde_json::from_value(value).unwrap();
    assert_eq!(export.shared_app_types, None);
}
