//! Read-only usage query API: offers statistics queries to TinyPane, automation
//! scripts and other HTTP clients.
//!
//! The protocol is described in `docs/ai-usage-query-api-design.md`. The essentials:
//!
//! - It only queries the statistics stored on this machine and does not aggregate the
//!   snapshots of other devices;
//! - The top level is "the total of the selected range", `daily` and `buckets` are two
//!   groupings of the same data and the three totals have to add up consistently;
//! - The amounts are accumulated internally as fixed point decimals and are returned as
//!   JSON numbers;
//! - The whole response is produced within a single connection snapshot, so a background
//!   scan that writes in between cannot make the parts contradict each other;
//! - When the history only consists of daily rollups, a range that cuts a day in half
//!   cannot be split proportionally, so a 422 is returned instead of a number that is too low.

use crate::database::Database;
use crate::error::AppError;
use crate::services::usage_stats::{daily_usage_rows, rollup_dates, DailyUsageRow};
use chrono::{DateTime, Datelike, Duration, FixedOffset, Local, NaiveDate, TimeZone};
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

/// An API error: HTTP status + a stable error code + optional details.
///
/// The string `error` is kept for compatibility with the v0.1.30 clients and the new `code`
/// lets programs decide programmatically.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            details: None,
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "INVALID_PARAMETERS", message)
    }

    fn conflicting(message: impl Into<String>) -> Self {
        Self::new(400, "CONFLICTING_TIME_PARAMETERS", message)
    }

    fn date_range(message: impl Into<String>) -> Self {
        Self::new(400, "INVALID_DATE_RANGE", message)
    }

    fn boundary(message: impl Into<String>) -> Self {
        Self::new(400, "INVALID_LOCAL_BOUNDARY", message)
    }

    fn not_shared(message: impl Into<String>) -> Self {
        Self::new(403, "APP_NOT_SHARED", message)
    }

    fn device_scope_forbidden() -> Self {
        Self::new(
            403,
            "DEVICE_SCOPE_FORBIDDEN",
            "配对设备只能查询本机自己的用量",
        )
    }

    /// No matching record was found (for example the app has no subscription).
    fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "SUBSCRIPTION_NOT_FOUND", message)
    }

    /// The cycle currently has no range that can be aggregated (it has not started yet, or it is outside the validity period of the subscription).
    fn cycle_unavailable(message: impl Into<String>) -> Self {
        Self::new(404, "CYCLE_NOT_AVAILABLE", message)
    }

    fn unsupported_app(app: &str) -> Self {
        Self::new(
            400,
            "UNSUPPORTED_APP_TYPE",
            format!("不支持的应用标识: {app}"),
        )
    }

    fn unsupported_group(value: &str) -> Self {
        Self::new(
            400,
            "UNSUPPORTED_GROUP_BY",
            format!("不支持的 groupBy: {value}"),
        )
    }

    /// The historical daily rollups cannot support the requested second-level range.
    fn precision(dates: Vec<String>) -> Self {
        let mut error = Self::new(
            422,
            "RANGE_PRECISION_UNAVAILABLE",
            "查询区间截断了仅有日汇总的历史日期，请改用完整日期",
        );
        error.details = Some(json!({ "dates": dates }));
        error
    }

    pub fn to_json(&self) -> Value {
        let mut body = json!({ "error": self.message, "code": self.code });
        if let Some(details) = &self.details {
            body["details"] = details.clone();
        }
        body
    }
}

impl From<AppError> for ApiError {
    fn from(error: AppError) -> Self {
        match error {
            AppError::InvalidInput(message) => Self::invalid(message),
            AppError::HttpStatus { status: 403, body } => Self::not_shared(body),
            other => Self::new(500, "USAGE_QUERY_FAILED", other.to_string()),
        }
    }
}

/// The query parameters. `deny_unknown_fields` makes unknown parameters be rejected right away.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryQuery {
    /// The last N calendar days (1-366), the end day included.
    pub days: Option<u32>,
    /// Query all the history from the earliest record until now.
    pub all_time: Option<bool>,
    /// The end date `YYYY-MM-DD`, the server's today by default.
    pub end_date: Option<String>,
    /// The start date `YYYY-MM-DD`, both ends included. Cannot be used together with `days`.
    pub start_date: Option<String>,
    /// The start instant of the second-level range, RFC 3339, a UTC offset is required.
    pub start_time: Option<String>,
    /// The end instant of the second-level range, exclusive.
    pub end_time: Option<String>,
    /// The app identifier. `claude` refers to Claude Code only, `claude-desktop` is queried separately.
    pub app_type: Option<String>,
    /// Filter exactly by the effective pricing model, with the same semantics as the page.
    pub model: Option<String>,
    /// A prefix of the effective pricing model ID, ASCII letters are matched case-insensitively.
    pub model_prefix: Option<String>,
    /// JSON object mapping selected app IDs to their selected model IDs.
    pub models_by_app: Option<String>,
    /// The grouping: `day` (default), `week`, `month`.
    pub group_by: Option<String>,
    /// Also return `models`: the split by model over the same window as the top level. Every
    /// entry `{"appType","model","usage"}` has the same shape as the `byAppAndModel`
    /// entries of `/dashboard/usage` (`usage.daily` covers the same window and includes the live
    /// detail records of the current day), so a caller gets the model composition of a natural
    /// window without depending on the billing term of a subscription.
    /// Cannot be used together with `allTime`.
    pub by_model: Option<bool>,
    /// The device set, comma separated. Only this machine is queried by default.
    ///
    /// `local` stands for this machine, the rest are the `deviceId` of the paired devices.
    /// Leaving it out is equivalent to `local`.
    pub devices: Option<String>,
}

/// The aggregation granularity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Day,
    Week,
    Month,
}

impl GroupBy {
    fn parse(value: &str) -> Result<Self, ApiError> {
        match value {
            "day" => Ok(GroupBy::Day),
            "week" => Ok(GroupBy::Week),
            "month" => Ok(GroupBy::Month),
            other => Err(ApiError::unsupported_group(other)),
        }
    }

    fn label(self) -> &'static str {
        match self {
            GroupBy::Day => "day",
            GroupBy::Week => "week",
            GroupBy::Month => "month",
        }
    }
}

/// The usage metrics of one query. The amounts are accumulated as fixed point decimals so that floating point errors do not grow across the groupings.
#[derive(Clone, Default)]
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

/// Validate and normalize an app identifier: it has to be supported and selected for sharing.
fn resolve_shared_app(shared: &[String], value: &str) -> Result<String, ApiError> {
    let app = value.trim();
    if app.is_empty() {
        return Err(ApiError::invalid("appType 不能为空"));
    }
    if !super::SHARED_APP_TYPES.contains(&app) {
        return Err(ApiError::unsupported_app(app));
    }
    if !shared.iter().any(|value| value == app) {
        return Err(ApiError::not_shared("该应用未允许共享"));
    }
    Ok(app.to_owned())
}

/// Parse a list of app identifiers: comma separated, duplicates removed, the order of the
/// request is kept.
///
/// An empty segment (`claude,,codex`) is treated as invalid and not as "all apps".
fn parse_app_list(shared: &[String], value: &str) -> Result<Vec<String>, ApiError> {
    let mut apps: Vec<String> = Vec::new();
    for part in value.split(',') {
        let app = resolve_shared_app(shared, part)?;
        if !apps.contains(&app) {
            apps.push(app);
        }
    }
    if apps.is_empty() {
        return Err(ApiError::invalid("appType 不能为空"));
    }
    Ok(apps)
}

