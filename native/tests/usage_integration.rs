use observatory_usage_core::{api::UsageBackend, database::Database, lan_usage, services};
use rusqlite::Connection;
use serde_json::json;
use chrono::{Local, TimeZone};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

#[test]
fn tray_summary_counts_only_local_today() {
    let _guard = isolate();
    let temp = tempfile::tempdir().unwrap();
    let old_home = std::env::var_os("OBSERVATORY_HOME");
    let old_cc = std::env::var_os("CC_SWITCH_DB");
    std::env::set_var("OBSERVATORY_HOME", temp.path());
    std::env::set_var("CC_SWITCH_DB", temp.path().join("no-source.db"));
    let backend = UsageBackend::open().unwrap();
    let now = chrono::Local::now();
    let today = now.timestamp();
    let prior = now.date_naive().pred_opt().unwrap().and_hms_opt(12, 0, 0).unwrap();
    let yesterday = Local.from_local_datetime(&prior).earliest().unwrap().timestamp();
    backend.db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,cache_read_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('today','p','codex','m',100,20,10,2,'1.25',1000,200,{today},'fixture'), ('yesterday','p','codex','m',500,50,0,2,'9.00',1000,200,{yesterday},'fixture');")).unwrap();
    let result = backend.today_summary().unwrap();
    assert_eq!(result.total_requests, 1);
    assert_eq!(result.real_total_tokens, 130);
    assert_eq!(result.total_cost, "1.250000");
    drop(backend);
    match old_home { Some(v) => std::env::set_var("OBSERVATORY_HOME", v), None => std::env::remove_var("OBSERVATORY_HOME") }
    match old_cc { Some(v) => std::env::set_var("CC_SWITCH_DB", v), None => std::env::remove_var("CC_SWITCH_DB") }
}

#[test]
fn international_ip_response_requires_valid_ipv4() {
    use observatory_usage_core::network_overview::parse_ipv4_response;
    assert_eq!(parse_ipv4_response(&json!({"ip":"192.0.2.7"})).unwrap().to_string(), "192.0.2.7");
    assert!(parse_ipv4_response(&json!({"ip":"2001:db8::1"})).is_err());
    assert!(parse_ipv4_response(&json!({"ip":"not-an-ip"})).is_err());
    assert!(parse_ipv4_response(&json!({"error":"rate limited"})).is_err());
}

#[tokio::test]
#[ignore = "requires a real network request to ipify"]
async fn international_ip_live_smoke() {
    let result = observatory_usage_core::network_overview::international_ipv4().await.unwrap();
    assert!(result["ip"].as_str().unwrap().parse::<std::net::Ipv4Addr>().is_ok());
    assert!(result["checkedAt"].as_i64().unwrap() > 0);
}

#[test]
fn network_snapshot_exposes_valid_local_interface_data() {
    let snapshot = observatory_usage_core::network_overview::snapshot(true);
    let interfaces = snapshot["interfaces"].as_array().expect("interface list");
    eprintln!("Network snapshot: {} interfaces, {} default route, {} with counters", interfaces.len(), interfaces.iter().filter(|i| i["defaultRoute"] == true).count(), interfaces.iter().filter(|i| i["received"].is_u64() && i["transmitted"].is_u64()).count());
    for interface in interfaces {
        assert!(interface["id"].as_str().is_some_and(|s| !s.is_empty()));
        assert!(interface["state"].is_string());
        assert!(interface["defaultRoute"].is_boolean());
        for key in ["ipv4", "ipv6", "gateways", "dns"] { assert!(interface[key].is_array()); }
        for key in ["received", "transmitted"] { assert!(interface[key].is_null() || interface[key].is_u64()); }
    }
}

fn isolate() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    static HOME: OnceLock<std::path::PathBuf> = OnceLock::new();
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let home = HOME.get_or_init(|| tempfile::tempdir().unwrap().keep());
    std::env::set_var("OBSERVATORY_HOME", home);
    std::env::set_var("OBSERVATORY_TEST_HOME", home);
    guard
}

