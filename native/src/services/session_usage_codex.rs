//! Usage tracking for the Codex session logs
//!
//! Extracts the exact token usage data from the JSONL session files below ~/.codex/sessions/,
//! replacing the former state_5.sqlite estimation scheme.
//!
//! ## Data flow
//! ```text
//! ~/.codex/sessions/YYYY/MM/DD/*.jsonl -> incremental parse -> delta computation -> cost calculation -> proxy_request_logs table
//! ```
//!
//! ## The event types that are parsed
//! - `session_meta` -> extract the unique thread_id (the session_id of a sub agent points at the parent thread)
//! - `turn_context` / `event_msg` (type=thread_settings_applied) -> extract the current model
//! - `event_msg` (type=token_count) -> extract the accumulated token usage and compute the delta
//! - `token_usage_record` -> record this usage when the following token_count has a last of 0

use crate::codex_config::get_codex_config_dir;
use crate::codex_rollout_file;
use crate::database::{lock_conn, Database};
use crate::error::AppError;
use crate::proxy::usage::calculator::{CostCalculator, ModelPricing};
use crate::proxy::usage::parser::TokenUsage;
use crate::services::session_usage::{
    estimated_first_token_ms, estimated_latency_ms, metadata_modified_nanos,
    parse_timestamp_millis, update_sync_state, update_sync_state_on_conn, EstimatedTiming,
    SessionSyncResult,
};
use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_TOTAL;
use crate::services::usage_stats::{
    find_model_pricing, has_suspected_codex_session_duplicate, should_skip_session_insert, DedupKey,
};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;
use std::fs;
use std::io::BufRead;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    FileIdInfo, GetFileInformationByHandleEx, FILE_ID_INFO,
};

const CODEX_THREAD_REQUEST_ID_PREFIX: &str = "codex_session:thread-v1";

/// The accumulated token usage (it tracks the total_token_usage field)
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CumulativeTokens {
    input: u64,
    cached_input: u64,
    cache_write_input: u64,
    output: u64,
}

/// The token delta of a single API call
#[derive(Debug)]
struct DeltaTokens {
    input: u32,
    cached_input: u32,
    cache_write_input: u32,
    output: u32,
}