/// Parse `modelsByApp`: a JSON object mapping a platform to an array of model IDs.
///
/// It has to be used together with `appType` and must not appear together with `model`.
/// The platforms of the mapping have to be part of `appType`; when a selected platform does
/// not appear in the mapping, all of its models take part in the statistics.
fn parse_models_by_app(
    apps: &[String],
    value: Option<&str>,
    models: Option<&Vec<String>>,
    app_type_provided: bool,
) -> Result<Option<BTreeMap<String, Vec<String>>>, ApiError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if models.is_some() || !app_type_provided || value.len() > 8192 {
        return Err(ApiError::invalid(
            "modelsByApp 需要指定 appType，不能与 model 同时使用",
        ));
    }
    let mut selected: BTreeMap<String, Vec<String>> = serde_json::from_str(value)
        .map_err(|_| ApiError::invalid("modelsByApp 必须是平台到模型数组的 JSON 对象"))?;
    if selected.is_empty() {
        return Err(ApiError::invalid("modelsByApp 不能为空"));
    }
    for (app, list) in &mut selected {
        if !apps.contains(app) || list.is_empty() || list.len() > 50 {
            return Err(ApiError::invalid(
                "modelsByApp 包含未选择的平台或无效模型列表",
            ));
        }
        let mut unique = Vec::new();
        for model in list.iter() {
            let model = model.trim();
            if model.is_empty() || model.len() > 256 {
                return Err(ApiError::invalid("模型 ID 无效"));
            }
            if !unique.iter().any(|m| m == model) {
                unique.push(model.to_owned());
            }
        }
        *list = unique;
    }
    Ok(Some(selected))
}

/// Parse a list of model identifiers: comma separated, matched exactly.
fn parse_model_list(value: &str) -> Result<Vec<String>, ApiError> {
    let mut models: Vec<String> = Vec::new();
    for part in value.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err(ApiError::invalid("model 不能为空"));
        }
        if !models.iter().any(|model| model == trimmed) {
            models.push(trimmed.to_owned());
        }
    }
    if models.is_empty() {
        return Err(ApiError::invalid("model 不能为空"));
    }
    Ok(models)
}

/// The device set involved in one query.
///
/// `local` is the alias of this machine, the rest are the `device_id` of the paired devices.
struct DeviceSelection {
    local: bool,
    peers: Vec<String>,
}

impl DeviceSelection {
    /// Whether only this machine is involved -- then no projection is needed and the local database can be queried directly.
    fn is_local_only(&self) -> bool {
        self.local && self.peers.is_empty()
    }
}

/// The device scope check for a call with a pairing token (instead of a read-only API key):
/// only this machine is allowed.
///
/// The data this machine synced from other devices belongs to the local user only; the LAN
/// export does not forward remote copies either (see `storage::export_local_usage`).
/// Otherwise the device B that is paired with this machine could use `devices=all` to read
/// the usage of the third device C, which is only paired with this machine and was never
/// authorized for B.
pub fn ensure_local_scope(devices: Option<&str>) -> Result<(), ApiError> {
    match devices.map(str::trim) {
        None => Ok(()),
        Some(value) if value.eq_ignore_ascii_case("local") => Ok(()),
        Some(_) => Err(ApiError::device_scope_forbidden()),
    }
}

/// Parse the device set. Only this machine is queried by default.
fn parse_device_selection(db: &Database, value: Option<&str>) -> Result<DeviceSelection, ApiError> {
    let Some(value) = value else {
        return Ok(DeviceSelection {
            local: true,
            peers: Vec::new(),
        });
    };
    // `all` is an explicit alias for local plus all currently paired and enabled peers.
    // Keep it exclusive to avoid silently accepting a misspelled or stale ID.
    if value.trim().eq_ignore_ascii_case("all") {
        let mut peers = known_devices(db)?;
        let local_id = super::identity::device_id(db)?;
        peers.retain(|peer| peer != &local_id);
        peers.sort();
        peers.dedup();
        return Ok(DeviceSelection { local: true, peers });
    }
    if value
        .split(',')
        .any(|part| part.trim().eq_ignore_ascii_case("all"))
    {
        return Err(ApiError::invalid("devices=all 不能与其他设备混用"));
    }
    let mut selection = DeviceSelection {
        local: false,
        peers: Vec::new(),
    };
    for part in value.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            return Err(ApiError::invalid("devices 不能为空"));
        }
        if trimmed.eq_ignore_ascii_case("local") {
            selection.local = true;
            continue;
        }
        if !selection.peers.iter().any(|peer| peer == trimmed) {
            selection.peers.push(trimmed.to_owned());
        }
    }
    if !selection.local && selection.peers.is_empty() {
        return Err(ApiError::invalid("devices 不能为空"));
    }
    // The device has to exist and be enabled, otherwise its usage cannot be determined.
    if !selection.peers.is_empty() {
        let known = known_devices(db)?;
        for peer in &selection.peers {
            if !known.iter().any(|item| item == peer) {
                return Err(ApiError::invalid(format!("设备不存在或未启用: {peer}")));
            }
        }
    }
    Ok(selection)
}

/// The external representation of the device set: this machine is `local`, a paired device is its `deviceId`.
fn device_labels(devices: &DeviceSelection) -> Vec<String> {
    let mut labels = Vec::new();
    if devices.local {
        labels.push("local".to_owned());
    }
    for peer in &devices.peers {
        if !labels.contains(peer) {
            labels.push(peer.clone());
        }
    }
    labels
}

/// The devices that are enabled and successfully paired.
fn known_devices(db: &Database) -> Result<Vec<String>, AppError> {
    db.with_connection(|conn| {
        let mut statement = conn.prepare(
            "SELECT device_id FROM lan_devices WHERE enabled = 1 AND EXISTS (SELECT 1 FROM settings WHERE key = 'lan_outgoing_token_v1:' || lan_devices.device_id)",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    })
}

/// The normalized query range: half-open `[start_ts, end_ts_exclusive)`.
struct ResolvedRange {
    start_ts: i64,
    end_ts_exclusive: i64,
    /// The first and last local date the range touches (inclusive).
    first_date: NaiveDate,
    last_date: NaiveDate,
}

/// The rules for resolving local midnight.
///
/// - A unique instant: it is taken as is;
/// - A repeated midnight (daylight saving time falling back): the earlier instant is taken
///   (following the `.earliest()` rule of v0.1.30);
/// - A midnight that does not exist (daylight saving time jumping forward skips 00:00): a
///   400 `INVALID_LOCAL_BOUNDARY` is returned, it is not silently moved to another time.
///
/// It is a separate function so that the three outcomes can be tested deterministically: the
/// daylight saving time rules depend on the operating system time zone, and `chrono::Local`
/// does not follow the `TZ` environment variable on Windows, so it cannot be switched in a
/// test.
pub fn resolve_local_midnight(
    result: chrono::LocalResult<DateTime<Local>>,
    date: NaiveDate,
) -> Result<DateTime<Local>, ApiError> {
    match result {
        chrono::LocalResult::Single(dt) => Ok(dt),
        chrono::LocalResult::Ambiguous(earliest, _) => Ok(earliest),
        chrono::LocalResult::None => {
            Err(ApiError::boundary(format!("本地日期 {date} 的午夜不存在")))
        }
    }
}

/// Local midnight.
fn local_midnight(date: NaiveDate) -> Result<DateTime<Local>, ApiError> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::boundary(format!("日期 {date} 无效")))?;
    resolve_local_midnight(Local.from_local_datetime(&naive), date)
}

/// The local time that corresponds to a timestamp.
fn local_datetime(timestamp: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(timestamp, 0)
        .earliest()
        .unwrap_or_else(Local::now)
}