#[test]
fn daily_rollups_require_the_entire_end_day_to_be_selected() {
    let _guard = isolate();
    let db = Database::memory().unwrap();
    db.execute_batch("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,output_tokens,total_cost_usd) VALUES('2024-01-02','codex','provider','fixture-model',3,3,90,20,'2.50');").unwrap();
    let start = Local.with_ymd_and_hms(2024, 1, 2, 0, 0, 0).single().unwrap().timestamp();
    let partial = Local.with_ymd_and_hms(2024, 1, 2, 23, 59, 58).single().unwrap().timestamp();
    let full = Local.with_ymd_and_hms(2024, 1, 2, 23, 59, 59).single().unwrap().timestamp();
    assert_eq!(db.get_usage_summary(Some(start), Some(partial), None, None, None).unwrap().total_requests, 0);
    assert_eq!(db.get_usage_summary(Some(start), Some(full), None, None, None).unwrap().total_requests, 3);
}
fn seed(
    db: &Database,
    id: &str,
    app: &str,
    cost: &str,
    input: i64,
    output: i64,
    cache: i64,
    semantics: i64,
) {
    db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,pricing_model,input_tokens,output_tokens,cache_read_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES('{id}','provider','{app}','fixture-model','fixture-model',{input},{output},{cache},{semantics},'{cost}',1200,200,1793000000,'fixture');")).unwrap();
}
#[test]
fn source_import_is_read_only_idempotent_and_excludes_credentials() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cc-switch.db");
    {
        let source = Database::open(&path).unwrap();
        seed(&source, "fixture-1", "codex", "1.25", 100, 10, 60, 1);
        source.execute_batch("INSERT INTO providers(id,app_type,name,settings_config) VALUES('provider','codex','Display only','{\"api_key\":\"SENTINEL_NOT_IMPORTED\"}'); INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,output_tokens,input_token_semantics,total_cost_usd) VALUES('2026-10-01','codex','provider','fixture-model',3,3,90,20,2,'2.50');").unwrap();
    }
    let before = std::fs::read(&path).unwrap();
    let target = Database::memory().unwrap();
    assert_eq!(target.import_cc_switch(&path, None).unwrap().requests, 1);
    assert_eq!(target.import_cc_switch(&path, None).unwrap().requests, 0);
    let summary = target
        .get_usage_summary(None, None, None, None, None)
        .unwrap();
    assert_eq!(summary.total_requests, 4);
    assert_eq!(summary.total_cost, "3.750000");
    assert_eq!(summary.total_input_tokens, 130);
    assert_eq!(summary.real_total_tokens, 220);
    assert_eq!(
        target
            .get_provider_stats(None, None, None, None, None)
            .unwrap()[0]
            .provider_name,
        "Display only"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // Source pruning moves an already imported detail into a new daily rollup.
    // The destination retains its detail and must not add the new rollup again.
    {
        let source = Database::open(&path).unwrap();
        source.execute_batch("DELETE FROM proxy_request_logs; INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,output_tokens,cache_read_tokens,input_token_semantics,total_cost_usd) VALUES('2026-10-26','codex','provider','fixture-model',1,1,40,10,60,2,'1.25');").unwrap();
    }
    let repeated = target.import_cc_switch(&path, None).unwrap();
    assert_eq!(repeated.rollups, 0);
    assert_eq!(
        target
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_cost,
        "3.750000"
    );
    let out = dir.path().join("target.db");
    let disk = Database::open(&out).unwrap();
    disk.import_cc_switch(&path, None).unwrap();
    drop(disk);
    let conn = Connection::open(out).unwrap();
    let config: String = conn
        .query_row("SELECT settings_config FROM providers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(config, "{}");
}
#[test]
fn ambiguous_rollup_import_into_existing_native_history_is_rejected_atomically() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source.db");
    {
        let db = Database::open(&source).unwrap();
        db.execute_batch("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,input_token_semantics,total_cost_usd) VALUES('2026-10-01','codex','provider','fixture-model',10,10,100,2,'2');").unwrap();
    }
    let target = Database::memory().unwrap();
    seed(&target, "already-native", "codex", "1", 10, 2, 0, 2);
    assert!(target.import_cc_switch(&source, None).is_err());
    assert_eq!(
        target
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
}
#[test]
fn filters_and_cache_semantics_match_native_queries() {
    let _guard = isolate();
    let db = Database::memory().unwrap();
    seed(&db, "codex-1", "codex", "1", 100, 10, 60, 1);
    seed(&db, "dsh-1", "dsh", "2", 80, 12, 20, 2);
    let all = db.get_usage_summary(None, None, None, None, None).unwrap();
    assert_eq!(all.total_requests, 2);
    assert_eq!(all.total_input_tokens, 120);
    assert_eq!(all.real_total_tokens, 222);
    assert!((all.cache_hit_rate - 0.4).abs() < 0.00001);
    assert_eq!(
        db.get_usage_summary(None, None, Some("dsh"), None, None)
            .unwrap()
            .total_requests,
        1
    );
    assert_eq!(
        db.get_usage_summary(Some(1793000001), None, None, None, None)
            .unwrap()
            .total_requests,
        0
    );
    assert_eq!(
        db.get_model_stats(None, None, None, None, Some("fixture-model"))
            .unwrap()[0]
            .request_count,
        2
    );
    let logs = db.get_request_logs(&Default::default(), 1, 1).unwrap();
    assert_eq!(logs.total, 2);
    assert_eq!(logs.data.len(), 1);
}
#[test]
fn dsh_concatenated_frames_and_persistent_dedup() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    std::env::set_var("DSH_HOME", dir.path());
    let sessions = dir.path().join("sessions/project");
    std::fs::create_dir_all(&sessions).unwrap();
    let path = sessions.join("session.v4.jsonl.zstd");
    let header = format!(
        "{}\n",
        json!({"type":"session","version":4,"id":"fixture-session","isSeeded":false})
    );
    let event = |seq| {
        format!(
            "{}\n",
            json!({"type":"assistant/message","seq":seq,"time":1792509550806i64,"data":{"message":{"source":{"model":"fixture-unpriced"}},"usage":{"inputTokens":80,"outputTokens":12,"cacheReadTokens":20,"cacheWriteTokens":5}}})
        )
    };
    let mut bytes = zstd::stream::encode_all(header.as_bytes(), 3).unwrap();
    bytes.extend(zstd::stream::encode_all(event(1).as_bytes(), 3).unwrap());
    std::fs::write(&path, &bytes).unwrap();
    let db = Database::memory().unwrap();
    assert_eq!(
        services::session_usage_dsh::sync_dsh_usage(&db)
            .unwrap()
            .imported,
        1
    );
    assert_eq!(
        services::session_usage_dsh::sync_dsh_usage(&db)
            .unwrap()
            .imported,
        0
    );
    bytes.extend(zstd::stream::encode_all(event(2).as_bytes(), 3).unwrap());
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(
        services::session_usage_dsh::sync_dsh_usage(&db)
            .unwrap()
            .imported,
        1
    );
    let summary = db.get_usage_summary(None, None, None, None, None).unwrap();
    assert_eq!(summary.real_total_tokens, 234);
    assert_eq!(summary.total_input_tokens, 160);
    db.execute_batch("DELETE FROM proxy_request_logs; DELETE FROM session_log_sync;")
        .unwrap();
    assert_eq!(
        services::session_usage_dsh::sync_dsh_usage(&db)
            .unwrap()
            .imported,
        0
    );
}
#[test]
fn zcode_retains_cache_timing_and_never_resurrects_pruned_rows() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.sqlite");
    std::env::set_var("ZCODE_DB", &path);
    let source = Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE model_usage(id TEXT,session_id TEXT,model_id TEXT,status TEXT,started_at INTEGER,completed_at INTEGER,duration_ms INTEGER,time_to_first_token_ms INTEGER,input_tokens INTEGER,output_tokens INTEGER,reasoning_tokens INTEGER,cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER,provider_total_tokens INTEGER,computed_total_tokens INTEGER); INSERT INTO model_usage VALUES('fixture','s','fixture-unpriced','completed',1792767776968,1792767779426,2458,1947,27528,37,0,22080,0,27565,27565);").unwrap();
    drop(source);
    let before = std::fs::read(&path).unwrap();
    let db = Database::memory().unwrap();
    assert_eq!(
        services::session_usage_zcode::sync_zcode_usage(&db)
            .unwrap()
            .imported,
        1
    );
    assert_eq!(
        services::session_usage_zcode::sync_zcode_usage(&db)
            .unwrap()
            .imported,
        0
    );
    let logs = db.get_request_logs(&Default::default(), 0, 20).unwrap();
    let r = &logs.data[0];
    assert_eq!(r.input_tokens, 5448);
    assert_eq!(r.cache_read_tokens, 22080);
    assert_eq!(r.output_tokens, 37);
    assert_eq!(r.first_token_ms, Some(1947));
    assert_eq!(r.duration_ms, Some(2458));
    db.execute_batch("DELETE FROM proxy_request_logs;").unwrap();
    assert_eq!(
        services::session_usage_zcode::sync_zcode_usage(&db)
            .unwrap()
            .imported,
        0
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[tokio::test]
async fn peer_snapshots_support_filters_replace_and_disabled_devices() {
    let _guard = isolate();
    let local = Arc::new(Database::memory().unwrap());
    let peer = Arc::new(Database::memory().unwrap());
    seed(&local, "same-request", "dsh", "1", 10, 2, 0, 2);
    seed(&peer, "same-request", "dsh", "2", 20, 4, 0, 2);
    let server = lan_usage::server::start(peer.clone(), 0).await.unwrap();
    let local_server = lan_usage::server::start(local.clone(), 0).await.unwrap();
    let info = lan_usage::identity::device_info(&peer).unwrap();
    let own_info = lan_usage::identity::device_info(&local).unwrap();
    let (code, _) = lan_usage::pairing::generate(&peer).unwrap();
    let reverse = lan_usage::pairing::new_token();
    lan_usage::pairing::store_grant(&local, &info.device_id, &reverse).unwrap();
    let token = lan_usage::client::pair("127.0.0.1", server.port, &own_info, local_server.port, &code, &reverse).await.unwrap();
    local.set_setting(&format!("lan_outgoing_token_v1:{}", info.device_id), &token).unwrap();
    lan_usage::storage::register_device(&local, &info.device_id, "Peer", "127.0.0.1", server.port)
        .unwrap();
    lan_usage::client::sync_device(&local, &info.device_id)
        .await
        .unwrap();
    let peer_only = lan_usage::projection::database_for_peer(&local, &info.device_id).unwrap();
    let peer_summary = peer_only.get_usage_summary(None, None, None, None, None).unwrap();
    assert_eq!(peer_summary.total_requests, 1);
    assert_eq!(peer_summary.total_cost, "2.000000");
    let local_summary = local.get_usage_summary(None, None, None, None, None).unwrap();
    assert_eq!(local_summary.total_requests, 1);
    assert_eq!(local_summary.total_cost, "1.000000");
    let tray_backend = UsageBackend::with_database(local.clone());
    let all = tray_backend.call("tray_dashboard", json!({"startDate":1792999900,"endDate":1793000100,"multiDevice":true})).await.unwrap();
    let only_peer = tray_backend.call("tray_dashboard", json!({"startDate":1792999900,"endDate":1793000100,"deviceId":info.device_id})).await.unwrap();
    assert_eq!(all["summary"]["totalRequests"], 2);
    assert_eq!(only_peer["summary"]["totalRequests"], 1);
    assert_eq!(only_peer["summary"]["totalCost"], "2.000000");
    local.execute_batch(&format!("UPDATE lan_devices SET last_sync_at = NULL WHERE device_id = '{}';", info.device_id)).unwrap();
    assert_eq!(lan_usage::projection::database_for_peer(&local, &info.device_id).unwrap().get_usage_summary(None, None, None, None, None).unwrap().total_requests, 1);
    let cached_total = lan_usage::projection::database(&local).unwrap();
    let cached_peer = lan_usage::projection::database_for_peer(&local, &info.device_id).unwrap();
    assert!(Arc::ptr_eq(&cached_total, &lan_usage::projection::database(&local).unwrap()));
    assert!(Arc::ptr_eq(&cached_peer, &lan_usage::projection::database_for_peer(&local, &info.device_id).unwrap()));
    assert_eq!(
        lan_usage::projection::database(&local)
            .unwrap()
            .get_usage_summary(None, None, Some("dsh"), None, Some("fixture-model"))
            .unwrap()
            .total_requests,
        2
    );
    lan_usage::client::sync_device(&local, &info.device_id)
        .await
        .unwrap();
    assert_eq!(
        lan_usage::projection::database(&local)
            .unwrap()
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        2
    );
    lan_usage::storage::set_device_enabled(&local, &info.device_id, false).unwrap();
    assert!(lan_usage::projection::database_for_peer(&local, &info.device_id).is_err());
    assert_eq!(
        lan_usage::projection::database(&local)
            .unwrap()
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
    lan_usage::storage::set_device_enabled(&local, &info.device_id, true).unwrap();
    peer.execute_batch("DELETE FROM proxy_request_logs;")
        .unwrap();
    lan_usage::client::sync_device(&local, &info.device_id)
        .await
        .unwrap();
    assert_eq!(
        lan_usage::projection::database(&local)
            .unwrap()
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
    server.stop();
    local_server.stop();
}
#[tokio::test]
async fn zero_cost_backfill_and_source_preferences_persist() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("observatory.db");
    let db = Arc::new(Database::open(&path).unwrap());
    seed(&db, "cost-fixture", "dsh", "0", 1000, 500, 100, 2);
    let backend = UsageBackend::with_database(db.clone());
    backend.call("pricing_update",json!({"modelId":"fixture-model","displayName":"Fixture","inputCostPerMillion":"2","outputCostPerMillion":"4","cacheReadCostPerMillion":"0.5","cacheCreationCostPerMillion":"0"})).await.unwrap();
    assert_eq!(
        db.get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_cost,
        "0.004050"
    );
    assert!(backend.call("pricing_update",json!({"modelId":"negative","displayName":"Negative","inputCostPerMillion":"-1","outputCostPerMillion":"1","cacheReadCostPerMillion":"0","cacheCreationCostPerMillion":"0"})).await.is_err());
    backend
        .call("sources_save", json!({"enabled":[]}))
        .await
        .unwrap();
    backend
        .call("preferences_save", json!({"refreshInterval":"manual"}))
        .await
        .unwrap();
    drop(backend);
    drop(db);
    let reopened = UsageBackend::with_database(Arc::new(Database::open(path).unwrap()));
    let status = reopened.call("status", json!({})).await.unwrap();
    assert_eq!(status["refreshInterval"], "manual");
    assert!(status["sources"]
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["enabled"] == false));
}
#[tokio::test]
async fn api_first_page_is_not_skipped_and_pages_do_not_overlap() {
    let _guard = isolate();
    let db = Arc::new(Database::memory().unwrap());
    for i in 0..25 {
        seed(&db, &format!("page-{i:02}"), "dsh", "1", 10, 2, 0, 2);
    }
    db.execute_batch("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,input_token_semantics,total_cost_usd) VALUES('2026-06-01','dsh','provider','fixture-model',5,5,100,2,'0.5');").unwrap();
    let backend = UsageBackend::with_database(db);
    let first = backend
        .call("dashboard", json!({"page":1,"pageSize":20}))
        .await
        .unwrap();
    let second = backend
        .call("dashboard", json!({"page":2,"pageSize":20}))
        .await
        .unwrap();
    let a = first["logs"]["data"].as_array().unwrap();
    let b = second["logs"]["data"].as_array().unwrap();
    assert_eq!(a.len(), 20);
    assert_eq!(b.len(), 5);
    assert_eq!(first["logs"]["page"], 1);
    assert!(
        first["trends"]
            .as_array()
            .unwrap()
            .iter()
            .any(|day| day["date"].as_str().unwrap().starts_with("2026-06-01")),
        "All-time chart must include the first historical day"
    );
    let mut ids = std::collections::HashSet::new();
    for row in a.iter().chain(b) {
        assert!(ids.insert(row["requestId"].as_str().unwrap().to_string()));
    }
    assert_eq!(ids.len(), 25);
}

#[test]
fn first_open_imports_history_before_any_native_scan() {
    let _guard = isolate();
    let source_dir = tempfile::tempdir().unwrap();
    let source_path = source_dir.path().join("cc-switch.db");
    {
        let source = Database::open(&source_path).unwrap();
        seed(&source, "startup-history", "dsh", "1.25", 100, 10, 0, 2);
    }
    let previous = std::env::var_os("CC_SWITCH_DB");
    std::env::set_var("CC_SWITCH_DB", &source_path);
    let backend = UsageBackend::open().unwrap();
    assert_eq!(
        backend
            .db
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
    assert!(backend
        .db
        .get_setting("cc_switch_import_complete")
        .unwrap()
        .is_some());
    drop(backend);
    let reopened = UsageBackend::open().unwrap();
    assert_eq!(
        reopened
            .db
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
    drop(reopened);
    match previous {
        Some(value) => std::env::set_var("CC_SWITCH_DB", value),
        None => std::env::remove_var("CC_SWITCH_DB"),
    }
}

#[tokio::test]
async fn history_import_preserves_existing_observatory_custom_prices() {
    let _guard = isolate();
    let target = Arc::new(Database::memory().unwrap());
    let backend = UsageBackend::with_database(target.clone());
    backend.call("pricing_update",json!({"modelId":"observatory-custom-priority","displayName":"Own price","inputCostPerMillion":"2","outputCostPerMillion":"4","cacheReadCostPerMillion":"0.2","cacheCreationCostPerMillion":"0"})).await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source_path = dir.path().join("cc-switch.db");
    drop(Database::open(&source_path).unwrap());
    std::fs::write(dir.path().join("model-pricing.json"),json!({"models":[{"modelId":"observatory-custom-priority","displayName":"Source price","inputCostPerMillion":"9","outputCostPerMillion":"9","cacheReadCostPerMillion":"9","cacheCreationCostPerMillion":"9"}]}).to_string()).unwrap();
    target.import_cc_switch(&source_path, None).unwrap();
    let prices = backend.call("pricing_list", json!({})).await.unwrap();
    let own = prices
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["modelId"] == "observatory-custom-priority")
        .unwrap();
    assert_eq!(own["inputCostPerMillion"], "2");
    assert_eq!(own["displayName"], "Own price");
}

#[test]
fn incompatible_source_schema_does_not_silently_drop_rows_or_change_target() {
    let _guard = isolate();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("incompatible.db");
    let source = Connection::open(&path).unwrap();
    source.execute_batch("CREATE TABLE proxy_request_logs(request_id TEXT); CREATE TABLE usage_daily_rollups(date TEXT);").unwrap();
    drop(source);
    let target = Database::memory().unwrap();
    seed(&target, "preserved", "dsh", "1", 10, 2, 0, 2);
    assert!(target.import_cc_switch(&path, None).is_err());
    assert_eq!(
        target
            .get_usage_summary(None, None, None, None, None)
            .unwrap()
            .total_requests,
        1
    );
}
