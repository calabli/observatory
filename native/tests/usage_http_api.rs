use chrono::{Datelike, Duration, Local, NaiveDate, TimeZone};
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

/// The unix seconds of a point in time on a given local day.
fn local_ts(date: NaiveDate, hour: u32, minute: u32) -> i64 {
    Local
        .from_local_datetime(&date.and_hms_opt(hour, minute, 0).unwrap())
        .earliest()
        .unwrap()
        .timestamp()
}

/// Insert one detail row.
#[allow(clippy::too_many_arguments)] // Fixture: the arguments are the columns of one row.
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

/// Insert one daily rollup row.
#[allow(clippy::too_many_arguments)] // Fixture: the arguments are the columns of one row.
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
    Local
        .timestamp_opt(local_ts(date, hour, 0), 0)
        .single()
        .unwrap()
        .to_rfc3339()
}

#[test]
fn omitted_end_time_defaults_to_current_second() {
    let db = Database::memory().unwrap();
    let now = Local::now().timestamp();
    insert_detail(
        &db,
        "inside",
        "claude",
        "m",
        None,
        10,
        2,
        0,
        0,
        "0.25",
        now - 60,
    );
    insert_detail(
        &db,
        "future",
        "claude",
        "m",
        None,
        90,
        2,
        0,
        0,
        "9.00",
        now + 3600,
    );
    let since = Local
        .timestamp_opt(now - 120, 0)
        .single()
        .unwrap()
        .to_rfc3339();
    let before = Local::now().timestamp();
    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_time: Some(since.clone()),
            app_type: Some("claude".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let after = Local::now().timestamp();
    let end =
        chrono::DateTime::parse_from_rfc3339(result["range"]["endExclusive"].as_str().unwrap())
            .unwrap();
    assert!((before..=after).contains(&end.timestamp()));
    assert_eq!(end.timestamp_subsec_nanos(), 0);
    assert_eq!(result["requests"], 1);
    assert_eq!(result["costUsd"], 0.25);
    let blank = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_time: Some(since.clone()),
            end_time: Some("  ".into()),
            app_type: Some("claude".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(blank["requests"], 1);
    let fixed = public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_time: Some(since),
            end_time: Some(
                Local
                    .timestamp_opt(now - 90, 0)
                    .single()
                    .unwrap()
                    .to_rfc3339(),
            ),
            app_type: Some("claude".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(fixed["requests"], 0);
    assert!(public_api::summary(
        &db,
        public_api::SummaryQuery {
            start_time: Some(
                Local
                    .timestamp_opt(now + 3600, 0)
                    .single()
                    .unwrap()
                    .to_rfc3339()
            ),
            ..Default::default()
        }
    )
    .is_err());
}

#[tokio::test]
async fn http_accepts_start_time_without_end_time() {
    let db = Arc::new(Database::memory().unwrap());
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let since = (Local::now() - Duration::minutes(5))
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = http
        .get(&url)
        .query(&[("startTime", since.as_str()), ("appType", "claude")])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let result: Value = response.json().await.unwrap();
    assert!(result["range"]["endExclusive"].is_string());
    let response = http
        .get(&url)
        .query(&[("startTime", since.as_str()), ("endTime", "")])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let response = http
        .get(&url)
        .query(&[("endTime", since.as_str())])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(
        response.json::<Value>().await.unwrap()["code"],
        "CONFLICTING_TIME_PARAMETERS"
    );
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

/// All top-level fields and paths of v0.1.30 are preserved; new fields are only
/// appended.
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

/// When `days` and `startDate`/`endDate` describe the same interval the results
/// must be exactly identical.
#[test]
fn date_range_matches_equivalent_days_window() {
    let db = Database::memory().unwrap();
    seed(&db);
    let end = today().pred_opt().unwrap();
    let start = end.pred_opt().unwrap().pred_opt().unwrap();
    let by_days =
        public_api::summary(&db, query(3, Some(end.to_string()), Some("claude"))).unwrap();
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

/// The interval is left-closed and right-open: the start is included, the end
/// is not.
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
    // From 09:00 to 18:00: the 09:00 and 12:00 rows are included, the 18:00
    // one is not.
    let result = public_api::summary(&db, base(9, 18)).unwrap();
    assert_eq!(result["requests"], 2);
    assert_eq!(result["costUsd"], 0.2);
    // The start itself is included.
    let inclusive = public_api::summary(&db, base(12, 18)).unwrap();
    assert_eq!(inclusive["requests"], 1);
}

/// A second-level interval that cuts a day where only a daily rollup is left
/// has to raise an error instead of returning truncated numbers.
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

    // When the whole day is covered it can be counted in full, without an error.
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

/// Days that have detail rows are not affected by the daily rollup precision
/// limit.
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

/// The top level, `daily` and `buckets` must come from the same batch of data
/// and add up to the same total.
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

/// The ISO week number uses the year the week belongs to; a week that spans the
/// year boundary is not glued to a calendar year.
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
    // 2025-12-21 belongs to 2025-W51; the week that spans the year boundary is
    // recorded as 2026-W01, the ISO year it belongs to.
    assert_eq!(
        periods,
        vec!["2025-W51", "2025-W52", "2026-W01", "2026-W02"]
    );
}

/// The period identifier, the boundaries and the zero filling of the month
/// grouping.
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
    // Empty periods are zero filled.
    assert_eq!(buckets[1]["requests"], 0);
    assert_eq!(buckets[1]["costUsd"], 0.0);
    // The first and the last bucket are the intersection of the natural period
    // with the query interval.
    assert!(buckets[0]["start"]
        .as_str()
        .unwrap()
        .starts_with("2026-01-15"));
    assert!(buckets[3]["endExclusive"]
        .as_str()
        .unwrap()
        .starts_with("2026-04-11"));
}

/// Leap days and months of different lengths are split correctly.
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

/// The model filter matches on the effective pricing model, consistent with
/// what the page shows.
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
    // When nothing matches, zero values are returned instead of an error.
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

/// Claude Code and Claude Desktop are told apart exactly.
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

/// The three modes of the time parameters are mutually exclusive; a conflict
/// yields a stable error code.
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

/// A missing time zone, a non-whole second and an unsupported grouping all
/// have to be rejected.
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

/// An app that is not shared returns 403; when the shared set is empty zero
/// values are returned.
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

/// The timestamps in the response carry the local offset, and both ends of
/// `range` carry their own.
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

/// Write one subscription record.
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

/// Covers one year back and one year forward, so the cycle lies entirely inside
/// the validity window and is not clamped.
fn year_window() -> (String, String) {
    let start = today() - chrono::Duration::days(365);
    let end = today() + chrono::Duration::days(365);
    (
        start.format("%Y-%m-%d").to_string(),
        end.format("%Y-%m-%d").to_string(),
    )
}

/// The cycle query endpoint: the body is exactly the same as the summary one,
/// and the interval is computed by the server from the subscription cycle.
#[tokio::test]
async fn http_cycle_endpoint_matches_the_summary_contract() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"});
    save_subscription(&db, "claude", &start, &end, rule_value.clone());

    // Put one detail row in the part of the current cycle that has "already
    // passed", so the assertions do not depend on "what time it is today".
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

    // No key -> 401.
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

    // The fields of the body are exactly the same as in summary.
    let range_start =
        chrono::DateTime::parse_from_rfc3339(cycle["range"]["start"].as_str().unwrap()).unwrap();
    let range_end =
        chrono::DateTime::parse_from_rfc3339(cycle["range"]["endExclusive"].as_str().unwrap())
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

    // The start of the cycle matches the subscription rule; the end is cut at
    // the current moment (usage in the future does not exist yet).
    assert_eq!(range_start, cycle_start);
    assert!(range_end <= cycle_end);
    assert!(range_end > range_start);
    let now_second = Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let now_second = chrono::DateTime::parse_from_rfc3339(&now_second).unwrap();
    assert!(
        (range_end - now_second).num_seconds().abs() <= 60,
        "终点应截到请求时刻"
    );

    // Aggregation basis: the detail row inside the cycle is counted in, and the
    // sum of the four token kinds equals the total.
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

    // The subscription cycle stays the same, so the same batch of data for the
    // selected models can be grouped by week or by month.
    for group in ["week", "month"] {
        let response = http
            .get(format!("{url}?appType=claude&model=m&groupBy={group}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let grouped: Value = response.json().await.unwrap();
        assert_eq!(grouped["groupBy"], group);
        assert_eq!(grouped["requests"], cycle["requests"]);
        assert_eq!(grouped["totalTokens"], cycle["totalTokens"]);
        assert_eq!(grouped["costUsd"], cycle["costUsd"]);
        assert_eq!(grouped["model"], json!(["m"]));
    }

    // A missing appType, an unknown parameter and a duplicated parameter all
    // have to be 400.
    for params in [
        "",
        "appType=",
        "unknown=1",
        "appType=claude&appType=codex",
        "offset=999",
        "appType=claude&groupBy=year",
    ] {
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

    // An app without a subscription -> 404.
    let response = http
        .get(format!("{url}?appType=codex"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["code"], "SUBSCRIPTION_NOT_FOUND");

    // An app that is not shared -> 403, and it does not leak whether a
    // subscription exists.
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

/// The cycle is clamped to the validity window of the subscription: the start
/// takes the later one, the end the earlier one.
#[tokio::test]
async fn http_cycle_endpoint_clamps_to_the_subscription_window() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    // The seeded daily rollup for "yesterday" makes a second-level window that
    // cuts exactly at yesterday return 422 (it hits when the test runs on a
    // Friday); only the window clamping matters here, no daily rollup data is
    // needed.
    db.execute_batch("DELETE FROM usage_daily_rollups").unwrap();
    // The validity window is only one day around today, so the cycle gets
    // narrowed.
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
    let status = response.status();
    let body: Value = response.json().await.unwrap();
    assert!(status.is_success(), "status {status} body {body}");
    let range_start =
        chrono::DateTime::parse_from_rfc3339(body["range"]["start"].as_str().unwrap()).unwrap();
    let range_end =
        chrono::DateTime::parse_from_rfc3339(body["range"]["endExclusive"].as_str().unwrap())
            .unwrap();
    // The end is not later than midnight of the expiry date.
    let expiry = Local
        .from_local_datetime(&today().succ_opt().unwrap().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap();
    assert!(range_end <= expiry, "结尾应收敛到到期时刻");
    // The start is not earlier than midnight of the start date.
    let begins = Local
        .from_local_datetime(&today().pred_opt().unwrap().and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .unwrap();
    assert!(range_start >= begins, "开头应收敛到开始时刻");
    // After clamping it is still a non-empty interval.
    assert!(range_end > range_start);

    handle.stop();
}

/// The previous cycle can be walked back following the subscription rule; a
/// cycle that has not started yet returns 404.
#[tokio::test]
async fn http_cycle_endpoint_walks_back_and_forward() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    // Same as above: a daily rollup whose date falls on the partial edge of the
    // second-level window would be 422, which is not needed here.
    db.execute_batch("DELETE FROM usage_daily_rollups").unwrap();
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

    // The start of the current cycle matches the one computed from the
    // subscription rule, and the end is cut at the current moment.
    let expected =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, Local::now(), 0).unwrap();
    assert_eq!(start0, expected.0);
    assert!(end0 <= expected.1);
    assert!(end0 > start0);

    // The end of the previous cycle is exactly the start of the current one, and
    // adjacent cycles are exactly seven days apart.
    assert_eq!(end1, start0);
    assert_eq!((start0 - start1).num_days(), 7);

    // The next cycle has not started yet, so there is no interval to aggregate
    // -> 404.
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

/// Several platforms and several models: comma separated, the results are
/// aggregated together.
#[tokio::test]
async fn http_query_accepts_multiple_apps_and_models() {
    let db = Arc::new(Database::memory().unwrap());
    let today = today();
    let ts = local_ts(today, 12, 0);
    // claude uses the model m-a, codex uses the model m-b.
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

    // Both platforms together.
    let response = get("days=1&appType=claude,codex").await;
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["appType"], json!(["claude", "codex"]));
    assert_eq!(body["apps"], json!(["claude", "codex"]));
    assert_eq!(body["requests"], 3);
    assert_eq!(body["totalTokens"], 840);

    // Two models selected.
    let response = get("days=1&appType=claude,codex&model=m-a,m-b").await;
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["model"], json!(["m-a", "m-b"]));
    assert_eq!(body["requests"], 2);
    assert_eq!(body["totalTokens"], 360);

    // Duplicated apps and models are deduplicated.
    let response = get("days=1&appType=claude,claude&model=m-a,m-a").await;
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["apps"], json!(["claude"]));
    assert_eq!(body["model"], json!(["m-a"]));

    // An empty segment and an unknown identifier both have to be 400.
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
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"});
    save_subscription(&db, "claude", &start, &end, rule_value.clone());
    let rule = serde_json::from_value(rule_value).unwrap();
    let now = Local::now();
    let (cycle_start, _) =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, now, 0).unwrap();
    let ts = (cycle_start + now.signed_duration_since(cycle_start) / 2).timestamp();
    insert_detail(&db, "a1", "claude", "m-a", None, 100, 20, 0, 0, "1.00", ts);
    insert_detail(&db, "a2", "codex", "m-b", None, 200, 40, 0, 0, "2.00", ts);
    insert_detail(
        &db, "wrong-a", "claude", "m-b", None, 1000, 0, 0, 0, "10.00", ts,
    );
    insert_detail(
        &db, "wrong-b", "codex", "m-a", None, 2000, 0, 0, 0, "20.00", ts,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let mapping = json!({"claude":["m-a"],"codex":["m-b"]}).to_string();
    for path in ["summary", "cycle"] {
        let url = format!("http://127.0.0.1:{}/api/v1/usage/{path}", handle.port);
        let response = http
            .get(&url)
            .query(&[
                ("appType", "claude,codex"),
                ("modelsByApp", mapping.as_str()),
                ("groupBy", "week"),
            ])
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["requests"], 2);
        assert_eq!(body["totalTokens"], 360);
        assert_eq!(body["costUsd"], 3.0);
        assert_eq!(
            body["modelsByApp"],
            json!({"claude":["m-a"],"codex":["m-b"]})
        );
    }
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let response = http
        .get(&url)
        .query(&[
            ("appType", "claude,codex"),
            ("model", "m-a"),
            ("modelsByApp", mapping.as_str()),
        ])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    for invalid in [
        "{}",
        "{\"claude\":[]}",
        "{\"other\":[\"m-a\"]}",
        "{\"claude\":[\"\"]}",
    ] {
        let response = http
            .get(&url)
            .query(&[("appType", "claude,codex"), ("modelsByApp", invalid)])
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{invalid}");
    }
    let partial = json!({"claude":["m-a"]}).to_string();
    let response = http
        .get(&url)
        .query(&[
            ("appType", "claude,codex"),
            ("modelsByApp", partial.as_str()),
        ])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["requests"], 3);
    assert_eq!(body["totalTokens"], 2360);
    let injection = json!({"claude":["m-a') OR 1=1 --"],"codex":["missing"]}).to_string();
    let response = http
        .get(&url)
        .query(&[
            ("appType", "claude,codex"),
            ("modelsByApp", injection.as_str()),
        ])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["requests"], 0);
    handle.stop();
}

