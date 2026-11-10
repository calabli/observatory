//! The usage statistics service
//!
//! Offers the aggregation queries over the usage data

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::ModelPricing;
use crate::services::sql_helpers::{
    fresh_input_sql, real_total_tokens_sql, INPUT_TOKEN_SEMANTICS_FRESH,
    INPUT_TOKEN_SEMANTICS_LEGACY, INPUT_TOKEN_SEMANTICS_TOTAL,
};
use chrono::{Local, NaiveDate, TimeZone, Timelike};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::LazyLock;

/// The usage aggregate
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub total_requests: u64,
    pub total_cost: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
    pub success_rate: f32,
    /// input + output + cache_creation + cache_read — the total tokens
    /// actually processed by the model (including cache hits). Used as the
    /// headline "real consumption" number in the usage hero.
    pub real_total_tokens: u64,
    /// cache_read / (input + cache_creation + cache_read). Range 0.0–1.0.
    /// Reported as a fraction; multiply by 100 in UI for percentage display.
    pub cache_hit_rate: f64,
}

/// Per-app-type usage summary used by the dashboard breakdown rail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummaryByApp {
    pub app_type: String,
    pub summary: UsageSummary,
}

/// Helper: compute (real_total, hit_rate) from the four token counters.
/// All inputs must already be cache-normalized (i.e. input excludes cache).
fn derive_real_total_and_hit_rate(
    fresh_input: u64,
    output: u64,
    cache_creation: u64,
    cache_read: u64,
) -> (u64, f64) {
    let real_total = fresh_input + output + cache_creation + cache_read;
    let cacheable_input = fresh_input + cache_creation + cache_read;
    let hit_rate = if cacheable_input > 0 {
        cache_read as f64 / cacheable_input as f64
    } else {
        0.0
    };
    (real_total, hit_rate)
}

/// Turn one row of the aggregate query (request count, cost, the four kinds of tokens, the
/// success count) into a [`UsageSummary`].
/// The column order has to match the SELECT of `get_usage_summary` / `get_session_usage_summary`.
fn usage_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<UsageSummary> {
    let total_requests: i64 = row.get(0)?;
    let total_cost: f64 = row.get(1)?;
    let total_input_tokens: i64 = row.get(2)?;
    let total_output_tokens: i64 = row.get(3)?;
    let total_cache_creation_tokens: i64 = row.get(4)?;
    let total_cache_read_tokens: i64 = row.get(5)?;
    let success_count: i64 = row.get(6)?;

    let success_rate = if total_requests > 0 {
        (success_count as f32 / total_requests as f32) * 100.0
    } else {
        0.0
    };

    let (real_total_tokens, cache_hit_rate) = derive_real_total_and_hit_rate(
        total_input_tokens as u64,
        total_output_tokens as u64,
        total_cache_creation_tokens as u64,
        total_cache_read_tokens as u64,
    );

    Ok(UsageSummary {
        total_requests: total_requests as u64,
        total_cost: format!("{total_cost:.6}"),
        total_input_tokens: total_input_tokens as u64,
        total_output_tokens: total_output_tokens as u64,
        total_cache_creation_tokens: total_cache_creation_tokens as u64,
        total_cache_read_tokens: total_cache_read_tokens as u64,
        success_rate,
        real_total_tokens,
        cache_hit_rate,
    })
}

/// The daily statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyStats {
    pub date: String,
    pub request_count: u64,
    pub total_cost: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
}

/// The provider statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderStats {
    pub provider_id: String,
    pub app_type: String,
    pub provider_name: String,
    pub request_count: u64,
    /// The real consumed tokens (fresh input + output + cache writes + cache hits), the same definition as the metric cards.
    pub total_tokens: u64,
    pub total_cost: String,
    pub success_rate: f32,
    pub avg_latency_ms: u64,
    /// The numerator of the speed: the sum of the output tokens of the eligible detail requests
    /// (there is a first token, the output is >= 100 tokens and the duration is > the first token).
    /// The aggregate speed = speed_output_tokens / (speed_generation_ms / 1000), not the average of the single rows.
    /// The daily rollups have no per-row timing and do not count.
    pub speed_output_tokens: u64,
    /// The denominator of the speed: the sum of (latency_ms - first_token_ms) of the same requests, in milliseconds.
    pub speed_generation_ms: u64,
    /// The numerator of the estimated speed: the sum of the output of the requests of the session
    /// log import that have an estimated duration and an output >= 200 tokens.
    /// It is accumulated separately from the group above: the estimated duration includes the wait
    /// for the first token, the semantics differ and the two must not be added together.
    pub est_speed_output_tokens: u64,
    /// The denominator of the estimated speed: the sum of the latency_ms of the same requests, in milliseconds.
    pub est_speed_duration_ms: u64,
    /// The median of the time to the first token (in milliseconds): the successful streaming detail requests that went through the router service; None when there is no such request.
    pub first_token_median_ms: Option<u64>,
    /// The median of the estimated time to the first token (in milliseconds): the detail requests of
    /// the session log import whose first token was derived from the timestamps.
    /// It is kept apart from the one above: the semantics differ and the UI only shows it when
    /// there is no exact value, with a ≈.
    pub est_first_token_median_ms: Option<u64>,
}

/// The threshold for counting the speed: a request whose output is below this number (a tool call for example) does not count as speed, otherwise 15 tokens in 0.1 seconds produce an absurd number.
pub const SPEED_MIN_OUTPUT_TOKENS: i64 = 100;

/// A generation window (duration - first token) shorter than this many milliseconds does not
/// count as speed: a relay that buffers and then dumps everything at once, or a short reply
/// that lands in a single network packet, measures a transmission burst and not the generation speed.
pub const SPEED_MIN_GENERATION_MS: i64 = 100;

/// The SQL condition that decides whether a detail row counts towards the speed (the same
/// definition as the frontend `isSpeedEligible`). The rows of the session log import
/// can also have a first token (derived from the timestamps), so they are separated by source
/// first and it does not only depend on whether there is a first token.
fn speed_eligible_sql(alias: &str) -> String {
    let data_source = data_source_expr(alias);
    format!(
        "{data_source} = 'proxy' AND {alias}.first_token_ms IS NOT NULL \
         AND {alias}.output_tokens >= {SPEED_MIN_OUTPUT_TOKENS} \
         AND {alias}.latency_ms - {alias}.first_token_ms >= {SPEED_MIN_GENERATION_MS}"
    )
}

/// The output threshold of the estimated speed: the estimated duration includes the wait for
/// the first token, the less output there is the bigger the share of the first token and the lower
/// the result, so the threshold is higher than the one of the exact definition. In the
/// measurements a request of 200-300 tokens is about a quarter lower than a long request and
/// one of 100-200 about 40% lower; and only a quarter of the Codex requests exceed 500, so a
/// much higher threshold would leave that half of the rows empty.
pub const SPEED_ESTIMATE_MIN_OUTPUT_TOKENS: i64 = 200;

/// A duration shorter than this many milliseconds is not estimated as a speed: an output of more than 200 tokens in less than a second most likely means the start was taken too late.
pub const SPEED_ESTIMATE_MIN_DURATION_MS: i64 = 1000;

/// The SQL condition that decides whether a detail row counts towards the estimated speed (the
/// same definition as the frontend `isSpeedEstimateEligible`):
/// the duration of a row of the session log import was estimated from the log timestamps while
/// it was imported. The estimated speed is still computed over the whole duration (including
/// the wait for the first token) and the derived first token is not subtracted: only some
/// requests have a derivable first token and subtracting it would mix the two algorithms in
/// the same row.
fn speed_estimate_eligible_sql(alias: &str) -> String {
    let data_source = data_source_expr(alias);
    format!(
        "{data_source} <> 'proxy' \
         AND {alias}.output_tokens >= {SPEED_ESTIMATE_MIN_OUTPUT_TOKENS} \
         AND {alias}.latency_ms >= {SPEED_ESTIMATE_MIN_DURATION_MS}"
    )
}

/// The SQL condition that decides whether a detail row counts towards the time to the first token: it went through the router service, it succeeded and it has a first token timing (a streaming request).
fn first_token_eligible_sql(alias: &str) -> String {
    let data_source = data_source_expr(alias);
    format!(
        "{data_source} = 'proxy' AND {alias}.first_token_ms IS NOT NULL \
         AND {alias}.status_code >= 200 AND {alias}.status_code < 300"
    )
}

/// The SQL condition of the estimated first token: the rows of the session log import whose
/// first token was derived from the timestamps while they were imported.
/// The median is taken apart from the exact first token of the router: the estimated start is
/// the moment the previous log line was written to disk, which includes the time the client
/// needed to assemble the request and is systematically too high.
fn est_first_token_eligible_sql(alias: &str) -> String {
    let data_source = data_source_expr(alias);
    format!("{data_source} <> 'proxy' AND {alias}.first_token_ms IS NOT NULL")
}

/// Take the median of the time to the first token (in milliseconds) per group key. The time to
/// the first token has a clear long tail, an occasional queue is a dozen seconds and
/// the average would be pulled up, so the median is used; SQLite has no median function, the
/// sorted values are fetched and the median is computed here.
/// `key_sql` is the group key expression, `from_sql` is the `FROM ... JOIN ...`, `detail_where` is
/// the `WHERE ...` of the detail part of the statistics query
/// (it may be empty) and `params` are its parameters.
fn first_token_medians(
    conn: &Connection,
    key_sql: &str,
    from_sql: &str,
    detail_where: &str,
    eligible_sql: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<HashMap<String, u64>, AppError> {
    let connector = if detail_where.is_empty() {
        "WHERE"
    } else {
        "AND"
    };
    let sql = format!(
        "SELECT {key_sql} AS k, l.first_token_ms {from_sql} {detail_where} {connector} {eligible_sql}
         ORDER BY k, l.first_token_ms"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params, |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?.max(0) as u64,
        ))
    })?;

    let mut grouped: HashMap<String, Vec<u64>> = HashMap::new();
    for row in rows {
        let (key, ms) = row?;
        grouped.entry(key).or_default().push(ms);
    }
    Ok(grouped
        .into_iter()
        .map(|(key, values)| {
            // Already fetched in ascending order by value
            let mid = values.len() / 2;
            let median = if values.len() % 2 == 0 {
                (values[mid - 1] + values[mid]) / 2
            } else {
                values[mid]
            };
            (key, median)
        })
        .collect())
}

/// The model statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStats {
    pub model: String,
    pub request_count: u64,
    /// The real consumed tokens (fresh input + output + cache writes + cache hits), the same definition as the metric cards.
    pub total_tokens: u64,
    pub total_cost: String,
    pub avg_cost_per_request: String,
    pub success_rate: f32,
    /// The numerator and the denominator of the speed, the same definition as the four fields of [`ProviderStats::speed_output_tokens`].
    pub speed_output_tokens: u64,
    pub speed_generation_ms: u64,
    pub est_speed_output_tokens: u64,
    pub est_speed_duration_ms: u64,
    /// The median of the time to the first token, the same definition as the two fields of [`ProviderStats`].
    pub first_token_median_ms: Option<u64>,
    pub est_first_token_median_ms: Option<u64>,
}

/// The filter of the request logs
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFilters {
    pub app_type: Option<String>,
    pub provider_name: Option<String>,
    pub model: Option<String>,
    pub status_code: Option<u16>,
    pub start_date: Option<i64>,
    pub end_date: Option<i64>,
}

/// The paginated response of the request logs
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedLogs {
    pub data: Vec<RequestLogDetail>,
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
}

/// The detail of a request log
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogDetail {
    pub request_id: String,
    pub provider_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_name: Option<String>,
    pub app_type: String,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_model: Option<String>,
    pub cost_multiplier: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    /// Internal storage semantics; omitted from the UI/API payload.
    #[serde(skip)]
    pub input_token_semantics: i64,
    pub input_cost_usd: String,
    pub output_cost_usd: String,
    pub cache_read_cost_usd: String,
    pub cache_creation_cost_usd: String,
    pub total_cost_usd: String,
    pub is_streaming: bool,
    pub latency_ms: u64,
    pub first_token_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub status_code: u16,
    pub error_message: Option<String>,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    /// The model name that was actually used for the pricing at write time. None = the historical rows before v11, "" = an error row that was not priced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing_model: Option<String>,
}

/// Map the result of the 26 column query to a `RequestLogDetail`.
///
/// The SELECT of the caller **must** return the 26 columns in this order:
/// `request_id, provider_id, provider_name, app_type, model, request_model,
///  cost_multiplier, input_tokens, output_tokens, cache_read_tokens,
///  cache_creation_tokens, input_cost_usd, output_cost_usd, cache_read_cost_usd,
///  cache_creation_cost_usd, total_cost_usd, is_streaming, latency_ms,
///  first_token_ms, duration_ms, status_code, error_message, created_at,
///  data_source, pricing_model, input_token_semantics`
///
/// When the provider_name is not needed (for example the backfill) it is enough to select
/// `NULL AS provider_name` as a placeholder.
fn row_to_request_log_detail(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestLogDetail> {
    Ok(RequestLogDetail {
        request_id: row.get(0)?,
        provider_id: row.get(1)?,
        provider_name: row.get(2)?,
        app_type: row.get(3)?,
        model: row.get(4)?,
        request_model: row.get(5)?,
        cost_multiplier: row
            .get::<_, Option<String>>(6)?
            .unwrap_or_else(|| "1".to_string()),
        input_tokens: row.get::<_, i64>(7)? as u32,
        output_tokens: row.get::<_, i64>(8)? as u32,
        cache_read_tokens: row.get::<_, i64>(9)? as u32,
        cache_creation_tokens: row.get::<_, i64>(10)? as u32,
        input_cost_usd: row.get(11)?,
        output_cost_usd: row.get(12)?,
        cache_read_cost_usd: row.get(13)?,
        cache_creation_cost_usd: row.get(14)?,
        total_cost_usd: row.get(15)?,
        is_streaming: row.get::<_, i64>(16)? != 0,
        latency_ms: row.get::<_, i64>(17)? as u64,
        first_token_ms: row.get::<_, Option<i64>>(18)?.map(|v| v as u64),
        duration_ms: row.get::<_, Option<i64>>(19)?.map(|v| v as u64),
        status_code: row.get::<_, i64>(20)? as u16,
        error_message: row.get(21)?,
        created_at: row.get(22)?,
        data_source: row.get(23)?,
        pricing_model: row.get(24)?,
        input_token_semantics: row.get::<_, i64>(25)?,
    })
}

/// SQL fragment: resolve provider_name with fallback for session-based entries.
/// Session logs use placeholder provider_ids (e.g., `_session`, `_<app>_session`)
/// that don't exist in the providers table — the CASE expression below is the
/// authoritative mapping from placeholder to readable name.
pub(crate) fn provider_name_coalesce(log_alias: &str, provider_alias: &str) -> String {
    format!(
        "COALESCE({provider_alias}.name, CASE {log_alias}.provider_id \
         WHEN '_session' THEN 'Claude (Session)' \
         WHEN '_codex_session' THEN 'Codex (Session)' \
         WHEN '_gemini_session' THEN 'Gemini (Session)' \
         WHEN '_opencode_session' THEN 'OpenCode (Session)' \
         WHEN '_grok_session' THEN 'Grok Build (Session)' \
         WHEN '_agy_session' THEN 'Antigravity (Session)' \
         WHEN '_kimi_session' THEN 'Kimi Code (Session)' \
         WHEN '_zcode_session' THEN 'ZCode (Session)' \
         WHEN '_dsh_session' THEN 'DeepSeekHarness (Session)' \
         WHEN '_mcode_session' THEN 'MiniMax Code (Session)' \
         WHEN '_pi_session' THEN 'Pi (Session)' \
         ELSE {log_alias}.provider_id END)"
    )
}

pub(crate) const SESSION_PROXY_DEDUP_WINDOW_SECONDS: i64 = 10 * 60;

/// The SQL fragment: wraps the `data_source` of the given alias in a COALESCE and treats NULL
/// as 'proxy'.
///
/// It defends against the NULL data_source rows that could be written before schema v9 (see
/// `tests::create_legacy_nullable_logs_table`). Every query that uses data_source
/// should build its fragment with this helper so that nothing is missed.
fn data_source_expr(log_alias: &str) -> String {
    format!("COALESCE({log_alias}.data_source, 'proxy')")
}

fn dedup_app_type_match_sql(left: &str, right: &str) -> String {
    format!(
        "{left} IN ({right}, CASE WHEN {right} = 'claude' THEN 'claude-desktop' ELSE {right} END)"
    )
}

/// The SQL scalar expression: folds the `claude-desktop` app_type of the Claude Desktop
/// gateway into `claude` for the "display semantics",
/// every other app_type is returned unchanged.
///
/// The background: the traffic of the Desktop gateway is written as `app_type='claude-desktop'`
/// at the accounting layer to keep the precision of the billing audit of a router takeover (do
/// not revert that). But the Dashboard presents it as
/// Claude Code -- it is essentially an embedded Claude Code runtime that runs inside the
/// Desktop shell, and the usage of the Desktop chat never goes through this software, so a
/// separate bucket would only make the user think it is "the whole usage of the desktop
/// version".
///
/// The usage: wrap any `app_type` column that takes part in a "filter / group by app" in this
/// expression, so that
/// a `= 'claude'` filter also hits `claude-desktop` and a `GROUP BY` merges the two,
/// without changing a single stored row (the detail panel still reads the original `app_type`).
///
/// Note: the index on that column is not used for this comparison anymore, but these are all
/// aggregation scans that carry a time filter anyway and the app_type is not the main access
/// path, so that is acceptable. It is only used on the read side; the cross-source
/// deduplication uses the narrower [`dedup_app_type_match_sql`] and the quota check
/// (`check_provider_limits`) keeps the original exact comparison.
fn folded_app_type_sql(column: &str) -> String {
    format!("CASE WHEN {column} = 'claude-desktop' THEN 'claude' ELSE {column} END")
}

fn app_type_filter_sql(column: &str, exact: bool) -> String {
    if exact {
        column.to_owned()
    } else {
        folded_app_type_sql(column)
    }
}

/// The SQL fragment: LEFT JOINs the log / rollup rows to the providers table to get the
/// provider name.
/// `proxy_request_logs` and `usage_daily_rollups` have the same (provider_id, app_type)
/// shape, so both of them can be the `log_alias`. The primary key of providers is (id, app_type),
/// the join is at most 1:1 and does not multiply the rows.
fn providers_join(log_alias: &str, provider_alias: &str) -> String {
    format!(
        "LEFT JOIN providers {provider_alias} \
         ON {log_alias}.provider_id = {provider_alias}.id \
         AND {log_alias}.app_type = {provider_alias}.app_type"
    )
}

/// The SQL scalar expression: the "effective pricing model" of a row -- pricing_model wins
/// when it is not empty and it falls back to
/// model for NULL/''. It is the group key of `get_model_stats` and the matching semantics of the
/// model filter of the Dashboard:
/// the filter value comes from the model statistics list and both sides have to use the same
/// expression for it to be selectable.
fn effective_model_sql(alias: &str) -> String {
    format!("COALESCE(NULLIF({alias}.pricing_model, ''), {alias}.model)")
}

/// Append the provider / model filter of the top of the Dashboard to the query conditions.
///
/// A provider matches exactly by its display name (reusing [`provider_name_coalesce`], so the
/// readable name of a session placeholder row such as "Claude (Session)" can be selected
/// too); a model matches via [`effective_model_sql`].
/// Note: when `provider_name` is passed in, the caller has to join [`providers_join`] into
/// the FROM,
/// otherwise `{provider_alias}.name` cannot be resolved.
fn push_provider_model_filters(
    conditions: &mut Vec<String>,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    log_alias: &str,
    provider_alias: &str,
    provider_name: Option<&str>,
    model: Option<&str>,
) {
    if let Some(name) = provider_name {
        conditions.push(format!(
            "{} = ?",
            provider_name_coalesce(log_alias, provider_alias)
        ));
        params.push(Box::new(name.to_string()));
    }
    if let Some(m) = model {
        conditions.push(format!("{} = ?", effective_model_sql(log_alias)));
        params.push(Box::new(m.to_string()));
    }
}

/// The projection reserves `lan@<hex device id>|` as an origin namespace.
/// Native IDs keep the empty origin; copies from different devices cannot deduplicate each other.
fn same_usage_origin_sql(left: &str, right: &str) -> String {
    let origin = |alias: &str| {
        format!("CASE WHEN substr({alias}.request_id, 1, 4) = 'lan@' THEN substr({alias}.request_id, 1, instr({alias}.request_id, '|')) ELSE '' END")
    };
    format!("({}) = ({})", origin(left), origin(right))
}

pub(crate) fn effective_usage_log_filter(log_alias: &str) -> String {
    let origin_match = same_usage_origin_sql("proxy_dedup", log_alias);
    let data_source = data_source_expr(log_alias);
    let proxy_data_source = data_source_expr("proxy_dedup");
    let app_type_match =
        dedup_app_type_match_sql("proxy_dedup.app_type", &format!("{log_alias}.app_type"));
    format!(
        "NOT (
            {data_source} IN ('session_log', 'codex_session', 'gemini_session', 'opencode_session')
            AND EXISTS (
                SELECT 1
                FROM proxy_request_logs proxy_dedup
                WHERE {proxy_data_source} = 'proxy'
                  AND {origin_match}
                  AND {app_type_match}
                  AND proxy_dedup.status_code >= 200
                  AND proxy_dedup.status_code < 300
                  AND proxy_dedup.input_tokens = {log_alias}.input_tokens
                  AND proxy_dedup.output_tokens = {log_alias}.output_tokens
                  AND proxy_dedup.cache_read_tokens = {log_alias}.cache_read_tokens
                  AND (
                      proxy_dedup.cache_creation_tokens = {log_alias}.cache_creation_tokens
                      OR (
                          {log_alias}.cache_creation_tokens = 0
                          AND {data_source} IN ('codex_session', 'gemini_session', 'opencode_session')
                      )
                      OR (
                          proxy_dedup.cache_creation_tokens = 0
                          AND proxy_dedup.input_token_semantics = {INPUT_TOKEN_SEMANTICS_LEGACY}
                          AND {data_source} = 'codex_session'
                      )
                  )
                  AND proxy_dedup.created_at BETWEEN
                      {log_alias}.created_at - {SESSION_PROXY_DEDUP_WINDOW_SECONDS}
                      AND {log_alias}.created_at + {SESSION_PROXY_DEDUP_WINDOW_SECONDS}
                  AND (
                      LOWER(proxy_dedup.model) = LOWER({log_alias}.model)
                      OR LOWER(proxy_dedup.model) = 'unknown'
                      OR LOWER({log_alias}.model) = 'unknown'
                  )
            )
        )"
    )
}

/// The session log sources that take part in the cross-source deduplication (the same definition as [`effective_usage_log_filter`]).
const DEDUP_SESSION_SOURCES_SQL: &str =
    "'session_log', 'codex_session', 'gemini_session', 'opencode_session'";