fn parse_date(value: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| ApiError::invalid(format!("日期必须为 YYYY-MM-DD: {value}")))
}

/// An RFC 3339 time, a UTC offset is required; a non-zero fractional second is not accepted.
fn parse_instant(value: &str) -> Result<DateTime<FixedOffset>, ApiError> {
    let parsed = DateTime::parse_from_rfc3339(value.trim())
        .map_err(|_| ApiError::invalid(format!("时间必须为带 UTC 偏移的 RFC 3339: {value}")))?;
    if parsed.timestamp_subsec_nanos() != 0 {
        return Err(ApiError::invalid(format!("时间必须精确到整秒: {value}")));
    }
    Ok(parsed)
}

/// Normalize the three ways of selecting a time range.
///
/// - The last N days: `days` plus an optional `endDate`;
/// - An explicit date range: `startDate` plus an optional `endDate`;
/// - An explicit second-level range: `startTime` plus an optional `endTime` (the time of the
///   request when it is omitted).
fn resolve_range(query: &SummaryQuery, now: DateTime<Local>) -> Result<ResolvedRange, ApiError> {
    let today = now.date_naive();
    let has_instant = query.start_time.is_some() || query.end_time.is_some();
    let has_start_date = query.start_date.is_some();

    if has_instant {
        if query.days.is_some() || has_start_date || query.end_date.is_some() {
            return Err(ApiError::conflicting(
                "秒级区间不能与 days / startDate / endDate 混用",
            ));
        }
        let start_value = query
            .start_time
            .as_deref()
            .ok_or_else(|| ApiError::conflicting("填写 endTime 时必须提供 startTime"))?;
        let start = parse_instant(start_value)?;
        let end = match query
            .end_time
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(value) => parse_instant(value)?,
            // Use one captured request time, truncated to a whole second.
            None => Local
                .timestamp_opt(now.timestamp(), 0)
                .single()
                .unwrap()
                .fixed_offset(),
        };
        if start >= end {
            return Err(ApiError::date_range("startTime 必须早于 endTime"));
        }
        let tomorrow = today
            .succ_opt()
            .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
        if end.with_timezone(&Local) > local_midnight(tomorrow)? {
            return Err(ApiError::date_range("endTime 不能晚于服务器明天的本地午夜"));
        }
        let start_ts = start.timestamp();
        let end_ts = end.timestamp();
        let first_date = local_datetime(start_ts).date_naive();
        let last_date = local_datetime(end_ts - 1).date_naive();
        return Ok(ResolvedRange {
            start_ts,
            end_ts_exclusive: end_ts,
            first_date,
            last_date,
        });
    }

    if has_start_date && query.days.is_some() {
        return Err(ApiError::conflicting("startDate 不能与 days 同时使用"));
    }

    let end_date = match &query.end_date {
        Some(value) => parse_date(value)?,
        None => today,
    };
    if end_date > today {
        return Err(ApiError::date_range("endDate 不能晚于服务器今天"));
    }

    let (first_date, day_count) = if has_start_date {
        let start = parse_date(query.start_date.as_deref().unwrap_or_default())?;
        if start > end_date {
            return Err(ApiError::date_range("startDate 不能晚于 endDate"));
        }
        let count = end_date.signed_duration_since(start).num_days() + 1;
        (start, count)
    } else {
        let days = query.days.unwrap_or(7);
        if !(1..=366).contains(&days) {
            return Err(ApiError::invalid("days 必须为 1–366"));
        }
        let start = end_date
            .checked_sub_days(chrono::Days::new(days as u64 - 1))
            .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
        (start, days as i64)
    };
    if day_count > 366 {
        return Err(ApiError::date_range("查询范围不能超过 366 天"));
    }
    if day_count <= 0 {
        return Err(ApiError::date_range("查询范围为空"));
    }
    let last_date = first_date
        .checked_add_days(chrono::Days::new((day_count - 1) as u64))
        .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
    let start_midnight = local_midnight(first_date)?;
    let end_midnight = local_midnight(
        last_date
            .succ_opt()
            .ok_or_else(|| ApiError::date_range("日期超出范围"))?,
    )?;
    Ok(ResolvedRange {
        start_ts: start_midnight.timestamp(),
        end_ts_exclusive: end_midnight.timestamp(),
        first_date,
        last_date,
    })
}

/// The start and end local dates of a natural period (day / ISO week / month).
fn period_bounds(date: NaiveDate, group: GroupBy) -> (NaiveDate, NaiveDate) {
    match group {
        GroupBy::Day => (date, date.succ_opt().unwrap_or(date)),
        GroupBy::Week => {
            let monday = date - Duration::days(date.weekday().num_days_from_monday() as i64);
            (monday, monday + Duration::days(7))
        }
        GroupBy::Month => {
            let first = date.with_day(1).unwrap_or(date);
            let next = if first.month() == 12 {
                NaiveDate::from_ymd_opt(first.year() + 1, 1, 1)
            } else {
                NaiveDate::from_ymd_opt(first.year(), first.month() + 1, 1)
            }
            .unwrap_or(first);
            (first, next)
        }
    }
}

/// The identifier of a natural period. A day is `YYYY-MM-DD`, a week is the ISO week number
/// `YYYY-Www`, a month is `YYYY-MM`.
///
/// An ISO week uses the year the week belongs to, which avoids a wrong week number across
/// the turn of the year.
fn period_label(date: NaiveDate, group: GroupBy) -> String {
    match group {
        GroupBy::Day => date.format("%Y-%m-%d").to_string(),
        GroupBy::Week => {
            let week = date.iso_week();
            format!("{}-W{:02}", week.year(), week.week())
        }
        GroupBy::Month => date.format("%Y-%m").to_string(),
    }
}

/// The local dates the range does not cover completely.
///
/// A date range always covers whole days, only a second-level range cuts a day in half. If a
/// day that is cut in half only has a daily rollup left (day granularity), it cannot be split
/// proportionally and has to be counted or excluded as a whole day.
fn partial_local_dates(range: &ResolvedRange) -> Result<Vec<NaiveDate>, ApiError> {
    let mut dates = Vec::new();
    let first_midnight = local_midnight(range.first_date)?.timestamp();
    if range.start_ts > first_midnight {
        dates.push(range.first_date);
    }
    let last_next = range
        .last_date
        .succ_opt()
        .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
    if range.end_ts_exclusive < local_midnight(last_next)?.timestamp()
        && dates.last() != Some(&range.last_date)
    {
        dates.push(range.last_date);
    }
    Ok(dates)
}

/// The aggregate query.
///
/// The returned top level metrics are "the total of the selected range", `daily` and
/// `buckets` are two groupings of the same data and the three totals add up consistently.
pub fn summary(db: &Database, query: SummaryQuery) -> Result<Value, ApiError> {
    summary_inner(db, query, true)
}

/// The cycle query parameters. `deny_unknown_fields` makes unknown parameters be rejected right away.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleQuery {
    /// The app identifiers (Agent platforms), comma separated. Required.
    ///
    /// The cycle rules are **based on the first platform** -- there is only one cycle, so it has
    /// to be clear which platform's subscription defines it. The other platforms only take part in
    /// the usage statistics.
    pub app_type: String,
    /// The cycle index: 0 is the current cycle (default), -1 the previous one and 1 the next one.
    #[serde(default)]
    pub offset: Option<i32>,
    /// The model identifiers, comma separated. Only these models are counted.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models_by_app: Option<String>,
    /// The device set, comma separated. Only this machine is queried by default.
    #[serde(default)]
    pub devices: Option<String>,
    /// The grouping inside a cycle, by day by default.
    #[serde(default)]
    pub group_by: Option<String>,
}

