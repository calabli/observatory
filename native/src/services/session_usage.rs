//! Usage tracking for the Claude Code session logs
//!
//! Extracts the token usage data from the JSONL session files below ~/.claude/projects/ and
//! thereby implements the usage statistics without a proxy.
//!
//! ## Data flow
//! ```text
//! ~/.claude/projects/*/*.jsonl -> incremental parse -> deduplication -> cost calculation -> proxy_request_logs table
//! ```

use crate::config::get_claude_config_dir;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::{CostCalculator, ModelPricing};
use crate::proxy::usage::parser::{claude_cache_creation_1h_tokens, TokenUsage};
use crate::services::usage_stats::{
    effective_usage_log_filter, find_model_pricing, has_matching_proxy_usage_log, DedupKey,
};
use rusqlite::OptionalExtension;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::SystemTime;

/// The sync result
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSyncResult {
    pub imported: u32,
    pub skipped: u32,
    pub files_scanned: u32,
    pub suspected_duplicates: u32,
    pub deferred_files: u32,
    pub errors: Vec<String>,
}

impl SessionSyncResult {
    pub fn merge(&mut self, other: SessionSyncResult) {
        self.imported = self.imported.saturating_add(other.imported);
        self.skipped = self.skipped.saturating_add(other.skipped);
        self.files_scanned = self.files_scanned.saturating_add(other.files_scanned);
        self.suspected_duplicates = self
            .suspected_duplicates
            .saturating_add(other.suspected_duplicates);
        self.deferred_files = self.deferred_files.saturating_add(other.deferred_files);
        self.errors.extend(other.errors);
    }
}

pub fn session_sync_mutex() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// The time the last complete scan ([`sync_all_unlocked`], which both the background timer
/// and the manual sync go through) finished,
/// as a millisecond timestamp; 0 means it has not been scanned yet since this startup. It only
/// lives in memory: a round is scanned right at the startup.
static LAST_SYNC_COMPLETED_AT_MS: AtomicI64 = AtomicI64::new(0);
static SCAN_REVISION: AtomicU64 = AtomicU64::new(0);

/// Record the time a scan finished; it only ever moves forward (a concurrent call cannot overwrite it with an earlier time).
pub(crate) fn record_sync_completed(at_ms: i64) {
    LAST_SYNC_COMPLETED_AT_MS.fetch_max(at_ms, Ordering::Relaxed);
    SCAN_REVISION.fetch_add(1, Ordering::Release);
}

/// Changes on every completed scan, including scans that found no new records.
pub fn scan_revision() -> u64 {
    SCAN_REVISION.load(Ordering::Acquire)
}

/// The time the last complete scan finished (a millisecond timestamp); `None` when it has not been scanned yet since this startup.
pub fn last_sync_completed_at() -> Option<i64> {
    let value = LAST_SYNC_COMPLETED_AT_MS.load(Ordering::Relaxed);
    (value > 0).then_some(value)
}

/// An in-memory snapshot of one row of the session_log_sync table.
///
/// The parsers prefetch the whole table once at the beginning of a scan with [`load_sync_cursors`]
/// instead of querying one row per file (the number of files only grows with the history, so a
/// per-file query means thousands of lock acquisitions per round). `last_synced_at` is the
/// encoded revision on the Pi path and the real sync timestamp on the other paths.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SyncCursor {
    pub last_modified: i64,
    pub last_line_offset: i64,
    pub last_byte_offset: Option<i64>,
    /// The fingerprint of the tail bytes before the cursor boundary (only written by the Claude
    /// path), used to recognize a file that was rewritten from the outside; NULL means there is
    /// no fingerprint to verify.
    pub last_tail_fingerprint: Option<i64>,
    pub last_synced_at: i64,
}

/// Prefetch the whole session_log_sync table of cursors once.
///
/// A failure has to abort this round of the sync, it must not fall back to treating an empty
/// table as a new database: `rollup_and_prune` aggregates the detail records older than
/// 30 days and deletes them, and the request_id deduplication only looks at the detail table
/// and is blind to the pruned entries --
/// an empty table fallback means a full re-import and the pruned old entries would be added
/// into the rollup again at the next rollup, permanently inflating the statistics.
pub(crate) fn load_sync_cursors(db: &Database) -> Result<HashMap<String, SyncCursor>, AppError> {
    let conn = lock_conn!(db.conn);
    let mut stmt = conn
        .prepare(
            "SELECT file_path, last_modified, last_line_offset, last_synced_at, last_byte_offset,
                    last_tail_fingerprint
             FROM session_log_sync",
        )
        .map_err(|e| AppError::Database(format!("预取同步游标失败: {e}")))?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            SyncCursor {
                last_modified: row.get(1)?,
                last_line_offset: row.get(2)?,
                last_synced_at: row.get(3)?,
                last_byte_offset: row.get(4)?,
                last_tail_fingerprint: row.get(5)?,
            },
        ))
    });
    rows.and_then(|rows| rows.collect::<Result<HashMap<_, _>, _>>())
        .map_err(|e| AppError::Database(format!("预取同步游标失败: {e}")))
}

fn merge_sync_step(
    aggregate: &mut SessionSyncResult,
    name: &str,
    step: Result<SessionSyncResult, AppError>,
) {
    match step {
        Ok(result) => aggregate.merge(result),
        Err(error) => aggregate.errors.push(format!("{name} 同步失败: {error}")),
    }
}

/// The caller has to hold [`session_sync_mutex`]. This function is the sync kernel and is
/// shared by the background task, the manual sync and the Codex rebuild, which avoids a
/// re-entrant tokio Mutex.
pub fn sync_all_unlocked(db: &Database) -> SessionSyncResult {
    let mut result = SessionSyncResult::default();
    merge_sync_step(&mut result, "Claude", sync_claude_session_logs(db));
    merge_sync_step(
        &mut result,
        "Codex",
        crate::services::session_usage_codex::sync_codex_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Gemini",
        crate::services::session_usage_gemini::sync_gemini_usage(db),
    );
    merge_sync_step(
        &mut result,
        "OpenCode",
        crate::services::session_usage_opencode::sync_opencode_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Grok Build",
        crate::services::session_usage_grokbuild::sync_grokbuild_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Pi",
        crate::services::session_usage_pi::sync_pi_usage(db),
    );
    merge_sync_step(
        &mut result,
        "MCode",
        crate::services::session_usage_mcode::sync_mcode_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Antigravity",
        crate::services::session_usage_agy::sync_agy_usage(db),
    );
    merge_sync_step(
        &mut result,
        "Kimi Code",
        crate::services::session_usage_kimi::sync_kimi_usage(db),
    );
    merge_sync_step(
        &mut result,
        "DeepSeekHarness",
        crate::services::session_usage_dsh::sync_dsh_usage(db),
    );
    merge_sync_step(
        &mut result,
        "ZCode",
        crate::services::session_usage_zcode::sync_zcode_usage(db),
    );
    notify_sync_result(&result);
    record_sync_completed(chrono::Utc::now().timestamp_millis());
    result
}

pub(crate) fn notify_sync_result(result: &SessionSyncResult) {
    if result.imported > 0 {
        crate::usage_events::notify_log_recorded();
    }
}

/// The distribution of the data sources
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceSummary {
    pub data_source: String,
    pub request_count: u32,
    pub total_cost_usd: String,
}

/// The usage data of an assistant message parsed out of a JSONL
#[derive(Debug)]
struct ParsedAssistantUsage {
    message_id: String,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
    cache_read_tokens: u32,
    cache_creation_tokens: u32,
    cache_creation_1h_tokens: u32,
    stop_reason: Option<String>,
    timestamp: Option<String>,
    session_id: Option<String>,
}

/// An estimated request duration shorter than this many milliseconds is dropped: a real request cannot be that fast, the start is most likely wrong.
const MIN_PLAUSIBLE_LATENCY_MS: i64 = 100;

/// An estimated request duration longer than this many milliseconds is dropped as well: there is most likely a sleep or a long retry wait in between.
const MAX_PLAUSIBLE_LATENCY_MS: i64 = 60 * 60 * 1000;

/// The session logs have no request timing, so the duration can only be estimated by subtracting two timestamps (it includes the wait for the first token).
/// A value outside the plausible range returns None and is stored as 0, which means there is no timing.
///
/// Shared by the Claude and Codex session parsers.
pub(crate) fn estimated_latency_ms(start_ms: i64, end_ms: i64) -> Option<i64> {
    let latency = end_ms.checked_sub(start_ms)?;
    (MIN_PLAUSIBLE_LATENCY_MS..=MAX_PLAUSIBLE_LATENCY_MS)
        .contains(&latency)
        .then_some(latency)
}

pub(crate) fn parse_timestamp_millis(timestamp: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// The position of a log line in the conversation chain; only the three things that are needed to estimate the request duration are kept.
struct ChainNode {
    parent: Option<uuid::Uuid>,
    timestamp_ms: Option<i64>,
    /// The timestamp of an `attachment` line is unreliable (some are only written after the reply started), so it is skipped when the start of the request is looked up.
    is_attachment: bool,
}

type ChainNodes = HashMap<uuid::Uuid, ChainNode>;

fn parse_line_uuid(value: &serde_json::Value, key: &str) -> Option<uuid::Uuid> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
}

fn record_chain_node(chain: &mut ChainNodes, value: &serde_json::Value) {
    let Some(uuid) = parse_line_uuid(value, "uuid") else {
        return;
    };
    chain.insert(
        uuid,
        ChainNode {
            parent: parse_line_uuid(value, "parentUuid"),
            timestamp_ms: value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_timestamp_millis),
            is_attachment: value.get("type").and_then(|t| t.as_str()) == Some("attachment"),
        },
    );
}

/// The number of bytes that are looked back before the cursor during an incremental read. The
/// first chunk of a reply often follows right after the cursor while the line it starts at
/// (the user message, the tool result) is before the cursor: the tool result lands on disk
/// first while the reply is only written in one go when it is complete. When the starting line
/// is bigger than this window (a tool result with an image for example), the duration of that
/// request is not estimated.
const CHAIN_SEED_BYTES: i64 = 256 * 1024;

/// Record the lines of a small section before the cursor in the conversation chain and in the
/// reply timing (only those two, nothing is imported). The first chunks of a reply may already
/// have been read by the previous round, this round needs them to know which chunk the reply
/// starts at. After the call the file position is at `offset`. Failing to read the look-back is
/// not an error, it just means a few durations are not estimated.
fn read_chain_seed(
    file: &mut fs::File,
    offset: i64,
    chain: &mut ChainNodes,
    timings: &mut HashMap<String, MessageTiming>,
) -> Result<(), AppError> {
    let len = offset.clamp(0, CHAIN_SEED_BYTES);
    if len == 0 {
        return Ok(());
    }
    let mut window = vec![0u8; len as usize];
    let read_ok = file
        .seek(SeekFrom::Start((offset - len) as u64))
        .and_then(|_| file.read_exact(&mut window))
        .is_ok();
    file.seek(SeekFrom::Start(offset as u64))
        .map_err(|e| AppError::Config(format!("无法定位文件偏移: {e}")))?;
    if !read_ok {
        return Ok(());
    }

    let mut lines = window.split(|b| *b == b'\n');
    if offset > len {
        // The window does not start at the head of the file, so the first section is most likely a half line
        lines.next();
    }
    for line in lines {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) {
            record_chain_node(chain, &value);
            if let Some(msg_id) = assistant_message_id(&value) {
                record_block_timing(timings, &value, msg_id);
            }
        }
    }
    Ok(())
}