#[tokio::test]
async fn per_app_model_filter_applies_to_archived_day_precision_checks() {
    let db = Arc::new(Database::memory().unwrap());
    let day = today().pred_opt().unwrap();
    insert_rollup(&db, day, "claude", "archived-model", 1, 100, 10, "1.00");
    let at = |hour| {
        Local
            .timestamp_opt(local_ts(day, hour, 0), 0)
            .single()
            .unwrap()
            .to_rfc3339()
    };
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    for (model, expected) in [("different-model", 200), ("archived-model", 422)] {
        let selection = json!({"claude":[model]}).to_string();
        let response = http
            .get(&url)
            .query(&[
                ("appType", "claude"),
                ("modelsByApp", selection.as_str()),
                ("startTime", &at(9)),
                ("endTime", &at(18)),
            ])
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), expected);
    }
    handle.stop();
}

/// The device set: by default only the local machine is queried, but paired
/// devices can be selected.
#[tokio::test]
async fn http_query_device_selection_defaults_to_local() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    // Both the default and an explicit local only query the local machine.
    for query in [
        "days=7&appType=claude",
        "days=7&appType=claude&devices=local",
        "days=7&appType=claude&devices=all",
    ] {
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

    // A device that is not paired -> 400.
    let response = http
        .get(format!("{url}?days=7&appType=claude&devices=some-peer"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("设备不存在"));

    // An empty segment -> 400.
    let response = http
        .get(format!("{url}?days=7&appType=claude&devices="))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);

    handle.stop();
}

