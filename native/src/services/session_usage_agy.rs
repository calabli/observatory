//! Exact counters exported by agy; absent models remain unknown (never use today's provider).
//! Amounts are the matching Gemini API prices, like every other session-log import.
use super::{
    session_usage::{estimated_latency_ms, SessionSyncResult},
    sql_helpers::INPUT_TOKEN_SEMANTICS_FRESH,
};
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::{
    database::{lock_conn, Database},
    error::AppError,
    session_manager::providers::agy,
};
use rusqlite::params;
#[cfg(test)]
use serde_json::Value;

// Reserved session_log_sync row; triggers one rescan of existing agy files to backfill latency_ms.
const LATENCY_BACKFILL_KEY: &str = "agy:latency-backfill-v1";

pub fn sync_agy_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let mut result = SessionSyncResult::default();
    let mut cursors = super::session_usage::load_sync_cursors(db)?;
    let needs_latency_backfill = !cursors.contains_key(LATENCY_BACKFILL_KEY);
    if needs_latency_backfill {
        cursors.retain(|k, _| !k.starts_with("agy:"));
    }
    for (session_id, path) in agy::scan_transcript_files() {
        // The historical session files do not change any more: an mtime+size gate skips the unchanged
        // files, which avoids reading and parsing everything again in every round (the same shape as
        // the DSH gate). The cursor is advanced and the import is committed atomically in one
        // transaction.
        let key = format!("agy:{}", path.display());
        let metadata = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis().min(i64::MAX as u128) as i64)
            .unwrap_or(0);
        let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
        if modified != 0
            && cursors
                .get(&key)
                .is_some_and(|c| c.last_modified == modified && c.last_line_offset == size)
        {
            continue;
        }
        result.files_scanned += 1;
        let imported_ok = match agy::usage_steps(&path) {
            Ok(steps) => match import_steps(db, &session_id, &steps) {
                Ok((imported, skipped)) => {
                    result.imported += imported;
                    result.skipped += skipped;
                    true
                }
                Err(e) => {
                    result
                        .errors
                        .push(format!("Antigravity {}: {e}", session_id));
                    false
                }
            },
            Err(e) => {
                result.errors.push(e);
                false
            }
        };
        // agy rewrites the steps while exporting: the cursor is only recorded when the file is still
        // identical to the one that was read; otherwise the file is left for a full re-import in the
        // next round and the tombstones plus the UPDATE semantics correct it on their own (the same
        // as for DSH).
        if imported_ok {
            let stable = std::fs::metadata(&path).ok().is_some_and(|m| {
                m.len() as i64 == size && m.modified().ok() == metadata.modified().ok()
            });
            if stable {
                let conn = lock_conn!(db.conn);
                super::session_usage::update_sync_state_on_conn(&conn, &key, modified, size)?;
            }
        }
    }
    if needs_latency_backfill && result.errors.is_empty() {
        let conn = lock_conn!(db.conn);
        super::session_usage::update_sync_state_on_conn(&conn, LATENCY_BACKFILL_KEY, 0, 0)?;
    }
    Ok(result)
}

/// agy writes the model switch into the session body:
/// `changed setting `Model Selection` from X to Y`.
/// The responses themselves have no model field, so inheriting the most recent switch in the
/// order of the steps restores the model of that moment.
fn model_from_settings_change(content: &str) -> Option<String> {
    let start = content.find("Model Selection")?;
    let rest = &content[start..];
    let to = rest.find(" to ")? + 4;
    let tail = &rest[to..];
    // The model name contains dots itself (Gemini 3.8), so only a "period plus space" is treated as the end of a sentence.
    let end = tail
        .find(". ")
        .or_else(|| tail.find('\n'))
        .or_else(|| tail.find('`'))
        .unwrap_or(tail.len());
    let model = tail[..end].trim().trim_matches('.').trim();
    if model.is_empty() || model.eq_ignore_ascii_case("none") {
        None
    } else {
        Some(model.to_string())
    }
}

