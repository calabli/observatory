//! Activity log: records which files a scan read, external API calls, device syncs
//! and key operations, for the UI's Activity Log page.
//!
//! - Stored in the local database only, kept for [`RETENTION_DAYS`] days and
//!   pruned at most once an hour.
//! - Writes go through [`Database::write_quietly`] and do not advance the write
//!   epoch, so logging never invalidates a statistics cache and never makes a LAN
//!   peer think the local content changed.
//! - A failed record only writes to the process log and never affects the
//!   operation being recorded.
//! - Sensitive arguments such as pairing codes, tokens and API keys are never
//!   recorded (see [`sanitize_args`]).
use crate::database::Database;
use crate::error::AppError;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicI64, Ordering};

/// Number of days the log is retained.
pub const RETENTION_DAYS: i64 = 7;

/// One activity waiting to be recorded.
pub struct Event {
    /// `scan` | `api_in` | `sync` | `action`
    pub kind: &'static str,
    pub title: String,
    /// `ok` | `error` | `skipped`
    pub status: &'static str,
    pub duration_ms: Option<i64>,
    /// The summary shown directly in the list (a small object).
    pub summary: Value,
    /// The detail records loaded only after expanding (e.g. per-file read records); they can be large.
    pub detail: Option<Value>,
}

/// Record one activity. Failures only write to the process log.
pub fn record(db: &Database, event: Event) {
    if let Err(error) = try_record(db, &event) {
        log::warn!("记录活动日志失败（{}）: {error}", event.title);
    }
}

fn try_record(db: &Database, event: &Event) -> Result<(), AppError> {
    static LAST_PRUNE: AtomicI64 = AtomicI64::new(0);
    let now = chrono::Utc::now().timestamp_millis();
    db.write_quietly(|conn| {
        conn.execute(
            "INSERT INTO activity_log (ts, kind, title, status, duration_ms, summary, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                now,
                event.kind,
                event.title,
                event.status,
                event.duration_ms,
                event.summary.to_string(),
                event.detail.as_ref().map(Value::to_string),
            ],
        )?;
        if now - LAST_PRUNE.load(Ordering::Relaxed) > 3_600_000 {
            LAST_PRUNE.store(now, Ordering::Relaxed);
            conn.execute(
                "DELETE FROM activity_log WHERE ts < ?1",
                [now - RETENTION_DAYS * 86_400_000],
            )?;
        }
        Ok(())
    })
}

/// One row of the list (without detail records).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: i64,
    pub ts: i64,
    pub kind: String,
    pub title: String,
    pub status: String,
    pub duration_ms: Option<i64>,
    pub summary: Value,
    pub has_detail: bool,
}

/// List activities newest first; `before_id` is used to page backwards.
pub fn list(
    db: &Database,
    kind: Option<&str>,
    before_id: Option<i64>,
    limit: u32,
) -> Result<Vec<Entry>, AppError> {
    let limit = limit.clamp(1, 500);
    db.with_read_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, ts, kind, title, status, duration_ms, summary, detail IS NOT NULL
             FROM activity_log
             WHERE (?1 IS NULL OR kind = ?1) AND (?2 IS NULL OR id < ?2)
             ORDER BY id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![kind, before_id, limit], |row| {
            let summary: String = row.get(6)?;
            Ok(Entry {
                id: row.get(0)?,
                ts: row.get(1)?,
                kind: row.get(2)?,
                title: row.get(3)?,
                status: row.get(4)?,
                duration_ms: row.get(5)?,
                summary: serde_json::from_str(&summary).unwrap_or(Value::Null),
                has_detail: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    })
}

/// The detail records of one activity.
pub fn detail(db: &Database, id: i64) -> Result<Option<Value>, AppError> {
    db.with_read_connection(|conn| {
        let raw: Option<Option<String>> = conn
            .query_row(
                "SELECT detail FROM activity_log WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(raw
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok()))
    })
}

// ---------------------------------------------------------------------------
// Scans: compare the cursors and the new detail rows before and after each collector runs to derive per-file read records.

/// What triggered this local scan.
#[derive(Debug, Clone)]
pub enum ScanTrigger {
    /// Scheduled background scan: recorded only when new content was read or an error occurred, so it does not produce an entry every 30 seconds.
    Background,
    /// Manual actions in the UI such as "Scan for updates" / "Sync all".
    Manual,
    /// The UI's automatic refresh (a sync triggered by the refresh interval).
    Auto,
    /// A scan triggered over the LAN by a paired device.
    Remote(String),
}