/// The cycle endpoint takes the subscription of the first platform.
#[tokio::test]
async fn http_cycle_endpoint_uses_the_first_platform_for_the_subscription() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    let rule_value = json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"});
    let rule: observatory_usage_core::subscriptions::WeeklyRule =
        serde_json::from_value(rule_value.clone()).unwrap();
    let now = Local::now();
    let (cycle_start, _) =
        observatory_usage_core::subscriptions::weekly_cycle(&rule, now, 0).unwrap();
    let inside = (cycle_start + now.signed_duration_since(cycle_start) / 2).timestamp();
    insert_detail(
        &db, "cycle-a", "claude", "m-a", None, 100, 20, 0, 0, "1.00", inside,
    );
    insert_detail(
        &db, "cycle-b", "codex", "m-b", None, 200, 40, 0, 0, "2.00", inside,
    );
    insert_detail(
        &db, "cycle-c", "claude", "m-c", None, 400, 80, 0, 0, "4.00", inside,
    );
    // The first platform has a subscription, the second one does not.
    save_subscription(&db, "claude", &start, &end, rule_value);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/cycle", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    // claude comes first -> the subscription of claude is used and the cycle
    // can be computed.
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

    let response = http
        .get(format!(
            "{url}?appType=claude,codex&model=m-a,m-b&groupBy=week"
        ))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["appType"], json!(["claude", "codex"]));
    assert_eq!(body["model"], json!(["m-a", "m-b"]));
    assert_eq!(body["groupBy"], "week");
    assert_eq!(body["requests"], 2);
    assert_eq!(body["totalTokens"], 360);
    assert_eq!(body["costUsd"], 3.0);
    assert_eq!(
        body["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["totalTokens"].as_u64().unwrap())
            .sum::<u64>(),
        360
    );

    // codex comes first -> codex has no subscription -> 404.
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

/// The cycle series: fetch the current cycle and the previous N cycles in one
/// call, to draw the bar chart on the dashboard.
#[tokio::test]
async fn http_cycle_series_returns_a_series_of_cycles() {
    let db = Arc::new(Database::memory().unwrap());
    let (start, end) = year_window();
    // The cycle is aligned to local midnight: every cycle covers exactly 7
    // natural days.
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"00:00:00","endDay":4,"endTime":"00:00:00"}),
    );
    // Lay down two full cycles backwards from the Thursday 00:00 that starts the
    // current cycle, and forwards up to today: one detail row per day.
    // The assertions are computed from the number of days that actually fall
    // into the current cycle, so they do not depend on "which weekday today
    // is".
    let today = today();
    let into_cycle = (today.weekday().num_days_from_monday() + 4) % 7; // the number of days since this Thursday 00:00 (u32)
    let cycle_start = today - chrono::Duration::days(into_cycle as i64);
    let mut day = cycle_start - chrono::Duration::days(14);
    let mut seeded = 0u64;
    while day <= today {
        insert_detail(
            &db,
            &format!("d{seeded}"),
            "claude",
            "m",
            None,
            100,
            20,
            0,
            0,
            "1.00",
            local_ts(day, 0, 1),
        );
        day += chrono::Duration::days(1);
        seeded += 1;
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

    // Grouped by natural day.
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
    // The current cycle has index=0, the ones before it are -1 and -2.
    assert_eq!(cycles[0]["index"], 0);
    assert_eq!(cycles[1]["index"], -1);
    assert_eq!(cycles[2]["index"], -2);
    // The cycle windows follow each other end to end.
    assert_eq!(cycles[1]["end"], cycles[0]["start"]);
    assert_eq!(cycles[2]["end"], cycles[1]["start"]);
    // Every cycle covers exactly 7 natural days.
    for cycle in cycles {
        let first =
            NaiveDate::parse_from_str(cycle["startDate"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        let last =
            NaiveDate::parse_from_str(cycle["endDate"].as_str().unwrap(), "%Y-%m-%d").unwrap();
        assert_eq!((last - first).num_days(), 6, "周期应覆盖 7 个自然日");
    }
    // Usage: the current cycle starts on the Thursday, the two previous cycles
    // are filled with 7 days each.
    let in_current = u64::from(into_cycle) + 1;
    assert_eq!(cycles[0]["requests"], in_current);
    assert_eq!(cycles[1]["requests"], 7);
    assert_eq!(cycles[2]["requests"], 7);
    assert_eq!(cycles[0]["totalTokens"], in_current * 120);
    assert_eq!(cycles[0]["costUsd"], in_current as f64);
    // The current cycle is not over yet.
    assert_eq!(cycles[0]["complete"], false);
    assert_eq!(cycles[1]["complete"], true);

    // By default 8 cycles are returned.
    let response = http
        .get(format!("{url}?appType=claude"))
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["cycles"].as_array().unwrap().len(), 8);

    // Invalid parameters.
    for params in [
        "",
        "appType=claude&count=0",
        "appType=claude&count=abc",
        "appType=claude&unknown=1",
    ] {
        let response = http
            .get(format!("{url}?{params}"))
            .header("X-API-Key", &key)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{params}");
    }
    // No subscription -> 404.
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

/// The cycle series is grouped by natural day, so it never reports 422 for
/// cutting into an archived day.
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
    // Put the daily rollup on the **boundary day** of a cycle - only the
    // boundary day gets cut by a second-level interval.
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

    // A single cycle query over the same interval is 422.
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

    // The series endpoint is grouped by natural day and returns as usual.
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
    // That archived daily rollup is counted in the cycle it belongs to.
    let total: u64 = cycles
        .iter()
        .map(|cycle| cycle["requests"].as_u64().unwrap_or(0))
        .sum();
    assert_eq!(total, 3);

    handle.stop();
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
    // 403 and a stable error code.
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

/// The new parameters are available at the HTTP layer and duplicated parameters
/// are rejected.
#[tokio::test]
async fn http_extended_parameters_and_duplicate_rejection() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    let handle = server::start(db.clone(), 0).await.unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = public_api::rotate_key(&db).unwrap();

    let response = http
        .get(format!("{url}?days=7&appType=claude&groupBy=week&model=m"))
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

/// While a background scan is writing, the total, `daily` and `buckets` inside
/// the same response still agree.
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

/// The rule for resolving local midnight is deterministic and independent of
/// the operating system time zone.
///
/// A daylight saving switch day hits two kinds of boundary: when the clock
/// goes back 00:00 happens twice, when it goes forward 00:00 does not exist at
/// all. The former takes the earlier instant, the latter has to raise an error
/// instead of silently shifting.
#[test]
fn local_midnight_resolution_rules() {
    let date = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
    let early = Local::now();
    let late = Local::now();

    // A single instant: returned as is.
    let single = chrono::LocalResult::Single(early);
    assert_eq!(
        public_api::resolve_local_midnight(single, date).unwrap(),
        early
    );

    // An ambiguous midnight: take the earlier one.
    let ambiguous = chrono::LocalResult::Ambiguous(early, late);
    assert_eq!(
        public_api::resolve_local_midnight(ambiguous, date).unwrap(),
        early
    );

    // A missing midnight: 400 INVALID_LOCAL_BOUNDARY.
    let missing = chrono::LocalResult::None;
    let error = public_api::resolve_local_midnight(missing, date).unwrap_err();
    assert_eq!(error.status, 400);
    assert_eq!(error.code, "INVALID_LOCAL_BOUNDARY");
}

/// Run the full "ambiguous midnight / missing midnight" path under a time zone
/// that has daylight saving.
///
/// On Unix `chrono::Local` follows the `TZ` environment variable, so a child
/// process can switch the time zone; on Windows it goes through the system API
/// and ignores `TZ`, so this path is covered by the deterministic case of
/// [`local_midnight_resolution_rules`].
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
        // `marker` already has the "MIDNIGHT_RULES " prefix stripped, so the
        // first field is offset=.
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

    // An ambiguous midnight: take the earlier instant, the query succeeds and
    // the start lands on the earlier midnight.
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

    // A missing midnight: returns 400 INVALID_LOCAL_BOUNDARY and does not
    // shift silently.
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

/// Read an example file under `docs/examples`.
fn read_example(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("docs")
        .join("examples")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()))
}

