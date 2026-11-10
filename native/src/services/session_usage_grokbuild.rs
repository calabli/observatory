//! Usage tracking for the Grok Build (Grok CLI) sessions
//!
//! Extracts the usage from the `turn_completed` events of
//! `~/.grok/{sessions,archived_sessions}/<enc-cwd>/<session-id>/updates.jsonl`,
//! writes it into proxy_request_logs and thereby implements the usage statistics for the
//! official OAuth direct connection mode (without proxy data).
//!
//! ## Data flow
//! ```text
//! updates.jsonl (turn_completed per turn) -> settling window / takeover guard -> cost calculation -> proxy_request_logs
//! ```
//!
//! ## Event semantics (confirmed twice: by a 2026-07-23 measurement with two prompts in one
//! process and by reverse engineering the CLI binary)
//! - The usage of a `sessionUpdate == "turn_completed"` event is the **independent total of
//!   that one user prompt round**: it accumulates across the inference loops of the round
//!   (`modelCalls`/`numTurns` = the number of loops of this round) and the next round starts
//!   from zero again. It is **not** a process or session total -- the process total goes
//!   through a separate channel inside the CLI (`GetSessionUsage`, "since start or last
//!   resume") and never lands in updates.jsonl. 🔴 Do not change it back to the difference
//!   of neighbouring events: that treats the total of a round as a cumulative snapshot and
//!   records the second round as the difference of the two rounds, which massively
//!   under-records (it was done before and a measurement with two prompts in one process
//!   disproved it).
//! - Recording every event at its face value is the correct per-round record; two rounds
//!   with exactly the same numbers mean two real usages and both are recorded as usual.
//! - `reasoningTokens` ⊂ `outputTokens` (totalTokens = input + output, and the output
//!   derived from costUsdTicks does not add the reasoning on top), they do not take part in
//!   the billing.
//! - `costUsdTicks` (1 tick = 1e-10 USD) is the exact cost of the round as reported by the
//!   CLI; the 6 measured samples match the local pricing grok-4.5-build 2/6/0.30 to the last
//!   digit. **When the CLI reports a complete cost, total_cost follows the report** (the
//!   backfill only fills rows with total<=0 and never corrects a wrong price, and there is no
//!   repair path after a row was recorded, so the pricing drift window must not be bet on the
//!   local price); the local pricing is responsible for the individual items and the drift
//!   warning. `costIsPartial` marks the report as a lower bound: with a local price the full
//!   amount is recomputed locally and the drift warning is suppressed, without a price the
//!   lower bound is recorded (the individual items are 0).
//! - The double counting in the takeover state is not avoided by deduplicating on a
//!   fingerprint: in the takeover state the CLI keeps writing updates.jsonl, but the turn
//!   events are aggregated values (the sum of several loops) and are structurally not equal
//!   to the per-request rows of the proxy. The "settling window plus a takeover activity
//!   window guard" is used instead: only events that are old enough are imported (by then
//!   the proxy rows of the takeover state have to be in the database), and before the insert
//!   the rows around the event time are queried for proxy activity (see
//!   `has_recent_grokbuild_proxy_activity`).

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    metadata_modified_nanos, update_sync_state_on_conn, SessionPricingCache, SessionSyncResult,
};
use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_TOTAL;
use crate::services::usage_stats::{
    has_recent_grokbuild_proxy_activity, SESSION_PROXY_DEDUP_WINDOW_SECONDS,
};
use rust_decimal::Decimal;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The settling window of the events: only events that are older than "now - window" are
/// imported.
///
/// In the takeover state the CLI keeps writing updates.jsonl while the proxy already
/// accounted the same request per request; the proxy rows and the session events are
/// produced almost at the same time, so if the import runs before the proxy rows are in the
/// database the takeover guard lets them through because it cannot find them and the double
/// counting stays forever. Once the events first "settle" and are imported afterwards, the
/// guard query necessarily sees the proxy rows that are already in the database and the race
/// is eliminated at the source. The price: the usage of the official mode shows up at most
/// one window plus one background sync cycle (60s) late.
///
/// The window only has to cover the few seconds during which "the proxy row lands after the
/// turn_completed is written": the proxy row of every request of a round is written when
/// the response ends, while the turn_completed is only written once the last request has
/// ended. 🔴 Do not tie it to the ±`SESSION_PROXY_DEDUP_WINDOW_SECONDS` of the guard as the
/// same constant -- the two have different semantics, and tying them together has delayed
/// the usage of the official mode by more than ten minutes for no reason.
const SETTLE_WINDOW_SECONDS: i64 = 60;

// A proxy row that landed during the settling period is at most one settling window later than the event and must still fall into the guard window.
const _: () = assert!(SETTLE_WINDOW_SECONDS < SESSION_PROXY_DEDUP_WINDOW_SECONDS);

/// The usage of one model in this round (taken from `modelUsage` or from the top level usage, both are per-round values)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct GrokCounters {
    input: u64,
    output: u64,
    cached: u64,
    api_ms: u64,
    model_calls: u64,
    /// The cost of this round as reported by the CLI, 1 tick = 1e-10 USD; 0 = the upstream did not provide one
    cost_ticks: u64,
    /// The upstream marks the cost_ticks as a partial cost (`costIsPartial`): then it is a lower bound
    cost_partial: bool,
}

impl GrokCounters {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cached == 0
    }

    fn reported_cost_usd(&self) -> Option<Decimal> {
        (self.cost_ticks > 0)
            .then(|| Decimal::from(self.cost_ticks) / Decimal::from(10_000_000_000u64))
    }
}

/// One `turn_completed` usage event
#[derive(Debug)]
struct GrokUsageEvent {
    created_at: i64,
    prompt_id: String,
    /// The event level `costIsPartial` (the place where it was observed on the top level usage; it applies to all models of this event)
    cost_is_partial: bool,
    per_model: Vec<(String, GrokCounters)>,
}