/// Query the usage inside the current quota cycle of an Agent platform.
///
/// The response body is **exactly the same** as `/api/v1/usage/summary`: the cycle only
/// computes `startTime` / `endTime` for the caller, the aggregation semantics, the meaning
/// of the fields and the error codes do not change. The cycle boundaries can be read from
/// `range.start` and `range.endExclusive`.
pub fn cycle_summary(db: &Database, query: CycleQuery) -> Result<Value, ApiError> {
    cycle_summary_inner(db, query, true)
}

/// A local read is not restricted by the LAN sharing selection.
pub(crate) fn local_cycle_summary(db: &Database, query: CycleQuery) -> Result<Value, ApiError> {
    cycle_summary_inner(db, query, false)
}

fn cycle_summary_inner(
    db: &Database,
    query: CycleQuery,
    enforce_share: bool,
) -> Result<Value, ApiError> {
    // Do the sharing check first: an app that is not selected for sharing must not leak whether it has a subscription.
    let shared = if enforce_share {
        super::identity::share_settings(db).shared_app_types
    } else {
        super::default_shared_app_types()
    };
    let apps = parse_app_list(&shared, &query.app_type)?;
    // The subscription is based on the first platform.
    let app_type = apps[0].clone();
    let offset = query.offset.unwrap_or(0);
    if !(-52..=52).contains(&offset) {
        return Err(ApiError::invalid("offset 必须在 -52 到 52 之间"));
    }

    let subscription = pick_subscription(db, &app_type)?;

    // The request instant is truncated to whole seconds, consistent with the summary endpoint.
    let now = Local
        .timestamp_opt(Local::now().timestamp(), 0)
        .single()
        .unwrap();
    let (cycle_start, cycle_end) =
        crate::subscriptions::weekly_cycle(&subscription.rule, now, offset)
            .map_err(ApiError::invalid)?;

    // The cycle has not started yet: there is no range that can be aggregated.
    if cycle_start >= now {
        return Err(ApiError::cycle_unavailable(format!(
            "该周期尚未开始: {app_type}"
        )));
    }

    // The cycle is narrowed by the validity period of the subscription: the later of the two is
    // taken for the start and the earlier one for the end; the usage of the future does not exist
    // yet, so the end is truncated to the current instant.
    let window_start = local_midnight(parse_date(&subscription.start_date)?)?;
    let window_end = local_midnight(parse_date(&subscription.end_date)?)?;
    let start = std::cmp::max(cycle_start, window_start);
    let end = std::cmp::min(cycle_end, window_end);
    let end = std::cmp::min(end, now);
    if end <= start {
        return Err(ApiError::cycle_unavailable(format!(
            "该周期不在订阅有效期内: {app_type}"
        )));
    }

    // It uses the same aggregate logic, the fields and the statistics semantics are identical to
    // the summary endpoint.
    // The usage statistics cover all platforms of the request, the cycle boundaries come only
    // from the first platform.
    summary_inner(
        db,
        SummaryQuery {
            start_time: Some(to_rfc3339(start)),
            end_time: Some(to_rfc3339(end)),
            app_type: Some(apps.join(",")),
            group_by: query.group_by,
            model: query.model,
            models_by_app: query.models_by_app,
            devices: query.devices,
            ..Default::default()
        },
        enforce_share,
    )
}

/// The cycle series query parameters. `deny_unknown_fields` makes unknown parameters be rejected right away.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleSeriesQuery {
    /// The app identifiers (Agent platforms), comma separated. Required.
    ///
    /// The cycle rules are **based on the first platform**, consistent with
    /// `/api/v1/usage/cycle`.
    pub app_type: String,
    /// How many cycles are returned, the current one included. Default 8, upper limit 52.
    #[serde(default)]
    pub count: Option<u32>,
    /// The model identifiers, comma separated.
    #[serde(default)]
    pub model: Option<String>,
    /// The model IDs per platform, as a JSON string.
    #[serde(default)]
    pub models_by_app: Option<String>,
    /// The device set, comma separated. Only this machine is queried by default.
    #[serde(default)]
    pub devices: Option<String>,
}

/// Query the usage series of the last few quota cycles of an Agent platform.
///
/// Meant for the dashboard: it returns the bar chart data of "the current cycle plus the
/// previous N cycles" in one go.
///
/// The difference to `/api/v1/usage/cycle` lies in the **grouping semantics**:
///
/// - The single cycle endpoint counts over a second-level range whose two ends cut a natural
///   day;
/// - The series endpoint counts by **natural day attribution** -- every natural day belongs
///   as a whole to the cycle "the one its midnight falls into".
///
/// The split exists because once the history only consists of daily rollups a single day
/// cannot be broken down by hour, so a day that is cut has to be counted or excluded as a
/// whole day (which answers with a 422). The natural day attribution can always be computed
/// and the bars stay consistent with each other.
pub fn cycle_series(db: &Database, query: CycleSeriesQuery) -> Result<Value, ApiError> {
    cycle_series_inner(db, query, true)
}

/// A local read is not restricted by the LAN sharing selection.
pub(crate) fn local_cycle_series(
    db: &Database,
    query: CycleSeriesQuery,
) -> Result<Value, ApiError> {
    cycle_series_inner(db, query, false)
}