impl ScanTrigger {
    fn label(&self) -> String {
        match self {
            Self::Background => "后台定时".into(),
            Self::Manual => "手动".into(),
            Self::Auto => "自动刷新".into(),
            Self::Remote(peer) => format!("远端设备 {peer}"),
        }
    }
}

/// Collectors that track read progress in `session_log_sync` can report per-file detail records.
const FILE_LEVEL_SOURCES: [&str; 8] = [
    "claude",
    "codex",
    "opencode",
    "grokbuild",
    "pi",
    "mcode",
    "kimi",
    "dsh",
];

/// One row of the collection cursor (`session_log_sync`).
#[derive(Debug, Clone, PartialEq)]
struct Cursor {
    modified: i64,
    line: i64,
    byte: Option<i64>,
}

/// High-water marks of the cursor table and the detail table at a point in time.
#[derive(Clone)]
pub(crate) struct ScanProbe {
    cursors: HashMap<String, Cursor>,
    max_rowid: i64,
}

impl ScanProbe {
    /// The maximum rowid of the detail table when the snapshot was taken.
    pub(crate) fn max_rowid(&self) -> i64 {
        self.max_rowid
    }
}

/// Read the maximum rowid of the cursor table and the detail table once. The cursor table usually holds a few hundred rows, so this takes milliseconds.
pub(crate) fn probe(db: &Database) -> ScanProbe {
    let read = db.with_read_connection(|conn| -> Result<ScanProbe, AppError> {
        let mut stmt = conn.prepare(
            "SELECT file_path, last_modified, last_line_offset, last_byte_offset FROM session_log_sync",
        )?;
        let cursors = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Cursor {
                        modified: row.get(1)?,
                        line: row.get(2)?,
                        byte: row.get(3)?,
                    },
                ))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        let max_rowid =
            conn.query_row("SELECT COALESCE(MAX(rowid), 0) FROM proxy_request_logs", [], |row| {
                row.get(0)
            })?;
        Ok(ScanProbe { cursors, max_rowid })
    });
    read.unwrap_or_else(|error| {
        log::warn!("活动日志读取采集游标失败: {error}");
        ScanProbe {
            cursors: HashMap::new(),
            max_rowid: i64::MAX,
        }
    })
}

/// The result of one collector for this round, plus its read detail records.
pub(crate) struct SourceScan {
    pub id: &'static str,
    pub name: &'static str,
    pub duration_ms: i64,
    pub result: Result<crate::services::session_usage::SessionSyncResult, String>,
    pub before: ScanProbe,
    pub after: ScanProbe,
    /// The detail records this collector added (aggregated per session); they must be
    /// taken with [`new_rows`] right after the collector finished: the pruning at the end of a
    /// scan deletes detail records outside the retention window (very common on the first history import).
    pub rows: Vec<(String, i64, i64)>,
}

/// Turn a cursor key back into a file path. The key may carry a source prefix
/// (`mcode:<path>`) or a session suffix (`<db path>:<session id>`); take the part that can be found on disk.
fn key_path(key: &str) -> (String, Option<String>) {
    let key = key
        .split_once(':')
        .filter(|(prefix, _)| prefix.len() > 1 && prefix.chars().all(|c| c.is_ascii_lowercase()))
        .map_or(key, |(_, rest)| rest);
    if Path::new(key).exists() {
        return (key.to_string(), None);
    }
    if let Some((path, part)) = key.rsplit_once(':') {
        if path.len() > 2 && Path::new(path).exists() {
            return (path.to_string(), Some(part.to_string()));
        }
    }
    (key.to_string(), None)
}

/// The unit of the modification time in a cursor varies by collector (seconds, milliseconds, nanoseconds); it is normalized to milliseconds.
fn normalize_ms(value: i64) -> Option<i64> {
    match value {
        v if v <= 0 => None,
        v if v > 100_000_000_000_000_000 => Some(v / 1_000_000),
        v if v > 100_000_000_000_000 => Some(v / 1_000),
        v if v > 100_000_000_000 => Some(v),
        v => Some(v * 1_000),
    }
}