/// What agy records is the display name (`Gemini 3.8 Flash (High)`); it only matches the pricing
/// table after being converted into an ID like the first column of `agy models`
/// (`gemini-3.8-flash-high`). The tier suffix is stripped during the price lookup. An ID is
/// returned unchanged.
fn model_id_of(display: &str) -> String {
    if !display.contains(' ') {
        return display.to_string();
    }
    display
        .to_ascii_lowercase()
        .replace(" (", "-")
        .replace(['(', ')'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// The five amounts (input, output, cache read, cache write, total); a model without a price
/// is recorded as 0.
///
/// The pricing is passed in by the caller, cached per round: in the same round hundreds of
/// rows look up the same pricing table over and over and a per-row `find_model_pricing` is a
/// full-table scan blown up for nothing.
fn costs(
    pricing: Option<&crate::proxy::usage::calculator::ModelPricing>,
    model: &str,
    tokens: [i64; 4],
) -> [String; 5] {
    let [input, output, cache_read, cache_write] =
        tokens.map(|v| u32::try_from(v).unwrap_or(u32::MAX));
    let usage = TokenUsage {
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_creation_tokens: cache_write,
        cache_creation_1h_tokens: 0,
        model: Some(model.to_string()),
        message_id: None,
    };
    match pricing {
        Some(pricing) => {
            let cost = CostCalculator::calculate_for_app(
                "agy",
                &usage,
                pricing,
                rust_decimal::Decimal::from(1),
            );
            [
                cost.input_cost.to_string(),
                cost.output_cost.to_string(),
                cost.cache_read_cost.to_string(),
                cost.cache_creation_cost.to_string(),
                cost.total_cost.to_string(),
            ]
        }
        None => std::array::from_fn(|_| "0".to_string()),
    }
}

/// Antigravity records `created_at` (step start, second precision) rather than completion time.
/// For intermediate `PLANNER_RESPONSE` steps that invoke tools, the immediately following tool
/// execution step is created right after the model finishes, so `next.created_at - curr.created_at`
/// approximates the request duration (including TTFT). Final replies without `tool_calls` are
/// followed by `USER_INPUT` (human wait time) or EOF and therefore have no reliable duration.
fn is_tool_followup(step: &agy::UsageStep) -> bool {
    step.step_type != "USER_INPUT"
        && step.step_type != "PLANNER_RESPONSE"
        && step.step_type != "SYSTEM_MESSAGE"
        && step.source.as_deref() != Some("USER_EXPLICIT")
        && step.source.as_deref() != Some("SYSTEM")
}

fn import_steps(
    db: &Database,
    session: &str,
    steps: &[agy::UsageStep],
) -> Result<(u32, u32), AppError> {
    // `usage_steps` already returns steps sorted by `step_index`, but sort defensively for callers
    // that pass arbitrary slices (such as unit tests).
    let mut ordered: Vec<&agy::UsageStep> = steps.iter().collect();
    ordered.sort_by_key(|step| step.step_index);
    let mut current_model: Option<String> = None;
    let mut model_by_step = std::collections::HashMap::<u64, String>::new();
    let mut latency_by_step = std::collections::HashMap::<u64, i64>::new();
    for (idx, step) in ordered.iter().enumerate() {
        if step.step_type == "USER_INPUT" {
            if let Some(model) = step.content.as_deref().and_then(model_from_settings_change) {
                current_model = Some(model);
            }
        } else if step.step_type == "PLANNER_RESPONSE" {
            if let Some(model) = &current_model {
                model_by_step
                    .entry(step.step_index)
                    .or_insert_with(|| model.clone());
            }
            if step.status.as_deref() == Some("DONE") && step.has_tool_calls {
                if let Some(next) = ordered.get(idx + 1).filter(|next| {
                    Some(next.step_index) == step.step_index.checked_add(1)
                        && is_tool_followup(next)
                }) {
                    if let (Some(start_ms), Some(end_ms)) = (step.created_at_ms, next.created_at_ms)
                    {
                        if let Some(latency_ms) = estimated_latency_ms(start_ms, end_ms) {
                            latency_by_step.insert(step.step_index, latency_ms);
                        }
                    }
                }
            }
        }
    }

    let mut conn = lock_conn!(db.conn);
    let tx = conn.transaction()?;
    let (mut imported, mut skipped) = (0, 0);
    // There are only a few kinds of models in a round: cache the price lookups per round to avoid a pricing table scan per row.
    let mut pricing_cache = std::collections::HashMap::<
        String,
        Option<crate::proxy::usage::calculator::ModelPricing>,
    >::new();
    for row in ordered {
        if row.step_type != "PLANNER_RESPONSE" || row.status.as_deref() != Some("DONE") {
            continue;
        }
        let step = row.step_index;
        let input = row.input_tokens;
        let output = row.output_tokens;
        let cache = row.cache_read_tokens;
        let write = row.cache_write_tokens;
        if input == 0 && output == 0 && cache == 0 && write == 0 {
            continue;
        }
        let id = format!("agy:{session}:{step}");
        // Persist a tombstone with the row so retention never imports pruned history again.
        let fresh = tx.execute("INSERT OR IGNORE INTO session_usage_dedup(data_source,request_id,semantic_id,has_entry_id) VALUES ('agy_session',?1,?1,1)", [&id])?;
        // `model` stores the ID (it matches the pricing and the statistics of the other apps), `request_model` keeps the display name.
        let display = row
            .model
            .clone()
            .or_else(|| model_by_step.get(&step).cloned())
            .unwrap_or_else(|| "unknown".to_string());
        let model = model_id_of(&display);
        let pricing = pricing_cache
            .entry(model.clone())
            .or_insert_with(|| crate::services::usage_stats::find_model_pricing(&tx, &model))
            .as_ref();
        let [input_cost, output_cost, cache_cost, write_cost, total_cost] =
            costs(pricing, &model, [input, output, cache, write]);
        let latency_ms = latency_by_step.get(&step).copied().unwrap_or(0);
        if fresh == 0 {
            // A rewritten completed step may correct counters; update retained detail only.
            // If retention already removed it, its tombstone still forbids resurrection.
            // Never overwrite a positive latency_ms back to 0 on a partial/truncated read.
            let changed = tx.execute("UPDATE proxy_request_logs SET model=?2,request_model=?7,input_tokens=?3,output_tokens=?4,cache_read_tokens=?5,cache_creation_tokens=?6,input_cost_usd=?8,output_cost_usd=?9,cache_read_cost_usd=?10,cache_creation_cost_usd=?11,total_cost_usd=?12,latency_ms=CASE WHEN ?13>0 THEN ?13 ELSE latency_ms END WHERE request_id=?1 AND (model!=?2 OR request_model!=?7 OR input_tokens!=?3 OR output_tokens!=?4 OR cache_read_tokens!=?5 OR cache_creation_tokens!=?6 OR total_cost_usd!=?12 OR (?13>0 AND latency_ms!=?13))", params![id,model,input,output,cache,write,display,input_cost,output_cost,cache_cost,write_cost,total_cost,latency_ms])?;
            imported += changed as u32;
            skipped += u32::from(changed == 0);
            continue;
        }
        // Native agy was verified with a local mock: input excludes cache, output includes
        // thinking. Never subtract cache or add thinking a second time. Missing historical
        // models remain unknown and unpriced; never fall back to the active provider.
        tx.execute("INSERT INTO proxy_request_logs (
            request_id,provider_id,app_type,model,request_model,input_tokens,output_tokens,
            cache_read_tokens,cache_creation_tokens,input_cost_usd,output_cost_usd,
            cache_read_cost_usd,cache_creation_cost_usd,total_cost_usd,latency_ms,status_code,
            session_id,provider_type,is_streaming,cost_multiplier,created_at,data_source,input_token_semantics
        ) VALUES (?1,'_agy_session','agy',?2,?10,?3,?4,?5,?6,?11,?12,?13,?14,?15,?16,200,?7,'agy_session',1,'1',?8,'agy_session',?9)",
        params![id, model, input, output, cache, write, session, row.created_at_ms.unwrap_or(0)/1000, INPUT_TOKEN_SEMANTICS_FRESH, display, input_cost, output_cost, cache_cost, write_cost, total_cost, latency_ms])?;
        imported += 1;
    }
    tx.commit()?;
    Ok((imported, skipped))
}

#[cfg(test)]
fn import_records(db: &Database, session: &str, rows: &[Value]) -> Result<(u32, u32), AppError> {
    let steps: Vec<agy::UsageStep> = rows
        .iter()
        .filter_map(|row| {
            let step_index = row["step_index"].as_u64()?;
            Some(agy::UsageStep {
                step_index,
                step_type: row["type"].as_str().unwrap_or_default().to_string(),
                source: row["source"].as_str().map(str::to_string),
                status: row["status"].as_str().map(str::to_string),
                created_at_ms: agy::timestamp(row),
                model: row["model"].as_str().map(str::to_string),
                input_tokens: row["input_tokens"]
                    .as_u64()
                    .and_then(|v| i64::try_from(v).ok())
                    .unwrap_or(0),
                output_tokens: row["output_tokens"]
                    .as_u64()
                    .and_then(|v| i64::try_from(v).ok())
                    .unwrap_or(0),
                cache_read_tokens: row["cache_read_tokens"]
                    .as_u64()
                    .and_then(|v| i64::try_from(v).ok())
                    .unwrap_or(0),
                cache_write_tokens: row["cache_write_tokens"]
                    .as_u64()
                    .and_then(|v| i64::try_from(v).ok())
                    .unwrap_or(0),
                has_tool_calls: row["tool_calls"]
                    .as_array()
                    .is_some_and(|calls| !calls.is_empty()),
                content: row["content"].as_str().map(str::to_string),
            })
        })
        .collect();
    import_steps(db, session, &steps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn agy_native_rows_import_finished_planner_responses_as_fresh_input() {
        let db = Database::memory().unwrap();
        // The original fixture file (tests/fixtures/sessions/agy.jsonl) was not carried over with the
        // migration, so an equivalent native log is inlined here: only the DONE PLANNER_RESPONSE
        // counts, the output_tokens already include the thinking output and the input_tokens are the
        // fresh input without the cache.
        let rows: Vec<Value> = [
            r#"{"step_index":0,"type":"USER_INPUT","status":"DONE","content":"hi"}"#,
            r#"{"step_index":1,"type":"PLANNER_RESPONSE","status":"RUNNING","input_tokens":80}"#,
            r#"{"step_index":2,"type":"PLANNER_RESPONSE","status":"DONE","input_tokens":80,"output_tokens":12,"cache_read_tokens":20}"#,
            r#"{"step_index":3,"type":"TOOL_CALL","status":"DONE","input_tokens":999}"#,
        ]
        .iter()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
        assert_eq!(import_records(&db, "fixture", &rows).unwrap(), (1, 0));
        let conn = db.conn.lock().unwrap();
        let got=conn.query_row("SELECT input_tokens,output_tokens,cache_read_tokens,input_token_semantics FROM proxy_request_logs",[],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,i64>(3)?))).unwrap();
        assert_eq!(got, (80, 12, 20, INPUT_TOKEN_SEMANTICS_FRESH));
    }

    #[test]
    fn agy_rewritten_counters_update_retained_rows_without_reimporting_pruned_rows() {
        let db = Database::memory().unwrap();
        let mut row =
            json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","input_tokens":10});
        assert_eq!(import_records(&db, "s", &[row.clone()]).unwrap(), (1, 0));
        row["input_tokens"] = json!(20);
        assert_eq!(import_records(&db, "s", &[row.clone()]).unwrap(), (1, 0));
        assert_eq!(import_records(&db, "s", &[row]).unwrap(), (0, 1));
        assert_eq!(
            db.conn
                .lock()
                .unwrap()
                .query_row("SELECT input_tokens FROM proxy_request_logs", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            20
        );
    }

    #[test]
    fn agy_usage_is_exact_deduplicated_and_survives_retention() {
        let db = Database::memory().unwrap();
        let rows = vec![
            json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","input_tokens":11830,"output_tokens":137,"cache_read_tokens":25}),
        ];
        assert_eq!(import_records(&db, "session", &rows).unwrap(), (1, 0));
        assert_eq!(import_records(&db, "session", &rows).unwrap(), (0, 1));
        let conn = db.conn.lock().unwrap();
        let got = conn.query_row("SELECT input_tokens,output_tokens,cache_read_tokens,model FROM proxy_request_logs WHERE app_type='agy'", [], |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?, r.get::<_,String>(3)?))).unwrap();
        assert_eq!(got, (11830, 137, 25, "unknown".into()));
        conn.execute("DELETE FROM proxy_request_logs", []).unwrap();
        drop(conn);
        assert_eq!(import_records(&db, "session", &rows).unwrap(), (0, 1));
        assert_eq!(import_records(&db, "other", &rows).unwrap(), (1, 0));
    }
    #[test]
    fn model_selection_change_labels_later_responses() {
        let db = Database::memory().unwrap();
        let rows = [
            json!({"step_index":0,"type":"USER_INPUT","status":"DONE","content":"The user changed setting `Model Selection` from None to Gemini 3.8 Flash (High). No need to comment"}),
            json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","input_tokens":10,"output_tokens":5}),
        ];
        assert_eq!(import_records(&db, "models", &rows).unwrap(), (1, 0));
        let conn = db.conn.lock().unwrap();
        let model: String = conn
            .query_row(
                "SELECT model FROM proxy_request_logs WHERE app_type='agy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(model, "gemini-3.8-flash-high");
    }

    #[test]
    fn display_names_map_to_priced_model_ids() {
        assert_eq!(
            model_id_of("Gemini 3.8 Flash (High)"),
            "gemini-3.8-flash-high"
        );
        assert_eq!(model_id_of("Gemini 4 Argon (Low)"), "gemini-4-argon-low");
        assert_eq!(model_id_of("gemini-3.8-flash"), "gemini-3.8-flash");
        assert_eq!(model_id_of("unknown"), "unknown");

        let db = Database::memory().unwrap();
        let rows = [
            json!({"step_index":0,"type":"USER_INPUT","status":"DONE","content":"The user changed setting `Model Selection` from None to Gemini 3.8 Flash (High). No need to comment"}),
            json!({"step_index":1,"type":"PLANNER_RESPONSE","status":"DONE","input_tokens":1_000_000,"output_tokens":0}),
            json!({"step_index":2,"type":"PLANNER_RESPONSE","status":"DONE","model":"Gemini 3.1 Pro (High)","input_tokens":1_000_000}),
            json!({"step_index":3,"type":"PLANNER_RESPONSE","status":"DONE","model":"Mystery Model","input_tokens":5}),
        ];
        assert_eq!(import_records(&db, "priced", &rows).unwrap(), (3, 0));
        let conn = db.conn.lock().unwrap();
        let price = |model: &str| -> (String, String) {
            conn.query_row(
                "SELECT request_model, total_cost_usd FROM proxy_request_logs WHERE model=?1",
                [model],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        };
        let (display, flash) = price("gemini-3.8-flash-high");
        assert_eq!(display, "Gemini 3.8 Flash (High)");
        assert!(flash.parse::<f64>().unwrap() > 0.0, "{flash}");
        let (_, pro) = price("gemini-3.1-pro-high");
        assert!(pro.parse::<f64>().unwrap() > 0.0, "{pro}");
        assert_eq!(price("mystery-model").1, "0");
    }

    #[test]
    fn agy_incomplete_steps_do_not_consume_dedup_ids() {
        let db = Database::memory().unwrap();
        let mut row =
            json!({"step_index":0,"type":"PLANNER_RESPONSE","status":"RUNNING","input_tokens":10});
        assert_eq!(import_records(&db, "s", &[row.clone()]).unwrap(), (0, 0));
        row["status"] = json!("DONE");
        assert_eq!(import_records(&db, "s", &[row]).unwrap(), (1, 0));
    }

    #[test]
    fn agy_estimates_latency_only_for_tool_call_steps_with_tool_followup() {
        let db = Database::memory().unwrap();
        let rows = vec![
            json!({"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-11-21T13:12:07Z","content":"hi"}),
            // Tool-calling step followed by GENERIC tool execution: 6000 ms
            json!({"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:07Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":300,"tool_calls":[{"name":"run_command"}]}),
            json!({"step_index":2,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:12:13Z","content":"ok"}),
            // Tool-calling step with same-second followup (0 ms < 100 ms): dropped to 0
            json!({"step_index":3,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:14Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":250,"tool_calls":[{"name":"read_file"}]}),
            json!({"step_index":4,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:12:14Z","content":"ok"}),
            // Final reply without tool_calls followed by USER_INPUT: must stay 0
            json!({"step_index":5,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:20Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":1200,"content":"done"}),
            json!({"step_index":6,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-11-21T13:13:30Z","content":"next"}),
            // Tool-calling step followed by USER_INPUT (aborted/interrupted): must stay 0
            json!({"step_index":7,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:13:30Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":250,"tool_calls":[{"name":"run_command"}]}),
            json!({"step_index":8,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-11-21T13:13:45Z","content":"stop"}),
            // Tool-calling step followed by SYSTEM_MESSAGE: must stay 0
            json!({"step_index":9,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:14:00Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":250,"tool_calls":[{"name":"run_command"}]}),
            json!({"step_index":10,"source":"SYSTEM","type":"SYSTEM_MESSAGE","status":"DONE","created_at":"2026-11-21T13:14:15Z","content":"timer"}),
            // Tool-calling step with a missing/corrupted step_index gap (11 -> 13): must stay 0
            json!({"step_index":11,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:15:00Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":250,"tool_calls":[{"name":"run_command"}]}),
            json!({"step_index":13,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:15:20Z","content":"ok"}),
        ];
        assert_eq!(import_records(&db, "timing", &rows).unwrap(), (6, 0));

        let conn = db.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT request_id, latency_ms FROM proxy_request_logs ORDER BY request_id")
            .unwrap();
        let got: Vec<(String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            got,
            vec![
                ("agy:timing:1".to_string(), 6_000),
                ("agy:timing:11".to_string(), 0),
                ("agy:timing:3".to_string(), 0),
                ("agy:timing:5".to_string(), 0),
                ("agy:timing:7".to_string(), 0),
                ("agy:timing:9".to_string(), 0),
            ]
        );
        drop(stmt);
        drop(conn);

        // Verify the estimated latency feeds into usage_stats estimated speed.
        let stats = db
            .get_model_stats(None, None, Some("agy"), None, None)
            .unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].est_speed_output_tokens, 300);
        assert_eq!(stats[0].est_speed_duration_ms, 6_000);
    }

    #[test]
    fn agy_rescan_backfills_latency_on_existing_rows() {
        let db = Database::memory().unwrap();
        let step1 = json!({"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:07Z","model":"gemini-4-neon-medium","input_tokens":100,"output_tokens":300,"tool_calls":[{"name":"run_command"}]});
        // First import before followup exists -> latency_ms is 0
        assert_eq!(
            import_records(&db, "bf", std::slice::from_ref(&step1)).unwrap(),
            (1, 0)
        );
        // Second import once tool followup exists -> updates latency_ms to 5000
        let step2 = json!({"step_index":2,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:12:12Z","content":"ok"});
        assert_eq!(
            import_records(&db, "bf", &[step1.clone(), step2.clone()]).unwrap(),
            (1, 0)
        );
        // Third identical import -> skipped (idempotent)
        assert_eq!(
            import_records(&db, "bf", &[step1.clone(), step2]).unwrap(),
            (0, 1)
        );
        // Fourth partial import without followup -> must not regress latency_ms back to 0
        assert_eq!(import_records(&db, "bf", &[step1]).unwrap(), (0, 1));
        let latency: i64 = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT latency_ms FROM proxy_request_logs WHERE request_id = 'agy:bf:1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(latency, 5_000);
    }
}