/// The example JSON in the docs must parse.
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

/// The Postman collection must stay importable and every request must point at
/// the same read-only interface.
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

    // Recursively collect all requests (skipping folders).
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
        // Only these three read-only query endpoints are allowed.
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
        // The cycle endpoints must name an Agent platform, except for the request
        // that deliberately demonstrates the missing `appType`: that one must not
        // carry any query parameter at all.
        if path.starts_with("api/v1/usage/cycle") {
            let query = request["request"]["url"]["query"].as_array().unwrap();
            let has_app = query
                .iter()
                .any(|item| item["key"] == "appType" && item["disabled"] != true);
            if !has_app {
                assert!(query.is_empty(), "{name} 缺少 appType 参数");
            }
        }
        // The only request allowed to omit the header is the one that demonstrates
        // the 401 response.
        let has_key = request["request"]["header"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header["key"] == "X-API-Key");
        assert!(
            has_key || name.starts_with("401"),
            "{name} 缺少 X-API-Key 请求头"
        );
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

/// Every request in the collection has to work against a real service and
/// return the expected status code.
///
/// This case ties the "documentation" and the "implementation" together: as
/// soon as the collection changes a path, a parameter or an error code, it
/// fails here.
#[tokio::test]
async fn postman_collection_requests_match_the_live_endpoint() {
    let db = Arc::new(Database::memory().unwrap());
    seed(&db);
    // Only share some apps, so that the "not allowed to share" request really
    // returns 403.
    let mut settings = identity::share_settings(&db);
    settings.shared_app_types = vec!["claude".into(), "claude-desktop".into(), "codex".into()];
    identity::save_share_settings(&db, &settings).unwrap();

    // The cycle endpoint needs a subscription record to compute the cycle: give
    // one to claude and deliberately none to codex.
    //
    // The cycle is aligned to local midnight: that way the first day of the
    // cycle is covered in full, it never cuts into a historical day that only
    // has a daily rollup left, and the assertions do not change with the
    // weekday the test happens to run on.
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

    // The variable values line up with the seed data: the daily rollup falls
    // on yesterday.
    let yesterday = today().pred_opt().unwrap();
    let day_before = yesterday.pred_opt().unwrap();
    let at = |date: NaiveDate, hour| {
        Local
            .timestamp_opt(local_ts(date, hour, 0), 0)
            .single()
            .unwrap()
            .to_rfc3339()
    };
    let variables = vec![
        ("baseUrl", base.clone()),
        ("apiKey", key.clone()),
        ("startDate", day_before.to_string()),
        ("endDate", yesterday.to_string()),
        ("startTime", at(today(), 9)),
        ("endTime", at(today(), 18)),
        ("startTimeNaive", format!("{yesterday}T09:00:00")),
        ("endTimeNaive", format!("{yesterday}T18:00:00")),
        ("dayStart", at(yesterday, 0)),
        ("dayEnd", at(yesterday.succ_opt().unwrap(), 0)),
        ("archivedStartTime", at(yesterday, 9)),
        ("archivedEndTime", at(yesterday, 18)),
        ("model", "m".to_string()),
        // Several models: comma separated. The model of the seed data is m, m2
        // has no records.
        ("models", "m,m2".to_string()),
        // Models selected per platform.
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
        // The path comes from the collection itself (`api/v1/usage/summary` or
        // `api/v1/usage/cycle`).
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
                if parameter["disabled"] == true {
                    continue;
                }
                let mut value = parameter["value"].as_str().unwrap().to_owned();
                for (key, replacement) in &variables {
                    value = value.replace(&format!("{{{{{key}}}}}"), replacement);
                }
                assert!(!value.contains("{{"), "{name} 还有未替换的变量");
                pairs.append_pair(parameter["key"].as_str().unwrap(), &value);
            }
        }

        // The expected status code is decided by the prefix in the request name.
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
        // Only the "missing key" request goes out without the X-API-Key header:
        // follow the headers the request itself declares.
        let declares_key = request["request"]["header"]
            .as_array()
            .unwrap()
            .iter()
            .any(|header| header["key"] == "X-API-Key");
        if declares_key {
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
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    );
    let timestamp = Local::now().timestamp() - 5;
    insert_detail(
        &db,
        "dashboard-model-a",
        "claude",
        "model-a",
        None,
        100,
        20,
        30,
        0,
        "1.25",
        timestamp,
    );
    insert_detail(
        &db,
        "dashboard-model-b",
        "claude",
        "model-b",
        None,
        200,
        10,
        0,
        0,
        "2.00",
        timestamp,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/dashboard/usage", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let denied = http
        .post(&url)
        .json(&json!({"appType":"claude"}))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    let response = http
        .post(&url)
        .header("X-API-Key", key)
        .json(&json!({"appType":"claude"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["billingTotal"]["totalTokens"], 360);
    assert_eq!(
        body["billingAggregations"]["byAppAndModel"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert!(!body["quotaCycles"].as_array().unwrap().is_empty());
    assert!(body["selectedQuotaCycle"]["usage"]["daily"].is_array());
    handle.stop();
}

#[tokio::test]
async fn dashboard_models_uses_platform_subscription_and_aggregates_shared_apps() {
    let db = Arc::new(Database::memory().unwrap());
    let start = (today() - Duration::days(40)).to_string();
    let end = (today() + Duration::days(40)).to_string();
    save_subscription(
        &db,
        "claude",
        &start,
        &end,
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    );
    let ts = Local::now().timestamp() - 5;
    insert_detail(
        &db,
        "cross-a",
        "claude",
        "shared-model",
        None,
        100,
        0,
        0,
        0,
        "1",
        ts,
    );
    insert_detail(
        &db,
        "cross-b",
        "codex",
        "shared-model",
        None,
        200,
        0,
        0,
        0,
        "2",
        ts,
    );
    insert_detail(
        &db,
        "cross-c",
        "codex",
        "other-model",
        None,
        500,
        0,
        0,
        0,
        "5",
        ts,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/dashboard/models", handle.port);
    let call = |body: Value| http.post(&url).header("X-API-Key", &key).json(&body).send();
    let matched: Value = call(json!({"platformCode":"claude","modelIds":["shared-model"]}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(matched["scope"]["subscriptionApp"], "claude");
    assert_eq!(matched["billingTotal"]["totalTokens"], 300);
    let default: Value = call(json!({"platformCode":"claude"}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(default["billingTotal"]["totalTokens"], 100);
    let empty: Value = call(json!({"platformCode":"claude","modelIds":[]}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(empty["billingTotal"]["totalTokens"], 100);
    let invalid = call(json!({"platformCode":"claude","modelIds":[""]}))
        .await
        .unwrap();
    assert_eq!(invalid.status(), 400);
    handle.stop();
}

#[tokio::test]
async fn dashboard_model_prefix_matches_case_insensitively_without_changing_exact_queries() {
    let db = Arc::new(Database::memory().unwrap());
    let start = (today() - Duration::days(40)).to_string();
    let end = (today() + Duration::days(40)).to_string();
    save_subscription(
        &db,
        "mcode",
        &start,
        &end,
        json!({"startDay":4,"startTime":"19:00:00","endDay":4,"endTime":"19:00:00"}),
    );
    let ts = Local::now().timestamp() - 5;
    insert_detail(
        &db,
        "prefix-a",
        "mcode",
        "MiniMax-M3",
        None,
        100,
        0,
        0,
        0,
        "1",
        ts,
    );
    insert_detail(
        &db,
        "prefix-b",
        "mcode",
        "minimax-m2",
        None,
        200,
        0,
        0,
        0,
        "2",
        ts,
    );
    insert_detail(
        &db,
        "prefix-c",
        "codex",
        "MINIMAX-M3",
        None,
        300,
        0,
        0,
        0,
        "3",
        ts,
    );
    insert_detail(
        &db,
        "prefix-other",
        "mcode",
        "other",
        None,
        500,
        0,
        0,
        0,
        "5",
        ts,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let base = format!("http://127.0.0.1:{}", handle.port);
    let call = |path: &str, body: Value| {
        http.post(format!("{base}{path}"))
            .header("X-API-Key", &key)
            .json(&body)
            .send()
    };
    let one: Value = call(
        "/api/v1/dashboard/usage",
        json!({"appType":"mcode","modelPrefix":"minimax"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(one["billingTotal"]["totalTokens"], 300);
    let all: Value = call(
        "/api/v1/dashboard/models",
        json!({"platformCode":"mcode","modelPrefix":"minimax"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(all["billingTotal"]["totalTokens"], 600);
    let combined:Value=call("/api/v1/dashboard/models",json!({"platformCode":"mcode","modelIds":["MiniMax-M3","other"],"modelPrefixes":["mini","MINIMAX","other"]})).await.unwrap().json().await.unwrap();
    assert_eq!(combined["billingTotal"]["totalTokens"], 1100);
    let zero: Value = call(
        "/api/v1/dashboard/models",
        json!({"platformCode":"mcode","modelPrefixes":["no-such-model","not-found"]}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(zero["billingTotal"]["totalTokens"], 0);
    assert_eq!(
        call(
            "/api/v1/dashboard/models",
            json!({"platformCode":"mcode","modelPrefixes":[""]})
        )
        .await
        .unwrap()
        .status(),
        400
    );

    let empty: Value = call(
        "/api/v1/dashboard/usage",
        json!({"appType":"mcode","modelPrefix":"no-such-model"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(empty["billingTotal"]["totalTokens"], 0);
    let original: Value = call(
        "/api/v1/dashboard/usage",
        json!({"appType":"mcode","model":"MiniMax-M3"}),
    )
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert_eq!(original["billingTotal"]["totalTokens"], 100);
    assert_eq!(
        call(
            "/api/v1/dashboard/usage",
            json!({"appType":"mcode","modelPrefix":""})
        )
        .await
        .unwrap()
        .status(),
        400
    );
    assert_eq!(
        call(
            "/api/v1/dashboard/usage",
            json!({"appType":"mcode","modelPrefix":"mini","model":"other"})
        )
        .await
        .unwrap()
        .status(),
        400
    );
    handle.stop();
}

#[tokio::test]
async fn summary_all_time_prefix_accepts_long_history_and_preserves_defaults() {
    let db = Arc::new(Database::memory().unwrap());
    let old = today() - Duration::days(400);
    insert_rollup(&db, old, "claude", "DeepSeek-old", 2, 100, 10, "1.00");
    insert_rollup(&db, old, "codex", "deepseek-new", 3, 200, 20, "2.00");
    insert_rollup(&db, old, "claude", "other", 4, 500, 0, "3.00");
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let call = |q: &str| {
        http.get(format!("{url}?{q}"))
            .header("X-API-Key", &key)
            .send()
    };
    let response = call("allTime=true&modelPrefix=deepseek&devices=all&groupBy=month")
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["totalTokens"], 330);
    assert_eq!(body["requests"], 5);
    assert_eq!(body["allTime"], true);
    assert_eq!(body["modelPrefix"], "deepseek");
    assert!(body["days"].as_u64().unwrap() > 366);
    let none: Value = call("allTime=true&modelPrefix=not-found")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(none["totalTokens"], 0);
    assert_eq!(none["model"], json!([]));
    let default: Value = call("modelPrefix=deepseek")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(default["totalTokens"], 0);
    assert_eq!(default["days"], 7);
    assert_eq!(call("allTime=true&days=7").await.unwrap().status(), 400);
    assert_eq!(
        call("modelPrefix=deepseek&model=other")
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(call("modelPrefix=").await.unwrap().status(), 400);
    handle.stop();
}

/// byModel=true: the per-model breakdown uses the same window and the same
/// basis as the top level, and its entries have the same shape as
/// byAppAndModel.
#[test]
fn by_model_returns_per_model_breakdown_over_the_same_window() {
    let db = Database::memory().unwrap();
    let today = today();
    insert_detail(
        &db,
        "a",
        "claude",
        "claude-opus",
        None,
        100,
        20,
        0,
        0,
        "1.00",
        local_ts(today, 1, 0),
    );
    insert_detail(
        &db,
        "b",
        "claude",
        "claude-sonnet",
        None,
        50,
        10,
        0,
        0,
        "0.50",
        local_ts(today, 2, 0),
    );
    insert_detail(
        &db,
        "c",
        "codex",
        "gpt-6",
        None,
        10,
        2,
        0,
        0,
        "0.10",
        local_ts(today, 3, 0),
    );
    insert_rollup(
        &db,
        today.pred_opt().unwrap(),
        "claude",
        "claude-opus",
        1,
        7,
        3,
        "0.07",
    );

    let result = public_api::summary(
        &db,
        public_api::SummaryQuery {
            days: Some(3),
            by_model: Some(true),
            ..Default::default()
        },
    )
    .unwrap();
    let items = result["models"].as_array().expect("响应应有 models 数组");
    let ranked: Vec<(String, u64)> = items
        .iter()
        .map(|m| {
            (
                m["model"].as_str().unwrap().to_owned(),
                m["usage"]["totalTokens"].as_u64().unwrap(),
            )
        })
        .collect();
    // Sorted by tokens descending; the 10 tokens of yesterday's daily rollup
    // count towards claude-opus
    assert_eq!(
        ranked,
        vec![
            ("claude-opus".into(), 130),
            ("claude-sonnet".into(), 60),
            ("gpt-6".into(), 12)
        ]
    );
    assert_eq!(result["totalTokens"], 202);
    // Every entry carries the appType, usage.daily covers the same 3 day
    // window (empty days are zero filled) and the current day comes from the
    // live detail rows
    let opus = items.iter().find(|m| m["model"] == "claude-opus").unwrap();
    assert_eq!(opus["appType"], "claude");
    let daily = opus["usage"]["daily"].as_array().unwrap();
    assert_eq!(daily.len(), 3);
    assert_eq!(
        daily.last().unwrap()["date"].as_str().unwrap(),
        today.to_string()
    );
    assert_eq!(daily.last().unwrap()["totalTokens"].as_u64().unwrap(), 120);
    // The model filter applies to the breakdown as well
    let filtered = public_api::summary(
        &db,
        public_api::SummaryQuery {
            days: Some(3),
            by_model: Some(true),
            model: Some("claude-sonnet".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let filtered_models: Vec<&str> = filtered["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert_eq!(filtered_models, vec!["claude-sonnet"]);
    // By default models is not returned
    assert!(public_api::summary(&db, query(3, None, None))
        .unwrap()
        .get("models")
        .is_none());
}

#[test]
fn by_model_conflicts_with_all_time() {
    let db = Database::memory().unwrap();
    let error = public_api::summary(
        &db,
        public_api::SummaryQuery {
            all_time: Some(true),
            by_model: Some(true),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(error.status, 400);
}

#[tokio::test]
async fn http_by_model_query_param_parses_and_returns_models() {
    let db = Arc::new(Database::memory().unwrap());
    let now = Local::now().timestamp();
    insert_detail(
        &db,
        "a",
        "claude",
        "claude-opus",
        None,
        100,
        20,
        0,
        0,
        "1.00",
        now - 60,
    );
    insert_detail(
        &db,
        "b",
        "claude",
        "claude-sonnet",
        None,
        50,
        10,
        0,
        0,
        "0.50",
        now - 30,
    );
    let handle = server::start(db.clone(), 0).await.unwrap();
    let key = public_api::rotate_key(&db).unwrap();
    let url = format!("http://127.0.0.1:{}/api/v1/usage/summary", handle.port);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let ok: Value = http
        .get(&url)
        .query(&[("days", "2"), ("appType", "claude"), ("byModel", "true")])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let models: Vec<&str> = ok["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["model"].as_str().unwrap())
        .collect();
    assert_eq!(models, vec!["claude-opus", "claude-sonnet"]);
    let bad = http
        .get(&url)
        .query(&[("days", "2"), ("byModel", "yes")])
        .header("X-API-Key", &key)
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
    handle.stop();
}