/// Sync the Grok Build usage data (from the updates.jsonl session logs)
pub fn sync_grokbuild_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let files = collect_grok_updates_files();

    let mut result = SessionSyncResult {
        files_scanned: files.len() as u32,
        ..Default::default()
    };

    let cursors = crate::services::session_usage::load_sync_cursors(db)?;

    for file_path in &files {
        match sync_single_grok_file(db, file_path, &cursors) {
            Ok(file_result) => result.merge(file_result),
            Err(e) => {
                let msg = format!("Grok Build 会话文件解析失败 {}: {e}", file_path.display());
                log::warn!("[GROK-SYNC] {msg}");
                result.errors.push(msg);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[GROK-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件, 延后 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned,
            result.deferred_files
        );
    }

    Ok(result)
}

/// Collect the updates.jsonl of all the Grok sessions (including the archived sessions, the same root as the session browser)
fn collect_grok_updates_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for root in crate::session_manager::providers::grokbuild::session_roots() {
        collect_files_named(&root, "updates.jsonl", &mut files, 0);
    }
    files
}

/// The read limit for a single updates.jsonl file (50 MiB). A JSONL event is usually a few
/// KiB and even an active session does not reach that after months; anything above is treated
/// as an anomalous or malicious file and skipped.
const MAX_GROK_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// The maximum directory depth when the session logs are collected recursively, to keep a symlink cycle from overflowing the stack.
const MAX_COLLECT_DEPTH: usize = 16;

/// Collect the files with the given name below a directory recursively (tolerating changes in the layout depth, like the session browser does)
fn collect_files_named(root: &Path, name: &str, files: &mut Vec<PathBuf>, depth: usize) {
    if depth > MAX_COLLECT_DEPTH {
        log::warn!(
            "Grok session directory traversal exceeded max depth {} at {}",
            MAX_COLLECT_DEPTH,
            root.display()
        );
        return;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // `entry.metadata()` does not follow symbolic links (unlike `path.is_dir()`), so **every
        // symlink is skipped unconditionally** here: a directory symlink is not recursed into (which
        // avoids cycles) and a file symlink is not collected either -- a file with the same name that
        // points through a symlink outside the sessions root would otherwise be read as a session log
        // although the user did not expect that. The price: a user who made the whole sessions
        // directory a symlink syncs no data, so the skip has to be logged to make a silently
        // missing usage diagnosable.
        let metadata = entry.metadata();
        if metadata.as_ref().map(|m| m.is_symlink()).unwrap_or(false) {
            log::info!("[GROK-SYNC] 跳过符号链接（不跟随）: {}", path.display());
            continue;
        }
        let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
        if is_dir {
            collect_files_named(&path, name, files, depth + 1);
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            files.push(path);
        }
    }
}

/// Sync a single updates.jsonl file. The cursors come from the prefetch of the caller.
fn sync_single_grok_file(
    db: &Database,
    file_path: &Path,
    cursors: &std::collections::HashMap<String, crate::services::session_usage::SyncCursor>,
) -> Result<SessionSyncResult, AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);

    // An abnormally large file is skipped right away, so that reading it does not exhaust the memory.
    if metadata.len() > MAX_GROK_FILE_BYTES {
        log::warn!(
            "Grok session log too large ({} bytes), skipping: {}",
            metadata.len(),
            file_path.display()
        );
        return Ok(SessionSyncResult::default());
    }

    let last_modified = cursors.get(&file_path_str).map_or(0, |c| c.last_modified);
    if file_modified <= last_modified {
        return Ok(SessionSyncResult::default());
    }

    // When the file changed it is read completely again: the idempotent UPSERT makes the re-read
    // harmless, and the events that were delayed by the settling window depend on the re-read of
    // the next round anyway. The events are independent per-round values, so an incremental read
    // from an offset would be correct (there is no dependence on a difference baseline), but the
    // offset rollback for the delayed events would have to be handled separately, and the gain
    // (saving one O(N) parse per cycle for an active session) does not justify that complexity
    // yet.
    let content = fs::read_to_string(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件: {e}")))?;
    let events = parse_grok_usage_events(&content);

    // The session id = the name of the session directory (the same as the info.id of summary.json).
    // The uniqueness of the request_id is bet on that UUIDv7 being globally unique: an archived
    // and an active copy with the same id converge idempotently through the UPSERT (on purpose),
    // and an id collision below a different <enc-cwd> is considered impossible.
    let session_id = file_path
        .parent()
        .and_then(|dir| dir.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    let mut result = SessionSyncResult::default();
    let mut deferred = false;

    // One transaction per file and one lock acquisition: before it was one lock per event (the
    // takeover guard) plus one lock per model row plus a price lookup per row, which meant O(N)
    // lock round trips and price lookups per round for an active session. The cursor and the data
    // advance atomically in the same transaction (they do not advance while the settling window
    // delays the events).
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("开启 Grok 会话写入事务失败: {e}")))?;
    let mut pricing_cache = SessionPricingCache::default();

    for (idx, event) in events.iter().enumerate() {
        // The settling window: the events are monotonic in time in append order, so the first event
        // that has not settled stops the walk and the later events wait for the next round together
        // with it (which keeps the simple invariant "the prefix of the file is imported").
        // Known limitation: a future timestamp (a wrongly set clock) keeps the file postponed and
        // rescanned as a whole, it heals itself once the wall clock has passed the event time plus
        // the window; the full re-read of an active session per cycle is the designed price.
        if now.saturating_sub(event.created_at) < SETTLE_WINDOW_SECONDS {
            deferred = true;
            break;
        }

        // The takeover guard decides once per event time, and all the model rows of that event move
        // together;
        // the tokens of an event that was skipped by the guard were already accounted by the proxy
        // rows, so skipping is the final state (the sync state still advances as usual).
        // Known limitation: the guard has no session dimension, see the comments in usage_stats.rs.
        let takeover_active = has_recent_grokbuild_proxy_activity(&tx, event.created_at)?;

        for (model, turn) in &event.per_model {
            if turn.is_zero() {
                continue;
            }
            if takeover_active {
                // Counted as skipped (aligned with the semantics of the gemini fingerprint deduplication:
                // not recorded, the proxy
                // rows are authoritative). Do not use suspected_duplicates instead -- codex has the opposite
                // semantics for it (recorded, to be checked) and merge() would simply add the two meanings
                // together.
                result.skipped += 1;
                continue;
            }

            // The idempotency key is anchored to the stable upstream id (the prompt_id is a UUID that is
            // unique per round) and contains no index inside the file: when the prefix of updates.jsonl
            // is rewritten (a rewind truncation) and the event indices shift forward, the surviving
            // rounds still hit their original row and are not double counted; the rows of the removed
            // rounds are kept --
            // a rewind does not give back the tokens that were already consumed, so keeping them is the
            // correct bookkeeping. If the upstream writes several turn_completed for the same prompt_id
            // (not observed), the UPSERT takes the later one, which under-records instead of double
            // counting. Without a prompt_id it falls back to "idx{N}" (a prompt_id in the UUID form
            // cannot collide with it).
            let turn_key = if event.prompt_id.is_empty() {
                format!("idx{idx}")
            } else {
                event.prompt_id.clone()
            };
            let request_id = format!("grok_session:{session_id}:{turn_key}:{model}");
            match insert_grok_session_entry_on_conn(
                &tx,
                &request_id,
                turn,
                event.cost_is_partial || turn.cost_partial,
                model,
                &session_id,
                event.created_at,
                &mut pricing_cache,
            ) {
                Ok(true) => result.imported += 1,
                Ok(false) => result.skipped += 1,
                Err(e) => {
                    log::warn!("[GROK-SYNC] 插入失败 ({request_id}): {e}");
                    result.skipped += 1;
                }
            }
        }
    }

    if deferred {
        // Do not record a sync state: the next round re-reads the whole file and adds the settled events.
        result.deferred_files += 1;
    } else {
        update_sync_state_on_conn(&tx, &file_path_str, file_modified, events.len() as i64)?;
    }
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交 Grok 会话写入事务失败: {e}")))?;

    Ok(result)
}

