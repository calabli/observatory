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

    /// 找不到对应的记录（比如该应用没有订阅）。
    fn not_found(message: impl Into<String>) -> Self {
        Self::new(404, "SUBSCRIPTION_NOT_FOUND", message)
    }

    /// 该周期暂时没有可统计的区间（尚未开始，或不在订阅有效期内）。
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
    /// JSON object mapping selected app IDs to their selected model IDs.
    pub models_by_app: Option<String>,
    /// 分组方式：`day`（默认）、`week`、`month`。
    pub group_by: Option<String>,
    /// 设备集合，逗号分隔。默认只查本机。
    ///
    /// `local` 指本机；其余填配对设备的 `deviceId`。不填时等价于 `local`。
    pub devices: Option<String>,
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

/// 校验并归一化应用标识：必须受支持、且已勾选共享。
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

/// 解析应用标识列表：逗号分隔，重复的去掉，顺序保持请求里的顺序。
///
/// 空段（`claude,,codex`）按无效处理，不当成「所有应用」。
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

/// 解析 `modelsByApp`：平台 → 模型 ID 数组的 JSON 对象。
///
/// 必须配合 `appType` 使用，且不能与 `model` 同时出现。映射里的平台必须
/// 在 `appType` 里；已选平台没出现在映射中时，它的全部模型都参与统计。
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

/// 解析模型标识列表：逗号分隔，精确匹配。
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

/// 一次查询涉及的设备集合。
///
/// `local` 是本机的别名，其余是配对设备的 `device_id`。
struct DeviceSelection {
    local: bool,
    peers: Vec<String>,
}

impl DeviceSelection {
    /// 是否只有本机——只有本机时不必走投影，直接查本机库。
    fn is_local_only(&self) -> bool {
        self.local && self.peers.is_empty()
    }
}