/// New detail records aggregated per session: (session id, row count, total tokens).
pub(crate) fn new_rows(db: &Database, after: i64, upto: i64) -> Vec<(String, i64, i64)> {
    if upto <= after {
        return Vec::new();
    }
    db.with_read_connection(|conn| -> Result<_, AppError> {
        let mut stmt = conn.prepare(
            "SELECT COALESCE(session_id, ''), COUNT(*),
                    SUM(input_tokens + output_tokens + cache_read_tokens + cache_creation_tokens)
             FROM proxy_request_logs WHERE rowid > ?1 AND rowid <= ?2 GROUP BY 1",
        )?;
        let rows = stmt
            .query_map([after, upto], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
    .unwrap_or_default()
}

/// Aggregate one round of scanning and decide whether to record it.
pub(crate) fn record_scan(
    db: &Database,
    trigger: &ScanTrigger,
    started: std::time::Instant,
    sources: Vec<SourceScan>,
) {
    let mut files: Vec<Value> = Vec::new();
    let mut source_rows = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let (mut total_rows, mut total_tokens, mut total_imported) = (0i64, 0i64, 0u64);
    for source in &sources {
        let rows = &source.rows;
        let rows_count: i64 = rows.iter().map(|r| r.1).sum();
        let tokens: i64 = rows.iter().map(|r| r.2).sum();
        total_rows += rows_count;
        total_tokens += tokens;
        let mut attributed = vec![false; rows.len()];
        let mut changed = 0;
        let mut keys: Vec<&String> = source.after.cursors.keys().collect();
        keys.sort();
        for key in keys {
            let current = &source.after.cursors[key];
            let previous = source.before.cursors.get(key);
            if previous == Some(current) {
                continue;
            }
            changed += 1;
            let (path, part) = key_path(key);
            let metadata = std::fs::metadata(&path).ok();
            // Attribute the new detail records whose session id appears in the file path (or cursor key) to this file.
            let (mut file_rows, mut file_tokens) = (0i64, 0i64);
            for (index, (session, count, sum)) in rows.iter().enumerate() {
                if !attributed[index] && session.len() >= 6 && key.contains(session.as_str()) {
                    attributed[index] = true;
                    file_rows += count;
                    file_tokens += sum;
                }
            }
            let bytes_read = match (previous.and_then(|p| p.byte), current.byte) {
                (Some(before), Some(after)) if after >= before => Some(after - before),
                (None, Some(after)) => Some(after),
                _ => None,
            };
            files.push(json!({
                "source": source.id,
                "path": path,
                "part": part,
                "isNew": previous.is_none(),
                "sizeBytes": metadata.as_ref().filter(|m| m.is_file()).map(|m| m.len()),
                "modifiedMs": normalize_ms(current.modified),
                "fromByte": previous.and_then(|p| p.byte),
                "toByte": current.byte,
                "bytesRead": bytes_read,
                "fromLine": previous.map(|p| p.line),
                "toLine": current.line,
                "rows": file_rows,
                "tokens": file_tokens,
            }));
        }
        let unattributed: i64 = rows
            .iter()
            .zip(&attributed)
            .filter(|(_, done)| !**done)
            .map(|(row, _)| row.1)
            .sum();
        let (imported, skipped, scanned, source_errors) = match &source.result {
            Ok(result) => (
                result.imported,
                result.skipped,
                result.files_scanned,
                result.errors.clone(),
            ),
            Err(error) => (0, 0, 0, vec![error.clone()]),
        };
        total_imported += u64::from(imported);
        errors.extend(
            source_errors
                .iter()
                .map(|e| format!("{}: {e}", source.name)),
        );
        source_rows.push(json!({
            "id": source.id,
            "name": source.name,
            "durationMs": source.duration_ms,
            "filesScanned": scanned,
            "filesChanged": changed,
            "imported": imported,
            "skipped": skipped,
            "newRows": rows_count,
            "tokens": tokens,
            "unattributedRows": unattributed,
            "errors": source_errors.len(),
            // Collectors that do not use the session_log_sync cursor (Gemini, ZCode, Antigravity)
            // have no per-file detail records, only source-level aggregates.
            "fileLevel": FILE_LEVEL_SOURCES.contains(&source.id),
        }));
    }
    let interesting =
        !files.is_empty() || total_rows > 0 || total_imported > 0 || !errors.is_empty();
    if matches!(trigger, ScanTrigger::Background) && !interesting {
        return;
    }
    let files_changed = files.len();
    record(
        db,
        Event {
            kind: "scan",
            title: "本机扫描".into(),
            status: if errors.is_empty() { "ok" } else { "error" },
            duration_ms: Some(started.elapsed().as_millis() as i64),
            summary: json!({
                "trigger": trigger.label(),
                "sources": sources.len(),
                "filesChanged": files_changed,
                "imported": total_imported,
                "newRows": total_rows,
                "tokens": total_tokens,
                "errors": errors.len(),
            }),
            detail: Some(json!({
                "sources": source_rows,
                "files": files,
                "errors": errors,
            })),
        },
    );
}

// ---------------------------------------------------------------------------
// Key operations: actions started from the UI through the unified command entry point
// that change data or issue external requests.

/// The operations that must be recorded and their display names; operations missing from the table (queries, polling) are not recorded.
pub fn action_label(op: &str, args: &Value) -> Option<&'static str> {
    Some(match op {
        // A sync triggered by the automatic refresh does not count as a user action; the scans it causes are recorded separately as scan records.
        "sync" if args.get("automatic").and_then(Value::as_bool) == Some(true) => return None,
        "sync" => "立即同步（本机扫描与全部设备）",
        "scan_local_only" => "扫描本机",
        "scan_remote_start" => "触发远端设备扫描",
        "scan_remote_sync" => "同步远端设备",
        "import_cc_switch" => "导入 CC Switch 历史",
        "pricing_update" => "修改模型价格",
        "pricing_batch" => "批量导入模型价格",
        "pricing_delete" => "删除模型价格",
        "modelsdev_config_save" => "保存 models.dev 同步设置",
        "modelsdev_fetch" => "从 models.dev 拉取价格（外部请求）",
        "preferences_save" => "修改刷新间隔",
        "sources_save" => "修改数据来源",
        "subscriptions_save" => "保存订阅与额度",
        "lan_settings_save" => "修改局域网共享设置",
        "lan_discover" => "扫描局域网设备",
        "lan_connect" => "连接设备",
        "lan_disconnect" => "断开设备",
        "lan_device_enabled" => "启用 / 停用设备",
        "lan_pair_code" => "生成配对码",
        "lan_incoming_revoke" => "撤销入站授权",
        "usage_api_key_rotate" => "生成只读 API 密钥",
        "usage_api_key_revoke" => "停用只读 API 密钥",
        "network_international_ipv4" => "查询国际出口 IP（外部请求）",
        _ => return None,
    })
}

/// The arguments with sensitive fields removed and large arrays collapsed, for display in the log.
pub fn sanitize_args(args: &Value) -> Value {
    const SENSITIVE: [&str; 6] = ["code", "key", "token", "reversetoken", "apikey", "password"];
    match args {
        Value::Object(map) => {
            let mut out = Map::new();
            for (name, value) in map {
                let lower = name.to_ascii_lowercase();
                if SENSITIVE.contains(&lower.as_str()) {
                    out.insert(name.clone(), json!("（已隐藏）"));
                } else {
                    out.insert(name.clone(), sanitize_args(value));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) if items.len() > 5 => json!(format!("[{} 项]", items.len())),
        Value::Array(items) => Value::Array(items.iter().map(sanitize_args).collect()),
        Value::String(text) if text.chars().count() > 200 => {
            json!(format!("{}…", text.chars().take(200).collect::<String>()))
        }
        other => other.clone(),
    }
}

/// Small fields of the operation result that are suitable for display (numbers, booleans, short strings), at most 8 of them.
pub fn result_brief(value: &Value) -> Value {
    let Value::Object(map) = value else {
        return Value::Null;
    };
    let mut out = Map::new();
    for (name, field) in map {
        if out.len() >= 8 {
            break;
        }
        match field {
            Value::Number(_) | Value::Bool(_) => {
                out.insert(name.clone(), field.clone());
            }
            Value::String(text)
                if text.chars().count() <= 80 && !name.eq_ignore_ascii_case("key") =>
            {
                out.insert(name.clone(), field.clone());
            }
            Value::Object(inner) if name == "device" || name == "scan" => {
                out.insert(name.clone(), result_brief(&Value::Object(inner.clone())));
            }
            _ => {}
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_hides_secrets_and_compacts() {
        let args = json!({
            "host": "192.168.1.5",
            "code": "123456",
            "reverseToken": "abc",
            "entries": [1, 2, 3, 4, 5, 6],
            "nested": {"apiKey": "x", "port": 17891}
        });
        let safe = sanitize_args(&args);
        assert_eq!(safe["host"], "192.168.1.5");
        assert_eq!(safe["code"], "（已隐藏）");
        assert_eq!(safe["reverseToken"], "（已隐藏）");
        assert_eq!(safe["entries"], "[6 项]");
        assert_eq!(safe["nested"]["apiKey"], "（已隐藏）");
        assert_eq!(safe["nested"]["port"], 17891);
    }

    #[test]
    fn result_brief_never_includes_key() {
        let brief = result_brief(&json!({"key": "secret-api-key", "configured": true}));
        assert!(brief.get("key").is_none());
        assert_eq!(brief["configured"], true);
    }

    #[test]
    fn normalize_ms_handles_units() {
        assert_eq!(normalize_ms(1_794_240_624), Some(1_794_240_624_000));
        assert_eq!(normalize_ms(1_794_240_624_512), Some(1_794_240_624_512));
        assert_eq!(
            normalize_ms(1_794_240_624_512_611_800),
            Some(1_794_240_624_512)
        );
        assert_eq!(normalize_ms(0), None);
    }
}
