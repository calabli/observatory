use crate::{database::Database, error::AppError};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const KEY: &str = "subscriptions_v1";
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WeeklyRule {
    /// 星期几，0 是周日。周期开始与结束都按本机时区计算。
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
    /// 自定义账期：`billing == "期"` 时，一个账期跨越多少天。
    ///
    /// 账期（多久付一次钱）与周额度周期（额度何时重置）是两回事——月度订阅
    /// 的额度仍可能每周重置，所以这个字段不参与周期计算，只用于展示。
    ///
    /// 旧记录没有这个字段，靠 `default` 补 `None`，既有配置继续可用。
    #[serde(default)]
    pub custom_interval_days: Option<u32>,
}
impl Subscription {
    /// 今天是否落在有效期内（结束日不含当日）。
    pub fn is_active_on(&self, today: NaiveDate) -> bool {
        let start = NaiveDate::parse_from_str(&self.start_date, "%Y-%m-%d");
        let end = NaiveDate::parse_from_str(&self.end_date, "%Y-%m-%d");
        self.enabled && matches!((start, end), (Ok(s), Ok(e)) if s <= today && today < e)
    }
}

/// 解析 `HH:MM:SS`。
fn parse_time(value: &str) -> Result<NaiveTime, String> {
    NaiveTime::parse_from_str(value, "%H:%M:%S").map_err(|_| "周期时间无效".to_string())
}

/// 本地时区的某个日期时刻。时刻不存在（夏令时跳过）时报错。
fn resolve_local(date: NaiveDate, time: NaiveTime) -> Result<DateTime<Local>, String> {
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .ok_or_else(|| "该周期时间在本机时区不存在".to_string())
}

/// 相对 `now` 最近一次出现的「星期几 + 时刻」。
fn local_occurrence(
    now: DateTime<Local>,
    day: u8,
    time: NaiveTime,
) -> Result<DateTime<Local>, String> {
    let today = now.date_naive();
    let back = (today.weekday().num_days_from_sunday() as i64 - day as i64).rem_euclid(7);
    resolve_local(today - Duration::days(back), time)
}

/// 墙上时间加减天数：先换算成本地日期再加天数，再按本地时刻解析。
fn shift_days(
    value: DateTime<Local>,
    days: i64,
    time: NaiveTime,
) -> Result<DateTime<Local>, String> {
    resolve_local(value.date_naive() + Duration::days(days), time)
}

/// 计算额度周期区间。
///
/// 按本机时区的「星期几 + 时分秒」推算：开始包含、结束不包含。`offset`
/// 为 0 是当前周期，-1 是上一个，1 是下一个。
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
    // 这一次的时刻还没到，往前推一周。
    if start > now {
        start = shift_days(start, -7, start_time)?;
    }
    if offset != 0 {
        start = shift_days(start, offset as i64 * 7, start_time)?;
    }
    let span = (rule.end_day as i64 + 7 - rule.start_day as i64) % 7;
    let mut end = resolve_local(start.date_naive() + Duration::days(span), end_time)?;
    // 结束不晚于开始，说明这一轮跨了一周。
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
            // 自定义账期：选了「期」就必须给出天数；没选就不该带值。
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