fn cycle_series_inner(
    db: &Database,
    query: CycleSeriesQuery,
    enforce_share: bool,
) -> Result<Value, ApiError> {
    let shared = if enforce_share {
        super::identity::share_settings(db).shared_app_types
    } else {
        super::default_shared_app_types()
    };
    let apps = parse_app_list(&shared, &query.app_type)?;
    // The subscription is based on the first platform.
    let app_type = apps[0].clone();
    let models: Option<Vec<String>> = match query.model.as_deref() {
        Some(value) => Some(parse_model_list(value)?),
        None => None,
    };
    let models_by_app =
        parse_models_by_app(&apps, query.models_by_app.as_deref(), models.as_ref(), true)?;
    let devices = parse_device_selection(db, query.devices.as_deref())?;
    let projection = if devices.is_local_only() {
        None
    } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else {
        Some(super::projection::database_for_peers_only(
            db,
            &devices.peers,
        )?)
    };
    let source: &Database = match &projection {
        Some(projection) => projection.as_ref(),
        None => db,
    };

    let subscription = pick_subscription(db, &app_type)?;
    let count = match query.count {
        None => 8,
        Some(value) if (1..=52).contains(&value) => value,
        Some(_) => return Err(ApiError::invalid("count 必须在 1 到 52 之间")),
    };
    let now = Local
        .timestamp_opt(Local::now().timestamp(), 0)
        .single()
        .unwrap();

    // The window of each cycle: the current cycle and count-1 cycles before it.
    let mut windows: Vec<(i32, DateTime<Local>, DateTime<Local>)> = Vec::new();
    for index in 0..count {
        let offset = -(index as i32);
        let (start, end) = crate::subscriptions::weekly_cycle(&subscription.rule, now, offset)
            .map_err(ApiError::invalid)?;
        windows.push((offset, start, end));
    }

    // The natural days that belong to each cycle: the dates whose midnight falls into the window.
    let mut spans: Vec<(NaiveDate, NaiveDate)> = Vec::new();
    for (_, start, end) in &windows {
        let first = ceil_midnight(*start)?;
        let last_exclusive = ceil_midnight(*end)?;
        spans.push((first.date_naive(), last_exclusive.date_naive()));
    }
    let range_start = spans
        .iter()
        .map(|(first, _)| *first)
        .min()
        .ok_or_else(|| ApiError::invalid("周期区间无效"))?;
    let range_end = spans
        .iter()
        .map(|(_, last)| *last)
        .max()
        .ok_or_else(|| ApiError::invalid("周期区间无效"))?;

    // The whole range consists of natural days, the data is not cut by day and therefore no 422 can occur.
    let start_ts = local_midnight(range_start)?.timestamp();
    let end_ts = local_midnight(range_end)?.timestamp();
    if end_ts <= start_ts {
        return Err(ApiError::invalid("周期区间无效"));
    }
    let app_refs: Vec<&str> = apps.iter().map(|value| value.as_str()).collect();
    let by_date = daily_metrics(
        source,
        &ResolvedRange {
            start_ts,
            end_ts_exclusive: end_ts,
            first_date: range_start,
            last_date: range_end.pred_opt().unwrap_or(range_start),
        },
        &app_refs,
        models.as_deref(),
        models_by_app.as_ref(),
    )?;

    let mut cycles = Vec::with_capacity(windows.len());
    for (index, (offset, start, end)) in windows.iter().enumerate() {
        let (first_date, last_date) = spans[index];
        let mut metrics = Metrics::default();
        let mut date = first_date;
        while date < last_date {
            if let Some(value) = by_date.get(&date) {
                metrics.merge(value);
            }
            match date.succ_opt() {
                Some(next) => date = next,
                None => break,
            }
        }
        let mut row = metrics.json();
        row["index"] = json!(offset);
        row["start"] = json!(start.to_rfc3339());
        row["end"] = json!(end.to_rfc3339());
        row["startDate"] = json!(first_date.format("%Y-%m-%d").to_string());
        row["endDate"] = json!(last_date
            .pred_opt()
            .map(|d| d.format("%Y-%m-%d").to_string()));
        // A cycle that has not run to its end is in progress.
        row["complete"] = json!(*end <= now);
        cycles.push(row);
    }

    let mut result = json!({
        "scope": if devices.is_local_only() { "local" } else { "multi" },
        "devices": device_labels(&devices),
        "appType": apps,
        "apps": apps,
        "model": models,
        "modelsByApp": models_by_app,
        "granularity": "day",
        "subscription": {
            "name": subscription.name,
            "rule": subscription.rule,
            "startDate": subscription.start_date,
            "endDate": subscription.end_date,
        },
        "cycles": cycles,
        "updatedAt": Local::now().to_rfc3339(),
    });
    result["lastScanAt"] = json!(crate::services::session_usage::last_sync_completed_at());
    Ok(result)
}

/// Round up to the next midnight in the local time zone.
fn ceil_midnight(value: DateTime<Local>) -> Result<DateTime<Local>, ApiError> {
    let midnight = local_midnight(value.date_naive())?;
    if midnight >= value {
        return Ok(midnight);
    }
    let next = value
        .date_naive()
        .succ_opt()
        .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
    local_midnight(next)
}

