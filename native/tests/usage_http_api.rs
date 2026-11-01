use chrono::{Local, TimeZone};
use observatory_usage_core::{
    database::Database,
    lan_usage::{identity, public_api, server},
};
use serde_json::{json, Value};
use std::sync::Arc;

fn seed(db: &Database) {
    let today = Local::now().date_naive();
    let ts = Local
        .from_local_datetime(&today.and_hms_opt(12, 0, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp();
    db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('claude','p','claude','m',100,20,300,10,2,'1.25',1000,200,{ts},'fixture'),('desktop','p','claude-desktop','m',900,0,0,0,2,'9',1000,200,{ts},'fixture'),('codex','p','codex','m',500,0,0,0,2,'5',1000,200,{ts},'fixture');")).unwrap();
    let yesterday = today.pred_opt().unwrap();
    db.execute_batch(&format!("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,output_tokens,input_token_semantics,total_cost_usd) VALUES ('{yesterday}','claude','p','m',2,2,50,5,2,'0.50');")).unwrap();
}
fn query(days: u32, end: Option<String>, app: Option<&str>) -> public_api::SummaryQuery {
    public_api::SummaryQuery {
        days: Some(days),
        end_date: end,
        app_type: app.map(str::to_owned),
    }
}
#[test]
fn calendar_days_numeric_totals_and_exact_app_filter() {
    let db = Database::memory().unwrap();
    seed(&db);
    let one = public_api::summary(&db, query(1, None, Some("claude"))).unwrap();
    assert_eq!(one["requests"], 1);
    assert_eq!(one["totalTokens"], 430);
    assert_eq!(one["costUsd"], 1.25);
    assert_eq!(one["daily"].as_array().unwrap().len(), 1);
    let seven = public_api::summary(&db, query(7, None, Some("claude"))).unwrap();
    assert_eq!(seven["requests"], 3);
    assert_eq!(seven["totalTokens"], 485);
    assert_eq!(seven["costUsd"], 1.75);
    assert_eq!(seven["daily"].as_array().unwrap().len(), 7);
    assert_eq!(seven["daily"][0]["requests"], 0);
    assert_eq!(
        public_api::summary(&db, query(7, None, None)).unwrap()["costUsd"],
        15.75
    );
    let yesterday = Local::now().date_naive().pred_opt().unwrap().to_string();
    let historic = public_api::summary(&db, query(1, Some(yesterday), Some("claude"))).unwrap();
    assert_eq!(historic["requests"], 2);
    assert_eq!(historic["costUsd"], 0.5);
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();
    assert!(public_api::summary(&db, query(7, None, Some("claude"))).is_err());
    let all = public_api::summary(&db, query(7, None, None)).unwrap();
    assert_eq!(all["costUsd"], 5.0);
    for days in [0, 367] {
        assert!(public_api::summary(&db, query(days, None, None)).is_err());
    }
    assert!(public_api::summary(&db, query(1, Some("bad".into()), None)).is_err());
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec![];
    identity::save_share_settings(&db, &settings).unwrap();
    let empty = public_api::summary(&db, query(7, None, None)).unwrap();
    assert_eq!(empty["costUsd"], 0.0);
    assert_eq!(empty["requests"], 0);
}

#[tokio::test]
async fn http_auth_validation_revocation_and_read_only_key() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    assert_eq!(http.get(&url).send().await.unwrap().status(), 401);
    let key = public_api::rotate_key(&db).unwrap();
    let response = http
        .get(format!("{url}?days=7&appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(response.headers()["cache-control"], "no-store");
    let result: Value = response.json().await.unwrap();
    assert_eq!(result["costUsd"], 1.75);
    for params in [
        "days=0",
        "days=999",
        "days=abc",
        "appType=missing",
        "endDate=2999-01-01",
        "unknown=1",
    ] {
        let response = http
            .get(format!("{url}?{params}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{params}");
        assert!(response.json::<Value>().await.unwrap()["error"].is_string());
    }
    let scan = http
        .post(format!("http://127.0.0.1:{}/api/lan/scan", handle.port))
        .header("X-API-Key", &key)
        .json(&json!({"key":"fixture"}))
        .send()
        .await
        .unwrap();
    assert_eq!(scan.status(), 403);
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();
    assert_eq!(
        http.get(format!("{url}?appType=claude"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    public_api::revoke_key(&db).unwrap();
    assert_eq!(
        http.get(&url)
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    handle.stop();
}