/// The `message.id` of an assistant line; the other lines return None.
fn assistant_message_id(value: &serde_json::Value) -> Option<&str> {
    if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
        return None;
    }
    value.get("message")?.get("id")?.as_str()
}

/// The timing clues of one reply (all the content blocks of the same message.id).
struct MessageTiming {
    /// The parent line of the block that appears first in the file; the start of the request is looked up along it.
    first_parent: Option<uuid::Uuid>,
    /// Whether the block that appears first is the first block of this reply. If it is not (the
    /// earlier blocks are outside the look-back window) the start is taken too late and the speed
    /// too high, so it is better not to estimate. The old logs have no `apiBlockIndex` and are
    /// treated as if they did.
    starts_at_first_block: bool,
    /// The moment the reply started to output, that is the first token: when the first block is a
    /// thinking block, the moment it was written minus the `thinkingDurationMs` that Claude Code
    /// records (from the content_block_start of that block to the content_block_stop).
    /// When the first block is text or a tool call, the timestamp of the block is only the moment
    /// it was written, so the first token cannot be derived and is None.
    first_output_ms: Option<i64>,
    /// The latest of the block timestamps (the moments the blocks were written), that is the end of the reply.
    end_ms: Option<i64>,
}

/// Record a content block in the timing of the reply it belongs to: the block that appears first sets the start, the latest block sets the end.
fn record_block_timing(
    timings: &mut HashMap<String, MessageTiming>,
    value: &serde_json::Value,
    msg_id: &str,
) {
    let block_end_ms = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(parse_timestamp_millis);
    let timing = timings.entry(msg_id.to_string()).or_insert_with(|| {
        let starts_at_first_block = value
            .get("apiBlockIndex")
            .is_none_or(|index| index.as_u64() == Some(0));
        MessageTiming {
            first_parent: parse_line_uuid(value, "parentUuid"),
            starts_at_first_block,
            first_output_ms: starts_at_first_block
                .then(|| thinking_block_start_ms(value, block_end_ms?))
                .flatten(),
            end_ms: None,
        }
    });
    timing.end_ms = timing.end_ms.max(block_end_ms);
}

/// The moment the thinking block started: the moment the block was written minus
/// `thinkingDurationMs`. It is None when it is not a thinking block or the field is missing
/// (older Claude Code).
fn thinking_block_start_ms(value: &serde_json::Value, block_end_ms: i64) -> Option<i64> {
    let duration_ms = value.get("thinkingDurationMs")?.as_i64()?;
    let first_block_type = value
        .get("message")?
        .get("content")?
        .get(0)?
        .get("type")?
        .as_str()?;
    (first_block_type == "thinking" && duration_ms > 0).then(|| block_end_ms - duration_ms)
}

/// The time to the first token: from the start until the output started. It has to fall into (0, duration], otherwise the start or the first token is wrong.
pub(crate) fn estimated_first_token_ms(
    start_ms: i64,
    first_output_ms: i64,
    latency_ms: i64,
) -> Option<i64> {
    let first_token = first_output_ms.checked_sub(start_ms)?;
    (first_token > 0 && first_token <= latency_ms).then_some(first_token)
}

/// At most this many lines are walked up along the parent chain.
const MAX_CHAIN_HOPS: usize = 32;

/// The start of the request: from the parent line of the first block of the reply upwards,
/// skipping the `attachment` lines, the timestamp of the first other line (the user
/// message, the tool result, the system line of an error retry).
fn resolve_request_start_ms(chain: &ChainNodes, first_parent: Option<uuid::Uuid>) -> Option<i64> {
    let mut cursor = first_parent?;
    for _ in 0..MAX_CHAIN_HOPS {
        let node = chain.get(&cursor)?;
        if !node.is_attachment {
            return node.timestamp_ms;
        }
        cursor = node.parent?;
    }
    None
}

/// Sync the Claude Code session logs into the usage statistics database
pub fn sync_claude_session_logs(db: &Database) -> Result<SessionSyncResult, AppError> {
    let projects_dir = get_claude_config_dir().join("projects");
    if !projects_dir.exists() {
        return Ok(SessionSyncResult {
            imported: 0,
            skipped: 0,
            files_scanned: 0,
            suspected_duplicates: 0,
            deferred_files: 0,
            errors: vec![],
        });
    }

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: 0,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };

    // Collect all the .jsonl files
    let jsonl_files = collect_jsonl_files(&projects_dir);
    let cursors = load_sync_cursors(db)?;

    for file_path in &jsonl_files {
        result.files_scanned += 1;

        let cursor = cursors.get(file_path.to_string_lossy().as_ref());
        match sync_single_file(db, file_path, cursor) {
            Ok(file_sync) => {
                result.imported += file_sync.imported;
                result.skipped += file_sync.skipped;
                if file_sync.incomplete_tail || file_sync.read_error.is_some() {
                    result.deferred_files += 1;
                }
                if let Some(err) = file_sync.read_error {
                    let msg = format!(
                        "{}: 读取中断，已入库部分保留、下轮从断点续读: {err}",
                        file_path.display()
                    );
                    log::warn!("[SESSION-SYNC] {msg}");
                    result.errors.push(msg);
                }
                if let Some(reason) = file_sync.pinned_rewrite {
                    // Skip permanently (to prevent a double count of the pruned detail records), there is no
                    // retry in the next round -- the
                    // user of a manual sync has to see it, it must not show up as a successful sync where nothing
                    // happened
                    result.errors.push(format!(
                        "{}: 检测到文件被外部{reason}，改写区间已跳过以防重复计数（不会再导入）",
                        file_path.display()
                    ));
                }
            }
            Err(e) => {
                let msg = format!("{}: {e}", file_path.display());
                log::warn!("[SESSION-SYNC] 文件解析失败: {msg}");
                result.errors.push(msg);
            }
        }
    }

    if result.imported > 0 {
        log::info!(
            "[SESSION-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.files_scanned
        );
    }

    Ok(result)
}