/// The deduplication condition of the Dashboard read path: the semantics are exactly the same
/// as [`effective_usage_log_filter`],
/// it only first looks at how many rows of each kind there are in the time window and then
/// picks the cheaper formulation.
///
/// - There is no successful proxy log in the window or no session log at all: there cannot be
///   a duplicate, no condition is added.
/// - The proxy logs are fewer: the duplicated session rows are looked up starting from the proxy
///   logs (a one-off subquery),
///   the main query only does a `rowid NOT IN` and no longer joins row by row. It is much
///   faster when the session logs are the vast majority.
/// - The session logs are fewer: the original row by row `EXISTS` is kept.
///
/// The time boundary is widened by [`SESSION_PROXY_DEDUP_WINDOW_SECONDS`], a proxy row outside
/// the window cannot match a session row inside it;
/// the boundary is an integer and is concatenated into the SQL directly.
pub(crate) fn effective_usage_log_filter_for_range(
    conn: &Connection,
    log_alias: &str,
    start_date: Option<i64>,
    end_date: Option<i64>,
) -> Result<String, AppError> {
    let lo = start_date
        .map(|v| v.saturating_sub(SESSION_PROXY_DEDUP_WINDOW_SECONDS))
        .unwrap_or(i64::MIN / 2);
    let hi = end_date
        .map(|v| v.saturating_add(SESSION_PROXY_DEDUP_WINDOW_SECONDS))
        .unwrap_or(i64::MAX / 2);
    let data_source = data_source_expr("c");
    let (proxy_count, session_count): (i64, i64) = conn.query_row(
        &format!(
            "SELECT
                COALESCE(SUM(CASE WHEN {data_source} = 'proxy'
                    AND c.status_code >= 200 AND c.status_code < 300 THEN 1 ELSE 0 END), 0),
                COALESCE(SUM(CASE WHEN {data_source} IN ({DEDUP_SESSION_SOURCES_SQL})
                    THEN 1 ELSE 0 END), 0)
             FROM proxy_request_logs c
             WHERE c.created_at BETWEEN ?1 AND ?2"
        ),
        rusqlite::params![lo, hi],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    if proxy_count == 0 || session_count == 0 {
        return Ok("1 = 1".to_string());
    }
    if proxy_count > session_count {
        return Ok(effective_usage_log_filter(log_alias));
    }

    let origin_match = same_usage_origin_sql("dedup_p", "dedup_s");
    let dp_source = data_source_expr("dedup_p");
    let ds_source = data_source_expr("dedup_s");
    Ok(format!(
        "{log_alias}.rowid NOT IN (
            SELECT dedup_s.rowid
            FROM proxy_request_logs dedup_p
            JOIN proxy_request_logs dedup_s
              ON dedup_s.app_type IN (
                     dedup_p.app_type,
                     CASE WHEN dedup_p.app_type = 'claude-desktop' THEN 'claude' ELSE dedup_p.app_type END
                 )
             AND {origin_match}
             AND {ds_source} IN ({DEDUP_SESSION_SOURCES_SQL})
             AND dedup_s.input_tokens = dedup_p.input_tokens
             AND dedup_s.output_tokens = dedup_p.output_tokens
             AND dedup_s.cache_read_tokens = dedup_p.cache_read_tokens
             AND dedup_s.created_at BETWEEN
                 dedup_p.created_at - {SESSION_PROXY_DEDUP_WINDOW_SECONDS}
                 AND dedup_p.created_at + {SESSION_PROXY_DEDUP_WINDOW_SECONDS}
             AND (
                 dedup_p.cache_creation_tokens = dedup_s.cache_creation_tokens
                 OR (
                     dedup_s.cache_creation_tokens = 0
                     AND {ds_source} IN ('codex_session', 'gemini_session', 'opencode_session')
                 )
                 OR (
                     dedup_p.cache_creation_tokens = 0
                     AND dedup_p.input_token_semantics = {INPUT_TOKEN_SEMANTICS_LEGACY}
                     AND {ds_source} = 'codex_session'
                 )
             )
             AND (
                 LOWER(dedup_p.model) = LOWER(dedup_s.model)
                 OR LOWER(dedup_p.model) = 'unknown'
                 OR LOWER(dedup_s.model) = 'unknown'
             )
            WHERE {dp_source} = 'proxy'
              AND dedup_p.status_code >= 200
              AND dedup_p.status_code < 300
              AND dedup_p.created_at BETWEEN {lo} AND {hi}
        )"
    ))
}

/// The cache of the total number of request logs: a `COUNT(*)` over the detail rows on every
/// refresh is the most expensive part for a wide range.
/// The total of the last time is reused when the filter conditions and the start are the same
/// and there was no write on the connection (the write epoch did not change, an insert, an
/// update and a delete all count);
/// when the end time is pushed forward (an activity window such as "today" changes on every
/// refresh) it is verified that the new time range holds no rows. Every filter keeps its own
/// entry (a small LRU with the capacity
/// [`LOG_COUNT_CACHE_CAPACITY`], the main window and the tray do not evict each other) and
/// [`LOG_COUNT_CACHE_TTL`] is the fallback.
#[derive(Default)]
pub(crate) struct LogCountCache {
    entries: Vec<LogCountEntry>,
}

#[derive(Clone)]
struct LogCountEntry {
    key: String,
    end_date: Option<i64>,
    epoch: u64,
    computed_at: std::time::Instant,
    total: u32,
}

const LOG_COUNT_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
const LOG_COUNT_CACHE_CAPACITY: usize = 4;

fn log_count_cache_key(filters: &LogFilters) -> String {
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}",
        filters.app_type,
        filters.provider_name,
        filters.model,
        filters.status_code,
        filters.start_date
    )
}

/// Return the cached total when the cache can be reused; a hit moves the entry to the end of the LRU.
fn cached_log_count(
    cache: &mut LogCountCache,
    epoch: u64,
    conn: &Connection,
    filters: &LogFilters,
) -> Result<Option<u32>, AppError> {
    let key = log_count_cache_key(filters);
    let Some(index) = cache.entries.iter().rposition(|entry| entry.key == key) else {
        return Ok(None);
    };
    let entry = cache.entries.remove(index);
    cache.entries.push(entry.clone());
    if entry.epoch != epoch || entry.computed_at.elapsed() > LOG_COUNT_CACHE_TTL {
        return Ok(None);
    }
    match (entry.end_date, filters.end_date) {
        (cached, current) if cached == current => Ok(Some(entry.total)),
        (Some(cached), Some(current)) if current > cached => {
            // The end time was pushed forward: the new range holds no rows, so the total does not change
            let has_new_rows: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM proxy_request_logs
                  WHERE created_at > ?1 AND created_at <= ?2)",
                params![cached, current],
                |row| row.get(0),
            )?;
            Ok((!has_new_rows).then_some(entry.total))
        }
        _ => Ok(None),
    }
}

/// Write / update one cache entry and evict the oldest one when the capacity is exceeded.
fn store_log_count(cache: &mut LogCountCache, filters: &LogFilters, epoch: u64, total: u32) {
    let key = log_count_cache_key(filters);
    cache.entries.retain(|entry| entry.key != key);
    if cache.entries.len() >= LOG_COUNT_CACHE_CAPACITY {
        cache.entries.remove(0);
    }
    cache.entries.push(LogCountEntry {
        key,
        end_date: filters.end_date,
        epoch,
        computed_at: std::time::Instant::now(),
        total,
    });
}

/// The fingerprint key of the cross-source deduplication.
///
/// `cache_creation_tokens`: the Gemini/OpenCode session logs and the old Codex rollouts do not
/// expose that field, the caller passes 0 to mean "unknown" and the matcher lets any
/// cache_creation_tokens value of the proxy side pass.
/// The other way round, the agents before v3.17.0 do not record the Codex cache writes, so a
/// Codex proxy row with the legacy semantics and a write amount of 0
/// also lets any session write amount pass.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DedupKey<'a> {
    pub app_type: &'a str,
    pub model: &'a str,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    pub created_at: i64,
}

/// The unified deduplication decision before a session log is written.
///
/// The insert is skipped when one of these conditions hits: ① the `request_id` already exists;
// ② there is a proxy log in the time window that matches the `key` (the fingerprint
// deduplication).
pub(crate) fn should_skip_session_insert(
    conn: &Connection,
    request_id: &str,
    key: &DedupKey,
) -> Result<bool, AppError> {
    if proxy_request_id_exists(conn, request_id)? {
        return Ok(true);
    }
    has_matching_proxy_usage_log(conn, key)
}

fn proxy_request_id_exists(conn: &Connection, request_id: &str) -> Result<bool, AppError> {
    conn.prepare_cached("SELECT EXISTS(SELECT 1 FROM proxy_request_logs WHERE request_id = ?1)")
        .and_then(|mut stmt| stmt.query_row(params![request_id], |row| row.get::<_, bool>(0)))
        .map_err(|e| AppError::Database(format!("查询 request_id 失败: {e}")))
}

// The session re-import runs this query once per token event; the static SQL text makes the
// prepare_cached hit reliably and also saves the format! allocation of every row.
static MATCHING_PROXY_USAGE_LOG_SQL: LazyLock<String> = LazyLock::new(|| {
    let l_data_source = data_source_expr("l");
    let app_type_match = dedup_app_type_match_sql("l.app_type", "?1");
    format!(
        "SELECT EXISTS (
            SELECT 1
            FROM proxy_request_logs l
            WHERE {l_data_source} = 'proxy'
              AND {app_type_match}
              AND l.status_code >= 200
              AND l.status_code < 300
              AND l.input_tokens = ?3
              AND l.output_tokens = ?4
              AND l.cache_read_tokens = ?5
              AND (
                  l.cache_creation_tokens = ?6
                  OR ?9 = 1
                  OR (
                      ?10 = 1
                      AND l.cache_creation_tokens = 0
                      AND l.input_token_semantics = {INPUT_TOKEN_SEMANTICS_LEGACY}
                  )
              )
              AND l.created_at BETWEEN ?7 - ?8 AND ?7 + ?8
              AND (
                  LOWER(l.model) = LOWER(?2)
                  OR LOWER(l.model) = 'unknown'
                  OR LOWER(?2) = 'unknown'
              )
        )"
    )
});

pub(crate) fn has_matching_proxy_usage_log(
    conn: &Connection,
    key: &DedupKey,
) -> Result<bool, AppError> {
    let allow_missing_cache_creation =
        matches!(key.app_type, "codex" | "gemini" | "opencode") && key.cache_creation_tokens == 0;
    let allow_legacy_proxy_cache_creation = key.app_type == "codex";

    conn.prepare_cached(&MATCHING_PROXY_USAGE_LOG_SQL)
        .and_then(|mut stmt| {
            stmt.query_row(
                params![
                    key.app_type,
                    key.model,
                    key.input_tokens as i64,
                    key.output_tokens as i64,
                    key.cache_read_tokens as i64,
                    key.cache_creation_tokens as i64,
                    key.created_at,
                    SESSION_PROXY_DEDUP_WINDOW_SECONDS,
                    allow_missing_cache_creation as i64,
                    allow_legacy_proxy_cache_creation as i64,
                ],
                |row| row.get::<_, bool>(0),
            )
        })
        .map_err(|e| AppError::Database(format!("查询重复代理用量日志失败: {e}")))
}

/// The takeover activity guard of the grokbuild session import: when any grokbuild proxy row
/// exists within the window around the given moment, the takeover state is assumed to be
/// active at that time and the session events are skipped as a whole -- the same request was
/// already accounted by the proxy per request and accounting it again on the session side
/// would double count.
///
/// The fingerprint matching of [`has_matching_proxy_usage_log`] is not reused: the Grok session
/// events are per-round aggregated values and are structurally not equal to the token values
/// of the per-request proxy rows, so the fingerprint would never hit.
/// It is designed as a "takeover state detection" and not as a "row matching", so the
// status_code is not filtered -- a failed
// proxy request proves just as well that the traffic goes through the proxy.
///
/// Known limitation (a deliberate trade-off, conservative in the direction of only losing
/// records and never double counting): the window has no session dimension, so any
/// grokbuild proxy row casts a shadow over all the session events within the ±window -- when
/// the takeover and the official mode alternate or run in parallel within ten minutes, the
/// rounds of the official mode are skipped (a lost record instead of a double count).
pub(crate) fn has_recent_grokbuild_proxy_activity(
    conn: &Connection,
    created_at: i64,
) -> Result<bool, AppError> {
    let l_data_source = data_source_expr("l");
    let sql = format!(
        "SELECT EXISTS (
            SELECT 1
            FROM proxy_request_logs l
            WHERE {l_data_source} = 'proxy'
              AND l.app_type = 'grokbuild'
              AND l.created_at BETWEEN ?1 - ?2 AND ?1 + ?2
        )"
    );
    conn.query_row(
        &sql,
        params![created_at, SESSION_PROXY_DEDUP_WINDOW_SECONDS],
        |row| row.get::<_, bool>(0),
    )
    .map_err(|e| AppError::Database(format!("查询 Grok 接管活动失败: {e}")))
}

static SUSPECTED_CODEX_DUPLICATE_SQL: LazyLock<String> = LazyLock::new(|| {
    let data_source = data_source_expr("l");
    format!(
        "SELECT EXISTS (
            SELECT 1
            FROM proxy_request_logs l
            WHERE l.app_type = 'codex'
              AND {data_source} = 'codex_session'
              AND l.request_id <> ?1
              AND LOWER(l.model) = LOWER(?2)
              AND l.input_tokens = ?3
              AND l.output_tokens = ?4
              AND l.cache_read_tokens = ?5
              AND l.created_at BETWEEN ?6 - ?7 AND ?6 + ?7
        )"
    )
});

pub(crate) fn has_suspected_codex_session_duplicate(
    conn: &Connection,
    request_id: &str,
    key: &DedupKey,
) -> Result<bool, AppError> {
    conn.prepare_cached(&SUSPECTED_CODEX_DUPLICATE_SQL)
        .and_then(|mut stmt| {
            stmt.query_row(
                params![
                    request_id,
                    key.model,
                    key.input_tokens as i64,
                    key.output_tokens as i64,
                    key.cache_read_tokens as i64,
                    key.created_at,
                    SESSION_PROXY_DEDUP_WINDOW_SECONDS,
                ],
                |row| row.get::<_, bool>(0),
            )
        })
        .map_err(|error| AppError::Database(format!("查询疑似重复 Codex 会话用量失败: {error}")))
}

#[derive(Debug, Clone, Default)]
struct RollupDateBounds {
    start: Option<String>,
    end: Option<String>,
    is_empty: bool,
}

fn local_datetime_from_timestamp(ts: i64) -> Result<chrono::DateTime<Local>, AppError> {
    Local
        .timestamp_opt(ts, 0)
        .single()
        .ok_or_else(|| AppError::Database(format!("无法解析本地时间戳: {ts}")))
}

fn compute_rollup_date_bounds(
    start_ts: Option<i64>,
    end_ts: Option<i64>,
) -> Result<RollupDateBounds, AppError> {
    let start = match start_ts {
        Some(ts) => {
            let local = local_datetime_from_timestamp(ts)?;
            let day = local.date_naive();
            if local.time().num_seconds_from_midnight() == 0 {
                Some(day.format("%Y-%m-%d").to_string())
            } else {
                day.succ_opt()
                    .map(|next| next.format("%Y-%m-%d").to_string())
            }
        }
        None => None,
    };

    let end = match end_ts {
        Some(ts) => {
            let local = local_datetime_from_timestamp(ts)?;
            let day = local.date_naive();
            if local.time().hour() == 23
                && local.time().minute() == 59
                && local.time().second() == 59
            {
                Some(day.format("%Y-%m-%d").to_string())
            } else {
                day.pred_opt()
                    .map(|prev| prev.format("%Y-%m-%d").to_string())
            }
        }
        None => None,
    };

    let is_empty = matches!((&start, &end), (Some(start), Some(end)) if start > end);

    Ok(RollupDateBounds {
        start,
        end,
        is_empty,
    })
}

fn push_rollup_date_filters(
    conditions: &mut Vec<String>,
    params: &mut Vec<Box<dyn rusqlite::ToSql>>,
    column: &str,
    bounds: &RollupDateBounds,
) {
    if bounds.is_empty {
        conditions.push("1 = 0".to_string());
        return;
    }

    if let Some(start) = &bounds.start {
        conditions.push(format!("{column} >= ?"));
        params.push(Box::new(start.clone()));
    }

    if let Some(end) = &bounds.end {
        conditions.push(format!("{column} <= ?"));
        params.push(Box::new(end.clone()));
    }
}

fn local_day_start_rfc3339(day: NaiveDate) -> String {
    let local_midnight = day
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| match Local.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => Some(dt),
            chrono::LocalResult::Ambiguous(earliest, _) => Some(earliest),
            chrono::LocalResult::None => None,
        })
        .unwrap_or_else(Local::now);

    local_midnight.to_rfc3339()
}

