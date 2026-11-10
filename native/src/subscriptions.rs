use crate::{database::Database, error::AppError};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const KEY: &str = "subscriptions_v1";
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeeklyRule {
    /// Day of the week, 0 is Sunday. Both the cycle start and the cycle end are computed in the local time zone.
    pub start_day: u8,
    /// `HH:MM:SS`。
    pub start_time: String,
    pub end_day: u8,
    pub end_time: String,
}
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    pub app_type: String,
    pub price: f64,
    pub currency: String,
    pub billing: String,
    pub start_date: String,
    pub end_date: String,
    pub enabled: bool,
    pub rule: WeeklyRule,
    /// Custom billing term: how many days one term spans when `billing` is the custom-term
    /// marker (`"\u{671f}"`).
    ///
    /// The billing term (how often you pay) and the weekly quota cycle (when the quota resets)
    /// are two different things -- a monthly subscription can still have a quota that resets
    /// weekly, so this field does not take part in the cycle computation and is only used for display.
    ///
    /// Older records do not have this field; `default` fills in `None` so existing
    /// configurations keep working.
    #[serde(default)]
    pub custom_interval_days: Option<u32>,
}
impl Subscription {
    /// Whether today falls inside the validity period (the end day is exclusive).
    pub fn is_active_on(&self, today: NaiveDate) -> bool {
        let start = NaiveDate::parse_from_str(&self.start_date, "%Y-%m-%d");
        let end = NaiveDate::parse_from_str(&self.end_date, "%Y-%m-%d");
        self.enabled && matches!((start, end), (Ok(s), Ok(e)) if s <= today && today < e)
    }
}

/// Parse an `HH:MM:SS` value.
fn parse_time(value: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(value, "%H:%M:%S").map_err(|_| "周期时间无效".to_string())
}

/// A date and time in the local time zone. Errors when that time does not exist (skipped by daylight saving time).
fn resolve_local(date: NaiveDate, time: NaiveTime) -> Result<DateTime<Local>, String> {
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .ok_or_else(|| "该周期时间在本机时区不存在".to_string())
}

/// The most recent occurrence of "weekday + time" relative to `now`.
fn local_occurrence(
    now: DateTime<Local>,
    day: u8,
    time: NaiveTime,
) -> Result<DateTime<Local>, String> {
    let today = now.date_naive();
    let back = (today.weekday().num_days_from_sunday() as i64 - day as i64).rem_euclid(7);
    resolve_local(today - Duration::days(back), time)
}

/// Add or subtract days from a wall-clock time: convert it to a local date first, add the days, then resolve it at the local time.
fn shift_days(
    value: DateTime<Local>,
    days: i64,
    time: NaiveTime,
) -> Result<DateTime<Local>, String> {
    resolve_local(value.date_naive() + Duration::days(days), time)
}

/// Compute the quota cycle interval.
///
/// Derived from "weekday + HH:MM:SS" in the local time zone: the start is inclusive, the end
/// is exclusive. An `offset` of 0 is the current cycle, -1 the previous one and 1 the next one.
pub fn weekly_cycle(
    rule: &WeeklyRule,
    now: DateTime<Local>,
    offset: i32,
) -> Result<(DateTime<Local>, DateTime<Local>), String> {
    if rule.start_day > 6 || rule.end_day > 6 {
        return Err("周期星期设置无效".to_string());
    }
    let start_time = parse_time(&rule.start_time)?;
    let end_time = parse_time(&rule.end_time)?;
    let mut start = local_occurrence(now, rule.start_day, start_time)?;
    // The time of this occurrence has not been reached yet, so push it back by a week.
    if start > now {
        start = shift_days(start, -7, start_time)?;
    }
    if offset != 0 {
        start = shift_days(start, offset as i64 * 7, start_time)?;
    }
    let span = (rule.end_day as i64 + 7 - rule.start_day as i64) % 7;
    let mut end = resolve_local(start.date_naive() + Duration::days(span), end_time)?;
    // The end is not later than the start, so this round spans a whole week.
    if end <= start {
        end = shift_days(end, 7, end_time)?;
    }
    Ok((start, end))
}
fn valid_time(value: &str) -> bool {
    value.len() == 8 && NaiveTime::parse_from_str(value, "%H:%M:%S").is_ok()
}
pub fn load(db: &Database) -> Result<Vec<Subscription>, AppError> {
    match db.get_setting(KEY)? {
        None => Ok(Vec::new()),
        Some(value) => {
            serde_json::from_str(&value).map_err(|e| AppError::Config(format!("订阅配置无效: {e}")))
        }
    }
}
pub fn save(db: &Database, items: Vec<Subscription>) -> Result<(), AppError> {
    if items.len() > 100 {
        return Err(AppError::InvalidInput("订阅最多 100 项".into()));
    }
    let mut ids = HashSet::new();
    for item in &items {
        let start = NaiveDate::parse_from_str(&item.start_date, "%Y-%m-%d");
        let end = NaiveDate::parse_from_str(&item.end_date, "%Y-%m-%d");
        if uuid::Uuid::parse_str(&item.id).is_err()
            || !ids.insert(&item.id)
            || item.name.trim().is_empty()
            || item.name.chars().count() > 120
            || !crate::lan_usage::SHARED_APP_TYPES.contains(&item.app_type.as_str())
            || !item.price.is_finite()
            || !(0.0..=1_000_000_000.0).contains(&item.price)
            || !["USD", "CNY", "EUR"].contains(&item.currency.as_str())
            || !["月", "年", "期"].contains(&item.billing.as_str())
            // Custom billing term: picking the custom-term marker requires the number of days; any other billing value must not carry one.
            || (item.billing == "期") != item.custom_interval_days.is_some()
            || item.custom_interval_days.is_some_and(|days| !(1..=366).contains(&days))
            || !matches!((start,end), (Ok(a),Ok(b)) if a < b)
            || item.rule.start_day > 6
            || item.rule.end_day > 6
            || !valid_time(&item.rule.start_time)
            || !valid_time(&item.rule.end_time)
        {
            return Err(AppError::InvalidInput("订阅信息或周期设置无效".into()));
        }
    }
    let data = serde_json::to_string(&items).map_err(|e| AppError::InvalidInput(e.to_string()))?;
    if data.len() > 65536 {
        return Err(AppError::InvalidInput("订阅配置过大".into()));
    }
    db.set_setting(KEY, &data)
}
