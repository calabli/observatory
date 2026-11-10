//! Usage tracking for the OpenCode session logs
//!
//! Extracts the exact token usage from ~/.local/share/opencode/opencode.db (SQLite).
//!
//! ## Data flow
//! ```text
//! ~/.local/share/opencode/opencode.db
//!   -> the session / session_v2 tables for all sessions (chosen by the storage layout)
//!   -> the message / session_message tables for the assistant messages
//!   -> parse the data JSON to extract tokens/cost/model
//!   -> the proxy_request_logs table
//! ```
//!
//! OpenCode V2 moves the session data into `session_v2` / `session_message` and freezes the
//! V1 tables, so the V2 tables are read first when they are detected; a database that was
//! not migrated keeps using the V1 layout.

use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::opencode_config::get_opencode_db_path;
use crate::proxy::usage::calculator::CostCalculator;
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    metadata_modified_nanos, update_sync_state, SessionPricingCache, SessionSyncResult,
};
use crate::services::usage_stats::{should_skip_session_insert, DedupKey};
use rust_decimal::Decimal;
use std::fs;
use std::time::SystemTime;

/// The token and cost data extracted from the opencode message.data JSON
struct OpenCodeMessageData {
    input_tokens: u32,
    output_tokens: u32,
    reasoning_tokens: u32,
    cache_read_tokens: u32,
    cache_write_tokens: u32,
    cost: f64,
    model_id: String,
    timestamp_ms: i64,
    /// The end of the duration is taken from time.streamed first (the end of the stream, without
    /// the tool execution) and falls back to time.completed when it is missing or broken; the
    /// offline backfill only has the start and the end of the whole message and no time to the
    /// first token, so latency has a value while first_token_ms is always empty
    latency_ms: i64,
}

struct OpenCodeMessageQueryResult {
    messages: Vec<(String, OpenCodeMessageData)>,
    has_incomplete_usage: bool,
}

/// The storage layout of the OpenCode database.
///
/// V1 uses the `session` / `message` tables; V2 (OpenCode 2.x) moves the session data into
/// `session_v2` / `session_message` and stops writing the V1 tables. The V1 tables still
/// exist after the migration but their content is frozen, so the V2 tables have to be read
/// first when they are detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenCodeSchema {
    V1,
    V2,
}

/// Detect the storage layout the database uses: `session_v2` and `session_message` existing
/// together means V2, otherwise it falls back to V1.
fn detect_schema(conn: &rusqlite::Connection) -> Result<OpenCodeSchema, AppError> {
    if sqlite_table_exists(conn, "session_v2")? && sqlite_table_exists(conn, "session_message")? {
        Ok(OpenCodeSchema::V2)
    } else {
        Ok(OpenCodeSchema::V1)
    }
}

fn sqlite_table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Database(format!("查询表 {table} 失败: {error}")))
}