impl Database {
    /// Get the usage aggregate
    pub fn get_usage_summary(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<UsageSummary, AppError> {
        let conn = lock_conn!(self.conn);
        Self::usage_summary_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            false,
        )
    }
    pub fn get_usage_summary_exact_app(
        &self,
        start: Option<i64>,
        end: Option<i64>,
        app: &str,
    ) -> Result<UsageSummary, AppError> {
        let conn = lock_conn!(self.conn);
        Self::usage_summary_impl(&conn, start, end, Some(app), None, None, true)
    }
    pub(crate) fn usage_summary_impl(
        conn: &Connection,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
        exact_app: bool,
    ) -> Result<UsageSummary, AppError> {
        // Build detail WHERE clause
        let mut conditions = vec![effective_usage_log_filter_for_range(
            conn, "l", start_date, end_date,
        )?];
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(start) = start_date {
            conditions.push("l.created_at >= ?".to_string());
            params_vec.push(Box::new(start));
        }
        if let Some(end) = end_date {
            conditions.push("l.created_at <= ?".to_string());
            params_vec.push(Box::new(end));
        }
        if let Some(at) = app_type {
            conditions.push(format!(
                "{} = ?",
                app_type_filter_sql("l.app_type", exact_app)
            ));
            params_vec.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut conditions,
            &mut params_vec,
            "l",
            "p",
            provider_name,
            model,
        );

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let detail_join = if provider_name.is_some() {
            providers_join("l", "p")
        } else {
            String::new()
        };

        // Only include rolled-up rows for full local days that are fully covered by the range.
        let mut rollup_conditions: Vec<String> = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let rollup_bounds = compute_rollup_date_bounds(start_date, end_date)?;

        push_rollup_date_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r.date",
            &rollup_bounds,
        );
        if let Some(at) = app_type {
            rollup_conditions.push(format!(
                "{} = ?",
                app_type_filter_sql("r.app_type", exact_app)
            ));
            rollup_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );

        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };
        let rollup_join = if provider_name.is_some() {
            providers_join("r", "p2")
        } else {
            String::new()
        };

        let fresh_input_detail = fresh_input_sql("l");
        let fresh_input_rollup = fresh_input_sql("r");
        let sql = format!(
            "SELECT
                COALESCE(d.total_requests, 0) + COALESCE(r.total_requests, 0),
                COALESCE(d.total_cost, 0) + COALESCE(r.total_cost, 0),
                COALESCE(d.total_input_tokens, 0) + COALESCE(r.total_input_tokens, 0),
                COALESCE(d.total_output_tokens, 0) + COALESCE(r.total_output_tokens, 0),
                COALESCE(d.total_cache_creation_tokens, 0) + COALESCE(r.total_cache_creation_tokens, 0),
                COALESCE(d.total_cache_read_tokens, 0) + COALESCE(r.total_cache_read_tokens, 0),
                COALESCE(d.success_count, 0) + COALESCE(r.success_count, 0)
            FROM
                (SELECT
                    COUNT(*) as total_requests,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
                    COALESCE(SUM({fresh_input_detail}), 0) as total_input_tokens,
                    COALESCE(SUM(l.output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(l.cache_creation_tokens), 0) as total_cache_creation_tokens,
                    COALESCE(SUM(l.cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0) as success_count
                 FROM proxy_request_logs l {detail_join} {where_clause}) d,
                (SELECT
                    COALESCE(SUM(r.request_count), 0) as total_requests,
                    COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0) as total_cost,
                    COALESCE(SUM({fresh_input_rollup}), 0) as total_input_tokens,
                    COALESCE(SUM(r.output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(r.cache_creation_tokens), 0) as total_cache_creation_tokens,
                    COALESCE(SUM(r.cache_read_tokens), 0) as total_cache_read_tokens,
                    COALESCE(SUM(r.success_count), 0) as success_count
                 FROM usage_daily_rollups r {rollup_join} {rollup_where}) r"
        );

        // Combine params: detail params first, then rollup params
        let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = params_vec;
        all_params.extend(rollup_params);
        let param_refs: Vec<&dyn rusqlite::ToSql> = all_params.iter().map(|p| p.as_ref()).collect();

        let result = conn.query_row(&sql, param_refs.as_slice(), usage_summary_from_row)?;

        Ok(result)
    }

    /// The usage aggregate of a single session (the header of the session reader page): the
    /// definition of the total tokens and the cost is the same as in the Dashboard.
    ///
    /// Only the rows of the session log import are counted: they carry the session ID of the
    /// client itself and are imported whether they went through the router or not;
    /// the session ID of a proxy row is inferred by the proxy side and does not necessarily match,
    /// and counting both sides would count it twice.
    /// The detail records are aggregated into the per-day table and deleted after 30 days (the
    /// per-day table has no session dimension), so an older session cannot be found.
    pub fn get_session_usage_summary(
        &self,
        app_type: &str,
        session_id: &str,
    ) -> Result<UsageSummary, AppError> {
        let conn = lock_conn!(self.conn);
        let fresh_input = fresh_input_sql("l");
        let app_type_expr = folded_app_type_sql("l.app_type");
        let data_source = data_source_expr("l");
        let sql = format!(
            "SELECT
                COUNT(*),
                COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0),
                COALESCE(SUM({fresh_input}), 0),
                COALESCE(SUM(l.output_tokens), 0),
                COALESCE(SUM(l.cache_creation_tokens), 0),
                COALESCE(SUM(l.cache_read_tokens), 0),
                COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0)
             FROM proxy_request_logs l
             WHERE l.session_id = ?1 AND {app_type_expr} = ?2 AND {data_source} <> 'proxy'"
        );

        let result = conn.query_row(
            &sql,
            rusqlite::params![session_id, app_type],
            usage_summary_from_row,
        )?;

        Ok(result)
    }

    /// Whether the detail table holds at least one row. The EXISTS short circuit is for the
    /// callers that only care about the existence (checking for an empty database at the startup
    // for example),
    // it avoids running a full-table aggregate just to get the total_requests.
    pub fn has_any_request_logs(&self) -> Result<bool, AppError> {
        let conn = lock_conn!(self.conn);
        conn.query_row("SELECT EXISTS(SELECT 1 FROM proxy_request_logs)", [], |r| {
            r.get(0)
        })
        .map_err(|e| AppError::Database(format!("查询明细存在性失败: {e}")))
    }

    /// The usage aggregate split by app_type, for the per-app display bars of the Dashboard.
    /// Returns every app_type that has data, sorted by real_total_tokens descending.
    ///
    /// Single SQL with `GROUP BY app_type` — avoids the N+1 round-trip that
    /// would result from invoking `get_usage_summary` once per app_type.
    pub fn get_usage_summary_by_app(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<UsageSummaryByApp>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::usage_summary_by_app_on_conn(&conn, start_date, end_date, provider_name, model)
    }

    pub(crate) fn usage_summary_by_app_on_conn(
        conn: &Connection,
        start_date: Option<i64>,
        end_date: Option<i64>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<UsageSummaryByApp>, AppError> {
        let mut detail_conditions = vec![effective_usage_log_filter_for_range(
            conn, "l", start_date, end_date,
        )?];
        let mut detail_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(start) = start_date {
            detail_conditions.push("l.created_at >= ?".to_string());
            detail_params.push(Box::new(start));
        }
        if let Some(end) = end_date {
            detail_conditions.push("l.created_at <= ?".to_string());
            detail_params.push(Box::new(end));
        }
        push_provider_model_filters(
            &mut detail_conditions,
            &mut detail_params,
            "l",
            "p",
            provider_name,
            model,
        );
        let detail_where = format!("WHERE {}", detail_conditions.join(" AND "));
        let detail_join = if provider_name.is_some() {
            providers_join("l", "p")
        } else {
            String::new()
        };

        let rollup_bounds = compute_rollup_date_bounds(start_date, end_date)?;
        let mut rollup_conditions: Vec<String> = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        push_rollup_date_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r.date",
            &rollup_bounds,
        );
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );
        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };
        let rollup_join = if provider_name.is_some() {
            providers_join("r", "p2")
        } else {
            String::new()
        };

        let fresh_input_detail = fresh_input_sql("l");
        let fresh_input_rollup = fresh_input_sql("r");
        // Fold claude-desktop -> claude: the inner projection turns them into the same bucket name and the outer GROUP BY merges them naturally.
        let detail_app_type = folded_app_type_sql("l.app_type");
        let rollup_app_type = folded_app_type_sql("r.app_type");

        let sql = format!(
            "SELECT app_type,
                SUM(req_count) as req_count,
                SUM(cost) as cost,
                SUM(input_t) as input_t,
                SUM(output_t) as output_t,
                SUM(cache_create_t) as cache_create_t,
                SUM(cache_read_t) as cache_read_t,
                SUM(success_count) as success_count
            FROM (
                SELECT {detail_app_type} as app_type,
                    COUNT(*) as req_count,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as cost,
                    COALESCE(SUM({fresh_input_detail}), 0) as input_t,
                    COALESCE(SUM(l.output_tokens), 0) as output_t,
                    COALESCE(SUM(l.cache_creation_tokens), 0) as cache_create_t,
                    COALESCE(SUM(l.cache_read_tokens), 0) as cache_read_t,
                    COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0) as success_count
                FROM proxy_request_logs l {detail_join} {detail_where}
                GROUP BY l.app_type
                UNION ALL
                SELECT {rollup_app_type} as app_type,
                    COALESCE(SUM(r.request_count), 0),
                    COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0),
                    COALESCE(SUM({fresh_input_rollup}), 0),
                    COALESCE(SUM(r.output_tokens), 0),
                    COALESCE(SUM(r.cache_creation_tokens), 0),
                    COALESCE(SUM(r.cache_read_tokens), 0),
                    COALESCE(SUM(r.success_count), 0)
                FROM usage_daily_rollups r {rollup_join} {rollup_where}
                GROUP BY r.app_type
            )
            GROUP BY app_type"
        );

        let mut combined: Vec<Box<dyn rusqlite::ToSql>> = detail_params;
        combined.extend(rollup_params);
        let refs: Vec<&dyn rusqlite::ToSql> = combined.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(refs.as_slice(), |row| {
            let app_type: String = row.get(0)?;
            let total_requests: i64 = row.get(1)?;
            let total_cost: f64 = row.get(2)?;
            let total_input_tokens: i64 = row.get(3)?;
            let total_output_tokens: i64 = row.get(4)?;
            let total_cache_creation_tokens: i64 = row.get(5)?;
            let total_cache_read_tokens: i64 = row.get(6)?;
            let success_count: i64 = row.get(7)?;

            let success_rate = if total_requests > 0 {
                (success_count as f32 / total_requests as f32) * 100.0
            } else {
                0.0
            };
            let (real_total_tokens, cache_hit_rate) = derive_real_total_and_hit_rate(
                total_input_tokens as u64,
                total_output_tokens as u64,
                total_cache_creation_tokens as u64,
                total_cache_read_tokens as u64,
            );

            Ok(UsageSummaryByApp {
                app_type,
                summary: UsageSummary {
                    total_requests: total_requests as u64,
                    total_cost: format!("{total_cost:.6}"),
                    total_input_tokens: total_input_tokens as u64,
                    total_output_tokens: total_output_tokens as u64,
                    total_cache_creation_tokens: total_cache_creation_tokens as u64,
                    total_cache_read_tokens: total_cache_read_tokens as u64,
                    success_rate,
                    real_total_tokens,
                    cache_hit_rate,
                },
            })
        })?;

        let mut summaries = Vec::new();
        for row in rows {
            let item = row?;
            if item.summary.total_requests == 0 && item.summary.real_total_tokens == 0 {
                continue;
            }
            summaries.push(item);
        }
        summaries.sort_by(|a, b| {
            b.summary
                .real_total_tokens
                .cmp(&a.summary.real_total_tokens)
        });
        Ok(summaries)
    }

    /// Get the daily trend (a sliding window, <=24h by hour, >24h by day, the same window as the aggregate)
    pub fn get_daily_trends(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<DailyStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::daily_trends_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            false,
        )
    }
    pub fn get_daily_trends_exact_app(
        &self,
        start: Option<i64>,
        end: Option<i64>,
        app: &str,
    ) -> Result<Vec<DailyStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::daily_trends_impl(&conn, start, end, Some(app), None, None, true)
    }
    pub(crate) fn daily_trends_impl(
        conn: &Connection,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
        exact_app: bool,
    ) -> Result<Vec<DailyStats>, AppError> {
        let end_ts = end_date.unwrap_or_else(|| Local::now().timestamp());
        let mut start_ts = start_date.unwrap_or_else(|| end_ts - 24 * 60 * 60);

        if start_ts >= end_ts {
            start_ts = end_ts - 24 * 60 * 60;
        }

        let duration = end_ts - start_ts;
        if duration <= 24 * 60 * 60 {
            let bucket_seconds: i64 = 60 * 60;
            let mut bucket_count: i64 = if duration <= 0 {
                1
            } else {
                (duration + bucket_seconds - 1) / bucket_seconds
            };

            if bucket_count < 1 {
                bucket_count = 1;
            }

            let mut extra_conditions: Vec<String> = Vec::new();
            let mut extra_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(at) = app_type {
                extra_conditions.push(format!(
                    "{} = ?",
                    app_type_filter_sql("l.app_type", exact_app)
                ));
                extra_params.push(Box::new(at.to_string()));
            }
            push_provider_model_filters(
                &mut extra_conditions,
                &mut extra_params,
                "l",
                "p",
                provider_name,
                model,
            );
            let extra_filter = extra_conditions
                .iter()
                .map(|c| format!("AND {c}"))
                .collect::<Vec<_>>()
                .join(" ");
            let detail_join = if provider_name.is_some() {
                providers_join("l", "p")
            } else {
                String::new()
            };

            let effective_filter =
                effective_usage_log_filter_for_range(conn, "l", Some(start_ts), Some(end_ts))?;
            let fresh_input = fresh_input_sql("l");
            let sql = format!(
                "SELECT
                    CAST((l.created_at - ?1) / ?3 AS INTEGER) as bucket_idx,
                    COUNT(*) as request_count,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
                    COALESCE(SUM({fresh_input} + l.output_tokens), 0) as total_tokens,
                    COALESCE(SUM({fresh_input}), 0) as total_input_tokens,
                    COALESCE(SUM(l.output_tokens), 0) as total_output_tokens,
                    COALESCE(SUM(l.cache_creation_tokens), 0) as total_cache_creation_tokens,
                    COALESCE(SUM(l.cache_read_tokens), 0) as total_cache_read_tokens
                FROM proxy_request_logs l {detail_join}
                WHERE l.created_at >= ?1 AND l.created_at <= ?2
                  AND {effective_filter} {extra_filter}
                GROUP BY bucket_idx
                ORDER BY bucket_idx ASC"
            );

            let mut stmt = conn.prepare(&sql)?;
            let row_mapper = |row: &rusqlite::Row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    DailyStats {
                        date: String::new(),
                        request_count: row.get::<_, i64>(1)? as u64,
                        total_cost: format!("{:.6}", row.get::<_, f64>(2)?),
                        total_tokens: row.get::<_, i64>(3)? as u64,
                        total_input_tokens: row.get::<_, i64>(4)? as u64,
                        total_output_tokens: row.get::<_, i64>(5)? as u64,
                        total_cache_creation_tokens: row.get::<_, i64>(6)? as u64,
                        total_cache_read_tokens: row.get::<_, i64>(7)? as u64,
                    },
                ))
            };

            let mut map: HashMap<i64, DailyStats> = HashMap::new();

            let mut all_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(start_ts),
                Box::new(end_ts),
                Box::new(bucket_seconds),
            ];
            all_params.extend(extra_params);
            let param_refs: Vec<&dyn rusqlite::ToSql> =
                all_params.iter().map(|p| p.as_ref()).collect();
            let rows = stmt.query_map(param_refs.as_slice(), row_mapper)?;
            for row in rows {
                let (mut bucket_idx, stat) = row?;
                if bucket_idx < 0 {
                    continue;
                }
                if bucket_idx >= bucket_count {
                    bucket_idx = bucket_count - 1;
                }
                map.insert(bucket_idx, stat);
            }

            let mut stats = Vec::with_capacity(bucket_count as usize);
            for i in 0..bucket_count {
                let bucket_start_ts = start_ts + i * bucket_seconds;
                let bucket_start = local_datetime_from_timestamp(bucket_start_ts)?;
                let date = bucket_start.to_rfc3339();

                if let Some(mut stat) = map.remove(&i) {
                    stat.date = date;
                    stats.push(stat);
                } else {
                    stats.push(DailyStats {
                        date,
                        request_count: 0,
                        total_cost: "0.000000".to_string(),
                        total_tokens: 0,
                        total_input_tokens: 0,
                        total_output_tokens: 0,
                        total_cache_creation_tokens: 0,
                        total_cache_read_tokens: 0,
                    });
                }
            }

            return Ok(stats);
        }

        let start_day = local_datetime_from_timestamp(start_ts)?.date_naive();
        let end_day = local_datetime_from_timestamp(end_ts)?.date_naive();
        let bucket_count = (end_day.signed_duration_since(start_day).num_days() + 1) as usize;

        let mut extra_conditions: Vec<String> = Vec::new();
        let mut extra_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(at) = app_type {
            extra_conditions.push(format!(
                "{} = ?",
                app_type_filter_sql("l.app_type", exact_app)
            ));
            extra_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut extra_conditions,
            &mut extra_params,
            "l",
            "p",
            provider_name,
            model,
        );
        let extra_filter = extra_conditions
            .iter()
            .map(|c| format!("AND {c}"))
            .collect::<Vec<_>>()
            .join(" ");
        let detail_join = if provider_name.is_some() {
            providers_join("l", "p")
        } else {
            String::new()
        };

        let effective_filter =
            effective_usage_log_filter_for_range(conn, "l", Some(start_ts), Some(end_ts))?;
        let fresh_input = fresh_input_sql("l");
        let detail_sql = format!(
            "SELECT
                date(l.created_at, 'unixepoch', 'localtime') as bucket_date,
                COUNT(*) as request_count,
                COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
                COALESCE(SUM({fresh_input} + l.output_tokens), 0) as total_tokens,
                COALESCE(SUM({fresh_input}), 0) as total_input_tokens,
                COALESCE(SUM(l.output_tokens), 0) as total_output_tokens,
                COALESCE(SUM(l.cache_creation_tokens), 0) as total_cache_creation_tokens,
                COALESCE(SUM(l.cache_read_tokens), 0) as total_cache_read_tokens
            FROM proxy_request_logs l {detail_join}
            WHERE l.created_at >= ?1 AND l.created_at <= ?2
              AND {effective_filter} {extra_filter}
            GROUP BY bucket_date
            ORDER BY bucket_date ASC"
        );

        let mut detail_stmt = conn.prepare(&detail_sql)?;
        let detail_row_mapper = |row: &rusqlite::Row| {
            Ok((
                row.get::<_, String>(0)?,
                DailyStats {
                    date: String::new(),
                    request_count: row.get::<_, i64>(1)? as u64,
                    total_cost: format!("{:.6}", row.get::<_, f64>(2)?),
                    total_tokens: row.get::<_, i64>(3)? as u64,
                    total_input_tokens: row.get::<_, i64>(4)? as u64,
                    total_output_tokens: row.get::<_, i64>(5)? as u64,
                    total_cache_creation_tokens: row.get::<_, i64>(6)? as u64,
                    total_cache_read_tokens: row.get::<_, i64>(7)? as u64,
                },
            ))
        };

        let mut map: HashMap<NaiveDate, DailyStats> = HashMap::new();
        let mut detail_all_params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(start_ts), Box::new(end_ts)];
        detail_all_params.extend(extra_params);
        let detail_param_refs: Vec<&dyn rusqlite::ToSql> =
            detail_all_params.iter().map(|p| p.as_ref()).collect();
        let detail_rows = detail_stmt.query_map(detail_param_refs.as_slice(), detail_row_mapper)?;

        for row in detail_rows {
            let (bucket_date, stat) = row?;
            let date = NaiveDate::parse_from_str(&bucket_date, "%Y-%m-%d")
                .map_err(|err| AppError::Database(format!("解析趋势日期失败: {err}")))?;
            map.insert(date, stat);
        }

        let rollup_bounds = compute_rollup_date_bounds(Some(start_ts), Some(end_ts))?;
        let mut rollup_conditions = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        push_rollup_date_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r.date",
            &rollup_bounds,
        );
        if let Some(at) = app_type {
            rollup_conditions.push(format!(
                "{} = ?",
                app_type_filter_sql("r.app_type", exact_app)
            ));
            rollup_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );

        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };
        let rollup_join = if provider_name.is_some() {
            providers_join("r", "p2")
        } else {
            String::new()
        };

        let fresh_input_rollup = fresh_input_sql("r");
        let rollup_sql = format!(
            "SELECT
                r.date,
                COALESCE(SUM(r.request_count), 0),
                COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0),
                COALESCE(SUM({fresh_input_rollup} + r.output_tokens), 0),
                COALESCE(SUM({fresh_input_rollup}), 0),
                COALESCE(SUM(r.output_tokens), 0),
                COALESCE(SUM(r.cache_creation_tokens), 0),
                COALESCE(SUM(r.cache_read_tokens), 0)
            FROM usage_daily_rollups r {rollup_join}
            {rollup_where}
            GROUP BY r.date
            ORDER BY r.date ASC"
        );

        let mut rollup_stmt = conn.prepare(&rollup_sql)?;
        let rollup_row_mapper = |row: &rusqlite::Row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, i64>(5)? as u64,
                    row.get::<_, i64>(6)? as u64,
                    row.get::<_, i64>(7)? as u64,
                ),
            ))
        };
        let rollup_param_refs: Vec<&dyn rusqlite::ToSql> =
            rollup_params.iter().map(|param| param.as_ref()).collect();
        let rollup_rows = rollup_stmt.query_map(rollup_param_refs.as_slice(), rollup_row_mapper)?;

        for row in rollup_rows {
            let (bucket_date, (req, cost, tok, inp, out, cc, cr)) = row?;
            let date = NaiveDate::parse_from_str(&bucket_date, "%Y-%m-%d")
                .map_err(|err| AppError::Database(format!("解析 rollup 趋势日期失败: {err}")))?;
            let entry = map.entry(date).or_insert_with(|| DailyStats {
                date: String::new(),
                request_count: 0,
                total_cost: "0.000000".to_string(),
                total_tokens: 0,
                total_input_tokens: 0,
                total_output_tokens: 0,
                total_cache_creation_tokens: 0,
                total_cache_read_tokens: 0,
            });
            entry.request_count += req;
            let existing_cost: f64 = entry.total_cost.parse().unwrap_or(0.0);
            entry.total_cost = format!("{:.6}", existing_cost + cost);
            entry.total_tokens += tok;
            entry.total_input_tokens += inp;
            entry.total_output_tokens += out;
            entry.total_cache_creation_tokens += cc;
            entry.total_cache_read_tokens += cr;
        }

        let mut stats = Vec::with_capacity(bucket_count);
        let mut current_day = start_day;
        for _ in 0..bucket_count {
            let date = local_day_start_rfc3339(current_day);

            if let Some(mut stat) = map.remove(&current_day) {
                stat.date = date;
                stats.push(stat);
            } else {
                stats.push(DailyStats {
                    date,
                    request_count: 0,
                    total_cost: "0.000000".to_string(),
                    total_tokens: 0,
                    total_input_tokens: 0,
                    total_output_tokens: 0,
                    total_cache_creation_tokens: 0,
                    total_cache_read_tokens: 0,
                });
            }

            current_day = current_day.succ_opt().unwrap_or(current_day);
        }

        Ok(stats)
    }

    /// The earliest day that has a usage record (the local date `YYYY-MM-DD`), the detail records
    /// and the daily aggregates are looked at together; None when there is no record.
    ///
    /// It is used to determine the first year of the yearly heatmap of "all", so the cross-source
    /// deduplication is not applied: the deduplicated session rows and their proxy rows are only
    /// a few minutes apart and cannot change the earliest date.
    pub fn get_first_usage_date(
        &self,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::first_usage_date_on_conn(&conn, app_type, provider_name, model)
    }

    pub(crate) fn first_usage_date_on_conn(
        conn: &Connection,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Option<String>, AppError> {
        let mut detail_conditions = Vec::new();
        let mut detail_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(at) = app_type {
            detail_conditions.push(format!("{} = ?", folded_app_type_sql("l.app_type")));
            detail_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut detail_conditions,
            &mut detail_params,
            "l",
            "p",
            provider_name,
            model,
        );
        let detail_where = if detail_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", detail_conditions.join(" AND "))
        };
        let detail_join = if provider_name.is_some() {
            providers_join("l", "p")
        } else {
            String::new()
        };
        let detail_param_refs: Vec<&dyn rusqlite::ToSql> =
            detail_params.iter().map(|p| p.as_ref()).collect();
        let first_detail_ts: Option<i64> = conn.query_row(
            &format!(
                "SELECT MIN(l.created_at) FROM proxy_request_logs l {detail_join} {detail_where}"
            ),
            detail_param_refs.as_slice(),
            |row| row.get(0),
        )?;
        let first_detail = first_detail_ts
            .map(|ts| local_datetime_from_timestamp(ts).map(|dt| dt.date_naive()))
            .transpose()?;

        let mut rollup_conditions = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(at) = app_type {
            rollup_conditions.push(format!("{} = ?", folded_app_type_sql("r.app_type")));
            rollup_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );
        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };
        let rollup_join = if provider_name.is_some() {
            providers_join("r", "p2")
        } else {
            String::new()
        };
        let rollup_param_refs: Vec<&dyn rusqlite::ToSql> =
            rollup_params.iter().map(|p| p.as_ref()).collect();
        let first_rollup_raw: Option<String> = conn.query_row(
            &format!("SELECT MIN(r.date) FROM usage_daily_rollups r {rollup_join} {rollup_where}"),
            rollup_param_refs.as_slice(),
            |row| row.get(0),
        )?;
        let first_rollup = first_rollup_raw
            .map(|raw| {
                NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
                    .map_err(|err| AppError::Database(format!("解析 rollup 日期失败: {err}")))
            })
            .transpose()?;

        let first = match (first_detail, first_rollup) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(first.map(|day| day.format("%Y-%m-%d").to_string()))
    }

    /// Get the provider statistics
    pub fn get_provider_stats(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<ProviderStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::provider_stats_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            true,
        )
    }

    /// The lightweight variant: it skips the scan for the median of the first token. The median
    /// has to pull all the eligible rows of the range into memory to sort them, so the callers
    // that only need the names / the count fields
    // (the dropdown options, the model matching) use this one.
    pub fn get_provider_stats_light(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<ProviderStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::provider_stats_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            false,
        )
    }

    pub(crate) fn provider_stats_impl(
        conn: &Connection,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
        with_medians: bool,
    ) -> Result<Vec<ProviderStats>, AppError> {
        let mut detail_conditions = vec![effective_usage_log_filter_for_range(
            conn, "l", start_date, end_date,
        )?];
        let mut detail_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(start) = start_date {
            detail_conditions.push("l.created_at >= ?".to_string());
            detail_params.push(Box::new(start));
        }
        if let Some(end) = end_date {
            detail_conditions.push("l.created_at <= ?".to_string());
            detail_params.push(Box::new(end));
        }
        if let Some(at) = app_type {
            detail_conditions.push(format!("{} = ?", folded_app_type_sql("l.app_type")));
            detail_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut detail_conditions,
            &mut detail_params,
            "l",
            "p",
            provider_name,
            model,
        );
        let detail_where = if detail_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", detail_conditions.join(" AND "))
        };

        let mut rollup_conditions = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let rollup_bounds = compute_rollup_date_bounds(start_date, end_date)?;
        push_rollup_date_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r.date",
            &rollup_bounds,
        );
        if let Some(at) = app_type {
            rollup_conditions.push(format!("{} = ?", folded_app_type_sql("r.app_type")));
            rollup_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );
        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };

        // UNION detail logs + rollup data, then aggregate
        let detail_pname = provider_name_coalesce("l", "p");
        let rollup_pname = provider_name_coalesce("r", "p2");
        let real_total_detail = real_total_tokens_sql("l");
        let real_total_rollup = real_total_tokens_sql("r");
        let speed_ok = speed_eligible_sql("l");
        let est_ok = speed_estimate_eligible_sql("l");
        let sql = format!(
            "SELECT
                provider_id, app_type, provider_name,
                SUM(request_count) as request_count,
                SUM(total_tokens) as total_tokens,
                SUM(total_cost) as total_cost,
                SUM(success_count) as success_count,
                CASE WHEN SUM(request_count) > 0
                    THEN SUM(latency_sum) / SUM(request_count)
                    ELSE 0 END as avg_latency,
                SUM(speed_output) as speed_output,
                SUM(speed_gen_ms) as speed_gen_ms,
                SUM(est_output) as est_output,
                SUM(est_ms) as est_ms
            FROM (
                SELECT l.provider_id, l.app_type,
                    {detail_pname} as provider_name,
                    COUNT(*) as request_count,
                    COALESCE(SUM({real_total_detail}), 0) as total_tokens,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
                    COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0) as success_count,
                    COALESCE(SUM(l.latency_ms), 0) as latency_sum,
                    COALESCE(SUM(CASE WHEN {speed_ok} THEN l.output_tokens ELSE 0 END), 0) as speed_output,
                    COALESCE(SUM(CASE WHEN {speed_ok} THEN l.latency_ms - l.first_token_ms ELSE 0 END), 0) as speed_gen_ms,
                    COALESCE(SUM(CASE WHEN {est_ok} THEN l.output_tokens ELSE 0 END), 0) as est_output,
                    COALESCE(SUM(CASE WHEN {est_ok} THEN l.latency_ms ELSE 0 END), 0) as est_ms
                FROM proxy_request_logs l
                LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type
                {detail_where}
                GROUP BY l.provider_id, l.app_type
                UNION ALL
                SELECT r.provider_id, r.app_type,
                    {rollup_pname} as provider_name,
                    COALESCE(SUM(r.request_count), 0),
                    COALESCE(SUM({real_total_rollup}), 0),
                    COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0),
                    COALESCE(SUM(r.success_count), 0),
                    COALESCE(SUM(r.avg_latency_ms * r.request_count), 0),
                    0,
                    0,
                    0,
                    0
                FROM usage_daily_rollups r
                LEFT JOIN providers p2 ON r.provider_id = p2.id AND r.app_type = p2.app_type
                {rollup_where}
                GROUP BY r.provider_id, r.app_type
            )
            GROUP BY provider_id, app_type
            ORDER BY total_cost DESC"
        );

        // The group key uses a 0x1F concatenation that cannot appear in an ID, so that it matches the (provider_id, app_type) of the main query below
        let (first_token_ms, est_first_token_ms) = if with_medians {
            let detail_refs: Vec<&dyn rusqlite::ToSql> =
                detail_params.iter().map(|p| p.as_ref()).collect();
            let medians = |eligible_sql: String| {
                first_token_medians(
                    conn,
                    "l.provider_id || char(31) || l.app_type",
                    "FROM proxy_request_logs l
                     LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type",
                    &detail_where,
                    &eligible_sql,
                    &detail_refs,
                )
            };
            (
                medians(first_token_eligible_sql("l"))?,
                medians(est_first_token_eligible_sql("l"))?,
            )
        } else {
            (HashMap::new(), HashMap::new())
        };

        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = detail_params;
        params.extend(rollup_params);
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let row_mapper = |row: &rusqlite::Row| {
            let request_count: i64 = row.get(3)?;
            let provider_id: String = row.get(0)?;
            let app_type: String = row.get(1)?;
            let key = format!("{provider_id}\u{1f}{app_type}");
            let first_token_median_ms = first_token_ms.get(&key).copied();
            let est_first_token_median_ms = est_first_token_ms.get(&key).copied();
            let success_count: i64 = row.get(6)?;
            let success_rate = if request_count > 0 {
                (success_count as f32 / request_count as f32) * 100.0
            } else {
                0.0
            };

            Ok(ProviderStats {
                provider_id,
                app_type,
                provider_name: row.get(2)?,
                request_count: request_count as u64,
                total_tokens: row.get::<_, i64>(4)? as u64,
                total_cost: format!("{:.6}", row.get::<_, f64>(5)?),
                success_rate,
                avg_latency_ms: row.get::<_, f64>(7)? as u64,
                speed_output_tokens: row.get::<_, i64>(8)?.max(0) as u64,
                speed_generation_ms: row.get::<_, i64>(9)?.max(0) as u64,
                est_speed_output_tokens: row.get::<_, i64>(10)?.max(0) as u64,
                est_speed_duration_ms: row.get::<_, i64>(11)?.max(0) as u64,
                first_token_median_ms,
                est_first_token_median_ms,
            })
        };

        let rows = stmt.query_map(param_refs.as_slice(), row_mapper)?;

        let mut stats = Vec::new();
        for row in rows {
            stats.push(row?);
        }

        Ok(stats)
    }

    /// Get the model statistics
    pub fn get_model_stats(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<ModelStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::model_stats_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            true,
        )
    }

    /// The lightweight variant: it skips the scan for the median of the first token, see [`Database::get_provider_stats_light`].
    pub fn get_model_stats_light(
        &self,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
    ) -> Result<Vec<ModelStats>, AppError> {
        let conn = lock_conn!(self.conn);
        Self::model_stats_impl(
            &conn,
            start_date,
            end_date,
            app_type,
            provider_name,
            model,
            false,
        )
    }

    pub(crate) fn model_stats_impl(
        conn: &Connection,
        start_date: Option<i64>,
        end_date: Option<i64>,
        app_type: Option<&str>,
        provider_name: Option<&str>,
        model: Option<&str>,
        with_medians: bool,
    ) -> Result<Vec<ModelStats>, AppError> {
        let mut detail_conditions = vec![effective_usage_log_filter_for_range(
            conn, "l", start_date, end_date,
        )?];
        let mut detail_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(start) = start_date {
            detail_conditions.push("l.created_at >= ?".to_string());
            detail_params.push(Box::new(start));
        }
        if let Some(end) = end_date {
            detail_conditions.push("l.created_at <= ?".to_string());
            detail_params.push(Box::new(end));
        }
        if let Some(at) = app_type {
            detail_conditions.push(format!("{} = ?", folded_app_type_sql("l.app_type")));
            detail_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut detail_conditions,
            &mut detail_params,
            "l",
            "p",
            provider_name,
            model,
        );
        let detail_where = if detail_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", detail_conditions.join(" AND "))
        };
        let detail_join = if provider_name.is_some() {
            providers_join("l", "p")
        } else {
            String::new()
        };

        let mut rollup_conditions = Vec::new();
        let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let rollup_bounds = compute_rollup_date_bounds(start_date, end_date)?;
        push_rollup_date_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r.date",
            &rollup_bounds,
        );
        if let Some(at) = app_type {
            rollup_conditions.push(format!("{} = ?", folded_app_type_sql("r.app_type")));
            rollup_params.push(Box::new(at.to_string()));
        }
        push_provider_model_filters(
            &mut rollup_conditions,
            &mut rollup_params,
            "r",
            "p2",
            provider_name,
            model,
        );
        let rollup_where = if rollup_conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", rollup_conditions.join(" AND "))
        };
        let rollup_join = if provider_name.is_some() {
            providers_join("r", "p2")
        } else {
            String::new()
        };

        // UNION detail logs + rollup data
        //
        // The group key uses the "effective pricing model": pricing_model wins when it is not empty
        // (the cost was computed with its
        // pricing, so the amount is consistent with the pricing table) and it falls back to model for
        // NULL/''. In the default response pricing
        // mode both are the same and the behaviour does not change; in the request mode with a
        // router takeover the money belongs under the name of the actual pricing
        // base and not under the name the upstream echoed or the client alias uses.
        let real_total_detail = real_total_tokens_sql("l");
        let real_total_rollup = real_total_tokens_sql("r");
        let detail_model = effective_model_sql("l");
        let rollup_model = effective_model_sql("r");
        let speed_ok = speed_eligible_sql("l");
        let est_ok = speed_estimate_eligible_sql("l");
        let sql = format!(
            "SELECT
                model,
                SUM(request_count) as request_count,
                SUM(total_tokens) as total_tokens,
                SUM(total_cost) as total_cost,
                SUM(success_count) as success_count,
                SUM(speed_output) as speed_output,
                SUM(speed_gen_ms) as speed_gen_ms,
                SUM(est_output) as est_output,
                SUM(est_ms) as est_ms
            FROM (
                SELECT {detail_model} as model,
                    COUNT(*) as request_count,
                    COALESCE(SUM({real_total_detail}), 0) as total_tokens,
                    COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
                    COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300 THEN 1 ELSE 0 END), 0) as success_count,
                    COALESCE(SUM(CASE WHEN {speed_ok} THEN l.output_tokens ELSE 0 END), 0) as speed_output,
                    COALESCE(SUM(CASE WHEN {speed_ok} THEN l.latency_ms - l.first_token_ms ELSE 0 END), 0) as speed_gen_ms,
                    COALESCE(SUM(CASE WHEN {est_ok} THEN l.output_tokens ELSE 0 END), 0) as est_output,
                    COALESCE(SUM(CASE WHEN {est_ok} THEN l.latency_ms ELSE 0 END), 0) as est_ms
                FROM proxy_request_logs l
                {detail_join}
                {detail_where}
                GROUP BY {detail_model}
                UNION ALL
                SELECT {rollup_model},
                    COALESCE(SUM(r.request_count), 0),
                    COALESCE(SUM({real_total_rollup}), 0),
                    COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0),
                    COALESCE(SUM(r.success_count), 0),
                    0,
                    0,
                    0,
                    0
                FROM usage_daily_rollups r
                {rollup_join}
                {rollup_where}
                GROUP BY {rollup_model}
            )
            GROUP BY model
            ORDER BY total_cost DESC"
        );

        let (first_token_ms, est_first_token_ms) = if with_medians {
            let detail_refs: Vec<&dyn rusqlite::ToSql> =
                detail_params.iter().map(|p| p.as_ref()).collect();
            let from_sql = format!("FROM proxy_request_logs l {detail_join}");
            let medians = |eligible_sql: String| {
                first_token_medians(
                    conn,
                    &detail_model,
                    &from_sql,
                    &detail_where,
                    &eligible_sql,
                    &detail_refs,
                )
            };
            (
                medians(first_token_eligible_sql("l"))?,
                medians(est_first_token_eligible_sql("l"))?,
            )
        } else {
            (HashMap::new(), HashMap::new())
        };

        let mut stmt = conn.prepare(&sql)?;
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = detail_params;
        params.extend(rollup_params);
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let row_mapper = |row: &rusqlite::Row| {
            let request_count: i64 = row.get(1)?;
            let model: String = row.get(0)?;
            let first_token_median_ms = first_token_ms.get(&model).copied();
            let est_first_token_median_ms = est_first_token_ms.get(&model).copied();
            let total_cost: f64 = row.get(3)?;
            let avg_cost = if request_count > 0 {
                total_cost / request_count as f64
            } else {
                0.0
            };
            let success_count: i64 = row.get(4)?;
            let success_rate = if request_count > 0 {
                (success_count as f32 / request_count as f32) * 100.0
            } else {
                0.0
            };

            Ok(ModelStats {
                model,
                request_count: request_count as u64,
                total_tokens: row.get::<_, i64>(2)? as u64,
                total_cost: format!("{total_cost:.6}"),
                avg_cost_per_request: format!("{avg_cost:.6}"),
                success_rate,
                speed_output_tokens: row.get::<_, i64>(5)?.max(0) as u64,
                speed_generation_ms: row.get::<_, i64>(6)?.max(0) as u64,
                est_speed_output_tokens: row.get::<_, i64>(7)?.max(0) as u64,
                est_speed_duration_ms: row.get::<_, i64>(8)?.max(0) as u64,
                first_token_median_ms,
                est_first_token_median_ms,
            })
        };

        let rows = stmt.query_map(param_refs.as_slice(), row_mapper)?;

        let mut stats = Vec::new();
        for row in rows {
            stats.push(row?);
        }

        Ok(stats)
    }

    /// Get the list of the request logs (paginated)
    pub fn get_request_logs(
        &self,
        filters: &LogFilters,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedLogs, AppError> {
        let conn = lock_conn!(self.conn);
        let mut count_cache = lock_conn!(self.log_count_cache);
        let epoch = self.write_epoch.load(std::sync::atomic::Ordering::Relaxed);
        Self::request_logs_on_conn(&conn, &mut count_cache, epoch, filters, page, page_size)
    }

    /// The single lock version of the paginated detail query: `count_cache` is passed in by the
    /// caller (it is reused within the same connection lock)
    /// and `epoch` is the write epoch that was read at the call time. It is used by the single
    // snapshot aggregation of the dashboard.
    pub(crate) fn request_logs_on_conn(
        conn: &Connection,
        count_cache: &mut LogCountCache,
        epoch: u64,
        filters: &LogFilters,
        page: u32,
        page_size: u32,
    ) -> Result<PaginatedLogs, AppError> {
        let mut conditions = vec![effective_usage_log_filter_for_range(
            conn,
            "l",
            filters.start_date,
            filters.end_date,
        )?];
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ref app_type) = filters.app_type {
            // Only the filter folds claude-desktop -> claude; the row projection still returns the
            // original app_type,
            // the detail panel shows the real entry point with it (the billing audit of a router
            // takeover needs that).
            conditions.push(format!("{} = ?", folded_app_type_sql("l.app_type")));
            params.push(Box::new(app_type.clone()));
        }
        // The same definition as the dropdown filter at the top of the Dashboard: a provider matches
        // exactly by its display name (a session placeholder
        // row such as "Claude (Session)" can be hit too) and a model matches by the effective
        // pricing model.
        push_provider_model_filters(
            &mut conditions,
            &mut params,
            "l",
            "p",
            filters.provider_name.as_deref(),
            filters.model.as_deref(),
        );
        if let Some(status) = filters.status_code {
            conditions.push("l.status_code = ?".to_string());
            params.push(Box::new(status as i64));
        }
        if let Some(start) = filters.start_date {
            conditions.push("l.created_at >= ?".to_string());
            params.push(Box::new(start));
        }
        if let Some(end) = filters.end_date {
            conditions.push("l.created_at <= ?".to_string());
            params.push(Box::new(end));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };

        // Get the total
        let count_sql = format!(
            "SELECT COUNT(*) FROM proxy_request_logs l
             LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type
             {where_clause}"
        );
        let total: u32 = match cached_log_count(count_cache, epoch, conn, filters)? {
            Some(total) => total,
            None => {
                let count_params: Vec<&dyn rusqlite::ToSql> =
                    params.iter().map(|p| p.as_ref()).collect();
                let total = conn.query_row(&count_sql, count_params.as_slice(), |row| {
                    row.get::<_, i64>(0).map(|v| v as u32)
                })?;
                store_log_count(count_cache, filters, epoch, total);
                total
            }
        };

        // Get the data
        let offset = page * page_size;
        params.push(Box::new(page_size as i64));
        params.push(Box::new(offset as i64));

        let logs_pname = provider_name_coalesce("l", "p");
        let sql = format!(
            "SELECT l.request_id, l.provider_id, {logs_pname} as provider_name, l.app_type, l.model,
                    l.request_model, l.cost_multiplier,
                    l.input_tokens, l.output_tokens, l.cache_read_tokens, l.cache_creation_tokens,
                    l.input_cost_usd, l.output_cost_usd, l.cache_read_cost_usd, l.cache_creation_cost_usd, l.total_cost_usd,
                    l.is_streaming, l.latency_ms, l.first_token_ms, l.duration_ms,
                    l.status_code, l.error_message, l.created_at, l.data_source, l.pricing_model,
                    l.input_token_semantics
             FROM proxy_request_logs l
             LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type
             {where_clause}
             ORDER BY l.created_at DESC
             LIMIT ? OFFSET ?"
        );

        let mut stmt = conn.prepare(&sql)?;
        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let rows = stmt.query_map(params_refs.as_slice(), row_to_request_log_detail)?;

        // The read path does not do a cost backfill: a write would interrupt the read operation and
        // every UPDATE would invalidate the
        // count caches and the projections. Every change of the pricing already triggers the
        // backfill explicitly (sync_local_model_pricing,
        // the bulk update), so it is enough to wait for the next pricing event.
        let logs: Vec<_> = rows.collect::<Result<_, _>>()?;

        Ok(PaginatedLogs {
            data: logs,
            total,
            page,
            page_size,
        })
    }

    /// Get the detail of a single request
    pub fn get_request_detail(
        &self,
        request_id: &str,
    ) -> Result<Option<RequestLogDetail>, AppError> {
        let conn = lock_conn!(self.conn);

        let detail_pname = provider_name_coalesce("l", "p");
        let detail_sql = format!(
            "SELECT l.request_id, l.provider_id, {detail_pname} as provider_name, l.app_type, l.model,
                    l.request_model, l.cost_multiplier,
                    l.input_tokens, l.output_tokens, l.cache_read_tokens, l.cache_creation_tokens,
                    l.input_cost_usd, l.output_cost_usd, l.cache_read_cost_usd, l.cache_creation_cost_usd, l.total_cost_usd,
                    l.is_streaming, l.latency_ms, l.first_token_ms, l.duration_ms,
                    l.status_code, l.error_message, l.created_at, l.data_source, l.pricing_model,
                    l.input_token_semantics
             FROM proxy_request_logs l
             LEFT JOIN providers p ON l.provider_id = p.id AND l.app_type = p.app_type
             WHERE l.request_id = ?"
        );
        let result = conn.query_row(&detail_sql, [request_id], row_to_request_log_detail);

        match result {
            // The read path does not do a cost backfill, see get_request_logs for the reason.
            Ok(detail) => Ok(Some(detail)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(AppError::Database(e.to_string())),
        }
    }

    /// Check the usage limits of a provider
    pub fn check_provider_limits(
        &self,
        provider_id: &str,
        app_type: &str,
    ) -> Result<ProviderLimitStatus, AppError> {
        let conn = lock_conn!(self.conn);

        // Get the limit settings of the provider
        let (limit_daily, limit_monthly) = conn
            .query_row(
                "SELECT meta FROM providers WHERE id = ? AND app_type = ?",
                params![provider_id, app_type],
                |row| {
                    let meta_str: String = row.get(0)?;
                    Ok(meta_str)
                },
            )
            .ok()
            .and_then(|meta_str| serde_json::from_str::<serde_json::Value>(&meta_str).ok())
            .map(|meta| {
                let daily = meta
                    .get("limitDailyUsd")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());
                let monthly = meta
                    .get("limitMonthlyUsd")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<f64>().ok());
                (daily, monthly)
            })
            .unwrap_or((None, None));

        // Compute the usage of today (detail logs + rollup)
        let daily_usage: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost), 0) FROM (
                    SELECT CAST(total_cost_usd AS REAL) as cost
                    FROM proxy_request_logs
                    WHERE provider_id = ? AND app_type = ?
                      AND date(datetime(created_at, 'unixepoch', 'localtime')) = date('now', 'localtime')
                    UNION ALL
                    SELECT CAST(total_cost_usd AS REAL)
                    FROM usage_daily_rollups
                    WHERE provider_id = ? AND app_type = ?
                      AND date = date('now', 'localtime')
                )",
                params![provider_id, app_type, provider_id, app_type],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        // Compute the usage of this month (detail logs + rollup)
        let monthly_usage: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(cost), 0) FROM (
                    SELECT CAST(total_cost_usd AS REAL) as cost
                    FROM proxy_request_logs
                    WHERE provider_id = ? AND app_type = ?
                      AND strftime('%Y-%m', datetime(created_at, 'unixepoch', 'localtime')) = strftime('%Y-%m', 'now', 'localtime')
                    UNION ALL
                    SELECT CAST(total_cost_usd AS REAL)
                    FROM usage_daily_rollups
                    WHERE provider_id = ? AND app_type = ?
                      AND strftime('%Y-%m', date) = strftime('%Y-%m', 'now', 'localtime')
                )",
                params![provider_id, app_type, provider_id, app_type],
                |row| row.get(0),
            )
            .unwrap_or(0.0);

        let daily_exceeded = limit_daily
            .map(|limit| daily_usage >= limit)
            .unwrap_or(false);
        let monthly_exceeded = limit_monthly
            .map(|limit| monthly_usage >= limit)
            .unwrap_or(false);

        Ok(ProviderLimitStatus {
            provider_id: provider_id.to_string(),
            daily_usage: format!("{daily_usage:.6}"),
            daily_limit: limit_daily.map(|l| format!("{l:.2}")),
            daily_exceeded,
            monthly_usage: format!("{monthly_usage:.6}"),
            monthly_limit: limit_monthly.map(|l| format!("{l:.2}")),
            monthly_exceeded,
        })
    }
}