/// Take the aggregates of the range, grouped by local date.
///
/// The same filtering, deduplication and pricing semantics as the summary endpoint are used,
/// so the numbers match.
fn daily_metrics(
    source: &Database,
    range: &ResolvedRange,
    app_refs: &[&str],
    models: Option<&[String]>,
    models_by_app: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<BTreeMap<NaiveDate, Metrics>, ApiError> {
    let partial_dates = partial_local_dates(range)?;
    let (rows, precision_dates): (Vec<DailyUsageRow>, Vec<String>) = source.with_connection(
        |conn| -> Result<(Vec<DailyUsageRow>, Vec<String>), ApiError> {
            let precision = rollup_dates(conn, &partial_dates, app_refs, models, models_by_app)?;
            if !precision.is_empty() {
                return Ok((Vec::new(), precision));
            }
            let rows = daily_usage_rows(
                conn,
                range.start_ts,
                range.end_ts_exclusive,
                app_refs,
                models,
                models_by_app,
            )?;
            Ok((rows, Vec::new()))
        },
    )?;
    if !precision_dates.is_empty() {
        return Err(ApiError::precision(precision_dates));
    }
    let mut by_date: BTreeMap<NaiveDate, Metrics> = BTreeMap::new();
    for row in rows {
        let entry = by_date.entry(row.date).or_default();
        entry.add(
            row.request_count,
            row.total_input_tokens,
            row.total_output_tokens,
            row.total_cache_read_tokens,
            row.total_cache_creation_tokens,
            &row.total_cost,
        )?;
    }
    Ok(by_date)
}

/// Pick the subscription to use for the app: the one that is effective today comes first, then an enabled one, and finally the first entry is used as a fallback.
fn pick_subscription(
    db: &Database,
    app_type: &str,
) -> Result<crate::subscriptions::Subscription, ApiError> {
    let all = crate::subscriptions::load(db)
        .map_err(|error| ApiError::new(500, "USAGE_QUERY_FAILED", error.to_string()))?;
    let mut matched: Vec<crate::subscriptions::Subscription> = all
        .into_iter()
        .filter(|item| item.app_type == app_type)
        .collect();
    if matched.is_empty() {
        return Err(ApiError::not_found(format!(
            "该应用没有订阅记录，无法确定额度周期: {app_type}"
        )));
    }
    let today = Local::now().date_naive();
    if let Some(position) = matched.iter().position(|item| item.is_active_on(today)) {
        return Ok(matched.swap_remove(position));
    }
    if let Some(position) = matched.iter().position(|item| item.enabled) {
        return Ok(matched.swap_remove(position));
    }
    Ok(matched.remove(0))
}

/// Write a local time as an RFC 3339 string with the offset.
fn to_rfc3339(value: DateTime<Local>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Local UI reads local records regardless of the LAN sharing selection.
pub(crate) fn local_summary(db: &Database, query: SummaryQuery) -> Result<Value, ApiError> {
    summary_inner(db, query, false)
}

fn summary_inner(
    db: &Database,
    mut query: SummaryQuery,
    enforce_share: bool,
) -> Result<Value, ApiError> {
    let shared = if enforce_share {
        super::identity::share_settings(db).shared_app_types
    } else {
        super::default_shared_app_types()
    };

    // App filtering: a specified app has to be supported and selected for sharing; without a specification only the apps that may be shared are summed up.
    let apps: Vec<String> = match query.app_type.as_deref() {
        Some(value) => parse_app_list(&shared, value)?,
        None => shared.clone(),
    };

    let prefix = query.model_prefix.take();
    if prefix.is_some() && (query.model.is_some() || query.models_by_app.is_some()) {
        return Err(ApiError::invalid(
            "modelPrefix 不能与 model 或 modelsByApp 同时使用",
        ));
    }
    if query.all_time == Some(true)
        && (query.days.is_some()
            || query.start_date.is_some()
            || query.end_date.is_some()
            || query.start_time.is_some()
            || query.end_time.is_some())
    {
        return Err(ApiError::conflicting("allTime 不能与其他时间参数混用"));
    }
    // The allTime window can span years, a per-model daily would blow up the response until it is unusable.
    let want_models = query.by_model.unwrap_or(false);
    if want_models && query.all_time == Some(true) {
        return Err(ApiError::conflicting("byModel 不能与 allTime 同时使用"));
    }
    let devices = parse_device_selection(db, query.devices.as_deref())?;
    let projection = if devices.is_local_only() {
        None
    } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else {
        Some(super::projection::database_for_peers_only(
            db,
            &devices.peers,
        )?)
    };
    let source: &Database = projection.as_deref().unwrap_or(db);
    let mut no_match = false;
    if let Some(value) = prefix.as_deref() {
        let ids = prefix_models(db, &apps, query.devices.as_deref(), value)?;
        no_match = ids.is_empty();
        query.model = Some(if no_match {
            format!("__observatory_no_match_{}", uuid::Uuid::new_v4())
        } else {
            ids.join(",")
        });
    }
    let models: Option<Vec<String>> = match query.model.as_deref() {
        Some(value) => Some(parse_model_list(value)?),
        None => None,
    };
    let models_by_app: Option<BTreeMap<String, Vec<String>>> = parse_models_by_app(
        &apps,
        query.models_by_app.as_deref(),
        models.as_ref(),
        query.app_type.is_some(),
    )?;

    let group = match query.group_by.as_deref() {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid("groupBy 不能为空"));
            }
            GroupBy::parse(trimmed)?
        }
        None => GroupBy::Day,
    };

    let range = if query.all_time == Some(true) {
        let today = Local::now().date_naive();
        let mut first = today;
        for app in &apps {
            if let Some(value) = source.get_first_usage_date(Some(app), None, None)? {
                let date = parse_date(&value)?;
                if date < first {
                    first = date;
                }
            }
        }
        let tomorrow = today
            .succ_opt()
            .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
        ResolvedRange {
            start_ts: local_midnight(first)?.timestamp(),
            end_ts_exclusive: local_midnight(tomorrow)?.timestamp(),
            first_date: first,
            last_date: today,
        }
    } else {
        resolve_range(&query, Local::now())?
    };
    let partial_dates = partial_local_dates(&range)?;

    let app_refs: Vec<&str> = apps.iter().map(|value| value.as_str()).collect();
    let model_ref = models.as_deref();
    let start_ts = range.start_ts;
    let end_ts = range.end_ts_exclusive;

    // Done within a single connection snapshot: the precision check comes first, then the data is fetched.
    let (rows, precision_dates): (Vec<DailyUsageRow>, Vec<String>) = source.with_connection(
        |conn| -> Result<(Vec<DailyUsageRow>, Vec<String>), ApiError> {
            let precision = rollup_dates(
                conn,
                &partial_dates,
                &app_refs,
                model_ref,
                models_by_app.as_ref(),
            )?;
            if !precision.is_empty() {
                return Ok((Vec::new(), precision));
            }
            let rows = daily_usage_rows(
                conn,
                start_ts,
                end_ts,
                &app_refs,
                model_ref,
                models_by_app.as_ref(),
            )?;
            Ok((rows, Vec::new()))
        },
    )?;
    if !precision_dates.is_empty() {
        return Err(ApiError::precision(precision_dates));
    }

    // Grouped by local date.
    let mut by_date: BTreeMap<NaiveDate, Metrics> = BTreeMap::new();
    for row in rows {
        let entry = by_date.entry(row.date).or_default();
        entry.add(
            row.request_count,
            row.total_input_tokens,
            row.total_output_tokens,
            row.total_cache_read_tokens,
            row.total_cache_creation_tokens,
            &row.total_cost,
        )?;
    }

    let mut dates: Vec<NaiveDate> = Vec::new();
    let mut cursor = range.first_date;
    while cursor <= range.last_date {
        dates.push(cursor);
        match cursor.succ_opt() {
            Some(next) => cursor = next,
            None => break,
        }
    }

    // The daily results. Missing dates are filled with zeros, the length equals the number of local dates the range touches.
    let mut daily = Vec::with_capacity(dates.len());
    let mut total = Metrics::default();
    for date in &dates {
        let metrics = by_date.get(date).cloned().unwrap_or_default();
        total.merge(&metrics);
        let mut row = metrics.json();
        row["date"] = json!(date.format("%Y-%m-%d").to_string());
        daily.push(row);
    }

    // Grouped by natural period. The top level total comes from the same daily data, so it adds up consistently with buckets and daily.
    let mut bucket_map: BTreeMap<(String, NaiveDate, NaiveDate), Metrics> = BTreeMap::new();
    for date in &dates {
        let (period_start, period_end) = period_bounds(*date, group);
        let key = (period_label(*date, group), period_start, period_end);
        let metrics = by_date.get(date).cloned().unwrap_or_default();
        bucket_map.entry(key).or_default().merge(&metrics);
    }

    let range_last_next = range
        .last_date
        .succ_opt()
        .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
    let mut buckets = Vec::with_capacity(bucket_map.len());
    for ((label, period_start, period_end), metrics) in bucket_map {
        // The start and end of a bucket are the intersection of the natural period and the query range.
        let start_ts = if period_start <= range.first_date {
            range.start_ts
        } else {
            local_midnight(period_start)?.timestamp()
        };
        let end_ts = if period_end >= range_last_next {
            range.end_ts_exclusive
        } else {
            local_midnight(period_end)?.timestamp()
        };
        let mut row = metrics.json();
        row["period"] = json!(label);
        row["start"] = json!(local_datetime(start_ts).to_rfc3339());
        row["endExclusive"] = json!(local_datetime(end_ts).to_rfc3339());
        buckets.push(row);
    }

    let mut result = total.json();
    // The scope is `local` when only this machine is queried, `multi` once the paired devices are included.
    result["scope"] = json!(if devices.is_local_only() {
        "local"
    } else {
        "multi"
    });
    result["devices"] = json!(device_labels(&devices));
    // The filters specified in the request; null when nothing was specified. Multiple platforms are passed in comma separated.
    result["appType"] = match query.app_type.as_deref() {
        Some(value) => json!(parse_app_list(&shared, value)?),
        None => json!(null),
    };
    result["apps"] = json!(apps);
    result["model"] = match &models {
        Some(models) => json!(models),
        None => json!(null),
    };
    if let Some(selection) = &models_by_app {
        result["modelsByApp"] = json!(selection);
    }
    result["modelFilterBasis"] = json!("effectivePricingModel");
    result["days"] = json!(dates.len());
    result["startDate"] = json!(range.first_date.format("%Y-%m-%d").to_string());
    result["endDate"] = json!(range.last_date.format("%Y-%m-%d").to_string());
    result["range"] = json!({
        "start": local_datetime(range.start_ts).to_rfc3339(),
        "endExclusive": local_datetime(range.end_ts_exclusive).to_rfc3339(),
        "timezone": "server-local",
    });
    result["groupBy"] = json!(group.label());
    result["currency"] = json!("USD");
    result["inputTokenSemantics"] = json!("fresh");
    if let Some(value) = prefix {
        result["modelPrefix"] = json!(value.trim());
    }
    if no_match {
        result["model"] = json!([]);
    }
    if query.all_time == Some(true) {
        result["allTime"] = json!(true);
    }
    result["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
    result["lastScanAt"] = json!(crate::services::session_usage::last_sync_completed_at());
    result["buckets"] = json!(buckets);
    // Split by model (byModel=true): the top level query is repeated for every model, with
    // exactly the same window and semantics, including the live detail records of the current
    // day. The model list comes from the detail statistics of the range and the model /
    // modelsByApp / modelPrefix filters take effect here as well.
    if want_models {
        let window_start = to_rfc3339(local_datetime(start_ts));
        let window_end = to_rfc3339(local_datetime(end_ts));
        let mut by_model_items: Vec<Value> = Vec::new();
        for app in &apps {
            // Only the model names are needed, use the lightweight variant that skips the median.
            let candidates = source.get_model_stats_light(
                Some(start_ts),
                Some(end_ts - 1),
                Some(app),
                None,
                None,
            )?;
            for candidate in candidates {
                let model = candidate.model;
                if models.as_ref().is_some_and(|list| !list.contains(&model)) {
                    continue;
                }
                if models_by_app
                    .as_ref()
                    .and_then(|map| map.get(app))
                    .is_some_and(|list| !list.contains(&model))
                {
                    continue;
                }
                let usage = summary_inner(
                    db,
                    SummaryQuery {
                        app_type: Some(app.clone()),
                        model: Some(model.clone()),
                        devices: query.devices.clone(),
                        start_time: Some(window_start.clone()),
                        end_time: Some(window_end.clone()),
                        ..Default::default()
                    },
                    true,
                )?;
                by_model_items.push(json!({"appType":app,"model":model,"usage":usage}));
            }
        }
        by_model_items.sort_by(|a, b| {
            b["usage"]["totalTokens"]
                .as_u64()
                .cmp(&a["usage"]["totalTokens"].as_u64())
        });
        result["models"] = json!(by_model_items);
    }
    result["daily"] = json!(daily);
    Ok(result)
}