/// Collect all the .jsonl files below a directory (including the sub agent files)
///
/// The scan has a fixed depth and is not recursive, which avoids an endless loop:
///   projects_dir/<project dir>/*.jsonl                                      (the main session)
///   projects_dir/<project dir>/SESSION_ID/subagents/*.jsonl                  (the Task/Agent sub agents)
///   projects_dir/<project dir>/SESSION_ID/subagents/workflows/wf_*/*.jsonl   (the Workflow sub agents)
///
/// The last level are the sub agent transcripts produced by the Claude Code Workflow
/// feature; they nest one level deeper than the normal sub agents with a `workflows/wf_<ID>/`.
/// Missing that level would leave the token usage of the Workflows completely out of the
/// statistics; `journal.jsonl` has no `type=="assistant"` line and is naturally skipped by
/// `sync_single_file` while parsing, so no filtering by file name is needed here.
fn collect_jsonl_files(projects_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let entries = match fs::read_dir(projects_dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // The .jsonl files of every project directory
        if let Ok(sub_entries) = fs::read_dir(&path) {
            for sub_entry in sub_entries.flatten() {
                let sub_path = sub_entry.path();
                if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    // The JSONL file of the main session
                    files.push(sub_path);
                } else if sub_path.is_dir() {
                    // Scan the sub agent directories: <project>/SESSION_ID/subagents/*.jsonl
                    let subagents_dir = sub_path.join("subagents");
                    if subagents_dir.is_dir() {
                        push_jsonl_children(&subagents_dir, &mut files);

                        // Additionally descend into the Workflow sub agents:
                        // <project>/SESSION_ID/subagents/workflows/wf_<ID>/*.jsonl
                        let workflows_dir = subagents_dir.join("workflows");
                        if workflows_dir.is_dir() {
                            if let Ok(wf_entries) = fs::read_dir(&workflows_dir) {
                                for wf_entry in wf_entries.flatten() {
                                    let wf_path = wf_entry.path();
                                    if wf_path.is_dir() {
                                        push_jsonl_children(&wf_path, &mut files);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    files
}

/// Append all the `.jsonl` files of the direct children of `dir` to `files` (not recursive).
fn push_jsonl_children(dir: &Path, files: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                files.push(path);
            }
        }
    }
}

/// The result of the sync of a single file
#[derive(Debug, Default)]
struct ClaudeFileSync {
    imported: u32,
    skipped: u32,
    /// There is an incomplete fragment at the end of the file that is not terminated with a `\n`
    /// (the writer may be appending right now). A fragment that can be
    /// parsed successfully is still imported (the request_id deduplication prevents duplicates), but the
    /// cursor does not move past it
    /// and it is confirmed again after the next round completed it.
    incomplete_tail: bool,
    /// An IO read error in the middle of the parsing. The part that was already committed is
    /// stored as usual and the cursor gets the old mtime so that the next round
    /// continues reading, but it has to be reported upwards: a manual sync has no "next round",
    /// and a silent return would make the user treat an unfinished file as a successful sync.
    read_error: Option<String>,
    /// An external truncation / rewrite was detected and the cursor was pinned to the current EOF
    /// (the value is the description of the reason). The old range
    /// is skipped permanently to prevent a double count of the pruned detail records -- this is
    /// not deferred (the next round will not retry),
    /// it has to go through the errors so that the user learns about an unrecoverable skip.
    pinned_rewrite: Option<&'static str>,
}

/// The number of tail bytes covered by the fingerprint of the cursor boundary (the same value as the `REVISION_TAIL_BYTES` of Pi).
const TAIL_FINGERPRINT_BYTES: i64 = 4096;

/// The fingerprint of the tail bytes before the cursor boundary. The domain label prevents a mix-up with the hashes of other uses.
fn claude_tail_fingerprint(tail: &[u8]) -> i64 {
    let mut hasher = Sha256::new();
    hasher.update(b"claude-session-tail-v1");
    hasher.update(tail);
    let digest = hasher.finalize();
    i64::from(u32::from_be_bytes(
        digest[..4].try_into().unwrap_or_default(),
    ))
}

/// Read at most [`TAIL_FINGERPRINT_BYTES`] bytes before `end`; after the call the file position
/// is exactly at `end`, so the incremental path can continue reading from there.
fn read_tail_before(file: &mut fs::File, end: i64) -> Result<Vec<u8>, AppError> {
    let len = end.clamp(0, TAIL_FINGERPRINT_BYTES);
    let mut tail = vec![0u8; len as usize];
    file.seek(SeekFrom::Start((end - len) as u64))
        .map_err(|e| AppError::Config(format!("无法定位文件偏移: {e}")))?;
    if len > 0 {
        file.read_exact(&mut tail)
            .map_err(|e| AppError::Config(format!("无法读取游标边界尾部: {e}")))?;
    }
    Ok(tail)
}

/// Append the bytes of the committed line to the rolling tail buffer and only keep the size of the trailing fingerprint window.
fn push_committed_tail(tail_buf: &mut Vec<u8>, bytes: &[u8]) {
    tail_buf.extend_from_slice(bytes);
    let max = TAIL_FINGERPRINT_BYTES as usize;
    if tail_buf.len() > max {
        tail_buf.drain(..tail_buf.len() - max);
    }
}

/// Sync a single JSONL file.
///
/// Incremental semantics: the cursor is a byte offset (`last_byte_offset`) and when the file is
/// appended to it seeks to the cursor and only reads the new tail, which avoids re-reading the
/// whole file of an active session in every round. The cursor only advances past the last
/// complete line (the one that ends with `\n`) -- an old cursor that counted by line numbers
/// also counted an unfinished half line, so that the line was skipped once it was completed
/// and the record was lost forever.
///
/// When `last_byte_offset` is NULL (the line number cursors of before the upgrade) the first L
/// lines are skipped by the old line number, the position is converted to a byte position and
/// the incremental read continues. A fallback to a full re-read is not allowed:
/// `rollup_and_prune` aggregates the detail records older than 30 days and deletes them, the
/// request_id deduplication only looks at the detail table and is blind to the pruned entries,
/// and a re-import would add them a second time at the next rollup, permanently inflating
/// the statistics. The price is that the history rows that the half line bug of the old line
/// number cursors swallowed cannot be recovered (without a persistent ledger they cannot be
/// told apart from the pruned entries, and a double count is worse than a lost row).
///
/// The same double count risk decides how a non-append change is handled (on the normal path
/// Claude Code only appends, so this case only comes from outside interference): a truncation
/// (the cursor is past the end of the file) and a rewrite (the fingerprint of the tail before
/// the cursor boundary does not match -- a replacement of the same or a larger size cannot be
/// detected through the size) both pin the cursor to the current EOF, replay none of the old
/// range and the later appends go back to the normal incremental path.
///
/// The transaction granularity of the bulk insert of a single file: the UI queries are blocked
/// by the connection mutex for a few milliseconds within a batch, the lock is released between
/// the batches so that the read side can jump the queue (the same pattern as on the Codex
/// path), and the first import of a big file no longer holds the lock for seconds to tens of
/// seconds.
const SESSION_INSERT_BATCH_SIZE: usize = 1000;

fn sync_single_file(
    db: &Database,
    file_path: &Path,
    cursor: Option<&SyncCursor>,
) -> Result<ClaudeFileSync, AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // Fetch the file metadata
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);
    let file_size = metadata.len() as i64;

    let last_modified = cursor.map_or(0, |c| c.last_modified);
    let last_byte_offset = cursor.and_then(|c| c.last_byte_offset);
    let last_fingerprint = cursor.and_then(|c| c.last_tail_fingerprint);

    // Skip the file when it did not change
    if file_modified <= last_modified {
        return Ok(ClaudeFileSync::default());
    }

    let mut file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;

    // Detection of a non-append change (only on the byte cursor path). After the detection the
    // cursor is pinned to the current EOF and none of the old offsets
    // is replayed (see the function documentation: a replay would double count the pruned detail
    // records into the rollup). When the fingerprint is NULL
    // (before the first round after the existing rows were converted) there is nothing to verify
    // and it is treated as a pure append -- that is an exposure the old
    // line number cursors had anyway, and there is a fingerprint after the first write.
    let mut chain = ChainNodes::new();
    let mut timings: HashMap<String, MessageTiming> = HashMap::new();
    let (start_byte, legacy_lines, mut tail_buf) = match last_byte_offset {
        Some(offset) => {
            let truncated = !(0..=file_size).contains(&offset);
            // The seed is used both for the fingerprint verification and for the rolling tail buffer; after the read the file position is exactly at the offset, so no explicit seek is needed
            let seed = if truncated {
                None
            } else {
                Some(read_tail_before(&mut file, offset)?)
            };
            let rewritten = match (&seed, last_fingerprint) {
                (Some(seed), Some(expected)) => claude_tail_fingerprint(seed) != expected,
                _ => false,
            };
            if truncated || rewritten {
                let reason = if truncated { "截断" } else { "重写" };
                log::warn!(
                    "[SESSION-SYNC] 文件被外部{reason}，游标钉至 EOF、不重放旧区间: {}",
                    file_path.display()
                );
                let tail = read_tail_before(&mut file, file_size)?;
                let fingerprint = claude_tail_fingerprint(&tail);
                let conn = lock_conn!(db.conn);
                update_claude_sync_state_on_conn(
                    &conn,
                    &file_path_str,
                    file_modified,
                    file_size,
                    Some(fingerprint),
                )?;
                return Ok(ClaudeFileSync {
                    pinned_rewrite: Some(reason),
                    ..Default::default()
                });
            }
            read_chain_seed(&mut file, offset, &mut chain, &mut timings)?;
            (offset, 0, seed.unwrap_or_default())
        }
        // The old line number cursor: convert it from the head line by line (the conversion section below), the tail buffer is built up from empty
        None => (
            0,
            cursor.map_or(0, |c| c.last_line_offset.max(0)),
            Vec::new(),
        ),
    };

    let mut reader = BufReader::new(file);

    // The cursor only advances past the last complete line
    let mut committed_offset = start_byte;
    let mut incomplete_tail = false;
    let mut buf: Vec<u8> = Vec::new();

    // The conversion of an old line number into a byte position: it only counts bytes, it does not
    // parse and does not import. The old `lines()` also counted a tail without a line break as a
    // line number and the read_until here counts one line per return as well, so the byte position
    // lines up exactly with the old cursor. When the file has fewer than L lines (a truncation)
    // it stops at the EOF, which is the same as the old cursor semantics of
    // "the new content has a too small line number and is skipped" for a truncated file. The
    // conversion has to run to the end before a cursor may be written:
    // a read error in the middle that was stored as usual would clear last_line_offset, lose the
    // number of lines that still have to be skipped and make the next round
    // re-import the rows that the old code already imported -- so an error fails the whole file here.
    let mut skipped_legacy_lines: i64 = 0;
    while skipped_legacy_lines < legacy_lines {
        buf.clear();
        let read = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| AppError::Config(format!("转换旧行号游标失败: {e}")))?;
        if read == 0 {
            break;
        }
        push_committed_tail(&mut tail_buf, &buf);
        committed_offset += read as i64;
        skipped_legacy_lines += 1;
    }

    let mut read_error: Option<String> = None;
    let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();
    let mut current_session_id: Option<String> = None;

    loop {
        buf.clear();
        let read = match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                // An IO error: the part that was already committed is stored as usual; the cursor gets the old
                // mtime so that the next round
                // passes the mtime gate and continues reading from committed_offset (see the write).
                // The error message is passed upwards: a manual sync has no next round, the user has to see it
                read_error = Some(e.to_string());
                break;
            }
        };
        if buf.ends_with(b"\n") {
            push_committed_tail(&mut tail_buf, &buf);
            committed_offset += read as i64;
        } else {
            incomplete_tail = true;
        }

        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_slice(&buf) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract the session ID (from the system line or the first message)
        if current_session_id.is_none() {
            if let Some(sid) = value.get("sessionId").and_then(|v| v.as_str()) {
                current_session_id = Some(sid.to_string());
            }
        }

        // Every line is recorded in the conversation chain, the start of the request is looked up along it while estimating the request duration
        record_chain_node(&mut chain, &value);

        // Only the messages of type assistant are processed
        if value.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }

        let message = match value.get("message") {
            Some(m) => m,
            None => continue,
        };

        let msg_id = match message.get("id").and_then(|v| v.as_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };

        record_block_timing(&mut timings, &value, &msg_id);

        let usage = match message.get("usage") {
            Some(u) => u,
            None => continue,
        };

        let parsed = ParsedAssistantUsage {
            message_id: msg_id.clone(),
            model: message
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string(),
            input_tokens: usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            output_tokens: usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_1h_tokens: claude_cache_creation_1h_tokens(usage),
            stop_reason: message
                .get("stop_reason")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            timestamp: value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            session_id: current_session_id.clone(),
        };

        // Deduplicate by message.id: the entry with a stop_reason is preferred, otherwise the newest one is kept
        let should_replace = match messages.get(&msg_id) {
            None => true,
            Some(existing) => {
                // The new entry has a stop_reason and the old one has none -> replace
                if parsed.stop_reason.is_some() && existing.stop_reason.is_none() {
                    true
                }
                // Both have a stop_reason or neither has one -> take the one with the larger output_tokens
                else if parsed.stop_reason.is_some() == existing.stop_reason.is_some() {
                    parsed.output_tokens > existing.output_tokens
                } else {
                    false
                }
            }
        };

        if should_replace {
            messages.insert(msg_id, parsed);
        }
    }

    // Write to the database in batched transactions. Originally the whole file was one
    // transaction, which held the connection mutex for seconds to tens of seconds during the
    // first import / the conversion of an old cursor and made all the UI queries queue up.
    // Now there is one transaction per batch and the lock is released between the batches so that
    // the read side can jump the queue (the same pattern as on the Codex path). The earlier batches
    // are committed on their own; the cursor is only committed together with the last batch. When
    // it crashes in between, the committed batches are kept and the
    // cursor does not advance, the next round rescans those rows from the old cursor and the
    // request_id primary key plus INSERT OR IGNORE takes care of the deduplication, so there is no
    // double count.
    let mut imported: u32 = 0;
    let mut skipped: u32 = 0;

    let all_messages: Vec<_> = messages.values().collect();
    let batch_count = all_messages.len().div_ceil(SESSION_INSERT_BATCH_SIZE);
    for (batch_index, chunk) in all_messages.chunks(SESSION_INSERT_BATCH_SIZE).enumerate() {
        let is_last_batch = batch_index + 1 == batch_count;
        let conn = lock_conn!(db.conn);
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Database(format!("启动会话用量导入事务失败: {e}")))?;
        // The pricing is reused inside the file transaction that holds the connection lock (including
        // the models that were not found).
        // A new cache per batch: a pricing update or a new model between the transactions is visible
        // immediately.
        let mut pricing_cache = SessionPricingCache::default();

        for msg in chunk {
            // A row is imported as soon as it produced real billable tokens, a stop_reason or an output>0
            // is no longer required.
            //
            // Anthropic bills the input + cache_read + cache_creation as soon as it accepts the request
            // (those are fixed when the request starts), the output is billed by what it actually
            // generated. The parallel short-lived requests of a Workflow / a sub agent often only wrote a
            // message_start snapshot (output=1, stop_reason=None)
            // without writing the final block, even though their cache/input cost was really billed. The
            // old logic filtered on a non-empty stop_reason plus output>0 and dropped those requests
            // entirely, which systematically underestimated by about 4.1% in the measurements and
            // 92% of it was concentrated in the workflow/subagent. It is now "import when any billable
            // dimension is > 0".
            //
            // The deduplication logic (above: prefer the stop_reason, otherwise the largest output per
            // message.id) stays unchanged: the input/cache of the representative row it selects are
            // accurate anyway; the request_id = session:msg_id
            // primary key plus INSERT OR IGNORE makes sure that a message is still stored only once, so
            // relaxing the gate does not double count.
            let has_billable_tokens = msg.input_tokens > 0
                || msg.output_tokens > 0
                || msg.cache_read_tokens > 0
                || msg.cache_creation_tokens > 0;
            if !has_billable_tokens {
                continue;
            }

            let request_id = format!(
                "{}{}",
                crate::proxy::usage::parser::SESSION_REQUEST_ID_PREFIX,
                msg.message_id
            );

            // The duration is only estimated for the replies that were written completely (they have a
            // stop_reason) and whose first block can be seen
            // (in this batch or in the look-back window before the cursor), otherwise the start does not
            // line up
            let timing = timings
                .get(&msg.message_id)
                .filter(|timing| msg.stop_reason.is_some() && timing.starts_at_first_block)
                .and_then(|timing| {
                    let start_ms = resolve_request_start_ms(&chain, timing.first_parent)?;
                    let latency_ms = estimated_latency_ms(start_ms, timing.end_ms?)?;
                    let first_token_ms = timing.first_output_ms.and_then(|first_output_ms| {
                        estimated_first_token_ms(start_ms, first_output_ms, latency_ms)
                    });
                    Some(EstimatedTiming {
                        latency_ms,
                        first_token_ms,
                    })
                });

            match upsert_session_log_entry_on_conn(
                &tx,
                &request_id,
                msg,
                timing,
                &mut pricing_cache,
            ) {
                // A row that was completed also counts towards imported: the data changed, so the UI has to refresh as well
                Ok(SessionRowOutcome::Inserted | SessionRowOutcome::Updated) => imported += 1,
                Ok(SessionRowOutcome::Skipped) => skipped += 1,
                Err(e) => {
                    log::warn!("[SESSION-SYNC] 插入失败 ({}): {e}", msg.message_id);
                    skipped += 1;
                }
            }
        }

        // Update the sync state (the byte cursor). On a read error the old mtime is written instead
        // of the current one: writing the current one
        // would let the next round be skipped by the mtime gate and the unfinished complete lines
        // would only get a chance when the file changes again.
        // An incomplete_tail does not need this treatment -- completing a half line always comes
        // with an append that raises the mtime.
        // The cursor only advances with the last batch: when it crashes in between it is rolled back
        // together with the uncommitted data.
        if is_last_batch {
            let stamped_modified = if read_error.is_some() {
                last_modified
            } else {
                file_modified
            };
            // The tail_buf is always exactly the last bytes before the committed_offset: the incremental
            // path starts with the boundary
            // seed and only appends when the cursor advances (a complete line)
            let fingerprint = claude_tail_fingerprint(&tail_buf);
            update_claude_sync_state_on_conn(
                &tx,
                &file_path_str,
                stamped_modified,
                committed_offset,
                Some(fingerprint),
            )?;
        }
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交会话用量导入事务失败: {e}")))?;
    }

    // When this round has no importable message (the new ones are only user messages, tool results
    // and the like, or the line number cursor has
    // moved past the EOF) the loop above does not run at all, but the cursor still has to advance;
    // otherwise every round of the scan
    // would read this section again from the old position until the next assistant reply shows up.
    if batch_count == 0 {
        let stamped_modified = if read_error.is_some() {
            last_modified
        } else {
            file_modified
        };
        let conn = lock_conn!(db.conn);
        update_claude_sync_state_on_conn(
            &conn,
            &file_path_str,
            stamped_modified,
            committed_offset,
            Some(claude_tail_fingerprint(&tail_buf)),
        )?;
    }

    Ok(ClaudeFileSync {
        imported,
        skipped,
        incomplete_tail,
        read_error,
        pinned_rewrite: None,
    })
}

