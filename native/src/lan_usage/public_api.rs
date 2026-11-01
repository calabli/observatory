//! Small, numeric summaries for HTTP clients and desktop widgets.
use crate::{database::Database, error::AppError};
use chrono::{Days, Local, NaiveDate, TimeZone};
use rust_decimal::{prelude::ToPrimitive, Decimal};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const API_KEY: &str = "usage_read_api_key_hash";
pub fn key_configured(db: &Database) -> Result<bool, AppError> {
    Ok(db.get_setting(API_KEY)?.is_some())
}
pub fn rotate_key(db: &Database) -> Result<String, AppError> {
    let key = super::pairing::new_token();
    db.set_setting(API_KEY, &format!("{:x}", Sha256::digest(key.as_bytes())))?;
    Ok(key)
}
pub fn revoke_key(db: &Database) -> Result<(), AppError> {
    db.delete_setting(API_KEY)
}
pub fn key_matches(db: &Database, key: &str) -> bool {
    key.len() == 64
        && db
            .get_setting(API_KEY)
            .ok()
            .flatten()
            .is_some_and(|saved| saved == format!("{:x}", Sha256::digest(key.as_bytes())))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryQuery {
    pub days: Option<u32>,
    pub end_date: Option<String>,
    pub app_type: Option<String>,
}

#[derive(Default)]
struct Metrics {
    requests: u64,
    input: u64,
    output: u64,
    read: u64,
    write: u64,
    cost: Decimal,
}
impl Metrics {
    fn add(
        &mut self,
        requests: u64,
        input: u64,
        output: u64,
        read: u64,
        write: u64,
        cost: &str,
    ) -> Result<(), AppError> {
        self.merge(&Self {
            requests,
            input,
            output,
            read,
            write,
            cost: cost
                .parse::<Decimal>()
                .map_err(|_| AppError::Database("用量金额无效".into()))?,
        });
        Ok(())
    }
    fn merge(&mut self, other: &Self) {
        self.requests += other.requests;
        self.input += other.input;
        self.output += other.output;
        self.read += other.read;
        self.write += other.write;
        self.cost += other.cost;
    }
    fn json(&self) -> Value {
        json!({"requests":self.requests,"inputTokens":self.input,"outputTokens":self.output,"cacheReadTokens":self.read,"cacheWriteTokens":self.write,
            "totalTokens":self.input+self.output+self.read+self.write,"costUsd":self.cost.to_f64().unwrap_or(0.0)})
    }
}
fn midnight(date: NaiveDate) -> Result<chrono::DateTime<Local>, AppError> {
    Local
        .from_local_datetime(&date.and_hms_opt(0, 0, 0).unwrap())
        .earliest()
        .ok_or_else(|| AppError::InvalidInput("所选日期的本地午夜不存在".into()))
}
pub fn summary(db: &Database, query: SummaryQuery) -> Result<Value, AppError> {
    let days = query.days.unwrap_or(7);
    if !(1..=366).contains(&days) {
        return Err(AppError::InvalidInput("days 必须为 1–366".into()));
    }
    let today = Local::now().date_naive();
    let end_date = match query.end_date {
        Some(value) => NaiveDate::parse_from_str(&value, "%Y-%m-%d")
            .map_err(|_| AppError::InvalidInput("endDate 必须为 YYYY-MM-DD".into()))?,
        None => today,
    };
    if end_date > today {
        return Err(AppError::InvalidInput("endDate 不能晚于今天".into()));
    }
    let start_date = end_date
        .checked_sub_days(Days::new((days - 1) as u64))
        .ok_or_else(|| AppError::InvalidInput("日期超出范围".into()))?;
    let start = midnight(start_date)?;
    let end = midnight(
        end_date
            .succ_opt()
            .ok_or_else(|| AppError::InvalidInput("日期超出范围".into()))?,
    )?;
    let shared = super::identity::share_settings(db).shared_app_types;
    let apps = if let Some(app) = query.app_type.as_deref() {
        if !super::SHARED_APP_TYPES.contains(&app) {
            return Err(AppError::InvalidInput("不支持的 appType".into()));
        }
        if !shared.iter().any(|v| v == app) {
            return Err(AppError::HttpStatus {
                status: 403,
                body: "该应用未允许共享".into(),
            });
        }
        vec![app.to_owned()]
    } else {
        shared
    };
    let mut daily = BTreeMap::new();
    for offset in 0..days {
        daily.insert(
            start_date
                .checked_add_days(Days::new(offset as u64))
                .unwrap()
                .to_string(),
            Metrics::default(),
        );
    }
    for app in &apps {
        if days == 1 {
            // The existing trend query uses hourly buckets for a single day.
            // Summary includes archived rows too and preserves the dashboard semantics.
            let s = db.get_usage_summary_exact_app(
                Some(start.timestamp()),
                Some(end.timestamp() - 1),
                app,
            )?;
            daily.get_mut(&start_date.to_string()).unwrap().add(
                s.total_requests,
                s.total_input_tokens,
                s.total_output_tokens,
                s.total_cache_read_tokens,
                s.total_cache_creation_tokens,
                &s.total_cost,
            )?;
        } else {
            for s in db.get_daily_trends_exact_app(
                Some(start.timestamp()),
                Some(end.timestamp() - 1),
                app,
            )? {
                let date = s
                    .date
                    .get(..10)
                    .ok_or_else(|| AppError::Database("趋势日期无效".into()))?;
                if let Some(day) = daily.get_mut(date) {
                    day.add(
                        s.request_count,
                        s.total_input_tokens,
                        s.total_output_tokens,
                        s.total_cache_read_tokens,
                        s.total_cache_creation_tokens,
                        &s.total_cost,
                    )?;
                }
            }
        }
    }
    let mut total = Metrics::default();
    let mut rows = Vec::new();
    for (date, day) in daily {
        total.merge(&day);
        let mut row = day.json();
        row["date"] = json!(date);
        rows.push(row);
    }
    let mut result = total.json();
    result["days"] = json!(days);
    result["startDate"] = json!(start_date.to_string());
    result["endDate"] = json!(end_date.to_string());
    result["range"] = json!({"start":start.to_rfc3339(),"endExclusive":end.to_rfc3339(),"timezone":"server-local"});
    result["scope"] = json!("local");
    result["appType"] = json!(query.app_type);
    result["apps"] = json!(apps);
    result["currency"] = json!("USD");
    result["inputTokenSemantics"] = json!("fresh");
    result["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
    result["lastScanAt"] = json!(crate::services::session_usage::last_sync_completed_at());
    result["daily"] = json!(rows);
    Ok(result)
}