/// Read-only dashboard aggregation. Time windows are half-open and clipped to
/// the configured subscription; monthly anniversaries clamp to the month's end.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardQuery {
    pub app_type: String,
    #[serde(default)]
    pub model_prefix: Option<String>,
    #[serde(default)]
    pub billing_offset: i32,
    #[serde(default)]
    pub quota_offset: i32,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models_by_app: Option<String>,
    #[serde(default)]
    pub devices: Option<String>,
}

fn anniversary(anchor: NaiveDate, months: u32) -> Result<NaiveDate, ApiError> {
    anchor
        .checked_add_months(chrono::Months::new(months))
        .ok_or_else(|| ApiError::invalid("订阅账期超出日期范围"))
}

pub fn dashboard_usage(db: &Database, mut query: DashboardQuery) -> Result<Value, ApiError> {
    if let Some(prefix) = query.model_prefix.take() {
        if query.model.is_some() || query.models_by_app.is_some() {
            return Err(ApiError::invalid(
                "modelPrefix 不能与 model 或 modelsByApp 同时使用",
            ));
        }
        let shared = super::identity::share_settings(db).shared_app_types;
        let apps = parse_app_list(&shared, &query.app_type)?;
        let ids = prefix_models(db, &apps, query.devices.as_deref(), &prefix)?;
        query.model = Some(if ids.is_empty() {
            format!("__observatory_no_match_{}", uuid::Uuid::new_v4())
        } else {
            ids.join(",")
        });
        let mut result = dashboard_usage(db, query)?;
        // Never expose the internal no-match sentinel as a real model selection.
        fn clean(value: &mut Value) {
            if let Some(obj) = value.as_object_mut() {
                if obj.get("model").and_then(Value::as_array).is_some_and(|v| {
                    v.iter().any(|v| {
                        v.as_str()
                            .is_some_and(|s| s.starts_with("__observatory_no_match_"))
                    })
                }) {
                    obj.insert("model".into(), json!([]));
                }
                for child in obj.values_mut() {
                    clean(child);
                }
            } else if let Some(items) = value.as_array_mut() {
                for child in items {
                    clean(child);
                }
            }
        }
        if ids.is_empty() {
            clean(&mut result);
        }
        result["modelPrefix"] = json!(prefix.trim());
        return Ok(result);
    }
    if !(-120..=0).contains(&query.billing_offset) {
        return Err(ApiError::invalid("billingOffset 必须在 -120 到 0 之间"));
    }
    if !(-52..=0).contains(&query.quota_offset) {
        return Err(ApiError::invalid("quotaOffset 必须在 -52 到 0 之间"));
    }
    let shared = super::identity::share_settings(db).shared_app_types;
    let apps = parse_app_list(&shared, &query.app_type)?;
    let selected_models = query.model.as_deref().map(parse_model_list).transpose()?;
    let models_by_app = parse_models_by_app(
        &apps,
        query.models_by_app.as_deref(),
        selected_models.as_ref(),
        true,
    )?;
    let devices = parse_device_selection(db, query.devices.as_deref())?;
    let projection = if devices.is_local_only() {
        None
    } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else {
        Some(super::projection::database_for_peers_only(
            db,
            &devices.peers,
        )?)
    };
    let source = projection.as_deref().unwrap_or(db);
    let sub = pick_subscription(db, &apps[0])?;
    let start_date = parse_date(&sub.start_date)?;
    let end_date = parse_date(&sub.end_date)?;
    let now = Local
        .timestamp_opt(Local::now().timestamp(), 0)
        .single()
        .unwrap();
    let today = now.date_naive();
    if today < start_date || !sub.enabled {
        return Err(ApiError::cycle_unavailable("订阅尚未生效或未启用"));
    }
    let effective_date = std::cmp::min(
        today,
        end_date
            .pred_opt()
            .ok_or_else(|| ApiError::invalid("订阅日期无效"))?,
    );
    let mut index: u32 = 0;
    let period_days = if sub.billing == "期" {
        sub.custom_interval_days.map(i64::from)
    } else {
        None
    };
    let boundary = |n: u32| -> Result<NaiveDate, ApiError> {
        if let Some(days) = period_days {
            return start_date
                .checked_add_signed(Duration::days(days * i64::from(n)))
                .ok_or_else(|| ApiError::invalid("订阅账期超出范围"));
        }
        let factor = if sub.billing == "年" { 12 } else { 1 };
        anniversary(
            start_date,
            n.checked_mul(factor)
                .ok_or_else(|| ApiError::invalid("订阅账期超出范围"))?,
        )
    };
    while boundary(index + 1)? <= effective_date {
        index += 1;
        if index > 12000 {
            return Err(ApiError::invalid("订阅账期过多"));
        }
    }
    let selected = i64::from(index) + i64::from(query.billing_offset);
    if selected < 0 {
        return Err(ApiError::cycle_unavailable("没有更早的订阅账期"));
    }
    let start = local_midnight(boundary(selected as u32)?)?;
    let end = local_midnight(std::cmp::min(boundary(selected as u32 + 1)?, end_date))?;
    let data_end = std::cmp::min(end, now);
    if data_end <= start {
        return Err(ApiError::cycle_unavailable("账期没有可查询的数据"));
    }
    let summary = |a: DateTime<Local>, b: DateTime<Local>| -> Result<Value, ApiError> {
        summary_inner(
            db,
            SummaryQuery {
                app_type: Some(query.app_type.clone()),
                model: query.model.clone(),
                models_by_app: query.models_by_app.clone(),
                devices: query.devices.clone(),
                start_time: Some(to_rfc3339(a)),
                end_time: Some(to_rfc3339(b)),
                ..Default::default()
            },
            true,
        )
    };
    let total = summary(start, data_end)?;
    let mut cycles = Vec::new();
    // Enumerate backwards from the request time, then reverse into chronological order.
    for n in 0..5200 {
        let (a, b) =
            crate::subscriptions::weekly_cycle(&sub.rule, now, -n).map_err(ApiError::invalid)?;
        if b <= start {
            break;
        }
        let from = std::cmp::max(a, start);
        let to = std::cmp::min(b, data_end);
        if from < to {
            cycles.push(
                json!({"cycleStart":to_rfc3339(a),"cycleEndExclusive":to_rfc3339(b),
                "countedStart":to_rfc3339(from),"countedEndExclusive":to_rfc3339(to),
                "partial":from != a || to != b,"usage":summary(from,to)?}),
            );
        }
        if n == 5199 {
            return Err(ApiError::invalid("订阅额度周期过多"));
        }
    }
    cycles.reverse();
    let (week_start, week_end) =
        crate::subscriptions::weekly_cycle(&sub.rule, now, query.quota_offset)
            .map_err(ApiError::invalid)?;
    let week_from = std::cmp::max(week_start, local_midnight(start_date)?);
    let week_to = std::cmp::min(std::cmp::min(week_end, local_midnight(end_date)?), now);
    let selected_week = if week_from < week_to {
        Some(summary(week_from, week_to)?)
    } else {
        None
    };
    let mut by_app_model = Vec::new();
    for app in &apps {
        // Only the model names are needed, use the lightweight variant that skips the median.
        let candidates = source.get_model_stats_light(
            Some(start.timestamp()),
            Some(data_end.timestamp() - 1),
            Some(app),
            None,
            None,
        )?;
        for candidate in candidates {
            let model = candidate.model;
            if selected_models
                .as_ref()
                .is_some_and(|v| !v.contains(&model))
                || models_by_app
                    .as_ref()
                    .and_then(|v| v.get(app))
                    .is_some_and(|v| !v.contains(&model))
            {
                continue;
            }
            let usage = summary_inner(
                db,
                SummaryQuery {
                    app_type: Some(app.clone()),
                    model: Some(model.clone()),
                    devices: query.devices.clone(),
                    start_time: Some(to_rfc3339(start)),
                    end_time: Some(to_rfc3339(data_end)),
                    ..Default::default()
                },
                true,
            )?;
            by_app_model.push(json!({"appType":app,"model":model,"usage":usage}));
        }
    }
    by_app_model.sort_by(|a, b| {
        b["usage"]["totalTokens"]
            .as_u64()
            .cmp(&a["usage"]["totalTokens"].as_u64())
    });
    Ok(
        json!({"scope":{"subscriptionApp":apps[0],"devices":query.devices,"timezone":"server-local"},
        "subscription":{"id":sub.id,"name":sub.name,"billing":sub.billing,
            "billingPeriod":{"offset":query.billing_offset,"start":to_rfc3339(start),"endExclusive":to_rfc3339(end),"dataEndExclusive":to_rfc3339(data_end)}},
        "billingTotal":total,"quotaCycles":cycles,"billingAggregations":{"byAppAndModel":by_app_model},
        "selectedQuotaCycle":{"offset":query.quota_offset,"cycleStart":to_rfc3339(week_start),"cycleEndExclusive":to_rfc3339(week_end),"usage":selected_week},
        "updatedAt":to_rfc3339(now)}),
    )
}

