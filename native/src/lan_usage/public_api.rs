//! 只读用量查询接口：给 TinyPane、自动化脚本和其他 HTTP 客户端提供统计查询。
//!
//! 协议见 `docs/AI用量查询接口设计.md`。要点：
//!
//! - 只查本机保存的统计，不聚合其他设备快照；
//! - 顶层是「所选区间的合计」，`daily` 与 `buckets` 是同一批数据的两种分组，
//!   三者加总必须一致；
//! - 金额内部用十进制定点数累加，输出为 JSON number；
//! - 全响应在同一个连接快照内产出，避免后台扫描写入时各项互相矛盾；
//! - 历史只剩日汇总时，区间切天时无法按比例拆分，返回 422 而不是偏低数字。

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

/// 接口错误：HTTP 状态 + 稳定错误码 + 可选细节。
///
/// 保留字符串 `error` 以兼容 v0.1.30 的客户端，新增 `code` 供程序判断。
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

    /// 历史日汇总无法支持所要求的秒级范围。
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
            AppError::HttpStatus {
                status: 403,
                body,
            } => Self::not_shared(body),
            other => Self::new(500, "USAGE_QUERY_FAILED", other.to_string()),
        }
    }
}

/// 查询参数。`deny_unknown_fields` 让未知参数直接被拒绝。
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SummaryQuery {
    /// 最近 N 个日历日（1–366），含结束日在内。
    pub days: Option<u32>,
    /// 结束日期 `YYYY-MM-DD`，默认服务器今天。
    pub end_date: Option<String>,
    /// 开始日期 `YYYY-MM-DD`，两端都包含。不能与 `days` 同时使用。
    pub start_date: Option<String>,
    /// 秒级区间开始时刻，RFC 3339，必须带 UTC 偏移。
    pub start_time: Option<String>,
    /// 秒级区间结束时刻，不包含。
    pub end_time: Option<String>,
    /// 应用标识。`claude` 仅指 Claude Code，`claude-desktop` 单独查询。
    pub app_type: Option<String>,
    /// 按有效计价模型精确筛选，口径与页面一致。
    pub model: Option<String>,
    /// 分组方式：`day`（默认）、`week`、`month`。
    pub group_by: Option<String>,
}

/// 聚合粒度。
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

/// 一次查询的用量指标。金额用十进制定点数累加，避免浮点误差在分组之间放大。
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

/// 归一化后的查询区间：左闭右开 `[start_ts, end_ts_exclusive)`。
struct ResolvedRange {
    start_ts: i64,
    end_ts_exclusive: i64,
    /// 区间触及的首末本地日期（含）。
    first_date: NaiveDate,
    last_date: NaiveDate,
}

/// 本地午夜的取值规则。
///
/// - 唯一时刻：直接采用；
/// - 重复午夜（夏令时回拨）：取较早时刻（沿用 v0.1.30 的 `.earliest()` 规则）；
/// - 不存在的午夜（夏令时前拨跳过 00:00）：返回 400 `INVALID_LOCAL_BOUNDARY`，
///   不静默平移到别的时间。
///
/// 单独成函数是为了能对三种结果做确定性测试：夏令时规则依赖操作系统时区，
/// 而 `chrono::Local` 在 Windows 上不跟随 `TZ` 环境变量，无法在测试里切换。
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

/// 本地午夜。
fn local_midnight(date: NaiveDate) -> Result<DateTime<Local>, ApiError> {
    let naive = date
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| ApiError::boundary(format!("日期 {date} 无效")))?;
    resolve_local_midnight(Local.from_local_datetime(&naive), date)
}

/// 时间戳对应的本地时刻。
fn local_datetime(timestamp: i64) -> DateTime<Local> {
    Local
        .timestamp_opt(timestamp, 0)
        .earliest()
        .unwrap_or_else(|| Local::now())
}

fn parse_date(value: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(value.trim(), "%Y-%m-%d")
        .map_err(|_| ApiError::invalid(format!("日期必须为 YYYY-MM-DD: {value}")))
}

/// RFC 3339 时间，必须带 UTC 偏移；不接受非零的小数秒。
fn parse_instant(value: &str) -> Result<DateTime<FixedOffset>, ApiError> {
    let parsed = DateTime::parse_from_rfc3339(value.trim()).map_err(|_| {
        ApiError::invalid(format!("时间必须为带 UTC 偏移的 RFC 3339: {value}"))
    })?;
    if parsed.timestamp_subsec_nanos() != 0 {
        return Err(ApiError::invalid(format!("时间必须精确到整秒: {value}")));
    }
    Ok(parsed)
}

