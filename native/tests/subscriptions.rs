use observatory_usage_core::{
    api::UsageBackend,
    database::Database,
    lan_usage::identity,
    subscriptions::{weekly_cycle, Subscription, WeeklyRule},
};
use chrono::{DateTime, Local, TimeZone};
use serde_json::json;
use std::sync::Arc;

/// 构造一条规则。
fn rule(start_day: u8, start_time: &str, end_day: u8, end_time: &str) -> WeeklyRule {
    WeeklyRule {
        start_day,
        start_time: start_time.to_string(),
        end_day,
        end_time: end_time.to_string(),
    }
}

/// 本机时区的某个时刻。
fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> DateTime<Local> {
    Local
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
}

/// 周四 19:00 起的整周周期。
#[test]
fn weekly_cycle_computes_current_previous_and_next() {
    let rule = rule(4, "19:00:00", 4, "19:00:00");
    // 2026-11-01 是周日。
    let now = at(2026, 11, 1, 12, 0);
    let (start, end) = weekly_cycle(&rule, now, 0).unwrap();
    assert_eq!(start, at(2026, 10, 29, 19, 0));
    assert_eq!(end, at(2026, 11, 5, 19, 0));

    let (start, end) = weekly_cycle(&rule, now, -1).unwrap();
    assert_eq!(start, at(2026, 10, 22, 19, 0));
    assert_eq!(end, at(2026, 10, 29, 19, 0));
    // 上一个周期的结束就是当前周期的开始。
    assert_eq!(end, at(2026, 10, 29, 19, 0));

    let (start, end) = weekly_cycle(&rule, now, 1).unwrap();
    assert_eq!(start, at(2026, 11, 5, 19, 0));
    assert_eq!(end, at(2026, 11, 12, 19, 0));
}

/// 本次时刻还没到时，当前周期是上一次的那个。
#[test]
fn weekly_cycle_falls_back_when_the_time_has_not_come_yet() {
    let rule = rule(4, "19:00:00", 4, "19:00:00");
    // 周四 10:00，当天 19:00 还没到 → 周期是上周四到本周四。
    let now = at(2026, 10, 29, 10, 0);
    let (start, end) = weekly_cycle(&rule, now, 0).unwrap();
    assert_eq!(start, at(2026, 10, 22, 19, 0));
    assert_eq!(end, at(2026, 10, 29, 19, 0));
}

/// 周期不满一周时，结束点按星期差算。
#[test]
fn weekly_cycle_handles_partial_weeks() {
    // 周四 19:00 → 周六 10:30。
    let rule = rule(4, "19:00:00", 6, "10:30:00");
    let now = at(2026, 11, 1, 12, 0);
    let (start, end) = weekly_cycle(&rule, now, 0).unwrap();
    assert_eq!(start, at(2026, 10, 29, 19, 0));
    assert_eq!(end, at(2026, 10, 31, 10, 30));
}

/// 结束时刻早于开始时刻说明这一轮跨了一周。
#[test]
fn weekly_cycle_wraps_to_the_next_week_when_the_end_is_earlier() {
    // 周六 22:00 → 周一 09:00。
    let rule = rule(6, "22:00:00", 1, "09:00:00");
    let now = at(2026, 11, 1, 12, 0);
    let (start, end) = weekly_cycle(&rule, now, 0).unwrap();
    assert_eq!(start, at(2026, 10, 31, 22, 0));
    assert_eq!(end, at(2026, 11, 2, 9, 0));
}

/// 非法的星期与时刻要报错，不能悄悄算出一个错周期。
#[test]
fn weekly_cycle_rejects_invalid_rules() {
    let bad_day = rule(9, "19:00:00", 4, "19:00:00");
    assert!(weekly_cycle(&bad_day, at(2026, 11, 1, 12, 0), 0).is_err());
    let bad_time = rule(4, "25:00:00", 4, "19:00:00");
    assert!(weekly_cycle(&bad_time, at(2026, 11, 1, 12, 0), 0).is_err());
    let bad_end_time = rule(4, "19:00:00", 4, "99:00:00");
    assert!(weekly_cycle(&bad_end_time, at(2026, 11, 1, 12, 0), 0).is_err());
}