/// The limit state of a provider
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimitStatus {
    pub provider_id: String,
    pub daily_usage: String,
    pub daily_limit: Option<String>,
    pub daily_exceeded: bool,
    pub monthly_usage: String,
    pub monthly_limit: Option<String>,
    pub monthly_exceeded: bool,
}

#[derive(Clone)]
struct PricingInfo {
    input: rust_decimal::Decimal,
    output: rust_decimal::Decimal,
    cache_read: rust_decimal::Decimal,
    cache_creation: rust_decimal::Decimal,
}

impl Database {
    /// Aggregate the detail records older than `days` days into `usage_daily_rollups` by day and
    /// then delete them (the pruning).
    ///
    /// Returns the number of deleted detail rows (including the session twin rows that were
    /// hidden by a proxy row and were deleted along with this batch).
    ///
    /// The invariant (it aligns the query layer with the two legs "detail + rollup", both of which
    /// cover the whole range, and it relies on the data to guarantee that the same logical
    /// request only ever ends up on one side): the aggregation and the deletion act on the same
    /// set of rows in the same transaction and the sum of both sides equals the real total at any
    // point in time. Two easy mistakes:
    // 1. A session row that is hidden by a proxy row does not produce a rollup (its
    //    contribution is carried by the proxy row), but it must
    //    be deleted with this batch -- otherwise the twin row would flip from "hidden" to
    //    "visible" once the proxy row is pruned and the query would
    //    count it twice. The pruning set is therefore the expired detail rows union the session
    //    twin rows that are hidden by the proxy rows of this batch
    //    (a twin and its proxy row are no further apart than the deduplication window, so both
    //    are old data anyway).
    // 2. The input of a rollup row records the fresh value after the per-row normalization (the
    //    semantics are set to FRESH and the query side
    //    [`fresh_input_sql`] returns it unchanged); when a rollup row is accumulated onto, the
    //    old value is first normalized to fresh according to its
    //    own semantics and only then added.
    ///
    /// The first call can process the whole backlog of the history at once (one transaction); in
    /// the steady state every round only handles about a day of newly expired detail records.
    pub fn rollup_and_prune(&self, days: i64) -> Result<u64, AppError> {
        if days <= 0 {
            return Err(AppError::InvalidInput("保留天数必须为正".into()));
        }
        let cutoff = chrono::Utc::now().timestamp() - days.saturating_mul(86_400);
        let mut conn = lock_conn!(self.conn);
        // The early return: without expired detail rows no transaction is opened (the created_at index short circuits, zero cost in the steady state).
        let has_old: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM proxy_request_logs WHERE created_at < ?1)",
                [cutoff],
                |row| row.get(0),
            )
            .map_err(|e| AppError::Database(format!("检查剪枝候选失败: {e}")))?;
        if !has_old {
            return Ok(0);
        }
        let tx = conn
            .transaction()
            .map_err(|e| AppError::Database(format!("开启剪枝事务失败: {e}")))?;

        // The predicate of the pruning set: the expired detail rows plus the session twin rows that
        // are hidden by the proxy rows of those rows.
        // The fingerprint matching is aligned row by row with the EXISTS of
        // [`effective_usage_log_filter`],
        // only the probe alias is fixed to proxy_prune and created_at < cutoff is appended.
        let data_source = data_source_expr("l");
        let proxy_data_source = data_source_expr("proxy_prune");
        let origin_match = same_usage_origin_sql("proxy_prune", "l");
        let app_type_match = dedup_app_type_match_sql("proxy_prune.app_type", "l.app_type");
        let window = SESSION_PROXY_DEDUP_WINDOW_SECONDS;
        let hidden_twin = format!(
            "{data_source} IN ({DEDUP_SESSION_SOURCES_SQL}) AND EXISTS (
                SELECT 1 FROM proxy_request_logs proxy_prune
                WHERE proxy_prune.created_at < ?1
                  AND {proxy_data_source} = 'proxy'
                  AND {origin_match}
                  AND {app_type_match}
                  AND proxy_prune.status_code >= 200 AND proxy_prune.status_code < 300
                  AND proxy_prune.input_tokens = l.input_tokens
                  AND proxy_prune.output_tokens = l.output_tokens
                  AND proxy_prune.cache_read_tokens = l.cache_read_tokens
                  AND (
                      proxy_prune.cache_creation_tokens = l.cache_creation_tokens
                      OR (l.cache_creation_tokens = 0
                          AND {data_source} IN ('codex_session', 'gemini_session', 'opencode_session'))
                      OR (proxy_prune.cache_creation_tokens = 0
                          AND proxy_prune.input_token_semantics = {INPUT_TOKEN_SEMANTICS_LEGACY}
                          AND {data_source} = 'codex_session')
                  )
                  AND proxy_prune.created_at BETWEEN
                      l.created_at - {window} AND l.created_at + {window}
                  AND (LOWER(proxy_prune.model) = LOWER(l.model)
                       OR LOWER(proxy_prune.model) = 'unknown' OR LOWER(l.model) = 'unknown')
            )"
        );
        let prune_set = format!("(l.created_at < ?1 OR {hidden_twin})");
        let visibility = effective_usage_log_filter("l");

        // Aggregate the visible rows: the date uses the local time zone (the same as the rollup date boundary of the query layer).
        let rollup_sql = format!(
            "INSERT INTO usage_daily_rollups (
                date, app_type, provider_id, model, request_model, pricing_model,
                request_count, success_count, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, input_token_semantics,
                total_cost_usd, avg_latency_ms)
            SELECT
                date(l.created_at, 'unixepoch', 'localtime'),
                l.app_type, l.provider_id, l.model,
                COALESCE(l.request_model, ''), COALESCE(l.pricing_model, ''),
                COUNT(*),
                COALESCE(SUM(CASE WHEN l.status_code >= 200 AND l.status_code < 300
                    THEN 1 ELSE 0 END), 0),
                COALESCE(SUM({fresh}), 0),
                COALESCE(SUM(l.output_tokens), 0),
                COALESCE(SUM(l.cache_read_tokens), 0),
                COALESCE(SUM(l.cache_creation_tokens), 0),
                {INPUT_TOKEN_SEMANTICS_FRESH},
                CAST(COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) AS TEXT),
                CAST(COALESCE(AVG(l.latency_ms), 0) AS INTEGER)
            FROM proxy_request_logs l
            WHERE {prune_set} AND {visibility}
            GROUP BY date(l.created_at, 'unixepoch', 'localtime'),
                l.app_type, l.provider_id, l.model,
                COALESCE(l.request_model, ''), COALESCE(l.pricing_model, '')
            ON CONFLICT(date, app_type, provider_id, model, request_model, pricing_model) DO UPDATE SET
                request_count = usage_daily_rollups.request_count + excluded.request_count,
                success_count = usage_daily_rollups.success_count + excluded.success_count,
                input_tokens = ({old_fresh}) + excluded.input_tokens,
                output_tokens = usage_daily_rollups.output_tokens + excluded.output_tokens,
                cache_read_tokens = usage_daily_rollups.cache_read_tokens + excluded.cache_read_tokens,
                cache_creation_tokens = usage_daily_rollups.cache_creation_tokens + excluded.cache_creation_tokens,
                input_token_semantics = {INPUT_TOKEN_SEMANTICS_FRESH},
                total_cost_usd = CAST(CAST(usage_daily_rollups.total_cost_usd AS REAL)
                    + CAST(excluded.total_cost_usd AS REAL) AS TEXT),
                avg_latency_ms = CAST(
                    (usage_daily_rollups.avg_latency_ms * usage_daily_rollups.request_count
                        + excluded.avg_latency_ms * excluded.request_count)
                    / MAX(1, usage_daily_rollups.request_count + excluded.request_count)
                    AS INTEGER)",
            fresh = fresh_input_sql("l"),
            old_fresh = fresh_input_sql("usage_daily_rollups"),
        );
        tx.execute(&rollup_sql, params![cutoff])
            .map_err(|e| AppError::Database(format!("剪枝汇总失败: {e}")))?;

        // Delete the whole pruning set: the visible rows were aggregated and the hidden rows (the
        // twins) are deleted but not aggregated -- their
        // logical contribution is carried by the proxy rows that are still visible. Keeping them
        // would make the twin flip from "hidden" to
        // "visible" and cause a double count once the proxy rows are pruned someday.
        let deleted = tx
            .execute(
                &format!(
                    "DELETE FROM proxy_request_logs WHERE rowid IN (
                    SELECT l.rowid FROM proxy_request_logs l WHERE {prune_set}
                )"
                ),
                params![cutoff],
            )
            .map_err(|e| AppError::Database(format!("剪枝删除明细失败: {e}")))?;
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交剪枝事务失败: {e}")))?;
        if deleted > 0 {
            log::info!("已将 {deleted} 条 {days} 天前的明细汇总进按天表并删除");
        }
        Ok(deleted as u64)
    }

    /// Recalculate stored zero-cost usage rows once pricing becomes available.
    pub(crate) fn backfill_missing_usage_costs(&self) -> Result<u64, AppError> {
        let conn = lock_conn!(self.conn);
        Self::backfill_missing_usage_costs_on_conn(&conn, None)
    }

    /// Only backfill the zero-cost rows that belong to the given model_id; it is used for the precise backfill after a single pricing update.
    pub(crate) fn backfill_missing_usage_costs_for_model(
        &self,
        model_id: &str,
    ) -> Result<u64, AppError> {
        let conn = lock_conn!(self.conn);
        Self::backfill_missing_usage_costs_on_conn(&conn, Some(model_id))
    }

    pub(crate) fn backfill_missing_usage_costs_on_conn(
        conn: &Connection,
        only_model_id: Option<&str>,
    ) -> Result<u64, AppError> {
        const BASE_SQL: &str =
            "SELECT request_id, provider_id, NULL AS provider_name, app_type, model, request_model,
                        cost_multiplier,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        input_cost_usd, output_cost_usd, cache_read_cost_usd,
                        cache_creation_cost_usd, total_cost_usd, is_streaming, latency_ms,
                        first_token_ms, duration_ms, status_code, error_message, created_at,
                        data_source, pricing_model, input_token_semantics
             FROM proxy_request_logs
             WHERE CAST(total_cost_usd AS REAL) <= 0
               AND (input_tokens > 0 OR output_tokens > 0
                    OR cache_read_tokens > 0 OR cache_creation_tokens > 0)";

        let mut logs = {
            let mut stmt = conn.prepare(BASE_SQL)?;
            let rows = stmt.query_map([], row_to_request_log_detail)?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        // The row filter of the precise backfill has to share the candidates normalization of the
        // price lookup layer: an exact SQL match misses the
        // rows that were stored under their raw alias (for example openrouter/anthropic/claude-sonnet-4.5:free),
        // those rows do normalize to a hit of the new pricing during the price lookup but are blocked
        // at the filter layer, so that after a pricing import
        // the historical costs only get updated at the next full backfill. A false positive is
        // harmless -- the rows without a price are skipped.
        if let Some(model_id) = only_model_id {
            let target = model_pricing_candidates(model_id);
            logs.retain(|log| log_pricing_scope_matches(log, &target));
        }

        if logs.is_empty() {
            return Ok(0);
        }

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Database(format!("启动用量成本回填事务失败: {e}")))?;

        let mut updated = 0u64;
        let mut pricing_cache = HashMap::new();
        for log in &mut logs {
            if Self::maybe_backfill_log_costs(&tx, log, &mut pricing_cache)? {
                updated += 1;
            }
        }
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交用量成本回填事务失败: {e}")))?;

        if updated > 0 {
            log::info!("已回填 {updated} 条缺失的用量成本");
        }

        Ok(updated)
    }

    /// Try to backfill the cost fields of a single log. Returns whether something was actually written (true = it was UPDATED, false = it was skipped).
    fn maybe_backfill_log_costs(
        conn: &Connection,
        log: &mut RequestLogDetail,
        pricing_cache: &mut HashMap<String, PricingInfo>,
    ) -> Result<bool, AppError> {
        let existing_cost = rust_decimal::Decimal::from_str(&log.total_cost_usd)
            .unwrap_or(rust_decimal::Decimal::ZERO);
        let has_cost = existing_cost > rust_decimal::Decimal::ZERO;
        let has_usage = log.input_tokens > 0
            || log.output_tokens > 0
            || log.cache_read_tokens > 0
            || log.cache_creation_tokens > 0;

        if has_cost || !has_usage {
            return Ok(false);
        }

        let pricing = match Self::get_log_model_pricing_cached(conn, pricing_cache, log)? {
            Some(info) => info,
            None => return Ok(false),
        };
        let multiplier =
            rust_decimal::Decimal::from_str(&log.cost_multiplier).unwrap_or_else(|e| {
                log::warn!(
                    "历史用量倍率解析失败 request_id={}: {} - {e}",
                    log.request_id,
                    log.cost_multiplier
                );
                rust_decimal::Decimal::ONE
            });

        let million = rust_decimal::Decimal::from(1_000_000u64);

        // The computation stays consistent with CostCalculator::calculate_for_app:
        // 1. A historical cache-inclusive row only contains the cache read; a new total row also
        //    contains the cache write.
        // 2. The input_tokens of Claude/Anthropic are already fresh input and must not be subtracted
        //    again
        // 3. The individual costs are the base costs (without the multiplier) and the multiplier only
        //    applies to the final total
        let cache_inclusive_app =
            crate::services::sql_helpers::is_cache_inclusive_app(log.app_type.as_str());
        let billable_input_tokens =
            if !cache_inclusive_app || log.input_token_semantics == INPUT_TOKEN_SEMANTICS_FRESH {
                log.input_tokens as u64
            } else if log.input_token_semantics == INPUT_TOKEN_SEMANTICS_TOTAL {
                (log.input_tokens as u64)
                    .saturating_sub(log.cache_read_tokens as u64)
                    .saturating_sub(log.cache_creation_tokens as u64)
            } else {
                // v12 and earlier: input included cache reads but excluded cache writes.
                (log.input_tokens as u64).saturating_sub(log.cache_read_tokens as u64)
            };
        let input_cost =
            rust_decimal::Decimal::from(billable_input_tokens) * pricing.input / million;
        let output_cost =
            rust_decimal::Decimal::from(log.output_tokens as u64) * pricing.output / million;
        let cache_read_cost = rust_decimal::Decimal::from(log.cache_read_tokens as u64)
            * pricing.cache_read
            / million;
        let cache_creation_cost = rust_decimal::Decimal::from(log.cache_creation_tokens as u64)
            * pricing.cache_creation
            / million;
        // The total cost = the sum of the base costs x the multiplier
        let base_total = input_cost + output_cost + cache_read_cost + cache_creation_cost;
        let total_cost = base_total * multiplier;

        log.input_cost_usd = format!("{input_cost:.6}");
        log.output_cost_usd = format!("{output_cost:.6}");
        log.cache_read_cost_usd = format!("{cache_read_cost:.6}");
        log.cache_creation_cost_usd = format!("{cache_creation_cost:.6}");
        log.total_cost_usd = format!("{total_cost:.6}");

        conn.execute(
            "UPDATE proxy_request_logs
             SET input_cost_usd = ?1,
                 output_cost_usd = ?2,
                 cache_read_cost_usd = ?3,
                 cache_creation_cost_usd = ?4,
                 total_cost_usd = ?5
             WHERE request_id = ?6",
            params![
                log.input_cost_usd,
                log.output_cost_usd,
                log.cache_read_cost_usd,
                log.cache_creation_cost_usd,
                log.total_cost_usd,
                log.request_id
            ],
        )
        .map_err(|e| AppError::Database(format!("更新请求成本失败: {e}")))?;

        Ok(true)
    }

    fn get_model_pricing_cached(
        conn: &Connection,
        cache: &mut HashMap<String, PricingInfo>,
        model: &str,
    ) -> Result<Option<PricingInfo>, AppError> {
        if let Some(info) = cache.get(model) {
            return Ok(Some(info.clone()));
        }

        let row = find_model_pricing_row(conn, model)?;
        let Some((input, output, cache_read, cache_creation)) = row else {
            return Ok(None);
        };

        let pricing = PricingInfo {
            input: rust_decimal::Decimal::from_str(&input)
                .map_err(|e| AppError::Database(format!("解析输入价格失败: {e}")))?,
            output: rust_decimal::Decimal::from_str(&output)
                .map_err(|e| AppError::Database(format!("解析输出价格失败: {e}")))?,
            cache_read: rust_decimal::Decimal::from_str(&cache_read)
                .map_err(|e| AppError::Database(format!("解析缓存读取价格失败: {e}")))?,
            cache_creation: rust_decimal::Decimal::from_str(&cache_creation)
                .map_err(|e| AppError::Database(format!("解析缓存写入价格失败: {e}")))?,
        };

        cache.insert(model.to_string(), pricing.clone());
        Ok(Some(pricing))
    }

    fn get_log_model_pricing_cached(
        conn: &Connection,
        cache: &mut HashMap<String, PricingInfo>,
        log: &RequestLogDetail,
    ) -> Result<Option<PricingInfo>, AppError> {
        // The pricing base at write time is already stored (v11+): the backfill only recomputes with
        // it and keeps the cost at 0 when it cannot be found,
        // waiting for a price. It must not be switched to model/request_model to guess -- in the
        // router takeover with the request pricing mode
        // the three of them can differ (model = the echo of the upstream, request_model = the client
        // alias,
        // pricing_model = the model that actually went out) and switching the base would permanently
        // freeze the row at the wrong price.
        // A placeholder ("" = an unpriced error row / "unknown") counts as missing and follows the
        // logic of the historical rows.
        if let Some(pricing_model) = log
            .pricing_model
            .as_deref()
            .filter(|pm| !is_placeholder_pricing_model(pm))
        {
            return Self::get_model_pricing_cached(conn, cache, pricing_model);
        }

        if let Some(pricing) = Self::get_model_pricing_cached(conn, cache, &log.model)? {
            return Ok(Some(pricing));
        }

        // The fallback to the request_model pricing only happens when the model column is a
        // placeholder (the "" / "unknown" that a failed parse left behind).
        // When the model is a real model name but the pricing is missing, the cost has to stay at 0
        // and wait for a price: under a router takeover the request_model is the client alias (for
        // example claude-sonnet-4-6) and
        // a backfill with the alias would permanently freeze the tokens of the real upstream model
        // at the wrong price (a row that has a cost does not
        // enter the backfill range any more).
        if !is_placeholder_pricing_model(&log.model) {
            return Ok(None);
        }

        let Some(request_model) = log.request_model.as_deref() else {
            return Ok(None);
        };
        if request_model == log.model {
            return Ok(None);
        }

        Self::get_model_pricing_cached(conn, cache, request_model)
    }
}