/// 归一化三种时间选择方式。
///
/// - 最近 N 天：`days` + 可选 `endDate`；
/// - 指定日期区间：`startDate` + 可选 `endDate`；
/// - 指定秒级区间：`startTime` + `endTime`（互斥于前两者）。
fn resolve_range(query: &SummaryQuery, today: NaiveDate) -> Result<ResolvedRange, ApiError> {
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
            .ok_or_else(|| ApiError::conflicting("startTime 与 endTime 必须成对提供"))?;
        let end_value = query
            .end_time
            .as_deref()
            .ok_or_else(|| ApiError::conflicting("startTime 与 endTime 必须成对提供"))?;
        let start = parse_instant(start_value)?;
        let end = parse_instant(end_value)?;
        if start >= end {
            return Err(ApiError::date_range("startTime 必须早于 endTime"));
        }
        let tomorrow = today
            .succ_opt()
            .ok_or_else(|| ApiError::date_range("日期超出范围"))?;
        if end.with_timezone(&Local) > local_midnight(tomorrow)? {
            return Err(ApiError::date_range(
                "endTime 不能晚于服务器明天的本地午夜",
            ));
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

/// 自然周期（日 / ISO 周 / 月）的起止本地日期。
fn period_bounds(date: NaiveDate, group: GroupBy) -> (NaiveDate, NaiveDate) {
    match group {
        GroupBy::Day => (
            date,
            date.succ_opt().unwrap_or(date),
        ),
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

/// 自然周期的标识。日为 `YYYY-MM-DD`，周为 ISO 周编号 `YYYY-Www`，月为 `YYYY-MM`。
///
/// ISO 周使用该周所属的年份，避免跨年周编号错误。
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

/// 区间没有完整覆盖的本地日期。
///
/// 日期区间总是覆盖整天，只有秒级区间会切半某一天。被切半的日期如果只剩
/// 日汇总（日粒度），就无法按比例拆分，只能整日计入或整日排除。
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

/// 汇总查询。
///
/// 返回的顶层指标是「所选区间的合计」，`daily` 与 `buckets` 是同一批数据的两种
/// 分组方式，三者加总一致。
pub fn summary(db: &Database, query: SummaryQuery) -> Result<Value, ApiError> {
    let shared = super::identity::share_settings(db).shared_app_types;

    // 应用筛选：指定的应用必须受支持且已勾选共享；不指定时只合计允许共享的应用。
    let apps: Vec<String> = match query.app_type.as_deref() {
        Some(value) => {
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
            vec![app.to_owned()]
        }
        None => shared.clone(),
    };

    let model = match query.model.as_deref() {
        Some(value) => {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                return Err(ApiError::invalid("model 不能为空"));
            }
            Some(trimmed.to_owned())
        }
        None => None,
    };

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

    let today = Local::now().date_naive();
    let range = resolve_range(&query, today)?;
    let partial_dates = partial_local_dates(&range)?;

    let app_refs: Vec<&str> = apps.iter().map(|value| value.as_str()).collect();
    let model_ref = model.as_deref();
    let start_ts = range.start_ts;
    let end_ts = range.end_ts_exclusive;

    // 一次连接快照内完成：先做精度检测，再取数。
    let (rows, precision_dates): (Vec<DailyUsageRow>, Vec<String>) =
        db.with_connection(|conn| -> Result<(Vec<DailyUsageRow>, Vec<String>), ApiError> {
            let precision = rollup_dates(conn, &partial_dates, &app_refs, model_ref)?;
            if !precision.is_empty() {
                return Ok((Vec::new(), precision));
            }
            let rows = daily_usage_rows(conn, start_ts, end_ts, &app_refs, model_ref)?;
            Ok((rows, Vec::new()))
        })?;
    if !precision_dates.is_empty() {
        return Err(ApiError::precision(precision_dates));
    }

    // 按本地日期归集。
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

    // 每日结果。空日期补零，长度等于区间触及的本地日期数。
    let mut daily = Vec::with_capacity(dates.len());
    let mut total = Metrics::default();
    for date in &dates {
        let metrics = by_date.get(date).cloned().unwrap_or_default();
        total.merge(&metrics);
        let mut row = metrics.json();
        row["date"] = json!(date.format("%Y-%m-%d").to_string());
        daily.push(row);
    }

    // 按自然周期归集。顶层合计来自同一批日数据，所以与 buckets、daily 加总一致。
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
        // bucket 的起止是自然周期与查询区间的交集。
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
    result["scope"] = json!("local");
    result["appType"] = json!(query.app_type);
    result["apps"] = json!(apps);
    result["model"] = json!(model);
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
    result["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
    result["lastScanAt"] = json!(crate::services::session_usage::last_sync_completed_at());
    result["buckets"] = json!(buckets);
    result["daily"] = json!(daily);
    Ok(result)
}
