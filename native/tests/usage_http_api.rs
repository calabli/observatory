use chrono::{Duration, Local, NaiveDate, TimeZone};
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
        ..Default::default()
    }
}

/// 本地某天的某个时刻的 Unix 秒。
fn local_ts(date: NaiveDate, hour: u32, minute: u32) -> i64 {
    Local
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp()
}

/// 插入一条明细行。
fn insert_detail(
    db: &Database,
    request_id: &str,
    app_type: &str,
    model: &str,
    pricing_model: Option<&str>,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_creation: i64,
    cost: &str,
    created_at: i64,
) {
    let pricing = match pricing_model {
        Some(value) => format!("'{value}'"),
        None => "NULL".to_string(),
    };
    db.execute_batch(&format!(
        "INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,pricing_model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('{request_id}','p','{app_type}','{model}',{pricing},{input},{output},{cache_read},{cache_creation},2,'{cost}',1000,200,{created_at},'fixture');"
    ))
    .unwrap();
}

/// 插入一条日汇总行。
fn insert_rollup(
    db: &Database,
    date: NaiveDate,
    app_type: &str,
    model: &str,
    requests: i64,
    input: i64,
    output: i64,
    cost: &str,
) {
    db.execute_batch(&format!(
        "INSERT INTO usage_daily_rollups(date,app_type,provider_id,model,request_count,success_count,input_tokens,output_tokens,input_token_semantics,total_cost_usd) VALUES ('{date}','{app_type}','p','{model}',{requests},{requests},{input},{output},2,'{cost}');"
    ))
    .unwrap();
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

fn local_instant(date: NaiveDate, hour: u32) -> String {
    Local.timestamp_opt(local_ts(date, hour, 0), 0).single().unwrap().to_rfc3339()
}

#[test]
fn omitted_end_time_defaults_to_current_second() {
    let db = Database::memory().unwrap();
    let now = Local::now().timestamp();
    insert_detail(&db,"inside","claude","m",None,10,2,0,0,"0.25",now-60);
    insert_detail(&db,"future","claude","m",None,90,2,0,0,"9.00",now+3600);
    let since = Local.timestamp_opt(now-120,0).single().unwrap().to_rfc3339();
    let before = Local::now().timestamp();
    let result = public_api::summary(&db,public_api::SummaryQuery {start_time:Some(since.clone()),app_type:Some("claude".into()),..Default::default()}).unwrap();
    let after = Local::now().timestamp();
    let end = chrono::DateTime::parse_from_rfc3339(result["range"]["endExclusive"].as_str().unwrap()).unwrap();
    assert!((before..=after).contains(&end.timestamp()));
    assert_eq!(end.timestamp_subsec_nanos(),0);
    assert_eq!(result["requests"],1);
    assert_eq!(result["costUsd"],0.25);
    let blank = public_api::summary(&db,public_api::SummaryQuery {start_time:Some(since.clone()),end_time:Some("  ".into()),app_type:Some("claude".into()),..Default::default()}).unwrap();
    assert_eq!(blank["requests"],1);
    let fixed = public_api::summary(&db,public_api::SummaryQuery {start_time:Some(since),end_time:Some(Local.timestamp_opt(now-90,0).single().unwrap().to_rfc3339()),app_type:Some("claude".into()),..Default::default()}).unwrap();
    assert_eq!(fixed["requests"],0);
    assert!(public_api::summary(&db,public_api::SummaryQuery {start_time:Some(Local.timestamp_opt(now+3600,0).single().unwrap().to_rfc3339()),..Default::default()}).is_err());
}

#[tokio::test]
async fn http_accepts_start_time_without_end_time() {
    let db=Arc::new(Database::memory().unwrap());
    let handle=server::start(db.clone(),0).await.unwrap();
    let key=public_api::rotate_key(&db).unwrap();
    let since=(Local::now()-Duration::minutes(5)).format("%Y-%m-%dT%H:%M:%S%:z").to_string();
    let url=format!("http://127.0.0.1:{}/api/v1/usage/summary",handle.port);
    let http=reqwest::Client::builder().no_proxy().build().unwrap();
    let response=http.get(&url).query(&[("startTime",since.as_str()),("appType","claude")]).header("X-API-Key",&key).send().await.unwrap();
    assert!(response.status().is_success());
    let result:Value=response.json().await.unwrap();
    assert!(result["range"]["endExclusive"].is_string());
    let response=http.get(&url).query(&[("startTime",since.as_str()),("endTime","")]).header("X-API-Key",&key).send().await.unwrap();
    assert!(response.status().is_success());
    let response=http.get(&url).query(&[("endTime",since.as_str())]).header("X-API-Key",&key).send().await.unwrap();
    assert_eq!(response.status(),400);
    assert_eq!(response.json::<Value>().await.unwrap()["code"],"CONFLICTING_TIME_PARAMETERS");
    handle.stop();
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
    assert_eq!(empty["apps"].as_array().unwrap().len(), 0);
}

/// v0.1.30 的顶层字段和路径全部保留，新增字段只做追加。
#[test]
fn v030_response_shape_is_backward_compatible() {
    let db = Database::memory().unwrap();
    seed(&db);
    let result = public_api::summary(&db, query(7, None, Some("claude"))).unwrap();
    for field in [
        "scope",
        "appType",
        "apps",
        "days",
        "startDate",
        "endDate",
        "range",
        "currency",
        "inputTokenSemantics",
        "updatedAt",
        "lastScanAt",
        "requests",
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
        "totalTokens",
        "costUsd",
        "daily",
    ] {
        assert!(result.get(field).is_some(), "缺少字段 {field}");
    }
    assert_eq!(result["scope"], "local");
    assert_eq!(result["currency"], "USD");
    assert_eq!(result["inputTokenSemantics"], "fresh");
    assert_eq!(result["range"]["timezone"], "server-local");
    assert_eq!(result["model"], Value::Null);
    assert_eq!(result["modelFilterBasis"], "effectivePricingModel");
    assert_eq!(result["groupBy"], "day");
    assert!(result["buckets"].is_array());
}

/// `days` 与 `startDate`/`endDate` 表达同一区间时结果必须完全一致。
#[test]
fn date_range_matches_equivalent_days_window() {
    let db = Database::memory().unwrap();
    seed(&db);
    let end = today().pred_opt().unwrap();
    let start = end.pred_opt().unwrap().pred_opt().unwrap();
    let by_days = public_api::summary(&db, query(3, Some(end.to_string()), Some("claude"))).unwrap();
    let by_range = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_date: Some(start.to_string()),
            end_date: Some(end.to_string()),
            app_type: Some("claude".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(by_days["requests"], by_range["requests"]);
    assert_eq!(by_days["costUsd"], by_range["costUsd"]);
    assert_eq!(by_days["totalTokens"], by_range["totalTokens"]);
    assert_eq!(by_days["daily"], by_range["daily"]);
    assert_eq!(by_range["startDate"], start.to_string());
    assert_eq!(by_range["endDate"], end.to_string());
    assert_eq!(by_range["days"], 3);
}

/// 区间是左闭右开：起点包含、终点不包含。
#[test]
fn instant_range_is_left_closed_right_open() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    insert_detail(
        &db,
        "a",
        "claude",
        "m",
        None,
        10,
        1,
        0,
        0,
        "0.10",
        local_ts(day, 9, 0),
    );
    insert_detail(
        &db,
        "b",
        "claude",
        "m",
        None,
        10,
        1,
        0,
        0,
        "0.10",
        local_ts(day, 12, 0),
    );
    insert_detail(
        &db,
        "c",
        "claude",
        "m",
        None,
        10,
        1,
        0,
        0,
        "0.10",
        local_ts(day, 18, 0),
    );

    let base = |start_hour: u32, end_hour: u32| public_api::SummaryQuery {
        start_time: Some(local_instant(day, start_hour)),
        end_time: Some(local_instant(day, end_hour)),
        app_type: Some("claude".into()),
        ..Default::default()
    };
    // 09:00 起、18:00 止：包含 09:00 与 12:00 两行，不含 18:00。
    let result = public_api::summary(&db, base(9, 18)).unwrap();
    assert_eq!(result["requests"], 2);
    assert_eq!(result["costUsd"], 0.2);
    // 起点本身包含。
    let inclusive = public_api::summary(&db, base(12, 18)).unwrap();
    assert_eq!(inclusive["requests"], 1);
}

/// 秒级区间切天、而该天只剩日汇总时必须报错，不能返回被截断的数字。
#[test]
fn partial_day_with_only_rollups_returns_422() {
    let db = Database::memory().unwrap();
    let yesterday = today().pred_opt().unwrap();
    insert_rollup(&db, yesterday, "claude", "m", 4, 100, 10, "1.00");

    let partial = public_api::SummaryQuery {
        start_time: Some(local_instant(yesterday, 9)),
        end_time: Some(local_instant(yesterday, 18)),
        app_type: Some("claude".into()),
        ..Default::default()
    };
    let error = public_api::summary(&db, partial).unwrap_err();
    assert_eq!(error.status, 422);
    assert_eq!(error.code, "RANGE_PRECISION_UNAVAILABLE");
    assert_eq!(error.details.unwrap()["dates"][0], yesterday.to_string());

    // 完整覆盖该日时可以整日计入，不报错。
    let whole = public_api::SummaryQuery {
        start_time: Some(local_instant(yesterday, 0)),
        end_time: Some(local_instant(yesterday.succ_opt().unwrap(), 0)),
        app_type: Some("claude".into()),
        ..Default::default()
    };
    let result = public_api::summary(&db, whole).unwrap();
    assert_eq!(result["requests"], 4);
    assert_eq!(result["costUsd"], 1.0);
}

/// 明细存在的日期不受日汇总精度限制影响。
#[test]
fn partial_day_with_details_is_not_blocked() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    insert_detail(
        &db,
        "a",
        "claude",
        "m",
        None,
        10,
        1,
        0,
        0,
        "0.10",
        local_ts(day, 10, 0),
    );
    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_time: Some(local_instant(day, 9)),
            end_time: Some(local_instant(day, 11)),
            app_type: Some("claude".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(result["requests"], 1);
    assert_eq!(result["daily"].as_array().unwrap().len(), 1);
}

/// 顶层、`daily` 与 `buckets` 必须来自同一批数据，加总一致。
#[test]
fn top_level_daily_and_buckets_agree() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    for offset in 0..10 {
        let date = day - Duration::days(offset);
        insert_detail(
            &db,
            &format!("d{offset}"),
            "claude",
            "m",
            None,
            100,
            10,
            5,
            2,
            "0.50",
            local_ts(date, 10, 0),
        );
    }
    let start = (day - Duration::days(9)).to_string();
    let end = day.to_string();
    for group in ["day", "week", "month"] {
        let result = public_api::summary(
            &db,
            public_api::SummaryQuery {
                start_date: Some(start.clone()),
                end_date: Some(end.clone()),
                app_type: Some("claude".into()),
                group_by: Some(group.to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result["groupBy"], group);
        let daily_total: f64 = result["daily"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["costUsd"].as_f64().unwrap())
            .sum();
        let bucket_total: f64 = result["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["costUsd"].as_f64().unwrap())
            .sum();
        assert!((daily_total - result["costUsd"].as_f64().unwrap()).abs() < 1e-9);
        assert!((bucket_total - result["costUsd"].as_f64().unwrap()).abs() < 1e-9);
        let daily_requests: u64 = result["daily"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["requests"].as_u64().unwrap())
            .sum();
        let bucket_requests: u64 = result["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["requests"].as_u64().unwrap())
            .sum();
        assert_eq!(daily_requests, result["requests"].as_u64().unwrap());
        assert_eq!(bucket_requests, result["requests"].as_u64().unwrap());
    }
}

/// ISO 周编号使用该周所属年份，跨年周不拼日历年。
#[test]
fn iso_week_uses_the_iso_week_year() {
    let db = Database::memory().unwrap();
    let start = NaiveDate::from_ymd_opt(2025, 12, 21).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 1, 10).unwrap();
    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_date: Some(start.to_string()),
            end_date: Some(end.to_string()),
            group_by: Some("week".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let periods: Vec<&str> = result["buckets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["period"].as_str().unwrap())
        .collect();
    // 2025-12-21 属于 2025-W51；跨年的那一周按 ISO 所属年记为 2026-W01。
    assert_eq!(
        periods,
        vec!["2025-W51", "2025-W52", "2026-W01", "2026-W02"]
    );
}

/// 月分组的周期标识、边界与补零。
#[test]
fn month_buckets_use_natural_months_and_zero_fill() {
    let db = Database::memory().unwrap();
    let start = NaiveDate::from_ymd_opt(2026, 1, 15).unwrap();
    let end = NaiveDate::from_ymd_opt(2026, 4, 10).unwrap();
    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_date: Some(start.to_string()),
            end_date: Some(end.to_string()),
            group_by: Some("month".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let buckets = result["buckets"].as_array().unwrap();
    let periods: Vec<&str> = buckets
        .iter()
        .map(|row| row["period"].as_str().unwrap())
        .collect();
    assert_eq!(periods, vec!["2026-01", "2026-02", "2026-03", "2026-04"]);
    // 空周期补零。
    assert_eq!(buckets[1]["requests"], 0);
    assert_eq!(buckets[1]["costUsd"], 0.0);
    // 首尾 bucket 是自然周期与查询区间的交集。
    assert!(buckets[0]["start"]
        .as_str()
        .unwrap()
        .starts_with("2026-01-15"));
    assert!(buckets[3]["endExclusive"]
        .as_str()
        .unwrap()
        .starts_with("2026-04-11"));
}

/// 闰日与不同长度月份都能正确切分。
#[test]
fn leap_day_and_variable_month_lengths_are_covered() {
    let db = Database::memory().unwrap();
    let start = NaiveDate::from_ymd_opt(2024, 2, 27).unwrap();
    let end = NaiveDate::from_ymd_opt(2024, 3, 2).unwrap();
    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_date: Some(start.to_string()),
            end_date: Some(end.to_string()),
            group_by: Some("month".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let buckets = result["buckets"].as_array().unwrap();
    assert_eq!(buckets.len(), 2);
    assert_eq!(buckets[0]["period"], "2024-02");
    assert_eq!(buckets[1]["period"], "2024-03");
    assert_eq!(result["days"], 5);
}

/// 模型筛选按有效计价模型匹配，与页面口径一致。
#[test]
fn model_filter_uses_effective_pricing_model() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    insert_detail(
        &db,
        "a",
        "claude",
        "claude-opus-5-5",
        Some("claude-opus-5-5"),
        100,
        10,
        0,
        0,
        "0.50",
        local_ts(day, 10, 0),
    );
    insert_detail(
        &db,
        "b",
        "claude",
        "vendor/model-x",
        Some("model-x"),
        200,
        20,
        0,
        0,
        "1.00",
        local_ts(day, 10, 0),
    );
    let filtered = public_api::summary(
        &db,
        public_api::SummaryQuery {
            days: Some(7),
            app_type: Some("claude".into()),
            model: Some("model-x".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered["requests"], 1);
    assert_eq!(filtered["costUsd"], 1.0);
    assert_eq!(filtered["model"], json!(["model-x"]));
    assert_eq!(filtered["modelFilterBasis"], "effectivePricingModel");
    // 没有匹配记录时返回零值而不是错误。
    let missing = public_api::summary(
        &db,
        public_api::SummaryQuery {
            days: Some(7),
            app_type: Some("claude".into()),
            model: Some("no-such-model".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(missing["requests"], 0);
    assert_eq!(missing["costUsd"], 0.0);
}

/// Claude Code 与 Claude Desktop 精确区分。
#[test]
fn claude_code_and_desktop_are_separate() {
    let db = Database::memory().unwrap();
    seed(&db);
    let code = public_api::summary(&db, query(7, None, Some("claude"))).unwrap();
    let desktop = public_api::summary(&db, query(7, None, Some("claude-desktop"))).unwrap();
    assert_eq!(code["costUsd"], 1.75);
    assert_eq!(desktop["costUsd"], 9.0);
    assert_eq!(code["apps"], json!(["claude"]));
    assert_eq!(desktop["apps"], json!(["claude-desktop"]));
}

/// 时间参数的三种模式互斥，冲突时给出稳定错误码。
#[test]
fn conflicting_time_parameters_are_rejected() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    let cases = vec![
        public_api::SummaryQuery {
            days: Some(7),
            start_date: Some(day.to_string()),
            ..Default::default()
        },
        public_api::SummaryQuery {
            days: Some(7),
            start_time: Some(format!("{day}T00:00:00+08:00")),
            end_time: Some(format!("{day}T01:00:00+08:00")),
            ..Default::default()
        },
        public_api::SummaryQuery {
            end_time: Some(format!("{day}T01:00:00+08:00")),
            ..Default::default()
        },
        public_api::SummaryQuery {
            start_time: Some(format!("{day}T02:00:00+08:00")),
            end_time: Some(format!("{day}T01:00:00+08:00")),
            ..Default::default()
        },
    ];
    for case in cases {
        let error = public_api::summary(&db, case).unwrap_err();
        assert_eq!(error.status, 400, "{error:?}");
        assert!(
            matches!(
                error.code,
                "CONFLICTING_TIME_PARAMETERS" | "INVALID_DATE_RANGE"
            ),
            "意外错误码 {}",
            error.code
        );
    }
}

/// 无时区、非整秒和不支持的分组都要被拒绝。
#[test]
fn invalid_parameters_have_stable_codes() {
    let db = Database::memory().unwrap();
    let day = today().pred_opt().unwrap();
    let cases = vec![
        (
            public_api::SummaryQuery {
                start_time: Some(format!("{day}T00:00:00")),
                end_time: Some(format!("{day}T01:00:00")),
                ..Default::default()
            },
            "INVALID_PARAMETERS",
        ),
        (
            public_api::SummaryQuery {
                start_time: Some(format!("{day}T00:00:00.500+08:00")),
                end_time: Some(format!("{day}T01:00:00+08:00")),
                ..Default::default()
            },
            "INVALID_PARAMETERS",
        ),
        (
            public_api::SummaryQuery {
                days: Some(7),
                group_by: Some("year".into()),
                ..Default::default()
            },
            "UNSUPPORTED_GROUP_BY",
        ),
        (
            public_api::SummaryQuery {
                days: Some(7),
                app_type: Some("nope".into()),
                ..Default::default()
            },
            "UNSUPPORTED_APP_TYPE",
        ),
        (
            public_api::SummaryQuery {
                days: Some(7),
                app_type: Some("".into()),
                ..Default::default()
            },
            "INVALID_PARAMETERS",
        ),
        (
            public_api::SummaryQuery {
                end_date: Some("2999-01-01".into()),
                ..Default::default()
            },
            "INVALID_DATE_RANGE",
        ),
    ];
    for (case, code) in cases {
        let error = public_api::summary(&db, case).unwrap_err();
        assert_eq!(error.status, 400, "{error:?}");
        assert_eq!(error.code, code);
    }
}

/// 未共享的应用返回 403；共享集合为空时返回零值。
#[test]
fn app_scope_is_enforced() {
    let db = Database::memory().unwrap();
    seed(&db);
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();
    let error = public_api::summary(&db, query(7, None, Some("claude"))).unwrap_err();
    assert_eq!(error.status, 403);
    assert_eq!(error.code, "APP_NOT_SHARED");
    settings.shared_app_types = vec![];
    identity::save_share_settings(&db, &settings).unwrap();
    let empty = public_api::summary(&db, query(7, None, None)).unwrap();
    assert_eq!(empty["requests"], 0);
    assert_eq!(empty["apps"].as_array().unwrap().len(), 0);
    assert_eq!(empty["buckets"].as_array().unwrap().len(), 7);
}

/// 响应里的时间戳带本地偏移，`range` 两端各自携带。
#[test]
fn range_carries_local_offsets() {
    let db = Database::memory().unwrap();
    seed(&db);
    let result = public_api::summary(&db, query(2, None, Some("claude"))).unwrap();
    let start = result["range"]["start"].as_str().unwrap();
    let end = result["range"]["endExclusive"].as_str().unwrap();
    assert!(start.ends_with('Z') || start.contains('+') || start.contains('-'));
    assert!(end > start);
    assert_eq!(result["range"]["timezone"], "server-local");
}

/// 写一条订阅记录。
fn save_subscription(db: &Database, app_type: &str, start: &str, end: &str, rule: Value) {
    let record = json!({
        "id": "10cc9853-04ff-4e68-9fee-ae617e516c23",
        "name": "Claude Max",
        "appType": app_type,
        "price": 100.0,
        "currency": "USD",
        "billing": "\u{6708}",
        "startDate": start,
        "endDate": end,
        "enabled": true,
        "rule": rule,
    });
    let items: Vec<observatory_usage_core::subscriptions::Subscription> =
        serde_json::from_value(json!([record])).unwrap();
    observatory_usage_core::subscriptions::save(db, items).unwrap();
}

/// 覆盖前一年与后一年，周期完全落在有效期内，不会被收敛。
fn year_window() -> (String, String) {
    let start = today() - chrono::Duration::days(365);
    let end = today() + chrono::Duration::days(365);
    (
        start.format("%Y-%m-%d").to_string(),
        end.format("%Y-%m-%d").to_string(),
    )
}

/// 周期查询端点：返回体与 summary 完全一致，区间由服务端按订阅周期算好。
#[tokio::test]
async fn http_cycle_endpoint_matches_the_summary_contract() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"});
    save_subscription(&db, "claude", &start, &end, rule_value.clone());

    // 在当前周期「已经过去」的那一段里放一条明细，让断言与「今天几点」无关。
    let rule: observatory_usage_core::subscriptions::WeeklyRule =
        serde_json::from_value(rule_value).unwrap();
    let (cycle_start, cycle_end) =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, Local::now(), 0).unwrap();
    let now = Local::now();
    let elapsed = now.signed_duration_since(cycle_start);
    assert!(elapsed.num_seconds() > 0, "当前周期应当已经开始");
    let inside = cycle_start + elapsed / 2;
    insert_detail(
        &db,
        "in-cycle",
        "claude",
        "m",
        None,
        100,
        20,
        300,
        10,
        "1.25",
        inside.timestamp(),
    );
    let _ = cycle_end;

    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycle", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // 没有密钥 → 401。
    assert_eq!(http.get(&url).send().await.unwrap().status(), 401);

    let key = public_api::rotate_key(&db).unwrap();
    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(response.headers()["cache-control"], "no-store");
    let cycle: Value = response.json().await.unwrap();

    // 返回体字段与 summary 完全一致。
    let range_start =
        chrono::DateTime::parse_from_rfc3339(cycle["range"]["start"].as_str().unwrap()).unwrap();
    let range_end = chrono::DateTime::parse_from_rfc3339(
        cycle["range"]["endExclusive"].as_str().unwrap(),
    )
    .unwrap();

    let summary: Value = http
        .get(format!(
            "http://127.0.0.1:{}/api/v1/usage/summary?days=1&appType=claude",
            handle.port
        ))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut cycle_keys: Vec<&String> = cycle.as_object().unwrap().keys().collect();
    let mut summary_keys: Vec<&String> = summary.as_object().unwrap().keys().collect();
    cycle_keys.sort();
    summary_keys.sort();
    assert_eq!(cycle_keys, summary_keys, "字段集合应与 summary 一致");

    // 周期起点与订阅规则一致；终点截到当前时刻（未来的用量还不存在）。
    assert_eq!(range_start, cycle_start);
    assert!(range_end <= cycle_end);
    assert!(range_end > range_start);
    let now_second = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let now_second = chrono::DateTime::parse_from_rfc3339(&now_second).unwrap();
    assert!(
        (range_end - now_second).num_seconds().abs() <= 60,
        "终点应截到请求时刻"
    );

    // 统计口径：周期里那条明细被算进来，四类 Token 之和等于总数。
    assert_eq!(cycle["requests"], 1);
    assert_eq!(cycle["costUsd"], 1.25);
    assert_eq!(cycle["totalTokens"], 430);
    assert_eq!(
        cycle["totalTokens"].as_u64().unwrap(),
        cycle["inputTokens"].as_u64().unwrap()
            + cycle["outputTokens"].as_u64().unwrap()
            + cycle["cacheReadTokens"].as_u64().unwrap()
            + cycle["cacheWriteTokens"].as_u64().unwrap()
    );
    assert_eq!(cycle["appType"], json!(["claude"]));
    assert_eq!(cycle["scope"], "local");
    assert_eq!(cycle["inputTokenSemantics"], "fresh");
    assert_eq!(cycle["currency"], "USD");
    assert_eq!(cycle["groupBy"], "day");

    // 订阅周期保持不变，可以将同一批指定模型的数据按周或按月合并。
    for group in ["week", "month"] {
        let response = http.get(format!("{url}?appType=claude&model=m&groupBy={group}"))
            .header("X-API-Key", &key).send().await.unwrap();
        assert!(response.status().is_success());
        let grouped: Value = response.json().await.unwrap();
        assert_eq!(grouped["groupBy"], group);
        assert_eq!(grouped["requests"], cycle["requests"]);
        assert_eq!(grouped["totalTokens"], cycle["totalTokens"]);
        assert_eq!(grouped["costUsd"], cycle["costUsd"]);
        assert_eq!(grouped["model"], json!(["m"]));
    }

    // 缺少 appType、未知参数、重复参数都要 400。
    for params in ["", "appType=", "unknown=1", "appType=claude&appType=codex", "offset=999", "appType=claude&groupBy=year"] {
        let response = http
            .get(format!("{url}?{params}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{params}");
        let body: Value = response.json().await.unwrap();
        assert!(body["code"].is_string(), "{params}");
    }

    // 没有订阅的应用 → 404。
    let response = http
        .get(format!("{url}?appType=codex"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "SUBSCRIPTION_NOT_FOUND");

    // 未共享的应用 → 403，且不泄露订阅是否存在。
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();
    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "APP_NOT_SHARED");

    handle.stop();
}

/// 周期按订阅有效期收敛：开头取较晚者，结尾取较早者。
#[tokio::test]
async fn http_cycle_endpoint_clamps_to_the_subscription_window() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    // 有效期只有今天前后一天，周期会被收窄。
    let start = today().pred_opt().unwrap().format("%Y-%m-%d").to_string();
    let end = today().succ_opt().unwrap().format("%Y-%m-%d").to_string();
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycle", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    let range_start =
        chrono::DateTime::parse_from_rfc3339(body["range"]["start"].as_str().unwrap()).unwrap();
    let range_end =
        chrono::DateTime::parse_from_rfc3339(body["range"]["endExclusive"].as_str().unwrap()).unwrap();
    // 结尾不晚于到期日零点。
    let expiry = Local
        .from_local_datetime(
            &today()
                .succ_opt()
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .earliest()
        .unwrap();
    assert!(range_end <= expiry, "结尾应收敛到到期时刻");
    // 开头不早于开始日零点。
    let begins = Local
        .from_local_datetime(
            &today()
                .pred_opt()
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap(),
        )
        .earliest()
        .unwrap();
    assert!(range_start >= begins, "开头应收敛到开始时刻");
    // 收敛后仍是一段非空区间。
    assert!(range_end > range_start);

    handle.stop();
}

/// 上一周期能按订阅规则往前推；还没开始的周期返回 404。
#[tokio::test]
async fn http_cycle_endpoint_walks_back_and_forward() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"});
    save_subscription(&db, "claude", &start, &end, rule_value.clone());
    let rule: observatory_usage_core::subscriptions::WeeklyRule =
        serde_json::from_value(rule_value).unwrap();

    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycle", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let read = |offset: i32| {
        let http = http.clone();
        let url = url.clone();
        let key = key.clone();
        async move {
            let response = http
                .get(format!("{url}?appType=claude&offset={offset}"))
                .header("X-API-Key", &key)
                .send()
                .await
                .unwrap();
            assert!(response.status().is_success(), "offset {offset}");
            let body: Value = response.json().await.unwrap();
            let start =
                chrono::DateTime::parse_from_rfc3339(body["range"]["start"].as_str().unwrap())
                    .unwrap();
            let end = chrono::DateTime::parse_from_rfc3339(
                body["range"]["endExclusive"].as_str().unwrap(),
            )
            .unwrap();
            (start, end)
        }
    };

    let (start0, end0) = read(0).await;
    let (start1, end1) = read(-1).await;

    // 当前周期的起点与按订阅规则算出来的一致，终点截到当前时刻。
    let expected =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, Local::now(), 0).unwrap();
    assert_eq!(start0, expected.0);
    assert!(end0 <= expected.1);
    assert!(end0 > start0);

    // 上一个周期的结束正好是当前周期的开始，相邻周期正好差七天。
    assert_eq!(end1, start0);
    assert_eq!((start0 - start1).num_days(), 7);

    // 下一个周期还没开始，没有可统计的区间 → 404。
    let response = http
        .get(format!("{url}?appType=claude&offset=1"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "CYCLE_NOT_AVAILABLE");

    handle.stop();
}

/// 多个平台与多个模型：逗号分隔，结果合并统计。
#[tokio::test]
async fn http_query_accepts_multiple_apps_and_models() {
    let db = Arc::new(Database::memory().unwrap());
    let today = today();
    let ts = local_ts(today, 12, 0);
    // claude 用模型 m-a，codex 用模型 m-b。
    insert_detail(&db, "a1", "claude", "m-a", None, 100, 20, 0, 0, "1.00", ts);
    insert_detail(&db, "a2", "codex", "m-b", None, 200, 40, 0, 0, "2.00", ts);
    insert_detail(&db, "a3", "claude", "m-c", None, 400, 80, 0, 0, "4.00", ts);

    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let get = |query: &str| {
        let http = http.clone();
        let url = url.clone();
        let key = key.clone();
        let query = query.to_string();
        async move {
            http.get(format!("{url}?{query}"))
                .header("X-API-Key", &key)
                .send()
                .await
                .unwrap()
        }
    };

    // 两个平台合计。
    let response = get("days=1&appType=claude,codex").await;
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["appType"], json!(["claude", "codex"]));
    assert_eq!(body["apps"], json!(["claude", "codex"]));
    assert_eq!(body["requests"], 3);
    assert_eq!(body["totalTokens"], 840);

    // 指定两个模型。
    let response = get("days=1&appType=claude,codex&model=m-a,m-b").await;
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["model"], json!(["m-a", "m-b"]));
    assert_eq!(body["requests"], 2);
    assert_eq!(body["totalTokens"], 360);

    // 重复的应用与模型去重。
    let response = get("days=1&appType=claude,claude&model=m-a,m-a").await;
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["apps"], json!(["claude"]));
    assert_eq!(body["model"], json!(["m-a"]));

    // 空段与未知标识都要 400。
    for query in [
        "days=1&appType=claude,",
        "days=1&appType=claude,,codex",
        "days=1&appType=claude,unknown",
        "days=1&appType=claude&model=",
        "days=1&appType=claude&model=m-a,",
    ] {
        let response = get(query).await;
        assert_eq!(response.status(), 400, "{query}");
    }

    handle.stop();
}

#[tokio::test]
async fn per_app_models_apply_to_their_own_platform_in_cycle_and_summary() {
    let db=Arc::new(Database::memory().unwrap());
    let (start,end)=year_window();
    let rule_value=json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"});
    save_subscription(&db,"claude",&start,&end,rule_value.clone());
    let rule=serde_json::from_value(rule_value).unwrap();
    let now=Local::now();
    let (cycle_start,_)=observatory_usage_core::subscriptions::weekly_cycle(&rule,now,0).unwrap();
    let ts=(cycle_start+now.signed_duration_since(cycle_start)/2).timestamp();
    insert_detail(&db,"a1","claude","m-a",None,100,20,0,0,"1.00",ts);
    insert_detail(&db,"a2","codex","m-b",None,200,40,0,0,"2.00",ts);
    insert_detail(&db,"wrong-a","claude","m-b",None,1000,0,0,0,"10.00",ts);
    insert_detail(&db,"wrong-b","codex","m-a",None,2000,0,0,0,"20.00",ts);
    let handle=server::start(db.clone(),0).await.unwrap();
    let http=reqwest::Client::builder().no_proxy().build().unwrap();
    let key=public_api::rotate_key(&db).unwrap();
    let mapping=json!({"claude":["m-a"],"codex":["m-b"]}).to_string();
    for path in ["summary","cycle"] {
        let url=format!("http://127.0.0.1:{}/api/v1/usage/{path}",handle.port);
        let response=http.get(&url).query(&[("appType","claude,codex"),("modelsByApp",mapping.as_str()),("groupBy","week")]).header("X-API-Key",&key).send().await.unwrap();
        assert_eq!(response.status(),200,"{path}");
        let body:Value=response.json().await.unwrap();
        assert_eq!(body["requests"],2);assert_eq!(body["totalTokens"],360);assert_eq!(body["costUsd"],3.0);
        assert_eq!(body["modelsByApp"],json!({"claude":["m-a"],"codex":["m-b"]}));
    }
    let url=format!("http://127.0.0.1:{}/api/v1/usage/summary",handle.port);
    let response=http.get(&url).query(&[("appType","claude,codex"),("model","m-a"),("modelsByApp",mapping.as_str())]).header("X-API-Key",&key).send().await.unwrap();
    assert_eq!(response.status(),400);
    for invalid in ["{}", "{\"claude\":[]}", "{\"other\":[\"m-a\"]}", "{\"claude\":[\"\"]}"] {
        let response=http.get(&url).query(&[("appType","claude,codex"),("modelsByApp",invalid)]).header("X-API-Key",&key).send().await.unwrap();
        assert_eq!(response.status(),400,"{invalid}");
    }
    let partial=json!({"claude":["m-a"]}).to_string();
    let response=http.get(&url).query(&[("appType","claude,codex"),("modelsByApp",partial.as_str())]).header("X-API-Key",&key).send().await.unwrap();
    let body:Value=response.json().await.unwrap();assert_eq!(body["requests"],3);assert_eq!(body["totalTokens"],2360);
    let injection=json!({"claude":["m-a') OR 1=1 --"],"codex":["missing"]}).to_string();
    let response=http.get(&url).query(&[("appType","claude,codex"),("modelsByApp",injection.as_str())]).header("X-API-Key",&key).send().await.unwrap();
    assert_eq!(response.status(),200);
    let body:Value=response.json().await.unwrap();assert_eq!(body["requests"],0);
    handle.stop();
}

#[tokio::test]
async fn per_app_model_filter_applies_to_archived_day_precision_checks() {
    let db=Arc::new(Database::memory().unwrap());
    let day=today().pred_opt().unwrap();
    insert_rollup(&db,day,"claude","archived-model",1,100,10,"1.00");
    let at=|hour| Local.timestamp_opt(local_ts(day,hour,0),0).single().unwrap().to_rfc3339();
    let handle=server::start(db.clone(),0).await.unwrap();
    let key=public_api::rotate_key(&db).unwrap();
    let http=reqwest::Client::builder().no_proxy().build().unwrap();
    let url=format!("http://127.0.0.1:{}/api/v1/usage/summary",handle.port);
    for (model, expected) in [("different-model",200),("archived-model",422)] {
        let selection=json!({"claude":[model]}).to_string();
        let response=http.get(&url).query(&[("appType","claude"),("modelsByApp",selection.as_str()),("startTime",&at(9)),("endTime",&at(18))]).header("X-API-Key",&key).send().await.unwrap();
        assert_eq!(response.status().as_u16(),expected);
    }
    handle.stop();
}

/// 设备集合：默认只查本机，可以指定配对设备。
#[tokio::test]
async fn http_query_device_selection_defaults_to_local() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    // 默认与显式 local 都是只查本机。
    for query in ["days=7&appType=claude", "days=7&appType=claude&devices=local"] {
        let response = http
            .get(format!("{url}?{query}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success(), "{query}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["scope"], "local", "{query}");
        assert_eq!(body["devices"], json!(["local"]), "{query}");
    }

    // 未配对的设备 → 400。
    let response = http
        .get(format!("{url}?days=7&appType=claude&devices=some-peer"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("设备不存在"));

    // 空段 → 400。
    let response = http
        .get(format!("{url}?days=7&appType=claude&devices="))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);

    handle.stop();
}

/// 周期端点的订阅以第一个平台为准。
#[tokio::test]
async fn http_cycle_endpoint_uses_the_first_platform_for_the_subscription() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"});
    let rule: observatory_usage_core::subscriptions::WeeklyRule = serde_json::from_value(rule_value.clone()).unwrap();
    let now = Local::now();
    let (cycle_start, _) = observatory_usage_core::subscriptions::weekly_cycle(&rule, now, 0).unwrap();
    let inside = (cycle_start + now.signed_duration_since(cycle_start) / 2).timestamp();
    insert_detail(&db, "cycle-a", "claude", "m-a", None, 100, 20, 0, 0, "1.00", inside);
    insert_detail(&db, "cycle-b", "codex", "m-b", None, 200, 40, 0, 0, "2.00", inside);
    insert_detail(&db, "cycle-c", "claude", "m-c", None, 400, 80, 0, 0, "4.00", inside);
    // 第一个平台有订阅，第二个没有。
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        rule_value,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycle", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    // claude 排在前面 → 用 claude 的订阅，周期算得出来。
    let response = http
        .get(format!("{url}?appType=claude,codex"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["appType"], json!(["claude", "codex"]));
    assert_eq!(body["apps"], json!(["claude", "codex"]));

    let response = http.get(format!("{url}?appType=claude,codex&model=m-a,m-b&groupBy=week"))
        .header("X-API-Key", &key).send().await.unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["appType"], json!(["claude", "codex"]));
    assert_eq!(body["model"], json!(["m-a", "m-b"]));
    assert_eq!(body["groupBy"], "week");
    assert_eq!(body["requests"], 2);
    assert_eq!(body["totalTokens"], 360);
    assert_eq!(body["costUsd"], 3.0);
    assert_eq!(body["buckets"].as_array().unwrap().iter().map(|b| b["totalTokens"].as_u64().unwrap()).sum::<u64>(), 360);

    // codex 排在前面 → codex 没有订阅 → 404。
    let response = http
        .get(format!("{url}?appType=codex,claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "SUBSCRIPTION_NOT_FOUND");

    handle.stop();
}

/// 周期序列：一次拿到本周期与前 N 个周期，给看板画柱状图。
#[tokio::test]
async fn http_cycle_series_returns_a_series_of_cycles() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    // 周期对齐到本地午夜：每个周期正好覆盖 7 个自然日。
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"}),
    );
    // 今天往前 10 天，每天一条明细，落满两个周期。
    let today = today();
    for offset in 0..10 {
        let date = today - chrono::Duration::days(offset);
        insert_detail(
            &db,
            &format!("d{offset}"),
            "claude",
            "m",
            None,
            100,
            20,
            0,
            0,
            "1.00",
            local_ts(date, 0, 1),
        );
    }

    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycles", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let response = http
        .get(format!("{url}?appType=claude&count=3"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();

    // 按自然日归集。
    assert_eq!(body["granularity"], "day");
    assert_eq!(body["appType"], json!(["claude"]));
    assert_eq!(body["scope"], "local");
    assert_eq!(body["subscription"]["name"], "Claude Max");
    assert_eq!(
        body["subscription"]["rule"],
        json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"})
    );

    let cycles = body["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 3, "应返回 3 个周期");
    // 当前周期 index=0，往前依次是 -1、-2。
    assert_eq!(cycles[0]["index"], 0);
    assert_eq!(cycles[1]["index"], -1);
    assert_eq!(cycles[2]["index"], -2);
    // 周期窗口首尾相接。
    assert_eq!(cycles[1]["end"], cycles[0]["start"]);
    assert_eq!(cycles[2]["end"], cycles[1]["start"]);
    // 每个周期正好 7 个自然日。
    for cycle in cycles {
        let first = NaiveDate::parse_from_str(cycle["startDate"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let last = NaiveDate::parse_from_str(cycle["endDate"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        assert_eq!((last - first).num_days(), 6, "周期应覆盖 7 个自然日");
    }
    // 用量：最近两个周期各有明细（7 天 + 3 天）。
    assert_eq!(cycles[0]["requests"], 5);
    assert_eq!(cycles[1]["requests"], 5);
    assert_eq!(cycles[2]["requests"], 0);
    assert_eq!(cycles[0]["totalTokens"], 600);
    assert_eq!(cycles[0]["costUsd"], 5.0);
    // 当前周期还没结束。
    assert_eq!(cycles[0]["complete"], false);
    assert_eq!(cycles[1]["complete"], true);

    // 默认返回 8 个周期。
    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["cycles"].as_array().unwrap().len(), 8);

    // 参数错误。
    for params in ["", "appType=claude&count=0", "appType=claude&count=abc", "appType=claude&unknown=1"] {
        let response = http
            .get(format!("{url}?{params}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{params}");
    }
    // 没有订阅 → 404。
    let response = http
        .get(format!("{url}?appType=codex"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "SUBSCRIPTION_NOT_FOUND");

    handle.stop();
}

/// 周期序列按自然日归集，所以不会因为切到归档日而报 422。
#[tokio::test]
async fn http_cycle_series_never_hits_the_precision_error() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    );
    // 把日汇总放在某个周期的**边界日**上——只有边界日会被秒级区间切开。
    let rule: observatory_usage_core::subscriptions::WeeklyRule = serde_json::from_value(
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    )
    .unwrap();
    let (boundary_start, _) =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, Local::now(), -6).unwrap();
    let old = boundary_start.date_naive();
    insert_rollup(&db, old, "claude", "m", 3, 30, 6, "0.30");

    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycles", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    // 单周期查询在同样的区间上会 422。
    let response = http
        .get(format!(
            "http://127.0.0.1:{}/api/v1/usage/cycle?appType=claude&offset=-6",
            handle.port
        ))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 422);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "RANGE_PRECISION_UNAVAILABLE");

    // 序列端点按自然日归集，照样能返回。
    let response = http
        .get(format!("{url}?appType=claude&count=8"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    let cycles = body["cycles"].as_array().unwrap();
    assert_eq!(cycles.len(), 8);
    // 那条归档日汇总被算进了它所属的周期。
    let total: u64 = cycles
        .iter()
        .map(|cycle| cycle["requests"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(total, 3);

    handle.stop();
}

#[tokio::test]
async fn http_auth_validation_revocation_and_read_only_key() {    let db = Arc::new(Database::memory().unwrap());
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
        "groupBy=year",
        "days=1&startDate=2026-01-01",
        "days=7&days=8",
        "startTime=2026-10-31T09:00:00",
    ] {
        let response = http
            .get(format!("{url}?{params}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{params}");
        let body: Value = response.json().await.unwrap();
        assert!(body["error"].is_string(), "{params}");
        assert!(body["code"].is_string(), "{params}");
    }
    // 403 与稳定错误码。
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();
    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "APP_NOT_SHARED");
    let scan = http
        .post(format!("http://127.0.0.1:{}/api/lan/scan", handle.port))
        .header("X-API-Key", &key)
        .json(&json!({"key":"fixture"}))
        .send()
        .await
        .unwrap();
    assert_eq!(scan.status(), 403);
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

/// 新参数在 HTTP 层可用，且重复参数被拒绝。
#[tokio::test]
async fn http_extended_parameters_and_duplicate_rejection() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let response = http
        .get(format!(
            "{url}?days=7&appType=claude&groupBy=week&model=m"
        ))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["groupBy"], "week");
    assert_eq!(body["model"], json!(["m"]));
    assert_eq!(body["modelFilterBasis"], "effectivePricingModel");
    assert!(!body["buckets"].as_array().unwrap().is_empty());

    let response = http
        .get(format!("{url}?days=7&days=8"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "INVALID_PARAMETERS");
    assert!(body["error"].as_str().unwrap().contains("重复"));

    handle.stop();
}

/// 后台扫描写入期间，同一响应里的总量、`daily` 与 `buckets` 仍然一致。
#[test]
fn summary_stays_internally_consistent_while_the_database_is_written() {
    use std::sync::atomic::{AtomicBool, Ordering};

    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let stop = Arc::new(AtomicBool::new(false));
    let writer = {
        let db = db.clone();
        let stop = stop.clone();
        std::thread::spawn(move || {
            let mut counter = 0i64;
            let day = Local::now().date_naive().pred_opt().unwrap();
            let ts = Local
                .from_local_datetime(&day.and_hms_opt(9, 0, 0).unwrap())
                .earliest()
                .unwrap()
                .timestamp();
            while !stop.load(Ordering::Relaxed) {
                counter += 1;
                let _ = db.execute_batch(&format!(
                    "INSERT INTO proxy_request_logs(request_id,provider_id,app_type,model,input_tokens,output_tokens,cache_read_tokens,cache_creation_tokens,input_token_semantics,total_cost_usd,latency_ms,status_code,created_at,data_source) VALUES ('noise-{counter}','p','claude','m',{counter},1,0,0,2,'0.01',1000,200,{ts},'fixture');"
                ));
            }
        })
    };

    for _ in 0..60 {
        let result = public_api::summary(&db, query(7, None, Some("claude"))).unwrap();
        let total = result["requests"].as_u64().unwrap();
        let daily: u64 = result["daily"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["requests"].as_u64().unwrap())
            .sum();
        let buckets: u64 = result["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["requests"].as_u64().unwrap())
            .sum();
        assert_eq!(total, daily, "顶层与 daily 不一致");
        assert_eq!(total, buckets, "顶层与 buckets 不一致");
    }

    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
}

/// 本地午夜的取值规则是确定的，与操作系统时区无关。
///
/// 夏令时切换日会撞上两种边界：回拨让 00:00 出现两次，前拨让 00:00 根本不
/// 存在。前者取较早时刻，后者必须报错而不是静默平移。
#[test]
fn local_midnight_resolution_rules() {
    let date = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    let early = Local::now();
    let late = Local::now();

    // 唯一时刻：原样返回。
    let single = chrono::LocalResult::Single(early);
    assert_eq!(
        public_api::resolve_local_midnight(single, date).unwrap(),
        early
    );

    // 重复午夜：取较早的那一个。
    let ambiguous = chrono::LocalResult::Ambiguous(early, late);
    assert_eq!(
        public_api::resolve_local_midnight(ambiguous, date).unwrap(),
        early
    );

    // 不存在的午夜：400 INVALID_LOCAL_BOUNDARY。
    let missing = chrono::LocalResult::None;
    let error = public_api::resolve_local_midnight(missing, date).unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.code, "INVALID_LOCAL_BOUNDARY");
}

/// 在有夏令时的时区下，跑一遍完整的「重复午夜 / 不存在的午夜」路径。
///
/// `chrono::Local` 在 Unix 上跟随 `TZ` 环境变量，可以用子进程切换时区；
/// 在 Windows 上它走系统 API、忽略 `TZ`，因此这条路径由
/// [`local_midnight_resolution_rules`] 的确定性用例保证。
#[test]
fn local_midnight_rules_hold_under_other_timezones() {
    const TZ_ENV: &str = "OBSERVATORY_TEST_TZ";
    if std::env::var(TZ_ENV).is_ok() {
        local_midnight_rules_in_this_timezone();
        return;
    }

    let parent_offset = Local::now().offset().local_minus_utc();
    let mut tz_honoured = false;
    let mut ambiguous_total = 0usize;
    let mut missing_total = 0usize;
    for zone in [
        "America/Havana",
        "America/Santiago",
        "Asia/Amman",
        "Asia/Damascus",
        "Africa/Cairo",
        "America/New_York",
        "Europe/Berlin",
        "Australia/Lord_Howe",
        "Pacific/Apia",
        "America/Sao_Paulo",
    ] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "local_midnight_rules_hold_under_other_timezones",
                "--nocapture",
            ])
            .env(TZ_ENV, zone)
            .env("TZ", zone)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "时区 {zone} 下的本地午夜断言失败: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        let marker = stdout
            .lines()
            .find_map(|line| line.strip_prefix("MIDNIGHT_RULES "))
            .unwrap_or_else(|| panic!("子进程没有报告午夜统计: {stdout}"));
        eprintln!("MIDNIGHT_ZONE {zone} {marker}");
        // `marker` 已经去掉了 "MIDNIGHT_RULES " 前缀，第一个字段就是 offset=。
        let fields: Vec<&str> = marker.split_whitespace().collect();
        let field = |name: &str| -> i64 {
            fields
                .iter()
                .find_map(|part| part.strip_prefix(name))
                .and_then(|value| value.parse().ok())
                .unwrap_or(0)
        };
        if field("offset=") != parent_offset as i64 {
            tz_honoured = true;
        }
        ambiguous_total += field("ambiguous=") as usize;
        missing_total += field("missing=") as usize;
    }

    if !tz_honoured {
        eprintln!(
            "跳过：本平台的 chrono::Local 不跟随 TZ 环境变量（Windows 走系统 API），\
             夏令时规则由 local_midnight_resolution_rules 覆盖"
        );
        return;
    }
    assert!(
        ambiguous_total + missing_total > 0,
        "时区可切换，但没有任何一天撞上午夜边界，断言空转"
    );
}

fn local_midnight_rules_in_this_timezone() {
    let db = Database::memory().unwrap();
    let today = Local::now().date_naive();
    let mut ambiguous = Vec::new();
    let mut missing = Vec::new();
    for offset in 0..800 {
        let date = today - Duration::days(offset);
        let naive = date.and_hms_opt(0, 0, 0).unwrap();
        match Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(_) => {}
            chrono::LocalResult::Ambiguous(_, _) => ambiguous.push(date),
            chrono::LocalResult::None => missing.push(date),
        }
    }
    println!(
        "MIDNIGHT_RULES offset={} ambiguous={} missing={}",
        Local::now().offset().local_minus_utc(),
        ambiguous.len(),
        missing.len()
    );

    // 重复午夜：取较早时刻，查询成功且起点落在较早的那个午夜。
    for date in ambiguous.iter().take(3) {
        let expected = Local
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap();
        let result = public_api::summary(
            &db,
            public_api::SummaryQuery {
                start_date: Some(date.to_string()),
                end_date: Some(date.to_string()),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("重复午夜 {date} 应查询成功: {error:?}"));
        assert_eq!(result["range"]["start"], expected.to_rfc3339());
    }

    // 不存在的午夜：返回 400 INVALID_LOCAL_BOUNDARY，不静默平移。
    for date in missing.iter().take(3) {
        let error = public_api::summary(
            &db,
            public_api::SummaryQuery {
                start_date: Some(date.to_string()),
                end_date: Some(date.to_string()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.status, 400);
        assert_eq!(error.code, "INVALID_LOCAL_BOUNDARY");
    }
}

/// 读取 `docs/examples` 下的示例文件。
fn read_example(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("examples")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()))
}

/// 文档里的示例 JSON 必须能解析。
#[test]
fn documented_examples_are_valid_json() {
    for name in [
        "usage-summary.example.json",
        "claude-tinypane.charts.json",
        "postman-observatory-usage.collection.json",
        "postman-observatory-usage.environment.json",
    ] {
        let value: Value = serde_json::from_str(&read_example(name))
            .unwrap_or_else(|error| panic!("{name} 不是合法 JSON: {error}"));
        assert!(value.is_object() || value.is_array());
    }
}

/// Postman 集合必须保持可导入，且每个请求都指向同一个只读接口。
#[test]
fn postman_collection_is_importable_and_consistent() {
    let collection: Value =
        serde_json::from_str(&read_example("postman-observatory-usage.collection.json")).unwrap();
    assert_eq!(
        collection["info"]["schema"],
        "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
    );
    assert!(collection["info"]["name"].as_str().unwrap().len() > 4);

    let variables = collection["variable"].as_array().unwrap();
    for key in ["baseUrl", "apiKey"] {
        assert!(
            variables.iter().any(|value| value["key"] == key),
            "集合缺少变量 {key}"
        );
    }

    // 递归收集所有请求（跳过文件夹）。
    fn collect(items: &Value, out: &mut Vec<Value>) {
        for item in items.as_array().cloned().unwrap_or_default() {
            if item.get("item").is_some() {
                collect(&item["item"], out);
            } else {
                out.push(item);
            }
        }
    }
    let mut requests = Vec::new();
    collect(&collection["item"], &mut requests);
    assert!(requests.len() >= 10, "请求数量偏少: {}", requests.len());

    for request in &requests {
        let name = request["name"].as_str().unwrap();
        assert_eq!(request["request"]["method"], "GET", "{name}");
        // 只允许指向这三个只读查询端点。
        let path = request["request"]["url"]["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("/");
        assert!(
            path == "api/v1/usage/summary"
                || path == "api/v1/usage/cycle"
                || path == "api/v1/usage/cycles",
            "{name} 的路径不对: {path}"
        );
        // 周期端点必须指定 Agent 平台——除了专门演示「缺少 appType」的那条。
        if path.starts_with("api/v1/usage/cycle") && !name.contains("缺少 appType") {
            let has_app = request["request"]["url"]["query"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["key"] == "appType" && item["disabled"] != true);
            assert!(has_app, "{name} 缺少 appType 参数");
        }
        let has_key = request["request"]["header"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header["key"] == "X-API-Key");
        if name.contains("缺少或已撤销") {
            assert!(!has_key, "{name} 是「缺少密钥」场景，不应带头");
        } else {
            assert!(has_key, "{name} 缺少 X-API-Key 请求头");
        }
    }

    let environment: Value =
        serde_json::from_str(&read_example("postman-observatory-usage.environment.json")).unwrap();
    for key in ["baseUrl", "apiKey"] {
        assert!(
            environment["values"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value["key"] == key),
            "环境文件缺少变量 {key}"
        );
    }
}

/// 集合里的每条请求都要能对着真实服务跑通，并得到预期状态码。
///
/// 这条用例把「文档」和「实现」绑在一起：集合改了路径、参数或错误码，
/// 这里就会失败。
#[tokio::test]
async fn postman_collection_requests_match_the_live_endpoint() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    // 只共享部分应用，好让「未允许共享」那条真的返回 403。
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["claude".into(), "claude-desktop".into(), "codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();

    // 周期端点要靠订阅记录算周期：给 claude 一条，codex 故意不给。
    //
    // 周期对齐到本地午夜：这样周期的第一天是整天覆盖的，不会切到只剩日汇总
    // 的历史日期上，断言不随运行当天是星期几而变化。
    let (window_start, window_end) = year_window();
    save_subscription(
        &db,
        "claude",
        &window_start,
        &window_end,
        json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"}),
    );

    let handle = server::start(db.clone(), 0).await.unwrap();
    let base = format!("http://127.0.0.1:{}", handle.port);
    let key = public_api::rotate_key(&db).unwrap();

    // 变量取值对齐种子数据：日汇总落在昨天。
    let yesterday = today().pred_opt().unwrap();
    let day_before = yesterday.pred_opt().unwrap();
    let at = |date: NaiveDate, hour| Local.timestamp_opt(local_ts(date, hour, 0), 0).single().unwrap().to_rfc3339();
    let variables = vec![
        ("baseUrl", base.clone()),
        ("apiKey", key.clone()),
        ("startDate", day_before.to_string()),
        ("endDate", yesterday.to_string()),
        (
            "startTime",
            at(today(), 9),
        ),
        (
            "endTime",
            at(today(), 18),
        ),
        ("startTimeNaive", format!("{yesterday}T09:00:00")),
        ("endTimeNaive", format!("{yesterday}T18:00:00")),
        (
            "dayStart",
            at(yesterday, 0),
        ),
        (
            "dayEnd",
            at(yesterday.succ_opt().unwrap(), 0),
        ),
        ("archivedStartTime", at(yesterday, 9)),
        ("archivedEndTime", at(yesterday, 18)),
        ("model", "m".to_string()),
        // 多个模型：逗号分隔。种子数据的模型是 m，m2 没有记录。
        ("models", "m,m2".to_string()),
        // 按平台分别指定模型。
        (
            "modelsByApp",
            "{\"claude\":[\"m\"],\"codex\":[\"m\"]}".to_string(),
        ),
    ];

    let collection: Value =
        serde_json::from_str(&read_example("postman-observatory-usage.collection.json")).unwrap();
    let mut requests = Vec::new();
    fn collect(items: &Value, out: &mut Vec<Value>) {
        for item in items.as_array().cloned().unwrap_or_default() {
            if item.get("item").is_some() {
                collect(&item["item"], out);
            } else {
                out.push(item);
            }
        }
    }
    collect(&collection["item"], &mut requests);

    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    for request in &requests {
        let name = request["name"].as_str().unwrap();
        // 路径来自集合本身（`api/v1/usage/summary` 或 `api/v1/usage/cycle`）。
        let path = request["request"]["url"]["path"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect::<Vec<_>>()
            .join("/");
        let mut url = reqwest::Url::parse(&format!("{base}/{path}")).unwrap();
        {
            let mut pairs = url.query_pairs_mut();
            for parameter in request["request"]["url"]["query"].as_array().unwrap() {
                if parameter["disabled"] == true { continue; }
                let mut value = parameter["value"].as_str().unwrap().to_owned();
                for (key, replacement) in &variables { value = value.replace(&format!("{{{{{key}}}}}"), replacement); }
                assert!(!value.contains("{{"), "{name} 还有未替换的变量");
                pairs.append_pair(parameter["key"].as_str().unwrap(), &value);
            }
        }

        // 期望状态码由请求名里的前缀决定。
        let expected = if name.starts_with("400") {
            400
        } else if name.starts_with("401") {
            401
        } else if name.starts_with("403") {
            403
        } else if name.starts_with("404") {
            404
        } else if name.starts_with("422") {
            422
        } else {
            200
        };

        let mut builder = http.get(url.clone());
        // 只有「缺少密钥」那条不带 X-API-Key。
        if !name.contains("缺少或已撤销") {
            builder = builder.header("X-API-Key", &key);
        }
        let response = builder.send().await.unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected,
            "{name} 的状态码不对: {url}"
        );
        if expected >= 400 {
            let body: Value = response.json().await.unwrap();
            assert!(body["error"].is_string(), "{name} 缺少 error 字段");
            assert!(body["code"].is_string(), "{name} 缺少 code 字段");
        }
    }

    handle.stop();
}

#[tokio::test]
async fn dashboard_http_returns_billing_cycles_and_model_breakdown() {
    let db = Arc::new(Database::memory().unwrap());
    let start = (today() - Duration::days(45)).to_string();
    let end = (today() + Duration::days(50)).to_string();
    save_subscription(&db, "claude", &start, &end, json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}));
    let timestamp = Local::now().timestamp() - 5;
    insert_detail(&db, "dashboard-model-a", "claude", "model-a", None, 100, 20, 30, 0, "1.25", timestamp);
    insert_detail(&db, "dashboard-model-b", "claude", "model-b", None, 200, 10, 0, 0, "2.00", timestamp);
    let handle = server::start(db.clone(),0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/dashboard/usage",handle.port);
    let http=reqwest::Client::builder().no_proxy().build().unwrap();
    let denied=http.post(&url).json(&json!({"appType":"claude"})).send().await.unwrap();
    assert_eq!(denied.status(),401);
    let response=http.post(&url).header("X-API-Key",key).json(&json!({"appType":"claude"})).send().await.unwrap();
    assert_eq!(response.status(),200);
    let body:Value=response.json().await.unwrap();
    assert_eq!(body["billingTotal"]["totalTokens"],360);
    assert_eq!(body["billingAggregations"]["byAppAndModel"].as_array().unwrap().len(),2);
    assert!(!body["quotaCycles"].as_array().unwrap().is_empty());
    assert!(body["selectedQuotaCycle"]["usage"]["daily"].is_array());
    handle.stop();
}

#[tokio::test]
async fn dashboard_models_uses_platform_subscription_and_aggregates_shared_apps() {
    let db=Arc::new(Database::memory().unwrap());
    let start=(today()-Duration::days(40)).to_string();
    let end=(today()+Duration::days(40)).to_string();
    save_subscription(&db,"claude",&start,&end,json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}));
    let ts=Local::now().timestamp()-5;
    insert_detail(&db,"cross-a","claude","shared-model",None,100,0,0,0,"1",ts);
    insert_detail(&db,"cross-b","codex","shared-model",None,200,0,0,0,"2",ts);
    insert_detail(&db,"cross-c","codex","other-model",None,500,0,0,0,"5",ts);
    let handle=server::start(db.clone(),0).await.unwrap();
    let key=public_api::rotate_key(&db).unwrap();
    let http=reqwest::Client::builder().no_proxy().build().unwrap();
    let url=format!("http://127.0.0.1:{}/api/v1/dashboard/models",handle.port);
    let call=|body:Value| http.post(&url).header("X-API-Key",&key).json(&body).send();
    let matched:Value=call(json!({"platformCode":"claude","modelIds":["shared-model"]})).await.unwrap().json().await.unwrap();
    assert_eq!(matched["scope"]["subscriptionApp"],"claude");
    assert_eq!(matched["billingTotal"]["totalTokens"],300);
    let default:Value=call(json!({"platformCode":"claude"})).await.unwrap().json().await.unwrap();
    assert_eq!(default["billingTotal"]["totalTokens"],100);
    let empty:Value=call(json!({"platformCode":"claude","modelIds":[]})).await.unwrap().json().await.unwrap();
    assert_eq!(empty["billingTotal"]["totalTokens"],100);
    let invalid=call(json!({"platformCode":"claude","modelIds":[""]})).await.unwrap();
    assert_eq!(invalid.status(),400);
    handle.stop();
}