/// Sync the OpenCode usage data
pub fn sync_opencode_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let db_path = get_opencode_db_path();

    if !db_path.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            suspected_duplicates: 0,
            deferred_files: 0,
            errors: vec![],
        });
    }

    let db_path_str = db_path.to_string_lossy().to_string();

    // Check the modification time of the file.
    // The opencode database runs in WAL mode: a new commit lands in the -wal file first and the
    // main database file is only updated at a checkpoint. The mtime of the -wal therefore has to
    // be taken into account as well, otherwise the sessions that were just written are missed
    // before the checkpoint.
    let metadata = fs::metadata(&db_path)
        .map_err(|e| AppError::Config(format!("无法读取 opencode.db 元数据: {e}")))?;
    let mut file_modified = metadata_modified_nanos(&metadata);

    let wal_path = db_path.with_extension("db-wal");
    if let Ok(wal_meta) = fs::metadata(&wal_path) {
        file_modified = file_modified.max(metadata_modified_nanos(&wal_meta));
    }

    let cursors = crate::services::session_usage::load_sync_cursors(db)?;
    let last_modified = cursors.get(&db_path_str).map_or(0, |c| c.last_modified);

    // Skip the file when it did not change
    if file_modified <= last_modified {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 1,
            suspected_duplicates: 0,
            deferred_files: 0,
            errors: vec![],
        });
    }

    // Open the SQLite database of opencode (read-only)
    let opencode_conn =
        rusqlite::Connection::open_with_flags(&db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| AppError::Database(format!("无法打开 opencode.db: {e}")))?;

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 1,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };
    let mut has_sync_errors = false;

    let schema = detect_schema(&opencode_conn)?;

    // Query all the sessions
    let sessions = query_sessions(&opencode_conn, schema)?;

    for (session_id, time_updated) in &sessions {
        // Check whether the session has to be synced again
        let sync_key = format!("{db_path_str}:{session_id}");
        let sess_last_modified = cursors.get(&sync_key).map_or(0, |c| c.last_modified);
        if *time_updated <= sess_last_modified {
            continue; // The session was not updated, skip it
        }

        let session_had_error;

        // Query all the assistant messages of that session
        let mut session_has_incomplete_usage = false;
        match query_assistant_messages(&opencode_conn, schema, session_id) {
            Ok(query_result) => {
                session_has_incomplete_usage = query_result.has_incomplete_usage;
                // One transaction per session: the lock is taken once and the messages are committed in a
                // batch. Before it was one lock acquisition plus an autocommit per message, which meant
                // O(N) lock round trips when an active session was fully replayed in every round.
                match sync_session_messages(db, session_id, &query_result.messages) {
                    Ok((imported, skipped, had_row_error)) => {
                        result.imported += imported;
                        result.skipped += skipped;
                        session_had_error = had_row_error;
                    }
                    Err(e) => {
                        let msg = format!("OpenCode 会话消息写入失败 {session_id}: {e}");
                        log::warn!("[OPENCODE-SYNC] {msg}");
                        result.errors.push(msg);
                        result.skipped += 1;
                        session_had_error = true;
                    }
                }
            }
            Err(e) => {
                let msg = format!("OpenCode 会话消息查询失败 {session_id}: {e}");
                log::warn!("[OPENCODE-SYNC] {msg}");
                result.errors.push(msg);
                session_had_error = true;
            }
        }

        if session_had_error {
            has_sync_errors = true;
            continue;
        }

        if session_has_incomplete_usage {
            continue;
        }

        // Update the session level sync state. Do not advance the file level state on failure, so that the next round can retry.
        if let Err(e) = update_sync_state(db, &sync_key, *time_updated, 0) {
            let msg = format!("OpenCode 会话同步状态更新失败 {session_id}: {e}");
            log::warn!("[OPENCODE-SYNC] {msg}");
            result.errors.push(msg);
            has_sync_errors = true;
        }
    }

    // Only advance the file level state when this round fully succeeded; otherwise keep the entry point for the next retry.
    if !has_sync_errors {
        update_sync_state(db, &db_path_str, file_modified, 0)?;
    }

    if result.imported > 0 {
        log::info!(
            "[OPENCODE-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个会话",
            result.imported,
            result.skipped,
            sessions.len()
        );
    }

    Ok(result)
}