impl DeltaTokens {
    fn is_zero(&self) -> bool {
        self.input == 0 && self.cached_input == 0 && self.output == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenCountersSignature {
    input: Option<u64>,
    cached_input: Option<u64>,
    output: Option<u64>,
    reasoning_output: Option<u64>,
    total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenUsageSignature {
    total: Option<TokenCountersSignature>,
    last: Option<TokenCountersSignature>,
}

#[derive(Debug, Clone)]
struct TimestampedTokenSignature {
    timestamp: DateTime<Utc>,
    signature: TokenUsageSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParentFileStamp {
    modified_nanos: i64,
    size: u64,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial: u64,
    #[cfg(windows)]
    file_id: [u8; 16],
}

impl ParentFileStamp {
    fn from_file(file: &fs::File) -> Option<Self> {
        let metadata = file.metadata().ok()?;
        #[cfg(windows)]
        let (volume_serial, file_id) = windows_file_identity(file)?;
        Some(Self {
            modified_nanos: metadata_modified_nanos(&metadata),
            size: metadata.len(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(windows)]
            volume_serial,
            #[cfg(windows)]
            file_id,
        })
    }
}

#[cfg(windows)]
fn windows_file_identity(file: &fs::File) -> Option<(u64, [u8; 16])> {
    let mut information = FILE_ID_INFO::default();
    // SAFETY: `file` owns a live handle for this call, and `information` is a
    // valid writable FILE_ID_INFO buffer of the size passed to Windows.
    let succeeded = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            std::ptr::addr_of_mut!(information).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    } != 0;
    succeeded.then_some((
        information.VolumeSerialNumber,
        information.FileId.Identifier,
    ))
}

#[derive(Debug)]
struct ParentTokenTimeline {
    events: Vec<TimestampedTokenSignature>,
    max_timestamp: Option<DateTime<Utc>>,
    has_token_without_timestamp: bool,
    high_water: Option<CumulativeTokens>,
}

impl ParentTokenTimeline {
    fn signatures_before(
        &self,
        parent_path: &Path,
        cutoff: DateTime<Utc>,
    ) -> Result<Vec<TokenUsageSignature>, String> {
        if self.has_token_without_timestamp {
            return Err(format!(
                "父 rollout {} 的 token_count 缺少有效 timestamp",
                parent_path.display()
            ));
        }
        if self
            .max_timestamp
            .is_none_or(|timestamp| timestamp < cutoff)
        {
            return Err(format!(
                "父 rollout {} 尚未写到 child fork 时刻",
                parent_path.display()
            ));
        }
        Ok(self.signatures_at(cutoff))
    }

    fn signatures_at(&self, cutoff: DateTime<Utc>) -> Vec<TokenUsageSignature> {
        self.events
            .iter()
            .filter(|event| event.timestamp <= cutoff)
            .map(|event| event.signature.clone())
            .collect()
    }
}

#[derive(Debug)]
struct CachedParentTimeline {
    stamp: ParentFileStamp,
    timeline: Arc<ParentTokenTimeline>,
}

#[derive(Debug)]
struct CachedReplayPrefix {
    modified: i64,
    size: u64,
    prefix: usize,
}

#[derive(Debug)]
struct ParsedTokenEvent {
    line_offset: i64,
    signature: TokenUsageSignature,
    delta: DeltaTokens,
    event_index: Option<u32>,
    /// A usage that does not occupy a `token_count` sequence number. It is used when a remote
    /// compaction only writes a `token_usage_record`,
    /// which avoids pushing the `event_index` of every later request further back.
    request_suffix: Option<String>,
    model: String,
    timestamp: Option<String>,
    /// The request duration (including the wait for the first token) and the time to the first token estimated from the event timestamps; None when it cannot be estimated.
    timing: Option<EstimatedTiming>,
}

/// Only the first few bytes of a line are looked at to determine its kind: the timestamp, the
/// type and the role are all in the head of the line,
/// the body behind it (a tool output can be hundreds of KB) does not have to be parsed.
const LINE_HEAD_BYTES: usize = 512;

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Take the value of a `"key":"value"` out of the head of a line. It is only used for short values without escapes such as the timestamp, the type name and the role.
fn head_str<'a>(head: &'a [u8], key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":\"");
    let start = find_bytes(head, needle.as_bytes())? + needle.len();
    let len = head[start..].iter().position(|b| *b == b'"')?;
    std::str::from_utf8(&head[start..start + len]).ok()
}

#[derive(Debug, PartialEq, Eq)]
enum TimingLine {
    /// The next request is only sent after it: the user message, `turn_context`
    Boundary,
    /// The tool result, which is likewise the start of the next request
    ToolOutput,
    /// One item of the model output (a thought, a reply, a tool call), written at the moment this item was finished
    ModelOutput,
    /// `token_usage_record`: the usage record that is written when the response ends (only the newer Codex has it)
    UsageRecord,
    /// The start of a round (`task_started`) or an interruption (`turn_aborted`): the request that
    /// never got its `token_count`
    /// is void from here on
    TurnReset,
    /// The `item_completed` of the newer Codex, and it is a thought or a reply of the model output:
    /// it carries the moment the generation of this item started
    /// (`started_at_ms`, the moment the upstream started to output this item), and the earliest item
    /// of a response is the first token
    OutputItemStarted(i64),
}

fn classify_timing_line(line: &str) -> Option<(i64, TimingLine)> {
    let bytes = line.as_bytes();
    let head = &bytes[..bytes.len().min(LINE_HEAD_BYTES)];
    let (envelope, payload) = match find_bytes(head, b"\"payload\":{") {
        Some(at) => head.split_at(at),
        None => (head, &[][..]),
    };
    let kind = match head_str(envelope, "type")? {
        "turn_context" => TimingLine::Boundary,
        "token_usage_record" => TimingLine::UsageRecord,
        "response_item" => match head_str(payload, "type")? {
            "message" => match head_str(payload, "role")? {
                "assistant" => TimingLine::ModelOutput,
                _ => TimingLine::Boundary,
            },
            "reasoning" => TimingLine::ModelOutput,
            item if item.ends_with("_output") => TimingLine::ToolOutput,
            item if item.ends_with("_call") => TimingLine::ModelOutput,
            _ => return None,
        },
        "event_msg" => match head_str(payload, "type")? {
            "task_started" | "turn_aborted" => TimingLine::TurnReset,
            "item_completed" => output_item_started_ms(line, payload)?,
            _ => return None,
        },
        _ => return None,
    };
    let timestamp_ms = head_str(envelope, "timestamp").and_then(parse_timestamp_millis)?;
    Some((timestamp_ms, kind))
}

/// The start moment of a model output item (a thought, a reply) in an `item_completed` line. The
/// tool executions, the user messages
/// and the like are not model output and are not looked at. `started_at_ms` is written after
/// `item` and can reach past the head of the line, so the whole line has to be parsed; such
/// lines are short (a measured median of about 400 bytes).
fn output_item_started_ms(line: &str, payload_head: &[u8]) -> Option<TimingLine> {
    let item_at = find_bytes(payload_head, b"\"item\":{")?;
    match head_str(&payload_head[item_at..], "type")? {
        "Reasoning" | "AgentMessage" => {}
        _ => return None,
    }
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    let started_at_ms = value.get("payload")?.get("started_at_ms")?.as_i64()?;
    Some(TimingLine::OutputItemStarted(started_at_ms))
}

/// Estimate the duration of every request from the order of the events. The Codex logs have no
/// request level timing and the event order differs between the versions:
///
/// - The end: the newer Codex writes a `token_usage_record` when the response ends, so it is
///   used directly. The old one only has a `token_count`, some versions write it when the
///   response ends and others only after the tools have run; the latter shows up as "a tool
///   result appears right after the last output item and the `token_count` is written right
///   after the tool result", in that case the moment of the last output item is used instead
///   (a bit earlier than the real end, so the speed is a bit too high).
/// - The start: when the first output item of this request is seen, the closest start line
///   before it (the `token_count` of the previous time, a tool result, a user message, a
///   `turn_context`). The start line closest before the end is not taken directly because a
///   tool result may already have been written while the response was not over yet.
///
/// The old layout of 2025 backfills the output items of the previous response after its
/// `token_count` (with the same timestamp as that `token_count`). Those output items do not
/// belong to the next request and are ignored directly; otherwise
/// the start of the next request would be pinned to the moment the previous response ended
/// and the duration would come out absurdly long when the user sends the next message much
/// later.
///
/// The time to the first token: the newer Codex records the moment the generation of every
/// model output item started (the `started_at_ms` of the `item_completed`), the earliest one
/// of this request minus the start is taken. When the response starts with a tool call and
/// has no thought and no reply item it cannot be derived.
///
/// When a request is interrupted or fails its `token_count` never arrives and the start it
/// left behind has to be cleared when the next round begins,
/// otherwise it would be inherited by the next request. It is only cleared on a
/// `task_started` / `turn_aborted` and not on a user
/// message: Codex inserts developer messages in the middle of a response and clearing there
/// would move the start later.
#[derive(Debug, Default)]
struct RequestTimer {
    last_boundary_ms: Option<i64>,
    last_token_count_ms: Option<i64>,
    request_start_ms: Option<i64>,
    last_model_output_ms: Option<i64>,
    /// The latest of the tool results that appear after the last output item; it is cleared when another output item appears.
    tool_output_after_model_output_ms: Option<i64>,
    usage_record: Option<(i64, CumulativeTokens)>,
    /// The moment the earliest output item of this request started to be generated, that is the first token.
    first_output_ms: Option<i64>,
}

/// When the timestamps of two lines differ by at most this many milliseconds they count as
/// written in the same batch (measured a difference of 0-1 milliseconds,
/// and a real request cannot be that fast).
const SAME_FLUSH_SLACK_MS: i64 = 100;

impl RequestTimer {
    fn observe_line(&mut self, line: &str) {
        let Some((timestamp_ms, kind)) = classify_timing_line(line) else {
            return;
        };
        match kind {
            TimingLine::Boundary => {
                self.last_boundary_ms = self.last_boundary_ms.max(Some(timestamp_ms));
            }
            TimingLine::ToolOutput => {
                self.last_boundary_ms = self.last_boundary_ms.max(Some(timestamp_ms));
                if self.last_model_output_ms.is_some() {
                    self.tool_output_after_model_output_ms = Some(timestamp_ms);
                }
            }
            TimingLine::ModelOutput => {
                let trails_token_count = self.request_start_ms.is_none()
                    && self
                        .last_token_count_ms
                        .is_some_and(|at| timestamp_ms - at <= SAME_FLUSH_SLACK_MS);
                if trails_token_count {
                    return;
                }
                if self.request_start_ms.is_none() {
                    self.request_start_ms = self.last_boundary_ms;
                }
                self.last_model_output_ms = Some(timestamp_ms);
                self.tool_output_after_model_output_ms = None;
            }
            TimingLine::UsageRecord => {
                // The head of the line is only enough to determine the kind, the usage needs the whole line parsed; such lines are short
                let usage = serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("payload")
                            .and_then(|payload| payload.get("usage"))
                            .and_then(parse_cumulative_tokens)
                    });
                self.usage_record = usage.map(|usage| (timestamp_ms, usage));
            }
            TimingLine::OutputItemStarted(started_at_ms) => {
                self.first_output_ms = Some(
                    self.first_output_ms
                        .map_or(started_at_ms, |at| at.min(started_at_ms)),
                );
            }
            TimingLine::TurnReset => {
                *self = RequestTimer {
                    last_boundary_ms: self.last_boundary_ms.max(Some(timestamp_ms)),
                    last_token_count_ms: self.last_token_count_ms,
                    ..RequestTimer::default()
                };
            }
        }
    }

    /// On a `token_count` with a usage: settle the duration and the first token of this request and
    /// record it as the start of the next request. `last` is the usage of this request
    /// itself and is used to confirm that the `token_usage_record`
    /// is about the same request.
    fn finish_request(
        &mut self,
        token_count_ms: Option<i64>,
        last: Option<&CumulativeTokens>,
    ) -> Option<EstimatedTiming> {
        let start_ms = self.request_start_ms.or(self.last_boundary_ms);
        let first_output_ms = self.first_output_ms;
        let record_end_ms = self
            .usage_record
            .take()
            .filter(|(_, usage)| {
                last.is_none_or(|last| {
                    usage.input == last.input
                        && usage.cached_input == last.cached_input
                        && usage.output == last.output
                })
            })
            .map(|(timestamp_ms, _)| timestamp_ms);
        let token_count_waited_for_tools =
            match (self.tool_output_after_model_output_ms, token_count_ms) {
                (Some(tool_output_ms), Some(token_count_ms)) => {
                    token_count_ms - tool_output_ms <= SAME_FLUSH_SLACK_MS
                }
                (Some(_), None) => true,
                (None, _) => false,
            };
        let end_ms = record_end_ms.or(if token_count_waited_for_tools {
            self.last_model_output_ms
        } else {
            token_count_ms
        });

        *self = RequestTimer {
            last_boundary_ms: self.last_boundary_ms.max(token_count_ms),
            last_token_count_ms: token_count_ms,
            ..RequestTimer::default()
        };
        let start_ms = start_ms?;
        let latency_ms = estimated_latency_ms(start_ms, end_ms?)?;
        Some(EstimatedTiming {
            latency_ms,
            first_token_ms: first_output_ms.and_then(|first_output_ms| {
                estimated_first_token_ms(start_ms, first_output_ms, latency_ms)
            }),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ParentResolution {
    None,
    Parent(String),
    Continuation {
        ancestor_parent_id: Option<String>,
        history_base_id: Option<String>,
    },
    Deferred(String),
}

#[derive(Debug)]
struct ParsedCodexFile {
    root_thread_id: Option<String>,
    /// The thread ID of the root `session_meta` (the stable logical thread): a single segment file
    /// name matches the UUID in the file name and a two segment file name of a revert/resume
    /// corresponds to the leading UUID.
    meta_thread_id: Option<String>,
    root_meta_seen: bool,
    root_timestamp: Option<DateTime<Utc>>,
    parent: ParentResolution,
    token_events: Vec<ParsedTokenEvent>,
    /// The usage that is recorded when a `token_count` with an all-zero last follows right after a `token_usage_record`.
    usage_record_events: Vec<ParsedTokenEvent>,
    line_offset: i64,
    /// Bytes actually read, including an incomplete final record. Persisted in
    /// `last_byte_offset` only to detect file changes, never used as a seek
    /// position: parsing restarts at the beginning and `line_offset` tracks
    /// consumed records so an incomplete tail can be retried after an append.
    observed_bytes: i64,
    has_billable_tokens: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PendingReason {
    MissingParent(String),
    Stable(String),
    Retryable {
        reason: String,
        parent_id: String,
        child_file: Option<ParentFileStamp>,
        // Sample before resolving the parent: a write during resolution must
        // invalidate this entry on the next pass, not be mistaken for the
        // already-observed state. None keeps metadata/open errors retryable.
        parent_files: Option<Vec<(PathBuf, ParentFileStamp)>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingEntry {
    modified: i64,
    size: u64,
    reason: PendingReason,
}

#[derive(Debug, Default)]
struct CodexReplayCaches {
    parent_timelines: HashMap<PathBuf, CachedParentTimeline>,
    replay_prefixes: HashMap<PathBuf, CachedReplayPrefix>,
    pending: HashMap<PathBuf, PendingEntry>,
}

static CODEX_REPLAY_CACHES: OnceLock<Mutex<CodexReplayCaches>> = OnceLock::new();

fn replay_caches() -> &'static Mutex<CodexReplayCaches> {
    CODEX_REPLAY_CACHES.get_or_init(|| Mutex::new(CodexReplayCaches::default()))
}

pub(crate) fn clear_codex_replay_caches() {
    // Clearing it also discards the content that may have been written half way when the lock was held and a panic happened, so it also unpoisons the lock.
    let mut caches = replay_caches()
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *caches = CodexReplayCaches::default();
    replay_caches().clear_poison();
}

fn is_rollout_filename(file_name: &str) -> bool {
    let file_name = codex_rollout_file::logical_file_name(file_name);
    if !file_name.starts_with("rollout-") || !file_name.ends_with(".jsonl") {
        return false;
    }
    let stem = file_name.trim_end_matches(".jsonl");
    stem.get(stem.len().saturating_sub(36)..)
        .is_some_and(|candidate| uuid::Uuid::parse_str(candidate).is_ok())
}

fn is_codex_cursor_path(file_path: &str, codex_dir: &Path) -> bool {
    let path = Path::new(file_path);
    let file_name = file_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if !is_rollout_filename(file_name) {
        return false;
    }

    if path.starts_with(codex_dir.join("sessions"))
        || path.starts_with(codex_dir.join("archived_sessions"))
    {
        return true;
    }

    // Tolerate the cursors that were left over after the user changed CODEX_HOME and whose source
    // file no longer exists. Only a clear directory segment plus a Codex rollout UUID file name
    // is accepted, so that a broad codex_dir does not delete the cursors of other importers.
    file_path
        .replace('\\', "/")
        .split('/')
        .any(|segment| matches!(segment, "sessions" | "archived_sessions"))
}

fn sqlite_table_exists(conn: &rusqlite::Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Database(format!("查询表 {table} 失败: {error}")))
}

fn sqlite_column_exists(
    conn: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = ?2)",
        rusqlite::params![table, column],
        |row| row.get(0),
    )
    .map_err(|error| AppError::Database(format!("查询列 {table}.{column} 失败: {error}")))
}

pub(crate) fn reset_codex_usage_on_conn(
    conn: &rusqlite::Connection,
    codex_dir: &Path,
) -> Result<(), AppError> {
    if sqlite_table_exists(conn, "proxy_request_logs")?
        && sqlite_column_exists(conn, "proxy_request_logs", "data_source")?
    {
        conn.execute(
            "DELETE FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
        )
        .map_err(|error| AppError::Database(format!("清理 Codex 会话明细失败: {error}")))?;
    }
    if sqlite_table_exists(conn, "usage_daily_rollups")?
        && sqlite_column_exists(conn, "usage_daily_rollups", "provider_id")?
    {
        conn.execute(
            "DELETE FROM usage_daily_rollups WHERE provider_id = '_codex_session'",
            [],
        )
        .map_err(|error| AppError::Database(format!("清理 Codex 用量汇总失败: {error}")))?;
    }
    if sqlite_table_exists(conn, "session_log_sync")?
        && sqlite_column_exists(conn, "session_log_sync", "file_path")?
    {
        let paths = {
            let mut statement = conn
                .prepare("SELECT file_path FROM session_log_sync")
                .map_err(|error| {
                    AppError::Database(format!("读取会话同步 cursor 失败: {error}"))
                })?;
            let paths = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| AppError::Database(format!("查询会话同步 cursor 失败: {error}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| {
                    AppError::Database(format!("解析会话同步 cursor 失败: {error}"))
                })?;
            paths
        };
        for file_path in paths
            .into_iter()
            .filter(|path| is_codex_cursor_path(path, codex_dir))
        {
            conn.execute(
                "DELETE FROM session_log_sync WHERE file_path = ?1",
                [file_path],
            )
            .map_err(|error| AppError::Database(format!("清理 Codex 同步 cursor 失败: {error}")))?;
        }
    }
    Ok(())
}

impl Database {
    pub(crate) fn reset_codex_usage(&self) -> Result<(), AppError> {
        let codex_dir = get_codex_config_dir();
        let conn = lock_conn!(self.conn);
        conn.execute("SAVEPOINT reset_codex_usage", [])
            .map_err(|error| AppError::Database(format!("开启 Codex 重建事务失败: {error}")))?;
        let result = reset_codex_usage_on_conn(&conn, &codex_dir);
        match result {
            Ok(()) => {
                conn.execute("RELEASE reset_codex_usage", [])
                    .map_err(|error| {
                        AppError::Database(format!("提交 Codex 重建事务失败: {error}"))
                    })?;
                drop(conn);
                clear_codex_replay_caches();
                Ok(())
            }
            Err(error) => {
                conn.execute("ROLLBACK TO reset_codex_usage", []).ok();
                conn.execute("RELEASE reset_codex_usage", []).ok();
                Err(error)
            }
        }
    }
}

fn non_empty_string(value: Option<&serde_json::Value>) -> Option<String> {
    value
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn thread_id_from_filename(path: &Path) -> Option<String> {
    let logical = codex_rollout_file::logical_path(path);
    let stem = logical.file_stem()?.to_str()?;
    let candidate = stem.get(stem.len().checked_sub(36)?..)?;
    uuid::Uuid::parse_str(candidate)
        .ok()
        .map(|value| value.hyphenated().to_string())
}

/// The UUID of the thread itself in a two segment file name (`rollout-…-<threadId>_<rolloutId>.jsonl`);
/// a single segment file name returns `None`.
///
/// `thread/revert` produces such a file name when it creates a replacement rollout for the
/// same thread (see openai/codex#38127): the last segment is the newly generated rollout ID
/// and a later resume keeps appending to that file. The `id` of the root meta is always the
/// original thread ID, so the consistency check has to accept
/// both UUIDs.
fn leading_thread_id_from_filename(path: &Path) -> Option<String> {
    let logical = codex_rollout_file::logical_path(path);
    let stem = logical.file_stem()?.to_str()?;
    let len = stem.len();
    // The end of the layout: ...<uuidA>_<uuidB>. uuidB takes 36 characters and is preceded by a '_' (37 in total)
    if !stem.get(len.checked_sub(37)?..)?.starts_with('_') {
        return None;
    }
    let candidate = stem.get(len.checked_sub(73)?..len.checked_sub(37)?)?;
    uuid::Uuid::parse_str(candidate)
        .ok()
        .map(|value| value.hyphenated().to_string())
}

fn explicit_parent_from_meta(payload: &serde_json::Value) -> ParentResolution {
    let forked_from = non_empty_string(payload.get("forked_from_id"));
    let spawned_from = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"))
        .and_then(|spawn| non_empty_string(spawn.get("parent_thread_id")));

    match (forked_from, spawned_from) {
        (None, None) => ParentResolution::None,
        (Some(parent), None) | (None, Some(parent)) => ParentResolution::Parent(parent),
        (Some(forked), Some(spawned)) if forked == spawned => ParentResolution::Parent(forked),
        (Some(forked), Some(spawned)) => ParentResolution::Deferred(format!(
            "forked_from_id ({forked}) 与 thread_spawn.parent_thread_id ({spawned}) 不一致"
        )),
    }
}

fn parse_timestamp(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    value
        .and_then(serde_json::Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

fn parse_signature_counters(value: Option<&serde_json::Value>) -> Option<TokenCountersSignature> {
    let value = value?.as_object()?;
    Some(TokenCountersSignature {
        input: value
            .get("input_tokens")
            .and_then(serde_json::Value::as_u64),
        cached_input: value
            .get("cached_input_tokens")
            .or_else(|| value.get("cache_read_input_tokens"))
            .and_then(serde_json::Value::as_u64),
        output: value
            .get("output_tokens")
            .and_then(serde_json::Value::as_u64),
        reasoning_output: value
            .get("reasoning_output_tokens")
            .and_then(serde_json::Value::as_u64),
        total: value
            .get("total_tokens")
            .and_then(serde_json::Value::as_u64),
    })
}

fn parse_token_signature(info: &serde_json::Value) -> Option<TokenUsageSignature> {
    let total = parse_signature_counters(info.get("total_token_usage"));
    let last = parse_signature_counters(info.get("last_token_usage"));
    (total.is_some() || last.is_some()).then_some(TokenUsageSignature { total, last })
}

fn token_snapshot_source(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("rate_limits")
        .and_then(|rate_limits| rate_limits.get("limit_id"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The shared state of one sync pass.
///
/// - `cursors`: the `session_log_sync` snapshot that is preloaded once at the beginning of the
///   pass, replacing the per-file SELECT (especially the cursor inheritance has to look up
///   the other paths of the same rollout by file name, and doing that per file means a full
///   table scan of N times per pass). The snapshot semantics: the cursors that another file
///   of the same pass just wrote are invisible to the later cursor
///   inheritance -- the only effect is one more rescan that the request_id deduplication
///   covers, no data is lost and nothing is double counted.
/// - `pricing`: the pass level cache of the model pricing. When the pricing table is modified
///   during a pass that pass still uses the old price, the next sync pass picks it up.
struct CodexSyncPass {
    cursors: HashMap<String, (i64, i64)>,
    /// The logical rollout file name (without `.zst`) -> the path a cursor was recorded for, so that the cursor inheritance can look it up by name
    rollout_cursor_paths: HashMap<String, Vec<String>>,
    byte_offsets: HashMap<String, Option<i64>>,
    pricing: HashMap<String, Option<ModelPricing>>,
}

impl CodexSyncPass {
    fn load(db: &Database) -> Result<Self, AppError> {
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare("SELECT file_path, last_modified, last_line_offset FROM session_log_sync")
            .map_err(|e| AppError::Database(format!("预载同步游标失败: {e}")))?;
        let cursors = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
                ))
            })
            .and_then(|rows| rows.collect::<Result<HashMap<_, _>, _>>())
            .map_err(|e| AppError::Database(format!("预载同步游标失败: {e}")))?;
        let mut stmt = conn.prepare("SELECT file_path, last_byte_offset FROM session_log_sync")?;
        let byte_offsets = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        let mut rollout_cursor_paths: HashMap<String, Vec<String>> = HashMap::new();
        for path in cursors.keys() {
            let name = codex_rollout_file::logical_file_name(path);
            if is_rollout_filename(name) {
                rollout_cursor_paths
                    .entry(name.to_string())
                    .or_default()
                    .push(path.clone());
            }
        }
        Ok(Self {
            cursors,
            rollout_cursor_paths,
            byte_offsets,
            pricing: HashMap::new(),
        })
    }
}

/// Read the cursors; the cursor of the same rollout that is recorded under a different path is
/// inherited when it is further along.
///
/// The cases where the path changes: the Codex archive / unarchive moves the file and the
/// compression to `.jsonl.zst` or the decompression back to `.jsonl` while a conversation is
/// resumed changes the suffix. The line number only looks at the content and is therefore the
/// same for every shape.
/// 🔴 This is the only barrier that keeps a compressed file from being billed twice: the
/// request_id deduplication only covers the 30 days that the detail records are kept, while a
/// compressed file was not touched for at least 7 days and is usually older.
fn get_codex_sync_state(
    db: &Database,
    file_path: &Path,
    pass: &CodexSyncPass,
) -> Result<(i64, i64), AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();
    let state = pass.cursors.get(&file_path_str).copied().unwrap_or((0, 0));
    let logical_name = codex_rollout_file::logical_file_name(&file_path_str);
    let Some(paths) = pass.rollout_cursor_paths.get(logical_name) else {
        return Ok(state);
    };
    // Equivalent to the original SQL: ORDER BY last_line_offset DESC, last_modified DESC LIMIT 1
    //  -> take the maximum (offset, modified) on the snapshot.
    let inherited = paths
        .iter()
        .filter(|path| path.as_str() != file_path_str)
        .filter_map(|path| pass.cursors.get(path))
        .map(|&(modified, offset)| (offset, modified))
        .max();

    match inherited {
        Some((offset, modified)) if offset > state.1 => {
            update_sync_state(db, &file_path_str, modified, offset)?;
            Ok((modified, offset))
        }
        _ => Ok(state),
    }
}

/// Normalize a Codex model name
///
/// The rules (in order):
/// 1. Lowercase it: `GLM-4.6` -> `glm-4.6`
/// 2. Strip the provider prefix: `openai/gpt-5.4` -> `gpt-5.4`
/// 3. Strip the ISO date suffix: `gpt-5.4-2026-03-05` -> `gpt-5.4`
/// 4. Strip the compact date suffix: `gpt-5.4-20260305` -> `gpt-5.4`
fn normalize_codex_model(raw: &str) -> String {
    // Step 1: lowercase
    let mut name = raw.to_lowercase();

    // Step 2: strip the "provider/" prefix (e.g. openai/, azure/)
    if let Some(pos) = name.rfind('/') {
        name = name[pos + 1..].to_string();
    }

    // Step 3: strip the ISO date suffix -YYYY-MM-DD (exactly 11 characters)
    if name.len() > 11 && name.is_char_boundary(name.len() - 11) {
        let suffix = &name[name.len() - 11..];
        if suffix.is_ascii()
            && suffix.as_bytes()[0] == b'-'
            && suffix[1..5].chars().all(|c| c.is_ascii_digit())
            && suffix.as_bytes()[5] == b'-'
            && suffix[6..8].chars().all(|c| c.is_ascii_digit())
            && suffix.as_bytes()[8] == b'-'
            && suffix[9..11].chars().all(|c| c.is_ascii_digit())
        {
            name.truncate(name.len() - 11);
        }
    }

    // Step 4: strip the compact date suffix -YYYYMMDD (exactly 9 characters)
    if name.len() > 9 {
        let parts: Vec<&str> = name.rsplitn(2, '-').collect();
        if parts.len() == 2 {
            if let Some(suffix) = parts.first() {
                if suffix.len() == 8 && suffix.chars().all(|c| c.is_ascii_digit()) {
                    name = parts[1].to_string();
                }
            }
        }
    }

    name
}

/// Compute the delta between two accumulated values
fn compute_delta(prev: &Option<CumulativeTokens>, current: &CumulativeTokens) -> DeltaTokens {
    match prev {
        None => DeltaTokens {
            input: current.input as u32,
            cached_input: current.cached_input as u32,
            cache_write_input: current.cache_write_input as u32,
            output: current.output as u32,
        },
        Some(p) => DeltaTokens {
            input: current.input.saturating_sub(p.input) as u32,
            cached_input: current.cached_input.saturating_sub(p.cached_input) as u32,
            cache_write_input: current
                .cache_write_input
                .saturating_sub(p.cache_write_input) as u32,
            output: current.output.saturating_sub(p.output) as u32,
        },
    }
}

fn usage_matches(last: &CumulativeTokens, usage: &CumulativeTokens) -> bool {
    last.input == usage.input
        && last.cached_input == usage.cached_input
        && last.cache_write_input == usage.cache_write_input
        && last.output == usage.output
}

fn usage_is_zero(usage: &CumulativeTokens) -> bool {
    usage.input == 0 && usage.cached_input == 0 && usage.cache_write_input == 0 && usage.output == 0
}

fn delta_from_usage(usage: &CumulativeTokens) -> DeltaTokens {
    let input = usage.input.min(u32::MAX as u64) as u32;
    let cached_input = usage.cached_input.min(usage.input).min(u32::MAX as u64) as u32;
    let fresh_input = input.saturating_sub(cached_input);
    DeltaTokens {
        input,
        cached_input,
        cache_write_input: (usage.cache_write_input.min(u32::MAX as u64) as u32).min(fresh_input),
        output: usage.output.min(u32::MAX as u64) as u32,
    }
}

fn signature_for_last_usage(usage: &CumulativeTokens) -> TokenUsageSignature {
    TokenUsageSignature {
        total: None,
        last: Some(TokenCountersSignature {
            input: Some(usage.input),
            cached_input: Some(usage.cached_input),
            output: Some(usage.output),
            reasoning_output: None,
            total: Some(usage.input.saturating_add(usage.output)),
        }),
    }
}

struct PendingUsageRecord {
    key: String,
    usage: CumulativeTokens,
    line_offset: i64,
    timestamp: Option<String>,
    model: String,
}

fn push_unmatched_usage(
    events: &mut Vec<ParsedTokenEvent>,
    pending: PendingUsageRecord,
    has_billable_tokens: &mut bool,
) {
    let delta = delta_from_usage(&pending.usage);
    if delta.is_zero() {
        return;
    }
    *has_billable_tokens = true;
    events.push(ParsedTokenEvent {
        line_offset: pending.line_offset,
        signature: signature_for_last_usage(&pending.usage),
        delta,
        event_index: None,
        request_suffix: Some(format!("usage:{}", pending.key)),
        model: pending.model,
        timestamp: pending.timestamp,
        timing: None,
    });
}

fn update_high_water(high_water: &mut CumulativeTokens, current: &CumulativeTokens) {
    high_water.input = high_water.input.max(current.input);
    high_water.cached_input = high_water.cached_input.max(current.cached_input);
    high_water.cache_write_input = high_water.cache_write_input.max(current.cache_write_input);
    high_water.output = high_water.output.max(current.output);
}

/// Extract the accumulated token usage from a JSON Value
fn parse_cumulative_tokens(total_usage: &serde_json::Value) -> Option<CumulativeTokens> {
    let fields = total_usage.as_object()?;
    if ![
        "input_tokens",
        "cached_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
        "reasoning_output_tokens",
        "total_tokens",
    ]
    .iter()
    .any(|field| fields.contains_key(*field))
    {
        return None;
    }
    Some(CumulativeTokens {
        input: total_usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cached_input: total_usage
            .get("cached_input_tokens")
            .or_else(|| total_usage.get("cache_read_input_tokens"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        cache_write_input: total_usage
            .get("cache_write_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
        output: total_usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0),
    })
}

type RolloutIndex = HashMap<String, Vec<PathBuf>>;

#[derive(Debug, Default)]
struct CodexFileSyncResult {
    imported: u32,
    skipped: u32,
    suspected_duplicates: u32,
    deferred: bool,
}

/// Sync the Codex usage data (from the JSONL session logs)
pub fn sync_codex_usage(db: &Database) -> Result<SessionSyncResult, AppError> {
    let codex_dir = get_codex_config_dir();
    let files = collect_codex_session_files(&codex_dir);
    let rollout_index = build_rollout_index(&files);
    let mut pass = CodexSyncPass::load(db)?;

    let mut result = SessionSyncResult {
        imported: 0,
        skipped: 0,
        files_scanned: files.len() as u32,
        suspected_duplicates: 0,
        deferred_files: 0,
        errors: vec![],
    };

    for file_path in &files {
        match sync_single_codex_file(db, file_path, &rollout_index, &mut pass) {
            Ok(file_result) => {
                result.imported = result.imported.saturating_add(file_result.imported);
                result.skipped = result.skipped.saturating_add(file_result.skipped);
                result.suspected_duplicates = result
                    .suspected_duplicates
                    .saturating_add(file_result.suspected_duplicates);
                if file_result.deferred {
                    result.deferred_files = result.deferred_files.saturating_add(1);
                }
            }
            Err(e) => {
                let msg = format!("Codex 会话文件解析失败 {}: {e}", file_path.display());
                log::warn!("[CODEX-SYNC] {msg}");
                result.errors.push(msg);
            }
        }
    }

    if result.imported > 0 || result.deferred_files > 0 {
        log::info!(
            "[CODEX-SYNC] 同步完成: 导入 {} 条, 跳过 {} 条, deferred {} 个, 扫描 {} 个文件",
            result.imported,
            result.skipped,
            result.deferred_files,
            result.files_scanned
        );
    }

    // The cache entries outside the file set of this round are never reused (the file was deleted or
    // archived), they are evicted per round
    // so that the memory does not slowly grow during a long uptime. An entry that is evicted by
    // mistake while it is still referenced only costs one more rebuild.
    let live: std::collections::HashSet<&PathBuf> = files.iter().collect();
    let caches = replay_caches();
    let mut caches = caches.lock().unwrap_or_else(PoisonError::into_inner);
    caches
        .parent_timelines
        .retain(|path, _| live.contains(path));
    caches.replay_prefixes.retain(|path, _| live.contains(path));
    caches.pending.retain(|path, _| live.contains(path));
    drop(caches);

    Ok(result)
}

/// Collect all the Codex session JSONL files
fn collect_codex_session_files(codex_dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    // 1. Scan sessions/YYYY/MM/DD/*.jsonl (the date partitioned directory)
    let sessions_dir = codex_dir.join("sessions");
    if sessions_dir.is_dir() {
        collect_jsonl_recursive(&sessions_dir, &mut files, 0, 3);
    }

    // 2. Scan archived_sessions/*.jsonl (the flat archive directory)
    let archived_dir = codex_dir.join("archived_sessions");
    if archived_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&archived_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if codex_rollout_file::is_rollout_file(&path) {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    // While both shapes coexist in the moment of a compression / decompression only the plain shape is read, so that a thread does not end up with two candidates
    codex_rollout_file::dedupe_siblings(files)
}

fn build_rollout_index(files: &[PathBuf]) -> RolloutIndex {
    let mut index = RolloutIndex::new();
    for path in files {
        if let Some(thread_id) = thread_id_from_filename(path) {
            index.entry(thread_id).or_default().push(path.clone());
        }
        // The continuation pages of a paginated / resumed thread (`<threadId>_<rolloutId>`) are also
        // registered under the UUID of the thread itself: the forked_from_id of a sub agent points at
        // the thread itself and the parent timeline
        // must be able to see the segments after the first page.
        if let Some(thread_id) = leading_thread_id_from_filename(path) {
            index.entry(thread_id).or_default().push(path.clone());
        }
    }
    for paths in index.values_mut() {
        paths.sort();
        paths.dedup();
    }
    index
}

/// Scan the .jsonl files below a directory recursively (with a maximum depth)
fn collect_jsonl_recursive(dir: &Path, files: &mut Vec<PathBuf>, depth: u32, max_depth: u32) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && depth < max_depth {
            collect_jsonl_recursive(&path, files, depth + 1, max_depth);
        } else if codex_rollout_file::is_rollout_file(&path) {
            files.push(path);
        }
    }
}

/// Resolve the cut-off accumulated high water mark baseline of the previous segment for a
/// continuation page that continues a thread by paging.
///
/// When the `token_count` of a continuation page lacks a valid `last_token_usage`, the code
/// relies on `compute_delta(&total_high_water, total)` to compute the delta. If the
/// continuation page does not restore the high water mark of the previous segment,
/// the first record would bill the whole accumulated history of the segment as the delta.
/// Through the candidate file list of the current logical thread (sorted by time)
/// or `history_base_id` the immediately preceding segment is located and its final high water
/// mark is extracted; if the previous segment is missing or cannot be read,
/// an error has to be returned to trigger the defensive deferred import, which prevents a
/// double count.
fn resolve_continuation_baseline(
    file_path: &Path,
    meta_thread_id: &str,
    history_base_id: Option<&str>,
    rollout_index: Option<&RolloutIndex>,
) -> Result<Option<CumulativeTokens>, String> {
    let Some(rollout_index) = rollout_index else {
        return Err("缺少 rollout_index 无法解析续写页基线".to_string());
    };

    let current_file_name = file_path.file_name();

    // 1. Prefer the candidate files that are linked to the current logical thread and look for the previous page that is older than the current segment in the reverse natural order of the file name times
    if let Some(candidates) = rollout_index.get(meta_thread_id) {
        for prev_path in candidates.iter().rev() {
            if prev_path.file_name() < current_file_name {
                let timeline = load_parent_timeline(prev_path).map_err(|e| {
                    format!(
                        "无法读取续写页前序分段 {}: {}",
                        prev_path.display(),
                        e.reason
                    )
                })?;
                if let Some(hw) = timeline.high_water.clone() {
                    return Ok(Some(hw));
                }
            }
        }
    }

    // 2. If it is not found in the candidate list of the current thread and the history_base_id points
    // at a separate UUID,
    // try to find a file that is older than the current segment in that separate candidate list
    if let Some(hb_id) = history_base_id {
        if hb_id != meta_thread_id {
            if let Some(candidates) = rollout_index.get(hb_id) {
                for prev_path in candidates.iter().rev() {
                    if prev_path.file_name() < current_file_name {
                        let timeline = load_parent_timeline(prev_path).map_err(|e| {
                            format!(
                                "无法读取续写页前序分段 {}: {}",
                                prev_path.display(),
                                e.reason
                            )
                        })?;
                        if let Some(hw) = timeline.high_water.clone() {
                            return Ok(Some(hw));
                        }
                    }
                }
            }
        }
    }

    if history_base_id.is_some() {
        return Err(format!(
            "找不到续写页 history_base 对应的前序分段文件 (meta: {meta_thread_id})"
        ));
    }

    Ok(None)
}

#[cfg(test)]
fn parse_codex_file(
    file_path: &Path,
    root_thread_id: Option<String>,
) -> Result<ParsedCodexFile, AppError> {
    parse_codex_file_with_index(file_path, root_thread_id, None)
}

fn parse_codex_file_with_index(
    file_path: &Path,
    root_thread_id: Option<String>,
    rollout_index: Option<&RolloutIndex>,
) -> Result<ParsedCodexFile, AppError> {
    #[cfg(test)]
    tests::record_codex_parse(file_path);
    let file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    // The observed_bytes of a compressed file (`.jsonl.zst`) record the compressed size on disk:
    // it is only used to compare with the file size of the next round, recording the length after
    // the decompression would decompress it again in every round.
    let compressed_size = if codex_rollout_file::is_compressed(file_path) {
        Some(
            file.metadata()
                .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?
                .len(),
        )
    } else {
        None
    };
    let mut reader = codex_rollout_file::reader_from_file(file_path, file)
        .map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let mut root_meta_seen = false;
    let mut root_timestamp = None;
    let mut meta_thread_id = None;
    let mut parent = ParentResolution::None;
    let mut current_model = "unknown".to_string();
    // `total_token_usage` is session-cumulative, including across model and
    // rate-limit bucket changes. Divergent snapshots are handled by preferring
    // exact `last_token_usage`, not by splitting the cumulative baseline.
    let mut total_high_water = None;
    // Rate-limit refreshes can re-emit unchanged token info under another
    // `limit_id`. Same-source repeats are identified by that source's latest
    // full snapshot; cross-source repeats must match the immediately preceding
    // token event. Do not compare against other sources' older snapshots:
    // those stale signatures can legitimately recur after a counter reset.
    let mut last_signature_by_source: HashMap<Option<String>, TokenUsageSignature> = HashMap::new();
    let mut previous_token_signature = None;
    let mut is_continuation = false;
    let mut history_base_thread_id = None;
    let mut event_index = 0u32;
    let mut token_events = Vec::new();
    let mut usage_record_events = Vec::new();
    let mut pending_usage: Option<PendingUsageRecord> = None;
    let mut line_offset = 0i64;
    let mut observed_bytes = 0i64;
    let mut has_billable_tokens = false;
    let mut timer = RequestTimer::default();

    loop {
        let mut bytes = Vec::new();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|e| AppError::Config(format!("无法读取 Codex 日志: {e}")))?;
        // Count the incomplete suffix too, so an unchanged crashed/closed
        // rollout is skipped rather than fully reparsed on every sync pass.
        observed_bytes += read as i64;
        // A live writer may have only written part of the final JSON record.
        // Leave its line cursor unconsumed for the next file change, but retain
        // support for a complete final JSON record without a newline.
        if read == 0
            || (bytes.last() != Some(&b'\n')
                && serde_json::from_slice::<serde_json::Value>(&bytes).is_err())
        {
            break;
        }
        line_offset += 1;
        let line = match String::from_utf8(bytes) {
            Ok(line) => line,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }

        timer.observe_line(&line);

        let is_event_msg = line.contains("\"event_msg\"");
        let is_turn_context = line.contains("\"turn_context\"");
        let is_session_meta = line.contains("\"session_meta\"");
        let is_usage_record = line.contains("\"token_usage_record\"");
        if !is_event_msg && !is_turn_context && !is_session_meta && !is_usage_record {
            continue;
        }
        if is_event_msg
            && !line.contains("\"token_count\"")
            && !line.contains("\"thread_settings_applied\"")
        {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(event_type) = value.get("type").and_then(serde_json::Value::as_str) else {
            continue;
        };

        match event_type {
            "session_meta" if !root_meta_seen => {
                root_meta_seen = true;
                root_timestamp = parse_timestamp(value.get("timestamp"));
                let payload = value.get("payload").unwrap_or(&serde_json::Value::Null);
                parent = explicit_parent_from_meta(payload);

                meta_thread_id = non_empty_string(
                    payload
                        .get("id")
                        .or_else(|| payload.get("thread_id"))
                        .or_else(|| payload.get("threadId")),
                )
                .map(|id| {
                    uuid::Uuid::parse_str(&id)
                        .map(|value| value.hyphenated().to_string())
                        .unwrap_or(id)
                });
                let leading_id = leading_thread_id_from_filename(file_path);
                if let (Some(filename_id), Some(meta_id)) =
                    (&root_thread_id, meta_thread_id.as_ref())
                {
                    let matches =
                        filename_id == meta_id || leading_id.as_deref() == Some(meta_id.as_str());
                    if !matches {
                        parent = ParentResolution::Deferred(format!(
                            "文件名线程 ID ({filename_id}) 与 root meta ID ({meta_id}) 不一致"
                        ));
                    }
                }

                // The continuation pages that the paging of a branch thread produces (`<branchId>_<pageId>`)
                // and the continuation segments that carry a history_base pointing at this thread's
                // history_base still keep the forked_from_id / parent_thread_id of the branch session they
                // belong to in their session_meta; but the continuation page is appended in real time after
                // this thread and
                // records the new events after the paging, and the new format continuation page does not copy
                // the usage of the parent session. Treating it as a
                // new fork would use the session_meta timestamp of the paging (later than the end of the
                // original parent thread) and it would be permanently deferred because the parent thread
                // cannot reach the child cutoff.
                // Once it is recognized as a continuation of this thread it is marked as a Continuation and the
                // parent replay is not removed (there is no need to align the parent timeline).
                history_base_thread_id = non_empty_string(
                    payload
                        .get("history_base")
                        .and_then(|hb| hb.get("thread_id")),
                )
                .map(|id| {
                    uuid::Uuid::parse_str(&id)
                        .map(|value| value.hyphenated().to_string())
                        .unwrap_or(id)
                });
                let is_paginated = payload
                    .get("history_mode")
                    .and_then(serde_json::Value::as_str)
                    == Some("paginated");
                let has_history_base = history_base_thread_id.is_some();
                let has_parent = matches!(parent, ParentResolution::Parent(_));
                is_continuation = match (leading_id.as_deref(), meta_thread_id.as_deref()) {
                    (Some(leading), Some(meta)) if leading == meta => {
                        is_paginated || has_history_base || has_parent
                    }
                    _ => {
                        has_history_base
                            && history_base_thread_id.as_deref() == meta_thread_id.as_deref()
                    }
                };

                if is_continuation {
                    let ancestor_parent_id = match &parent {
                        ParentResolution::Parent(p) => Some(p.clone()),
                        _ => None,
                    };
                    parent = ParentResolution::Continuation {
                        ancestor_parent_id,
                        history_base_id: history_base_thread_id.clone(),
                    };
                }

                if let ParentResolution::Parent(parent_id) = &mut parent {
                    match uuid::Uuid::parse_str(parent_id) {
                        Ok(value) => *parent_id = value.hyphenated().to_string(),
                        Err(_) => {
                            parent = ParentResolution::Deferred(format!(
                                "显式 parent_thread_id 不是有效 UUID: {parent_id}"
                            ));
                        }
                    }
                }
                if matches!((&root_thread_id, &parent), (Some(root), ParentResolution::Parent(parent_id)) if root == parent_id)
                {
                    parent = ParentResolution::Deferred(
                        "parent_thread_id 与 root_thread_id 相同".to_string(),
                    );
                }
            }
            "turn_context" => {
                if let Some(payload) = value.get("payload") {
                    if let Some(model) = payload
                        .get("model")
                        .or_else(|| payload.get("info").and_then(|info| info.get("model")))
                        .and_then(serde_json::Value::as_str)
                    {
                        current_model = normalize_codex_model(model);
                    }
                }
            }
            "event_msg" => {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                if payload.get("type").and_then(serde_json::Value::as_str)
                    == Some("thread_settings_applied")
                {
                    // Resumed/Desktop threads may change models without another
                    // turn_context. Only update attribution: the cumulative token
                    // baseline and duplicate snapshots still belong to the session.
                    if let Some(model) = payload
                        .get("thread_settings")
                        .and_then(|settings| settings.get("model"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                    {
                        current_model = normalize_codex_model(model);
                    }
                    continue;
                }
                if payload.get("type").and_then(serde_json::Value::as_str) != Some("token_count") {
                    continue;
                }
                let Some(info) = payload.get("info").filter(|info| !info.is_null()) else {
                    continue;
                };
                let Some(signature) = parse_token_signature(info) else {
                    continue;
                };

                if let Some(model) = info
                    .get("model")
                    .or_else(|| info.get("model_name"))
                    .or_else(|| payload.get("model"))
                    .and_then(serde_json::Value::as_str)
                {
                    current_model = normalize_codex_model(model);
                }

                let snapshot_source = token_snapshot_source(payload);
                let total = info
                    .get("total_token_usage")
                    .and_then(parse_cumulative_tokens);
                let last = info
                    .get("last_token_usage")
                    .and_then(parse_cumulative_tokens);
                if let Some(pending) = pending_usage.take() {
                    // A matching last was already accounted by this token_count. A last of 0 is the empty snapshot
                    // after a remote compaction
                    // wrote this usage only in the token_usage_record. The other records that do not match are
                    // only used for the duration and must not record another row.
                    let covered = last
                        .as_ref()
                        .is_some_and(|last| usage_matches(last, &pending.usage));
                    if !covered && last.as_ref().is_some_and(usage_is_zero) {
                        push_unmatched_usage(
                            &mut usage_record_events,
                            pending,
                            &mut has_billable_tokens,
                        );
                    }
                }
                if total.is_none() && last.is_none() {
                    continue;
                }
                let has_total_snapshot = total.is_some();
                let duplicate_snapshot = has_total_snapshot
                    && (last_signature_by_source.get(&snapshot_source) == Some(&signature)
                        || previous_token_signature.as_ref() == Some(&signature));
                if has_total_snapshot {
                    last_signature_by_source.insert(snapshot_source, signature.clone());
                }
                previous_token_signature = Some(signature.clone());

                let request_usage = last.clone();
                let delta = if duplicate_snapshot {
                    DeltaTokens {
                        input: 0,
                        cached_input: 0,
                        cache_write_input: 0,
                        output: 0,
                    }
                } else if let Some(last) = last {
                    // Codex provides the exact per-request usage. Prefer it to
                    // subtracting cumulative snapshots, which may come from
                    // multiple independently advancing rate-limit lanes.
                    DeltaTokens {
                        input: last.input as u32,
                        cached_input: last.cached_input as u32,
                        cache_write_input: last.cache_write_input as u32,
                        output: last.output as u32,
                    }
                } else if let Some(total) = total.as_ref() {
                    if is_continuation && total_high_water.is_none() {
                        let meta_id = meta_thread_id.as_deref().unwrap_or_default();
                        match resolve_continuation_baseline(
                            file_path,
                            meta_id,
                            history_base_thread_id.as_deref(),
                            rollout_index,
                        ) {
                            Ok(Some(baseline)) => {
                                if total.input < baseline.input {
                                    // The counter of the continuation page was reset (it starts at 0 or a smaller value), the current snapshot is the baseline of the first round
                                    total_high_water = None;
                                } else {
                                    // Inherit the cut-off high water mark baseline of the previous segment
                                    total_high_water = Some(baseline);
                                }
                            }
                            Ok(None) => {
                                total_high_water = None;
                            }
                            Err(err) => {
                                // The defensive mechanism: when the previous segment cannot be read or the baseline cannot be restored reliably, the import has to be deferred
                                parent = ParentResolution::Deferred(format!(
                                    "续写页缺少有效的前序累计用量基线: {err}"
                                ));
                                has_billable_tokens = true;
                                break;
                            }
                        }
                    }
                    compute_delta(&total_high_water, total)
                } else {
                    continue;
                };
                if let Some(total) = total {
                    if let Some(high_water) = total_high_water.as_mut() {
                        update_high_water(high_water, &total);
                    } else {
                        total_high_water = Some(total);
                    }
                }
                // Cache reads and writes are disjoint parts of input_tokens.
                let cached_input = delta.cached_input.min(delta.input);
                let delta = DeltaTokens {
                    cached_input,
                    cache_write_input: delta.cache_write_input.min(delta.input - cached_input),
                    ..delta
                };
                let nonzero_index = if delta.is_zero() {
                    None
                } else {
                    has_billable_tokens = true;
                    event_index = event_index.saturating_add(1);
                    Some(event_index)
                };

                let timestamp = value
                    .get("timestamp")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                // A repeated snapshot (it is resent when the quota is refreshed) is not a request and does not take part in the timing
                let timing = nonzero_index.and_then(|_| {
                    timer.finish_request(
                        timestamp.as_deref().and_then(parse_timestamp_millis),
                        request_usage.as_ref(),
                    )
                });

                token_events.push(ParsedTokenEvent {
                    line_offset,
                    signature,
                    delta,
                    event_index: nonzero_index,
                    request_suffix: None,
                    model: current_model.clone(),
                    timestamp,
                    timing,
                });
            }
            "token_usage_record" => {
                pending_usage = None;
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                let Some(usage) = payload.get("usage").and_then(parse_cumulative_tokens) else {
                    continue;
                };
                if usage_is_zero(&usage) {
                    continue;
                }
                let response_id = payload
                    .get("response_id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("");
                let key = if response_id.is_empty() {
                    format!("line{line_offset}")
                } else {
                    response_id.to_string()
                };
                pending_usage = Some(PendingUsageRecord {
                    key,
                    usage,
                    line_offset,
                    timestamp: value
                        .get("timestamp")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                    model: current_model.clone(),
                });
            }
            _ => {}
        }
    }

    Ok(ParsedCodexFile {
        root_thread_id,
        meta_thread_id,
        root_meta_seen,
        root_timestamp,
        parent,
        token_events,
        usage_record_events,
        line_offset,
        observed_bytes: compressed_size
            .and_then(|size| i64::try_from(size).ok())
            .unwrap_or(observed_bytes),
        has_billable_tokens,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct ParentSignatureError {
    reason: String,
    cacheable: bool,
}

impl ParentSignatureError {
    fn content(reason: String) -> Self {
        Self {
            reason,
            cacheable: true,
        }
    }

    fn transient(reason: String) -> Self {
        Self {
            reason,
            cacheable: false,
        }
    }
}

#[cfg(test)]
fn parent_signatures_before(
    parent_path: &Path,
    cutoff: DateTime<Utc>,
) -> Result<Vec<TokenUsageSignature>, ParentSignatureError> {
    load_parent_timeline(parent_path)?
        .signatures_before(parent_path, cutoff)
        .map_err(ParentSignatureError::content)
}

/// Read (or hit the cache of) the complete token timeline of a single file of the parent rollout.
fn load_parent_timeline(
    parent_path: &Path,
) -> Result<Arc<ParentTokenTimeline>, ParentSignatureError> {
    let file = fs::File::open(parent_path).map_err(|error| {
        ParentSignatureError::transient(format!(
            "无法打开父 rollout {}: {error}",
            parent_path.display()
        ))
    })?;
    let stamp = ParentFileStamp::from_file(&file);
    let cached_timeline = stamp.and_then(|stamp| {
        replay_caches().lock().ok().and_then(|caches| {
            caches
                .parent_timelines
                .get(parent_path)
                .filter(|entry| entry.stamp == stamp)
                .map(|entry| Arc::clone(&entry.timeline))
        })
    });
    if let Some(timeline) = cached_timeline {
        return Ok(timeline);
    }

    let mut events = Vec::new();
    let mut max_timestamp: Option<DateTime<Utc>> = None;
    let mut has_token_without_timestamp = false;
    let mut high_water: Option<CumulativeTokens> = None;

    // The complete parent file has to be scanned, it must not break at the first future timestamp:
    // the write order of a rollout does not promise strictly monotonic timestamps. Once the
    // complete timeline is cached, the different child cutoffs only need an in-memory filter.
    let mut reader = codex_rollout_file::reader_from_file(parent_path, file).map_err(|error| {
        ParentSignatureError::transient(format!(
            "无法打开父 rollout {}: {error}",
            parent_path.display()
        ))
    })?;
    let mut bytes = Vec::new();
    loop {
        bytes.clear();
        // I/O/decompression failures are not evidence of stable content. Do
        // not cache a partial timeline or suppress its next resolution attempt.
        let read = reader.read_until(b'\n', &mut bytes).map_err(|error| {
            ParentSignatureError::transient(format!(
                "无法读取父 rollout {}: {error}",
                parent_path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        // Decode separately from reading: malformed UTF-8 is a skipped record,
        // just like malformed JSON, rather than a retryable reader failure.
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let timestamp = parse_timestamp(value.get("timestamp"));
        if let Some(timestamp) = timestamp {
            max_timestamp = Some(max_timestamp.map_or(timestamp, |current| current.max(timestamp)));
        }
        if value.get("type").and_then(serde_json::Value::as_str) != Some("event_msg")
            || value
                .get("payload")
                .and_then(|payload| payload.get("type"))
                .and_then(serde_json::Value::as_str)
                != Some("token_count")
        {
            continue;
        }
        let Some(info) = value
            .get("payload")
            .and_then(|payload| payload.get("info"))
            .filter(|info| !info.is_null())
        else {
            continue;
        };
        let Some(signature) = parse_token_signature(info) else {
            continue;
        };
        let Some(timestamp) = timestamp else {
            has_token_without_timestamp = true;
            continue;
        };
        events.push(TimestampedTokenSignature {
            timestamp,
            signature,
        });

        if let Some(total) = info
            .get("total_token_usage")
            .and_then(parse_cumulative_tokens)
        {
            if let Some(hw) = high_water.as_mut() {
                update_high_water(hw, &total);
            } else {
                high_water = Some(total);
            }
        }
    }

    let timeline = Arc::new(ParentTokenTimeline {
        events,
        max_timestamp,
        has_token_without_timestamp,
        high_water,
    });
    if let (Some(stamp), Ok(mut caches)) = (stamp, replay_caches().lock()) {
        caches.parent_timelines.insert(
            parent_path.to_path_buf(),
            CachedParentTimeline {
                stamp,
                timeline: Arc::clone(&timeline),
            },
        );
    }
    Ok(timeline)
}

/// One segment of the parent thread (the first page or a continuation page), there may be several copies with the same content.
struct ParentSegment {
    rollout_id: Option<String>,
    timeline: Arc<ParentTokenTimeline>,
    /// The earliest end time of the copies.
    max_timestamp: Option<DateTime<Utc>>,
}

fn resolve_parent_signatures(
    parent_id: &str,
    cutoff: DateTime<Utc>,
    rollout_index: &RolloutIndex,
) -> Result<Vec<TokenUsageSignature>, ParentSignatureError> {
    let Some(candidates) = rollout_index.get(parent_id) else {
        return Err(ParentSignatureError::transient(format!(
            "找不到父 rollout: {parent_id}"
        )));
    };

    // The same thread UUID can correspond to several files: the copies of the same rollout (for
    // example one in sessions/ and one in
    // archived_sessions/) must have the same content; the continuation pages of a paginated /
    // resumed thread
    // (`<parent_id>_<rolloutId>`) on the other hand are later segments of that thread -- after the
    // paging the first page does not grow any more, so they have to be joined into one timeline
    // in the order of the time prefix of the file names before it can be decided whether the child
    // fork moment was already written.
    let mut ordered = candidates.iter().collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.file_name().cmp(&right.file_name()));

    let mut merged = ParentTokenTimeline {
        events: Vec::new(),
        max_timestamp: None,
        has_token_without_timestamp: false,
        high_water: None,
    };
    let mut segments: Vec<ParentSegment> = Vec::new();
    let mut latest_path: Option<&Path> = None;
    for candidate in ordered {
        let timeline = load_parent_timeline(candidate)?;
        // No matter whether they are duplicate copies, as soon as any candidate has the flag of a missing timestamp error it has to be merged in to keep the defensive check
        merged.has_token_without_timestamp |= timeline.has_token_without_timestamp;
        let rollout_id = thread_id_from_filename(candidate);
        if let Some(segment) = segments
            .iter_mut()
            .find(|segment| segment.rollout_id == rollout_id)
        {
            if segment.timeline.signatures_at(cutoff) != timeline.signatures_at(cutoff) {
                return Err(ParentSignatureError::content(format!(
                    "父 rollout UUID {parent_id} 对应多个内容不一致的文件"
                )));
            }
            // Every copy of the same rollout has to be written up to the fork moment: that segment takes
            // the earliest end time of the copies, so the result does not depend on the order of the copies.
            segment.max_timestamp = match (segment.max_timestamp, timeline.max_timestamp) {
                (Some(current), Some(other)) => Some(current.min(other)),
                _ => None,
            };
            continue;
        }
        merged.events.extend(timeline.events.iter().cloned());
        segments.push(ParentSegment {
            rollout_id,
            max_timestamp: timeline.max_timestamp,
            timeline,
        });
        latest_path = Some(candidate.as_path());
    }
    merged.max_timestamp = segments
        .iter()
        .filter_map(|segment| segment.max_timestamp)
        .max();
    let Some(latest_path) = latest_path else {
        return Err(ParentSignatureError::transient(format!(
            "找不到父 rollout: {parent_id}"
        )));
    };
    merged
        .signatures_before(latest_path, cutoff)
        .map_err(ParentSignatureError::content)
}

/// Metadata-only dependency snapshot; opening a file checks accessibility and
/// gives the same identity stamp used by the parent timeline cache. Include all
/// candidates so archive moves, replacements and duplicate candidates retry.
/// This must be called without holding the replay-cache mutex.
fn parent_dependency_stamp(
    parent_id: &str,
    rollout_index: &RolloutIndex,
) -> Option<Vec<(PathBuf, ParentFileStamp)>> {
    let candidates = rollout_index.get(parent_id)?;
    let mut files = Vec::with_capacity(candidates.len());
    for path in candidates {
        let file = fs::File::open(path).ok()?;
        files.push((path.clone(), ParentFileStamp::from_file(&file)?));
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Some(files)
}

fn matching_replay_prefix(child: &[ParsedTokenEvent], parent: &[TokenUsageSignature]) -> usize {
    let mut parent_offset = 0usize;
    let mut matched = 0usize;
    for event in child {
        let Some(relative_match) = parent[parent_offset..]
            .iter()
            .position(|signature| signature == &event.signature)
        else {
            break;
        };
        parent_offset += relative_match + 1;
        matched += 1;
    }
    matched
}

fn mark_deferred(
    file_path: &Path,
    modified: i64,
    size: u64,
    reason: PendingReason,
) -> CodexFileSyncResult {
    let entry = PendingEntry {
        modified,
        size,
        reason,
    };
    let should_warn = replay_caches()
        .lock()
        .ok()
        .and_then(|mut caches| {
            caches
                .pending
                .insert(file_path.to_path_buf(), entry.clone())
        })
        .as_ref()
        != Some(&entry);
    if should_warn {
        let reason = match &entry.reason {
            PendingReason::MissingParent(parent) => format!("找不到父 rollout {parent}"),
            PendingReason::Stable(reason) | PendingReason::Retryable { reason, .. } => {
                reason.clone()
            }
        };
        log::warn!("[CODEX-SYNC] deferred {}: {reason}", file_path.display());
    }
    CodexFileSyncResult {
        deferred: true,
        ..CodexFileSyncResult::default()
    }
}

/// The transaction granularity of the bulk insert of a single file. The UI queries are blocked
/// by the connection mutex for a few milliseconds within a batch, the lock is released between
/// the batches so that the read side can jump the queue -- that balances the throughput
/// (avoiding the fsync of every row of a per-row autocommit)
/// with the responsiveness of the panels during a re-import of a big file.
const CODEX_INSERT_BATCH_SIZE: usize = 1000;

fn update_codex_sync_state_on_conn(
    conn: &rusqlite::Connection,
    file_path: &str,
    modified: i64,
    parsed: &ParsedCodexFile,
) -> Result<(), AppError> {
    update_sync_state_on_conn(conn, file_path, modified, parsed.line_offset)?;
    conn.execute(
        "UPDATE session_log_sync SET last_byte_offset = ?1 WHERE file_path = ?2",
        rusqlite::params![parsed.observed_bytes, file_path],
    )?;
    Ok(())
}

fn update_codex_sync_state(
    db: &Database,
    file_path: &str,
    modified: i64,
    parsed: &ParsedCodexFile,
) -> Result<(), AppError> {
    let conn = lock_conn!(db.conn);
    let tx = conn.unchecked_transaction()?;
    update_codex_sync_state_on_conn(&tx, file_path, modified, parsed)?;
    tx.commit()?;
    Ok(())
}

/// Sync a single Codex JSONL file.
fn sync_single_codex_file(
    db: &Database,
    file_path: &Path,
    rollout_index: &RolloutIndex,
    pass: &mut CodexSyncPass,
) -> Result<CodexFileSyncResult, AppError> {
    let file_path_str = file_path.to_string_lossy().to_string();

    // Fetch the file metadata
    let metadata = fs::metadata(file_path)
        .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
    let file_modified = metadata_modified_nanos(&metadata);
    let file_size = metadata.len();

    // Check the sync state
    let (last_modified, last_offset) = get_codex_sync_state(db, file_path, pass)?;

    // Windows may keep mtime unchanged while Codex holds its write handle open.
    // Legacy cursors have no byte offset: rescan once to catch up and persist it.
    let last_byte_offset = pass.byte_offsets.get(&file_path_str).copied().flatten();
    if file_modified == last_modified && last_byte_offset == i64::try_from(file_size).ok() {
        return Ok(CodexFileSyncResult::default());
    }

    // Capture child identity before parsing as well as before checking a
    // pending entry: replacing a deferred child can preserve mtime and size.
    // Unknown/inaccessible identity must not suppress the parser's retry/error.
    let child_file = fs::File::open(file_path)
        .ok()
        .and_then(|file| ParentFileStamp::from_file(&file));
    let pending = replay_caches()
        .lock()
        .ok()
        .and_then(|caches| caches.pending.get(file_path).cloned());
    if let Some(pending) = pending {
        if pending.modified == file_modified && pending.size == file_size {
            let unchanged = match &pending.reason {
                PendingReason::MissingParent(parent) => !rollout_index.contains_key(parent),
                PendingReason::Stable(_) => true,
                PendingReason::Retryable {
                    parent_id,
                    child_file: Some(previous_child),
                    parent_files: Some(parent_files),
                    ..
                } => {
                    child_file.as_ref() == Some(previous_child)
                        && parent_dependency_stamp(parent_id, rollout_index).as_ref()
                            == Some(parent_files)
                }
                PendingReason::Retryable { .. } => false,
            };
            if unchanged {
                return Ok(CodexFileSyncResult {
                    deferred: true,
                    ..CodexFileSyncResult::default()
                });
            }
        }
    }

    let parsed = parse_codex_file_with_index(
        file_path,
        thread_id_from_filename(file_path),
        Some(rollout_index),
    )?;
    if !parsed.has_billable_tokens {
        update_codex_sync_state(db, &file_path_str, file_modified, &parsed)?;
        return Ok(CodexFileSyncResult::default());
    }
    let Some(root_thread_id) = parsed.root_thread_id.as_deref() else {
        return Ok(mark_deferred(
            file_path,
            file_modified,
            file_size,
            PendingReason::Stable("文件名缺少有效的尾部 UUID".to_string()),
        ));
    };
    if !parsed.root_meta_seen {
        return Ok(mark_deferred(
            file_path,
            file_modified,
            file_size,
            PendingReason::Stable("含计费 token 但尚无 session_meta".to_string()),
        ));
    }

    let replay_prefix = match &parsed.parent {
        ParentResolution::None | ParentResolution::Continuation { .. } => 0,
        ParentResolution::Deferred(reason) => {
            let pending_reason = if let Some(meta_id) = parsed.meta_thread_id.as_deref() {
                if rollout_index.contains_key(meta_id) {
                    PendingReason::Retryable {
                        reason: reason.clone(),
                        parent_id: meta_id.to_string(),
                        child_file,
                        parent_files: parent_dependency_stamp(meta_id, rollout_index),
                    }
                } else {
                    PendingReason::Stable(reason.clone())
                }
            } else {
                PendingReason::Stable(reason.clone())
            };
            return Ok(mark_deferred(
                file_path,
                file_modified,
                file_size,
                pending_reason,
            ));
        }
        ParentResolution::Parent(parent_id) => {
            let Some(cutoff) = parsed.root_timestamp else {
                return Ok(mark_deferred(
                    file_path,
                    file_modified,
                    file_size,
                    PendingReason::Stable(
                        "parented rollout 的 root meta 缺少有效 timestamp".to_string(),
                    ),
                ));
            };
            // Copy the query result out before the lock is released: under Rust 2021 the temporary of an
            // `if let .. else` lives until the end of the else branch, so on a poisoned lock the Err still
            // holds the guard and locking again in the else branch deadlocks.
            // None = the lock is poisoned (the cache is not used), Some(None) = a miss.
            let cached_prefix = replay_caches().lock().ok().map(|caches| {
                caches
                    .replay_prefixes
                    .get(file_path)
                    .filter(|cached| cached.modified == file_modified && cached.size == file_size)
                    .map(|cached| cached.prefix)
            });
            match cached_prefix {
                Some(Some(prefix)) => prefix,
                Some(None) => {
                    let parent_files = parent_dependency_stamp(parent_id, rollout_index);
                    let parent_signatures =
                        match resolve_parent_signatures(parent_id, cutoff, rollout_index) {
                            Ok(signatures) => signatures,
                            Err(error) => {
                                let pending_reason = if rollout_index.contains_key(parent_id) {
                                    PendingReason::Retryable {
                                        reason: error.reason,
                                        parent_id: parent_id.clone(),
                                        child_file,
                                        parent_files: if error.cacheable {
                                            parent_files
                                        } else {
                                            None
                                        },
                                    }
                                } else {
                                    PendingReason::MissingParent(parent_id.clone())
                                };
                                return Ok(mark_deferred(
                                    file_path,
                                    file_modified,
                                    file_size,
                                    pending_reason,
                                ));
                            }
                        };
                    let prefix = matching_replay_prefix(&parsed.token_events, &parent_signatures);
                    if let Ok(mut caches) = replay_caches().lock() {
                        caches.replay_prefixes.insert(
                            file_path.to_path_buf(),
                            CachedReplayPrefix {
                                modified: file_modified,
                                size: file_size,
                                prefix,
                            },
                        );
                    }
                    prefix
                }
                None => {
                    let parent_signatures =
                        resolve_parent_signatures(parent_id, cutoff, rollout_index)
                            .map_err(|error| AppError::Config(error.reason))?;
                    matching_replay_prefix(&parsed.token_events, &parent_signatures)
                }
            }
        }
    };

    if let Ok(mut caches) = replay_caches().lock() {
        caches.pending.remove(file_path);
    }

    let mut result = CodexFileSyncResult::default();
    let mut to_insert: Vec<(&ParsedTokenEvent, String)> = Vec::new();
    for (token_offset, event) in parsed.token_events.iter().enumerate() {
        let Some(event_index) = event.event_index else {
            continue;
        };
        if token_offset < replay_prefix {
            if event.line_offset > last_offset {
                result.skipped = result.skipped.saturating_add(1);
            }
            continue;
        }
        if event.line_offset <= last_offset {
            continue;
        }
        to_insert.push((event, event_index.to_string()));
    }
    for event in &parsed.usage_record_events {
        let Some(suffix) = event.request_suffix.as_deref() else {
            continue;
        };
        if event.line_offset <= last_offset {
            continue;
        }
        to_insert.push((event, suffix.to_string()));
    }

    // Write to the database in batched transactions: a per-row autocommit (under journal_mode=delete
    // a whole journal create/fsync/delete per row) is the biggest cost of a full re-import. A failed
    // single insert within a batch
    // keeps the old behaviour of skipping that row and continuing; a failing commit of a batch rolls
    // the whole batch back and the cursor does not advance,
    // and the rescan of the next pass is covered by the request_id primary key plus the fingerprint
    // deduplication, so there is no double count.
    //
    // The session_id records the thread ID of the root meta: under a two segment file name (the
    // replacement rollout of a thread/revert)
    // that is the leading UUID, the same as the session identity on the side of the session
    // manager; the trailing rollout
    // ID only serves as the request_id deduplication key (the event_index counts per physical file
    // and must not be switched to the
    // leading ID).
    let session_thread_id = parsed.meta_thread_id.as_deref().unwrap_or(root_thread_id);
    let batch_count = to_insert.len().div_ceil(CODEX_INSERT_BATCH_SIZE);
    for (batch_index, batch) in to_insert.chunks(CODEX_INSERT_BATCH_SIZE).enumerate() {
        let is_last_batch = batch_index + 1 == batch_count;
        let conn = lock_conn!(db.conn);
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| AppError::Database(format!("开启 Codex 会话写入事务失败: {e}")))?;

        let mut batch_imported = 0u32;
        let mut batch_skipped = 0u32;
        let mut batch_suspected = 0u32;
        for (event, suffix) in batch {
            let request_id = format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{root_thread_id}:{suffix}");
            match insert_codex_session_entry_on_conn(
                &tx,
                &request_id,
                &event.delta,
                &event.model,
                Some(session_thread_id),
                event.timestamp.as_deref(),
                event.timing,
                &mut batch_suspected,
                &mut pass.pricing,
            ) {
                Ok(true) => batch_imported += 1,
                Ok(false) => batch_skipped += 1,
                Err(e) => {
                    log::warn!("[CODEX-SYNC] 插入失败 ({request_id}): {e}");
                    batch_skipped += 1;
                }
            }
        }
        if is_last_batch {
            // The cursor advance is committed in the same transaction as the last batch of the data: when
            // it crashes in between both are rolled back,
            // so there is no window of lost data with "the cursor advanced but the data is missing".
            update_codex_sync_state_on_conn(&tx, &file_path_str, file_modified, &parsed)?;
        }
        tx.commit()
            .map_err(|e| AppError::Database(format!("提交 Codex 会话写入事务失败: {e}")))?;

        result.imported = result.imported.saturating_add(batch_imported);
        result.skipped = result.skipped.saturating_add(batch_skipped);
        result.suspected_duplicates = result.suspected_duplicates.saturating_add(batch_suspected);
    }

    if to_insert.is_empty() {
        update_codex_sync_state(db, &file_path_str, file_modified, &parsed)?;
    }
    Ok(result)
}

/// Insert a single Codex session record into proxy_request_logs (a convenience wrapper that
/// takes the lock itself, for the tests only;
/// the production path uses [`insert_codex_session_entry_on_conn`] to reuse the batch
/// transaction and the pricing cache)
#[cfg(test)]
fn insert_codex_session_entry(
    db: &Database,
    request_id: &str,
    delta: &DeltaTokens,
    model: &str,
    session_id: Option<&str>,
    timestamp: Option<&str>,
    suspected_duplicates: &mut u32,
) -> Result<bool, AppError> {
    let conn = lock_conn!(db.conn);
    insert_codex_session_entry_on_conn(
        &conn,
        request_id,
        delta,
        model,
        session_id,
        timestamp,
        None,
        suspected_duplicates,
        &mut HashMap::new(),
    )
}

/// Insert a single Codex session record into proxy_request_logs.
///
/// The caller is responsible for the lock / transaction; `pricing_cache` is keyed by the raw
/// model string
/// (`find_codex_pricing` is a pure functional lookup, the same string always gives the same
/// result), which turns the pricing SELECT per event into one per model during a full
/// re-import. `timing` is the request duration and the time to the first token estimated from
/// the event timestamps (see [`RequestTimer`]), None when it cannot be estimated.
#[allow(clippy::too_many_arguments)]
fn insert_codex_session_entry_on_conn(
    conn: &rusqlite::Connection,
    request_id: &str,
    delta: &DeltaTokens,
    model: &str,
    session_id: Option<&str>,
    timestamp: Option<&str>,
    timing: Option<EstimatedTiming>,
    suspected_duplicates: &mut u32,
    pricing_cache: &mut HashMap<String, Option<ModelPricing>>,
) -> Result<bool, AppError> {
    let created_at = timestamp
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
        app_type: "codex",
        model,
        input_tokens: delta.input,
        output_tokens: delta.output,
        cache_read_tokens: delta.cached_input,
        cache_creation_tokens: delta.cache_write_input,
        created_at,
    };
    if should_skip_session_insert(conn, request_id, &dedup_key)? {
        return Ok(false);
    }
    if has_suspected_codex_session_duplicate(conn, request_id, &dedup_key)? {
        *suspected_duplicates = suspected_duplicates.saturating_add(1);
        log::warn!(
            "[CODEX-SYNC] 疑似重复会话用量: request_id={request_id}, model={model}, input={}, output={}, cache_read={}",
            delta.input,
            delta.output,
            delta.cached_input
        );
    }

    // Compute the cost
    let usage = TokenUsage {
        input_tokens: delta.input,
        output_tokens: delta.output,
        cache_read_tokens: delta.cached_input,
        cache_creation_tokens: delta.cache_write_input,
        cache_creation_1h_tokens: 0,
        model: Some(model.to_string()),
        message_id: None,
    };

    let pricing = pricing_cache
        .entry(model.to_string())
        .or_insert_with(|| find_codex_pricing(conn, model));
    let multiplier = Decimal::from(1);
    let (input_cost, output_cost, cache_read_cost, cache_creation_cost, total_cost) = match pricing
    {
        Some(p) => {
            let cost = CostCalculator::calculate_for_app("codex", &usage, p, multiplier);
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
    };

    let inserted_rows = conn
        .prepare_cached(
            "INSERT OR IGNORE INTO proxy_request_logs (
            request_id, provider_id, app_type, model, request_model,
            input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
            input_cost_usd, output_cost_usd, cache_read_cost_usd, cache_creation_cost_usd, total_cost_usd,
            latency_ms, first_token_ms, status_code, error_message, session_id,
            provider_type, is_streaming, cost_multiplier, created_at, data_source,
            input_token_semantics
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)",
        )
        .and_then(|mut stmt| stmt.execute(rusqlite::params![
                request_id,
                "_codex_session",    // provider_id
                "codex",             // app_type
                model,
                model,               // request_model = model
                delta.input,
                delta.output,
                delta.cached_input,
                delta.cache_write_input,
                input_cost,
                output_cost,
                cache_read_cost,
                cache_creation_cost,
                total_cost,
                timing.map_or(0, |t| t.latency_ms), // latency_ms: estimated from the timestamps, 0 = no timing
                timing.and_then(|t| t.first_token_ms), // first_token_ms: estimated from the timestamps, NULL when it cannot be derived
                200i64,              // status_code
                Option::<String>::None, // error_message
                session_id.map(|s| s.to_string()),
                Some("codex_session"), // provider_type
                1i64,                // is_streaming
                "1.0",               // cost_multiplier
                created_at,
                "codex_session",     // data_source
                INPUT_TOKEN_SEMANTICS_TOTAL, // The input_tokens include the cache reads and writes
            ]))
        .map_err(|e| AppError::Database(format!("插入 Codex 会话日志失败: {e}")))?;

    Ok(inserted_rows > 0)
}

/// Look the Codex model pricing up (with the normalization)
fn find_codex_pricing(conn: &rusqlite::Connection, model_id: &str) -> Option<ModelPricing> {
    find_model_pricing(conn, &normalize_codex_model(model_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::session_usage::get_sync_state;
    use crate::services::sql_helpers::INPUT_TOKEN_SEMANTICS_LEGACY;
    use tempfile::tempdir;

    const PARENT_ID: &str = "00000000-0000-4000-8000-000000000001";
    const CHILD_A_ID: &str = "00000000-0000-4000-8000-000000000002";
    const CHILD_B_ID: &str = "00000000-0000-4000-8000-000000000003";
    const PARENT_PAGE_ID: &str = "00000000-0000-4000-8000-000000000004";
    const CHILD_A_PAGE_ID: &str = "00000000-0000-4000-8000-000000000005";
    const CHILD_A_PAGE_2_ID: &str = "00000000-0000-4000-8000-000000000006";

    thread_local! {
        static CODEX_PARSE_COUNTS: std::cell::RefCell<HashMap<PathBuf, usize>> =
            std::cell::RefCell::new(HashMap::new());
    }

    pub(super) fn record_codex_parse(path: &Path) {
        CODEX_PARSE_COUNTS.with(|counts| {
            *counts.borrow_mut().entry(path.to_path_buf()).or_default() += 1;
        });
    }

    fn codex_parse_count(path: &Path) -> usize {
        CODEX_PARSE_COUNTS.with(|counts| counts.borrow().get(path).copied().unwrap_or(0))
    }

    fn retryable_parent_and_child(dir: &Path) -> (PathBuf, PathBuf) {
        let parent = rollout_path(dir, PARENT_ID);
        let child = rollout_path(dir, CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:02Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:06Z"),
                token_count_at(1_300, 1_050, 150, "2026-07-10T03:00:07Z"),
            ],
        );
        (parent, child)
    }

    fn append_jsonl(path: &Path, value: serde_json::Value) {
        use std::io::Write;
        let mut writer = fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(writer, "{value}").unwrap();
    }

    fn write_jsonl(path: &Path, values: &[serde_json::Value]) {
        let contents = values
            .iter()
            .map(serde_json::Value::to_string)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, contents).unwrap();
    }

    fn rollout_path(dir: &Path, thread_id: &str) -> PathBuf {
        dir.join(format!("rollout-2026-07-10T03-00-00-{thread_id}.jsonl"))
    }

    fn paginated_session_meta_at(
        thread_id: &str,
        forked_from_id: Option<&str>,
        spawned_from_id: Option<&str>,
        history_base_thread_id: Option<&str>,
        timestamp: &str,
    ) -> serde_json::Value {
        let source = spawned_from_id.map_or_else(
            || serde_json::Value::String("cli".to_string()),
            |parent| {
                serde_json::json!({
                    "subagent": {
                        "thread_spawn": { "parent_thread_id": parent }
                    }
                })
            },
        );
        let mut payload = serde_json::json!({
            "id": thread_id,
            "forked_from_id": forked_from_id,
            "source": source,
            "history_mode": "paginated",
        });
        if let Some(hb_id) = history_base_thread_id {
            payload["history_base"] = serde_json::json!({
                "thread_id": hb_id,
                "end_ordinal_exclusive": 100,
                "end_byte_offset": 5000,
            });
        }
        serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": payload
        })
    }

    fn session_meta_at(
        thread_id: &str,
        forked_from_id: Option<&str>,
        spawned_from_id: Option<&str>,
        timestamp: &str,
    ) -> serde_json::Value {
        let source = spawned_from_id.map_or_else(
            || serde_json::Value::String("cli".to_string()),
            |parent| {
                serde_json::json!({
                    "subagent": {
                        "thread_spawn": { "parent_thread_id": parent }
                    }
                })
            },
        );
        serde_json::json!({
            "timestamp": timestamp,
            "type": "session_meta",
            "payload": {
                "id": thread_id,
                "forked_from_id": forked_from_id,
                "source": source
            }
        })
    }

    fn session_meta(thread_id: &str) -> serde_json::Value {
        session_meta_at(thread_id, None, None, "2026-07-10T03:00:00Z")
    }

    fn turn_context_for_model_at(model: &str, timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "turn_context",
            "payload": { "model": model }
        })
    }

    fn turn_context_at(timestamp: &str) -> serde_json::Value {
        turn_context_for_model_at("gpt-5.6-sol", timestamp)
    }

    fn turn_context() -> serde_json::Value {
        turn_context_at("2026-07-10T03:00:01Z")
    }

    fn thread_settings_at(model: serde_json::Value, timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "thread_settings_applied",
                "thread_settings": { "model": model }
            }
        })
    }

    fn token_count_at(input: u64, cached: u64, output: u64, timestamp: &str) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": { "total_token_usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "output_tokens": output,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input + output
                }}
            }
        })
    }

    fn token_count(input: u64, cached: u64, output: u64) -> serde_json::Value {
        token_count_at(input, cached, output, "2026-07-10T03:00:02Z")
    }

    fn token_count_without_timestamp(input: u64, cached: u64, output: u64) -> serde_json::Value {
        let mut value = token_count(input, cached, output);
        value
            .as_object_mut()
            .expect("token_count must be an object")
            .remove("timestamp");
        value
    }

    #[allow(clippy::too_many_arguments)]
    fn token_count_with_last_at(
        total_input: u64,
        total_cached: u64,
        total_output: u64,
        last_input: u64,
        last_cached: u64,
        last_output: u64,
        limit_id: &str,
        timestamp: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": total_input,
                        "cached_input_tokens": total_cached,
                        "output_tokens": total_output,
                        "reasoning_output_tokens": 0,
                        "total_tokens": total_input + total_output
                    },
                    "last_token_usage": {
                        "input_tokens": last_input,
                        "cached_input_tokens": last_cached,
                        "output_tokens": last_output,
                        "reasoning_output_tokens": 0,
                        "total_tokens": last_input + last_output
                    }
                },
                "rate_limits": { "limit_id": limit_id }
            }
        })
    }

    fn token_usage_record_at(
        response_id: &str,
        input: u64,
        cached: u64,
        output: u64,
        timestamp: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "timestamp": timestamp,
            "type": "token_usage_record",
            "payload": {
                "response_id": response_id,
                "usage": {
                    "input_tokens": input,
                    "cached_input_tokens": cached,
                    "cache_write_input_tokens": 0,
                    "output_tokens": output,
                    "reasoning_output_tokens": 0,
                    "total_tokens": input + output
                }
            }
        })
    }

    fn codex_usage_rows(db: &Database) -> Result<Vec<(String, i64, i64)>, AppError> {
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare(
                "SELECT request_id, input_tokens, output_tokens
                 FROM proxy_request_logs WHERE data_source = 'codex_session'
                 ORDER BY request_id",
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|error| AppError::Database(error.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(rows)
    }

    #[test]
    fn remote_compaction_usage_record_is_imported_when_the_following_token_count_is_zero(
    ) -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let path = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &path,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_usage_record_at("resp_compact", 26_705, 0, 491, "2026-07-10T03:00:02Z"),
                token_count_with_last_at(0, 0, 0, 0, 0, 0, "default", "2026-07-10T03:00:02Z"),
            ],
        );

        let result = sync_test_file(&db, &path, &[&path])?;

        assert_eq!(result.imported, 1, "compaction usage must be imported");
        let rows = codex_usage_rows(&db)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 26_705);
        assert_eq!(rows[0].2, 491);
        assert!(rows[0].0.ends_with(":usage:resp_compact"));
        Ok(())
    }

    #[test]
    fn usage_record_covered_by_token_count_is_not_imported_twice() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let path = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &path,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_usage_record_at("resp_turn", 26_705, 1_000, 491, "2026-07-10T03:00:02Z"),
                token_count_with_last_at(
                    26_705,
                    1_000,
                    491,
                    26_705,
                    1_000,
                    491,
                    "default",
                    "2026-07-10T03:00:03Z",
                ),
            ],
        );

        let result = sync_test_file(&db, &path, &[&path])?;

        assert_eq!(result.imported, 1);
        let rows = codex_usage_rows(&db)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, 26_705);
        assert_eq!(rows[0].2, 491);
        assert_eq!(
            rows[0].0,
            format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{PARENT_ID}:1")
        );
        Ok(())
    }

    #[test]
    fn already_scanned_compaction_stays_behind_the_cursor() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let path = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &path,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_usage_record_at("resp_compact", 26_705, 0, 491, "2026-07-10T03:00:02Z"),
                token_count_with_last_at(0, 0, 0, 0, 0, 0, "default", "2026-07-10T03:00:02Z"),
            ],
        );
        let metadata = fs::metadata(&path).unwrap();
        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO session_log_sync
                    (file_path, last_modified, last_line_offset, last_byte_offset, last_synced_at)
                 VALUES (?1, ?2, 100, ?3, 0)",
                rusqlite::params![
                    path.to_string_lossy().to_string(),
                    metadata_modified_nanos(&metadata),
                    i64::try_from(metadata.len()).unwrap()
                ],
            )
            .map_err(|error| AppError::Database(error.to_string()))?;
        }

        let result = sync_test_file(&db, &path, &[&path])?;

        assert_eq!(result.imported, 0);
        assert!(codex_usage_rows(&db)?.is_empty());
        Ok(())
    }

    fn sync_test_file(
        db: &Database,
        file: &Path,
        all_files: &[&Path],
    ) -> Result<CodexFileSyncResult, AppError> {
        let files = all_files
            .iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        let mut pass = CodexSyncPass::load(db)?;
        sync_single_codex_file(db, file, &build_rollout_index(&files), &mut pass)
    }

    fn assert_unchanged_codex_file_is_skipped(db: &Database, file: &Path) -> Result<(), AppError> {
        let changes_before: i64 = {
            let conn = lock_conn!(db.conn);
            conn.query_row("SELECT total_changes()", [], |row| row.get(0))?
        };
        // Reload the persisted cursor each time: returning zero imports alone
        // does not prove that the file was skipped rather than fully reparsed.
        for _ in 0..3 {
            assert_eq!(sync_test_file(db, file, &[file])?.imported, 0);
        }
        let conn = lock_conn!(db.conn);
        let changes_after: i64 = conn.query_row("SELECT total_changes()", [], |row| row.get(0))?;
        assert_eq!(
            changes_after, changes_before,
            "unchanged file rewrote its cursor"
        );
        Ok(())
    }

    #[test]
    fn test_unchanged_incomplete_tail_is_skipped_after_cursor_reload() -> Result<(), AppError> {
        use std::io::Write;
        for has_usage in [false, true] {
            for tail in ["{\"type\":\"event_msg\"", "  "] {
                let db = Database::memory()?;
                let dir = tempdir().unwrap();
                let file = rollout_path(dir.path(), PARENT_ID);
                let mut records = vec![session_meta(PARENT_ID), turn_context()];
                if has_usage {
                    records.push(token_count(100, 50, 10));
                }
                write_jsonl(&file, &records);
                {
                    let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
                    writer.write_all(tail.as_bytes()).unwrap();
                }
                assert_eq!(
                    sync_test_file(&db, &file, &[&file])?.imported,
                    u32::from(has_usage)
                );
                assert_eq!(
                    get_sync_state(&db, &file.to_string_lossy())?.1,
                    records.len() as i64
                );
                assert_unchanged_codex_file_is_skipped(&db, &file)?;
            }
        }
        Ok(())
    }

    #[test]
    fn test_append_with_unchanged_mtime_survives_reload_without_duplicates() -> Result<(), AppError>
    {
        use std::io::Write;
        let db = Database::memory()?;
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
            ],
        );
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(writer, "{}", token_count(250, 100, 30)).unwrap();
        writer
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        // Each call reloads its cursor from the DB, as after an application restart.
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 0);
        let conn = lock_conn!(db.conn);
        let totals: (i64, i64, i64) = conn.query_row(
            "SELECT count(*), sum(input_tokens), sum(output_tokens) FROM proxy_request_logs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(totals, (2, 250, 30));
        Ok(())
    }

    #[test]
    fn test_legacy_cursor_catches_up_with_unchanged_mtime() -> Result<(), AppError> {
        let db = Database::memory()?;
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
                token_count(250, 100, 30),
            ],
        );
        let modified = metadata_modified_nanos(&fs::metadata(&file).unwrap());
        update_sync_state(&db, &file.to_string_lossy(), modified, 3)?;
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 0);
        let conn = lock_conn!(db.conn);
        let bytes: i64 =
            conn.query_row("SELECT last_byte_offset FROM session_log_sync", [], |r| {
                r.get(0)
            })?;
        assert_eq!(bytes as u64, fs::metadata(&file).unwrap().len());
        Ok(())
    }

    #[test]
    fn test_complete_final_record_without_newline_is_imported_once() -> Result<(), AppError> {
        use std::io::Write;
        let db = Database::memory()?;
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(&file, &[session_meta(PARENT_ID), turn_context()]);
        let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
        write!(writer, "{}", token_count(100, 50, 10)).unwrap();
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 0);
        writeln!(writer).unwrap();
        writeln!(writer, "{}", token_count(250, 100, 30)).unwrap();
        writer
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 0);
        let conn = lock_conn!(db.conn);
        let totals: (i64, i64, i64) = conn.query_row(
            "SELECT count(*), sum(input_tokens), sum(output_tokens) FROM proxy_request_logs",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        assert_eq!(totals, (2, 250, 30));
        Ok(())
    }

    #[test]
    fn test_partial_live_record_is_retried_after_append() -> Result<(), AppError> {
        use std::io::Write;
        let db = Database::memory()?;
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
            ],
        );
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        let next = format!("{}\n", token_count(250, 100, 30));
        let split = next.len() / 2;
        let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
        writer.write_all(&next.as_bytes()[..split]).unwrap();
        writer
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(get_sync_state(&db, &file.to_string_lossy())?.1, 3);
        assert_unchanged_codex_file_is_skipped(&db, &file)?;
        writer.write_all(&next.as_bytes()[split..]).unwrap();
        writer
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 0);
        assert_eq!(get_sync_state(&db, &file.to_string_lossy())?.1, 4);
        Ok(())
    }

    /// The replacement rollout of a revert is a two segment file name `<threadId>_<rolloutId>`,
    /// the id of the root meta is the original thread ID (the first UUID) and the forked_from_id is
    /// empty.
    /// The old check only accepted the trailing UUID and would defer such a file forever, so all
    /// the usage that a later resume appended
    /// would be lost.
    #[test]
    fn test_resumed_rollout_meta_id_matching_leading_uuid_is_not_deferred() -> Result<(), AppError>
    {
        let dir = tempdir().unwrap();
        let file = dir.path().join(format!(
            "rollout-2026-08-26T17-18-13-{PARENT_ID}_{CHILD_A_ID}.jsonl"
        ));
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_at(100, 50, 20, "2026-08-26T09:18:20Z"),
            ],
        );

        let parsed = parse_codex_file(&file, thread_id_from_filename(&file))?;

        // A resumed session has no explicit parent and must not be rejected because of an ID mismatch
        assert!(
            !matches!(parsed.parent, ParentResolution::Deferred(_)),
            "恢复会话不应被 deferred，实际: {:?}",
            parsed.parent
        );
        assert_eq!(parsed.root_thread_id.as_deref(), Some(CHILD_A_ID));
        assert!(parsed.has_billable_tokens);
        Ok(())
    }

    /// The two segment file name that the paging of a branch thread produces still carries a
    /// leftover forked_from_id,
    /// and while parsing it has to be recognized as a continuation of this thread (Continuation)
    /// and the baseline of the previous page has to be restored to compute the delta.
    #[test]
    fn test_parse_codex_file_recognizes_branch_thread_paginated_continuation(
    ) -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let root = rollout_path(dir.path(), CHILD_A_ID);
        write_jsonl(
            &root,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-08-26T09:18:00Z"),
                token_count_at(50, 25, 10, "2026-08-26T09:18:05Z"),
            ],
        );
        let file = dir.path().join(format!(
            "rollout-2026-08-26T17-18-13-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &file,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_ID),
                    "2026-08-26T09:18:20Z",
                ),
                turn_context(),
                token_count_at(100, 50, 20, "2026-08-26T09:18:21Z"),
            ],
        );

        let index = build_rollout_index(&[root, file.clone()]);
        let parsed =
            parse_codex_file_with_index(&file, thread_id_from_filename(&file), Some(&index))?;

        assert!(
            matches!(parsed.parent, ParentResolution::Continuation { .. }),
            "分支线程续写页应被识别为 Continuation，实际: {:?}",
            parsed.parent
        );
        assert_eq!(parsed.root_thread_id.as_deref(), Some(CHILD_A_PAGE_ID));
        assert_eq!(parsed.meta_thread_id.as_deref(), Some(CHILD_A_ID));
        assert!(parsed.has_billable_tokens);
        // Verify the restoration of the previous baseline: 100 - 50 = a delta of 50
        assert_eq!(parsed.token_events[0].delta.input, 50);
        assert_eq!(parsed.token_events[0].delta.output, 10);
        Ok(())
    }

    /// The division of labour of the two UUIDs of a two segment rollout in the complete sync chain:
    /// the request_id uses the trailing
    /// rollout ID (the event_index counts per physical file, the deduplication key must not be
    /// swapped) and the
    /// session_id in the database records the leading thread ID, the same as the session identity on
    /// the side of the session manager.
    #[test]
    fn test_resumed_rollout_session_id_uses_leading_thread_id() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let file = temp.path().join(format!(
            "rollout-2026-08-26T17-18-13-{PARENT_ID}_{CHILD_A_ID}.jsonl"
        ));
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
            ],
        );

        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);

        let conn = lock_conn!(db.conn);
        let (request_id, session_id) = conn
            .prepare(
                "SELECT request_id, session_id FROM proxy_request_logs
                 WHERE data_source = 'codex_session'",
            )?
            .query_row([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
        assert_eq!(
            request_id,
            format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:1")
        );
        assert_eq!(session_id, PARENT_ID);
        Ok(())
    }

    /// An inconsistency of a single segment file name still has to be rejected.
    #[test]
    fn test_single_uuid_filename_meta_mismatch_still_deferred() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        // The meta names the ID of another thread -- that is not a shape the two segment file name of a revert can explain
        write_jsonl(
            &file,
            &[
                session_meta(CHILD_B_ID),
                turn_context(),
                token_count_at(1, 1, 1, "2026-07-10T03:00:02Z"),
            ],
        );

        let parsed = parse_codex_file(&file, thread_id_from_filename(&file))?;
        assert!(matches!(parsed.parent, ParentResolution::Deferred(_)));
        Ok(())
    }

    #[test]
    fn test_delta_first_event() {
        let prev = None;
        let current = CumulativeTokens {
            input: 17934,
            cached_input: 9600,
            cache_write_input: 0,
            output: 454,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 17934);
        assert_eq!(delta.cached_input, 9600);
        assert_eq!(delta.output, 454);
        assert!(!delta.is_zero());
    }

    #[test]
    fn test_delta_subsequent_event() {
        let prev = Some(CumulativeTokens {
            input: 17934,
            cached_input: 9600,
            cache_write_input: 0,
            output: 454,
        });
        let current = CumulativeTokens {
            input: 36722,
            cached_input: 27904,
            cache_write_input: 0,
            output: 804,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 36722 - 17934);
        assert_eq!(delta.cached_input, 27904 - 9600);
        assert_eq!(delta.output, 804 - 454);
    }

    #[test]
    fn test_delta_zero_at_task_boundary() {
        let prev = Some(CumulativeTokens {
            input: 58346,
            cached_input: 46976,
            cache_write_input: 0,
            output: 1045,
        });
        // The task boundary: the same accumulated value
        let current = CumulativeTokens {
            input: 58346,
            cached_input: 46976,
            cache_write_input: 0,
            output: 1045,
        };
        let delta = compute_delta(&prev, &current);
        assert!(delta.is_zero());
    }

    #[test]
    fn test_delta_saturating_sub() {
        // The exceptional case: the current value is smaller than the previous one (it should not happen, but it has to be guarded)
        let prev = Some(CumulativeTokens {
            input: 100,
            cached_input: 50,
            cache_write_input: 0,
            output: 30,
        });
        let current = CumulativeTokens {
            input: 80,
            cached_input: 40,
            cache_write_input: 0,
            output: 20,
        };
        let delta = compute_delta(&prev, &current);
        assert_eq!(delta.input, 0);
        assert_eq!(delta.cached_input, 0);
        assert_eq!(delta.output, 0);
        assert!(delta.is_zero());
    }

    #[test]
    fn test_interleaved_counter_lanes_use_exact_last_usage() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let bengal_event = token_count_with_last_at(
            87_709_262,
            83_563_008,
            240_919,
            151_258,
            147_200,
            87,
            "codex_bengalfox",
            "2026-07-10T03:00:03Z",
        );
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_with_last_at(
                    76_780_408,
                    73_010_432,
                    243_036,
                    175_074,
                    169_728,
                    6_827,
                    "codex",
                    "2026-07-10T03:00:02Z",
                ),
                bengal_event.clone(),
                token_count_with_last_at(
                    76_962_538,
                    73_180_160,
                    243_258,
                    182_130,
                    169_728,
                    222,
                    "codex",
                    "2026-07-10T03:00:04Z",
                ),
                // Repeated snapshots are notifications, not additional API usage.
                bengal_event,
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| {
                (
                    event.delta.input,
                    event.delta.cached_input,
                    event.delta.output,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            deltas,
            vec![
                (175_074, 169_728, 6_827),
                (151_258, 147_200, 87),
                (182_130, 169_728, 222),
            ]
        );
        assert!(parsed.token_events[3].delta.is_zero());
        Ok(())
    }

    #[test]
    fn test_cross_limit_snapshot_replay_is_not_double_counted() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:02Z"),
                token_count_with_last_at(
                    1_000,
                    0,
                    10,
                    100,
                    0,
                    10,
                    "codex_bengalfox",
                    "2026-07-10T03:00:03Z",
                ),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100]);
        Ok(())
    }

    #[test]
    fn test_adjacent_replay_burst_across_multiple_sources_is_deduped() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:02Z"),
                token_count_with_last_at(
                    1_000,
                    0,
                    10,
                    100,
                    0,
                    10,
                    "codex_bengalfox",
                    "2026-07-10T03:00:03Z",
                ),
                token_count_with_last_at(
                    1_000,
                    0,
                    10,
                    100,
                    0,
                    10,
                    "codex_spark",
                    "2026-07-10T03:00:04Z",
                ),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100]);
        Ok(())
    }

    #[test]
    fn test_cross_source_replay_remains_adjacent_across_non_token_events() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:02Z"),
                turn_context_for_model_at("gpt-5.6-sol", "2026-07-10T03:00:03Z"),
                token_count_with_last_at(
                    1_000,
                    0,
                    10,
                    100,
                    0,
                    10,
                    "codex_bengalfox",
                    "2026-07-10T03:00:04Z",
                ),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100]);
        Ok(())
    }

    #[test]
    fn test_same_source_repeat_is_deduped_after_another_source_advances() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:02Z"),
                token_count_with_last_at(
                    2_000,
                    0,
                    20,
                    100,
                    0,
                    10,
                    "codex_bengalfox",
                    "2026-07-10T03:00:03Z",
                ),
                // `codex` has not advanced since its X snapshot, so this is a
                // same-source replay even though another source was interleaved.
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:04Z"),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100, 100]);
        Ok(())
    }

    #[test]
    fn test_stale_cross_source_signature_does_not_swallow_reset() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                // `codex` emits snapshot X.
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:02Z"),
                // X is replayed under another rate-limit source.
                token_count_with_last_at(
                    1_000,
                    0,
                    10,
                    100,
                    0,
                    10,
                    "codex_bengalfox",
                    "2026-07-10T03:00:03Z",
                ),
                // The original source advances to Y.
                token_count_with_last_at(2_000, 0, 20, 100, 0, 10, "codex", "2026-07-10T03:00:04Z"),
                // A genuine reset later reproduces X. The stale copy retained
                // by `codex_bengalfox` must not classify this as a replay.
                token_count_with_last_at(1_000, 0, 10, 100, 0, 10, "codex", "2026-07-10T03:00:05Z"),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100, 100, 100]);
        Ok(())
    }

    #[test]
    fn test_full_snapshot_dedupe_allows_counter_reset() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let first =
            token_count_with_last_at(100, 50, 10, 100, 50, 10, "codex", "2026-07-10T03:00:02Z");
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                first.clone(),
                first,
                token_count_with_last_at(
                    200,
                    100,
                    20,
                    100,
                    50,
                    10,
                    "codex",
                    "2026-07-10T03:00:04Z",
                ),
                // A restarted counter may legitimately return to an older
                // total after another full snapshot has advanced the source.
                token_count_with_last_at(100, 50, 10, 50, 25, 5, "codex", "2026-07-10T03:00:05Z"),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100, 100, 50]);
        Ok(())
    }

    #[test]
    fn test_cache_write_tokens_are_billed_as_cache_creation() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        // Codex reports cache writes as a subset of input_tokens, disjoint from cached reads.
        let usage = serde_json::json!({
            "input_tokens": 1_000,
            "cached_input_tokens": 300,
            "cache_write_input_tokens": 600,
            "output_tokens": 50,
            "reasoning_output_tokens": 0,
            "total_tokens": 1_050
        });
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                serde_json::json!({
                    "timestamp": "2026-07-10T03:00:02Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": { "total_token_usage": usage, "last_token_usage": usage }
                    }
                }),
            ],
        );

        let db = Database::memory()?;
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);

        let conn = lock_conn!(db.conn);
        let fresh_input = crate::services::sql_helpers::fresh_input_sql("");
        let (fresh, cache_read, cache_creation, cache_creation_cost, total_cost): (
            i64,
            i64,
            i64,
            String,
            String,
        ) = conn.query_row(
            &format!(
                "SELECT {fresh_input}, cache_read_tokens, cache_creation_tokens,
                        cache_creation_cost_usd, total_cost_usd
                 FROM proxy_request_logs WHERE data_source = 'codex_session'"
            ),
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
        assert_eq!((fresh, cache_read, cache_creation), (100, 300, 600));
        // gpt-5.6-sol: $4 input, $20 output, $0.40 cache read, $5 cache write per MTok.
        assert_eq!(
            cache_creation_cost.parse::<Decimal>().unwrap(),
            "0.003".parse().unwrap()
        );
        assert_eq!(
            total_cost.parse::<Decimal>().unwrap(),
            "0.00452".parse().unwrap()
        );
        Ok(())
    }

    #[test]
    fn test_empty_last_usage_falls_back_to_total() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                serde_json::json!({
                    "timestamp": "2026-07-10T03:00:02Z",
                    "type": "event_msg",
                    "payload": {
                        "type": "token_count",
                        "info": {
                            "total_token_usage": {
                                "input_tokens": 100,
                                "cached_input_tokens": 0,
                                "output_tokens": 10,
                                "reasoning_output_tokens": 0,
                                "total_tokens": 110
                            },
                            "last_token_usage": {}
                        }
                    }
                }),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100]);
        Ok(())
    }

    #[test]
    fn test_empty_total_does_not_enable_snapshot_deduplication() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let event = |limit_id: &str, timestamp: &str| {
            serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": {},
                        "last_token_usage": {
                            "input_tokens": 100,
                            "cached_input_tokens": 0,
                            "output_tokens": 10,
                            "reasoning_output_tokens": 0,
                            "total_tokens": 110
                        }
                    },
                    "rate_limits": { "limit_id": limit_id }
                }
            })
        };
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                event("codex", "2026-07-10T03:00:02Z"),
                // Without a usable cumulative total, identical per-request
                // usage is not enough evidence that this is a replay.
                event("codex_bengalfox", "2026-07-10T03:00:03Z"),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100, 100]);
        Ok(())
    }

    #[test]
    fn test_total_fallback_uses_session_baseline_across_model_switch() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context_for_model_at("model-a", "2026-07-10T03:00:01Z"),
                token_count_at(100, 50, 10, "2026-07-10T03:00:02Z"),
                turn_context_for_model_at("model-b", "2026-07-10T03:00:03Z"),
                token_count_at(150, 75, 15, "2026-07-10T03:00:04Z"),
            ],
        );

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        let deltas = parsed
            .token_events
            .iter()
            .filter(|event| !event.delta.is_zero())
            .map(|event| event.delta.input)
            .collect::<Vec<_>>();

        assert_eq!(deltas, vec![100, 50]);
        Ok(())
    }

    #[test]
    fn test_thread_settings_model_switch_imports_correct_usage_and_cost() -> Result<(), AppError> {
        use std::io::Write;

        let db = Database::memory()?;
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let first_usage = token_count_at(100, 50, 10, "2026-07-10T03:00:02Z");
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                turn_context_for_model_at("gpt-6-astra", "2026-07-10T03:00:01Z"),
                first_usage.clone(),
            ],
        );
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);

        {
            let mut writer = fs::OpenOptions::new().append(true).open(&file).unwrap();
            for event in [
                thread_settings_at(
                    serde_json::json!("OpenAI/GPT-6.1-SOL-2026-10-01"),
                    "2026-07-10T03:00:03Z",
                ),
                // A model change must not turn a repeated snapshot into usage.
                first_usage,
                token_count_at(150, 75, 15, "2026-07-10T03:00:04Z"),
            ] {
                writeln!(writer, "{event}").unwrap();
            }
        }
        assert_eq!(sync_test_file(&db, &file, &[&file])?.imported, 1);
        assert_unchanged_codex_file_is_skipped(&db, &file)?;

        let conn = lock_conn!(db.conn);
        let mut statement = conn.prepare(
            "SELECT model, request_model, input_tokens, cache_read_tokens, output_tokens,
                    total_cost_usd, request_id
             FROM proxy_request_logs ORDER BY created_at",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u32>(2)?,
                    row.get::<_, u32>(3)?,
                    row.get::<_, u32>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(rows.len(), 2);
        for (row, (model, input, cached, output, cost, index)) in rows.iter().zip([
            ("gpt-6-astra", 100, 50, 10, "0.00105", 1),
            ("gpt-6.1-sol", 50, 25, 5, "0.0001025", 2),
        ]) {
            assert_eq!(
                (&*row.0, &*row.1, row.2, row.3, row.4),
                (model, model, input, cached, output)
            );
            assert_eq!(
                row.5.parse::<Decimal>().unwrap(),
                cost.parse::<Decimal>().unwrap()
            );
            assert_eq!(
                row.6,
                format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{PARENT_ID}:{index}")
            );
        }
        Ok(())
    }

    #[test]
    fn test_thread_settings_model_without_turn_context() -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        write_jsonl(
            &file,
            &[
                session_meta(PARENT_ID),
                thread_settings_at(serde_json::json!("gpt-6.1-sol"), "2026-07-10T03:00:01Z"),
                token_count(100, 50, 10),
            ],
        );
        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        assert_eq!(parsed.token_events.len(), 1);
        assert_eq!(parsed.token_events[0].model, "gpt-6.1-sol");
        Ok(())
    }

    #[test]
    fn test_thread_settings_model_ignores_invalid_metadata_and_allows_overrides(
    ) -> Result<(), AppError> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let mut events = vec![
            session_meta(PARENT_ID),
            thread_settings_at(serde_json::json!("gpt-6.1-sol"), "2026-07-10T03:00:01Z"),
        ];
        for model in [
            serde_json::Value::Null,
            serde_json::json!(42),
            serde_json::json!(""),
            serde_json::json!(" \t "),
        ] {
            events.push(thread_settings_at(model, "2026-07-10T03:00:02Z"));
        }
        events.push(serde_json::json!({
            "type": "event_msg",
            "payload": { "type": "thread_settings_applied", "thread_settings": { "service_tier": "priority" } }
        }));
        // The fast text filter is not sufficient evidence of an applied setting.
        let mut unrelated =
            thread_settings_at(serde_json::json!("wrong-model"), "2026-07-10T03:00:02Z");
        unrelated["type"] = serde_json::json!("response_item");
        events.push(unrelated);
        events.push(token_count(100, 50, 10));
        events.push(turn_context_for_model_at(
            "gpt-6-astra",
            "2026-07-10T03:00:03Z",
        ));
        events.push(token_count_at(150, 75, 15, "2026-07-10T03:00:04Z"));
        let mut explicit_model = token_count_at(200, 100, 20, "2026-07-10T03:00:05Z");
        explicit_model["payload"]["info"]["model"] = serde_json::json!("custom-model");
        events.push(explicit_model);
        write_jsonl(&file, &events);

        let parsed = parse_codex_file(&file, Some(PARENT_ID.to_string()))?;
        assert_eq!(
            parsed
                .token_events
                .iter()
                .map(|event| event.model.as_str())
                .collect::<Vec<_>>(),
            vec!["gpt-6.1-sol", "gpt-6-astra", "custom-model"]
        );
        Ok(())
    }

    #[test]
    fn test_parse_cumulative_tokens_valid() {
        let json: serde_json::Value = serde_json::json!({
            "input_tokens": 17934,
            "cached_input_tokens": 9600,
            "output_tokens": 454,
            "reasoning_output_tokens": 233,
            "total_tokens": 18388
        });
        let tokens = parse_cumulative_tokens(&json).unwrap();
        assert_eq!(tokens.input, 17934);
        assert_eq!(tokens.cached_input, 9600);
        assert_eq!(tokens.output, 454);
    }

    #[test]
    fn test_parse_cumulative_tokens_null() {
        let json = serde_json::Value::Null;
        assert!(parse_cumulative_tokens(&json).is_none());
    }

    #[test]
    fn test_parse_cumulative_tokens_rejects_empty_object_but_accepts_explicit_zero() {
        assert!(parse_cumulative_tokens(&serde_json::json!({})).is_none());

        let tokens = parse_cumulative_tokens(&serde_json::json!({ "input_tokens": 0 }))
            .expect("an explicit zero is valid usage");
        assert_eq!(tokens.input, 0);
        assert_eq!(tokens.cached_input, 0);
        assert_eq!(tokens.output, 0);
    }

    #[test]
    fn test_parse_cumulative_tokens_alt_field_names() {
        // Some versions may use cache_read_input_tokens instead of cached_input_tokens
        let json: serde_json::Value = serde_json::json!({
            "input_tokens": 1000,
            "cache_read_input_tokens": 500,
            "output_tokens": 200
        });
        let tokens = parse_cumulative_tokens(&json).unwrap();
        assert_eq!(tokens.cached_input, 500);
    }

    #[test]
    fn test_collect_codex_session_files_nonexistent() {
        let files = collect_codex_session_files(Path::new("/nonexistent/path"));
        assert!(files.is_empty());
    }

    #[test]
    #[serial_test::serial]
    fn test_thread_spawn_parent_strips_replay_and_keeps_live_usage() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:06Z"),
                token_count_at(1_300, 1_050, 150, "2026-07-10T03:00:07Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (1, 1, false)
        );

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:2")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (300, 150, 50));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_poisoned_replay_cache_does_not_deadlock_parented_sync() -> Result<(), AppError> {
        clear_codex_replay_caches();
        // Simulate a failing assertion while the lock is held so that the cache lock gets poisoned.
        let _ = std::thread::spawn(|| {
            let _guard = replay_caches().lock().unwrap();
            panic!("poison the replay cache");
        })
        .join();
        assert!(replay_caches().is_poisoned());

        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:06Z"),
                token_count_at(1_300, 1_050, 150, "2026-07-10T03:00:07Z"),
            ],
        );

        // Run it in a child thread: once it regresses into a self-deadlock the test fails with a timeout instead of hanging the whole test process.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = sync_test_file(&db, &child, &[&parent, &child])
                .map(|result| (result.imported, result.skipped, result.deferred));
            let _ = tx.send(result);
            drop(temp);
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("parented sync deadlocked on a poisoned replay cache")?;
        assert_eq!(result, (1, 1, false));

        clear_codex_replay_caches();
        assert!(!replay_caches().is_poisoned());
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_filtered_parent_events_use_subsequence_prefix_alignment() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:02Z"),
                token_count_at(300, 150, 30, "2026-07-10T03:00:03Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
                token_count_at(300, 150, 30, "2026-07-10T03:00:07Z"),
                token_count_at(450, 220, 45, "2026-07-10T03:00:08Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!((result.imported, result.skipped), (1, 2));
        Ok(())
    }

    /// When the file system cannot provide a file identity (for example Windows accessing
    // \\wsl.localhost) the parent timeline does not enter the cache by design,
    /// such an environment only verifies the result and not the reuse of the cache.
    fn parent_cache_supported(path: &Path) -> bool {
        let supported = ParentFileStamp::from_file(&fs::File::open(path).unwrap()).is_some();
        if !supported {
            eprintln!(
                "no file identity for {}; skipping parent cache assertions",
                path.display()
            );
        }
        supported
    }

    /// The cache is process wide and `serial` only excludes the tests that carry the marker: a test
    /// without the marker (for example the parsing of a continuation page goes through
    /// `load_parent_timeline`, which caches the previous page) may write another entry at the same
    /// time.
    /// Only the keys below the temp directory of this test are counted, not the global total.
    fn cached_parent_timelines_under(dir: &Path) -> usize {
        replay_caches()
            .lock()
            .unwrap()
            .parent_timelines
            .keys()
            .filter(|path| path.starts_with(dir))
            .count()
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_malformed_lines_do_not_defer_child_or_prevent_warm_skip() -> Result<(), AppError>
    {
        let cases: &[(&[u8], &[u8], bool)] = &[
            // A bad record must not hide subsequent valid records, including
            // a complete final record without a trailing newline.
            (b"\xff\n", b"", true),
            (b"", b"\xff\n", false),
            (b"", b"\xff", false),
            // Keep the existing malformed-JSON/blank-line behavior too.
            (b"{malformed\n \t\r\n", b"{malformed", false),
        ];
        for compressed in [false, true] {
            for &(prefix, suffix, remove_final_newline) in cases {
                clear_codex_replay_caches();
                let db = Database::memory()?;
                let temp = tempdir().unwrap();
                let (parent, child) = retryable_parent_and_child(temp.path());
                append_jsonl(&parent, turn_context_at("2026-07-10T03:00:10Z"));
                let mut contents = fs::read(&parent).unwrap();
                if remove_final_newline {
                    assert_eq!(contents.pop(), Some(b'\n'));
                }
                fs::write(&parent, [prefix, contents.as_slice(), suffix].concat()).unwrap();
                let parent = if compressed {
                    compress_rollout(&parent)
                } else {
                    parent
                };

                let result = sync_test_file(&db, &child, &[&parent, &child])?;
                assert_eq!(
                    (result.imported, result.skipped, result.deferred),
                    (1, 1, false)
                );
                let timeline = load_parent_timeline(&parent).unwrap();
                assert_eq!(timeline.events.len(), 1);
                assert_eq!(
                    timeline.max_timestamp,
                    Some("2026-07-10T03:00:10Z".parse::<DateTime<Utc>>().unwrap())
                );
                for _ in 0..3 {
                    let warm = sync_test_file(&db, &child, &[&parent, &child])?;
                    assert_eq!((warm.imported, warm.deferred), (0, false));
                }
                assert_eq!(codex_parse_count(&child), 1);
                if parent_cache_supported(&parent) {
                    assert!(Arc::ptr_eq(
                        &timeline,
                        &load_parent_timeline(&parent).unwrap()
                    ));
                }
                let conn = lock_conn!(db.conn);
                let usage: (i64, i64, i64) = conn.query_row(
                    "SELECT input_tokens, cache_read_tokens, output_tokens FROM proxy_request_logs",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
                assert_eq!(usage, (300, 150, 50));
            }
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_rollout_is_cached_once_across_fork_cutoffs() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:10Z"),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );

        let early = "2026-07-10T03:00:05Z".parse::<DateTime<Utc>>().unwrap();
        let late = "2026-07-10T03:00:15Z".parse::<DateTime<Utc>>().unwrap();
        assert_eq!(parent_signatures_before(&parent, early).unwrap().len(), 1);
        let first_timeline = parent_cache_supported(&parent).then(|| {
            Arc::clone(&replay_caches().lock().unwrap().parent_timelines[&parent].timeline)
        });
        assert_eq!(parent_signatures_before(&parent, late).unwrap().len(), 2);

        match first_timeline {
            Some(first_timeline) => {
                assert_eq!(cached_parent_timelines_under(temp.path()), 1);
                let caches = replay_caches().lock().unwrap();
                assert!(Arc::ptr_eq(
                    &first_timeline,
                    &caches.parent_timelines[&parent].timeline
                ));
            }
            None => assert_eq!(cached_parent_timelines_under(temp.path()), 0),
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_rollout_cache_invalidates_after_append() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let cutoff = "2026-07-10T03:00:15Z".parse::<DateTime<Utc>>().unwrap();
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );
        assert_eq!(parent_signatures_before(&parent, cutoff).unwrap().len(), 1);

        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:10Z"),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );
        assert_eq!(parent_signatures_before(&parent, cutoff).unwrap().len(), 2);

        let expected_entries = usize::from(parent_cache_supported(&parent));
        assert_eq!(cached_parent_timelines_under(temp.path()), expected_entries);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_rollout_content_error_cache_preserves_open_errors() {
        clear_codex_replay_caches();
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let cutoff = "2026-07-10T03:00:05Z".parse::<DateTime<Utc>>().unwrap();
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_without_timestamp(100, 50, 10),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );

        let first_error = parent_signatures_before(&parent, cutoff).unwrap_err();
        assert!(first_error
            .reason
            .contains("token_count 缺少有效 timestamp"));
        assert!(first_error.cacheable);
        let cached_timeline =
            || Arc::clone(&replay_caches().lock().unwrap().parent_timelines[&parent].timeline);
        let first_timeline = parent_cache_supported(&parent).then(cached_timeline);

        let second_error = parent_signatures_before(&parent, cutoff).unwrap_err();
        assert_eq!(second_error, first_error);
        if let Some(first_timeline) = first_timeline {
            assert!(Arc::ptr_eq(&first_timeline, &cached_timeline()));
        }

        fs::remove_file(&parent).unwrap();
        let open_error = parent_signatures_before(&parent, cutoff).unwrap_err();
        assert!(open_error.reason.contains("无法打开父 rollout"));
        assert!(!open_error.cacheable);
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_rollout_nanosecond_cutoffs_are_exact() {
        clear_codex_replay_caches();
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:00.000000500Z"),
                turn_context_at("2026-07-10T03:00:00.000000900Z"),
            ],
        );

        let before = "2026-07-10T03:00:00.000000300Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        let after = "2026-07-10T03:00:00.000000700Z"
            .parse::<DateTime<Utc>>()
            .unwrap();
        assert!(parent_signatures_before(&parent, before)
            .unwrap()
            .is_empty());
        assert_eq!(parent_signatures_before(&parent, after).unwrap().len(), 1);
        let expected_entries = usize::from(parent_cache_supported(&parent));
        assert_eq!(cached_parent_timelines_under(temp.path()), expected_entries);
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn test_parent_file_stamp_distinguishes_same_size_same_mtime_files() {
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let replacement = temp.path().join("replacement.jsonl");
        let values = [session_meta(PARENT_ID), token_count(100, 50, 10)];
        write_jsonl(&parent, &values);
        write_jsonl(&replacement, &values);
        if !parent_cache_supported(&parent) {
            return;
        }
        let original_file = fs::File::open(&parent).unwrap();
        let original_metadata = original_file.metadata().unwrap();
        let replacement_file = fs::OpenOptions::new()
            .write(true)
            .open(&replacement)
            .unwrap();
        replacement_file
            .set_times(fs::FileTimes::new().set_modified(original_metadata.modified().unwrap()))
            .unwrap();
        let original_stamp = ParentFileStamp::from_file(&original_file).unwrap();
        let replacement_stamp = ParentFileStamp::from_file(&replacement_file).unwrap();
        assert_eq!(
            (original_stamp.size, original_stamp.modified_nanos),
            (replacement_stamp.size, replacement_stamp.modified_nanos)
        );
        assert_ne!(original_stamp, replacement_stamp);
    }

    #[test]
    #[serial_test::serial]
    fn test_empty_fork_imports_no_parent_usage() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:02Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:07Z"),
                serde_json::json!({
                    "timestamp": "2026-07-10T03:00:08Z",
                    "type": "event_msg",
                    "payload": { "type": "thread_settings_applied" }
                }),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (0, 2, false)
        );
        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_conflicting_explicit_parents_are_deferred() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    Some(CHILD_B_ID),
                    "2026-07-10T03:00:05Z",
                ),
                token_count_at(100, 50, 10, "2026-07-10T03:00:06Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_future_signature_cannot_extend_replay_prefix() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:06Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:05Z"),
                token_count_at(200, 100, 20, "2026-07-10T03:00:07Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (1, 0, false)
        );
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_duplicate_parent_replica_with_missing_timestamp_rejects_both_orders(
    ) -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let sessions_parent = temp
            .path()
            .join("sessions")
            .join(format!("rollout-2026-07-10T03-00-01-{PARENT_ID}.jsonl"));
        let archived_parent = temp
            .path()
            .join("archived_sessions")
            .join(format!("rollout-2026-07-10T03-00-01-{PARENT_ID}.jsonl"));
        fs::create_dir_all(sessions_parent.parent().unwrap()).unwrap();
        fs::create_dir_all(archived_parent.parent().unwrap()).unwrap();

        write_jsonl(
            &archived_parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        write_jsonl(
            &sessions_parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                token_count(500, 400, 50),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );

        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                token_count_at(1_500, 1_000, 200, "2026-07-10T03:00:06Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&archived_parent, &sessions_parent, &child])?;
        assert!(
            result.deferred,
            "任一副本含有无效时间戳时，必须保留无效时间戳保护并 deferred"
        );

        let result_swapped =
            sync_test_file(&db, &child, &[&sessions_parent, &archived_parent, &child])?;
        assert!(
            result_swapped.deferred,
            "候选顺序不应改变无效时间戳的防御结果"
        );

        Ok(())
    }

    /// After a paginated thread was paged, the continuation page is a two segment file name
    /// `<threadId>_<rolloutId>` and
    /// the first page does not grow any more. A sub agent that is spawned afterwards forks later
    /// than the last event of the first page, so the
    /// parent timeline has to look at the first page and the continuation page together, otherwise
    /// the sub agent would be deferred forever.
    #[test]
    #[serial_test::serial]
    fn test_paginated_parent_pages_are_merged_before_fork_cutoff() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent_root = rollout_path(temp.path(), PARENT_ID);
        let parent_page = temp.path().join(format!(
            "rollout-2026-07-10T03-00-20-{PARENT_ID}_{PARENT_PAGE_ID}.jsonl"
        ));
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &parent_root,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
            ],
        );
        write_jsonl(
            &parent_page,
            &[
                session_meta_at(PARENT_ID, None, None, "2026-07-10T03:00:20Z"),
                token_count_at(2_000, 1_800, 200, "2026-07-10T03:00:21Z"),
                turn_context_at("2026-07-10T03:00:40Z"),
            ],
        );
        write_jsonl(
            &child,
            &[
                session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    Some(PARENT_ID),
                    "2026-07-10T03:00:30Z",
                ),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:31Z"),
                token_count_at(2_000, 1_800, 200, "2026-07-10T03:00:32Z"),
                token_count_at(2_300, 1_950, 250, "2026-07-10T03:00:33Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&parent_root, &parent_page, &child])?;
        assert_eq!(
            (result.imported, result.skipped, result.deferred),
            (1, 2, false)
        );

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:3")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (300, 150, 50));
        Ok(())
    }

    /// #8161: the continuation page that the paging of a branch thread produces (the two segment
    /// file name `<branchId>_<pageId>`)
    /// still keeps the forked_from_id of the branch session it belongs to, but the paging time is
    /// later than the end of the original parent thread.
    /// The continuation page has to be recognized as a continuation of the branch thread itself and
    /// the parent replay must not be removed (the new format continuation page does not copy the
    /// usage of the parent),
    /// so that it is not deferred forever because the parent thread cannot reach the paging moment.
    #[test]
    #[serial_test::serial]
    fn test_paginated_continuation_pages_for_forked_branch_thread_are_imported(
    ) -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        // 1. The parent thread ends at 03:00:20
        let parent_root = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &parent_root,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );

        // 2. The first page of the branch thread CHILD_A forks from PARENT_ID at 03:00:15
        // The first page replays the 1000 tokens of the parent thread and newly produces 500 tokens
        let branch_root = temp
            .path()
            .join(format!("rollout-2026-07-10T03-00-15-{CHILD_A_ID}.jsonl"));
        write_jsonl(
            &branch_root,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:15Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:16Z"),
                token_count_at(1_500, 1_300, 150, "2026-07-10T03:00:20Z"),
            ],
        );

        // 3. The branch thread CHILD_A is paged, which produces a two segment continuation page with a
        // timestamp of 03:01:00 (much later than the end of the parent thread at 03:00:10)
        // The session_meta still carries forked_from_id: PARENT_ID, it newly produces 200 tokens
        let branch_page = temp.path().join(format!(
            "rollout-2026-07-10T03-01-00-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_ID),
                    "2026-07-10T03:01:00Z",
                ),
                turn_context_at("2026-07-10T03:01:01Z"),
                token_count_at(200, 150, 50, "2026-07-10T03:01:05Z"),
            ],
        );

        let files = [
            parent_root.as_path(),
            branch_root.as_path(),
            branch_page.as_path(),
        ];

        // Sync the first page of the branch thread: 1 row imported successfully (500 tokens), 1 replay skipped (1000 tokens)
        let root_result = sync_test_file(&db, &branch_root, &files)?;
        assert_eq!(
            (
                root_result.imported,
                root_result.skipped,
                root_result.deferred
            ),
            (1, 1, false)
        );

        // Sync the continuation page of the branch thread: recognized as its own continuation, 1 row imported directly (200 tokens), never deferred!
        let page_result = sync_test_file(&db, &branch_page, &files)?;
        assert_eq!(
            (
                page_result.imported,
                page_result.skipped,
                page_result.deferred
            ),
            (1, 0, false)
        );

        // Verify the database: the session_id is uniformly CHILD_A_ID and the usage of both rows is correct
        let conn = lock_conn!(db.conn);
        let mut stmt = conn.prepare(
            "SELECT request_id, session_id, input_tokens, output_tokens
             FROM proxy_request_logs
             WHERE session_id = ?1
             ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map([CHILD_A_ID], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        assert_eq!(rows.len(), 2);
        // The record of the first page
        assert_eq!(
            rows[0].0,
            format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:2")
        );
        assert_eq!(rows[0].1, CHILD_A_ID);
        assert_eq!(rows[0].2, 500); // 1500 - 1000
        assert_eq!(rows[0].3, 50); // 150 - 100
                                   // The record of the continuation page
        assert_eq!(
            rows[1].0,
            format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_PAGE_ID}:1")
        );
        assert_eq!(rows[1].1, CHILD_A_ID);
        assert_eq!(rows[1].2, 200); // 1700 - 1500
        assert_eq!(rows[1].3, 50); // 200 - 150

        Ok(())
    }

    /// Test the paging of a branch thread several times (the continuation page of the third page,
    /// whose history_base points at the rollout ID of the previous page),
    /// and the complete chain of deriving a sub agent after the branch thread was continued over
    /// several pages.
    #[test]
    #[serial_test::serial]
    fn test_paginated_continuation_multiple_pages_and_subagent_on_branch() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        let parent_root = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &parent_root,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:20Z"),
            ],
        );

        // The first page of the branch thread CHILD_A
        let branch_root = temp
            .path()
            .join(format!("rollout-2026-07-10T03-00-15-{CHILD_A_ID}.jsonl"));
        write_jsonl(
            &branch_root,
            &[
                session_meta_at(CHILD_A_ID, Some(PARENT_ID), None, "2026-07-10T03:00:15Z"),
                turn_context(),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:16Z"),
                token_count_at(1_500, 1_300, 150, "2026-07-10T03:00:20Z"),
            ],
        );

        // The second page of the branch thread (continuation page 1)
        let branch_page_1 = temp.path().join(format!(
            "rollout-2026-07-10T03-01-00-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page_1,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_ID),
                    "2026-07-10T03:01:00Z",
                ),
                turn_context_at("2026-07-10T03:01:01Z"),
                token_count_at(1_700, 1_450, 200, "2026-07-10T03:01:05Z"),
            ],
        );

        // The third page of the branch thread (continuation page 2), the history_base points at the rollout ID of the second page
        let branch_page_2 = temp.path().join(format!(
            "rollout-2026-07-10T03-02-00-{CHILD_A_ID}_{CHILD_A_PAGE_2_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page_2,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_PAGE_ID),
                    "2026-07-10T03:02:00Z",
                ),
                turn_context_at("2026-07-10T03:02:01Z"),
                token_count_at(2_000, 1_650, 250, "2026-07-10T03:02:10Z"),
                turn_context_at("2026-07-10T03:02:30Z"),
            ],
        );

        // After the branch thread finished its paging, a sub agent CHILD_B is derived from the branch thread (fork moment 03:02:30)
        let subagent = temp
            .path()
            .join(format!("rollout-2026-07-10T03-02-30-{CHILD_B_ID}.jsonl"));
        write_jsonl(
            &subagent,
            &[
                session_meta_at(
                    CHILD_B_ID,
                    Some(CHILD_A_ID),
                    Some(CHILD_A_ID),
                    "2026-07-10T03:02:30Z",
                ),
                turn_context(),
                // Replay the tokens of CHILD_A from the beginning to the end (including the several pages)
                token_count_at(1_000, 900, 100, "2026-07-10T03:02:31Z"),
                token_count_at(1_500, 1_300, 150, "2026-07-10T03:02:32Z"),
                token_count_at(1_700, 1_450, 200, "2026-07-10T03:02:33Z"),
                token_count_at(2_000, 1_650, 250, "2026-07-10T03:02:34Z"),
                // The sub agent consumed 150 tokens itself
                token_count_at(2_150, 1_750, 280, "2026-07-10T03:02:40Z"),
            ],
        );

        let files = [
            parent_root.as_path(),
            branch_root.as_path(),
            branch_page_1.as_path(),
            branch_page_2.as_path(),
            subagent.as_path(),
        ];

        // First sync the first page of the branch thread: 500 tokens imported successfully (the 1000 of the parent replay skipped)
        let r_res = sync_test_file(&db, &branch_root, &files)?;
        assert_eq!(
            (r_res.imported, r_res.skipped, r_res.deferred),
            (1, 1, false)
        );

        // Verify that continuation page 1 was imported successfully: the baseline inherits the 1500 of the first page, the delta is only 200 (1700 - 1500)
        let p1_res = sync_test_file(&db, &branch_page_1, &files)?;
        assert_eq!((p1_res.imported, p1_res.deferred), (1, false));

        // Verify that continuation page 2 was imported successfully: the baseline inherits the 1700 of the second page, the delta is only 300 (2000 - 1700)
        let p2_res = sync_test_file(&db, &branch_page_2, &files)?;
        assert_eq!((p2_res.imported, p2_res.deferred), (1, false));

        // Verify that the child agent can merge the first page of CHILD_A and its two continuation pages
        // when it parses the timeline of the parent thread CHILD_A,
        // that it reaches 03:02:30 successfully, removes all 4 replays and imports the one usage row
        // of the sub agent itself (150 tokens)
        let sub_res = sync_test_file(&db, &subagent, &files)?;
        assert_eq!(
            (sub_res.imported, sub_res.skipped, sub_res.deferred),
            (1, 4, false)
        );

        let conn = lock_conn!(db.conn);
        // Assert the usage of continuation page 1 exactly (so that the 1700 of the accumulated history is not counted as the delta)
        let p1_usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!(
                "{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_PAGE_ID}:1"
            )],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(p1_usage, (200, 150, 50));

        // Assert the usage of continuation page 2 exactly (so that the 2000 of the accumulated history is not counted as the delta)
        let p2_usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!(
                "{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_PAGE_2_ID}:1"
            )],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(p2_usage, (300, 200, 50));

        // Assert the usage of the sub agent
        let sub_usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_B_ID}:5")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(sub_usage, (150, 100, 30));

        // Assert the total input usage of the whole life cycle of the branch thread CHILD_A exactly:
        // the first page 500 + the second page 200 + the third page 300 = 1000 tokens
        let branch_total_input: i64 = conn.query_row(
            "SELECT sum(input_tokens) FROM proxy_request_logs WHERE session_id = ?1",
            [CHILD_A_ID],
            |row| row.get(0),
        )?;
        assert_eq!(branch_total_input, 1_000);

        Ok(())
    }

    /// Make sure it stays defensive: a new fork first page with a single segment file name is not
    /// misjudged as a continuation page even when its history_base points at a page of the parent
    /// session, and it still has to be deferred when the parent thread has not reached the cutoff.
    #[test]
    #[serial_test::serial]
    fn test_single_segment_fork_is_not_treated_as_continuation() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        let parent_root = rollout_path(temp.path(), PARENT_ID);
        write_jsonl(
            &parent_root,
            &[
                session_meta(PARENT_ID),
                token_count_at(1_000, 900, 100, "2026-07-10T03:00:01Z"),
                // The parent thread ends at 03:00:05
                turn_context_at("2026-07-10T03:00:05Z"),
            ],
        );

        // The fork moment of the first page of the child session is 03:00:20 (later than the end of the
        // parent thread at 03:00:05)
        // A single segment file name, the history_base points at PARENT_ID (a page of the parent session)
        let child = temp
            .path()
            .join(format!("rollout-2026-07-10T03-00-20-{CHILD_A_ID}.jsonl"));
        write_jsonl(
            &child,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(PARENT_ID),
                    "2026-07-10T03:00:20Z",
                ),
                turn_context_at("2026-07-10T03:00:21Z"),
                token_count_at(1_200, 1_000, 120, "2026-07-10T03:00:25Z"),
            ],
        );

        let files = [parent_root.as_path(), child.as_path()];
        let result = sync_test_file(&db, &child, &files)?;
        assert!(
            result.deferred,
            "单段文件名的新 fork 首页在父线程未达到 cutoff 时必须被 deferred"
        );

        Ok(())
    }

    /// The control test 1: a continuation page with a valid last_token_usage bills the last directly, no need to trace back the baseline.
    #[test]
    #[serial_test::serial]
    fn test_paginated_continuation_with_valid_last_token_usage() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        let branch_page = temp.path().join(format!(
            "rollout-2026-07-10T03-01-00-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_ID),
                    "2026-07-10T03:01:00Z",
                ),
                turn_context_at("2026-07-10T03:01:01Z"),
                // With an explicit last_token_usage
                token_count_with_last_at(
                    1_700,
                    1_450,
                    200,
                    120,
                    80,
                    30,
                    "codex",
                    "2026-07-10T03:01:05Z",
                ),
            ],
        );

        let files = [branch_page.as_path()];
        let result = sync_test_file(&db, &branch_page, &files)?;
        assert_eq!((result.imported, result.deferred), (1, false));

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!(
                "{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_PAGE_ID}:1"
            )],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (120, 80, 30));
        Ok(())
    }

    /// The control test 2: the defensive mechanism -- when a continuation page lacks a
    /// last_token_usage and the previous segment is missing so the baseline cannot be restored,
    /// it has to be deferred safely and the accumulated total must never be stored as the delta.
    #[test]
    #[serial_test::serial]
    fn test_paginated_continuation_deferred_when_prev_segment_missing() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        // Only the continuation page exists, the file of its previous first page is missing
        let branch_page = temp.path().join(format!(
            "rollout-2026-07-10T03-01-00-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    Some(PARENT_ID),
                    None,
                    Some(CHILD_A_ID),
                    "2026-07-10T03:01:00Z",
                ),
                turn_context_at("2026-07-10T03:01:01Z"),
                token_count_at(1_700, 1_450, 200, "2026-07-10T03:01:05Z"),
            ],
        );

        let files = [branch_page.as_path()];
        let result = sync_test_file(&db, &branch_page, &files)?;
        assert!(
            result.deferred,
            "前序分段缺失且无 last_token_usage 时必须 deferred"
        );

        // Verify that nothing was written into the database by mistake
        let count: i64 = {
            let conn = lock_conn!(db.conn);
            conn.query_row(
                "SELECT count(*) FROM proxy_request_logs WHERE session_id = ?1",
                [CHILD_A_ID],
                |row| row.get(0),
            )?
        };
        assert_eq!(count, 0, "缺失前序基线时绝不应有数据入库");

        // Verify that the cursor did not advance
        let sync_state = get_sync_state(&db, &branch_page.to_string_lossy())?;
        assert_eq!(sync_state, (0, 0), "deferred 时游标不应推进");
        Ok(())
    }

    /// The control test 3: the scenario where the counter of the continuation page was reset -- the
    /// total_token_usage of the new page starts at a smaller value,
    /// it is recognized automatically as a counter reset, the current value is billed as the delta and
    /// it is not swallowed to 0 by the saturating subtraction.
    #[test]
    #[serial_test::serial]
    fn test_paginated_continuation_with_counter_reset() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();

        // The first page ends at an accumulated 1500
        let branch_root = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &branch_root,
            &[
                session_meta(CHILD_A_ID),
                token_count_at(1_500, 1_300, 150, "2026-07-10T03:00:20Z"),
            ],
        );

        // The counter of the continuation page is reset to 180 (180 < 1500)
        let branch_page = temp.path().join(format!(
            "rollout-2026-07-10T03-01-00-{CHILD_A_ID}_{CHILD_A_PAGE_ID}.jsonl"
        ));
        write_jsonl(
            &branch_page,
            &[
                paginated_session_meta_at(
                    CHILD_A_ID,
                    None,
                    None,
                    Some(CHILD_A_ID),
                    "2026-07-10T03:01:00Z",
                ),
                turn_context_at("2026-07-10T03:01:01Z"),
                token_count_at(180, 100, 25, "2026-07-10T03:01:05Z"),
            ],
        );

        let files = [branch_root.as_path(), branch_page.as_path()];
        let root_res = sync_test_file(&db, &branch_root, &files)?;
        assert_eq!((root_res.imported, root_res.deferred), (1, false));

        let page_res = sync_test_file(&db, &branch_page, &files)?;
        assert_eq!((page_res.imported, page_res.deferred), (1, false));

        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs WHERE request_id = ?1",
            [format!(
                "{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_PAGE_ID}:1"
            )],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (180, 100, 25), "计数器重置时应以新页数值作为增量");
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_missing_parent_is_deferred_and_recovered_without_child_change() -> Result<(), AppError>
    {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let parent = rollout_path(temp.path(), PARENT_ID);
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                session_meta_at(CHILD_A_ID, None, Some(PARENT_ID), "2026-07-10T03:00:05Z"),
                token_count_at(900, 400, 90, "2026-07-10T03:00:06Z"),
            ],
        );

        let deferred = sync_test_file(&db, &child, &[&child])?;
        assert!(deferred.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        write_jsonl(
            &parent,
            &[
                session_meta(PARENT_ID),
                token_count_at(100, 50, 10, "2026-07-10T03:00:01Z"),
                turn_context_at("2026-07-10T03:00:10Z"),
            ],
        );
        let recovered = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!((recovered.imported, recovered.deferred), (1, false));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_skips_child_body_until_parent_append() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        let cached = parent_cache_supported(&parent);

        for _ in 0..4 {
            assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        }
        if cached {
            assert_eq!(codex_parse_count(&child), 1);
        }
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        append_jsonl(&parent, turn_context_at("2026-07-10T03:00:10Z"));
        let recovered = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (1, 1, false)
        );
        if cached {
            assert_eq!(codex_parse_count(&child), 2);
        }
        assert_eq!(sync_test_file(&db, &child, &[&parent, &child])?.imported, 0);
        if cached {
            assert_eq!(codex_parse_count(&child), 2);
        }
        let conn = lock_conn!(db.conn);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens FROM proxy_request_logs",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (300, 150, 50));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_invalidates_when_child_changes() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        let cached = parent_cache_supported(&parent);
        assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        append_jsonl(
            &child,
            token_count_at(1_600, 1_200, 200, "2026-07-10T03:00:08Z"),
        );
        for _ in 0..3 {
            assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        }
        if cached {
            assert_eq!(codex_parse_count(&child), 2);
        }
        append_jsonl(&parent, turn_context_at("2026-07-10T03:00:10Z"));
        let recovered = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (2, 1, false)
        );
        if cached {
            assert_eq!(codex_parse_count(&child), 3);
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_tracks_added_and_removed_parent_candidates() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        let cached = parent_cache_supported(&parent);
        assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        let archived_dir = temp.path().join("archived_sessions");
        fs::create_dir(&archived_dir).unwrap();
        let archived_parent = rollout_path(&archived_dir, PARENT_ID);
        fs::copy(&parent, &archived_parent).unwrap();
        append_jsonl(&archived_parent, turn_context_at("2026-07-10T03:00:10Z"));
        assert!(sync_test_file(&db, &child, &[&parent, &archived_parent, &child])?.deferred);
        if cached {
            assert_eq!(codex_parse_count(&child), 2, "new candidate must retry");
        }
        assert!(sync_test_file(&db, &child, &[&parent, &archived_parent, &child])?.deferred);
        if cached {
            assert_eq!(codex_parse_count(&child), 2);
        }
        fs::remove_file(&parent).unwrap();
        let recovered = sync_test_file(&db, &child, &[&archived_parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (1, 1, false)
        );
        if cached {
            assert_eq!(codex_parse_count(&child), 3, "removed candidate must retry");
        }
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_does_not_cache_unknown_parent_metadata() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        let parent_contents = fs::read(&parent).unwrap();
        fs::remove_file(&parent).unwrap();
        // The index can outlive a file removed during a pass. An inaccessible
        // candidate must retry, rather than cache an unknown stamp forever.
        for _ in 0..2 {
            assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        }
        assert_eq!(codex_parse_count(&child), 2);
        fs::write(&parent, parent_contents).unwrap();
        append_jsonl(&parent, turn_context_at("2026-07-10T03:00:10Z"));
        assert!(!sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        assert_eq!(codex_parse_count(&child), 3);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_detects_same_size_same_mtime_parent_replacement(
    ) -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        if !parent_cache_supported(&parent) {
            return Ok(());
        }
        assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        let metadata = fs::metadata(&parent).unwrap();
        let replacement = temp.path().join("replacement.jsonl");
        let contents = fs::read_to_string(&parent)
            .unwrap()
            .replace("03:00:02Z", "03:00:10Z");
        fs::write(&replacement, contents).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        fs::rename(&replacement, &parent).unwrap();
        let replacement_metadata = fs::metadata(&parent).unwrap();
        assert_eq!(metadata.len(), replacement_metadata.len());
        assert_eq!(
            metadata.modified().unwrap(),
            replacement_metadata.modified().unwrap()
        );
        let recovered = sync_test_file(&db, &child, &[&parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (1, 1, false)
        );
        assert_eq!(codex_parse_count(&child), 2);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_retries_parent_decompression_errors() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        let compressed_parent = PathBuf::from(format!("{}.zst", parent.display()));
        fs::remove_file(&parent).unwrap();
        fs::write(&compressed_parent, b"invalid zstd bytes").unwrap();
        // Opening/statting this file succeeds; decoding its body fails. That
        // transient failure must not be suppressed by an unchanged fingerprint.
        for _ in 0..2 {
            assert!(sync_test_file(&db, &child, &[&compressed_parent, &child])?.deferred);
        }
        assert_eq!(codex_parse_count(&child), 2);
        assert!(!replay_caches()
            .lock()
            .unwrap()
            .parent_timelines
            .contains_key(&compressed_parent));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_parent_decompression_error_after_valid_prefix_does_not_cache_partial_timeline(
    ) -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        append_jsonl(&parent, turn_context_at("2026-07-10T03:00:10Z"));
        let mut contents = fs::read(&parent).unwrap();
        // Fill the rollout reader's buffer before it reaches a corrupt second
        // frame, so valid token events are read before decompression fails.
        contents.extend(vec![b'\n'; 2 << 20]);
        fs::write(&parent, contents).unwrap();
        let compressed_parent = compress_rollout(&parent);
        let valid_frame = fs::read(&compressed_parent).unwrap();
        let mut corrupt_frames = valid_frame.clone();
        corrupt_frames.extend_from_slice(b"invalid zstd bytes");
        fs::write(&compressed_parent, corrupt_frames).unwrap();
        let mut reader = codex_rollout_file::open_reader(&compressed_parent).unwrap();
        let mut first_line = Vec::new();
        assert!(reader.read_until(b'\n', &mut first_line).unwrap() > 0);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&first_line).unwrap(),
            session_meta(PARENT_ID)
        );
        drop(reader);

        for _ in 0..2 {
            let error = load_parent_timeline(&compressed_parent).unwrap_err();
            assert!(!error.cacheable);
            assert!(error.reason.contains("无法读取父 rollout"));
            assert!(sync_test_file(&db, &child, &[&compressed_parent, &child])?.deferred);
            assert!(!replay_caches()
                .lock()
                .unwrap()
                .parent_timelines
                .contains_key(&compressed_parent));
        }
        assert_eq!(codex_parse_count(&child), 2);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        fs::write(&compressed_parent, valid_frame).unwrap();
        let recovered = sync_test_file(&db, &child, &[&compressed_parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (1, 1, false)
        );
        assert_eq!(codex_parse_count(&child), 3);
        assert_eq!(
            sync_test_file(&db, &child, &[&compressed_parent, &child])?.imported,
            0
        );
        assert_eq!(codex_parse_count(&child), 3);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_retryable_pending_detects_same_size_same_mtime_child_replacement(
    ) -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let (parent, child) = retryable_parent_and_child(temp.path());
        if !parent_cache_supported(&child) {
            return Ok(());
        }
        assert!(sync_test_file(&db, &child, &[&parent, &child])?.deferred);
        let alternate_parent = rollout_path(temp.path(), CHILD_B_ID);
        fs::write(
            &alternate_parent,
            fs::read_to_string(&parent)
                .unwrap()
                .replace(PARENT_ID, CHILD_B_ID),
        )
        .unwrap();
        append_jsonl(&alternate_parent, turn_context_at("2026-07-10T03:00:10Z"));

        let metadata = fs::metadata(&child).unwrap();
        let replacement = temp.path().join("replacement.jsonl");
        fs::write(
            &replacement,
            fs::read_to_string(&child)
                .unwrap()
                .replace(PARENT_ID, CHILD_B_ID),
        )
        .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
            .unwrap();
        fs::rename(&replacement, &child).unwrap();
        let replacement_metadata = fs::metadata(&child).unwrap();
        assert_eq!(metadata.len(), replacement_metadata.len());
        assert_eq!(
            metadata.modified().unwrap(),
            replacement_metadata.modified().unwrap()
        );
        let recovered = sync_test_file(&db, &child, &[&parent, &alternate_parent, &child])?;
        assert_eq!(
            (recovered.imported, recovered.skipped, recovered.deferred),
            (1, 1, false)
        );
        assert_eq!(codex_parse_count(&child), 2);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_billable_file_without_meta_is_deferred_without_cursor() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(&child, &[turn_context(), token_count(100, 50, 10)]);

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?, (0, 0));

        std::thread::sleep(std::time::Duration::from_millis(2));
        write_jsonl(
            &child,
            &[
                turn_context(),
                token_count(100, 50, 10),
                session_meta_at(CHILD_A_ID, None, None, "2026-07-10T03:00:03Z"),
            ],
        );
        let recovered = sync_test_file(&db, &child, &[&child])?;
        assert_eq!((recovered.imported, recovered.deferred), (1, false));
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_non_billable_file_without_meta_advances_cursor() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child = rollout_path(temp.path(), CHILD_A_ID);
        write_jsonl(
            &child,
            &[
                turn_context(),
                token_count_at(0, 0, 0, "2026-07-10T03:00:02Z"),
            ],
        );

        let result = sync_test_file(&db, &child, &[&child])?;
        assert!(!result.deferred);
        assert_eq!(get_sync_state(&db, &child.to_string_lossy())?.1, 2);
        Ok(())
    }

    #[test]
    #[serial_test::serial]
    fn test_subagents_use_filename_thread_ids() -> Result<(), AppError> {
        clear_codex_replay_caches();
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let child_a = rollout_path(temp.path(), CHILD_A_ID);
        let child_b = rollout_path(temp.path(), CHILD_B_ID);
        write_jsonl(
            &child_a,
            &[
                session_meta(CHILD_A_ID),
                turn_context(),
                token_count(100, 50, 10),
            ],
        );
        write_jsonl(
            &child_b,
            &[
                session_meta(CHILD_B_ID),
                turn_context(),
                token_count(200, 100, 20),
            ],
        );

        assert_eq!(
            sync_test_file(&db, &child_a, &[&child_a, &child_b])?.imported,
            1
        );
        assert_eq!(
            sync_test_file(&db, &child_b, &[&child_a, &child_b])?.imported,
            1
        );

        let conn = lock_conn!(db.conn);
        let request_ids = conn
            .prepare(
                "SELECT request_id FROM proxy_request_logs
                 WHERE data_source = 'codex_session' ORDER BY request_id",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(
            request_ids,
            vec![
                format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_A_ID}:1"),
                format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{CHILD_B_ID}:1")
            ]
        );
        Ok(())
    }

    #[test]
    fn test_archived_log_inherits_cursor_and_only_imports_appended_usage() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        let archived = temp.path().join("archived_sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(&archived).unwrap();
        let source = rollout_path(&sessions, PARENT_ID);
        let archived_file = rollout_path(&archived, PARENT_ID);
        write_jsonl(
            &archived_file,
            &[
                session_meta(PARENT_ID),
                turn_context(),
                token_count(100, 50, 10),
                token_count(200, 100, 20),
            ],
        );

        {
            let conn = lock_conn!(db.conn);
            conn.execute(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, request_model,
                    input_tokens, output_tokens, cache_read_tokens,
                    total_cost_usd, latency_ms, status_code, session_id,
                    created_at, data_source
                ) VALUES ('codex_session:parent:2', '_codex_session', 'codex',
                          'gpt-5.6-sol', 'gpt-5.6-sol', 999, 99, 0, '0', 0,
                          200, 'parent', 1, 'codex_session')",
                [],
            )?;
        }
        let source_path = source.to_string_lossy().to_string();
        update_sync_state(&db, &source_path, 1, 3)?;

        assert_eq!(
            sync_test_file(&db, &archived_file, &[&archived_file])?.imported,
            1
        );
        assert_eq!(
            sync_test_file(&db, &archived_file, &[&archived_file])?.imported,
            0
        );

        let conn = lock_conn!(db.conn);
        let old_row_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs
             WHERE request_id = 'codex_session:parent:2'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(old_row_count, 1);
        let usage: (i64, i64, i64) = conn.query_row(
            "SELECT input_tokens, cache_read_tokens, output_tokens
             FROM proxy_request_logs
             WHERE request_id = ?1",
            [format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{PARENT_ID}:2")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(usage, (100, 50, 10));
        drop(conn);
        assert_eq!(get_sync_state(&db, &archived_file.to_string_lossy())?.1, 4);

        Ok(())
    }

    fn compress_rollout(plain: &Path) -> PathBuf {
        let compressed = PathBuf::from(format!("{}.zst", plain.display()));
        let bytes = fs::read(plain).unwrap();
        fs::write(
            &compressed,
            zstd::stream::encode_all(bytes.as_slice(), 3).unwrap(),
        )
        .unwrap();
        fs::remove_file(plain).unwrap();
        compressed
    }

    fn decompress_rollout(compressed: &Path) -> PathBuf {
        let plain = codex_rollout_file::logical_path(compressed);
        let text = codex_rollout_file::read_to_string(compressed).unwrap();
        fs::write(&plain, text).unwrap();
        fs::remove_file(compressed).unwrap();
        plain
    }

    fn prune_detail_rows(db: &Database) -> Result<(), AppError> {
        let conn = lock_conn!(db.conn);
        conn.execute("DELETE FROM proxy_request_logs", [])?;
        Ok(())
    }

    /// Codex compresses a cold rollout and it is decompressed again when the conversation is
    /// resumed: both path changes are guarded against a double count by the cursor inheritance,
    /// the detail records are deleted in between (to simulate the 30 day pruning) and the
    /// request_id deduplication cannot hold.
    #[test]
    fn test_compressed_rollout_round_trip_never_double_counts() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let sessions = temp.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        let plain = rollout_path(&sessions, PARENT_ID);
        let mut lines = vec![
            session_meta(PARENT_ID),
            turn_context(),
            token_count(100, 50, 10),
        ];
        write_jsonl(&plain, &lines);
        assert_eq!(sync_test_file(&db, &plain, &[&plain])?.imported, 1);

        // CC Switch did not catch up with the second usage and the file was compressed
        lines.push(token_count(200, 100, 20));
        write_jsonl(&plain, &lines);
        let compressed = compress_rollout(&plain);
        prune_detail_rows(&db)?;
        assert_eq!(
            sync_test_file(&db, &compressed, &[&compressed])?.imported,
            1
        );
        assert_unchanged_codex_file_is_skipped(&db, &compressed)?;

        // Resume the conversation: decompress it back to the plain shape and append; the old cursor of the plain shape is behind the compressed shape
        prune_detail_rows(&db)?;
        let plain = decompress_rollout(&compressed);
        lines.push(token_count(300, 150, 30));
        write_jsonl(&plain, &lines);
        assert_eq!(sync_test_file(&db, &plain, &[&plain])?.imported, 1);

        let conn = lock_conn!(db.conn);
        let request_ids: Vec<String> = conn
            .prepare("SELECT request_id FROM proxy_request_logs")?
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            request_ids,
            vec![format!("{CODEX_THREAD_REQUEST_ID_PREFIX}:{PARENT_ID}:3")]
        );
        drop(conn);
        assert_eq!(get_sync_state(&db, &plain.to_string_lossy())?.1, 5);
        Ok(())
    }

    #[test]
    fn test_compressed_rollout_filename_parsing() {
        let plain = PathBuf::from(format!("/s/rollout-2026-07-10T03-00-00-{PARENT_ID}.jsonl"));
        let compressed = PathBuf::from(format!("{}.zst", plain.display()));
        assert_eq!(
            thread_id_from_filename(&compressed),
            thread_id_from_filename(&plain)
        );
        assert!(thread_id_from_filename(&compressed).is_some());
        assert!(is_rollout_filename(
            compressed.file_name().unwrap().to_str().unwrap()
        ));

        let two_segment = PathBuf::from(format!(
            "/s/rollout-2026-08-26T17-18-13-{PARENT_ID}_{CHILD_A_ID}.jsonl.zst"
        ));
        assert_eq!(
            leading_thread_id_from_filename(&two_segment).as_deref(),
            Some(PARENT_ID)
        );
        assert_eq!(
            thread_id_from_filename(&two_segment).as_deref(),
            Some(CHILD_A_ID)
        );
    }

    #[test]
    fn test_insert_codex_session_skips_matching_proxy_log() -> Result<(), AppError> {
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
                    "codex-proxy",
                    "openai",
                    "codex",
                    "gpt-5.4",
                    "gpt-5.4",
                    10,
                    2,
                    1,
                    7,
                    "0.01",
                    100,
                    200,
                    1000,
                    "proxy"
                ],
            )?;
        }

        // Older rollouts carry no cache-write count; newer ones report the same
        // count the proxy logged. Either way the session row is the proxied request.
        for (request_id, cache_write_input) in
            [("codex-session-dup", 0), ("codex-session-dup-write", 7)]
        {
            let delta = DeltaTokens {
                input: 10,
                cached_input: 1,
                cache_write_input,
                output: 2,
            };
            let mut suspected_duplicates = 0;
            let inserted = insert_codex_session_entry(
                &db,
                request_id,
                &delta,
                "gpt-5.4",
                Some("session-1"),
                Some("1970-01-01T00:16:45Z"),
                &mut suspected_duplicates,
            )?;
            assert!(!inserted, "cache_write_input={cache_write_input}");
        }

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM proxy_request_logs", [], |row| {
            row.get(0)
        })?;
        assert_eq!(count, 1);

        Ok(())
    }

    #[test]
    fn test_codex_session_cache_writes_dedup_against_legacy_proxy_log() -> Result<(), AppError> {
        // The agents before v3.17.0 do not record the Codex cache writes (the written amount of a
        // legacy semantics row is always 0).
        // A session row with a write amount is still the same request during a rebuild; the rows of
        // the new semantics keep the exact match.
        let db = Database::memory()?;
        {
            let conn = lock_conn!(db.conn);
            for (request_id, created_at, semantics) in [
                ("legacy-proxy", 1000, INPUT_TOKEN_SEMANTICS_LEGACY),
                ("total-proxy", 5000, INPUT_TOKEN_SEMANTICS_TOTAL),
            ] {
                conn.execute(
                    "INSERT INTO proxy_request_logs (
                        request_id, provider_id, app_type, model, request_model,
                        input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                        total_cost_usd, latency_ms, status_code, created_at, data_source,
                        input_token_semantics
                    ) VALUES (?1, 'openai', 'codex', 'gpt-5.6', 'gpt-5.6', 1000, 50, 300, 0,
                              '0.01', 100, 200, ?2, 'proxy', ?3)",
                    rusqlite::params![request_id, created_at, semantics],
                )?;
            }
        }

        let delta = DeltaTokens {
            input: 1000,
            cached_input: 300,
            cache_write_input: 600,
            output: 50,
        };
        let mut suspected_duplicates = 0;
        for (request_id, timestamp, expect_inserted) in [
            ("session-legacy", "1970-01-01T00:16:45Z", false),
            ("session-total", "1970-01-01T01:23:25Z", true),
        ] {
            let inserted = insert_codex_session_entry(
                &db,
                request_id,
                &delta,
                "gpt-5.6",
                Some("session-1"),
                Some(timestamp),
                &mut suspected_duplicates,
            )?;
            assert_eq!(inserted, expect_inserted, "{request_id}");
        }

        Ok(())
    }

    #[test]
    fn test_codex_session_duplicate_is_observed_but_still_inserted() -> Result<(), AppError> {
        let db = Database::memory()?;
        let delta = DeltaTokens {
            input: 10,
            cached_input: 1,
            cache_write_input: 0,
            output: 2,
        };
        let mut suspected_duplicates = 0;
        assert!(insert_codex_session_entry(
            &db,
            "codex-session-a",
            &delta,
            "gpt-5.4",
            Some("session-a"),
            Some("1970-01-01T00:16:40Z"),
            &mut suspected_duplicates,
        )?);
        assert!(insert_codex_session_entry(
            &db,
            "codex-session-b",
            &delta,
            "gpt-5.4",
            Some("session-b"),
            Some("1970-01-01T00:16:45Z"),
            &mut suspected_duplicates,
        )?);
        assert_eq!(suspected_duplicates, 1);

        let conn = lock_conn!(db.conn);
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 2);
        Ok(())
    }

    #[test]
    fn reset_codex_usage_only_removes_codex_rows_and_structural_cursors() -> Result<(), AppError> {
        let db = Database::memory()?;
        let temp = tempdir().unwrap();
        let wide_dir = temp.path();
        let current_codex = rollout_path(&wide_dir.join("sessions"), CHILD_A_ID);
        let legacy_codex =
            format!("C:\\old-codex\\archived_sessions\\rollout-old-{CHILD_B_ID}.jsonl");
        let gemini_cursor = wide_dir.join("gemini/sessions/session-123.json");
        let claude_cursor = wide_dir.join(format!("projects/rollout-{PARENT_ID}.jsonl"));

        {
            let conn = lock_conn!(db.conn);
            conn.execute_batch(
                "INSERT INTO proxy_request_logs (
                    request_id, provider_id, app_type, model, input_tokens,
                    output_tokens, cache_read_tokens, latency_ms, status_code,
                    created_at, data_source
                 ) VALUES
                    ('codex-row', '_codex_session', 'codex', 'gpt', 1, 1, 0, 0, 200, 1, 'codex_session'),
                    ('gemini-row', '_gemini_session', 'gemini', 'gemini', 1, 1, 0, 0, 200, 1, 'gemini_session');
                 INSERT INTO usage_daily_rollups (date, app_type, provider_id, model)
                 VALUES
                    ('2026-07-10', 'codex', '_codex_session', 'gpt'),
                    ('2026-07-10', 'gemini', '_gemini_session', 'gemini');",
            )?;
            for path in [
                current_codex.to_string_lossy().to_string(),
                legacy_codex,
                gemini_cursor.to_string_lossy().to_string(),
                claude_cursor.to_string_lossy().to_string(),
            ] {
                conn.execute(
                    "INSERT INTO session_log_sync
                     (file_path, last_modified, last_line_offset, last_synced_at)
                     VALUES (?1, 1, 1, 1)",
                    [path],
                )?;
            }

            reset_codex_usage_on_conn(&conn, wide_dir)?;
            let codex_rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'codex_session'",
                [],
                |row| row.get(0),
            )?;
            let gemini_rows: i64 = conn.query_row(
                "SELECT COUNT(*) FROM proxy_request_logs WHERE data_source = 'gemini_session'",
                [],
                |row| row.get(0),
            )?;
            let codex_rollups: i64 = conn.query_row(
                "SELECT COUNT(*) FROM usage_daily_rollups WHERE provider_id = '_codex_session'",
                [],
                |row| row.get(0),
            )?;
            let remaining_cursors: i64 =
                conn.query_row("SELECT COUNT(*) FROM session_log_sync", [], |row| {
                    row.get(0)
                })?;
            assert_eq!((codex_rows, gemini_rows, codex_rollups), (0, 1, 0));
            assert_eq!(remaining_cursors, 2);
        }
        Ok(())
    }

    // ── Model name normalization tests ──

    #[test]
    fn test_normalize_codex_model_lowercase() {
        assert_eq!(normalize_codex_model("GLM-4.6"), "glm-4.6");
        assert_eq!(normalize_codex_model("DeepSeek-Chat"), "deepseek-chat");
        assert_eq!(normalize_codex_model("GPT-5.4"), "gpt-5.4");
    }

    #[test]
    fn test_normalize_codex_model_strip_prefix() {
        assert_eq!(normalize_codex_model("openai/gpt-5.4"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("azure/gpt-5.2-codex"),
            "gpt-5.2-codex"
        );
        assert_eq!(normalize_codex_model("OPENAI/GPT-5.4"), "gpt-5.4");
    }

    #[test]
    fn test_normalize_codex_model_strip_iso_date() {
        assert_eq!(normalize_codex_model("gpt-5.4-2026-03-05"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("gpt-5.4-pro-2026-03-05"),
            "gpt-5.4-pro"
        );
    }

    #[test]
    fn test_normalize_codex_model_strip_compact_date() {
        assert_eq!(normalize_codex_model("gpt-5.4-20260305"), "gpt-5.4");
        assert_eq!(
            normalize_codex_model("claude-opus-4-6-20260206"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn test_normalize_codex_model_no_change() {
        assert_eq!(normalize_codex_model("gpt-5.4"), "gpt-5.4");
        assert_eq!(normalize_codex_model("gpt-5.2-codex"), "gpt-5.2-codex");
        assert_eq!(normalize_codex_model("o3"), "o3");
        assert_eq!(normalize_codex_model("deepseek-chat"), "deepseek-chat");
    }

    #[test]
    fn test_normalize_codex_model_combined() {
        // prefix + uppercase + ISO date
        assert_eq!(
            normalize_codex_model("openai/GPT-5.4-2026-03-05"),
            "gpt-5.4"
        );
        // prefix + compact date
        assert_eq!(normalize_codex_model("openai/gpt-5.4-20260305"), "gpt-5.4");
    }

    #[test]
    fn test_cached_clamped_to_input() {
        // The exceptional case where cached > input has to be clamped by min()
        let prev = Some(CumulativeTokens {
            input: 100,
            cached_input: 0,
            cache_write_input: 0,
            output: 50,
        });
        let current = CumulativeTokens {
            input: 110,       // delta = 10
            cached_input: 80, // delta = 80 (exceptional: larger than the input delta)
            cache_write_input: 0,
            output: 60,
        };
        let delta = compute_delta(&prev, &current);
        // Before the clamping: cached_input = 80, input = 10
        assert_eq!(delta.cached_input, 80);
        assert_eq!(delta.input, 10);
        // The clamping actually happens on the call side: delta.cached_input.min(delta.input)
        let clamped = delta.cached_input.min(delta.input);
        assert_eq!(clamped, 10);
    }

    /// The acceptance harness that replays a real corpus (only run manually, never in CI).
    ///
    /// It does a full re-import of the real `~/.codex/sessions` corpus into an in-memory database
    /// and writes the timings and a
    /// result snapshot. It is used to verify the behavioral equivalence of a performance change:
    /// run it once before and once after the change, the
    /// `CODEX_REPLAY_OUT` files of both sides have to be byte identical.
    ///
    /// ```bash
    /// CODEX_REPLAY_OUT=/tmp/replay.tsv \
    ///   cargo test --release replay_real_codex_corpus -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore]
    fn replay_real_codex_corpus() -> Result<(), AppError> {
        let Some(real_home) = dirs::home_dir() else {
            eprintln!("[REPLAY] no home dir, skipping");
            return Ok(());
        };
        let real_sessions = real_home.join(".codex").join("sessions");
        if !real_sessions.is_dir() {
            eprintln!("[REPLAY] {} not found, skipping", real_sessions.display());
            return Ok(());
        }

        // The temp HOME only holds a read-only symlink to the real corpus, so that the test
        // does not touch any other content of the real ~/.cc-switch / ~/.codex.
        let temp = tempfile::tempdir().expect("create temp home");
        fs::create_dir_all(temp.path().join(".codex")).expect("mkdir .codex");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real_sessions, temp.path().join(".codex").join("sessions"))
            .expect("symlink sessions");
        let previous_home = std::env::var_os("OBSERVATORY_TEST_HOME");
        std::env::set_var("OBSERVATORY_TEST_HOME", temp.path());

        clear_codex_replay_caches();
        // With CODEX_REPLAY_DISK=1 the disk database below the temp HOME is used: the main cost of a
        // per-row autocommit is the fsync of the journal on the disk, an in-memory database cannot
        // measure the real write cost.
        let db = if std::env::var("CODEX_REPLAY_DISK").is_ok() {
            Database::open(crate::config::get_app_config_dir().join("observatory.db"))?
        } else {
            Database::memory()?
        };
        let start = std::time::Instant::now();
        let result = sync_codex_usage(&db)?;
        let full_elapsed = start.elapsed();
        eprintln!(
            "[REPLAY] full reimport: imported={} skipped={} suspected_dup={} deferred={} files={} errors={} elapsed={:.2?}",
            result.imported,
            result.skipped,
            result.suspected_duplicates,
            result.deferred_files,
            result.files_scanned,
            result.errors.len(),
            full_elapsed
        );

        let start = std::time::Instant::now();
        let steady = sync_codex_usage(&db)?;
        eprintln!(
            "[REPLAY] steady pass: imported={} deferred={} elapsed={:.2?}",
            steady.imported,
            steady.deferred_files,
            start.elapsed()
        );

        if let Ok(out_path) = std::env::var("CODEX_REPLAY_OUT") {
            use std::io::Write;
            let conn = lock_conn!(db.conn);
            let mut stmt = conn
                .prepare(
                    "SELECT request_id, model, request_model, input_tokens, output_tokens,
                            cache_read_tokens, cache_creation_tokens,
                            input_cost_usd, output_cost_usd, cache_read_cost_usd,
                            cache_creation_cost_usd, total_cost_usd,
                            session_id, provider_id, provider_type, status_code,
                            is_streaming, cost_multiplier, created_at, data_source
                     FROM proxy_request_logs
                     WHERE data_source = 'codex_session'
                     ORDER BY request_id",
                )
                .map_err(|e| AppError::Database(e.to_string()))?;
            let rows = stmt
                .query_map([], |row| {
                    let mut fields = Vec::with_capacity(20);
                    for idx in 0..20 {
                        fields.push(match row.get_ref(idx)? {
                            rusqlite::types::ValueRef::Null => "NULL".to_string(),
                            rusqlite::types::ValueRef::Integer(v) => v.to_string(),
                            rusqlite::types::ValueRef::Real(v) => v.to_string(),
                            rusqlite::types::ValueRef::Text(v) => {
                                String::from_utf8_lossy(v).into_owned()
                            }
                            rusqlite::types::ValueRef::Blob(v) => format!("blob:{}", v.len()),
                        });
                    }
                    Ok(fields.join("\t"))
                })
                .map_err(|e| AppError::Database(e.to_string()))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AppError::Database(e.to_string()))?;
            let mut out = fs::File::create(&out_path).expect("create replay out file");
            for line in &rows {
                writeln!(out, "{line}").expect("write replay row");
            }
            eprintln!("[REPLAY] wrote {} rows to {out_path}", rows.len());
        }

        match previous_home {
            Some(value) => std::env::set_var("OBSERVATORY_TEST_HOME", value),
            None => std::env::remove_var("OBSERVATORY_TEST_HOME"),
        }
        Ok(())
    }

    /// Build one line in the key order that Codex really writes (timestamp, type, payload); `json!`
    /// sorts the keys alphabetically and the kind of a line is determined by the real order
    fn raw_line(timestamp: &str, kind: &str, payload: &str) -> String {
        format!(r#"{{"timestamp":"{timestamp}","type":"{kind}","payload":{payload}}}"#)
    }

    fn raw_item(timestamp: &str, payload: &str) -> String {
        raw_line(timestamp, "response_item", payload)
    }

    fn raw_usage(input: u64, output: u64) -> String {
        format!(
            r#"{{"input_tokens":{input},"cached_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0,"total_tokens":{}}}"#,
            input + output
        )
    }

    /// `total` is the accumulated usage, `last` is the usage of this request
    fn raw_token_count(timestamp: &str, total: (u64, u64), last: (u64, u64)) -> String {
        raw_line(
            timestamp,
            "event_msg",
            &format!(
                r#"{{"type":"token_count","info":{{"total_token_usage":{},"last_token_usage":{}}},"rate_limits":{{"limit_id":"codex"}}}}"#,
                raw_usage(total.0, total.1),
                raw_usage(last.0, last.1)
            ),
        )
    }

    fn parsed_timings(lines: &[String]) -> Vec<Option<EstimatedTiming>> {
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        fs::write(&file, lines.join("\n") + "\n").unwrap();
        parse_codex_file(&file, Some(PARENT_ID.to_string()))
            .unwrap()
            .token_events
            .iter()
            .filter(|event| event.event_index.is_some())
            .map(|event| event.timing)
            .collect()
    }

    fn parsed_latencies(lines: &[String]) -> Vec<Option<i64>> {
        parsed_timings(lines)
            .into_iter()
            .map(|timing| timing.map(|t| t.latency_ms))
            .collect()
    }

    fn parsed_first_tokens(lines: &[String]) -> Vec<Option<i64>> {
        parsed_timings(lines)
            .into_iter()
            .map(|timing| timing.and_then(|t| t.first_token_ms))
            .collect()
    }

    /// The `item_completed` of the newer Codex, the key order follows the real log
    fn raw_item_completed(timestamp: &str, item_type: &str, started_at_ms: i64) -> String {
        raw_line(
            timestamp,
            "event_msg",
            &format!(
                r#"{{"type":"item_completed","thread_id":"t","turn_id":"u","item":{{"type":"{item_type}","id":"i1","content":[]}},"started_at_ms":{started_at_ms},"completed_at_ms":{started_at_ms}}}"#
            ),
        )
    }

    const USER_MESSAGE: &str = r#"{"type":"message","role":"user","content":[]}"#;
    const ASSISTANT_MESSAGE: &str = r#"{"type":"message","role":"assistant","content":[]}"#;
    const REASONING: &str = r#"{"type":"reasoning","summary":[]}"#;
    const TOOL_CALL: &str = r#"{"type":"custom_tool_call","call_id":"c1","name":"shell"}"#;
    const TOOL_OUTPUT: &str = r#"{"type":"custom_tool_call_output","call_id":"c1","output":"ok"}"#;

    #[test]
    fn test_latency_ends_at_usage_record_not_at_delayed_token_count() -> Result<(), AppError> {
        let lines = vec![
            raw_line(
                "2026-10-02T03:00:00.000Z",
                "turn_context",
                r#"{"model":"gpt-5"}"#,
            ),
            raw_item("2026-10-02T03:00:01.000Z", USER_MESSAGE),
            raw_item("2026-10-02T03:00:05.000Z", REASONING),
            raw_item("2026-10-02T03:00:09.000Z", TOOL_CALL),
            // The response ends here; the token_count is only written after the tools have run
            raw_line(
                "2026-10-02T03:00:09.500Z",
                "token_usage_record",
                &format!(r#"{{"turn_id":"t1","usage":{}}}"#, raw_usage(100, 600)),
            ),
            raw_item("2026-10-02T03:00:20.000Z", TOOL_OUTPUT),
            raw_token_count("2026-10-02T03:00:20.001Z", (100, 600), (100, 600)),
            // The second request: the usage record does not match this request, it is not trusted and the moment of the token_count is used instead
            raw_item("2026-10-02T03:00:25.000Z", REASONING),
            raw_item("2026-10-02T03:00:30.000Z", ASSISTANT_MESSAGE),
            raw_line(
                "2026-10-02T03:00:30.200Z",
                "token_usage_record",
                &format!(r#"{{"turn_id":"other","usage":{}}}"#, raw_usage(1, 999)),
            ),
            raw_token_count("2026-10-02T03:00:30.300Z", (300, 1300), (200, 700)),
        ];

        assert_eq!(parsed_latencies(&lines), vec![Some(8_500), Some(10_299)]);

        // The estimated duration has to end up in the database
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let mut contents = session_meta(PARENT_ID).to_string();
        contents.push('\n');
        contents.push_str(&(lines.join("\n") + "\n"));
        fs::write(&file, contents).unwrap();
        let db = Database::memory()?;
        sync_test_file(&db, &file, &[&file])?;
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare("SELECT latency_ms FROM proxy_request_logs ORDER BY request_id")
            .unwrap();
        let stored: Vec<i64> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(stored, vec![8_500, 10_299]);
        Ok(())
    }

    #[test]
    fn test_first_token_is_the_earliest_model_output_item_start() -> Result<(), AppError> {
        // The millisecond timestamp of 2026-10-02T03:00:00Z
        let base = 1_790_910_000_000;
        let lines = vec![
            raw_line(
                "2026-10-02T03:00:00.000Z",
                "turn_context",
                r#"{"model":"gpt-5"}"#,
            ),
            raw_item("2026-10-02T03:00:01.000Z", USER_MESSAGE),
            // The first request: the thinking starts at 3.2 seconds and the reply comes a bit later; the tool execution items are not model output
            raw_item_completed("2026-10-02T03:00:05.000Z", "Reasoning", base + 3_200),
            raw_item("2026-10-02T03:00:05.001Z", REASONING),
            raw_item_completed("2026-10-02T03:00:06.000Z", "AgentMessage", base + 5_500),
            raw_item("2026-10-02T03:00:06.001Z", ASSISTANT_MESSAGE),
            raw_line(
                "2026-10-02T03:00:06.100Z",
                "token_usage_record",
                &format!(r#"{{"turn_id":"t1","usage":{}}}"#, raw_usage(100, 600)),
            ),
            raw_token_count("2026-10-02T03:00:06.101Z", (100, 600), (100, 600)),
            // The second request starts with a tool call and has no thought and no reply item, the first token cannot be derived
            raw_item_completed("2026-10-02T03:00:07.000Z", "CommandExecution", base + 6_500),
            raw_item("2026-10-02T03:00:09.000Z", TOOL_CALL),
            raw_line(
                "2026-10-02T03:00:09.100Z",
                "token_usage_record",
                &format!(r#"{{"turn_id":"t1","usage":{}}}"#, raw_usage(200, 700)),
            ),
            raw_token_count("2026-10-02T03:00:09.101Z", (300, 1300), (200, 700)),
            // The third request: the start is earlier than the start line, so it does not line up and is dropped
            raw_item_completed("2026-10-02T03:00:11.000Z", "AgentMessage", base + 9_000),
            raw_item("2026-10-02T03:00:11.001Z", ASSISTANT_MESSAGE),
            raw_line(
                "2026-10-02T03:00:11.100Z",
                "token_usage_record",
                &format!(r#"{{"turn_id":"t1","usage":{}}}"#, raw_usage(300, 800)),
            ),
            raw_token_count("2026-10-02T03:00:11.101Z", (600, 2100), (300, 800)),
        ];

        assert_eq!(
            parsed_latencies(&lines),
            vec![Some(5_100), Some(2_999), Some(1_999)]
        );
        assert_eq!(parsed_first_tokens(&lines), vec![Some(2_200), None, None]);

        // The derived first token has to end up in the database
        let dir = tempdir().unwrap();
        let file = rollout_path(dir.path(), PARENT_ID);
        let mut contents = session_meta(PARENT_ID).to_string();
        contents.push('\n');
        contents.push_str(&(lines.join("\n") + "\n"));
        fs::write(&file, contents).unwrap();
        let db = Database::memory()?;
        sync_test_file(&db, &file, &[&file])?;
        let conn = lock_conn!(db.conn);
        let mut stmt = conn
            .prepare("SELECT first_token_ms FROM proxy_request_logs ORDER BY request_id")
            .unwrap();
        let stored: Vec<Option<i64>> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(stored, vec![Some(2_200), None, None]);
        Ok(())
    }

    #[test]
    fn test_latency_without_usage_record_stops_before_tool_execution() {
        let lines = vec![
            raw_item("2026-08-03T03:00:01.000Z", USER_MESSAGE),
            raw_item("2026-08-03T03:00:06.000Z", TOOL_CALL),
            // The tool ran for 10 seconds and the token_count is written right after the tool result
            raw_item("2026-08-03T03:00:16.000Z", TOOL_OUTPUT),
            raw_token_count("2026-08-03T03:00:16.001Z", (100, 600), (100, 600)),
            // A snapshot that is resent when the quota is refreshed is not a request and must not move the start of the next request further back
            raw_token_count("2026-08-03T03:00:17.000Z", (100, 600), (100, 600)),
            raw_item("2026-08-03T03:00:20.000Z", REASONING),
            raw_item("2026-08-03T03:00:22.000Z", ASSISTANT_MESSAGE),
            raw_token_count("2026-08-03T03:00:22.100Z", (300, 1300), (200, 700)),
        ];

        assert_eq!(parsed_latencies(&lines), vec![Some(5_000), Some(6_099)]);
    }

    #[test]
    fn test_latency_ignores_items_written_after_their_token_count() {
        // The layout of 2025: the output items and the tool result are backfilled after the token_count of their response
        let lines = vec![
            raw_item("2025-10-30T08:00:01.000Z", USER_MESSAGE),
            raw_token_count("2025-10-30T08:00:08.000Z", (100, 600), (100, 600)),
            raw_item("2025-10-30T08:00:08.000Z", REASONING),
            raw_item("2025-10-30T08:00:08.001Z", TOOL_CALL),
            raw_item("2025-10-30T08:00:08.001Z", TOOL_OUTPUT),
            raw_line(
                "2025-10-30T08:00:08.002Z",
                "turn_context",
                r#"{"model":"gpt-5"}"#,
            ),
            // The user sends the next message more than a minute later, that idle time does not count towards the request duration
            raw_item("2025-10-30T08:01:40.000Z", USER_MESSAGE),
            raw_token_count("2025-10-30T08:01:50.000Z", (300, 1300), (200, 700)),
        ];

        assert_eq!(parsed_latencies(&lines), vec![Some(7_000), Some(10_000)]);
    }

    #[test]
    fn test_timing_line_is_classified_from_the_line_head() {
        let long_output = format!(
            r#"{{"type":"function_call_output","call_id":"c1","output":"{}"}}"#,
            "x".repeat(4 * LINE_HEAD_BYTES)
        );
        assert_eq!(
            classify_timing_line(&raw_item("2026-10-02T03:00:00.250Z", &long_output)),
            Some((1_790_910_000_250, TimingLine::ToolOutput))
        );
        // A wording that appears in the body does not matter: the kind only looks at the type on the envelope
        let quoted =
            r#"{"type":"message","role":"user","content":"\"type\":\"token_usage_record\""}"#;
        assert_eq!(
            classify_timing_line(&raw_item("2026-10-02T03:00:00.250Z", quoted)).map(|line| line.1),
            Some(TimingLine::Boundary)
        );
        assert_eq!(
            classify_timing_line(&raw_line(
                "2026-10-02T03:00:00.250Z",
                "event_msg",
                r#"{"type":"task_started"}"#
            ))
            .map(|line| line.1),
            Some(TimingLine::TurnReset)
        );
        assert_eq!(
            classify_timing_line(&raw_line(
                "2026-10-02T03:00:00.250Z",
                "event_msg",
                r#"{"type":"item_completed"}"#
            )),
            None
        );
    }

    #[test]
    fn test_latency_does_not_inherit_the_start_of_an_aborted_request() {
        const DEVELOPER_MESSAGE: &str = r#"{"type":"message","role":"developer","content":[]}"#;
        let lines = vec![
            raw_item("2026-10-02T03:00:01.000Z", USER_MESSAGE),
            // This request was interrupted by the user after a thought, the token_count never arrives
            raw_item("2026-10-02T03:00:10.000Z", REASONING),
            raw_line(
                "2026-10-02T03:00:20.000Z",
                "event_msg",
                r#"{"type":"turn_aborted","reason":"interrupted"}"#,
            ),
            raw_line(
                "2026-10-02T03:02:00.000Z",
                "event_msg",
                r#"{"type":"task_started"}"#,
            ),
            raw_item("2026-10-02T03:02:00.010Z", USER_MESSAGE),
            raw_item("2026-10-02T03:02:20.000Z", REASONING),
            raw_item("2026-10-02T03:02:30.000Z", ASSISTANT_MESSAGE),
            raw_token_count("2026-10-02T03:02:30.100Z", (100, 600), (100, 600)),
            // A developer message that was inserted in the middle of the response is not a new round, the start does not move
            raw_item("2026-10-02T03:02:35.000Z", REASONING),
            raw_item("2026-10-02T03:02:36.000Z", DEVELOPER_MESSAGE),
            raw_item("2026-10-02T03:02:40.000Z", ASSISTANT_MESSAGE),
            raw_token_count("2026-10-02T03:02:40.200Z", (300, 1300), (200, 700)),
        ];

        assert_eq!(parsed_latencies(&lines), vec![Some(30_090), Some(10_100)]);
    }
}