/// 解析设备集合。默认只查本机。
fn parse_device_selection(
    db: &Database,
    value: Option<&str>,
) -> Result<DeviceSelection, ApiError> {
    let Some(value) = value else {
        return Ok(DeviceSelection {
            local: true,
            peers: Vec::new(),
        });
    };
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
    // 设备必须存在且已启用，否则无法确定它的用量。
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

/// 设备集合的对外表示：本机用 `local` 表示，配对设备用它的 `deviceId`。
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

/// 已启用且配对成功的设备。
fn known_devices(db: &Database) -> Result<Vec<String>, AppError> {    db.with_connection(|conn| {
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
/// - 指定秒级区间：`startTime` + 可选 `endTime`（省略时为请求当前时间）。
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
        let end = match query.end_time.as_deref().map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => parse_instant(value)?,
            // Use one captured request time, truncated to a whole second.
            None => Local.timestamp_opt(now.timestamp(), 0).single().unwrap().fixed_offset(),
        };
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
    summary_inner(db, query, true)
}

/// 周期查询参数。`deny_unknown_fields` 让未知参数直接被拒绝。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleQuery {
    /// 应用标识（Agent 平台），多个用逗号分隔。必填。
    ///
    /// 周期规则**以第一个平台为准**——周期只有一个，用哪个平台的订阅来定义
    /// 就要明确。其余平台只参与用量统计。
    pub app_type: String,
    /// 周期序号：0 是当前周期（默认），-1 是上一个，1 是下一个。
    #[serde(default)]
    pub offset: Option<i32>,
    /// 模型标识，多个用逗号分隔。只统计这些模型。
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub models_by_app: Option<String>,
    /// 设备集合，逗号分隔。默认只查本机。
    #[serde(default)]
    pub devices: Option<String>,
    /// 周期内的分组方式，默认按天。
    #[serde(default)]
    pub group_by: Option<String>,
}

/// 查询某个 Agent 平台当前额度周期内的用量。
///
/// 返回体与 `/api/v1/usage/summary` **完全一致**：周期只是替调用方把
/// `startTime` / `endTime` 算好，聚合口径、字段含义、错误码都不变。
/// 周期边界可以从 `range.start` 与 `range.endExclusive` 读出。
pub fn cycle_summary(db: &Database, query: CycleQuery) -> Result<Value, ApiError> {
    cycle_summary_inner(db, query, true)
}

/// 本机读取不受局域网共享勾选限制。
pub(crate) fn local_cycle_summary(
    db: &Database,
    query: CycleQuery,
) -> Result<Value, ApiError> {
    cycle_summary_inner(db, query, false)
}

fn cycle_summary_inner(
    db: &Database,
    query: CycleQuery,
    enforce_share: bool,
) -> Result<Value, ApiError> {
    // 先做共享校验：没勾选共享的应用不该泄露它有没有订阅。
    let shared = if enforce_share {
        super::identity::share_settings(db).shared_app_types
    } else {
        super::default_shared_app_types()
    };
    let apps = parse_app_list(&shared, &query.app_type)?;
    // 订阅以第一个平台为准。
    let app_type = apps[0].clone();
    let offset = query.offset.unwrap_or(0);
    if !(-52..=52).contains(&offset) {
        return Err(ApiError::invalid("offset 必须在 -52 到 52 之间"));
    }

    let subscription = pick_subscription(db, &app_type)?;

    // 请求时刻截到整秒，与汇总接口的口径一致。
    let now = Local.timestamp_opt(Local::now().timestamp(), 0).single().unwrap();
    let (cycle_start, cycle_end) =
        crate::subscriptions::weekly_cycle(&subscription.rule, now, offset)
            .map_err(|message| ApiError::invalid(message))?;

    // 周期尚未开始：还没有可统计的区间。
    if cycle_start >= now {
        return Err(ApiError::cycle_unavailable(format!(
            "该周期尚未开始: {app_type}"
        )));
    }

    // 周期按订阅有效期收敛：开头取较晚者，结尾取较早者；未来的用量还不存在，
    // 结尾再截到当前时刻。
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

    // 走同一套汇总逻辑，字段与统计口径都与 summary 一致。
    // 用量统计覆盖请求里的全部平台，周期边界只来自第一个平台。
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

/// 周期序列查询参数。`deny_unknown_fields` 让未知参数直接被拒绝。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CycleSeriesQuery {
    /// 应用标识（Agent 平台），多个用逗号分隔。必填。
    ///
    /// 周期规则**以第一个平台为准**，与 `/api/v1/usage/cycle` 一致。
    pub app_type: String,
    /// 返回多少个周期，含当前周期。默认 8，上限 52。
    #[serde(default)]
    pub count: Option<u32>,
    /// 模型标识，多个用逗号分隔。
    #[serde(default)]
    pub model: Option<String>,
    /// 按平台分别指定模型 ID，JSON 字符串。
    #[serde(default)]
    pub models_by_app: Option<String>,
    /// 设备集合，逗号分隔。默认只查本机。
    #[serde(default)]
    pub devices: Option<String>,
}

/// 查询某个 Agent 平台最近若干个额度周期的用量序列。
///
/// 给看板用：一次拿到「本周期 + 前 N 个周期」的柱状图数据。
///
/// 与 `/api/v1/usage/cycle` 的区别在**归集口径**：
///
/// - 单周期端点按秒级区间统计，区间两端会切开某个自然日；
/// - 序列端点按**自然日归属**统计——每个自然日整体归入「午夜落在哪个周期」
///   的那个周期。
///
/// 之所以这样分，是因为历史只剩每日汇总时无法把某一天按小时拆开，切开的
/// 那天只能整日计入或整日排除（那会返回 422）。自然日归属永远可算，柱子
/// 之间也口径一致。
pub fn cycle_series(db: &Database, query: CycleSeriesQuery) -> Result<Value, ApiError> {
    cycle_series_inner(db, query, true)
}

/// 本机读取不受局域网共享勾选限制。
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
    // 订阅以第一个平台为准。
    let app_type = apps[0].clone();
    let models: Option<Vec<String>> = match query.model.as_deref() {
        Some(value) => Some(parse_model_list(value)?),
        None => None,
    };
    let models_by_app = parse_models_by_app(&apps, query.models_by_app.as_deref(), models.as_ref(), true)?;
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

    // 各周期窗口：当前周期往前数 count-1 个。
    let mut windows: Vec<(i32, DateTime<Local>, DateTime<Local>)> = Vec::new();
    for index in 0..count {
        let offset = -(index as i32);
        let (start, end) =
            crate::subscriptions::weekly_cycle(&subscription.rule, now, offset)
                .map_err(|message| ApiError::invalid(message))?;
        windows.push((offset, start, end));
    }

    // 每个周期归属的自然日：午夜落在窗口内的那些日期。
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

    // 整段都是自然日，取数不会切天，因此不会出现 422。
    let start_ts = local_midnight(range_start)?.timestamp();
    let end_ts = local_midnight(range_end)?.timestamp();
    if end_ts <= start_ts {
        return Err(ApiError::invalid("周期区间无效"));
    }
    let app_refs: Vec<&str> = apps.iter().map(|value| value.as_str()).collect();
    let by_date = daily_metrics(
        source,
        start_ts,
        end_ts,
        range_start,
        range_end.pred_opt().unwrap_or(range_start),
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
        row["endDate"] = json!(last_date.pred_opt().map(|d| d.format("%Y-%m-%d").to_string()));
        // 周期还没走完就是进行中。
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

/// 向上取整到本机时区的下一个午夜。
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

/// 取区间内按本地日期归集的聚合值。
///
/// 与汇总接口用同一套筛选、去重与定价口径，所以数字一致。
fn daily_metrics(
    source: &Database,
    start_ts: i64,
    end_ts_exclusive: i64,
    first_date: NaiveDate,
    last_date: NaiveDate,
    app_refs: &[&str],
    models: Option<&[String]>,
    models_by_app: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<BTreeMap<NaiveDate, Metrics>, ApiError> {
    let range = ResolvedRange {
        start_ts,
        end_ts_exclusive,
        first_date,
        last_date,
    };
    let partial_dates = partial_local_dates(&range)?;
    let (rows, precision_dates): (Vec<DailyUsageRow>, Vec<String>) =
        source.with_connection(|conn| -> Result<(Vec<DailyUsageRow>, Vec<String>), ApiError> {
            let precision =
                rollup_dates(conn, &partial_dates, app_refs, models, models_by_app)?;
            if !precision.is_empty() {
                return Ok((Vec::new(), precision));
            }
            let rows = daily_usage_rows(
                conn,
                start_ts,
                end_ts_exclusive,
                app_refs,
                models,
                models_by_app,
            )?;
            Ok((rows, Vec::new()))
        })?;
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

/// 挑出该应用要用的订阅：优先今天生效的，其次是启用的，最后退回第一条。
fn pick_subscription(
    db: &Database,
    app_type: &str,
) -> Result<crate::subscriptions::Subscription, ApiError> {
    let all = crate::subscriptions::load(db).map_err(|error| {
        ApiError::new(500, "USAGE_QUERY_FAILED", error.to_string())
    })?;
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

/// 本地时刻写成带偏移的 RFC 3339。
fn to_rfc3339(value: DateTime<Local>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

/// Local UI reads local records regardless of the LAN sharing selection.
pub(crate) fn local_summary(db: &Database, query: SummaryQuery) -> Result<Value, ApiError> {
    summary_inner(db, query, false)
}

fn summary_inner(db: &Database, query: SummaryQuery, enforce_share: bool) -> Result<Value, ApiError> {
    let shared = if enforce_share { super::identity::share_settings(db).shared_app_types } else { super::default_shared_app_types() };

    // 应用筛选：指定的应用必须受支持且已勾选共享；不指定时只合计允许共享的应用。
    let apps: Vec<String> = match query.app_type.as_deref() {
        Some(value) => parse_app_list(&shared, value)?,
        None => shared.clone(),
    };

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

    let range = resolve_range(&query, Local::now())?;
    let partial_dates = partial_local_dates(&range)?;

    // 设备集合：默认只查本机；指定配对设备时走投影，把那些设备的用量并进来。
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

    let app_refs: Vec<&str> = apps.iter().map(|value| value.as_str()).collect();
    let model_ref = models.as_deref();
    let start_ts = range.start_ts;
    let end_ts = range.end_ts_exclusive;

    // 一次连接快照内完成：先做精度检测，再取数。
    let (rows, precision_dates): (Vec<DailyUsageRow>, Vec<String>) =
        source.with_connection(|conn| -> Result<(Vec<DailyUsageRow>, Vec<String>), ApiError> {
            let precision = rollup_dates(conn, &partial_dates, &app_refs, model_ref, models_by_app.as_ref())?;
            if !precision.is_empty() {
                return Ok((Vec::new(), precision));
            }
            let rows = daily_usage_rows(conn, start_ts, end_ts, &app_refs, model_ref, models_by_app.as_ref())?;
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
    // 只查本机时 scope 是 `local`；并入配对设备后是 `multi`。
    result["scope"] = json!(if devices.is_local_only() { "local" } else { "multi" });
    result["devices"] = json!(device_labels(&devices));
    // 请求里指定的筛选条件；没指定就是 null。多个平台用逗号分隔传入。
    result["appType"] = match query.app_type.as_deref() {
        Some(value) => json!(parse_app_list(&shared, value)?),
        None => json!(null),
    };
    result["apps"] = json!(apps);
    result["model"] = match &models {
        Some(models) => json!(models),
        None => json!(null),
    };
    if let Some(selection) = &models_by_app { result["modelsByApp"] = json!(selection); }
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

/// Read-only dashboard aggregation. Time windows are half-open and clipped to
/// the configured subscription; monthly anniversaries clamp to the month's end.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardQuery {
    pub app_type: String,
    #[serde(default)] pub model_prefix: Option<String>,
    #[serde(default)] pub billing_offset: i32,
    #[serde(default)] pub quota_offset: i32,
    #[serde(default)] pub model: Option<String>,
    #[serde(default)] pub models_by_app: Option<String>,
    #[serde(default)] pub devices: Option<String>,
}

fn anniversary(anchor: NaiveDate, months: u32) -> Result<NaiveDate, ApiError> {
    anchor.checked_add_months(chrono::Months::new(months))
        .ok_or_else(|| ApiError::invalid("订阅账期超出日期范围"))
}

pub fn dashboard_usage(db: &Database, mut query: DashboardQuery) -> Result<Value, ApiError> {
    if let Some(prefix) = query.model_prefix.take() {
        if query.model.is_some() || query.models_by_app.is_some() {
            return Err(ApiError::invalid("modelPrefix 不能与 model 或 modelsByApp 同时使用"));
        }
        let shared = super::identity::share_settings(db).shared_app_types;
        let apps = parse_app_list(&shared, &query.app_type)?;
        let ids = prefix_models(db, &apps, query.devices.as_deref(), &prefix)?;
        query.model = Some(if ids.is_empty() { format!("__observatory_no_match_{}", uuid::Uuid::new_v4()) } else { ids.join(",") });
        let mut result = dashboard_usage(db, query)?;
        // Never expose the internal no-match sentinel as a real model selection.
        fn clean(value: &mut Value) {
            if let Some(obj) = value.as_object_mut() {
                if obj.get("model").and_then(Value::as_array).is_some_and(|v| v.iter().any(|v| v.as_str().is_some_and(|s| s.starts_with("__observatory_no_match_")))) { obj.insert("model".into(), json!([])); }
                for child in obj.values_mut() { clean(child); }
            } else if let Some(items) = value.as_array_mut() { for child in items { clean(child); } }
        }
        if ids.is_empty() { clean(&mut result); }
        result["modelPrefix"] = json!(prefix.trim());
        return Ok(result);
    }
    if !(-120..=0).contains(&query.billing_offset) {
        return Err(ApiError::invalid("billingOffset 必须在 -120 到 0 之间"));
    }
    if !(-52..=0).contains(&query.quota_offset) { return Err(ApiError::invalid("quotaOffset 必须在 -52 到 0 之间")); }
    let shared = super::identity::share_settings(db).shared_app_types;
    let apps = parse_app_list(&shared, &query.app_type)?;
    let selected_models = query.model.as_deref().map(parse_model_list).transpose()?;
    let models_by_app = parse_models_by_app(&apps, query.models_by_app.as_deref(), selected_models.as_ref(), true)?;
    let devices = parse_device_selection(db, query.devices.as_deref())?;
    let projection = if devices.is_local_only() { None } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else { Some(super::projection::database_for_peers_only(db, &devices.peers)?) };
    let source = projection.as_deref().unwrap_or(db);
    let sub = pick_subscription(db, &apps[0])?;
    let start_date = parse_date(&sub.start_date)?;
    let end_date = parse_date(&sub.end_date)?;
    let now = Local.timestamp_opt(Local::now().timestamp(), 0).single().unwrap();
    let today = now.date_naive();
    if today < start_date || !sub.enabled { return Err(ApiError::cycle_unavailable("订阅尚未生效或未启用")); }
    let effective_date = std::cmp::min(today, end_date.pred_opt().ok_or_else(|| ApiError::invalid("订阅日期无效"))?);
    let mut index: u32 = 0;
    let period_days = if sub.billing == "期" { sub.custom_interval_days.map(i64::from) } else { None };
    let boundary = |n: u32| -> Result<NaiveDate, ApiError> {
        if let Some(days) = period_days { return start_date.checked_add_signed(Duration::days(days * i64::from(n))).ok_or_else(|| ApiError::invalid("订阅账期超出范围")); }
        let factor = if sub.billing == "年" { 12 } else { 1 };
        anniversary(start_date, n.checked_mul(factor).ok_or_else(|| ApiError::invalid("订阅账期超出范围"))?)
    };
    while boundary(index + 1)? <= effective_date {
        index += 1;
        if index > 12000 { return Err(ApiError::invalid("订阅账期过多")); }
    }
    let selected = i64::from(index) + i64::from(query.billing_offset);
    if selected < 0 { return Err(ApiError::cycle_unavailable("没有更早的订阅账期")); }
    let start = local_midnight(boundary(selected as u32)?)?;
    let end = local_midnight(std::cmp::min(boundary(selected as u32 + 1)?, end_date))?;
    let data_end = std::cmp::min(end, now);
    if data_end <= start { return Err(ApiError::cycle_unavailable("账期没有可查询的数据")); }
    let summary = |a: DateTime<Local>, b: DateTime<Local>| -> Result<Value, ApiError> {
        summary_inner(db, SummaryQuery {
            app_type: Some(query.app_type.clone()), model: query.model.clone(),
            models_by_app: query.models_by_app.clone(), devices: query.devices.clone(),
            start_time: Some(to_rfc3339(a)), end_time: Some(to_rfc3339(b)), ..Default::default()
        }, true)
    };
    let total = summary(start, data_end)?;
    let mut cycles = Vec::new();
    // Enumerate backwards from the request time, then reverse into chronological order.
    for n in 0..5200 {
        let (a, b) = crate::subscriptions::weekly_cycle(&sub.rule, now, -n)
            .map_err(ApiError::invalid)?;
        if b <= start { break; }
        let from = std::cmp::max(a, start);
        let to = std::cmp::min(b, data_end);
        if from < to {
            cycles.push(json!({"cycleStart":to_rfc3339(a),"cycleEndExclusive":to_rfc3339(b),
                "countedStart":to_rfc3339(from),"countedEndExclusive":to_rfc3339(to),
                "partial":from != a || to != b,"usage":summary(from,to)?}));
        }
        if n == 5199 { return Err(ApiError::invalid("订阅额度周期过多")); }
    }
    cycles.reverse();
    let (week_start, week_end) = crate::subscriptions::weekly_cycle(&sub.rule, now, query.quota_offset).map_err(ApiError::invalid)?;
    let week_from = std::cmp::max(week_start, local_midnight(start_date)?);
    let week_to = std::cmp::min(std::cmp::min(week_end, local_midnight(end_date)?), now);
    let selected_week = if week_from < week_to { Some(summary(week_from, week_to)?) } else { None };
    let mut by_app_model = Vec::new();
    for app in &apps {
        let candidates = source.get_model_stats(Some(start.timestamp()), Some(data_end.timestamp() - 1), Some(app), None, None)?;
        for candidate in candidates {
            let model = candidate.model;
            if selected_models.as_ref().is_some_and(|v| !v.contains(&model)) ||
                models_by_app.as_ref().and_then(|v| v.get(app)).is_some_and(|v| !v.contains(&model)) { continue; }
            let usage = summary_inner(db, SummaryQuery { app_type:Some(app.clone()), model:Some(model.clone()),
                devices:query.devices.clone(), start_time:Some(to_rfc3339(start)), end_time:Some(to_rfc3339(data_end)), ..Default::default() }, true)?;
            by_app_model.push(json!({"appType":app,"model":model,"usage":usage}));
        }
    }
    by_app_model.sort_by(|a,b| b["usage"]["totalTokens"].as_u64().cmp(&a["usage"]["totalTokens"].as_u64()));
    Ok(json!({"scope":{"subscriptionApp":apps[0],"devices":query.devices,"timezone":"server-local"},
        "subscription":{"id":sub.id,"name":sub.name,"billing":sub.billing,
            "billingPeriod":{"offset":query.billing_offset,"start":to_rfc3339(start),"endExclusive":to_rfc3339(end),"dataEndExclusive":to_rfc3339(data_end)}},
        "billingTotal":total,"quotaCycles":cycles,"billingAggregations":{"byAppAndModel":by_app_model},
        "selectedQuotaCycle":{"offset":query.quota_offset,"cycleStart":to_rfc3339(week_start),"cycleEndExclusive":to_rfc3339(week_end),"usage":selected_week},
        "updatedAt":to_rfc3339(now)}))
}


/// Query models across shared platforms, using one platform's subscription.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DashboardModelsQuery {
    pub platform_code: String,
    #[serde(default)] pub model_prefix: Option<String>,
    #[serde(default)] pub model_ids: Vec<String>,
    #[serde(default)] pub billing_offset: i32,
    #[serde(default)] pub quota_offset: i32,
    #[serde(default)] pub devices: Option<String>,
}

pub fn dashboard_models(db: &Database, query: DashboardModelsQuery) -> Result<Value, ApiError> {
    let shared = super::identity::share_settings(db).shared_app_types;
    let platform = resolve_shared_app(&shared, &query.platform_code)?;
    if query.model_ids.len() > 50 { return Err(ApiError::invalid("modelIds 最多 50 项")); }
    let mut models = Vec::new();
    for id in &query.model_ids {
        let trimmed = id.trim();
        if trimmed.is_empty() || trimmed.len() > 256 || trimmed.contains(',') {
            return Err(ApiError::invalid("模型 ID 不能为空、过长或包含逗号"));
        }
        if !models.iter().any(|v| v == trimmed) { models.push(trimmed.to_string()); }
    }
    if query.model_prefix.is_some() && !models.is_empty() { return Err(ApiError::invalid("modelPrefix 不能与 modelIds 同时使用")); }
    let mut apps = vec![platform];
    if !models.is_empty() || query.model_prefix.is_some() {
        for app in shared { if !apps.contains(&app) { apps.push(app); } }
    }
    let mut value = dashboard_usage(db, DashboardQuery {
        app_type: apps.join(","),
        model_prefix: query.model_prefix,
        billing_offset: query.billing_offset, quota_offset: query.quota_offset,
        model: if models.is_empty() { None } else { Some(models.join(",")) },
        models_by_app: None, devices: query.devices,
    })?;
    value["modelIds"] = json!(models);
    Ok(value)
}

/// Resolve prefixes against effective pricing models across the selected devices.
fn prefix_models(db: &Database, apps: &[String], device_filter: Option<&str>, prefix: &str) -> Result<Vec<String>, ApiError> {
    let prefix = prefix.trim();
    if prefix.is_empty() || prefix.len() > 256 { return Err(ApiError::invalid("modelPrefix 不能为空或超过 256 字节")); }
    let devices = parse_device_selection(db, device_filter)?;
    let projection = if devices.is_local_only() { None } else if devices.local {
        Some(super::projection::database_for_peers(db, &devices.peers)?)
    } else { Some(super::projection::database_for_peers_only(db, &devices.peers)?) };
    let source = projection.as_deref().unwrap_or(db);
    let needle = prefix.to_ascii_lowercase();
    let mut ids = Vec::new();
    for app in apps {
        for row in source.get_model_stats(None, None, Some(app), None, None)? {
            if row.model.to_ascii_lowercase().starts_with(&needle) && !ids.contains(&row.model) {
                if row.model.contains(',') { return Err(ApiError::invalid("匹配的模型 ID 包含逗号，暂不支持前缀查询")); }
                ids.push(row.model);
            }
        }
    }
    Ok(ids)
}