/// The per-local-date aggregate rows of the read-only usage query.
///
/// The same definition as [`DailyStats`]. `total_cost` keeps a 6 decimal string instead of an
/// f64 so that
/// the layer above can accumulate it as a fixed point decimal without another rounding
/// error.
#[derive(Debug, Clone)]
pub struct DailyUsageRow {
    pub date: NaiveDate,
    pub request_count: u64,
    pub total_cost: String,
    pub total_tokens: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cache_creation_tokens: u64,
    pub total_cache_read_tokens: u64,
}

/// Build the `IN (...)` fragment out of the list of app identifiers.
///
/// The caller has to make sure the list is not empty: `IN ()` is not valid SQL.
fn app_type_in_list(apps: &[&str]) -> String {
    apps.iter()
        .map(|value| format!("'{}'", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Build the `IN (...)` fragment out of the list of model identifiers.
///
/// The same escaping rules as [`app_type_in_list`]: a single quote is doubled so that it
/// cannot be concatenated into the SQL.
fn model_in_list(models: &[String]) -> String {
    models
        .iter()
        .map(|value| format!("'{}'", value.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// If a platform has no entry in the map, all of its models are included.
fn app_model_filter(
    alias: &str,
    apps: &[&str],
    models: Option<&[String]>,
    by_app: Option<&BTreeMap<String, Vec<String>>>,
) -> String {
    if let Some(by_app) = by_app {
        let parts: Vec<String> = apps
            .iter()
            .map(|app| {
                let app_condition = format!("{alias}.app_type = '{}'", app.replace('\'', "''"));
                match by_app.get(*app) {
                    Some(list) => format!(
                        "({app_condition} AND {} IN ({}))",
                        effective_model_sql(alias),
                        model_in_list(list)
                    ),
                    None => format!("({app_condition})"),
                }
            })
            .collect();
        format!("({})", parts.join(" OR "))
    } else {
        let app_condition = format!("{alias}.app_type IN ({})", app_type_in_list(apps));
        match models.filter(|m| !m.is_empty()) {
            Some(list) => format!(
                "{app_condition} AND {} IN ({})",
                effective_model_sql(alias),
                model_in_list(list)
            ),
            None => app_condition,
        }
    }
}

/// The read-only usage query: fetch the per-local-date aggregate of "detail + daily rollup"
/// of the range in one go.
///
/// The same definition as [`Database::get_daily_trends`]: the same cross-source deduplication
/// filter, the same normalization of the cache input semantics and the same
/// rules for the daily rollup boundaries. The differences are:
///
/// - Several apps are supported (`app_type IN (...)`) and no loop over the apps is needed;
/// - The range is half-open `[start_ts, end_ts_exclusive)`;
/// - There are no hourly buckets and no zero padding, the caller assembles them with its own
///   grouping;
/// - It is a connection level function: the caller has to call it through
///   [`crate::database::Database::with_connection`]
///   under the same lock, so that the multiple queries see the same data.
pub(crate) fn daily_usage_rows(
    conn: &Connection,
    start_ts: i64,
    end_ts_exclusive: i64,
    apps: &[&str],
    models: Option<&[String]>,
    models_by_app: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<Vec<DailyUsageRow>, AppError> {
    if apps.is_empty() || end_ts_exclusive <= start_ts {
        return Ok(Vec::new());
    }

    let mut detail_conditions = vec![effective_usage_log_filter_for_range(
        conn,
        "l",
        Some(start_ts),
        Some(end_ts_exclusive - 1),
    )?];
    detail_conditions.push(app_model_filter("l", apps, models, models_by_app));
    let detail_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    let fresh_input = fresh_input_sql("l");
    let detail_sql = format!(
        "SELECT
            date(l.created_at, 'unixepoch', 'localtime') as bucket_date,
            COUNT(*) as request_count,
            COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as total_cost,
            COALESCE(SUM({fresh_input} + l.output_tokens), 0) as total_tokens,
            COALESCE(SUM({fresh_input}), 0) as total_input_tokens,
            COALESCE(SUM(l.output_tokens), 0) as total_output_tokens,
            COALESCE(SUM(l.cache_creation_tokens), 0) as total_cache_creation_tokens,
            COALESCE(SUM(l.cache_read_tokens), 0) as total_cache_read_tokens
         FROM proxy_request_logs l
         WHERE l.created_at >= ?1 AND l.created_at < ?2 AND {conditions}
         GROUP BY bucket_date
         ORDER BY bucket_date ASC",
        conditions = detail_conditions.join(" AND ")
    );

    let mut rows: Vec<DailyUsageRow> = Vec::new();
    {
        let mut statement = conn.prepare(&detail_sql)?;
        let mut detail_params = detail_params;
        detail_params.insert(0, Box::new(start_ts));
        detail_params.insert(1, Box::new(end_ts_exclusive));
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            detail_params.iter().map(|param| param.as_ref()).collect();
        let mapped = statement.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                DailyUsageRow {
                    date: NaiveDate::from_ymd_opt(1970, 1, 1).unwrap(),
                    request_count: row.get::<_, i64>(1)? as u64,
                    total_cost: format!("{:.6}", row.get::<_, f64>(2)?),
                    total_tokens: row.get::<_, i64>(3)? as u64,
                    total_input_tokens: row.get::<_, i64>(4)? as u64,
                    total_output_tokens: row.get::<_, i64>(5)? as u64,
                    total_cache_creation_tokens: row.get::<_, i64>(6)? as u64,
                    total_cache_read_tokens: row.get::<_, i64>(7)? as u64,
                },
            ))
        })?;
        for row in mapped {
            let (bucket_date, mut usage) = row?;
            usage.date = NaiveDate::parse_from_str(&bucket_date, "%Y-%m-%d")
                .map_err(|error| AppError::Database(format!("解析用量日期失败: {error}")))?;
            rows.push(usage);
        }
    }

    // The daily rollup: only the natural days that are covered completely by the range are counted, the same definition as the page.
    let rollup_bounds = compute_rollup_date_bounds(Some(start_ts), Some(end_ts_exclusive - 1))?;
    let mut rollup_conditions: Vec<String> = Vec::new();
    let mut rollup_params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
    let mut rollup_index = 1;
    if rollup_bounds.is_empty {
        rollup_conditions.push("1 = 0".to_string());
    } else {
        if let Some(start) = &rollup_bounds.start {
            rollup_conditions.push(format!("r.date >= ?{rollup_index}"));
            rollup_params.push(Box::new(start.clone()));
            rollup_index += 1;
        }
        if let Some(end) = &rollup_bounds.end {
            rollup_conditions.push(format!("r.date <= ?{rollup_index}"));
            rollup_params.push(Box::new(end.clone()));
        }
    }
    rollup_conditions.push(app_model_filter("r", apps, models, models_by_app));
    let rollup_where = format!("WHERE {}", rollup_conditions.join(" AND "));

    let fresh_input_rollup = fresh_input_sql("r");
    let rollup_sql = format!(
        "SELECT
            r.date,
            COALESCE(SUM(r.request_count), 0),
            COALESCE(SUM(CAST(r.total_cost_usd AS REAL)), 0),
            COALESCE(SUM({fresh_input_rollup} + r.output_tokens), 0),
            COALESCE(SUM({fresh_input_rollup}), 0),
            COALESCE(SUM(r.output_tokens), 0),
            COALESCE(SUM(r.cache_creation_tokens), 0),
            COALESCE(SUM(r.cache_read_tokens), 0)
         FROM usage_daily_rollups r
         {rollup_where}
         GROUP BY r.date
         ORDER BY r.date ASC"
    );
    {
        let mut statement = conn.prepare(&rollup_sql)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            rollup_params.iter().map(|param| param.as_ref()).collect();
        let mapped = statement.query_map(param_refs.as_slice(), |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)? as u64,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)? as u64,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, i64>(5)? as u64,
                    row.get::<_, i64>(6)? as u64,
                    row.get::<_, i64>(7)? as u64,
                ),
            ))
        })?;
        for row in mapped {
            let (bucket_date, (requests, cost, tokens, input, output, creation, read)) = row?;
            let date = NaiveDate::parse_from_str(&bucket_date, "%Y-%m-%d")
                .map_err(|error| AppError::Database(format!("解析日汇总日期失败: {error}")))?;
            rows.push(DailyUsageRow {
                date,
                request_count: requests,
                total_cost: format!("{cost:.6}"),
                total_tokens: tokens,
                total_input_tokens: input,
                total_output_tokens: output,
                total_cache_creation_tokens: creation,
                total_cache_read_tokens: read,
            });
        }
    }

    // The same date can have both detail records and a daily rollup (the history baseline overlaps
    // with the local collection),
    // they are added the same way as on the page instead of picking one of them.
    rows.sort_by_key(|row| row.date);
    Ok(rows)
}

/// Find out which of these local dates have a matching daily rollup row.
///
/// It is used for the precision check of a second-level range: when the range cuts a day in
/// half, the daily rollup of that day (day granularity)
/// cannot be split proportionally and the day has to be counted or excluded as a whole. The
/// caller answers with a 422 instead of returning
/// a silently truncated number that is too low.
///
/// The filter conditions have to be identical to the ones of the actual fetch (the same app
/// and model semantics), otherwise a non-matching archived
/// history would wrongly block the query.
pub(crate) fn rollup_dates(
    conn: &Connection,
    dates: &[NaiveDate],
    apps: &[&str],
    models: Option<&[String]>,
    models_by_app: Option<&BTreeMap<String, Vec<String>>>,
) -> Result<Vec<String>, AppError> {
    if dates.is_empty() || apps.is_empty() {
        return Ok(Vec::new());
    }
    let date_list = dates
        .iter()
        .map(|date| format!("'{}'", date.format("%Y-%m-%d")))
        .collect::<Vec<_>>()
        .join(", ");

    let conditions = [
        format!("r.date IN ({date_list})"),
        app_model_filter("r", apps, models, models_by_app),
    ];
    let where_clause = format!("WHERE {}", conditions.join(" AND "));
    let sql = format!(
        "SELECT DISTINCT r.date FROM usage_daily_rollups r {where_clause} ORDER BY r.date ASC"
    );
    let mut statement = conn.prepare(&sql)?;
    let mapped = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut dates = Vec::new();
    for row in mapped {
        dates.push(row?);
    }
    Ok(dates)
}

pub(crate) fn find_model_pricing(conn: &Connection, model_id: &str) -> Option<ModelPricing> {
    find_model_pricing_row(conn, model_id)
        .ok()
        .flatten()
        .and_then(|(input, output, cache_read, cache_creation)| {
            ModelPricing::from_strings(&input, &output, &cache_read, &cache_creation).ok()
        })
}

pub(crate) fn find_model_pricing_row(
    conn: &Connection,
    model_id: &str,
) -> Result<Option<(String, String, String, String)>, AppError> {
    let candidates = model_pricing_candidates(model_id);
    if candidates.is_empty() {
        return Ok(None);
    }

    for candidate in &candidates {
        if let Some(row) = query_model_pricing_exact(conn, candidate)? {
            return Ok(Some(row));
        }
    }

    for candidate in &candidates {
        if should_try_pricing_prefix_match(candidate) {
            if let Some(row) = query_model_pricing_prefix(conn, candidate)? {
                return Ok(Some(row));
            }
        }
    }

    Ok(None)
}

/// The row filter of the precise backfill: it counts as related when any model field of the
/// log normalizes to an intersection with the candidates of the target model
/// or when it hits the target through the prefix rules of the price lookup layer. It mirrors
/// the matching semantics of find_model_pricing_row and prefers a false positive (the price
// lookup catches it later) over a missed filter.
fn log_pricing_scope_matches(log: &RequestLogDetail, target_candidates: &[String]) -> bool {
    [
        Some(log.model.as_str()),
        log.request_model.as_deref(),
        log.pricing_model.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| {
        model_pricing_candidates(field).iter().any(|candidate| {
            target_candidates.iter().any(|target| {
                target == candidate
                    || (should_try_pricing_prefix_match(candidate)
                        && target
                            .strip_prefix(candidate.as_str())
                            .is_some_and(|rest| rest.starts_with('-')))
            })
        })
    })
}

pub(crate) fn is_placeholder_pricing_model(model_id: &str) -> bool {
    let normalized = model_id.trim().to_ascii_lowercase();
    normalized.is_empty() || matches!(normalized.as_str(), "unknown" | "null" | "none")
}