/// 前后端两套周期计算必须给出同样的结果。
///
/// 前端在 `tests/subscriptions.test.mjs` 里用同一组固定值断言（2027-01-01
/// 12:00、周四 19:00 起的整周周期）。这里复刻那组期望值，两边一旦漂移就会
/// 有一个先红。
#[test]
fn weekly_cycle_matches_the_frontend_expectations() {
    let weekly = rule(4, "19:00:00", 4, "19:00:00");
    // 2027-01-01 12:00（周五）。上一个周四是 2026-12-31。
    let now = at(2027, 1, 1, 12, 0);
    let (start, end) = weekly_cycle(&weekly, now, 0).unwrap();
    assert_eq!(start, at(2026, 12, 31, 19, 0));
    assert_eq!(end, at(2027, 1, 7, 19, 0));

    let (start, end) = weekly_cycle(&weekly, now, -1).unwrap();
    assert_eq!(start, at(2026, 12, 24, 19, 0));
    assert_eq!(end, at(2026, 12, 31, 19, 0));

    let (start, end) = weekly_cycle(&weekly, now, 1).unwrap();
    assert_eq!(start, at(2027, 1, 7, 19, 0));
    assert_eq!(end, at(2027, 1, 14, 19, 0));

    // 不满一周的周期：周四 19:00 → 周六 10:30。
    let short = rule(4, "19:00:00", 6, "10:30:00");
    let (short_start, short_end) = weekly_cycle(&short, now, 0).unwrap();
    assert_eq!(short_start, at(2026, 12, 31, 19, 0));
    assert_eq!(short_end, at(2027, 1, 2, 10, 30));
}

/// 今天的生效判断：结束日不含当日。
#[test]
fn subscription_active_window_excludes_the_end_date() {
    let item = Subscription {
        id: "id".into(),
        name: "n".into(),
        app_type: "claude".into(),
        price: 1.0,
        currency: "USD".into(),
        billing: "月".into(),
        start_date: "2026-10-01".into(),
        end_date: "2026-11-01".into(),
        enabled: true,
        rule: rule(4, "19:00:00", 4, "19:00:00"),
        custom_interval_days: None,
    };
    assert!(item.is_active_on(chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap()));
    assert!(item.is_active_on(chrono::NaiveDate::from_ymd_opt(2026, 10, 31).unwrap()));
    assert!(!item.is_active_on(chrono::NaiveDate::from_ymd_opt(2026, 11, 1).unwrap()));
    assert!(!item.is_active_on(chrono::NaiveDate::from_ymd_opt(2026, 9, 30).unwrap()));
}