/// Write the byte cursor of the Claude path. `last_line_offset` is always written as 0: under
/// the byte cursor semantics
/// the line number is no longer maintained and the 0 clearly says "the line number cursor is
/// not available".
fn update_claude_sync_state_on_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    last_modified: i64,
    byte_offset: i64,
    tail_fingerprint: Option<i64>,
) -> Result<(), AppError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.prepare_cached(
        "INSERT OR REPLACE INTO session_log_sync
             (file_path, last_modified, last_line_offset, last_synced_at, last_byte_offset,
              last_tail_fingerprint)
         VALUES (?1, ?2, 0, ?3, ?4, ?5)",
    )
    .and_then(|mut stmt| {
        stmt.execute(rusqlite::params![
            file_path,
            last_modified,
            now,
            byte_offset,
            tail_fingerprint
        ])
    })
    .map_err(|e| AppError::Database(format!("更新同步状态失败: {e}")))?;
    Ok(())
}

/// Get the sync progress of one entry of the session_log_sync table.
///
/// The production path switched to the bulk prefetch [`load_sync_cursors`]; this single row
/// query is kept for the tests
/// that assert the cursor state.
#[cfg(test)]
pub(crate) fn get_sync_state(db: &Database, file_path: &str) -> Result<(i64, i64), AppError> {
    let conn = lock_conn!(db.conn);
    let result = conn.query_row(
        "SELECT last_modified, last_line_offset FROM session_log_sync WHERE file_path = ?1",
        rusqlite::params![file_path],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    );
    Ok(result.unwrap_or((0, 0)))
}

/// Return the nanosecond timestamp of the file mtime.
///
/// The old data of `session_log_sync.last_modified` is a second precision timestamp; writing
/// the nanosecond value does not need a schema migration, the old values naturally trigger one
/// incremental rescan and keep relying on the line offset to avoid a duplicate import.
pub(crate) fn metadata_modified_nanos(metadata: &fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Update the sync progress of one entry of the session_log_sync table.
///
/// Shared by all session_usage_* parsers.
pub(crate) fn update_sync_state(
    db: &Database,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    update_sync_state_on_conn(&conn, file_path, last_modified, last_offset)
}

/// The lock free version of [`update_sync_state`] for callers that advance the cursor inside a
/// transaction they already hold the lock in, so that the cursor advance and
/// the data insert are committed atomically.
pub(crate) fn update_sync_state_on_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Result<(), AppError> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    conn.prepare_cached(
        "INSERT OR REPLACE INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .and_then(|mut stmt| stmt.execute(rusqlite::params![file_path, last_modified, last_offset, now]))
    .map_err(|e| AppError::Database(format!("更新同步状态失败: {e}")))?;
    Ok(())
}

/// The result of writing one reply to the database.
#[derive(Debug, PartialEq, Eq)]
enum SessionRowOutcome {
    Inserted,
    /// This reply was stored earlier with an intermediate block and this time its usage or its
    /// duration was completed; or after the completion it turned out that it is the same request
    /// the router service recorded, so the earlier row was deleted.
    Updated,
    Skipped,
}

/// The timing estimated from the log timestamps: the duration (including the wait for the
/// first token) and the time to the first token (None when it cannot be derived).
/// Shared by the Claude and Codex session log imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EstimatedTiming {
    pub(crate) latency_ms: i64,
    pub(crate) first_token_ms: Option<i64>,
}

/// The few things of the row that is already in the database that are needed to complete it.
struct StoredSessionRow {
    data_source: String,
    output_tokens: i64,
    latency_ms: i64,
    first_token_ms: Option<i64>,
}

fn find_stored_session_row(
    conn: &rusqlite::Connection,
    request_id: &str,
) -> Result<Option<StoredSessionRow>, AppError> {
    conn.prepare_cached(
        "SELECT COALESCE(data_source, 'proxy'), output_tokens, latency_ms, first_token_ms
         FROM proxy_request_logs WHERE request_id = ?1",
    )
    .and_then(|mut stmt| {
        stmt.query_row([request_id], |row| {
            Ok(StoredSessionRow {
                data_source: row.get(0)?,
                output_tokens: row.get(1)?,
                latency_ms: row.get(2)?,
                first_token_ms: row.get(3)?,
            })
        })
        .optional()
    })
    .map_err(|e| AppError::Database(format!("查询已入库的会话日志失败: {e}")))
}

/// The pricing cache of the import transaction of a single file; it is dropped together with
/// the connection lock and no old price or miss is cached across transactions.
/// Shared by the collectors: in the same file hundreds of rows repeatedly hit a few models and
/// a price lookup per row
/// is a scan blown up for nothing.
#[derive(Default)]
pub(crate) struct SessionPricingCache {
    entries: HashMap<String, Option<ModelPricing>>,
    #[cfg(test)]
    lookup_count: usize,
}

impl SessionPricingCache {
    pub(crate) fn get(
        &mut self,
        conn: &rusqlite::Connection,
        model: &str,
    ) -> Option<&ModelPricing> {
        let Self {
            entries,
            #[cfg(test)]
            lookup_count,
        } = self;
        entries
            .entry(model.to_string())
            .or_insert_with(|| {
                #[cfg(test)]
                {
                    *lookup_count += 1;
                }
                find_model_pricing_for_session(conn, model)
            })
            .as_ref()
    }
}