fn query_model_pricing_exact(
    conn: &Connection,
    model_id: &str,
) -> Result<Option<(String, String, String, String)>, AppError> {
    conn.query_row(
        "SELECT input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
         FROM model_pricing
         WHERE model_id = ?1 COLLATE NOCASE",
        [model_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .map_err(|e| AppError::Database(format!("查询模型定价失败: {e}")))
}

fn query_model_pricing_prefix(
    conn: &Connection,
    model_id: &str,
) -> Result<Option<(String, String, String, String)>, AppError> {
    let pattern = format!("{model_id}-%");
    conn.query_row(
        "SELECT input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
         FROM model_pricing
         WHERE model_id LIKE ?1
         ORDER BY LENGTH(model_id) ASC
         LIMIT 1",
        [pattern],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )
    .optional()
    .map_err(|e| AppError::Database(format!("查询模型前缀定价失败: {e}")))
}

fn model_pricing_candidates(model_id: &str) -> Vec<String> {
    let cleaned = clean_model_id_for_pricing(model_id);
    if is_placeholder_pricing_model(&cleaned) {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    let mut queue = vec![cleaned];

    while let Some(candidate) = queue.pop() {
        if !push_unique_candidate(&mut candidates, candidate.clone()) {
            continue;
        }

        if let Some(stripped) = strip_known_model_namespace(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_claude_desktop_non_anthropic_prefix(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_bedrock_model_version_suffix(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_model_date_suffix(&candidate) {
            queue.push(stripped);
        }
        if let Some(stripped) = strip_reasoning_effort_suffix(&candidate) {
            queue.push(stripped);
        }
        if candidate.starts_with("claude-") && candidate.contains('.') {
            queue.push(candidate.replace('.', "-"));
        }
    }

    candidates
}

fn clean_model_id_for_pricing(model_id: &str) -> String {
    let normalized = model_id
        .rsplit_once('/')
        .map_or(model_id, |(_, r)| r)
        .split(':')
        .next()
        .unwrap_or(model_id)
        .trim()
        .replace('@', "-")
        .to_ascii_lowercase();

    normalized
        .trim_end_matches(crate::claude_desktop_config::ONE_M_CONTEXT_MARKER)
        .trim()
        .to_string()
}

fn push_unique_candidate(candidates: &mut Vec<String>, candidate: String) -> bool {
    if candidate.is_empty() || candidates.iter().any(|existing| existing == &candidate) {
        return false;
    }
    candidates.push(candidate);
    true
}

fn strip_known_model_namespace(model_id: &str) -> Option<String> {
    if let Some(pos) = model_id.rfind("claude-") {
        if pos > 0 {
            return Some(model_id[pos..].to_string());
        }
    }

    for marker in [
        "openai.",
        "anthropic.",
        "google.",
        "moonshot.",
        "moonshotai.",
        "bedrock.",
        "global.",
    ] {
        if let Some(stripped) = model_id.strip_prefix(marker) {
            return Some(stripped.to_string());
        }
    }

    None
}

fn strip_claude_desktop_non_anthropic_prefix(model_id: &str) -> Option<String> {
    const NON_ANTHROPIC_MARKERS: &[&str] = &[
        "abab",
        "ark-code",
        "arctic",
        "astron",
        "codex",
        "command-r",
        "deepseek",
        "doubao",
        "ernie",
        "gemini",
        "gemma",
        "glm",
        "gpt",
        "grok",
        "hermes",
        "hy3",
        "hunyuan",
        "jamba",
        "kimi",
        "lfm",
        "llama",
        "longcat",
        "mercury",
        "mimo",
        "minimax",
        "mistral",
        "mixtral",
        "moonshot",
        "nemotron",
        "nova-",
        "openai",
        "qianfan",
        "qwen",
        "seed-",
        "solar",
        "stepfun",
    ];

    let rest = model_id.strip_prefix("claude-")?;
    NON_ANTHROPIC_MARKERS
        .iter()
        .any(|marker| rest.starts_with(marker))
        .then(|| rest.to_string())
}

fn strip_bedrock_model_version_suffix(model_id: &str) -> Option<String> {
    let (base, suffix) = model_id.rsplit_once("-v")?;
    (!base.is_empty() && !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit()))
        .then(|| base.to_string())
}

fn strip_model_date_suffix(model_id: &str) -> Option<String> {
    let bytes = model_id.as_bytes();
    if bytes.len() > 11 {
        let start = bytes.len() - 11;
        let suffix = &bytes[start..];
        let is_iso_date = suffix[0] == b'-'
            && suffix[1..5].iter().all(|b| b.is_ascii_digit())
            && suffix[5] == b'-'
            && suffix[6..8].iter().all(|b| b.is_ascii_digit())
            && suffix[8] == b'-'
            && suffix[9..11].iter().all(|b| b.is_ascii_digit());
        if is_iso_date {
            return Some(model_id[..start].to_string());
        }
    }

    let (base, suffix) = model_id.rsplit_once('-')?;
    if base.is_empty() || !suffix.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    // The 8 digit YYYYMMDD (for example -20250615; OpenAI / Claude / Tongyi Qianwen and the like).
    if suffix.len() == 8 {
        return Some(base.to_string());
    }
    // The 6 digit YYMMDD (for example -260628; the Volcengine doubao-seed-* and some of the
    // domestic vendors).
    // 6 digits hit a non-date tail (a version number like -123456) more easily than 8 digits, so
    // the month 01-12 and the day 01-31 are validated as well before
    // it is stripped; when it cannot be stripped it falls back to None and the layer above
    // matches it exactly.
    if suffix.len() == 6 {
        let month: u32 = suffix[2..4].parse().unwrap_or(0);
        let day: u32 = suffix[4..6].parse().unwrap_or(0);
        if (1..=12).contains(&month) && (1..=31).contains(&day) {
            return Some(base.to_string());
        }
    }
    None
}

fn strip_reasoning_effort_suffix(model_id: &str) -> Option<String> {
    for suffix in ["-minimal", "-low", "-medium", "-high", "-xhigh"] {
        if let Some(stripped) = model_id.strip_suffix(suffix) {
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

fn should_try_pricing_prefix_match(model_id: &str) -> bool {
    let dash_count = model_id.matches('-').count();

    if model_id.starts_with("claude-") {
        return dash_count >= 3;
    }

    if ["o1", "o3", "o4", "o5"]
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
    {
        return dash_count >= 1;
    }

    const PREFIX_MATCH_FAMILIES: &[&str] = &[
        "gpt-",
        "gemini-",
        "deepseek-",
        "qwen-",
        "glm-",
        "kimi-",
        "minimax-",
    ];

    PREFIX_MATCH_FAMILIES
        .iter()
        .any(|prefix| model_id.starts_with(prefix))
        && dash_count >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_ts(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> i64 {
        match Local.with_ymd_and_hms(year, month, day, hour, minute, second) {
            chrono::LocalResult::Single(dt) => dt.timestamp(),
            chrono::LocalResult::Ambiguous(earliest, _) => earliest.timestamp(),
            chrono::LocalResult::None => panic!("valid local datetime"),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_usage_log(
        conn: &Connection,
        request_id: &str,
        app_type: &str,
        provider_id: &str,
        model: &str,
        data_source: &str,
        created_at: i64,
        input_tokens: i64,
        output_tokens: i64,
        cache_read_tokens: i64,
        cache_creation_tokens: i64,
        status_code: i64,
        total_cost_usd: &str,
    ) -> Result<(), AppError> {
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, provider_id, app_type, model, request_model,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                total_cost_usd, latency_ms, status_code, created_at, data_source
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, '0', '0', '0', '0', ?, 100, ?, ?, ?)",
            params![
                request_id,
                provider_id,
                app_type,
                model,
                model,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_creation_tokens,
                total_cost_usd,
                status_code,
                created_at,
                data_source
            ],
        )?;
        Ok(())
    }

    fn create_legacy_nullable_logs_table(conn: &Connection) -> Result<(), AppError> {
        conn.execute(
            "CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                app_type TEXT NOT NULL,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read_tokens INTEGER NOT NULL,
                cache_creation_tokens INTEGER NOT NULL,
                status_code INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                data_source TEXT,
                input_token_semantics INTEGER NOT NULL DEFAULT 0
            )",
            [],
        )?;
        Ok(())
    }

    #[test]
    fn test_effective_filter_keeps_legacy_null_data_source_proxy_rows() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        create_legacy_nullable_logs_table(&conn)?;
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, status_code, created_at, data_source
            ) VALUES ('legacy-proxy', 'codex', 'gpt-5.5', 10, 2, 1, 0, 200, 1000, NULL)",
            [],
        )?;

        let filter = effective_usage_log_filter("l");
        let sql = format!("SELECT COUNT(*) FROM proxy_request_logs l WHERE {filter}");
        let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
        assert_eq!(count, 1);

        Ok(())
    }

    /// The cache of the total number of logs: it is reused when there was no write and it is recounted after an insert, a delete or when the end time was pushed forward and there are new rows.
    #[test]
    fn test_request_log_total_cache_invalidates_on_writes() -> Result<(), AppError> {
        let db = Database::memory()?;
        let insert = |id: &str, created_at: i64| -> Result<(), AppError> {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                id,
                "claude",
                "anthropic",
                "claude-sonnet-4-5",
                "proxy",
                created_at,
                10,
                2,
                0,
                0,
                200,
                "0.01",
            )
        };
        insert("a", 1_000)?;
        insert("b", 2_000)?;

        let mut filters = LogFilters {
            start_date: Some(0),
            end_date: Some(5_000),
            ..LogFilters::default()
        };
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 2);
        // No write: the cache is reused and the result does not change
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 2);

        // After an insert it has to be recounted
        insert("c", 3_000)?;
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 3);

        // After a delete it also has to be recounted
        {
            let conn = lock_conn!(db.conn);
            conn.execute("DELETE FROM proxy_request_logs WHERE request_id = 'a'", [])?;
        }
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 2);

        // The end time was pushed forward: the cache is reused when the new range holds no rows and recounted when there are rows (rows that existed before with a future time)
        filters.end_date = Some(6_000);
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 2);
        insert("d", 9_000)?;
        filters.end_date = Some(5_000);
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 2);
        filters.end_date = Some(10_000);
        assert_eq!(db.get_request_logs(&filters, 0, 10)?.total, 3);

        Ok(())
    }

    /// The range version of the deduplication condition has to give the same result as the original row by row EXISTS formulation, no matter which variant is chosen.
    #[test]
    fn test_range_filter_matches_original_dedup() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        create_legacy_nullable_logs_table(&conn)?;
        // request_id, app_type, model, input, output, cache_read, cache_creation,
        // status_code, created_at, data_source
        type LogRow = (
            &'static str,
            &'static str,
            &'static str,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            Option<&'static str>,
        );
        let rows: &[LogRow] = &[
            // A duplicate of proxy-1 (it should be removed)
            (
                "p1",
                "claude",
                "opus",
                10,
                2,
                1,
                5,
                200,
                1000,
                Some("proxy"),
            ),
            (
                "s1",
                "claude",
                "opus",
                10,
                2,
                1,
                5,
                200,
                1100,
                Some("session_log"),
            ),
            // The proxy row of claude-desktop can also remove a claude session row
            (
                "p2",
                "claude-desktop",
                "sonnet",
                20,
                3,
                0,
                0,
                200,
                5000,
                Some("proxy"),
            ),
            (
                "s2",
                "claude",
                "SONNET",
                20,
                3,
                0,
                0,
                200,
                5300,
                Some("session_log"),
            ),
            // A codex session without cache_creation: 0 counts as unknown
            ("p3", "codex", "gpt", 30, 4, 2, 9, 200, 9000, Some("proxy")),
            (
                "s3",
                "codex",
                "gpt",
                30,
                4,
                2,
                0,
                200,
                9100,
                Some("codex_session"),
            ),
            // Outside the 10 minute window: it is kept
            (
                "s4",
                "claude",
                "opus",
                10,
                2,
                1,
                5,
                200,
                1000 + 601,
                Some("session_log"),
            ),
            // A failed proxy row does not take part in the deduplication, the session row is kept
            (
                "p5",
                "claude",
                "haiku",
                7,
                7,
                7,
                7,
                500,
                20000,
                Some("proxy"),
            ),
            (
                "s5",
                "claude",
                "haiku",
                7,
                7,
                7,
                7,
                200,
                20010,
                Some("session_log"),
            ),
            // The model name unknown also counts as a match
            ("p6", "claude", "unknown", 8, 8, 8, 8, 200, 30000, None),
            (
                "s6",
                "claude",
                "opus",
                8,
                8,
                8,
                8,
                200,
                30010,
                Some("session_log"),
            ),
            // An unrelated session row, so that there are more session rows than proxy rows
            (
                "s7",
                "claude",
                "opus",
                1,
                1,
                1,
                1,
                200,
                40000,
                Some("session_log"),
            ),
            (
                "s8",
                "claude",
                "opus",
                2,
                2,
                2,
                2,
                200,
                40001,
                Some("session_log"),
            ),
            (
                "s9",
                "claude",
                "opus",
                3,
                3,
                3,
                3,
                200,
                40002,
                Some("session_log"),
            ),
        ];
        for r in rows {
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, app_type, model, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, status_code, created_at, data_source
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![r.0, r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9],
            )?;
        }

        let ids = |filter: &str, start: i64, end: i64| -> Result<Vec<String>, AppError> {
            let sql = format!(
                "SELECT request_id FROM proxy_request_logs l
                 WHERE l.created_at BETWEEN ?1 AND ?2 AND {filter}
                 ORDER BY request_id"
            );
            let mut stmt = conn.prepare(&sql)?;
            let rows = stmt
                .query_map(params![start, end], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        };

        let original = effective_usage_log_filter("l");
        // The full set (fewer proxies than sessions -> NOT IN), only proxy rows, only session rows (skipped directly)
        for (start, end) in [(0, 50000), (900, 1050), (40000, 40002), (1100, 9100)] {
            let ranged = effective_usage_log_filter_for_range(&conn, "l", Some(start), Some(end))?;
            assert_eq!(
                ids(&ranged, start, end)?,
                ids(&original, start, end)?,
                "range {start}..{end}"
            );
        }
        let all = effective_usage_log_filter_for_range(&conn, "l", None, None)?;
        assert!(all.contains("NOT IN"));
        assert_eq!(ids(&all, 0, 50000)?, ids(&original, 0, 50000)?);
        assert_eq!(
            ids(&all, 0, 50000)?,
            vec!["p1", "p2", "p3", "p5", "p6", "s4", "s5", "s7", "s8", "s9"]
        );

        Ok(())
    }

    #[test]
    fn test_matching_proxy_log_treats_legacy_null_data_source_as_proxy() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        create_legacy_nullable_logs_table(&conn)?;
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, status_code, created_at, data_source
            ) VALUES ('legacy-proxy', 'codex', 'gpt-5.5', 10, 2, 1, 0, 200, 1000, NULL)",
            [],
        )?;

        let key = DedupKey {
            app_type: "codex",
            model: "gpt-5.5",
            input_tokens: 10,
            output_tokens: 2,
            cache_read_tokens: 1,
            cache_creation_tokens: 0,
            created_at: 1000,
        };
        assert!(has_matching_proxy_usage_log(&conn, &key)?);

        Ok(())
    }

    #[test]
    fn test_matching_proxy_log_matches_claude_desktop_for_claude_session() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        create_legacy_nullable_logs_table(&conn)?;
        conn.execute(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, status_code, created_at, data_source
            ) VALUES ('desktop-proxy', 'claude-desktop', 'claude-sonnet-4-5', 100, 20, 10, 5, 200, 1000, 'proxy')",
            [],
        )?;

        let key = DedupKey {
            app_type: "claude",
            model: "claude-sonnet-4-5",
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            created_at: 1060,
        };
        assert!(has_matching_proxy_usage_log(&conn, &key)?);

        let mut outside_window = key;
        outside_window.created_at = 1_601;
        assert!(!has_matching_proxy_usage_log(&conn, &outside_window)?);

        let mut different_model = key;
        different_model.model = "claude-opus-4-5";
        assert!(!has_matching_proxy_usage_log(&conn, &different_model)?);

        let mut different_input = key;
        different_input.input_tokens += 1;
        assert!(!has_matching_proxy_usage_log(&conn, &different_input)?);

        let mut different_cache_creation = key;
        different_cache_creation.cache_creation_tokens += 1;
        assert!(!has_matching_proxy_usage_log(
            &conn,
            &different_cache_creation
        )?);

        Ok(())
    }

    #[test]
    fn test_effective_filter_dedups_claude_session_against_desktop_proxy() -> Result<(), AppError> {
        let conn = Connection::open_in_memory()?;
        create_legacy_nullable_logs_table(&conn)?;
        conn.execute_batch(
            "INSERT INTO proxy_request_logs (
                request_id, app_type, model, input_tokens, output_tokens,
                cache_read_tokens, cache_creation_tokens, status_code, created_at, data_source
            ) VALUES
                ('desktop-proxy', 'claude-desktop', 'claude-sonnet-4-5', 100, 20, 10, 5, 200, 1000, 'proxy'),
                ('claude-session', 'claude', 'claude-sonnet-4-5', 100, 20, 10, 5, 200, 1060, 'session_log');",
        )?;

        let filter = effective_usage_log_filter("l");
        let sql = format!("SELECT request_id FROM proxy_request_logs l WHERE {filter}");
        let request_ids = conn
            .prepare(&sql)?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(request_ids, vec!["desktop-proxy"]);

        Ok(())
    }

    #[test]
    fn test_claude_desktop_folds_into_claude_for_display() -> Result<(), AppError> {
        let db = Database::memory()?;
        let ts = local_ts(2026, 6, 10, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            // One Claude Code row plus one Claude Desktop gateway row, in the same time window.
            insert_usage_log(
                &conn,
                "cc-1",
                "claude",
                "p-claude",
                "claude-sonnet-4-5",
                "proxy",
                ts,
                100,
                10,
                0,
                0,
                200,
                "0.5",
            )?;
            insert_usage_log(
                &conn,
                "cd-1",
                "claude-desktop",
                "p-desktop",
                "claude-opus-4-8",
                "proxy",
                ts,
                200,
                20,
                0,
                0,
                200,
                "1.5",
            )?;
        }

        // ① The aggregate per app: desktop is folded into claude, there is no separate claude-desktop bucket any more.
        let by_app = db.get_usage_summary_by_app(None, None, None, None)?;
        assert_eq!(by_app.len(), 1, "应只剩一个合并后的 claude 桶");
        assert_eq!(by_app[0].app_type, "claude");
        assert_eq!(by_app[0].summary.total_requests, 2, "两条行都计入 claude");
        assert!(
            !by_app.iter().any(|a| a.app_type == "claude-desktop"),
            "不应再出现 claude-desktop 桶"
        );

        // ② Selecting claude in the filter: the aggregate has to cover the desktop rows as well.
        let claude_summary = db.get_usage_summary(None, None, Some("claude"), None, None)?;
        assert_eq!(claude_summary.total_requests, 2);

        // ③ The request logs filtered by claude return two rows and the projection of the desktop row is still the original app_type.
        let logs = db.get_request_logs(
            &LogFilters {
                app_type: Some("claude".to_string()),
                ..Default::default()
            },
            0, // The page number starts at 0
            50,
        )?;
        assert_eq!(logs.total, 2, "claude 过滤含 desktop 行");
        assert!(
            logs.data.iter().any(|r| r.app_type == "claude-desktop"),
            "详情面板需要看到真实入口，行投影不可被折叠"
        );

        // ④ The fold does not leak: the codex filter is empty.
        let codex_summary = db.get_usage_summary(None, None, Some("codex"), None, None)?;
        assert_eq!(codex_summary.total_requests, 0);

        Ok(())
    }

    #[test]
    fn test_backfill_missing_usage_costs_uses_new_gpt_5_5_pricing() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "codex-gpt-5-5-zero-cost",
                "codex",
                "_codex_session",
                "gpt-5.5",
                "codex_session",
                1000,
                1_000_000,
                1_000_000,
                0,
                0,
                200,
                "0",
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, output_cost, total_cost): (String, String, String) = conn.query_row(
            "SELECT input_cost_usd, output_cost_usd, total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'codex-gpt-5-5-zero-cost'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(input_cost, "5.000000");
        assert_eq!(output_cost, "30.000000");
        assert_eq!(total_cost, "35.000000");

        Ok(())
    }

    #[test]
    fn test_backfill_new_anthropic_openai_pricing_after_upgrade() -> Result<(), AppError> {
        let db = Database::memory()?;
        let cases = [
            (
                "anthropic/claude-opus-5.5",
                "claude",
                1_000_000,
                ["4.000000", "20.000000", "0.200000", "5.000000", "29.200000"],
            ),
            (
                "OpenAI/GPT-6-SOL@HIGH",
                "codex",
                3_000_000,
                ["2.000000", "10.000000", "0.200000", "2.500000", "14.700000"],
            ),
            (
                "OpenAI/GPT-6.1-SOL@HIGH",
                "codex",
                3_000_000,
                ["2.000000", "10.000000", "0.100000", "2.500000", "14.600000"],
            ),
            (
                "gpt-6-luna",
                "codex",
                3_000_000,
                ["0.100000", "0.500000", "0.010000", "0.125000", "0.735000"],
            ),
            (
                "gpt-5.6-cyber",
                "codex",
                3_000_000,
                [
                    "12.500000",
                    "75.000000",
                    "1.250000",
                    "15.625000",
                    "104.375000",
                ],
            ),
            // The Pro series has no cache discount, the cache column is 0
            (
                "gpt-5.5-pro",
                "codex",
                3_000_000,
                [
                    "30.000000",
                    "180.000000",
                    "0.000000",
                    "0.000000",
                    "210.000000",
                ],
            ),
            // After stripping the date suffix it hits gpt-4o-mini exactly and does not fall through to the shorter gpt-4o
            (
                "gpt-4o-mini-2024-07-18",
                "codex",
                3_000_000,
                ["0.150000", "0.600000", "0.075000", "0.000000", "0.825000"],
            ),
        ];
        {
            let conn = lock_conn!(db.conn);
            // Simulate an existing database with unpriced usage before the update.
            conn.execute(
                "DELETE FROM model_pricing WHERE model_id IN
                 ('claude-opus-5-5', 'gpt-6-sol', 'gpt-6.1-sol', 'gpt-6-luna', 'gpt-5.6-cyber',
                  'gpt-5.5-pro', 'gpt-4o-mini')",
                [],
            )?;
            for (model, app, input, _) in &cases {
                insert_usage_log(
                    &conn, model, app, "p1", model, "proxy", 1000, *input, 1_000_000, 1_000_000,
                    1_000_000, 200, "0",
                )?;
            }
            conn.execute(
                "UPDATE proxy_request_logs SET input_token_semantics = ?1",
                [INPUT_TOKEN_SEMANTICS_TOTAL],
            )?;
        }
        assert_eq!(db.backfill_missing_usage_costs()?, 0);
        db.ensure_model_pricing_seeded()?;
        assert_eq!(db.backfill_missing_usage_costs()?, cases.len() as u64);

        let conn = lock_conn!(db.conn);
        for (model, _, _, expected) in cases {
            let costs: [String; 5] = conn.query_row(
                "SELECT input_cost_usd, output_cost_usd, cache_read_cost_usd,
                        cache_creation_cost_usd, total_cost_usd
                 FROM proxy_request_logs WHERE request_id = ?1",
                [model],
                |row| {
                    Ok([
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ])
                },
            )?;
            assert_eq!(costs, expected, "{model}");
        }
        Ok(())
    }

    #[test]
    fn test_backfill_distinguishes_legacy_and_total_cache_semantics() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // v12 mirror row: input = fresh + read; creation was reported separately.
            insert_usage_log(
                &conn,
                "legacy-cache-semantics",
                "codex",
                "p1",
                "gpt-5.5",
                "proxy",
                1000,
                800_000,
                0,
                600_000,
                200_000,
                200,
                "0",
            )?;
            // v13 proxy row: input = fresh + read + creation.
            insert_usage_log(
                &conn,
                "total-cache-semantics",
                "codex",
                "p1",
                "gpt-5.5",
                "proxy",
                1001,
                1_000_000,
                0,
                600_000,
                200_000,
                200,
                "0",
            )?;
            conn.execute(
                "UPDATE proxy_request_logs
                 SET input_token_semantics = ?1
                 WHERE request_id = 'total-cache-semantics'",
                [INPUT_TOKEN_SEMANTICS_TOTAL],
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 2);

        let conn = lock_conn!(db.conn);
        let mut stmt = conn.prepare(
            "SELECT request_id, input_cost_usd
             FROM proxy_request_logs
             WHERE request_id IN ('legacy-cache-semantics', 'total-cache-semantics')
             ORDER BY request_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            rows,
            vec![
                ("legacy-cache-semantics".to_string(), "1.000000".to_string()),
                ("total-cache-semantics".to_string(), "1.000000".to_string()),
            ]
        );

        Ok(())
    }

    #[test]
    fn test_backfill_deducts_cache_read_for_grokbuild_total_rows() -> Result<(), AppError> {
        // Regression: the cache-inclusive decision of the backfill side once hardcoded codex|gemini and
        // missed
        // grokbuild, so the TOTAL rows were priced with the full input and the cache_read was double
        // counted.
        // After the decision was converged to sql_helpers::is_cache_inclusive_app they are priced with
        // 450 fresh.
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "grokbuild-total-backfill",
                "grokbuild",
                "_grok_session",
                "grok-4.5",
                "grok_session",
                1000,
                700,
                100,
                250,
                0,
                200,
                "0",
            )?;
            conn.execute(
                "UPDATE proxy_request_logs
                 SET input_token_semantics = ?1
                 WHERE request_id = 'grokbuild-total-backfill'",
                [INPUT_TOKEN_SEMANTICS_TOTAL],
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, cache_read_cost, total_cost): (String, String, String) = conn.query_row(
            "SELECT input_cost_usd, cache_read_cost_usd, total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'grokbuild-total-backfill'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        // The grok-4.5 pricing 2/6/0.30: the input = (700-250)x2/1M, the cache_read = 250x0.3/1M
        assert_eq!(input_cost, "0.000900");
        assert_eq!(cache_read_cost, "0.000075");
        assert_eq!(total_cost, "0.001575");
        Ok(())
    }

    #[test]
    fn test_backfill_missing_usage_costs_uses_stored_multiplier() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "codex-gpt-5-5-multiplier",
                "codex",
                "_codex_session",
                "gpt-5.5",
                "codex_session",
                1000,
                1_000_000,
                0,
                0,
                0,
                200,
                "0",
            )?;
            conn.execute(
                "UPDATE proxy_request_logs
                 SET cost_multiplier = '1.5'
                 WHERE request_id = 'codex-gpt-5-5-multiplier'",
                [],
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, total_cost): (String, String) = conn.query_row(
            "SELECT input_cost_usd, total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'codex-gpt-5-5-multiplier'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(input_cost, "5.000000");
        assert_eq!(total_cost, "7.500000");

        Ok(())
    }

    #[test]
    fn test_backfill_missing_usage_costs_falls_back_to_request_model() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (
                    'codex-request-model-fallback', '_codex_session', 'codex', 'unknown', 'gpt-5.5',
                    1000000, 0, 0, 0,
                    '0', '0', '0', '0',
                    '0', 100, 200, 1000, 'codex_session'
                )",
                [],
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let total_cost: String = conn.query_row(
            "SELECT total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'codex-request-model-fallback'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total_cost, "5.000000");

        Ok(())
    }

    #[test]
    fn test_backfill_skips_request_model_fallback_for_real_unpriced_model() -> Result<(), AppError>
    {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // The router takeover scenario: the model is the real model echoed by the upstream (the
            // pricing is missing) and the request_model
            // is the client alias (it has a pricing). The backfill must not price with the alias and
            // has to keep the cost at 0 and wait for a price.
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (
                    'takeover-unpriced-model', 'provider-1', 'claude',
                    'takeover-real-model-unpriced', 'claude-sonnet-4-6',
                    1000000, 0, 0, 0,
                    '0', '0', '0', '0',
                    '0', 100, 200, 1000, 'proxy'
                )",
                [],
            )?;
        }

        // The request_model (claude-sonnet-4-6) has a pricing, but the model is a real model name: there must be no fallback
        assert_eq!(db.backfill_missing_usage_costs()?, 0);

        {
            let conn = lock_conn!(db.conn);
            let total_cost: String = conn.query_row(
                "SELECT total_cost_usd
                 FROM proxy_request_logs WHERE request_id = 'takeover-unpriced-model'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(total_cost, "0");

            // After the pricing of the real model was added, the backfill has to repair it with the price of the real model (the zero-cost row was not polluted and frozen)
            conn.execute(
                "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million, output_cost_per_million)
                 VALUES ('takeover-real-model-unpriced', 'Takeover Real Model', '0.6', '2.5')",
                [],
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let total_cost: String = conn.query_row(
            "SELECT total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'takeover-unpriced-model'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total_cost, "0.600000");

        Ok(())
    }

    #[test]
    fn test_backfill_uses_persisted_pricing_model() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // The request pricing mode + a takeover: at write time the outbound model kimi-k2-novel was
            // anchored (it had no price at that time),
            // but the upstream echoed an alias -> the model/request_model are both claude-sonnet-4-6 (it has
            // a pricing).
            // The backfill has to recompute with the stored pricing_model and must not switch to the alias
            // price of the model column.
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model, pricing_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (
                    'persisted-pricing-model', 'provider-1', 'claude',
                    'claude-sonnet-4-6', 'claude-sonnet-4-6', 'kimi-k2-novel',
                    1000000, 0, 0, 0,
                    '0', '0', '0', '0',
                    '0', 100, 200, 1000, 'proxy'
                )",
                [],
            )?;
        }

        // The pricing_model (kimi-k2-novel) has no price: there must be no fallback to the alias price of the model column
        assert_eq!(db.backfill_missing_usage_costs()?, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million, output_cost_per_million)
                 VALUES ('kimi-k2-novel', 'Kimi K2 Novel', '0.6', '2.5')",
                [],
            )?;
        }

        // The pricing_model locates the row as well (neither the model nor the request_model is kimi-k2-novel)
        assert_eq!(
            db.backfill_missing_usage_costs_for_model("kimi-k2-novel")?,
            1
        );

        let conn = lock_conn!(db.conn);
        let total_cost: String = conn.query_row(
            "SELECT total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'persisted-pricing-model'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total_cost, "0.600000");

        Ok(())
    }

    #[test]
    fn test_scoped_backfill_matches_raw_alias_rows() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // The proxy logs are stored with the raw text of the upstream: the alias form with the router
            // prefix and the :free suffix.
            // The filter of the precise backfill has to match after the normalization, otherwise those
            // rows are only updated at the next full backfill.
            insert_usage_log(
                &conn,
                "openrouter-alias-zero-cost",
                "claude",
                "provider-1",
                "openrouter/moonshot/kimi-k2-novel:free",
                "proxy",
                1000,
                1_000_000,
                0,
                0,
                0,
                200,
                "0",
            )?;
        }

        // Nothing is backfilled when the pricing is missing
        assert_eq!(db.backfill_missing_usage_costs()?, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO model_pricing (model_id, display_name, input_cost_per_million, output_cost_per_million)
                 VALUES ('kimi-k2-novel', 'Kimi K2 Novel', '0.6', '2.5')",
                [],
            )?;
        }

        // The precise backfill by the normalized ID has to hit the rows that were stored under their raw alias
        assert_eq!(
            db.backfill_missing_usage_costs_for_model("kimi-k2-novel")?,
            1
        );

        let conn = lock_conn!(db.conn);
        let total_cost: String = conn.query_row(
            "SELECT total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'openrouter-alias-zero-cost'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(total_cost, "0.600000");

        Ok(())
    }

    #[test]
    fn test_backfill_missing_usage_costs_keeps_claude_fresh_input() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "claude-cache-fresh-input",
                "claude",
                "_session",
                "claude-haiku-4-5",
                "session_log",
                1000,
                100,
                0,
                200,
                0,
                200,
                "0",
            )?;
        }

        assert_eq!(db.backfill_missing_usage_costs()?, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, cache_read_cost, total_cost): (String, String, String) = conn.query_row(
            "SELECT input_cost_usd, cache_read_cost_usd, total_cost_usd
             FROM proxy_request_logs WHERE request_id = 'claude-cache-fresh-input'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(input_cost, "0.000100");
        assert_eq!(cache_read_cost, "0.000020");
        assert_eq!(total_cost, "0.000120");

        Ok(())
    }

    #[test]
    fn test_get_usage_summary() -> Result<(), AppError> {
        let db = Database::memory()?;

        // Insert the test data
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params!["req1", "p1", "claude", "claude-3", 100, 50, "0.01", 100, 200, 1000],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params!["req2", "p1", "claude", "claude-3", 200, 100, "0.02", 150, 200, 2000],
            )?;
        }

        let summary = db.get_usage_summary(None, None, None, None, None)?;
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.success_rate, 100.0);

        Ok(())
    }

    #[test]
    fn test_get_session_usage_summary_counts_only_session_log_rows() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // (request_id, app_type, session_id, data_source, input, output, cache_read, cost)
            let rows = [
                (
                    "s1",
                    "claude",
                    "sess-a",
                    "session_log",
                    10,
                    200,
                    5000,
                    "0.10",
                ),
                (
                    "s2",
                    "claude-desktop",
                    "sess-a",
                    "session_log",
                    20,
                    300,
                    7000,
                    "0.20",
                ),
                // The proxy rows are not counted even when the session ID is the same, that avoids a duplicate with the session logs
                ("p1", "claude", "sess-a", "proxy", 10, 200, 5000, "0.10"),
                ("s3", "claude", "sess-b", "session_log", 1, 1, 1, "9.00"),
                ("s4", "codex", "sess-a", "codex_session", 1, 1, 0, "9.00"),
            ];
            for (id, app, session, source, input, output, cache_read, cost) in rows {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, cache_read_tokens, total_cost_usd,
                        latency_ms, status_code, created_at, session_id, data_source
                    ) VALUES (?, 'p1', ?, 'claude-x', ?, ?, ?, ?, 0, 200, 1000, ?, ?)",
                    params![id, app, input, output, cache_read, cost, session, source],
                )?;
            }
        }

        let summary = db.get_session_usage_summary("claude", "sess-a")?;
        assert_eq!(summary.total_requests, 2);
        assert_eq!(summary.real_total_tokens, 10 + 200 + 5000 + 20 + 300 + 7000);
        assert_eq!(summary.total_cost, "0.300000");

        let empty = db.get_session_usage_summary("claude", "missing")?;
        assert_eq!(empty.total_requests, 0);

        Ok(())
    }

    /// The earliest date: the detail records and the daily aggregates take the earlier of the two, the filters have the same definition as in the Dashboard (claude includes claude-desktop).
    #[test]
    fn test_get_first_usage_date_spans_rollups_and_logs() -> Result<(), AppError> {
        let db = Database::memory()?;
        assert_eq!(db.get_first_usage_date(None, None, None)?, None);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES ('2024-03-05', 'claude', 'p1', 'claude-3', 1, 1, 10, 5, 0, 0, '0', 100)",
                [],
            )?;
            insert_usage_log(
                &conn,
                "codex-1",
                "codex",
                "p2",
                "gpt-5.5",
                "proxy",
                local_ts(2025, 1, 2, 10, 0, 0),
                10,
                5,
                0,
                0,
                200,
                "0",
            )?;
            insert_usage_log(
                &conn,
                "desktop-1",
                "claude-desktop",
                "p3",
                "claude-3",
                "proxy",
                local_ts(2023, 12, 31, 23, 30, 0),
                10,
                5,
                0,
                0,
                200,
                "0",
            )?;
        }

        assert_eq!(
            db.get_first_usage_date(None, None, None)?.as_deref(),
            Some("2023-12-31")
        );
        assert_eq!(
            db.get_first_usage_date(Some("claude"), None, None)?
                .as_deref(),
            Some("2023-12-31")
        );
        assert_eq!(
            db.get_first_usage_date(Some("codex"), None, None)?
                .as_deref(),
            Some("2025-01-02")
        );
        assert_eq!(
            db.get_first_usage_date(None, None, Some("claude-3"))?
                .as_deref(),
            Some("2023-12-31")
        );
        assert_eq!(db.get_first_usage_date(Some("gemini"), None, None)?, None);

        Ok(())
    }

    #[test]
    fn test_get_usage_summary_excludes_partial_rollup_boundary_days() -> Result<(), AppError> {
        let db = Database::memory()?;
        let start = local_ts(2024, 1, 1, 12, 0, 0);
        let end = local_ts(2024, 1, 3, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-01-01",
                    "claude",
                    "p1",
                    "claude-3",
                    10,
                    10,
                    1000,
                    500,
                    0,
                    0,
                    "1.00",
                    100
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-01-02",
                    "claude",
                    "p1",
                    "claude-3",
                    20,
                    19,
                    2000,
                    1000,
                    0,
                    0,
                    "2.00",
                    120
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-01-03",
                    "claude",
                    "p1",
                    "claude-3",
                    30,
                    29,
                    3000,
                    1500,
                    0,
                    0,
                    "3.00",
                    140
                ],
            )?;
        }

        let summary = db.get_usage_summary(Some(start), Some(end), Some("claude"), None, None)?;
        assert_eq!(summary.total_requests, 20);
        assert_eq!(summary.total_input_tokens, 2000);
        assert_eq!(summary.total_output_tokens, 1000);

        Ok(())
    }

    #[test]
    fn test_get_request_detail_reads_proxy_and_session_rows() -> Result<(), AppError> {
        let db = Database::memory()?;
        let ts = local_ts(2026, 6, 10, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            // The providers table has created_at / cost_multiplier columns as well: the columns of the
            // detail query need the table alias,
            // otherwise SQLite reports an ambiguous column name and the whole command fails.
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config, created_at) VALUES
                 ('prov-a', 'claude', 'Packy', '{}', 1)",
                [],
            )?;
            insert_usage_log(
                &conn,
                "a-1",
                "claude",
                "prov-a",
                "claude-sonnet-4-6",
                "proxy",
                ts,
                100,
                10,
                0,
                0,
                200,
                "1.0",
            )?;
            insert_usage_log(
                &conn,
                "session:msg_1",
                "claude",
                "_session",
                "claude-sonnet-4-6",
                "session_log",
                ts,
                999,
                99,
                0,
                0,
                200,
                "0.5",
            )?;
        }

        let proxy = db.get_request_detail("a-1")?.expect("proxy row");
        assert_eq!(proxy.provider_name.as_deref(), Some("Packy"));
        assert_eq!(proxy.created_at, ts);
        assert_eq!(proxy.input_tokens, 100);

        let session = db
            .get_request_detail("session:msg_1")?
            .expect("session row");
        assert_eq!(session.provider_name.as_deref(), Some("Claude (Session)"));
        assert_eq!(session.created_at, ts);

        assert!(db.get_request_detail("missing")?.is_none());

        Ok(())
    }

    #[test]
    fn test_provider_and_model_filters_cover_detail_and_rollup() -> Result<(), AppError> {
        let db = Database::memory()?;
        let detail_ts = local_ts(2026, 6, 10, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES
                 ('prov-a', 'claude', 'Packy', '{}'),
                 ('prov-b', 'claude', 'DeepSeek', '{}')",
                [],
            )?;

            insert_usage_log(
                &conn,
                "a-1",
                "claude",
                "prov-a",
                "claude-sonnet-4-6",
                "proxy",
                detail_ts,
                100,
                10,
                0,
                0,
                200,
                "1.0",
            )?;
            insert_usage_log(
                &conn,
                "b-1",
                "claude",
                "prov-b",
                "deepseek-v3",
                "proxy",
                detail_ts,
                200,
                20,
                0,
                0,
                200,
                "2.0",
            )?;
            // A session placeholder row: the providers table has no such id, the display name goes through the CASE mapping.
            insert_usage_log(
                &conn,
                "s-1",
                "claude",
                "_session",
                "claude-sonnet-4-6",
                "session_log",
                detail_ts,
                999,
                99,
                0,
                0,
                200,
                "0.5",
            )?;
            // A row whose pricing model differs from the request model: the model filter has to hit it by the effective pricing model.
            insert_usage_log(
                &conn,
                "a-2",
                "claude",
                "prov-a",
                "alias-model",
                "proxy",
                detail_ts,
                50,
                5,
                0,
                0,
                200,
                "0.3",
            )?;
            conn.execute(
                "UPDATE proxy_request_logs SET pricing_model = 'real-model' WHERE request_id = 'a-2'",
                [],
            )?;

            // A historical daily rollup row: without a range filter it all counts.
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES
                ('2026-06-08', 'claude', 'prov-a', 'claude-sonnet-4-6', 5, 5, 500, 50, 0, 0, '5.0', 100),
                ('2026-06-08', 'claude', 'prov-b', 'deepseek-v3', 7, 7, 700, 70, 0, 0, '7.0', 100)",
                [],
            )?;
        }

        // ① The aggregate filtered by the provider display name: the detail records + the rollup are both hit.
        let packy = db.get_usage_summary(None, None, None, Some("Packy"), None)?;
        assert_eq!(packy.total_requests, 7, "a-1 + a-2 + rollup 5");

        // ② The aggregate filtered by model (the effective pricing model semantics).
        let deepseek = db.get_usage_summary(None, None, None, None, Some("deepseek-v3"))?;
        assert_eq!(deepseek.total_requests, 8, "b-1 + rollup 7");

        // ③ The pricing_model wins over the model: the alias model is not found, the real model is.
        let by_alias = db.get_usage_summary(None, None, None, None, Some("alias-model"))?;
        assert_eq!(by_alias.total_requests, 0);
        let by_real = db.get_usage_summary(None, None, None, None, Some("real-model"))?;
        assert_eq!(by_real.total_requests, 1);

        // ④ A session placeholder row can be selected by its readable name.
        let session = db.get_usage_summary(None, None, None, Some("Claude (Session)"), None)?;
        assert_eq!(session.total_requests, 1);

        // ⑤ The provider statistics + the model filter: only the DeepSeek row is left.
        let provider_stats = db.get_provider_stats(None, None, None, None, Some("deepseek-v3"))?;
        assert_eq!(provider_stats.len(), 1);
        assert_eq!(provider_stats[0].provider_name, "DeepSeek");
        assert_eq!(provider_stats[0].request_count, 8);

        // ⑥ The model statistics + the provider filter: only the models of Packy are left.
        let model_stats = db.get_model_stats(None, None, None, Some("Packy"), None)?;
        let models: Vec<&str> = model_stats.iter().map(|m| m.model.as_str()).collect();
        assert!(models.contains(&"claude-sonnet-4-6"));
        assert!(models.contains(&"real-model"));
        assert!(!models.contains(&"deepseek-v3"));

        // ⑦ The per-app aggregate (the data source of the Hero card) is affected by the filter as well.
        let by_app = db.get_usage_summary_by_app(None, None, Some("Packy"), None)?;
        assert_eq!(by_app.len(), 1);
        assert_eq!(by_app[0].app_type, "claude");
        assert_eq!(by_app[0].summary.total_requests, 7);

        // ⑧ The trend (>24h takes the day buckets + the rollup branch).
        let t_start = local_ts(2026, 6, 8, 0, 0, 0);
        let t_end = local_ts(2026, 6, 10, 23, 59, 59);
        let trends = db.get_daily_trends(Some(t_start), Some(t_end), None, Some("Packy"), None)?;
        let total_req: u64 = trends.iter().map(|d| d.request_count).sum();
        assert_eq!(total_req, 7, "明细 2 + rollup 5");

        // ⑨ The trend <=24h takes the hourly bucket branch (the path that mixes the numbered
        // parameters ?1/?2/?3 with the appended filters),
        // and at the same time the combination of the provider + the model filter is verified.
        let h_start = local_ts(2026, 6, 10, 0, 0, 0);
        let h_end = local_ts(2026, 6, 10, 20, 0, 0);
        let hourly = db.get_daily_trends(
            Some(h_start),
            Some(h_end),
            None,
            Some("Packy"),
            Some("claude-sonnet-4-6"),
        )?;
        let hourly_req: u64 = hourly.iter().map(|d| d.request_count).sum();
        assert_eq!(hourly_req, 1, "仅 a-1 命中（a-2 计价模型不同）");

        // ⑩ The list of the request logs has the same definition as the dropdown: the exact name + the effective pricing model.
        let logs = db.get_request_logs(
            &LogFilters {
                provider_name: Some("Packy".to_string()),
                model: Some("real-model".to_string()),
                ..Default::default()
            },
            0,
            10,
        )?;
        assert_eq!(logs.total, 1);
        assert_eq!(logs.data[0].request_id, "a-2");

        Ok(())
    }

    #[test]
    fn test_get_usage_summary_includes_end_day_rollup_for_full_day_end_time() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        let start = local_ts(2024, 1, 1, 0, 0, 0);
        let end = local_ts(2024, 1, 2, 23, 59, 59);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-01-01",
                    "claude",
                    "p1",
                    "claude-3",
                    10,
                    10,
                    1000,
                    500,
                    0,
                    0,
                    "1.00",
                    100
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-01-02",
                    "claude",
                    "p1",
                    "claude-3",
                    20,
                    19,
                    2000,
                    1000,
                    0,
                    0,
                    "2.00",
                    120
                ],
            )?;
        }

        let summary = db.get_usage_summary(Some(start), Some(end), Some("claude"), None, None)?;
        assert_eq!(summary.total_requests, 30);
        assert_eq!(summary.total_input_tokens, 3000);
        assert_eq!(summary.total_output_tokens, 1500);

        Ok(())
    }

    #[test]
    fn test_effective_usage_dedup_prefers_proxy_for_session_sources() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "codex-proxy",
                "codex",
                "openai",
                "GPT-5.4",
                "proxy",
                10_000,
                100,
                20,
                10,
                7,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "codex-session-dup",
                "codex",
                "_codex_session",
                "gpt-5.4",
                "codex_session",
                10_060,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "claude-proxy",
                "claude",
                "openai-compatible",
                "claude-sonnet-4-5",
                "proxy",
                25_000,
                300,
                60,
                20,
                5,
                200,
                "0.30",
            )?;
            insert_usage_log(
                &conn,
                "claude-session-dup",
                "claude",
                "_session",
                "claude-sonnet-4-5",
                "session_log",
                25_060,
                300,
                60,
                20,
                5,
                200,
                "0.30",
            )?;
            insert_usage_log(
                &conn,
                "gemini-proxy",
                "gemini",
                "google",
                "gemini-2.5-pro",
                "proxy",
                20_000,
                200,
                40,
                30,
                0,
                200,
                "0.20",
            )?;
            insert_usage_log(
                &conn,
                "gemini-session-dup",
                "gemini",
                "_gemini_session",
                "gemini-2.5-pro",
                "gemini_session",
                20_060,
                200,
                40,
                30,
                0,
                200,
                "0.20",
            )?;
            insert_usage_log(
                &conn,
                "codex-session-only",
                "codex",
                "_codex_session",
                "gpt-5.4",
                "codex_session",
                30_000,
                50,
                5,
                0,
                0,
                200,
                "0.02",
            )?;
        }

        let summary = db.get_usage_summary(None, None, None, None, None)?;
        assert_eq!(summary.total_requests, 4);
        // codex-proxy contributes 100-10=90; gemini-proxy contributes 200-30=170
        // (both cache-inclusive providers). claude-proxy=300, codex-session-only=50.
        // 90 + 170 + 300 + 50 = 610.
        assert_eq!(summary.total_input_tokens, 610);
        assert_eq!(summary.total_output_tokens, 125);
        assert_eq!(summary.total_cache_read_tokens, 60);
        assert_eq!(summary.total_cache_creation_tokens, 12);
        // real_total = fresh_input(610) + output(125) + cache_create(12) + cache_read(60) = 807
        assert_eq!(summary.real_total_tokens, 807);
        // hit_rate = 60 / (610 + 12 + 60) = 60 / 682
        let expected_hit_rate = 60.0_f64 / 682.0_f64;
        assert!((summary.cache_hit_rate - expected_hit_rate).abs() < 1e-9);

        let trends = db.get_daily_trends(Some(0), Some(40_000), None, None, None)?;
        assert_eq!(trends.iter().map(|stat| stat.request_count).sum::<u64>(), 4);

        let provider_stats = db.get_provider_stats(None, None, None, None, None)?;
        assert_eq!(
            provider_stats
                .iter()
                .map(|stat| stat.request_count)
                .sum::<u64>(),
            4
        );
        assert!(provider_stats
            .iter()
            .any(|stat| stat.provider_id == "_codex_session" && stat.request_count == 1));
        assert!(!provider_stats
            .iter()
            .any(|stat| stat.provider_id == "_gemini_session"));
        assert!(!provider_stats
            .iter()
            .any(|stat| stat.provider_id == "_session"));

        let model_stats = db.get_model_stats(None, None, None, None, None)?;
        assert_eq!(
            model_stats
                .iter()
                .map(|stat| stat.request_count)
                .sum::<u64>(),
            4
        );

        let logs = db.get_request_logs(&LogFilters::default(), 0, 10)?;
        let request_ids: Vec<&str> = logs
            .data
            .iter()
            .map(|log| log.request_id.as_str())
            .collect();
        assert_eq!(logs.total, 4);
        assert!(request_ids.contains(&"codex-proxy"));
        assert!(request_ids.contains(&"claude-proxy"));
        assert!(request_ids.contains(&"gemini-proxy"));
        assert!(request_ids.contains(&"codex-session-only"));
        assert!(!request_ids.contains(&"codex-session-dup"));
        assert!(!request_ids.contains(&"claude-session-dup"));
        assert!(!request_ids.contains(&"gemini-session-dup"));

        let breakdown = crate::services::session_usage::get_data_source_breakdown(&db)?;
        let proxy_count = breakdown
            .iter()
            .find(|item| item.data_source == "proxy")
            .map(|item| item.request_count);
        let codex_session_count = breakdown
            .iter()
            .find(|item| item.data_source == "codex_session")
            .map(|item| item.request_count);
        let gemini_session_count = breakdown
            .iter()
            .find(|item| item.data_source == "gemini_session")
            .map(|item| item.request_count);
        let session_log_count = breakdown
            .iter()
            .find(|item| item.data_source == "session_log")
            .map(|item| item.request_count);
        assert_eq!(proxy_count, Some(3));
        assert_eq!(codex_session_count, Some(1));
        assert_eq!(gemini_session_count, None);
        assert_eq!(session_log_count, None);

        Ok(())
    }

    #[test]
    fn test_effective_usage_dedup_keeps_non_matching_session_rows() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "proxy-base",
                "codex",
                "openai",
                "gpt-5.4",
                "proxy",
                10_000,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "session-outside-window",
                "codex",
                "_codex_session",
                "gpt-5.4",
                "codex_session",
                10_601,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "session-token-mismatch",
                "codex",
                "_codex_session",
                "gpt-5.4",
                "codex_session",
                10_060,
                101,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "session-app-mismatch",
                "gemini",
                "_gemini_session",
                "gpt-5.4",
                "gemini_session",
                10_060,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "session-model-mismatch",
                "codex",
                "_codex_session",
                "different-model",
                "codex_session",
                10_060,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "proxy-error",
                "codex",
                "openai",
                "gpt-5.4",
                "proxy",
                20_000,
                300,
                60,
                0,
                0,
                500,
                "0.00",
            )?;
            insert_usage_log(
                &conn,
                "session-matches-error-proxy",
                "codex",
                "_codex_session",
                "gpt-5.4",
                "codex_session",
                20_060,
                300,
                60,
                0,
                0,
                200,
                "0.30",
            )?;
            insert_usage_log(
                &conn,
                "claude-proxy-cache-creation",
                "claude",
                "anthropic",
                "claude-sonnet-4-5",
                "proxy",
                30_000,
                100,
                20,
                10,
                5,
                200,
                "0.10",
            )?;
            insert_usage_log(
                &conn,
                "claude-session-cache-creation-mismatch",
                "claude",
                "_session",
                "claude-sonnet-4-5",
                "session_log",
                30_060,
                100,
                20,
                10,
                0,
                200,
                "0.10",
            )?;
        }

        let summary = db.get_usage_summary(None, None, None, None, None)?;
        assert_eq!(summary.total_requests, 9);

        let logs = db.get_request_logs(&LogFilters::default(), 0, 10)?;
        let request_ids: Vec<&str> = logs
            .data
            .iter()
            .map(|log| log.request_id.as_str())
            .collect();
        assert_eq!(logs.total, 9);
        assert!(request_ids.contains(&"session-outside-window"));
        assert!(request_ids.contains(&"session-token-mismatch"));
        assert!(request_ids.contains(&"session-app-mismatch"));
        assert!(request_ids.contains(&"session-model-mismatch"));
        assert!(request_ids.contains(&"session-matches-error-proxy"));
        assert!(request_ids.contains(&"claude-session-cache-creation-mismatch"));

        Ok(())
    }

    #[test]
    fn test_effective_usage_dedup_matches_legacy_codex_proxy_without_cache_writes(
    ) -> Result<(), AppError> {
        // The agents before v3.17.0 do not record the Codex cache writes; the rows of the new semantics still require a matching write amount.
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            for (request_id, provider_id, data_source, created_at, cache_creation) in [
                ("legacy-proxy", "openai", "proxy", 10_000, 0),
                (
                    "session-vs-legacy",
                    "_codex_session",
                    "codex_session",
                    10_060,
                    600,
                ),
                ("total-proxy", "openai", "proxy", 20_000, 0),
                (
                    "session-vs-total",
                    "_codex_session",
                    "codex_session",
                    20_060,
                    600,
                ),
            ] {
                insert_usage_log(
                    &conn,
                    request_id,
                    "codex",
                    provider_id,
                    "gpt-5.6",
                    data_source,
                    created_at,
                    1000,
                    50,
                    300,
                    cache_creation,
                    200,
                    "0.01",
                )?;
            }
            conn.execute(
                "UPDATE proxy_request_logs SET input_token_semantics = ?1
                 WHERE request_id != 'legacy-proxy'",
                [INPUT_TOKEN_SEMANTICS_TOTAL],
            )?;
        }

        // Both formulations, the row by row EXISTS and the rowid NOT IN by window, have to remove the session-vs-legacy.
        let conn = lock_conn!(db.conn);
        let ranged = effective_usage_log_filter_for_range(&conn, "l", None, None)?;
        assert!(ranged.contains("NOT IN"));
        for filter in [effective_usage_log_filter("l"), ranged] {
            let mut stmt = conn.prepare(&format!(
                "SELECT request_id FROM proxy_request_logs l WHERE {filter} ORDER BY request_id"
            ))?;
            let request_ids = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            assert_eq!(
                request_ids,
                ["legacy-proxy", "session-vs-total", "total-proxy"]
            );
        }

        Ok(())
    }

    #[test]
    fn test_get_model_stats() -> Result<(), AppError> {
        let db = Database::memory()?;

        // Insert the test data
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "req1",
                    "p1",
                    "claude",
                    "claude-3-sonnet",
                    100,
                    50,
                    "0.01",
                    100,
                    200,
                    1000
                ],
            )?;
        }

        let stats = db.get_model_stats(None, None, None, None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].model, "claude-3-sonnet");
        assert_eq!(stats[0].request_count, 1);

        Ok(())
    }

    #[test]
    fn test_get_model_stats_success_rate_and_speed_per_model() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            let insert = |id: &str,
                          model: &str,
                          output: i64,
                          latency: i64,
                          first: Option<i64>,
                          status: i64,
                          source: &str| {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, first_token_ms, status_code, created_at, data_source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        id, "p1", "claude", model, 10, output, "0", latency, first, status, 1000,
                        source
                    ],
                )
            };
            // fast: an exact speed of 1000 tokens / 10000 ms plus a failed request
            insert("fast-ok", "fast", 1_000, 11_000, Some(1_000), 200, "proxy")?;
            insert("fast-err", "fast", 0, 500, None, 500, "proxy")?;
            // slow: only the session log, an estimated speed of 400 tokens / 8000 ms
            insert("slow-a", "slow", 400, 8_000, None, 200, "session_log")?;
        }

        let stats = db.get_model_stats(None, None, None, None, None)?;
        let fast = stats.iter().find(|s| s.model == "fast").expect("fast");
        assert_eq!(fast.request_count, 2);
        assert!((fast.success_rate - 50.0).abs() < f32::EPSILON);
        assert_eq!(fast.speed_output_tokens, 1_000);
        assert_eq!(fast.speed_generation_ms, 10_000);
        assert_eq!(fast.est_speed_output_tokens, 0);

        let slow = stats.iter().find(|s| s.model == "slow").expect("slow");
        assert!((slow.success_rate - 100.0).abs() < f32::EPSILON);
        assert_eq!(slow.speed_output_tokens, 0);
        assert_eq!(slow.est_speed_output_tokens, 400);
        assert_eq!(slow.est_speed_duration_ms, 8_000);

        Ok(())
    }

    #[test]
    fn test_stats_first_token_median_only_counts_successful_proxy_streams() -> Result<(), AppError>
    {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            let insert = |id: &str,
                          provider: &str,
                          model: &str,
                          first: Option<i64>,
                          status: i64,
                          source: &str| {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, first_token_ms, status_code, created_at, data_source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        id,
                        provider,
                        "codex",
                        model,
                        10,
                        // The usage of a session log is filtered out as a duplicate just like the one of the router, so they are offset
                        if source == "proxy" { 500 } else { 777 },
                        "0",
                        20_000,
                        first,
                        status,
                        1000,
                        source
                    ],
                )
            };
            // p1 / m1: with an odd number of samples the middle one is taken, the long tail does not pull it up
            insert("a", "p1", "m1", Some(1_000), 200, "proxy")?;
            insert("b", "p1", "m1", Some(2_000), 200, "proxy")?;
            insert("c", "p1", "m1", Some(15_000), 200, "proxy")?;
            // Not counted: failed, not streaming, from the session log
            insert("err", "p1", "m1", Some(100), 500, "proxy")?;
            insert("non-stream", "p1", "m1", None, 200, "proxy")?;
            insert("session", "p1", "m1", Some(50), 200, "codex_session")?;
            // The first token derived from the session log: the median is taken separately
            insert("est-a", "p1", "m1", Some(3_000), 200, "codex_session")?;
            // p2 / m2: with an even number of samples the average of the two middle ones is taken
            insert("d", "p2", "m2", Some(1_000), 200, "proxy")?;
            insert("e", "p2", "m2", Some(3_000), 200, "proxy")?;
            // p3 / m3: there is no countable request
            insert("f", "p3", "m3", None, 200, "proxy")?;
        }

        let providers = db.get_provider_stats(None, None, None, None, None)?;
        let median_of = |id: &str| {
            providers
                .iter()
                .find(|s| s.provider_id == id)
                .expect(id)
                .first_token_median_ms
        };
        assert_eq!(median_of("p1"), Some(2_000));
        assert_eq!(median_of("p2"), Some(2_000));
        assert_eq!(median_of("p3"), None);
        let p1 = providers.iter().find(|s| s.provider_id == "p1").unwrap();
        // Two session log rows: 50 and 3000
        assert_eq!(p1.est_first_token_median_ms, Some(1_525));
        // The first token derived from the session log does not count towards the exact speed: only the four rows a, b, c and err of the router go into the speed
        assert_eq!(p1.speed_output_tokens, 2_000);

        let models = db.get_model_stats(None, None, None, None, None)?;
        let median_of = |m: &str| {
            models
                .iter()
                .find(|s| s.model == m)
                .expect(m)
                .first_token_median_ms
        };
        assert_eq!(median_of("m1"), Some(2_000));
        assert_eq!(median_of("m2"), Some(2_000));
        assert_eq!(median_of("m3"), None);

        // With a filter condition the WHERE has to be appended, two WHERE must not be concatenated
        let filtered = db.get_provider_stats(None, None, Some("codex"), None, Some("m2"))?;
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].first_token_median_ms, Some(2_000));

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_with_time_filter() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params!["old", "p1", "claude", "claude-3", 100, 50, "0.01", 100, 200, 1000],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params!["new", "p1", "claude", "claude-3", 200, 75, "0.02", 120, 200, 2000],
            )?;
        }

        let stats = db.get_provider_stats(Some(1500), Some(2500), Some("claude"), None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].provider_id, "p1");
        assert_eq!(stats[0].app_type, "claude");
        assert_eq!(stats[0].request_count, 1);
        assert_eq!(stats[0].total_tokens, 275);

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_preserves_app_type_identity_across_apps() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?, ?, ?, '{}')",
                params!["shared-p1", "claude", "Shared Provider"],
            )?;
            conn.execute(
                "INSERT INTO providers (id, app_type, name, settings_config) VALUES (?, ?, ?, '{}')",
                params!["shared-p1", "claude-desktop", "Shared Provider"],
            )?;

            // Detail log under claude
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "req-1",
                    "shared-p1",
                    "claude",
                    "m1",
                    100,
                    50,
                    "0.01",
                    100,
                    200,
                    1000
                ],
            )?;

            // Rollup under claude-desktop
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, total_cost_usd,
                    success_count, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2026-10-08",
                    "claude-desktop",
                    "shared-p1",
                    "m1",
                    10,
                    1000,
                    500,
                    0,
                    0,
                    "0.10",
                    10,
                    120
                ],
            )?;
        }

        // Folded query for "claude" matches both claude and claude-desktop
        let stats = db.get_provider_stats(None, None, Some("claude"), None, None)?;
        assert_eq!(stats.len(), 2);

        let claude_stat = stats
            .iter()
            .find(|s| s.app_type == "claude")
            .expect("claude stat");
        assert_eq!(claude_stat.provider_id, "shared-p1");
        assert_eq!(claude_stat.provider_name, "Shared Provider");
        assert_eq!(claude_stat.request_count, 1);

        let desktop_stat = stats
            .iter()
            .find(|s| s.app_type == "claude-desktop")
            .expect("claude-desktop stat");
        assert_eq!(desktop_stat.provider_id, "shared-p1");
        assert_eq!(desktop_stat.provider_name, "Shared Provider");
        assert_eq!(desktop_stat.request_count, 10);

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_speed_sums_only_eligible_requests() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            let insert = |id: &str, output: i64, latency: i64, first: Option<i64>| {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, first_token_ms, status_code, created_at
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![id, "p1", "claude", "m", 10, output, "0", latency, first, 200, 1000],
                )
            };
            // Counted: 1000 tokens / (11000 - 1000) ms
            insert("ok-a", 1000, 11_000, Some(1_000))?;
            // Counted: 300 tokens / (4000 - 1000) ms
            insert("ok-b", 300, 4_000, Some(1_000))?;
            // Not counted: the output is below 100
            insert("short", 50, 2_000, Some(100))?;
            // Not counted: no first token (session log / not streaming)
            insert("no-ttft", 5_000, 9_000, None)?;
            // Not counted: the duration is not greater than the first token
            insert("zero-gen", 500, 1_000, Some(1_000))?;
            // Not counted: the generation window is below 100ms, it is a transmission burst
            insert("burst", 500, 1_050, Some(1_000))?;
        }

        let stats = db.get_provider_stats(None, None, None, None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].request_count, 6);
        assert_eq!(stats[0].speed_output_tokens, 1_300);
        assert_eq!(stats[0].speed_generation_ms, 13_000);
        // The rows of the router service have no first token (not streaming) and do not count towards the estimated speed either
        assert_eq!(stats[0].est_speed_output_tokens, 0);
        assert_eq!(stats[0].est_speed_duration_ms, 0);

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_estimated_speed_sums_session_rows() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            let insert = |id: &str, output: i64, latency: i64, source: &str| {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model,
                        input_tokens, output_tokens, total_cost_usd,
                        latency_ms, status_code, created_at, data_source
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        id, "_session", "claude", "m", 10, output, "0", latency, 200, 1000, source
                    ],
                )
            };
            // Counted: 2000 tokens / 20000 ms
            insert("ok-a", 2_000, 20_000, "session_log")?;
            // Counted: 500 tokens / 5000 ms
            insert("ok-b", 200, 5_000, "session_log")?;
            // Not counted: the output is below 200
            insert("short", 199, 5_000, "session_log")?;
            // Not counted: no duration was estimated
            insert("no-timing", 3_000, 0, "session_log")?;
            // Not counted: the duration is below 1 second
            insert("too-fast", 800, 900, "session_log")?;
        }

        let stats = db.get_provider_stats(None, None, None, None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].est_speed_output_tokens, 2_200);
        assert_eq!(stats[0].est_speed_duration_ms, 25_000);
        // The estimated ones are not mixed into the exact definition
        assert_eq!(stats[0].speed_output_tokens, 0);
        assert_eq!(stats[0].speed_generation_ms, 0);

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_labels_opencode_session_provider() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            insert_usage_log(
                &conn,
                "opencode-session",
                "opencode",
                "_opencode_session",
                "opencode-model",
                "opencode_session",
                1000,
                100,
                50,
                0,
                0,
                200,
                "0.01",
            )?;
        }

        let stats = db.get_provider_stats(None, None, Some("opencode"), None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].provider_id, "_opencode_session");
        assert_eq!(stats[0].provider_name, "OpenCode (Session)");

        Ok(())
    }

    #[test]
    fn test_get_provider_stats_excludes_partial_rollup_boundary_days() -> Result<(), AppError> {
        let db = Database::memory()?;
        let start = local_ts(2024, 2, 1, 12, 0, 0);
        let end = local_ts(2024, 2, 3, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-02-01",
                    "claude",
                    "p-rollup",
                    "claude-3",
                    5,
                    5,
                    500,
                    250,
                    0,
                    0,
                    "0.50",
                    100
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-02-02",
                    "claude",
                    "p-rollup",
                    "claude-3",
                    8,
                    7,
                    800,
                    400,
                    0,
                    0,
                    "0.80",
                    120
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-02-03",
                    "claude",
                    "p-rollup",
                    "claude-3",
                    12,
                    11,
                    1200,
                    600,
                    0,
                    0,
                    "1.20",
                    140
                ],
            )?;
        }

        let stats = db.get_provider_stats(Some(start), Some(end), Some("claude"), None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].provider_id, "p-rollup");
        assert_eq!(stats[0].request_count, 8);
        assert_eq!(stats[0].total_tokens, 1200);

        Ok(())
    }

    #[test]
    fn test_get_daily_trends_respects_shorter_than_24_hours() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "req-short",
                    "p1",
                    "claude",
                    "claude-3",
                    100,
                    50,
                    "0.01",
                    100,
                    200,
                    10_800
                ],
            )?;
        }

        let stats = db.get_daily_trends(Some(0), Some(15 * 60 * 60), Some("claude"), None, None)?;
        assert_eq!(stats.len(), 15);
        assert_eq!(stats[3].request_count, 1);

        Ok(())
    }

    #[test]
    fn test_get_daily_trends_groups_ranges_longer_than_24_hours_by_local_day(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        let start = local_ts(2024, 3, 1, 12, 0, 0);
        let end = local_ts(2024, 3, 3, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "day-1-detail",
                    "p1",
                    "claude",
                    "claude-3",
                    100,
                    50,
                    "0.01",
                    100,
                    200,
                    local_ts(2024, 3, 1, 13, 0, 0)
                ],
            )?;
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model,
                    input_tokens, output_tokens, total_cost_usd,
                    latency_ms, status_code, created_at
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "day-3-detail",
                    "p1",
                    "claude",
                    "claude-3",
                    200,
                    75,
                    "0.02",
                    110,
                    200,
                    local_ts(2024, 3, 3, 10, 0, 0)
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-03-02",
                    "claude",
                    "p1",
                    "claude-3",
                    4,
                    4,
                    400,
                    200,
                    0,
                    0,
                    "0.40",
                    120
                ],
            )?;
        }

        let stats = db.get_daily_trends(Some(start), Some(end), Some("claude"), None, None)?;
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0].request_count, 1);
        assert_eq!(stats[0].total_tokens, 150);
        assert_eq!(stats[1].request_count, 4);
        assert_eq!(stats[1].total_tokens, 600);
        assert_eq!(stats[2].request_count, 1);
        assert_eq!(stats[2].total_tokens, 275);

        Ok(())
    }

    #[test]
    fn test_provider_and_model_stats_tokens_sum_to_summary_real_total() -> Result<(), AppError> {
        let db = Database::memory()?;

        {
            let conn = lock_conn!(db.conn);
            // Claude: the input does not include the cache. The real consumption = 100 + 200 + 1000 + 5000
            insert_usage_log(
                &conn,
                "claude-1",
                "claude",
                "p-claude",
                "claude-x",
                "session_log",
                1000,
                100,
                200,
                5000,
                1000,
                200,
                "0.10",
            )?;
            // Codex: the input already includes the 600 cache hits. The real consumption = (1000 - 600) + 50 + 600
            insert_usage_log(
                &conn,
                "codex-1",
                "codex",
                "p-codex",
                "gpt-x",
                "codex_session",
                1000,
                1000,
                50,
                600,
                0,
                200,
                "0.05",
            )?;
            // A daily rollup row also has to carry the cache. The real consumption = (900 - 300) + 40 + 300 + 0
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2020-01-01",
                    "codex",
                    "p-codex",
                    "gpt-x",
                    3,
                    3,
                    900,
                    40,
                    300,
                    0,
                    "0.30",
                    100
                ],
            )?;
        }

        let summary = db.get_usage_summary(None, None, None, None, None)?;
        assert_eq!(summary.real_total_tokens, 6300 + 1050 + 940);

        let providers = db.get_provider_stats(None, None, None, None, None)?;
        let tokens_of = |id: &str| {
            providers
                .iter()
                .find(|s| s.provider_id == id)
                .map(|s| s.total_tokens)
        };
        assert_eq!(tokens_of("p-claude"), Some(6300));
        assert_eq!(tokens_of("p-codex"), Some(1050 + 940));
        let provider_sum: u64 = providers.iter().map(|s| s.total_tokens).sum();
        assert_eq!(provider_sum, summary.real_total_tokens);

        let models = db.get_model_stats(None, None, None, None, None)?;
        let model_sum: u64 = models.iter().map(|s| s.total_tokens).sum();
        assert_eq!(model_sum, summary.real_total_tokens);

        Ok(())
    }

    #[test]
    fn test_get_model_stats_excludes_partial_rollup_boundary_days() -> Result<(), AppError> {
        let db = Database::memory()?;
        let start = local_ts(2024, 4, 1, 12, 0, 0);
        let end = local_ts(2024, 4, 3, 12, 0, 0);

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-04-01",
                    "claude",
                    "p1",
                    "claude-3-haiku",
                    6,
                    6,
                    600,
                    300,
                    0,
                    0,
                    "0.60",
                    100
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-04-02",
                    "claude",
                    "p1",
                    "claude-3-haiku",
                    9,
                    8,
                    900,
                    450,
                    0,
                    0,
                    "0.90",
                    110
                ],
            )?;
            conn.execute(
                "INSERT INTO usage_daily_rollups (
                    date, app_type, provider_id, model,
                    request_count, success_count, input_tokens, output_tokens,
                    cache_read_tokens, cache_creation_tokens, total_cost_usd, avg_latency_ms
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    "2024-04-03",
                    "claude",
                    "p1",
                    "claude-3-haiku",
                    12,
                    11,
                    1200,
                    600,
                    0,
                    0,
                    "1.20",
                    130
                ],
            )?;
        }

        let stats = db.get_model_stats(Some(start), Some(end), Some("claude"), None, None)?;
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].model, "claude-3-haiku");
        assert_eq!(stats[0].request_count, 9);
        assert_eq!(stats[0].total_tokens, 1350);

        Ok(())
    }

    #[test]
    fn test_strip_model_date_suffix_is_utf8_safe() {
        assert_eq!(
            strip_model_date_suffix("模型-2026-05-14").as_deref(),
            Some("模型")
        );
        assert_eq!(strip_model_date_suffix("abc🚀12345678"), None);
    }

    #[test]
    fn test_strip_model_date_suffix_handles_six_digit_yymmdd() {
        // The 6 digit YYMMDD suffix of the Volcengine should be stripped (the whole doubao family uses that format).
        assert_eq!(
            strip_model_date_suffix("doubao-seed-2-1-pro-260628").as_deref(),
            Some("doubao-seed-2-1-pro")
        );
        assert_eq!(
            strip_model_date_suffix("doubao-seed-1-6-250615").as_deref(),
            Some("doubao-seed-1-6")
        );
        // The 8 digit YYYYMMDD is still stripped as before.
        assert_eq!(
            strip_model_date_suffix("claude-3-5-sonnet-20241022").as_deref(),
            Some("claude-3-5-sonnet")
        );
        // A 6 digit tail with an invalid month/day (a version number and the like) is not stripped, that avoids a false hit.
        assert_eq!(strip_model_date_suffix("foo-bar-123456"), None); // month=34
        assert_eq!(strip_model_date_suffix("widget-209900"), None); // month=99
        assert_eq!(strip_model_date_suffix("gizmo-251200"), None); // day=00
    }

    #[test]
    fn test_pricing_resolves_volcengine_dated_model_to_bare_seed_row() -> Result<(), AppError> {
        // Regression: the real Volcengine usage carries a 6 digit date suffix
        // (doubao-seed-2-1-pro-260628) and
        // has to normalize to a hit of the bare seed row of the pricing table (doubao-seed-2-1-pro),
        // otherwise the cost shows $0.
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);

        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
            ) VALUES ('doubao-seed-2-1-pro', 'Doubao Seed 2.1 Pro', '0.84', '4.2', '0.17', '0')",
            [],
        )?;

        let row = find_model_pricing_row(&conn, "doubao-seed-2-1-pro-260628")?;
        assert!(
            row.is_some(),
            "带日期的火山模型应通过 6 位日期剥离命中裸名定价行"
        );
        let (input, output, ..) = row.unwrap();
        assert_eq!(input, "0.84");
        assert_eq!(output, "4.2");

        Ok(())
    }

    #[test]
    fn test_pricing_lookup_matches_custom_rows_with_uppercase_model_id() -> Result<(), AppError> {
        // Regression #8413: a custom pricing is often stored with the display name of the UI as it is
        // (for example GLM-5.3-Flash-EXL3),
        // while the candidates of the price lookup are lowercased by clean_model_id_for_pricing; if the
        // exact match is case sensitive
        // those rows never hit and the cost stays at 0. The price lookup has to be case insensitive.
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);

        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
            ) VALUES ('GLM-5.3-Flash-EXL3', 'GLM-5.3-Flash-EXL3', '1.4', '4.4', '0.26', '0')",
            [],
        )?;

        let row = find_model_pricing_row(&conn, "GLM-5.3-Flash-EXL3")?;
        assert!(row.is_some(), "小写候选应能命中按大写入库的自定义定价行");
        let (input, output, ..) = row.unwrap();
        assert_eq!(input, "1.4");
        assert_eq!(output, "4.4");

        Ok(())
    }

    #[test]
    fn test_prefix_pricing_does_not_match_short_base_model_to_variant() -> Result<(), AppError> {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);

        conn.execute("DELETE FROM model_pricing WHERE model_id LIKE 'gpt-5%'", [])?;
        for (model_id, display_name) in [("gpt-5-mini", "GPT-5 Mini"), ("gpt-5-pro", "GPT-5 Pro")] {
            conn.execute(
                "INSERT INTO model_pricing (
                    model_id, display_name, input_cost_per_million, output_cost_per_million,
                    cache_read_cost_per_million, cache_creation_cost_per_million
                ) VALUES (?1, ?2, '1', '2', '0', '0')",
                params![model_id, display_name],
            )?;
        }

        let result = find_model_pricing_row(&conn, "gpt-5")?;
        assert!(
            result.is_none(),
            "缺少 gpt-5 基础定价时，不应前缀误匹配到 gpt-5-mini/gpt-5-pro"
        );

        Ok(())
    }

    #[test]
    fn test_model_pricing_matching() -> Result<(), AppError> {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);

        // Prepare the extra pricing data to cover the prefix / suffix cleaning scenarios
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
            ) VALUES (?, ?, ?, ?, ?, ?)",
            params![
                "claude-haiku-4.5",
                "Claude Haiku 4.5",
                "1.0",
                "2.0",
                "0.0",
                "0.0"
            ],
        )?;

        // Test the exact match (seed_model_pricing already has claude-sonnet-4-5-20250929)
        let result = find_model_pricing_row(&conn, "claude-sonnet-4-5-20250929")?;
        assert!(
            result.is_some(),
            "应该能精确匹配 claude-sonnet-4-5-20250929"
        );

        // Cleaning: remove the prefix and the colon suffix
        let result = find_model_pricing_row(&conn, "anthropic/claude-haiku-4.5")?;
        assert!(
            result.is_some(),
            "带前缀的模型 anthropic/claude-haiku-4.5 应能匹配到 claude-haiku-4.5"
        );
        let result = find_model_pricing_row(&conn, "moonshotai/kimi-k2-0905:exa")?;
        assert!(
            result.is_some(),
            "带前缀+冒号后缀的模型应清洗后匹配到 kimi-k2-0905"
        );

        // Cleaning: @ becomes - (seed_model_pricing already has gpt-5.2-codex-low)
        let result = find_model_pricing_row(&conn, "gpt-5.2-codex@low")?;
        assert!(
            result.is_some(),
            "带 @ 分隔符的模型 gpt-5.2-codex@low 应能匹配到 gpt-5.2-codex-low"
        );
        let result = find_model_pricing_row(&conn, "OpenAI/GPT-5.5@HIGH")?;
        assert!(
            result.is_some(),
            "大小写混合的 GPT-5.5 模型应能归一化匹配到 gpt-5.5-high"
        );
        let result = find_model_pricing_row(&conn, "OpenAI/GPT-5.5-2026-05-14")?;
        assert!(
            result.is_some(),
            "OpenAI 日期后缀模型应能回退到 gpt-5.5 基础定价"
        );
        let result = find_model_pricing_row(&conn, "google/gemini-3-pro-preview-20260514")?;
        assert!(
            result.is_some(),
            "Gemini 日期后缀模型应能回退到 gemini-3-pro-preview 基础定价"
        );

        // The short id of a Claude Desktop route: it should match the pricing with a date through the prefix
        let result = find_model_pricing_row(&conn, "claude-haiku-4-5")?;
        assert!(
            result.is_some(),
            "Claude Desktop 短路由 claude-haiku-4-5 应能匹配到 claude-haiku-4-5-20251001"
        );
        let result = find_model_pricing_row(&conn, "anthropic/claude-opus-4.8")?;
        assert!(
            result.is_some(),
            "聚合商点号格式 anthropic/claude-opus-4.8 应能匹配到 claude-opus-4-8"
        );

        // The non-Anthropic route of an old / abnormal Claude Desktop wrapper: claude-gpt-5.5 -> gpt-5.5
        let result = find_model_pricing_row(&conn, "claude-gpt-5.5")?;
        assert!(
            result.is_some(),
            "带 claude- 包装的非 Anthropic 模型应能剥离后匹配到真实模型定价"
        );

        // The common shape of Bedrock/Vertex: a provider prefix + a -vN suffix + the :0 modifier
        let result =
            find_model_pricing_row(&conn, "global.anthropic.claude-haiku-4-5-20251001-v1:0")?;
        assert!(
            result.is_some(),
            "Bedrock/Vertex 风格 Claude 模型 ID 应能归一化到基础 Claude 模型定价"
        );
        let result = find_model_pricing_row(&conn, "global.anthropic.claude-opus-4-8-v1:0")?;
        assert!(
            result.is_some(),
            "Bedrock 风格 Claude Opus 4.8 模型 ID 应能归一化到基础 Claude 模型定价"
        );
        let result = find_model_pricing_row(&conn, "claude-opus-4-8@20260527")?;
        assert!(
            result.is_some(),
            "Vertex 风格 Claude Opus 4.8 模型 ID 应能归一化到基础 Claude 模型定价"
        );

        // The reasoning effort suffix: it falls back to the base model when there is no dedicated price
        let result = find_model_pricing_row(&conn, "gpt-5.4@low")?;
        assert!(
            result.is_some(),
            "缺少专门 effort 价格时应回退到 gpt-5.4 基础模型定价"
        );

        // Kimi Code is a subscription / quota model and must not pretend to be a public per-token model
        let result = find_model_pricing_row(&conn, "kimi-for-coding")?;
        assert!(result.is_none(), "kimi-for-coding 没有固定 token 单价");

        // Test a model that does not exist
        let result = find_model_pricing_row(&conn, "unknown-model-123")?;
        assert!(result.is_none(), "不应该匹配不存在的模型");

        Ok(())
    }
}