/// Query the (id, sync_watermark) of all the sessions
fn query_sessions(
    conn: &rusqlite::Connection,
    schema: OpenCodeSchema,
) -> Result<Vec<(String, i64)>, AppError> {
    let sessions_table = match schema {
        OpenCodeSchema::V1 => "session",
        OpenCodeSchema::V2 => "session_v2",
    };
    let messages_table = match schema {
        OpenCodeSchema::V1 => "message",
        OpenCodeSchema::V2 => "session_message",
    };
    let sql = format!(
        "SELECT s.id,
                MAX(s.time_updated, COALESCE(MAX(m.time_updated), s.time_updated)) AS sync_watermark
         FROM {sessions_table} s
         LEFT JOIN {messages_table} m ON m.session_id = s.id
         GROUP BY s.id
         ORDER BY sync_watermark"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| AppError::Database(format!("准备会话查询失败: {e}")))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| AppError::Database(format!("查询会话失败: {e}")))?;

    let mut sessions = Vec::new();
    for row in rows {
        sessions.push(row.map_err(|e| AppError::Database(format!("读取会话行失败: {e}")))?);
    }

    Ok(sessions)
}

/// Query the finished billable messages of a session (V1: assistant, V2: assistant +
/// compaction) and mark whether there are still unfinished usage messages.
fn query_assistant_messages(
    conn: &rusqlite::Connection,
    schema: OpenCodeSchema,
    session_id: &str,
) -> Result<OpenCodeMessageQueryResult, AppError> {
    // The message type of V2 is recorded in the `type` column and the usage of a context
    // compaction request is written to the type='compaction' rows (in V1 the compaction summary
    // is an assistant message and needs no special case);
    // V1 has to filter by data.role.
    let sql = match schema {
        OpenCodeSchema::V1 => {
            "SELECT id, data, NULL FROM message WHERE session_id = ?1 ORDER BY time_created"
        }
        OpenCodeSchema::V2 => {
            "SELECT id, data, type FROM session_message \
             WHERE session_id = ?1 AND type IN ('assistant', 'compaction') \
             ORDER BY time_created"
        }
    };
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| AppError::Database(format!("准备消息查询失败: {e}")))?;

    let rows = stmt
        .query_map([session_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(|e| AppError::Database(format!("查询消息失败: {e}")))?;

    let mut messages = Vec::new();
    let mut has_incomplete_usage = false;
    for row in rows {
        let (message_id, data_json, message_type) =
            row.map_err(|e| AppError::Database(format!("读取消息行失败: {e}")))?;

        // The assistant messages of V2 have no role field (the `type` column identifies them), the V1 messages are still filtered by data.role.
        let value: serde_json::Value = match serde_json::from_str(&data_json) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if schema == OpenCodeSchema::V1
            && value.get("role").and_then(|r| r.as_str()) != Some("assistant")
        {
            continue;
        }

        // A tokens field is required
        if value.get("tokens").is_none() {
            continue;
        }

        // Skip the unfinished messages: while they are running they only have half the tokens and
        // they cannot be filled in later because of INSERT OR IGNORE.
        // An assistant is final when it has a time.completed; a V2 compaction is final through its
        // status (completed/failed), it has no time.completed field.
        let is_completed = if message_type.as_deref() == Some("compaction") {
            matches!(
                value.get("status").and_then(|s| s.as_str()),
                Some("completed") | Some("failed")
            )
        } else {
            value.get("time").and_then(|t| t.get("completed")).is_some()
        };
        if !is_completed {
            has_incomplete_usage = true;
            continue;
        }

        if let Some(msg_data) = parse_message_data(&value) {
            messages.push((message_id, msg_data));
        }
    }

    Ok(OpenCodeMessageQueryResult {
        messages,
        has_incomplete_usage,
    })
}

/// Parse the opencode message.data JSON into structured data
fn parse_message_data(value: &serde_json::Value) -> Option<OpenCodeMessageData> {
    let tokens = value.get("tokens")?;

    let input_tokens = tokens.get("input").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let output_tokens = tokens.get("output").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let reasoning_tokens = tokens
        .get("reasoning")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let cache_obj = tokens.get("cache");
    let cache_read_tokens = cache_obj
        .and_then(|c| c.get("read"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cache_write_tokens = cache_obj
        .and_then(|c| c.get("write"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // Skip the messages whose tokens are all zero
    if input_tokens == 0
        && output_tokens == 0
        && reasoning_tokens == 0
        && cache_read_tokens == 0
        && cache_write_tokens == 0
    {
        return None;
    }

    let cost = value.get("cost").and_then(|v| v.as_f64()).unwrap_or(0.0);

    // V1 puts the model ID into the `modelID` string; the `model` of V2 is an
    // `{ id, providerID, variant }` object (the plain string form is supported as well).
    let model_id = value
        .get("modelID")
        .and_then(|v| v.as_str())
        .or_else(|| match value.get("model") {
            Some(serde_json::Value::String(model)) => Some(model.as_str()),
            Some(model) => model.get("id").and_then(|v| v.as_str()),
            None => None,
        })
        .unwrap_or("unknown")
        .to_string();

    let timestamp_ms = value
        .get("time")
        .and_then(|t| t.get("created"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    // The end of the duration is taken from time.streamed first (the end of the stream), which
    // does not include the tool execution time;
    // when it is missing or earlier than created (a broken clock) it falls back to time.completed
    // (which includes the tool execution, so the speed estimate is too low).
    // The caller already filtered out the unfinished messages without time.completed; this is
    // the last resort:
    // when created is missing (timestamp_ms falls back to 0, so the difference becomes a value in
    // milliseconds since the epoch) or the clock runs backwards (the end is not later than
    // created) it is treated as 0
    let time = value.get("time");
    let streamed_ms = time
        .and_then(|t| t.get("streamed"))
        .and_then(|v| v.as_i64());
    let completed_ms = time
        .and_then(|t| t.get("completed"))
        .and_then(|v| v.as_i64());
    let end_ms = streamed_ms
        .filter(|&end| end > timestamp_ms)
        .or(completed_ms);
    let latency_ms = match end_ms {
        Some(end) if timestamp_ms > 0 && end > timestamp_ms => end - timestamp_ms,
        _ => 0,
    };

    Some(OpenCodeMessageData {
        input_tokens,
        output_tokens,
        reasoning_tokens,
        cache_read_tokens,
        cache_write_tokens,
        cost,
        model_id,
        timestamp_ms,
        latency_ms,
    })
}

/// Write all the messages of a session: one transaction per session and one lock
/// acquisition, replacing the per-message lock plus autocommit.
/// A failed single insert keeps the old behaviour of skipping that row and continuing, and
/// hands `had_row_error` back to the caller to decide whether the cursor is advanced; a
/// failing transaction commit rolls the whole session back and the next round re-imports it
/// with the idempotent UPSERT.
fn sync_session_messages(
    db: &Database,
    session_id: &str,
    messages: &[(String, OpenCodeMessageData)],
) -> Result<(u32, u32, bool), AppError> {
    let mut conn = lock_conn!(db.conn);
    let tx = conn
        .transaction()
        .map_err(|e| AppError::Database(format!("开启 OpenCode 会话写入事务失败: {e}")))?;
    // The same session repeatedly hits a few models: cache the price lookups per transaction.
    let mut pricing_cache = SessionPricingCache::default();
    let mut imported = 0u32;
    let mut skipped = 0u32;
    let mut had_row_error = false;
    for (message_id, msg_data) in messages {
        let request_id = format!("opencode_session:{session_id}:{message_id}");
        match insert_opencode_message_on_conn(
            &tx,
            &request_id,
            msg_data,
            session_id,
            &mut pricing_cache,
        ) {
            Ok(true) => imported += 1,
            Ok(false) => skipped += 1,
            Err(e) => {
                let msg = format!("OpenCode 消息插入失败 {request_id}: {e}");
                log::warn!("[OPENCODE-SYNC] {msg}");
                skipped += 1;
                had_row_error = true;
            }
        }
    }
    tx.commit()
        .map_err(|e| AppError::Database(format!("提交 OpenCode 会话写入事务失败: {e}")))?;
    Ok((imported, skipped, had_row_error))
}

/// Insert a single OpenCode message record into proxy_request_logs; the caller is responsible for the lock / transaction.
fn insert_opencode_message_on_conn(
    conn: &rusqlite::Connection,
    request_id: &str,
    msg: &OpenCodeMessageData,
    session_id: &str,
    pricing_cache: &mut SessionPricingCache,
) -> Result<bool, AppError> {
    let created_at = if msg.timestamp_ms > 0 {
        msg.timestamp_ms / 1000
    } else {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    };

    // OpenCode uses the Anthropic style: the input is the fresh input and the cache is billed
    // separately
    // the output includes the reasoning tokens (they are billed as output)
    let output_with_reasoning = msg.output_tokens + msg.reasoning_tokens;

    let dedup_key = DedupKey {
        app_type: "opencode",
        model: &msg.model_id,
        input_tokens: msg.input_tokens,
        output_tokens: output_with_reasoning,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_write_tokens,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }

    // If opencode already provided the cost, use it; otherwise compute it from the model pricing
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) =
        if msg.cost > 0.0 {
            // opencode already computed the cost, use it
            // Simplified handling: everything goes into total_cost (the cost of opencode is an aggregated
            // value and cannot be split up precisely)
            (
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                "0".to_string(),
                msg.cost.to_string(),
            )
        } else {
            // The opencode cost is 0 (a free model for example), try to compute it with the model pricing that ships with cc-switch
            let usage = TokenUsage {
                input_tokens: msg.input_tokens,
                output_tokens: output_with_reasoning,
                cache_read_tokens: msg.cache_read_tokens,
                cache_creation_tokens: msg.cache_write_tokens,
                cache_creation_1h_tokens: 0,
                model: Some(msg.model_id.clone()),
                message_id: None,
            };

            match pricing_cache.get(conn, &msg.model_id) {
                Some(pricing) => {
                    let cost = CostCalculator::calculate_for_app(
                        "opencode",
                        &usage,
                        pricing,
                        Decimal::from(1),
                    );
                    (
                        cost.input_cost.to_string(),
                        cost.output_cost.to_string(),
                        cost.cache_read_cost.to_string(),
                        cost.cache_creation_cost.to_string(),
                        cost.total_cost.to_string(),
                    )
                }
                None => (
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                    "0".to_string(),
                ),
            }
        };

    let inserted_rows = conn.execute(
        "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        rusqlite::params![
            request_id,
            "_opencode_session",   // provider_id
            "opencode",            // app_type
            msg.model_id,
            msg.model_id,          // request_model = model
            msg.input_tokens,
            output_with_reasoning,
            msg.cache_read_tokens,
            msg.cache_write_tokens,
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            msg.latency_ms,
            // There is no field for the time to the first token in opencode.db, the offline backfill cannot measure first_token_ms
            Option::<i64>::None,   // first_token_ms
            200i64,                // status_code
            Option::<String>::None,// error_message
            Some(session_id.to_string()),
            Some("opencode_session"), // provider_type
            1i64,                  // is_streaming
            "1.0",                 // cost_multiplier
            created_at,
            "opencode_session",    // data_source
        ],
    )
    .map_err(|e| AppError::Database(format!("插入 OpenCode 会话日志失败: {e}")))?;

    Ok(inserted_rows > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_message_data_full() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0023113,
            "tokens": {
                "total": 56554,
                "input": 3272,
                "output": 383,
                "reasoning": 419,
                "cache": {
                    "write": 0,
                    "read": 52480
                }
            },
            "modelID": "deepseek-v4-pro",
            "providerID": "deepseek",
            "time": {
                "created": 1779755333700i64,
                "completed": 1779755350639i64
            }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 3272);
        assert_eq!(data.output_tokens, 383);
        assert_eq!(data.reasoning_tokens, 419);
        assert_eq!(data.cache_read_tokens, 52480);
        assert_eq!(data.cache_write_tokens, 0);
        assert!((data.cost - 0.0023113).abs() < 1e-10);
        assert_eq!(data.model_id, "deepseek-v4-pro");
        assert_eq!(data.timestamp_ms, 1779755333700);
        assert_eq!(data.latency_ms, 1779755350639 - 1779755333700);
    }

    #[test]
    fn test_parse_message_data_latency_fallbacks() {
        // No time.completed: the latency falls back to 0
        let no_completed = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "created": 1000i64 }
        });
        let data = parse_message_data(&no_completed).unwrap();
        assert_eq!(data.latency_ms, 0);

        // completed is earlier than created: the fallback is 0, no negative number
        let negative = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "created": 2000i64, "completed": 1000i64 }
        });
        let data = parse_message_data(&negative).unwrap();
        assert_eq!(data.latency_ms, 0);

        // completed is there but created is missing: the difference must not be computed from the milliseconds since the epoch
        let no_created = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "completed": 1779755350639i64 }
        });
        let data = parse_message_data(&no_created).unwrap();
        assert_eq!(data.timestamp_ms, 0);
        assert_eq!(data.latency_ms, 0);
    }

    #[test]
    fn test_parse_message_data_missing_cache() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "cost": 0.0,
            "tokens": {
                "input": 1000,
                "output": 200
            },
            "modelID": "mimo-v2.5-pro",
            "time": { "created": 1779755333700i64 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 1000);
        assert_eq!(data.output_tokens, 200);
        assert_eq!(data.reasoning_tokens, 0);
        assert_eq!(data.cache_read_tokens, 0);
        assert_eq!(data.cache_write_tokens, 0);
        // An unfinished message (without time.completed) has no usable duration
        assert_eq!(data.latency_ms, 0);
    }

    #[test]
    fn test_parse_message_data_skips_zero_tokens() {
        let json: serde_json::Value = serde_json::json!({
            "role": "assistant",
            "tokens": {
                "input": 0,
                "output": 0,
                "reasoning": 0,
                "cache": { "read": 0, "write": 0 }
            },
            "modelID": "test"
        });
        assert!(parse_message_data(&json).is_none());
    }

    #[test]
    fn test_parse_message_data_ignores_role() {
        // parse_message_data does not filter by role; that's the caller's job
        let json: serde_json::Value = serde_json::json!({
            "role": "user",
            "tokens": { "input": 100, "output": 0 }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.input_tokens, 100);
    }

    #[test]
    fn test_query_assistant_messages_skips_incomplete() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();

        let done = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 1000, "output": 200 },
            "modelID": "m",
            "time": { "created": 1, "completed": 2 }
        })
        .to_string();
        let in_progress = serde_json::json!({
            "role": "assistant",
            "tokens": { "input": 500, "output": 0 },
            "modelID": "m",
            "time": { "created": 3 }
        })
        .to_string();

        conn.execute(
            "INSERT INTO message VALUES ('done', 's1', 1, ?1), ('wip', 's1', 2, ?2)",
            rusqlite::params![done, in_progress],
        )
        .unwrap();

        let result = query_assistant_messages(&conn, OpenCodeSchema::V1, "s1").unwrap();
        // Only the finished messages (with time.completed) are returned, the half finished ones are skipped
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].0, "done");
        assert!(result.has_incomplete_usage);
    }

    #[test]
    fn test_query_sessions_uses_message_update_watermark() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT, time_updated INTEGER);
             CREATE TABLE message (
                 id TEXT,
                 session_id TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session VALUES ('s1', 100);
             INSERT INTO message VALUES ('m1', 's1', 90, 200, '{}');",
        )
        .unwrap();

        let sessions = query_sessions(&conn, OpenCodeSchema::V1).unwrap();
        assert_eq!(sessions, vec![("s1".to_string(), 200)]);
    }

    #[test]
    fn test_detect_schema_prefers_v2_when_present() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT); CREATE TABLE message (id TEXT);")
            .unwrap();
        assert_eq!(detect_schema(&conn).unwrap(), OpenCodeSchema::V1);

        conn.execute_batch(
            "CREATE TABLE session_v2 (id TEXT); CREATE TABLE session_message (id TEXT);",
        )
        .unwrap();
        assert_eq!(detect_schema(&conn).unwrap(), OpenCodeSchema::V2);
    }

    #[test]
    fn test_parse_message_data_v2_model_object() {
        // The assistant messages of V2 have no role field and the model is an object
        let json: serde_json::Value = serde_json::json!({
            "agent": "build",
            "cost": 0.00143667,
            "tokens": {
                "input": 1373,
                "output": 96,
                "reasoning": 0,
                "cache": { "read": 391040, "write": 0 }
            },
            "model": { "id": "deepseek-v4.1-flash", "providerID": "opencode-go", "variant": "max" },
            "time": {
                "created": 1789945285532i64,
                "streamed": 1789945288379i64,
                "completed": 1789945288526i64
            }
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.model_id, "deepseek-v4.1-flash");
        assert_eq!(data.input_tokens, 1373);
        assert_eq!(data.cache_read_tokens, 391040);
        assert_eq!(data.timestamp_ms, 1789945285532);
        // With streamed the end is taken from streamed (the end of the stream), which does not include the wrap-up after completed
        assert_eq!(data.latency_ms, 1789945288379 - 1789945285532);
    }

    #[test]
    fn test_parse_message_data_prefers_streamed_for_latency() {
        // With both streamed and completed: the end is taken from streamed, which excludes the tool execution time after completed
        let with_streamed = serde_json::json!({
            "tokens": { "input": 1000, "output": 674 },
            "model": { "id": "m" },
            "time": { "created": 1000i64, "streamed": 2000i64, "completed": 120000i64 }
        });
        let data = parse_message_data(&with_streamed).unwrap();
        assert_eq!(data.latency_ms, 1000);

        // Only completed: fall back to completed
        let only_completed = serde_json::json!({
            "tokens": { "input": 1000, "output": 674 },
            "model": { "id": "m" },
            "time": { "created": 1000i64, "completed": 3000i64 }
        });
        let data = parse_message_data(&only_completed).unwrap();
        assert_eq!(data.latency_ms, 2000);

        // streamed is earlier than created (a broken clock): ignore streamed and fall back to completed
        let skewed = serde_json::json!({
            "tokens": { "input": 1000, "output": 674 },
            "model": { "id": "m" },
            "time": { "created": 5000i64, "streamed": 4000i64, "completed": 9000i64 }
        });
        let data = parse_message_data(&skewed).unwrap();
        assert_eq!(data.latency_ms, 4000);
    }

    #[test]
    fn test_parse_message_data_v2_model_string() {
        // Support the form where the model is a plain string
        let json: serde_json::Value = serde_json::json!({
            "tokens": { "input": 10, "output": 1 },
            "model": "some-model"
        });
        let data = parse_message_data(&json).unwrap();
        assert_eq!(data.model_id, "some-model");
    }

    #[test]
    fn test_query_sessions_v2_uses_message_update_watermark() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_v2 (id TEXT, time_updated INTEGER);
             CREATE TABLE session_message (
                 id TEXT,
                 session_id TEXT,
                 type TEXT,
                 time_created INTEGER,
                 time_updated INTEGER,
                 data TEXT
             );
             INSERT INTO session_v2 VALUES ('s1', 100);
             INSERT INTO session_message VALUES ('m1', 's1', 'assistant', 90, 200, '{}');",
        )
        .unwrap();

        let sessions = query_sessions(&conn, OpenCodeSchema::V2).unwrap();
        assert_eq!(sessions, vec![("s1".to_string(), 200)]);
    }

    #[test]
    fn test_query_assistant_messages_v2_filters_by_type() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_message (
                 id TEXT,
                 session_id TEXT,
                 type TEXT,
                 time_created INTEGER,
                 data TEXT
             );",
        )
        .unwrap();

        let done = serde_json::json!({
            "tokens": { "input": 1000, "output": 200 },
            "model": { "id": "m" },
            "time": { "created": 1, "completed": 2 }
        })
        .to_string();
        let in_progress = serde_json::json!({
            "tokens": { "input": 500, "output": 0 },
            "model": { "id": "m" },
            "time": { "created": 3 }
        })
        .to_string();
        let user = serde_json::json!({
            "tokens": { "input": 1, "output": 0 }
        })
        .to_string();

        conn.execute(
            "INSERT INTO session_message VALUES ('done', 's1', 'assistant', 1, ?1),
                                                ('wip', 's1', 'assistant', 2, ?2),
                                                ('u1', 's1', 'user', 3, ?3)",
            rusqlite::params![done, in_progress, user],
        )
        .unwrap();

        let result = query_assistant_messages(&conn, OpenCodeSchema::V2, "s1").unwrap();
        // Only the finished assistant messages (with time.completed) are returned; the user rows are filtered out by type
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].0, "done");
        assert!(result.has_incomplete_usage);
    }

    #[test]
    fn test_query_assistant_messages_v2_includes_completed_compaction() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE session_message (
                 id TEXT,
                 session_id TEXT,
                 type TEXT,
                 time_created INTEGER,
                 data TEXT
             );",
        )
        .unwrap();

        // A V2 compaction: the final state is expressed by the status, there is no time.completed
        let compaction_done = serde_json::json!({
            "status": "completed",
            "tokens": { "input": 741, "output": 2604 },
            "cost": 0.004736382,
            "model": { "id": "deepseek-v4.1-flash" },
            "time": { "created": 10 }
        })
        .to_string();
        let compaction_wip = serde_json::json!({
            "status": "pending",
            "tokens": { "input": 100, "output": 0 },
            "model": { "id": "deepseek-v4.1-flash" },
            "time": { "created": 11 }
        })
        .to_string();
        // A failed compaction request produced billable usage as well, it has to be imported too
        let compaction_failed = serde_json::json!({
            "status": "failed",
            "tokens": { "input": 300, "output": 0 },
            "cost": 0.001,
            "model": { "id": "deepseek-v4.1-flash" },
            "time": { "created": 12 }
        })
        .to_string();
        let assistant = serde_json::json!({
            "tokens": { "input": 1000, "output": 200 },
            "model": { "id": "m" },
            "time": { "created": 13, "completed": 14 }
        })
        .to_string();

        conn.execute(
            "INSERT INTO session_message VALUES ('c1', 's1', 'compaction', 1, ?1),
                                                ('c2', 's1', 'compaction', 2, ?2),
                                                ('c3', 's1', 'compaction', 3, ?3),
                                                ('a1', 's1', 'assistant', 4, ?4)",
            rusqlite::params![
                compaction_done,
                compaction_wip,
                compaction_failed,
                assistant
            ],
        )
        .unwrap();

        let result = query_assistant_messages(&conn, OpenCodeSchema::V2, "s1").unwrap();
        // Both compaction(completed/failed) and assistant are collected; a pending compaction is skipped
        let ids: Vec<&str> = result.messages.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["c1", "c3", "a1"]);
        assert_eq!(result.messages[0].1.input_tokens, 741);
        assert_eq!(result.messages[0].1.model_id, "deepseek-v4.1-flash");
        assert_eq!(result.messages[1].1.input_tokens, 300);
        assert!(result.has_incomplete_usage);
    }
}