/// Parse all the per-round usage events out of the updates.jsonl content (keeping the file order)
fn parse_grok_usage_events(content: &str) -> Vec<GrokUsageEvent> {
    let mut events = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(record) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if record.get("method").and_then(|v| v.as_str()) != Some("_x.ai/session/update") {
            continue;
        }
        let update = record.get("params").and_then(|p| p.get("update"));
        // Only turn_completed is accepted (in the measurement every event with a usage was of that
        // kind; the discriminating field is sessionUpdate and the serde enum is internally tagged).
        // A missing field is let through for backwards compatibility,
        // but an event that is explicitly marked as another type is not imported even when it has a
        // usage -- if a mid-run snapshot coexists with an end-of-round
        // event, a double import would double count.
        let kind = update
            .and_then(|u| u.get("sessionUpdate"))
            .and_then(|v| v.as_str());
        if kind.is_some() && kind != Some("turn_completed") {
            continue;
        }
        let Some(usage) = update
            .and_then(|u| u.get("usage"))
            .filter(|u| u.is_object())
        else {
            continue;
        };
        // Both the settling window and the takeover guard depend on the event time, so an event without a timestamp cannot be imported safely.
        let Some(created_at) = parse_event_timestamp(record.get("timestamp")) else {
            continue;
        };

        let prompt_id = update
            .and_then(|u| u.get("prompt_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let mut per_model: Vec<(String, GrokCounters)> = usage
            .get("modelUsage")
            .and_then(|m| m.as_object())
            .map(|map| {
                map.iter()
                    .map(|(model, counters)| (model.clone(), parse_grok_counters(counters)))
                    .collect()
            })
            .unwrap_or_default();
        if per_model.is_empty() {
            // Without modelUsage it falls back to the top level per-round values; the model name is unknown and the price lookup layer takes care of it.
            per_model.push(("unknown".to_string(), parse_grok_counters(usage)));
        }
        // modelUsage is a JSON object whose iteration order is not guaranteed to be stable; sorting makes the insertion order deterministic across several rescans of the log.
        per_model.sort_by(|a, b| a.0.cmp(&b.0));

        events.push(GrokUsageEvent {
            created_at,
            prompt_id,
            cost_is_partial: usage
                .get("costIsPartial")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            per_model,
        });
    }

    events
}

fn parse_grok_counters(value: &serde_json::Value) -> GrokCounters {
    let get = |key: &str| value.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
    GrokCounters {
        input: get("inputTokens"),
        output: get("outputTokens"),
        cached: get("cachedReadTokens"),
        api_ms: get("apiDurationMs"),
        model_calls: get("modelCalls"),
        cost_ticks: get("costUsdTicks"),
        cost_partial: value
            .get("costIsPartial")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    }
}

/// The top level `timestamp` of updates.jsonl was measured to be a numeric epoch in seconds
/// (do not confuse it with the RFC3339 string of summary.json); the string form is only a
/// defensive fallback.
fn parse_event_timestamp(value: Option<&serde_json::Value>) -> Option<i64> {
    let value = value?;
    if let Some(n) = value.as_i64() {
        // Guard against the millisecond form of the future: above 1e11 it is treated as milliseconds
        return Some(if n > 100_000_000_000 { n / 1000 } else { n });
    }
    value
        .as_str()
        .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
        .map(|dt| dt.timestamp())
}

/// Insert a single Grok session record into proxy_request_logs
/// It has the same shape as the insert_codex_session_entry_on_conn of Codex: the connection, the pricing cache plus the fields of one row.
#[allow(clippy::too_many_arguments)]
fn insert_grok_session_entry_on_conn(
    conn: &rusqlite::Connection,
    request_id: &str,
    turn: &GrokCounters,
    cost_is_partial: bool,
    model: &str,
    session_id: &str,
    created_at: i64,
    pricing_cache: &mut SessionPricingCache,
) -> Result<bool, AppError> {
    let clamp = |v: u64| v.min(u32::MAX as u64) as u32;
    let usage = TokenUsage {
        input_tokens: clamp(turn.input),
        output_tokens: clamp(turn.output),
        cache_read_tokens: clamp(turn.cached),
        cache_creation_tokens: 0,
        cache_creation_1h_tokens: 0,
        model: Some(model.to_string()),
        message_id: None,
    };

    let pricing = pricing_cache.get(conn, model);
    let multiplier = Decimal::from(1);
    let reported = turn.reported_cost_usd();
    // Only send it after a successful insert (changed), so that a rescan does not spam the log again
    let mut deferred_warn: Option<String> = None;

    // The priority of the total_cost sources (🔴 the backfill mechanism only fills rows with
    // total<=0 and never corrects an existing positive value, see
    // backfill_missing_usage_costs; the UPSERT of this importer does not update a row either just
    // because the cost changed -- so the value has to be right when it is recorded, there is no
    // repair path afterwards):
    // 1. A complete report from the CLI -> the report wins (the upstream ground truth, it is
    //    also accurate inside the pricing drift window;
    //    the local pricing is responsible for the individual items and the drift warning, during a
    //    drift the individual items and the total are allowed to be inconsistent for a while);
    // 2. An incomplete report (costIsPartial) -> with a local price the full amount is recomputed
    //    locally (the token counts are complete),
    //    and the meaningless drift warning is suppressed at that moment; without a price the
    //    reported lower bound is still used (better than recording 0);
    // 3. No report -> recompute locally; only a complete lack of a price records the whole row as 0.
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate_for_app("grokbuild", &usage, p, multiplier);
            let total = match reported {
                Some(reported) if !cost_is_partial => {
                    // A deviation of more than 1% (a minimum of 1e-6) means the local pricing drifted -- the
                    // earliest observable signal when xAI changes its prices, a reminder to update the
                    // seed/repair.
                    let tolerance = (reported * Decimal::new(1, 2)).max(Decimal::new(1, 6));
                    if (cost.total_cost - reported).abs() > tolerance {
                        deferred_warn = Some(format!(
                            "本地定价与 CLI 自报成本偏差超阈值，total 已以自报为准，请更新本地定价: model={model} local={} reported={reported} request_id={request_id}",
                            cost.total_cost
                        ));
                    }
                    reported
                }
                _ => cost.total_cost,
            };
            (
                cost.input_cost.to_string(),
                cost.output_cost.to_string(),
                cost.cache_read_cost.to_string(),
                cost.cache_creation_cost.to_string(),
                total.to_string(),
            )
        }
        None => {
            // A new alias that was not seeded: the tokens are recorded as usual; with a cost reported by
            // the CLI it is used directly (the individual items are
            // 0), only a complete lack of a price records the whole row as 0. The internal aliases of
            // xAI change periodically
            // (grok-4.5-build is the precedent), both cases have to leave a diagnosable trace.
            let total = match reported {
                Some(reported) => {
                    if model != "unknown" {
                        let partial_note = if cost_is_partial {
                            "（上游标记为部分费用，实际为下界）"
                        } else {
                            ""
                        };
                        deferred_warn = Some(format!(
                            "模型定价未找到，采用 CLI 自报成本入账{partial_note}: model={model} total={reported} request_id={request_id}"
                        ));
                    }
                    reported.to_string()
                }
                None => {
                    if model != "unknown" {
                        deferred_warn = Some(format!(
                            "模型定价未找到且无自报成本，成本记 0: model={model} request_id={request_id}"
                        ));
                    }
                    "0".to_string()
                }
            };
            (
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                total,
            )
        }
    };

    // UPSERT: idempotent across rescans; after the parsing semantics were fixed a rescan updates
    // the existing row (tokens/cost/
    // latency; created_at keeps the value of the first insert, so that the row does not drift
    // between the settling window and the rollup boundary).
    // The data_source guard in the WHERE clause is defense in depth: the request_id prefix
    // namespace is already isolated, and even if it collided with a row of another importer that
    // row would never be overwritten.
    // input_token_semantics is written as TOTAL on purpose -- the inputTokens of xAI include the
    // cache read, the same semantics as the grokbuild rows of the proxy path (logger), do not
    // rely on the column default.
    conn.execute(
        "INSERT INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source,
            input_token_semantics
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
        ON CONFLICT(request_id) DO UPDATE SET
            model = excluded.model,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            cache_read_tokens = excluded.cache_read_tokens,
            input_cost_usd = excluded.input_cost_usd,
            output_cost_usd = excluded.output_cost_usd,
            cache_read_cost_usd = excluded.cache_read_cost_usd,
            cache_creation_cost_usd = excluded.cache_creation_cost_usd,
            total_cost_usd = excluded.total_cost_usd,
            latency_ms = excluded.latency_ms
        WHERE data_source = 'grok_session'
          AND (input_tokens != excluded.input_tokens
           OR output_tokens != excluded.output_tokens
           OR cache_read_tokens != excluded.cache_read_tokens
           OR latency_ms != excluded.latency_ms
           OR model != excluded.model)",
        rusqlite::params![
            request_id,
            "_grok_session",     // provider_id
            "grokbuild",         // app_type
            model,
            model,               // request_model = model
            usage.input_tokens,
            usage.output_tokens,
            usage.cache_read_tokens,
            0i64,                // cache_creation_tokens
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            turn.api_ms.min(i64::MAX as u64) as i64, // latency_ms (the API duration of this round)
            Option::<i64>::None, // first_token_ms
            200i64,              // status_code
            Option::<String>::None, // error_message
            session_id,
            Some("grok_session"), // provider_type
            1i64,                // is_streaming
            "1.0",               // cost_multiplier
            created_at,
            "grok_session",      // data_source
            INPUT_TOKEN_SEMANTICS_TOTAL,
        ],
    )
    .map_err(|e| AppError::Database(format!("插入 Grok Build 会话日志失败: {e}")))?;

    // changes() > 0 means a row was inserted or updated, == 0 means the values are exactly the same (no actual change)
    let changed = conn.changes() > 0;
    if changed {
        if let Some(msg) = deferred_warn {
            log::warn!("[GROK-SYNC] {msg}");
        }
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::session_usage::get_sync_state;
    use std::io::Write;
    use tempfile::tempdir;

    /// The fixed reference point that is older than the settling window (2023-11-14T22:13:20Z)
    const OLD_EPOCH: i64 = 1_700_000_000;

    fn epoch_to_rfc3339(epoch: i64) -> String {
        chrono::DateTime::from_timestamp(epoch, 0)
            .expect("valid epoch")
            .to_rfc3339()
    }

    /// The top level timestamp uses the real numeric epoch second format (the RFC3339 fallback is covered by the parses test)
    fn usage_event_line(epoch: i64, prompt_id: &str, model_usage: &str) -> String {
        format!(
            r#"{{"timestamp":{epoch},"method":"_x.ai/session/update","params":{{"update":{{"sessionUpdate":"turn_completed","prompt_id":"{prompt_id}","stop_reason":"end_turn","usage":{{"modelUsage":{{{model_usage}}}}}}}}}}}"#
        )
    }

    /// The variant with the event level costIsPartial marker
    fn usage_event_line_partial(epoch: i64, prompt_id: &str, model_usage: &str) -> String {
        format!(
            r#"{{"timestamp":{epoch},"method":"_x.ai/session/update","params":{{"update":{{"sessionUpdate":"turn_completed","prompt_id":"{prompt_id}","stop_reason":"end_turn","usage":{{"costIsPartial":true,"modelUsage":{{{model_usage}}}}}}}}}}}"#
        )
    }

    fn model_counters(model: &str, input: u64, output: u64, cached: u64, calls: u64) -> String {
        model_counters_with_ticks(model, input, output, cached, calls, 0)
    }

    fn model_counters_with_ticks(
        model: &str,
        input: u64,
        output: u64,
        cached: u64,
        calls: u64,
        ticks: u64,
    ) -> String {
        format!(
            r#""{model}":{{"inputTokens":{input},"outputTokens":{output},"cachedReadTokens":{cached},"reasoningTokens":0,"modelCalls":{calls},"apiDurationMs":1000,"costUsdTicks":{ticks}}}"#
        )
    }

    fn write_session_file(dir: &Path, session_id: &str, lines: &[String]) -> PathBuf {
        let session_dir = dir.join("sessions").join("enc-project").join(session_id);
        std::fs::create_dir_all(&session_dir).expect("create session dir");
        let path = session_dir.join("updates.jsonl");
        let mut file = std::fs::File::create(&path).expect("create updates.jsonl");
        for line in lines {
            writeln!(file, "{line}").expect("write line");
        }
        path
    }

    /// (request_id, input, output, cache_read, input_token_semantics)
    type GrokSessionRow = (String, u32, u32, u32, i64);

    fn query_rows(db: &Database) -> Result<Vec<GrokSessionRow>, AppError> {
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare(
                "SELECT request_id, input_tokens, output_tokens, cache_read_tokens, input_token_semantics
                 FROM proxy_request_logs WHERE data_source = 'grok_session' ORDER BY request_id",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .expect("query")
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    fn query_costs(db: &Database) -> Result<Vec<(String, String)>, AppError> {
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare(
                "SELECT request_id, total_cost_usd FROM proxy_request_logs
                 WHERE data_source = 'grok_session' ORDER BY created_at, request_id",
            )
            .expect("prepare");
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .filter_map(Result::ok)
            .collect();
        Ok(rows)
    }

    #[test]
    fn parses_turn_completed_and_ignores_noise_and_other_kinds() {
        let content = concat!(
            "{\"timestamp\":\"2026-07-20T13:26:10Z\",\"method\":\"session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{}}}}\n",
            "not json at all\n",
            // Explicitly marked as not turn_completed but with a usage: it prevents a double count from a mid-run snapshot and must not be imported
            "{\"timestamp\":\"2026-07-20T13:26:20Z\",\"method\":\"_x.ai/session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"usage_snapshot\",\"prompt_id\":\"px\",\"usage\":{\"inputTokens\":9999,\"outputTokens\":9,\"cachedReadTokens\":0}}}}\n",
            "{\"timestamp\":\"2026-07-20T13:26:24Z\",\"method\":\"_x.ai/session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"turn_completed\",\"prompt_id\":\"p1\",\"usage\":{\"inputTokens\":16632,\"outputTokens\":104,\"cachedReadTokens\":0,\"modelUsage\":{\"grok-4.5-build\":{\"inputTokens\":16632,\"outputTokens\":104,\"cachedReadTokens\":0,\"apiDurationMs\":5342,\"costUsdTicks\":338880000}}}}}}\n",
        );
        let events = parse_grok_usage_events(content);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].prompt_id, "p1");
        assert_eq!(events[0].per_model.len(), 1);
        assert_eq!(events[0].per_model[0].0, "grok-4.5-build");
        assert_eq!(
            events[0].per_model[0].1,
            GrokCounters {
                input: 16632,
                output: 104,
                cached: 0,
                api_ms: 5342,
                model_calls: 0,
                cost_ticks: 338_880_000,
                cost_partial: false,
            }
        );
    }

    #[test]
    fn missing_model_usage_falls_back_to_top_level_counters() {
        // Covered as well: a missing sessionUpdate field is let through for backwards compatibility
        let line = format!(
            r#"{{"timestamp":"{}","method":"_x.ai/session/update","params":{{"update":{{"prompt_id":"p1","usage":{{"inputTokens":100,"outputTokens":10,"cachedReadTokens":5}}}}}}}}"#,
            epoch_to_rfc3339(OLD_EPOCH)
        );
        let events = parse_grok_usage_events(&line);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].per_model[0].0, "unknown");
        assert_eq!(events[0].per_model[0].1.input, 100);
    }

    #[test]
    fn two_turns_import_at_face_value_matching_reported_ticks() -> Result<(), AppError> {
        use std::str::FromStr;
        // The original values of the 2026-07-23 measurement with two prompts in one process: a
        // turn_completed is an independent total per round.
        // If the difference of the neighbouring events were used by mistake, the second round would
        // be recorded as a fake delta of 53/28/6144 (it was done before).
        // The ticks of every round pin down the per-round semantics and the 2/6/0.30 pricing at the
        // same time:
        //   round 1 (17294-11136)x2 + 11136x0.30 + 28x6 = 15824.8 uUSD = 158248000 ticks
        //   round 2 (17347-17280)x2 + 17280x0.30 + 56x6 =  5654.0 uUSD =  56540000 ticks
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters_with_ticks("grok-4.5-build", 17294, 28, 11136, 1, 158_248_000),
            ),
            usage_event_line(
                OLD_EPOCH + 60,
                "p2",
                &model_counters_with_ticks("grok-4.5-build", 17347, 56, 17280, 1, 56_540_000),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-two-turns", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 2);
        assert_eq!(result.deferred_files, 0);

        let rows = query_rows(&db)?;
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].1, rows[0].2, rows[0].3), (17294, 28, 11136));
        assert_eq!((rows[1].1, rows[1].2, rows[1].3), (17347, 56, 17280));
        // The semantics column is explicitly TOTAL, the same as on the proxy path
        assert!(rows.iter().all(|r| r.4 == INPUT_TOKEN_SEMANTICS_TOTAL));

        // The local recomputation has to match the ticks reported by the CLI to the last digit (the drift warning stays silent below this threshold)
        let costs = query_costs(&db)?;
        let expected1 = Decimal::from(158_248_000u64) / Decimal::from(10_000_000_000u64);
        let expected2 = Decimal::from(56_540_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(Decimal::from_str(&costs[0].1).expect("decimal"), expected1);
        assert_eq!(Decimal::from_str(&costs[1].1).expect("decimal"), expected2);
        Ok(())
    }

    #[test]
    fn second_turn_with_smaller_counters_imports_at_face_value() -> Result<(), AppError> {
        // The original values of the 2026-07-23 measurement across processes (process A with a single
        // round 27386/74/15360, the --resume process B with a single round 13793/21/13696). Under the
        // per-round semantics "the second round is smaller" is normal,
        // no matter whether it crosses processes, and every round is recorded at its face value.
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 27386, 74, 15360, 2),
            ),
            usage_event_line(
                OLD_EPOCH + 15,
                "p2",
                &model_counters("grok-4.5-build", 13793, 21, 13696, 1),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-resume", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 2);

        let rows = query_rows(&db)?;
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].1, rows[0].2, rows[0].3), (27386, 74, 15360));
        assert_eq!((rows[1].1, rows[1].2, rows[1].3), (13793, 21, 13696));
        Ok(())
    }

    #[test]
    fn identical_turns_both_import() -> Result<(), AppError> {
        // Regression (per-round semantics): two rounds with exactly the same numbers mean two real
        // usages, both have to be recorded.
        // The difference semantics would skip the second round as a zero delta -- that is exactly the
        // old behaviour that was disproved.
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
            usage_event_line(
                OLD_EPOCH + 60,
                "p2",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-identical", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 2, "相同数值的两轮都是真实用量");
        assert_eq!(query_rows(&db)?.len(), 2);
        Ok(())
    }

    #[test]
    fn multi_model_event_produces_row_per_model() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let both = format!(
            "{},{}",
            model_counters("grok-4.5-build", 100, 10, 0, 1),
            model_counters("grok-4.3", 30, 3, 0, 1)
        );
        let lines = vec![usage_event_line(OLD_EPOCH, "p1", &both)];
        let path = write_session_file(temp.path(), "sess-multi", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 2);
        let rows = query_rows(&db)?;
        assert!(rows[0].0.ends_with(":grok-4.3"));
        assert!(rows[1].0.ends_with(":grok-4.5-build"));
        Ok(())
    }

    #[test]
    fn settle_window_defers_recent_events_without_recording_sync_state() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("now")
            .as_secs() as i64;
        let lines = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
            // A new event that has not settled: it is postponed for this round and no sync state is recorded so that the next round re-reads the file
            usage_event_line(now, "p2", &model_counters("grok-4.5-build", 250, 30, 0, 1)),
        ];
        let path = write_session_file(temp.path(), "sess-settle", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);
        assert_eq!(result.deferred_files, 1);
        assert_eq!(query_rows(&db)?.len(), 1);

        let (last_modified, _) = get_sync_state(&db, &path.to_string_lossy())?;
        assert_eq!(last_modified, 0, "延后时不得记录同步状态");

        // The next round re-reads: the old events do not change through the UPSERT, the new events still have not settled and are postponed again
        let rerun = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(rerun.imported, 0);
        assert_eq!(rerun.skipped, 1);
        assert_eq!(rerun.deferred_files, 1);
        assert_eq!(query_rows(&db)?.len(), 1);
        Ok(())
    }

    fn unix_now() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("now")
            .as_secs() as i64
    }

    #[test]
    fn settled_event_imports_without_waiting_for_dedup_window() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        // A round that ended a minute and a half ago (far before the 10 minutes of the guard window)
        // should already be imported;
        // this test uses a fixed offset instead of the settling window, so it only fails when the
        // settling window is turned back to a large value
        let event_at = unix_now() - 90;
        let lines = vec![usage_event_line(
            event_at,
            "p1",
            &model_counters("grok-4.5-build", 100, 10, 0, 1),
        )];
        let path = write_session_file(temp.path(), "sess-settled", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);
        assert_eq!(result.deferred_files, 0);
        assert_eq!(query_rows(&db)?.len(), 1);
        Ok(())
    }

    #[test]
    fn proxy_row_landing_after_event_still_blocks_import() -> Result<(), AppError> {
        let db = Database::memory()?;
        let event_at = unix_now() - SETTLE_WINDOW_SECONDS - 5;
        {
            // The takeover state: the proxy rows land a few seconds after the turn_completed
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "grok-proxy-late",
                    "some-provider",
                    "grokbuild",
                    "grok-4.5",
                    "grok-4.5",
                    100,
                    10,
                    0,
                    0,
                    "0.01",
                    100,
                    200,
                    event_at + 3,
                    "proxy"
                ],
            )?;
        }
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            event_at,
            "p1",
            &model_counters("grok-4.5-build", 100, 10, 0, 1),
        )];
        let path = write_session_file(temp.path(), "sess-late-proxy", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1, "代理行已记账，会话事件不得再入账");
        assert!(query_rows(&db)?.is_empty());
        Ok(())
    }

    #[test]
    fn takeover_guard_skips_events_near_proxy_activity() -> Result<(), AppError> {
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "grok-proxy-req",
                    "some-provider",
                    "grokbuild",
                    "grok-4.5",
                    "grok-4.5",
                    999,
                    88,
                    0,
                    0,
                    "0.01",
                    100,
                    200,
                    OLD_EPOCH + 30,
                    "proxy"
                ],
            )?;
        }
        let temp = tempdir().expect("tempdir");
        let lines = vec![
            // The event time falls within the window around a proxy row -> takeover state, skip it (the proxy rows are authoritative)
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
            // A later event that is far away from the takeover window is imported normally at its face value
            usage_event_line(
                OLD_EPOCH + SESSION_PROXY_DEDUP_WINDOW_SECONDS + 3600,
                "p2",
                &model_counters("grok-4.5-build", 250, 30, 0, 1),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-guard", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.skipped, 1, "守卫跳过计入 skipped（未入账）");
        assert_eq!(result.imported, 1);

        let rows = query_rows(&db)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 250, "守卫外事件按本轮面值入账");
        Ok(())
    }

    #[test]
    fn rescan_is_idempotent() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
            usage_event_line(
                OLD_EPOCH + 60,
                "p2",
                &model_counters("grok-4.5-build", 250, 30, 50, 1),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-idem", &lines);

        let first = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(first.imported, 2);

        // The mtime did not change -> short circuit
        let second = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(second.imported + second.skipped, 0);

        // A forced re-read (clearing the sync state) -> the UPSERT does not change anything
        {
            let conn = lock_conn!(db.conn);
            conn.execute("DELETE FROM session_log_sync", [])?;
        }
        let third = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(third.imported, 0);
        assert_eq!(third.skipped, 2);
        assert_eq!(query_rows(&db)?.len(), 2);
        Ok(())
    }

    #[test]
    fn rewind_truncation_does_not_double_count() -> Result<(), AppError> {
        // Regression (found during a review): if the idempotency key contained the index inside the
        // file, a rewritten prefix of updates.jsonl
        // (a rewind truncation) would shift the surviving event indices forward, generate a new
        // request_id and double count.
        // With the key anchored to the prompt_id: a surviving round hits its original row; the rows of
        // the removed rounds are kept (a rewind does not
        // give back the consumed tokens, so keeping them is correct).
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let full = vec![
            usage_event_line(
                OLD_EPOCH,
                "p1",
                &model_counters("grok-4.5-build", 100, 10, 0, 1),
            ),
            usage_event_line(
                OLD_EPOCH + 60,
                "p2",
                &model_counters("grok-4.5-build", 200, 20, 0, 1),
            ),
            usage_event_line(
                OLD_EPOCH + 120,
                "p3",
                &model_counters("grok-4.5-build", 300, 30, 0, 1),
            ),
        ];
        let path = write_session_file(temp.path(), "sess-rewind", &full);
        assert_eq!(
            sync_single_grok_file(
                &db,
                &path,
                &crate::services::session_usage::load_sync_cursors(&db).unwrap()
            )?
            .imported,
            3
        );

        // Simulate a rewind that cuts off p2: p3 moves forward from idx2 to idx1
        let truncated = vec![full[0].clone(), full[2].clone()];
        write_session_file(temp.path(), "sess-rewind", &truncated);
        {
            let conn = lock_conn!(db.conn);
            conn.execute("DELETE FROM session_log_sync", [])?;
        }

        let rescan = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(rescan.imported, 0, "幸存轮不得因序号前移重新入账");

        let rows = query_rows(&db)?;
        assert_eq!(rows.len(), 3, "被截掉轮次的行保留（token 已实际消耗）");
        let p3: Vec<_> = rows.iter().filter(|r| r.0.contains(":p3:")).collect();
        assert_eq!(p3.len(), 1);
        assert_eq!(p3[0].1, 300);
        Ok(())
    }

    #[test]
    fn empty_prompt_id_falls_back_to_index_key() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            OLD_EPOCH,
            "",
            &model_counters("grok-4.5-build", 100, 10, 0, 1),
        )];
        let path = write_session_file(temp.path(), "sess-noprompt", &lines);

        assert_eq!(
            sync_single_grok_file(
                &db,
                &path,
                &crate::services::session_usage::load_sync_cursors(&db).unwrap()
            )?
            .imported,
            1
        );
        let rows = query_rows(&db)?;
        assert!(rows[0].0.contains(":idx0:"), "空 prompt_id 回退序号键");
        Ok(())
    }

    #[test]
    fn cost_matches_cli_reported_ticks_for_seeded_grok45_build() -> Result<(), AppError> {
        use std::str::FromStr;
        // A real sample: inputTokens=16632, outputTokens=104, cache=0,
        // costUsdTicks=338880000 (1 tick = 1e-10 USD). The seeded grok-4.5-build
        // pricing (2/6) has to reproduce the cost reported by the CLI exactly. The fixture
        // deliberately has no ticks,
        // what is verified is the independent recomputation from the local pricing.
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            OLD_EPOCH,
            "p1",
            &model_counters("grok-4.5-build", 16632, 104, 0, 1),
        )];
        let path = write_session_file(temp.path(), "sess-ticks", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let total: String = conn.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE data_source = 'grok_session'",
            [],
            |row| row.get(0),
        )?;
        let expected = Decimal::from(338_880_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(Decimal::from_str(&total).expect("decimal"), expected);
        Ok(())
    }

    #[test]
    fn cost_matches_cli_reported_ticks_with_cache_reads() -> Result<(), AppError> {
        use std::str::FromStr;
        // The sample with a cache measured on 2026-07-23: 13793/21/13696, costUsdTicks=44288000.
        // The measured cache read unit price of 0.30 is pinned down: billable_input=(13793-13696)x2/1M
        // + 21x6/1M + 13696x0.30/1M = 0.0044288. This test goes red if the seed is changed back to 0.50.
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            OLD_EPOCH,
            "p1",
            &model_counters("grok-4.5-build", 13793, 21, 13696, 1),
        )];
        let path = write_session_file(temp.path(), "sess-ticks-cache", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let total: String = conn.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE data_source = 'grok_session'",
            [],
            |row| row.get(0),
        )?;
        let expected = Decimal::from(44_288_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(Decimal::from_str(&total).expect("decimal"), expected);
        Ok(())
    }

    #[test]
    fn reported_ticks_override_stale_local_pricing() -> Result<(), AppError> {
        use std::str::FromStr;
        // The pricing drift window: the CLI reports twice what the local recomputation gives
        // (338880000 ticks), simulating that
        // xAI changed its prices while the seed was not updated. The total has to follow the report (the
        // backfill does not correct rows with a positive value, so a wrong
        // local price stays wrong forever); the individual items still follow the local price (temporarily
        // inconsistent, with the drift warning pointing it out).
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            OLD_EPOCH,
            "p1",
            &model_counters_with_ticks("grok-4.5-build", 16632, 104, 0, 1, 677_760_000),
        )];
        let path = write_session_file(temp.path(), "sess-drift", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, total): (String, String) = conn.query_row(
            "SELECT input_cost_usd, total_cost_usd FROM proxy_request_logs
             WHERE data_source = 'grok_session'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let expected_total = Decimal::from(677_760_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(
            Decimal::from_str(&total).expect("decimal"),
            expected_total,
            "total 以自报为准"
        );
        assert!(
            Decimal::from_str(&input_cost).expect("decimal") > Decimal::ZERO,
            "分项仍按本地定价"
        );
        Ok(())
    }

    #[test]
    fn partial_reported_cost_prefers_local_pricing_when_priced() -> Result<(), AppError> {
        use std::str::FromStr;
        // costIsPartial=true: the report is only a lower bound and must not be used as the total. The token
        // counts are complete,
        // with a local price the full amount is recomputed locally (here it should give the equivalent of
        // 338880000 ticks).
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line_partial(
            OLD_EPOCH,
            "p1",
            &model_counters_with_ticks("grok-4.5-build", 16632, 104, 0, 1, 1_000),
        )];
        let path = write_session_file(temp.path(), "sess-partial", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let total: String = conn.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE data_source = 'grok_session'",
            [],
            |row| row.get(0),
        )?;
        let expected = Decimal::from(338_880_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(Decimal::from_str(&total).expect("decimal"), expected);
        Ok(())
    }

    #[test]
    fn unpriced_model_falls_back_to_reported_ticks() -> Result<(), AppError> {
        use std::str::FromStr;
        // A new alias that was not seeded: total_cost takes the ticks reported by the CLI (the individual
        // items are 0),
        // the whole row is no longer recorded as 0.
        let db = Database::memory()?;
        let temp = tempdir().expect("tempdir");
        let lines = vec![usage_event_line(
            OLD_EPOCH,
            "p1",
            &model_counters_with_ticks("grok-6-future-alias", 1000, 100, 0, 1, 56_540_000),
        )];
        let path = write_session_file(temp.path(), "sess-unpriced", &lines);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )?;
        assert_eq!(result.imported, 1);

        let conn = lock_conn!(db.conn);
        let (input_cost, total): (String, String) = conn.query_row(
            "SELECT input_cost_usd, total_cost_usd FROM proxy_request_logs
             WHERE data_source = 'grok_session'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(
            Decimal::from_str(&input_cost).expect("decimal"),
            Decimal::ZERO
        );
        let expected = Decimal::from(56_540_000u64) / Decimal::from(10_000_000_000u64);
        assert_eq!(Decimal::from_str(&total).expect("decimal"), expected);
        Ok(())
    }

    #[test]
    fn oversized_updates_jsonl_is_skipped_without_reading_into_memory() {
        let db = Database::memory().expect("memory db");
        let temp = tempdir().expect("tempdir");
        let path = write_session_file(temp.path(), "sess-huge", &[]);

        // Create a file that is larger than 50 MiB, but with empty content (it will not be parsed).
        let huge = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("open");
        huge.set_len(MAX_GROK_FILE_BYTES + 1).expect("set_len");
        drop(huge);

        let result = sync_single_grok_file(
            &db,
            &path,
            &crate::services::session_usage::load_sync_cursors(&db).unwrap(),
        )
        .expect("sync should not fail");
        assert_eq!(result.imported, 0, "oversized file must not be imported");
        assert_eq!(result.skipped, 0);
        assert_eq!(result.deferred_files, 0);
    }

    #[test]
    fn symlink_cycle_does_not_cause_stack_overflow() {
        let temp = tempdir().expect("tempdir");
        let sessions = temp.path().join("sessions");
        let enc = sessions.join("enc-project");
        let sub = enc.join("sub");
        std::fs::create_dir_all(&sub).expect("create dirs");

        // Build a cycle: sub/cycle -> the enc parent directory
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&enc, sub.join("cycle"));
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&enc, sub.join("cycle"));
        if let Err(err) = linked {
            // Windows cannot create symlinks on \\wsl.localhost (Incorrect function);
            // a normal Windows account without developer mode also has no permission to create one
            // (ERROR_PRIVILEGE_NOT_HELD = 1314). The fixture cannot be constructed in either case.
            let no_privilege = cfg!(windows) && err.raw_os_error() == Some(1314);
            assert!(
                no_privilege || crate::config::is_wsl_path(temp.path()),
                "symlink: {err}"
            );
            eprintln!("cannot create symlinks here ({err}); skipping");
            return;
        }

        // Also put a real target file in there to confirm that the normal traversal still works
        std::fs::write(enc.join("updates.jsonl"), b"{}\n").expect("write real file");

        let mut files = Vec::new();
        collect_files_named(&sessions, "updates.jsonl", &mut files, 0);

        assert_eq!(
            files.len(),
            1,
            "only the real updates.jsonl should be collected; symlink cycle must not crash"
        );
    }
}
