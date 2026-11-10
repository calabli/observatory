use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use chrono::{DateTime, FixedOffset};
use serde_json::Value;

use crate::session_manager::model::ContentRef;
use crate::session_manager::SessionMeta;

/// Maximum number of characters for session titles (shared across providers).
pub const TITLE_MAX_CHARS: usize = 80;

/// The cache of the parsed session files: the modification time and the size of a file tell
/// whether it changed, and an unchanged file reuses the [`SessionMeta`] parsed last time, so
/// only the new or the modified files are parsed again.
///
/// Only summaries are stored (title, path, time, ...), never the session text; every scan
/// rebuilds the whole table from the files it saw in this round, so the entries of deleted
/// files are dropped as well and the memory usage is proportional to the number of sessions
/// (about 1 MB per thousand sessions). The parse result must depend on the file alone -- the
/// parts that depend on external data (such as the thread titles of Codex) have to be layered
/// on top after the cached result was fetched.
pub struct FileParseCache {
    entries: Mutex<HashMap<PathBuf, CachedParse>>,
}

struct CachedParse {
    modified: Option<SystemTime>,
    len: u64,
    meta: Option<SessionMeta>,
}

impl FileParseCache {
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Parse `files` in order and reuse the cache for the files that did not change. Returns the ones a session could be parsed from.
    pub fn scan<F>(&self, files: Vec<PathBuf>, parse: F) -> Vec<SessionMeta>
    where
        F: Fn(&Path) -> io::Result<Option<SessionMeta>>,
    {
        // On a poisoned lock (the previous scan panicked) the old cache is dropped and the scan starts over, which does not affect this result
        let mut previous = match self.entries.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                guard.clear();
                HashMap::new()
            }
        };

        let mut next = HashMap::with_capacity(files.len());
        let mut sessions = Vec::new();
        for path in files {
            let (modified, len) = match std::fs::metadata(&path) {
                Ok(meta) => (meta.modified().ok(), meta.len()),
                Err(_) => continue,
            };
            let meta = match previous.remove(&path) {
                Some(entry) if entry.modified == modified && entry.len == len => entry.meta,
                _ => match parse(&path) {
                    Ok(meta) => meta,
                    // A file that cannot be read (permissions, locked by another process, ...) is not cached:
                    // otherwise the "no sessions" result would keep being hit after reading works again, until
                    // the file changes or the application restarts
                    Err(err) => {
                        log::debug!("会话文件暂时读取失败，下轮重试 {}: {err}", path.display());
                        continue;
                    }
                },
            };
            if let Some(meta) = &meta {
                sessions.push(meta.clone());
            }
            next.insert(
                path,
                CachedParse {
                    modified,
                    len,
                    meta,
                },
            );
        }

        if let Ok(mut guard) = self.entries.lock() {
            *guard = next;
        }
        sessions
    }
}

impl Default for FileParseCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Read the first `head_n` lines and last `tail_n` lines from a file.
/// For small files (< 16 KB), reads all lines once to avoid unnecessary seeking.
pub fn read_head_tail_lines(
    path: &Path,
    head_n: usize,
    tail_n: usize,
) -> io::Result<(Vec<String>, Vec<String>)> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();

    // For small files, read all lines once and split
    if file_len < 16_384 {
        let reader = BufReader::new(file);
        let all: Vec<String> = reader.lines().map_while(Result::ok).collect();
        let head = all.iter().take(head_n).cloned().collect();
        let skip = all.len().saturating_sub(tail_n);
        let tail = all.into_iter().skip(skip).collect();
        return Ok((head, tail));
    }

    // Read head lines from the beginning
    let reader = BufReader::new(file);
    let head: Vec<String> = reader.lines().take(head_n).map_while(Result::ok).collect();

    // Seek to last ~16 KB for tail lines
    let seek_pos = file_len.saturating_sub(16_384);
    let mut file2 = File::open(path)?;
    file2.seek(SeekFrom::Start(seek_pos))?;
    let tail_reader = BufReader::new(file2);
    let all_tail: Vec<String> = tail_reader.lines().map_while(Result::ok).collect();

    // Skip first partial line if we seeked into the middle of a line
    let skip_first = if seek_pos > 0 { 1 } else { 0 };
    let usable: Vec<String> = all_tail.into_iter().skip(skip_first).collect();
    let skip = usable.len().saturating_sub(tail_n);
    let tail = usable.into_iter().skip(skip).collect();

    Ok((head, tail))
}

pub fn parse_timestamp_to_ms(value: &Value) -> Option<i64> {
    // Integer: milliseconds (>1e12) or seconds
    if let Some(n) = value.as_i64() {
        return Some(if n > 1_000_000_000_000 { n } else { n * 1000 });
    }
    if let Some(n) = value.as_f64() {
        let n = n as i64;
        return Some(if n > 1_000_000_000_000 { n } else { n * 1000 });
    }
    // RFC3339 string
    let raw = value.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt: DateTime<FixedOffset>| dt.timestamp_millis())
}