/// Query models across shared platforms, using one platform's subscription.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardModelsQuery {
    pub platform_code: String,
    #[serde(default)]
    pub model_prefix: Option<String>,
    #[serde(default)]
    pub model_prefixes: Vec<String>,
    #[serde(default)]
    pub model_ids: Vec<String>,
    #[serde(default)]
    pub billing_offset: i32,
    #[serde(default)]
    pub quota_offset: i32,
    #[serde(default)]
    pub devices: Option<String>,
}

pub fn dashboard_models(db: &Database, query: DashboardModelsQuery) -> Result<Value, ApiError> {
    let shared = super::identity::share_settings(db).shared_app_types;
    let platform = resolve_shared_app(&shared, &query.platform_code)?;
    if query.model_ids.len() > 50 {
        return Err(ApiError::invalid("modelIds 最多 50 项"));
    }
    let mut models = Vec::new();
    for id in &query.model_ids {
        let trimmed = id.trim();
        if trimmed.is_empty() || trimmed.len() > 256 || trimmed.contains(',') {
            return Err(ApiError::invalid("模型 ID 不能为空、过长或包含逗号"));
        }
        if !models.iter().any(|v| v == trimmed) {
            models.push(trimmed.to_string());
        }
    }
    if query.model_prefixes.len() > 50 {
        return Err(ApiError::invalid("modelPrefixes 最多 50 项"));
    }
    // Preserve the original single-prefix contract; plural prefixes can be
    // combined with exact IDs using OR semantics.
    if query.model_prefix.is_some() && (!models.is_empty() || !query.model_prefixes.is_empty()) {
        return Err(ApiError::invalid(
            "modelPrefix 不能与 modelIds 或 modelPrefixes 同时使用",
        ));
    }
    let mut prefixes = Vec::new();
    for item in &query.model_prefixes {
        let prefix = item.trim();
        if prefix.is_empty() || prefix.len() > 256 {
            return Err(ApiError::invalid("模型前缀不能为空或超过 256 字节"));
        }
        if !prefixes
            .iter()
            .any(|v: &String| v.eq_ignore_ascii_case(prefix))
        {
            prefixes.push(prefix.to_owned());
        }
    }
    let mut apps = vec![platform];
    if !models.is_empty() || query.model_prefix.is_some() || !prefixes.is_empty() {
        for app in shared {
            if !apps.contains(&app) {
                apps.push(app);
            }
        }
    }
    let mut resolved = models.clone();
    for prefix in &prefixes {
        for id in prefix_models(db, &apps, query.devices.as_deref(), prefix)? {
            if !resolved.contains(&id) {
                resolved.push(id);
            }
        }
    }
    let no_matches = !prefixes.is_empty() && resolved.is_empty();
    if no_matches {
        resolved.push(format!("__observatory_no_match_{}", uuid::Uuid::new_v4()));
    }
    let mut value = dashboard_usage(
        db,
        DashboardQuery {
            app_type: apps.join(","),
            model_prefix: query.model_prefix,
            billing_offset: query.billing_offset,
            quota_offset: query.quota_offset,
            model: if resolved.is_empty() {
                None
            } else {
                Some(resolved.join(","))
            },
            models_by_app: None,
            devices: query.devices,
        },
    )?;
    if no_matches {
        fn clean(value: &mut Value) {
            if let Some(obj) = value.as_object_mut() {
                if obj.get("model").and_then(Value::as_array).is_some_and(|v| {
                    v.iter().any(|v| {
                        v.as_str()
                            .is_some_and(|s| s.starts_with("__observatory_no_match_"))
                    })
                }) {
                    obj.insert("model".into(), json!([]));
                }
                for child in obj.values_mut() {
                    clean(child);
                }
            } else if let Some(items) = value.as_array_mut() {
                for child in items {
                    clean(child);
                }
            }
        }
        clean(&mut value);
    }
    if !prefixes.is_empty() {
        value["modelPrefixes"] = json!(prefixes);
    }
    value["modelIds"] = json!(models);
    Ok(value)
}

/// Resolve prefixes against effective pricing models across the selected devices.
fn prefix_models(
    db: &Database,
    apps: &[String],
    device_filter: Option<&str>,
    prefix: &str,
) -> Result<Vec<String>, ApiError> {
    let prefix = prefix.trim();
    if prefix.is_empty() || prefix.len() > 256 {
        return Err(ApiError::invalid("modelPrefix 不能为空或超过 256 字节"));
    }
    let devices = parse_device_selection(db, device_filter)?;
    let projection = if devices.is_local_only() {
        None
    } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else {
        Some(super::projection::database_for_peers_only(
            db,
            &devices.peers,
        )?)
    };
    let source = projection.as_deref().unwrap_or(db);
    let needle = prefix.to_ascii_lowercase();
    let mut ids = Vec::new();
    for app in apps {
        // Only the model names are needed, use the lightweight variant that skips the median.
        for row in source.get_model_stats_light(None, None, Some(app), None, None)? {
            if row.model.to_ascii_lowercase().starts_with(&needle) && !ids.contains(&row.model) {
                if row.model.contains(',') {
                    return Err(ApiError::invalid(
                        "匹配的模型 ID 包含逗号，暂不支持前缀查询",
                    ));
                }
                ids.push(row.model);
            }
        }
    }
    Ok(ids)
}
