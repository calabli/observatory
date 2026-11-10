// Activity log: per-file read records of scans, key actions (sensitive arguments
// hidden), and external API calls.

use observatory_usage_core::{api::UsageBackend, database::Database, lan_usage::server};
use serde_json::{json, Value};
use std::sync::Arc;

fn assistant_line(id: &str, output: u32) -> String {
    format!(
        r#"{{"type":"assistant","message":{{"id":"{id}","model":"claude-opus-4-8","usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":100,"cache_creation_input_tokens":50}},"stop_reason":"end_turn"}},"timestamp":"2026-06-07T13:01:23Z","sessionId":"session-x"}}"#
    )
}

async fn entries(backend: &UsageBackend, kind: &str, at_least: usize) -> Vec<Value> {
    // The records are completed asynchronously on the blocking thread pool, so
    // give them a moment.
    for _ in 0..50 {
        let list = backend
            .call("activity_list", json!({"kind": kind}))
            .await
            .unwrap();
        let rows = list.as_array().cloned().unwrap_or_default();
        if rows.len() >= at_least {
            return rows;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("no {kind} activity recorded");
}

#[tokio::test]
async fn activity_log_records_files_actions_and_api_calls() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join(".claude/projects/demo");
    std::fs::create_dir_all(&project).unwrap();
    let file = project.join("session-x.jsonl");
    std::fs::write(
        &file,
        format!("{}\n{}\n", assistant_line("m1", 5), assistant_line("m2", 7)),
    )
    .unwrap();
    std::env::set_var("CLAUDE_CONFIG_DIR", temp.path().join(".claude"));
    let db = Arc::new(Database::open(temp.path().join("obs.db")).unwrap());
    db.set_setting("usage_enabled_sources", r#"["claude"]"#)
        .unwrap();
    let backend = UsageBackend::with_database(db.clone());

    // 1. Manual scan: one scan record whose per-file detail lists this session file.
    backend.call("scan_local_only", json!({})).await.unwrap();
    let scans = entries(&backend, "scan", 1).await;
    let scan = &scans[0];
    assert_eq!(scan["summary"]["trigger"], "手动");
    assert_eq!(scan["summary"]["newRows"], 2);
    let tokens = (10 + 5 + 100 + 50) + (10 + 7 + 100 + 50);
    assert_eq!(scan["summary"]["tokens"], tokens);
    let detail = backend
        .call("activity_detail", json!({"id": scan["id"]}))
        .await
        .unwrap();
    let files = detail["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{detail}");
    let entry = &files[0];
    assert!(entry["path"].as_str().unwrap().ends_with("session-x.jsonl"));
    let size = std::fs::metadata(&file).unwrap().len();
    assert_eq!(entry["sizeBytes"], size);
    assert_eq!(entry["bytesRead"], size);
    assert_eq!(entry["isNew"], true);
    assert_eq!(entry["rows"], 2);
    assert_eq!(entry["tokens"], tokens);
    assert_eq!(detail["sources"][0]["id"], "claude");

    // When a file has no new content the background scan should not produce a
    // record; a manual scan still records one entry (no file changes).
    backend.call("scan_local_only", json!({})).await.unwrap();
    let scans = entries(&backend, "scan", 2).await;
    assert_eq!(scans[0]["summary"]["filesChanged"], 0);

    // 2. Key actions: scanning the local machine is recorded; a failed connect
    //    action records the error, the pairing code is hidden, and writing the
    //    log itself does not advance the write epoch.
    let actions = entries(&backend, "action", 2).await;
    assert!(actions.iter().any(|a| a["title"] == "扫描本机"));
    let epoch = db.data_epoch();
    assert!(backend
        .call(
            "lan_connect",
            json!({"host": "127.0.0.1", "port": 1, "code": "123456"})
        )
        .await
        .is_err());
    let actions = entries(&backend, "action", 3).await;
    let connect = &actions[0];
    assert_eq!(connect["title"], "连接设备");
    assert_eq!(connect["status"], "error");
    assert_eq!(connect["summary"]["args"]["code"], "（已隐藏）");
    assert!(!connect.to_string().contains("123456"));
    assert_eq!(
        db.data_epoch(),
        epoch,
        "activity logging bumped the write epoch"
    );

    // Read-only queries are not recorded.
    backend.call("status", json!({})).await.unwrap();
    backend.call("dashboard", json!({})).await.unwrap();

    // 3. External API calls: requests made to the LAN service are recorded.
    let handle = server::start(db.clone(), 0).await.unwrap();
    let resp = reqwest::get(format!("http://127.0.0.1:{}/api/lan/info", handle.port))
        .await
        .unwrap();
    assert!(resp.status().is_success());
    let unauthorized = reqwest::get(format!("http://127.0.0.1:{}/api/lan/usage", handle.port))
        .await
        .unwrap();
    assert_eq!(unauthorized.status(), 403);
    let calls = entries(&backend, "api_in", 2).await;
    let info = calls
        .iter()
        .find(|c| c["summary"]["path"] == "/api/lan/info")
        .unwrap();
    assert_eq!(info["status"], "ok");
    assert_eq!(info["summary"]["caller"], "未认证");
    assert_eq!(info["summary"]["httpStatus"], 200);
    let denied = calls
        .iter()
        .find(|c| c["summary"]["path"] == "/api/lan/usage")
        .unwrap();
    assert_eq!(denied["status"], "error");
    assert_eq!(denied["summary"]["httpStatus"], 403);
    handle.stop();

    // 4. All kinds are listed together, newest first.
    let all = backend
        .call("activity_list", json!({"limit": 100}))
        .await
        .unwrap();
    let ids: Vec<i64> = all
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["id"].as_i64().unwrap())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] > pair[1]));
}