#[tokio::test]
async fn subscriptions_persist_validate_and_local_usage_does_not_depend_on_sharing() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("observatory.db");
    let db = Arc::new(Database::open(&path).unwrap());
    let backend = UsageBackend::with_database(db.clone());
    let record = json!({"id":"10cc9853-04ff-4e68-9fee-ae617e516c23","name":"Claude Max","appType":"claude","price":100.0,"currency":"USD","billing":"月","startDate":"2026-01-01","endDate":"2027-01-01","enabled":true,"rule":{"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}});
    backend
        .call(
            "subscriptions_save",
            json!({"subscriptions":[record.clone()]}),
        )
        .await
        .unwrap();
    assert_eq!(
        backend.call("subscriptions_get", json!({})).await.unwrap()[0]["name"],
        "Claude Max"
    );
    let mut invalid = record.clone();
    invalid["rule"]["startDay"] = json!(8);
    assert!(backend
        .call("subscriptions_save", json!({"subscriptions":[invalid]}))
        .await
        .is_err());
    assert_eq!(
        backend.call("subscriptions_get", json!({})).await.unwrap()[0]["name"],
        "Claude Max"
    );
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types.clear();
    identity::save_share_settings(&db, &settings).unwrap();
    let now = chrono::Utc::now().timestamp();
    db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('test','p','claude','m',100,20,2,'1.25',100,200,{now},'fixture')")).unwrap();
    db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('desktop','p','claude-desktop','m',500,20,2,'5.00',100,200,{now},'fixture')")).unwrap();
    let result = backend
        .call("subscription_usage", json!({"days":1,"appType":"claude"}))
        .await
        .unwrap();
    assert_eq!(result["totalTokens"], 120);
    assert_eq!(result["costUsd"], 1.25);
    drop(backend);
    drop(db);
    let reopened = Arc::new(Database::open(&path).unwrap());
    assert_eq!(
        UsageBackend::with_database(reopened)
            .call("subscriptions_get", json!({}))
            .await
            .unwrap()[0]["name"],
        "Claude Max"
    );
}

#[tokio::test]
async fn partial_archived_days_are_reported_instead_of_silently_omitted() {
    use chrono::{Local, TimeZone};
    let db = Arc::new(Database::memory().unwrap());
    let day = Local::now().date_naive().pred_opt().unwrap();
    db.execute_batch(&format!("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,input_token_semantics,total_cost_usd) VALUES ('{day}','claude','p','m',1,1,100,2,'1.00')")).unwrap();
    let at = |hour| {
        Local
            .from_local_datetime(&day.and_hms_opt(hour, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .to_rfc3339()
    };
    let backend = UsageBackend::with_database(db);
    let error = backend
        .call(
            "subscription_usage",
            json!({"appType":"claude","startTime":at(9),"endTime":at(18)}),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("日汇总"));
}

/// 精度错误要保留错误码与受影响日期，前端才能给出补救路径。
///
/// `usage_call` 的错误通道是纯字符串，所以结构化信息被编码成
/// `{"observatoryError":{...}}` 信封。
#[tokio::test]
async fn precision_error_carries_code_and_affected_dates() {
    use chrono::{Local, TimeZone};
    let db = Arc::new(Database::memory().unwrap());
    let day = Local::now().date_naive().pred_opt().unwrap();
    db.execute_batch(&format!("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,input_token_semantics,total_cost_usd) VALUES ('{day}','claude','p','m',1,1,100,2,'1.00')")).unwrap();
    let at = |hour| {
        Local
            .from_local_datetime(&day.and_hms_opt(hour, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .to_rfc3339()
    };
    let backend = UsageBackend::with_database(db);
    let error = backend
        .call(
            "subscription_usage",
            json!({"appType":"claude","startTime":at(9),"endTime":at(18)}),
        )
        .await
        .unwrap_err();
    let envelope: serde_json::Value =
        serde_json::from_str(&error.to_string()).expect("错误应当是结构化信封");
    let body = &envelope["observatoryError"];
    assert_eq!(body["code"], "RANGE_PRECISION_UNAVAILABLE");
    assert_eq!(body["details"]["dates"], json!([day.to_string()]));
    assert!(body["error"].as_str().unwrap().contains("日汇总"));
}

/// 完整日期范围能绕开精度限制：被截断的那天整日计入。
#[tokio::test]
async fn full_date_range_covers_archived_days_without_precision_error() {
    use chrono::Local;
    let db = Arc::new(Database::memory().unwrap());
    let day = Local::now().date_naive().pred_opt().unwrap();
    db.execute_batch(&format!("INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,input_token_semantics,total_cost_usd) VALUES ('{day}','claude','p','m',1,1,100,2,'1.00')")).unwrap();
    let backend = UsageBackend::with_database(db);
    let result = backend
        .call(
            "subscription_usage",
            json!({"appType":"claude","startDate":day.to_string(),"endDate":day.to_string()}),
        )
        .await
        .unwrap();
    assert_eq!(result["requests"], 1);
    assert_eq!(result["totalTokens"], 100);
}

/// `dailyLimit` 只裁每日序列，不动汇总口径。
#[tokio::test]
async fn daily_limit_trims_the_series_but_not_the_totals() {
    let db = Arc::new(Database::memory().unwrap());
    let now = chrono::Utc::now().timestamp();
    for day in 0..10 {
        db.execute_batch(&format!("INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('r{day}','p','claude','m',100,20,2,'1.00',100,200,{},'fixture')", now - day * 86400)).unwrap();
    }
    let backend = UsageBackend::with_database(db);
    let full = backend
        .call("subscription_usage", json!({"appType":"claude","days":10}))
        .await
        .unwrap();
    assert_eq!(full["daily"].as_array().unwrap().len(), 10);
    assert_eq!(full["requests"], 10);

    let trimmed = backend
        .call(
            "subscription_usage",
            json!({"appType":"claude","days":10,"dailyLimit":3}),
        )
        .await
        .unwrap();
    assert_eq!(trimmed["daily"].as_array().unwrap().len(), 3);
    // 汇总口径不受裁剪影响。
    assert_eq!(trimmed["requests"], 10);
    assert_eq!(trimmed["totalTokens"], full["totalTokens"]);
}

/// 自定义账期要给出天数；没选「期」就不该带这个字段。
#[tokio::test]
async fn custom_billing_interval_is_validated_and_round_trips() {
    let db = Arc::new(Database::memory().unwrap());
    let backend = UsageBackend::with_database(db);
    let base = json!({"id":"10cc9853-04ff-4e68-9fee-ae617e516c23","name":"Claude Max","appType":"claude","price":100.0,"currency":"USD","billing":"期","startDate":"2026-01-01","endDate":"2027-01-01","enabled":true,"rule":{"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}});

    // 选了「期」却没给天数：拒绝。
    assert!(backend
        .call("subscriptions_save", json!({"subscriptions":[base.clone()]}))
        .await
        .is_err());

    // 天数越界：拒绝。
    let mut too_long = base.clone();
    too_long["customIntervalDays"] = json!(400);
    assert!(backend
        .call("subscriptions_save", json!({"subscriptions":[too_long]}))
        .await
        .is_err());

    // 合法值：存得进去也读得回来。
    let mut valid = base.clone();
    valid["customIntervalDays"] = json!(30);
    backend
        .call("subscriptions_save", json!({"subscriptions":[valid]}))
        .await
        .unwrap();
    let stored = backend.call("subscriptions_get", json!({})).await.unwrap();
    assert_eq!(stored[0]["customIntervalDays"], 30);

    // 固定账期带了天数：拒绝。
    let mut monthly = base.clone();
    monthly["billing"] = json!("月");
    monthly["customIntervalDays"] = json!(30);
    assert!(backend
        .call("subscriptions_save", json!({"subscriptions":[monthly]}))
        .await
        .is_err());
}

/// 旧版本存下的记录没有 `customIntervalDays`，仍要能读出来。
#[tokio::test]
async fn legacy_subscription_records_still_load() {
    let db = Arc::new(Database::memory().unwrap());
    db.set_setting(
        "subscriptions_v1",
        r#"[{"id":"10cc9853-04ff-4e68-9fee-ae617e516c23","name":"Claude Max","appType":"claude","price":100.0,"currency":"USD","billing":"月","startDate":"2026-01-01","endDate":"2027-01-01","enabled":true,"rule":{"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}}]"#,
    )
    .unwrap();
    let backend = UsageBackend::with_database(db);
    let stored = backend.call("subscriptions_get", json!({})).await.unwrap();
    assert_eq!(stored[0]["name"], "Claude Max");
    assert!(stored[0]["customIntervalDays"].is_null());
}