/// The five costs of one reply according to the pricing: input, output, cache read, cache write and the total.
fn session_costs(
    conn: &rusqlite::Connection,
    msg: &ParsedAssistantUsage,
    pricing_cache: &mut SessionPricingCache,
) -> [String; 5] {
    let usage = TokenUsage {
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        cache_creation_1h_tokens: msg.cache_creation_1h_tokens,
        model: Some(msg.model.clone()),
        message_id: None,
    };
    match pricing_cache.get(conn, &msg.model) {
        Some(pricing) => {
            let cost = CostCalculator::calculate(&usage, pricing, Decimal::from(1));
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

/// Complete a reply that is already in the database.
///
/// The logs of a sub agent are written block by block and the earlier blocks only carry the
/// intermediate usage of that moment (a handful of output tokens).
/// When the sync happens to land while a reply is only half written, it is stored with the
/// intermediate values; once the final block arrives the output tokens, the cost and the
/// duration have to be filled in, otherwise the output of this reply is under-recorded
/// forever. The input and cache tokens
/// are already accurate in the first block.
///
/// Only the detail rows of the session log import are touched. The detail records older than
/// 30 days were already aggregated and deleted, but the blocks of one reply
/// are never written that far apart, so there is no case of "completing a reply that was
/// already rolled up".
fn refresh_session_log_entry_on_conn(
    conn: &rusqlite::Connection,
    request_id: &str,
    msg: &ParsedAssistantUsage,
    timing: Option<EstimatedTiming>,
    stored: &StoredSessionRow,
    dedup_key: &DedupKey,
    pricing_cache: &mut SessionPricingCache,
) -> Result<SessionRowOutcome, AppError> {
    if stored.data_source != "session_log" {
        return Ok(SessionRowOutcome::Skipped);
    }
    let output_grew = i64::from(msg.output_tokens) > stored.output_tokens;
    let latency_ms = timing.map_or(stored.latency_ms, |timing| {
        timing.latency_ms.max(stored.latency_ms)
    });
    // The first token only looks at the first block, the later blocks do not change it; only what could not be derived before is filled in
    let first_token_ms = stored
        .first_token_ms
        .or(timing.and_then(|timing| timing.first_token_ms));
    if !output_grew && latency_ms == stored.latency_ms && first_token_ms == stored.first_token_ms {
        return Ok(SessionRowOutcome::Skipped);
    }

    if !output_grew {
        conn.execute(
            "UPDATE proxy_request_logs SET latency_ms = ?1, first_token_ms = ?2
             WHERE request_id = ?3",
            rusqlite::params![latency_ms, first_token_ms, request_id],
        )
        .map_err(|e| AppError::Database(format!("更新会话日志耗时失败: {e}")))?;
        return Ok(SessionRowOutcome::Updated);
    }

    // The usage at the time of the insert does not match the row the router service recorded;
    // after the completion it matches, which means that the router service
    // already recorded this request and the row stored earlier is a duplicate
    if has_matching_proxy_usage_log(conn, dedup_key)? {
        conn.execute(
            "DELETE FROM proxy_request_logs WHERE request_id = ?1",
            [request_id],
        )
        .map_err(|e| AppError::Database(format!("删除重复的会话日志失败: {e}")))?;
        return Ok(SessionRowOutcome::Updated);
    }

    let [input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost] =
        session_costs(conn, msg, pricing_cache);
    conn.execute(
        "UPDATE proxy_request_logs SET
            input_tokens = ?1, output_tokens = ?2, cache_read_tokens = ?3,
            cache_creation_tokens = ?4, input_cost_usd = ?5, output_cost_usd = ?6,
            cache_read_cost_usd = ?7, cache_creation_cost_usd = ?8, total_cost_usd = ?9,
            latency_ms = ?10, first_token_ms = ?11
         WHERE request_id = ?12",
        rusqlite::params![
            msg.input_tokens,
            msg.output_tokens,
            msg.cache_read_tokens,
            msg.cache_creation_tokens,
            input_cost,
            output_cost,
            cache_read_cost,
            cache_creation_cost,
            total_cost,
            latency_ms,
            first_token_ms,
            request_id,
        ],
    )
    .map_err(|e| AppError::Database(format!("补全会话日志失败: {e}")))?;
    Ok(SessionRowOutcome::Updated)
}

/// Write one reply into proxy_request_logs: insert it when it was not stored yet and complete
/// it when needed
/// (see [`refresh_session_log_entry_on_conn`]).
///
/// The caller holds the connection lock (usually inside a transaction). `timing` is the request
/// duration (including the wait for the first token) and the time to the first token estimated
/// from the log timestamps, None when it cannot be estimated.
fn upsert_session_log_entry_on_conn(
    conn: &rusqlite::Connection,
    request_id: &str,
    msg: &ParsedAssistantUsage,
    timing: Option<EstimatedTiming>,
    pricing_cache: &mut SessionPricingCache,
) -> Result<SessionRowOutcome, AppError> {
    let created_at = msg
        .timestamp
        .as_ref()
        .and_then(|ts| {
            chrono::DateTime::parse_from_rfc3339(ts)
                .ok()
                .map(|dt| dt.timestamp())
        })
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });

    let dedup_key = DedupKey {
        app_type: "claude",
        model: &msg.model,
        input_tokens: msg.input_tokens,
        output_tokens: msg.output_tokens,
        cache_read_tokens: msg.cache_read_tokens,
        cache_creation_tokens: msg.cache_creation_tokens,
        created_at,
    };
    if let Some(stored) = find_stored_session_row(conn, request_id)? {
        return refresh_session_log_entry_on_conn(
            conn,
            request_id,
            msg,
            timing,
            &stored,
            &dedup_key,
            pricing_cache,
        );
    }
    if has_matching_proxy_usage_log(conn, &dedup_key)? {
        return Ok(SessionRowOutcome::Skipped);
    }

    let [input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost] =
        session_costs(conn, msg, pricing_cache);

    let inserted_rows = conn
        .execute(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
            rusqlite::params![
                request_id,
                "_session",         // provider_id: marks the origin as a session
                "claude",           // app_type
                msg.model,
                msg.model,          // request_model = model
                msg.input_tokens,
                msg.output_tokens,
                msg.cache_read_tokens,
                msg.cache_creation_tokens,
                input_cost,
                output_cost,
                cache_read_cost,
                cache_creation_cost,
                total_cost,
                timing.map_or(0, |t| t.latency_ms), // latency_ms: estimated from the timestamps, 0 = no timing
                timing.and_then(|t| t.first_token_ms), // first_token_ms: estimated from the timestamps, NULL when it cannot be derived
                200i64,             // status_code: a request of the session logs counts as successful as soon as it produced billable tokens
                Option::<String>::None, // error_message
                msg.session_id,
                Some("session_log"), // provider_type
                1i64,               // is_streaming: Claude Code usually streams
                "1.0",              // cost_multiplier
                created_at,
                "session_log",      // data_source
            ],
        )
        .map_err(|e| AppError::Database(format!("插入会话日志失败: {e}")))?;

    Ok(if inserted_rows > 0 {
        SessionRowOutcome::Inserted
    } else {
        SessionRowOutcome::Skipped
    })
}

/// Look the model pricing up in the model_pricing table (fuzzy matching is supported)
fn find_model_pricing_for_session(
    conn: &rusqlite::Connection,
    model_id: &str,
) -> Option<ModelPricing> {
    find_model_pricing(conn, model_id)
}

/// Query the distribution statistics of the data sources
pub fn get_data_source_breakdown(db: &Database) -> Result<Vec<DataSourceSummary>, AppError> {
    let conn = lock_conn!(db.conn);

    let effective_filter = effective_usage_log_filter("l");
    let sql = format!(
        "SELECT COALESCE(l.data_source, 'proxy') as ds, COUNT(*) as cnt,
                COALESCE(SUM(CAST(l.total_cost_usd AS REAL)), 0) as cost
         FROM proxy_request_logs l
         WHERE {effective_filter}
         GROUP BY ds
         ORDER BY cnt DESC"
    );

    let mut stmt = conn.prepare(&sql)?;

    let rows = stmt.query_map([], |row| {
        Ok(DataSourceSummary {
            data_source: row.get(0)?,
            request_count: row.get::<_, i64>(1)? as u32,
            total_cost_usd: format!("{:.6}", row.get::<_, f64>(2)?),
        })
    })?;

    let mut summaries = Vec::new();
    for row in rows {
        summaries.push(row.map_err(|e| AppError::Database(e.to_string()))?);
    }

    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricing_test_message(model: &str) -> ParsedAssistantUsage {
        ParsedAssistantUsage {
            message_id: "pricing-test".to_string(),
            model: model.to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            cache_creation_1h_tokens: 2,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("2026-06-07T13:01:23Z".to_string()),
            session_id: Some("pricing-session".to_string()),
        }
    }

    fn seed_test_pricing(conn: &rusqlite::Connection, model: &str, factor: u32) {
        conn.execute(
            "INSERT OR REPLACE INTO model_pricing (
                model_id, display_name, input_cost_per_million, output_cost_per_million,
                cache_read_cost_per_million, cache_creation_cost_per_million
             ) VALUES (?1, ?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                model,
                factor.to_string(),
                (2 * factor).to_string(),
                (3 * factor).to_string(),
                (4 * factor).to_string(),
            ],
        )
        .unwrap();
    }

    fn assert_test_costs(costs: &[String; 5], factor: u32, output_tokens: u32) {
        let output = Decimal::from(output_tokens * 2 * factor) / Decimal::from(1_000_000);
        let expected = [
            Decimal::new(100 * i64::from(factor), 6),
            output,
            Decimal::new(30 * i64::from(factor), 6),
            // 3 five-minute tokens + 2 one-hour tokens at 1.6x the write rate.
            Decimal::new(248 * i64::from(factor), 7),
            Decimal::new(1548 * i64::from(factor), 7) + output,
        ];
        let actual: [Decimal; 5] = std::array::from_fn(|index| costs[index].parse().unwrap());
        assert_eq!(actual, expected);
    }

    #[test]
    fn test_transaction_pricing_cache_reuses_hits_misses_and_streamed_updates(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);
        let tx = conn.unchecked_transaction()?;
        let models = [
            "pricing-model-a",
            "pricing-model-b",
            "pricing-model-missing",
        ];
        seed_test_pricing(&tx, models[0], 1);
        seed_test_pricing(&tx, models[1], 2);
        let mut cache = SessionPricingCache::default();
        let mut uncached_lookups = 0;

        for index in 0..100 {
            for (model_index, model) in models.iter().enumerate() {
                let msg = pricing_test_message(model);
                // A fresh cache per response reproduces the old lookup frequency.
                let mut uncached = SessionPricingCache::default();
                let expected = session_costs(&tx, &msg, &mut uncached);
                uncached_lookups += uncached.lookup_count;
                if model_index < 2 {
                    assert_test_costs(&expected, model_index as u32 + 1, msg.output_tokens);
                } else {
                    assert_eq!(expected, std::array::from_fn(|_| "0".to_string()));
                }
                let id = format!("session:pricing-{model_index}-{index}");
                assert_eq!(
                    upsert_session_log_entry_on_conn(&tx, &id, &msg, None, &mut cache)?,
                    SessionRowOutcome::Inserted
                );
                let stored: [String; 5] = tx.query_row(
                    "SELECT input_cost_usd, output_cost_usd, cache_read_cost_usd,
                            cache_creation_cost_usd, total_cost_usd
                     FROM proxy_request_logs WHERE request_id = ?1",
                    [&id],
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
                assert_eq!(stored, expected);
            }
        }
        assert_eq!(uncached_lookups, 300);
        assert_eq!(cache.lookup_count, 3);

        // Completing a partial reply shares the same model lookup with inserts.
        let mut completed = pricing_test_message(models[0]);
        completed.output_tokens = 40;
        assert_eq!(
            upsert_session_log_entry_on_conn(
                &tx,
                "session:pricing-0-0",
                &completed,
                None,
                &mut cache,
            )?,
            SessionRowOutcome::Updated
        );
        let total: String = tx.query_row(
            "SELECT total_cost_usd FROM proxy_request_logs WHERE request_id = ?1",
            ["session:pricing-0-0"],
            |row| row.get(0),
        )?;
        assert_eq!(total.parse::<Decimal>().unwrap(), Decimal::new(2348, 7));
        assert_eq!(cache.lookup_count, 3);
        tx.commit()?;
        Ok(())
    }

    #[test]
    fn test_new_transaction_observes_updated_and_previously_missing_pricing() -> Result<(), AppError>
    {
        let db = Database::memory()?;
        let conn = lock_conn!(db.conn);
        let priced = pricing_test_message("pricing-model-existing");
        let missing = pricing_test_message("pricing-model-new");
        seed_test_pricing(&conn, &priced.model, 1);
        {
            let tx = conn.unchecked_transaction()?;
            let mut cache = SessionPricingCache::default();
            for _ in 0..2 {
                assert_test_costs(&session_costs(&tx, &priced, &mut cache), 1, 20);
                assert_eq!(
                    session_costs(&tx, &missing, &mut cache),
                    std::array::from_fn(|_| "0".to_string())
                );
            }
            assert_eq!(cache.lookup_count, 2);
            tx.commit()?;
        }
        seed_test_pricing(&conn, &priced.model, 2);
        seed_test_pricing(&conn, &missing.model, 3);
        {
            let tx = conn.unchecked_transaction()?;
            let mut cache = SessionPricingCache::default();
            assert_test_costs(&session_costs(&tx, &priced, &mut cache), 2, 20);
            assert_test_costs(&session_costs(&tx, &missing, &mut cache), 3, 20);
            assert_eq!(cache.lookup_count, 2);
            tx.commit()?;
        }
        Ok(())
    }

    #[test]
    fn sync_result_notification_is_coalesced_to_one_call() {
        crate::usage_events::take_test_notify_count();
        notify_sync_result(&SessionSyncResult::default());
        let result = SessionSyncResult {
            imported: 25,
            ..SessionSyncResult::default()
        };
        notify_sync_result(&result);
        assert_eq!(crate::usage_events::take_test_notify_count(), 1);
    }

    #[test]
    fn last_sync_completed_at_only_moves_forward() {
        // The other tests run sync_all_unlocked and record "now", so a time one hour later is used here to stay clear of them
        let later = chrono::Utc::now().timestamp_millis() + 3_600_000;
        record_sync_completed(later);
        assert_eq!(last_sync_completed_at(), Some(later));

        record_sync_completed(later - 1_000);
        assert_eq!(last_sync_completed_at(), Some(later));
    }

    #[tokio::test]
    async fn session_sync_mutex_serializes_callers() {
        let first = session_sync_mutex().lock().await;
        assert!(session_sync_mutex().try_lock().is_err());
        drop(first);
        assert!(session_sync_mutex().try_lock().is_ok());
    }

    #[test]
    fn test_parse_usage_from_jsonl_line() {
        let line = r#"{"type":"assistant","message":{"id":"msg_test123","model":"claude-opus-4-6","usage":{"input_tokens":3,"output_tokens":150,"cache_read_input_tokens":5000,"cache_creation_input_tokens":10000},"stop_reason":"end_turn"},"timestamp":"2026-04-05T12:00:00Z","sessionId":"session-abc"}"#;

        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(
            value.get("type").and_then(|t| t.as_str()),
            Some("assistant")
        );

        let message = value.get("message").unwrap();
        let usage = message.get("usage").unwrap();

        assert_eq!(usage.get("input_tokens").unwrap().as_u64().unwrap(), 3);
        assert_eq!(usage.get("output_tokens").unwrap().as_u64().unwrap(), 150);
        assert_eq!(
            usage
                .get("cache_read_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            5000
        );
        assert_eq!(
            usage
                .get("cache_creation_input_tokens")
                .unwrap()
                .as_u64()
                .unwrap(),
            10000
        );
        assert_eq!(
            message.get("stop_reason").unwrap().as_str().unwrap(),
            "end_turn"
        );
    }

    #[test]
    fn test_dedup_by_message_id() {
        // The same message.id has several entries, the one with a stop_reason should be taken
        let mut messages: HashMap<String, ParsedAssistantUsage> = HashMap::new();

        // The intermediate entry (without stop_reason)
        let intermediate = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 26,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            cache_creation_1h_tokens: 0,
            stop_reason: None,
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };
        messages.insert("msg_1".to_string(), intermediate);

        // The final entry (with stop_reason)
        let final_entry = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-opus-4-6".to_string(),
            input_tokens: 3,
            output_tokens: 1349,
            cache_read_tokens: 5000,
            cache_creation_tokens: 10000,
            cache_creation_1h_tokens: 0,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("2026-04-05T12:00:00Z".to_string()),
            session_id: None,
        };

        // It should be replaced
        let should_replace = final_entry.stop_reason.is_some()
            && messages.get("msg_1").unwrap().stop_reason.is_none();
        assert!(should_replace);

        messages.insert("msg_1".to_string(), final_entry);
        assert_eq!(messages.get("msg_1").unwrap().output_tokens, 1349);
    }

    #[test]
    fn test_insert_claude_session_skips_matching_proxy_log() -> Result<(), AppError> {
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
                    "proxy-different-id",
                    "openai-compatible",
                    "claude",
                    "claude-sonnet-4-5",
                    "claude-sonnet-4-5",
                    100,
                    20,
                    10,
                    5,
                    "0.10",
                    100,
                    200,
                    1000,
                    "proxy"
                ],
            )?;
        }

        let msg = ParsedAssistantUsage {
            message_id: "msg_1".to_string(),
            model: "claude-sonnet-4-5".to_string(),
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 10,
            cache_creation_tokens: 5,
            cache_creation_1h_tokens: 0,
            stop_reason: Some("end_turn".to_string()),
            timestamp: Some("1970-01-01T00:16:45Z".to_string()),
            session_id: Some("session-1".to_string()),
        };

        let outcome = {
            let conn = lock_conn!(db.conn);
            upsert_session_log_entry_on_conn(
                &conn,
                "session:msg_1",
                &msg,
                None,
                &mut SessionPricingCache::default(),
            )?
        };
        assert_eq!(outcome, SessionRowOutcome::Skipped);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_collect_jsonl_files_includes_subagents() {
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let project = tmp.join("project");
        let session_dir = project.join("test-session");
        let subagents_dir = session_dir.join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();

        fs::write(project.join("main.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-abc.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        assert_eq!(files.len(), 2);
        let paths: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().any(|p| p.contains("main.jsonl")));
        assert!(paths.iter().any(|p| p.contains("agent-abc.jsonl")));

        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_collect_jsonl_files_includes_workflow_subagents() {
        // Claude Code Workflow embeds the sub agent transcripts below
        // <project>/SESSION_ID/subagents/workflows/wf_<ID>/, one level deeper than the normal sub agents.
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        let project = tmp.join("project");
        let session_dir = project.join("test-session");
        let subagents_dir = session_dir.join("subagents");
        let wf_dir = subagents_dir.join("workflows").join("wf_test123");
        fs::create_dir_all(&wf_dir).unwrap();

        fs::write(project.join("main.jsonl"), "{}").unwrap();
        fs::write(subagents_dir.join("agent-plain.jsonl"), "{}").unwrap();
        fs::write(wf_dir.join("agent-wf.jsonl"), "{}").unwrap();
        // journal.jsonl is collected as well, but it has no assistant line and therefore yields 0 entries while parsing
        fs::write(wf_dir.join("journal.jsonl"), "{}").unwrap();

        let files = collect_jsonl_files(&tmp);
        let paths: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();

        // The main session + the normal sub agents + the Workflow sub agents (agent-wf + journal) = 4
        assert_eq!(files.len(), 4);
        assert!(paths.iter().any(|p| p.contains("main.jsonl")));
        assert!(paths.iter().any(|p| p.contains("agent-plain.jsonl")));
        assert!(
            paths.iter().any(|p| p.contains("agent-wf.jsonl")),
            "Workflow 子 agent transcript 必须被收集"
        );

        fs::remove_dir_all(&tmp).ok();
    }

    /// Build one assistant JSONL line with billable tokens
    fn assistant_line(msg_id: &str, output_tokens: u32) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{msg_id}","model":"claude-opus-4-8","usage":{{"input_tokens":10,"output_tokens":{output_tokens},"cache_read_input_tokens":100,"cache_creation_input_tokens":50}},"stop_reason":"end_turn"}},"timestamp":"2026-06-07T13:01:23Z","sessionId":"session-x"}}"#
        )
    }

    fn byte_cursor(db: &Database, path: &Path) -> Option<i64> {
        load_sync_cursors(db)
            .unwrap()
            .get(path.to_string_lossy().as_ref())
            .and_then(|c| c.last_byte_offset)
    }

    fn sync_with_cursor(db: &Database, path: &Path) -> Result<ClaudeFileSync, AppError> {
        let cursors = load_sync_cursors(db)?;
        let cursor = cursors.get(path.to_string_lossy().as_ref()).copied();
        sync_single_file(db, path, cursor.as_ref())
    }

    fn bump_mtime(path: &Path) {
        // Two writes inside the test can land in the same tick of the file system mtime precision,
        // the mtime is explicitly moved forward so that the second round of the scan is not blocked
        // by the mtime gate
        let later = SystemTime::now() + std::time::Duration::from_secs(2);
        let file = fs::OpenOptions::new().append(true).open(path).unwrap();
        file.set_times(fs::FileTimes::new().set_modified(later))
            .unwrap();
    }

    #[test]
    fn test_incremental_append_advances_byte_cursor() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        fs::write(
            &file,
            format!(
                "{}\n{}\n",
                assistant_line("msg_a", 5),
                assistant_line("msg_b", 6)
            ),
        )
        .unwrap();
        let first = sync_with_cursor(&db, &file)?;
        assert_eq!(first.imported, 2);
        assert!(!first.incomplete_tail);
        let size_after_first = fs::metadata(&file).unwrap().len() as i64;
        assert_eq!(byte_cursor(&db, &file), Some(size_after_first));

        // After appending two lines only the new ones are imported, the cursor advances to the new end
        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(format!("{}\n", assistant_line("msg_c", 7)).as_bytes());
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!((second.imported, second.skipped), (1, 0));
        let size_after_second = fs::metadata(&file).unwrap().len() as i64;
        assert_eq!(byte_cursor(&db, &file), Some(size_after_second));

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_complete_tail_without_newline_imports_but_holds_cursor() -> Result<(), AppError> {
        // The tail is complete JSON but has no newline: it should be imported (no data loss), but the
        // cursor stops at
        // the end of the last complete line; after the newline was completed the rescan relies on the
        // request_id deduplication and does not double count
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        let first_line = format!("{}\n", assistant_line("msg_a", 5));
        fs::write(
            &file,
            format!("{first_line}{}", assistant_line("msg_tail", 6)),
        )
        .unwrap();
        let first = sync_with_cursor(&db, &file)?;
        assert_eq!(first.imported, 2, "无换行的完整尾段也必须导入");
        assert!(first.incomplete_tail);
        assert_eq!(
            byte_cursor(&db, &file),
            Some(first_line.len() as i64),
            "游标不得越过未终结的尾段"
        );

        // Completing the newline + appending a new line: the tail is rescanned but deduplicated, only the new line is imported
        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(format!("\n{}\n", assistant_line("msg_c", 7)).as_bytes());
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!((second.imported, second.skipped), (1, 1));
        assert!(!second.incomplete_tail);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_partial_line_completed_later_is_not_lost() -> Result<(), AppError> {
        // Regression (the data loss bug of the line number cursor): when the writer is stuck in the
        // middle of a half line the old implementation counted the half line in the
        // line number cursor and the line was skipped forever once it was completed because the
        // line_offset had already counted it. The byte
        // cursor only advances past the last complete line, so it has to be imported after the completion
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        let first_line = format!("{}\n", assistant_line("msg_a", 5));
        let full_second = assistant_line("msg_partial", 6);
        let (head, rest) = full_second.split_at(full_second.len() / 2);
        fs::write(&file, format!("{first_line}{head}")).unwrap();
        let first = sync_with_cursor(&db, &file)?;
        assert_eq!(first.imported, 1, "半行不可解析，只导入完整行");
        assert!(first.incomplete_tail);
        assert_eq!(byte_cursor(&db, &file), Some(first_line.len() as i64));

        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(format!("{rest}\n").as_bytes());
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!(second.imported, 1, "补全后的行必须被导入，不得因游标丢失");

        let conn = lock_conn!(db.conn);
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxy_request_logs WHERE request_id = 'session:msg_partial')",
            [],
            |row| row.get(0),
        )?;
        assert!(exists);
        drop(conn);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_truncated_file_pins_cursor_at_eof_without_replay() -> Result<(), AppError> {
        // The file was truncated from the outside (the cursor is past the new size): the cursor is pinned
        // to the current EOF and the old range is not replayed.
        // The critical case: the detail records were already pruned by rollup_and_prune (deleted here
        // after the import to simulate it), the request_id deduplication is
        // blind to them -- a fallback to a rescan from the head would re-import the
        // msg_a that is left in the truncated file and add it a second time at the next rollup. The
        // price (on purpose): the msg_a is
        // not replayed either, losing a row is better than double counting
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        fs::write(
            &file,
            format!(
                "{}\n{}\n",
                assistant_line("msg_a", 5),
                assistant_line("msg_b", 6)
            ),
        )
        .unwrap();
        assert_eq!(sync_with_cursor(&db, &file)?.imported, 2);

        // Simulate the rollup pruning the detail records
        {
            let conn = lock_conn!(db.conn);
            conn.execute("DELETE FROM proxy_request_logs", [])?;
        }

        fs::write(&file, format!("{}\n", assistant_line("msg_a", 5))).unwrap();
        bump_mtime(&file);
        let rescan = sync_with_cursor(&db, &file)?;
        assert_eq!(
            (rescan.imported, rescan.skipped),
            (0, 0),
            "截断后不重放任何旧区间"
        );
        assert_eq!(
            rescan.pinned_rewrite,
            Some("截断"),
            "永久跳过必须上报，不得静默成功"
        );
        let size = fs::metadata(&file).unwrap().len() as i64;
        assert_eq!(byte_cursor(&db, &file), Some(size), "游标钉在当前 EOF");
        {
            let conn = lock_conn!(db.conn);
            let rows: i64 =
                conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |r| r.get(0))?;
            assert_eq!(rows, 0, "已剪明细不得被重导");
        }

        // After the pinning the appends go back to the normal incremental path
        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(format!("{}\n", assistant_line("msg_c", 7)).as_bytes());
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let after = sync_with_cursor(&db, &file)?;
        assert_eq!((after.imported, after.skipped), (1, 0), "追加恢复增量");

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_same_size_rewrite_detected_by_tail_fingerprint() -> Result<(), AppError> {
        // A rewrite of the same size cannot be detected through the size (the cursor is still in
        // range), only the fingerprint of the tail before
        // the cursor boundary finds it. After the detection it is pinned to the EOF as well and
        // nothing is replayed: the rewritten content may contain
        // old events that were already pruned, so starting at the old offset or rescanning backwards
        // would double count
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        fs::write(&file, format!("{}\n", assistant_line("msg_a", 5))).unwrap();
        assert_eq!(sync_with_cursor(&db, &file)?.imported, 1);
        let original_size = fs::metadata(&file).unwrap().len() as i64;

        // A rewrite with the same length but different content (msg_x and msg_a have the same number of bytes)
        fs::write(&file, format!("{}\n", assistant_line("msg_x", 5))).unwrap();
        bump_mtime(&file);
        assert_eq!(
            fs::metadata(&file).unwrap().len() as i64,
            original_size,
            "前置条件：重写后文件大小不变"
        );
        let rescan = sync_with_cursor(&db, &file)?;
        assert_eq!(
            (rescan.imported, rescan.skipped),
            (0, 0),
            "指纹失配后不重放重写内容"
        );
        assert_eq!(
            rescan.pinned_rewrite,
            Some("重写"),
            "永久跳过必须上报，不得静默成功"
        );
        {
            let conn = lock_conn!(db.conn);
            let msg_x_rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = ?1",
                rusqlite::params![format!(
                    "{}msg_x",
                    crate::proxy::usage::parser::SESSION_REQUEST_ID_PREFIX
                )],
                |row| row.get(0),
            )?;
            assert_eq!(msg_x_rows, 0, "重写内容不得入库");
        }
        assert_eq!(byte_cursor(&db, &file), Some(original_size));

        // The fingerprint has been updated to the new content: the later appends go back to the normal incremental path
        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(format!("{}\n", assistant_line("msg_c", 7)).as_bytes());
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let after = sync_with_cursor(&db, &file)?;
        assert_eq!((after.imported, after.skipped), (1, 0), "追加恢复增量");

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_legacy_line_cursor_converts_without_reimport() -> Result<(), AppError> {
        // The upgrade path: the existing rows only have a line number cursor (last_byte_offset is
        // NULL) -> the first L lines are skipped by the line number and converted into a byte
        // position, only the new lines after them are imported. The critical case: the detail row of
        // msg_a was already pruned by rollup_and_prune (it is deliberately not pre-inserted here), the
        // request_id
        // deduplication is blind to it -- a fallback to a full re-read would re-import the msg_a and
        // add it a second time at the next rollup
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");
        let file_path_str = file.to_string_lossy().to_string();

        fs::write(
            &file,
            format!(
                "{}\n{}\n",
                assistant_line("msg_a", 5),
                assistant_line("msg_b", 6)
            ),
        )
        .unwrap();

        // The cursor of the old version: line number=1 (the old code imported the msg_a, the detail records were pruned), no byte cursor, old mtime
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
                 VALUES (?1, 1, 1, 1)",
                rusqlite::params![file_path_str],
            )?;
        }

        let result = sync_with_cursor(&db, &file)?;
        assert_eq!(
            (result.imported, result.skipped),
            (1, 0),
            "只导入行号游标之后的 msg_b，已剪的 msg_a 不重导"
        );
        let conn = lock_conn!(db.conn);
        let msg_a_rows: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE request_id = ?1",
            rusqlite::params![format!(
                "{}msg_a",
                crate::proxy::usage::parser::SESSION_REQUEST_ID_PREFIX
            )],
            |row| row.get(0),
        )?;
        drop(conn);
        assert_eq!(msg_a_rows, 0, "msg_a 不得被重导");
        let size = fs::metadata(&file).unwrap().len() as i64;
        assert_eq!(byte_cursor(&db, &file), Some(size), "转换后写入字节游标");

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_legacy_line_cursor_beyond_eof_imports_nothing() -> Result<(), AppError> {
        // The line number cursor is past the number of lines of the file (it became shorter after a
        // truncation / a rewrite): the conversion stops at the EOF and does not import
        // any row -- the same as the old line number cursor semantics of
        // "the new content has a too small line number and is skipped" for a truncated file
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");
        let file_path_str = file.to_string_lossy().to_string();

        fs::write(&file, format!("{}\n", assistant_line("msg_a", 5))).unwrap();
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
                 VALUES (?1, 1, 5, 1)",
                rusqlite::params![file_path_str],
            )?;
        }

        let result = sync_with_cursor(&db, &file)?;
        assert_eq!((result.imported, result.skipped), (0, 0));
        let size = fs::metadata(&file).unwrap().len() as i64;
        assert_eq!(byte_cursor(&db, &file), Some(size), "游标停在 EOF");

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_sync_imports_billable_message_without_stop_reason() -> Result<(), AppError> {
        // Regression: a message without a stop_reason but with a real cache/input cost (the shape that
        // is common for a Workflow / a
        // sub agent: "only a message_start snapshot, no final block") has to be counted and must not
        // be dropped entirely because of the missing stop_reason or output==0; a placeholder row with
        // all tokens 0 is still skipped.
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("agent-wf.jsonl");

        // The first line: no stop_reason, output=1, but a large cache_read/cache_creation -> it should be imported
        // The second line: all the tokens are 0 -> it should be skipped (no billing meaning)
        let billable = r#"{"type":"assistant","message":{"id":"msg_nostop","model":"claude-opus-4-8","usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":48719,"cache_creation_input_tokens":2061}},"timestamp":"2026-06-07T13:01:23Z","sessionId":"session-wf"}"#;
        let empty = r#"{"type":"assistant","message":{"id":"msg_empty","model":"claude-opus-4-8","usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"timestamp":"2026-06-07T13:01:24Z","sessionId":"session-wf"}"#;
        fs::write(&file, format!("{billable}\n{empty}\n")).unwrap();

        let file_sync = sync_single_file(&db, &file, None)?;
        assert_eq!(
            file_sync.imported, 1,
            "有 cache 成本但无 stop_reason 的 message 必须被导入"
        );

        let conn = lock_conn!(db.conn);
        let cache_read: i64 = conn.query_row(
            "SELECT cache_read_tokens FROM proxy_request_logs WHERE request_id = 'session:msg_nostop'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(cache_read, 48719, "cache_read 必须被完整记录");
        let empty_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM proxy_request_logs WHERE request_id = 'session:msg_empty')",
            [],
            |row| row.get(0),
        )?;
        assert!(!empty_exists, "全 0 token 的 message 应被跳过");
        drop(conn);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_sync_bills_1h_cache_writes_at_1h_rate() -> Result<(), AppError> {
        let db = Database::memory()?;
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");

        // cache_creation_input_tokens = the 5m writes + the 1h writes
        let line = r#"{"type":"assistant","message":{"id":"msg_1h","model":"claude-opus-4-8","usage":{"input_tokens":2,"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":1000,"cache_creation":{"ephemeral_5m_input_tokens":400,"ephemeral_1h_input_tokens":600}},"stop_reason":"end_turn"},"timestamp":"2026-06-07T13:01:23Z","sessionId":"session-1h"}"#;
        fs::write(&file, format!("{line}\n")).unwrap();
        assert_eq!(sync_single_file(&db, &file, None)?.imported, 1);

        let conn = lock_conn!(db.conn);
        let cache_creation_cost: String = conn.query_row(
            "SELECT cache_creation_cost_usd FROM proxy_request_logs WHERE request_id = 'session:msg_1h'",
            [],
            |row| row.get(0),
        )?;
        // claude-opus-4-8：400 × $6.25/M + 600 × $10/M = 0.0085
        assert_eq!(
            cache_creation_cost.parse::<Decimal>().unwrap(),
            Decimal::new(85, 4)
        );
        drop(conn);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    fn chain_uuid(n: u8) -> String {
        format!("00000000-0000-4000-8000-0000000000{n:02x}")
    }

    /// A log line without a usage (a user message, a tool result, an attachment), it only carries the chain information
    fn chain_line(kind: &str, uuid: u8, parent: Option<u8>, timestamp: &str) -> String {
        let parent = parent.map_or("null".to_string(), |p| format!("\"{}\"", chain_uuid(p)));
        format!(
            r#"{{"parentUuid":{parent},"type":"{kind}","uuid":"{}","timestamp":"{timestamp}","sessionId":"session-t"}}"#,
            chain_uuid(uuid)
        )
    }

    /// One content block of a reply; the blocks of the same reply share the message.id and the
    /// final usage
    /// (the way the main session writes it: the whole reply is written in one go when it ends)
    fn assistant_block(
        msg_id: &str,
        uuid: u8,
        parent: u8,
        block_index: u32,
        timestamp: &str,
    ) -> String {
        assistant_block_with(msg_id, uuid, parent, block_index, timestamp, 900, true)
    }

    /// The way a sub agent writes it is block by block: the non-final blocks only carry the output tokens of that moment and have no stop_reason
    fn assistant_block_with(
        msg_id: &str,
        uuid: u8,
        parent: u8,
        block_index: u32,
        timestamp: &str,
        output_tokens: u32,
        finished: bool,
    ) -> String {
        let stop_reason = if finished { "\"tool_use\"" } else { "null" };
        format!(
            r#"{{"parentUuid":"{}","type":"assistant","uuid":"{}","apiBlockIndex":{block_index},"message":{{"id":"{msg_id}","model":"claude-opus-4-8","usage":{{"input_tokens":10,"output_tokens":{output_tokens},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"stop_reason":{stop_reason}}},"timestamp":"{timestamp}","sessionId":"session-t"}}"#,
            chain_uuid(parent),
            chain_uuid(uuid)
        )
    }

    /// The first block of a reply is a thinking block: it carries the `thinkingDurationMs` that Claude Code records
    fn thinking_block(
        msg_id: &str,
        uuid: u8,
        parent: u8,
        timestamp: &str,
        thinking_duration_ms: i64,
        finished: bool,
    ) -> String {
        let stop_reason = if finished { "\"tool_use\"" } else { "null" };
        format!(
            r#"{{"parentUuid":"{}","type":"assistant","uuid":"{}","message":{{"id":"{msg_id}","model":"claude-opus-4-8","content":[{{"type":"thinking","thinking":""}}],"usage":{{"input_tokens":10,"output_tokens":900,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}},"stop_reason":{stop_reason}}},"thinkingDurationMs":{thinking_duration_ms},"apiBlockIndex":0,"timestamp":"{timestamp}","sessionId":"session-t"}}"#,
            chain_uuid(parent),
            chain_uuid(uuid)
        )
    }

    fn first_token_of(db: &Database, msg_id: &str) -> Option<i64> {
        let conn = db.conn.lock().unwrap();
        conn.query_row(
            "SELECT first_token_ms FROM proxy_request_logs WHERE request_id = ?1",
            [format!("session:{msg_id}")],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn append_lines(file: &Path, lines: &[String]) {
        let mut content = fs::read(file).unwrap_or_default();
        content.extend_from_slice((lines.join("\n") + "\n").as_bytes());
        fs::write(file, &content).unwrap();
        bump_mtime(file);
    }

    /// (output tokens, total cost, duration)
    fn stored_row(db: &Database, msg_id: &str) -> Option<(i64, String, i64)> {
        let conn = db.conn.lock().unwrap();
        conn.query_row(
            "SELECT output_tokens, total_cost_usd, latency_ms FROM proxy_request_logs
             WHERE request_id = ?1",
            [format!("session:{msg_id}")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .unwrap()
    }

    fn latency_of(db: &Database, msg_id: &str) -> i64 {
        let conn = db.conn.lock().unwrap();
        conn.query_row(
            "SELECT latency_ms FROM proxy_request_logs WHERE request_id = ?1",
            [format!("session:{msg_id}")],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn temp_session_file() -> (PathBuf, PathBuf) {
        let tmp = std::env::temp_dir().join(format!("cc-switch-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let file = tmp.join("session.jsonl");
        (tmp, file)
    }

    #[test]
    fn test_latency_spans_from_tool_result_to_last_block() -> Result<(), AppError> {
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        let lines = [
            // The start: the tool result
            chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
            chain_line("attachment", 2, Some(1), "2026-06-07T13:00:00.003Z"),
            // An attachment that was only recorded after the reply started has a timestamp that is even later than the first block and cannot be the start
            chain_line("attachment", 3, Some(2), "2026-06-07T13:00:05.001Z"),
            assistant_block("msg_t", 4, 3, 0, "2026-06-07T13:00:05.000Z"),
            assistant_block("msg_t", 5, 4, 1, "2026-06-07T13:00:07.000Z"),
            // Parallel tool calls: the result of the previous tool is inserted between two blocks of the same reply
            chain_line("user", 6, Some(5), "2026-06-07T13:00:08.000Z"),
            assistant_block("msg_t", 7, 6, 2, "2026-06-07T13:00:12.500Z"),
        ];
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        let file_sync = sync_single_file(&db, &file, None)?;
        assert_eq!(file_sync.imported, 1);
        assert_eq!(latency_of(&db, "msg_t"), 12_500);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_first_token_is_derived_from_a_leading_thinking_block() -> Result<(), AppError> {
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        let lines = [
            // The thinking finished after 3 seconds and 5 seconds: the first token is at 2 seconds
            chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
            thinking_block("msg_think", 2, 1, "2026-06-07T13:00:05.000Z", 3_000, true),
            assistant_block("msg_think", 3, 2, 1, "2026-06-07T13:00:09.000Z"),
            // The first block is a tool call: the timestamp of the block is only the moment it was written, so the first token cannot be derived
            chain_line("user", 4, Some(3), "2026-06-07T13:00:10.000Z"),
            assistant_block("msg_tool", 5, 4, 0, "2026-06-07T13:00:14.000Z"),
            // The thinking duration is longer than the whole span, the start does not line up, drop it
            chain_line("user", 6, Some(5), "2026-06-07T13:00:20.000Z"),
            thinking_block("msg_bad", 7, 6, "2026-06-07T13:00:22.000Z", 5_000, true),
        ];
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        sync_single_file(&db, &file, None)?;
        assert_eq!(latency_of(&db, "msg_think"), 9_000);
        assert_eq!(first_token_of(&db, "msg_think"), Some(2_000));
        assert_eq!(latency_of(&db, "msg_tool"), 4_000);
        assert_eq!(first_token_of(&db, "msg_tool"), None);
        assert_eq!(latency_of(&db, "msg_bad"), 2_000);
        assert_eq!(first_token_of(&db, "msg_bad"), None);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_first_token_is_filled_in_when_a_partial_reply_completes() -> Result<(), AppError> {
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        // The sub agent writes block by block: when the sync lands after the first block there is no stop_reason yet, do not estimate the duration
        fs::write(
            &file,
            [
                chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
                thinking_block("msg_sub", 2, 1, "2026-06-07T13:00:04.000Z", 2_500, false),
            ]
            .join("\n")
                + "\n",
        )
        .unwrap();
        sync_with_cursor(&db, &file)?;
        assert_eq!(first_token_of(&db, "msg_sub"), None);

        // The last block arrived: the duration and the first token are filled in together (the first token relies on the first block seen in the look-back before the cursor)
        append_lines(
            &file,
            &[assistant_block_with(
                "msg_sub",
                3,
                2,
                1,
                "2026-06-07T13:00:08.000Z",
                900,
                true,
            )],
        );
        sync_with_cursor(&db, &file)?;
        assert_eq!(latency_of(&db, "msg_sub"), 8_000);
        assert_eq!(first_token_of(&db, "msg_sub"), Some(1_500));

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_latency_start_is_found_before_the_cursor() -> Result<(), AppError> {
        // The tool result landed on disk first and was read away by the previous round of the sync;
        // the reply is only written as a whole when it ends,
        // its starting line is before the cursor
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        let before = [
            chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
            chain_line("attachment", 2, Some(1), "2026-06-07T13:00:00.003Z"),
        ];
        fs::write(&file, before.join("\n") + "\n").unwrap();
        let first = sync_with_cursor(&db, &file)?;
        assert_eq!(first.imported, 0);

        let mut content = fs::read(&file).unwrap();
        content.extend_from_slice(
            format!(
                "{}\n",
                assistant_block("msg_t", 3, 2, 0, "2026-06-07T13:00:09.000Z")
            )
            .as_bytes(),
        );
        fs::write(&file, &content).unwrap();
        bump_mtime(&file);
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!(second.imported, 1);
        assert_eq!(latency_of(&db, "msg_t"), 9_000);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_latency_is_not_estimated_without_a_reliable_start() -> Result<(), AppError> {
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        let lines = [
            chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
            // The first block is not visible: the one that appears first is the second block and its parent line is the tool result that was inserted
            assistant_block("msg_no_first", 2, 1, 2, "2026-06-07T13:00:09.000Z"),
            // The parent line is not in the file
            assistant_block("msg_orphan", 3, 9, 0, "2026-06-07T13:00:20.000Z"),
            // The start and the end are too close to each other, that does not look like a real request
            chain_line("user", 4, None, "2026-06-07T13:01:00.000Z"),
            assistant_block("msg_instant", 5, 4, 0, "2026-06-07T13:01:00.020Z"),
        ];
        fs::write(&file, lines.join("\n") + "\n").unwrap();

        let file_sync = sync_single_file(&db, &file, None)?;
        assert_eq!(file_sync.imported, 3);
        assert_eq!(latency_of(&db, "msg_no_first"), 0);
        assert_eq!(latency_of(&db, "msg_orphan"), 0);
        assert_eq!(latency_of(&db, "msg_instant"), 0);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_partial_subagent_reply_is_completed_by_its_final_block() -> Result<(), AppError> {
        let first_block = [
            chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
            // Written block by block: when the first block landed the output tokens were only the first few
            assistant_block_with("msg_s", 2, 1, 0, "2026-06-07T13:00:02.000Z", 5, false),
        ];
        let final_block = [assistant_block_with(
            "msg_s",
            3,
            2,
            1,
            "2026-06-07T13:00:30.000Z",
            4_660,
            true,
        )];

        // The sync happened to land between two blocks
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        append_lines(&file, &first_block);
        assert_eq!(sync_with_cursor(&db, &file)?.imported, 1);
        assert_eq!(
            stored_row(&db, "msg_s").map(|row| (row.0, row.2)),
            Some((5, 0))
        );
        append_lines(&file, &final_block);
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!((second.imported, second.skipped), (1, 0));

        // For comparison: the whole reply was read in one round of the sync
        let whole_db = Database::memory()?;
        let (whole_tmp, whole_file) = temp_session_file();
        append_lines(&whole_file, &first_block);
        append_lines(&whole_file, &final_block);
        sync_with_cursor(&whole_db, &whole_file)?;

        let completed = stored_row(&db, "msg_s");
        assert_eq!(completed, stored_row(&whole_db, "msg_s"));
        assert_eq!(completed.map(|row| (row.0, row.2)), Some((4_660, 30_000)));

        // Another round of the sync does not complete it twice
        bump_mtime(&file);
        let third = sync_with_cursor(&db, &file)?;
        assert_eq!((third.imported, third.skipped), (0, 0));

        fs::remove_dir_all(&tmp).ok();
        fs::remove_dir_all(&whole_tmp).ok();
        Ok(())
    }

    #[test]
    fn test_latency_is_extended_when_later_blocks_arrive() -> Result<(), AppError> {
        // The blocks of the main session all carry the final usage; when the sync read in the middle of
        // a write it first computed a shorter
        // duration from the first blocks, and the later blocks have to fill it up
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        append_lines(
            &file,
            &[
                chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
                assistant_block("msg_t", 2, 1, 0, "2026-06-07T13:00:05.000Z"),
            ],
        );
        sync_with_cursor(&db, &file)?;
        assert_eq!(latency_of(&db, "msg_t"), 5_000);

        append_lines(
            &file,
            &[
                chain_line("user", 3, Some(2), "2026-06-07T13:00:06.000Z"),
                assistant_block("msg_t", 4, 3, 1, "2026-06-07T13:00:12.500Z"),
            ],
        );
        let second = sync_with_cursor(&db, &file)?;
        assert_eq!(second.imported, 1);
        assert_eq!(latency_of(&db, "msg_t"), 12_500);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }

    #[test]
    fn test_completed_reply_matching_a_proxy_log_is_removed() -> Result<(), AppError> {
        let db = Database::memory()?;
        let (tmp, file) = temp_session_file();
        append_lines(
            &file,
            &[
                chain_line("user", 1, None, "2026-06-07T13:00:00.000Z"),
                assistant_block_with("msg_p", 2, 1, 0, "2026-06-07T13:00:02.000Z", 5, false),
            ],
        );
        // The intermediate value did not match the row the router service recorded, it was stored first
        sync_with_cursor(&db, &file)?;
        assert!(stored_row(&db, "msg_p").is_some());

        // After the request ended the router service recorded the same request
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                    total_cost_usd, latency_ms, status_code, created_at, data_source
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "proxy-id",
                    "p1",
                    "claude",
                    "claude-opus-4-8",
                    "claude-opus-4-8",
                    10,
                    4_660,
                    0,
                    0,
                    "0.10",
                    30_000,
                    200,
                    parse_timestamp_millis("2026-06-07T13:00:30.000Z").unwrap() / 1000,
                    "proxy"
                ],
            )?;
        }
        append_lines(
            &file,
            &[assistant_block_with(
                "msg_p",
                3,
                2,
                1,
                "2026-06-07T13:00:30.000Z",
                4_660,
                true,
            )],
        );
        sync_with_cursor(&db, &file)?;

        assert_eq!(stored_row(&db, "msg_p"), None);
        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);
        drop(conn);

        fs::remove_dir_all(&tmp).ok();
        Ok(())
    }
}