pub fn extract_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.to_string(),
        Value::Array(items) => items
            .iter()
            .filter_map(extract_text_from_item)
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(map) => map
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        _ => String::new(),
    }
}

fn extract_text_from_item(item: &Value) -> Option<String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

    // Anthropic uses tool_use; Pi's assistant messages use toolCall.
    if matches!(item_type, "tool_use" | "toolCall") {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Some(format!("[Tool: {name}]"));
    }

    // tool_result: extract nested content
    if item_type == "tool_result" {
        if let Some(content) = item.get("content") {
            let text = extract_text(content);
            if !text.is_empty() {
                return Some(text);
            }
        }
        return None;
    }

    if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(text) = item.get("input_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(text) = item.get("output_text").and_then(|v| v.as_str()) {
        return Some(text.to_string());
    }

    if let Some(content) = item.get("content") {
        let text = extract_text(content);
        if !text.is_empty() {
            return Some(text);
        }
    }

    None
}

pub fn truncate_summary(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }

    let mut result = trimmed.chars().take(max_chars).collect::<String>();
    result.push_str("...");
    result
}

/// The byte range of one JSONL line inside the file (including the trailing `\n`), used to build a [`ContentRef::Jsonl`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonlSpan {
    /// The byte offset of the start of the line
    pub offset: u64,
    /// The number of bytes of the whole line, including the trailing `\n` (the last line of a file may not have one)
    pub len: u32,
}

impl JsonlSpan {
    /// A reference to the content at `pointer` (RFC 6901) inside this line
    pub fn content_ref(&self, pointer: impl Into<String>) -> ContentRef {
        ContentRef::Jsonl {
            offset: self.offset,
            len: self.len,
            pointer: pointer.into(),
        }
    }
}

/// One line of a JSONL file and its byte range.
pub struct LineSpan<'a> {
    pub span: JsonlSpan,
    /// The content of the line, with the trailing `\r\n` / `\n` removed
    pub bytes: &'a [u8],
}

/// Read the file line by line with `read_until(b'\n')` and accumulate the byte offsets, for the
/// parsers that have to refer back to the raw text
/// (all JSONL parsers share this one, so the ranges of `ContentRef::Jsonl` stay consistent).
///
/// The same buffer is reused and no UTF-8 validation is done (that is left to
/// `serde_json::from_slice`), so it saves one allocation and one validation compared to
/// `BufRead::lines()`.
pub struct LineSpans<R> {
    reader: R,
    buf: Vec<u8>,
    offset: u64,
}

impl<R: BufRead> LineSpans<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            buf: Vec::with_capacity(64 * 1024),
            offset: 0,
        }
    }

    /// Read the next line; returns `Ok(None)` at the end of the file.
    pub fn next_line(&mut self) -> io::Result<Option<LineSpan<'_>>> {
        self.buf.clear();
        let read = self.reader.read_until(b'\n', &mut self.buf)?;
        if read == 0 {
            return Ok(None);
        }
        let offset = self.offset;
        self.offset += read as u64;

        let mut end = self.buf.len();
        if end > 0 && self.buf[end - 1] == b'\n' {
            end -= 1;
            if end > 0 && self.buf[end - 1] == b'\r' {
                end -= 1;
            }
        }
        Ok(Some(LineSpan {
            span: JsonlSpan {
                offset,
                len: u32::try_from(read).unwrap_or(u32::MAX),
            },
            bytes: &self.buf[..end],
        }))
    }
}

/// Read a JSONL file line by line, skipping empty and unparsable lines, and hand the callback
/// the (byte range, JSON). When the callback returns `Err` the reading stops right away and
/// the error is passed on.
pub fn for_each_jsonl_value(
    path: &Path,
    mut f: impl FnMut(JsonlSpan, Value) -> Result<(), String>,
) -> Result<(), String> {
    let file = File::open(path).map_err(|e| format!("Failed to open session file: {e}"))?;
    let mut lines = LineSpans::new(BufReader::new(file));
    while let Some(line) = lines
        .next_line()
        .map_err(|e| format!("Failed to read session file: {e}"))?
    {
        let bytes = line.bytes.trim_ascii();
        if bytes.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            continue;
        };
        f(line.span, value)?;
    }
    Ok(())
}

pub fn path_basename(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let normalized = trimmed.trim_end_matches(['/', '\\']);
    let last = normalized
        .split(['/', '\\'])
        .next_back()
        .filter(|segment| !segment.is_empty())?;
    Some(last.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta_from_file(path: &Path) -> Option<SessionMeta> {
        let text = std::fs::read_to_string(path).ok()?;
        if text.trim() == "skip" {
            return None;
        }
        Some(SessionMeta {
            provider_id: "test".to_string(),
            session_id: text.trim().to_string(),
            title: None,
            summary: None,
            project_dir: None,
            created_at: None,
            last_active_at: None,
            source_path: Some(path.to_string_lossy().to_string()),
            resume_command: None,
        })
    }

    /// A read failure must not be cached as "no sessions": once reading works again, an unchanged file has to be parsed again
    #[test]
    fn file_parse_cache_retries_files_that_failed_to_read() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        std::fs::write(&a, "alpha").unwrap();

        let cache = FileParseCache::new();
        let fail = Cell::new(true);
        let calls = Cell::new(0);
        let parse = |path: &Path| {
            calls.set(calls.get() + 1);
            if fail.get() {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"))
            } else {
                Ok(meta_from_file(path))
            }
        };

        assert!(cache.scan(vec![a.clone()], parse).is_empty());
        assert!(cache.scan(vec![a.clone()], parse).is_empty());
        assert_eq!(calls.get(), 2, "失败的文件每轮都要重试，不能命中缓存");

        fail.set(false);
        let sessions = cache.scan(vec![a.clone()], parse);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].session_id, "alpha");
        assert_eq!(calls.get(), 3);

        // Only a success enters the cache: a later scan does not parse it again
        assert_eq!(cache.scan(vec![a], parse).len(), 1);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn file_parse_cache_reparses_only_changed_files() {
        use std::cell::Cell;

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        let c = dir.path().join("c.jsonl");
        std::fs::write(&a, "alpha").unwrap();
        std::fs::write(&b, "beta").unwrap();
        std::fs::write(&c, "skip").unwrap();

        let cache = FileParseCache::new();
        let calls = Cell::new(0);
        let parse = |path: &Path| {
            calls.set(calls.get() + 1);
            Ok(meta_from_file(path))
        };
        let ids = |sessions: Vec<SessionMeta>| -> Vec<String> {
            sessions.into_iter().map(|m| m.session_id).collect()
        };

        // The first time everything is parsed; files without a session are recorded as well so they are not parsed again next time
        let files = vec![a.clone(), b.clone(), c.clone()];
        assert_eq!(ids(cache.scan(files.clone(), parse)), ["alpha", "beta"]);
        assert_eq!(calls.get(), 3);

        // Nothing changed: not a single file is parsed again
        assert_eq!(ids(cache.scan(files.clone(), parse)), ["alpha", "beta"]);
        assert_eq!(calls.get(), 3);

        // The content changed (the length differs): only this one is parsed again
        std::fs::write(&b, "beta-2").unwrap();
        assert_eq!(ids(cache.scan(files.clone(), parse)), ["alpha", "beta-2"]);
        assert_eq!(calls.get(), 4);

        // The file was deleted: it no longer shows up and its entry is dropped; it has to be parsed again when it reappears
        std::fs::remove_file(&a).unwrap();
        assert_eq!(ids(cache.scan(files.clone(), parse)), ["beta-2"]);
        assert_eq!(calls.get(), 4);
        std::fs::write(&a, "alpha").unwrap();
        assert_eq!(ids(cache.scan(files, parse)), ["alpha", "beta-2"]);
        assert_eq!(calls.get(), 5);
    }

    #[test]
    fn parse_timestamp_to_ms_supports_integers_and_rfc3339() {
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_771_061_953_033_i64)),
            Some(1_771_061_953_033)
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!(1_771_061_953_i64)),
            Some(1_771_061_953_000)
        );
        assert_eq!(
            parse_timestamp_to_ms(&json!("1970-01-01T00:00:01Z")),
            Some(1_000)
        );
    }

    #[test]
    fn line_spans_report_byte_offsets() {
        let data = "{\"a\":1}\r\n\n{\"b\":\"字\"}\n{\"c\":3}";
        let mut spans = LineSpans::new(io::Cursor::new(data.as_bytes()));
        let mut got = Vec::new();
        while let Some(span) = spans.next_line().unwrap() {
            got.push((
                span.span.offset,
                span.span.len,
                String::from_utf8(span.bytes.to_vec()).unwrap(),
            ));
        }
        assert_eq!(
            got,
            [
                (0, 9, "{\"a\":1}".to_string()),
                (9, 1, String::new()),
                (10, 12, "{\"b\":\"字\"}".to_string()),
                (22, 7, "{\"c\":3}".to_string()),
            ]
        );
        // The offset plus the length cuts back to the original line
        assert_eq!(&data.as_bytes()[10..10 + 12], "{\"b\":\"字\"}\n".as_bytes());
    }

    #[test]
    fn jsonl_reader_reports_byte_spans() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{\"a\":1}\n\n{bad\n{\"b\":\"字\"}").unwrap();
        let mut seen = Vec::new();
        for_each_jsonl_value(&path, |span, value| {
            seen.push((span, value));
            Ok(())
        })
        .unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, JsonlSpan { offset: 0, len: 8 });
        // The second line: 8 + 1 + 5 bytes in front of it, the last line has no line break
        assert_eq!(
            seen[1].0,
            JsonlSpan {
                offset: 14,
                len: 11
            }
        );
        assert_eq!(seen[1].1, json!({"b":"字"}));
    }

    #[test]
    fn extract_text_supports_pi_tool_calls() {
        assert_eq!(
            extract_text(&json!([{ "type": "toolCall", "name": "read" }])),
            "[Tool: read]"
        );
    }
}
