//! Read Antigravity's exported transcript; never open its binary conversation store for writing.
use super::{
    blocks::assign_turn_ids,
    utils::{parse_timestamp_to_ms, truncate_summary},
};
use crate::session_manager::model::{MessageMeta, SessionBlock};
use crate::session_manager::{SessionMessage, SessionMeta};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

/// Minimal per-step metadata needed by the usage collector. Large fields like
/// `thinking`, `media`, `tool_calls` arguments, and `GENERIC` tool outputs are
/// skipped during deserialization instead of building a full `serde_json::Value` DOM.
#[derive(Debug, Clone)]
pub(crate) struct UsageStep {
    pub step_index: u64,
    pub step_type: String,
    pub source: Option<String>,
    pub status: Option<String>,
    pub created_at_ms: Option<i64>,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub has_tool_calls: bool,
    pub content: Option<String>,
}

fn deserialize_non_empty_seq<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct NonEmptySeq;
    impl<'de> serde::de::Visitor<'de> for NonEmptySeq {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a JSON array or null")
        }
        fn visit_none<E>(self) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_unit<E>(self) -> Result<bool, E> {
            Ok(false)
        }
        fn visit_seq<A>(self, mut seq: A) -> Result<bool, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut non_empty = false;
            while seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                non_empty = true;
            }
            Ok(non_empty)
        }
    }
    deserializer.deserialize_any(NonEmptySeq)
}

#[derive(Deserialize)]
struct RawUsageStep {
    step_index: Option<u64>,
    #[serde(rename = "type", default)]
    step_type: String,
    source: Option<String>,
    status: Option<String>,
    created_at: Option<Value>,
    model: Option<String>,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
    #[serde(default)]
    cache_write_tokens: u64,
    #[serde(default, deserialize_with = "deserialize_non_empty_seq")]
    tool_calls: bool,
}

#[derive(Deserialize)]
struct RawUsageStepWithContent {
    #[serde(flatten)]
    base: RawUsageStep,
    content: Option<String>,
}

impl RawUsageStep {
    fn into_usage_step(self, content: Option<String>) -> Option<UsageStep> {
        let step_index = self.step_index?;
        let created_at_ms = self.created_at.as_ref().and_then(parse_timestamp_to_ms);
        Some(UsageStep {
            step_index,
            step_type: self.step_type,
            source: self.source,
            status: self.status,
            created_at_ms,
            model: self.model,
            input_tokens: i64::try_from(self.input_tokens).unwrap_or(0),
            output_tokens: i64::try_from(self.output_tokens).unwrap_or(0),
            cache_read_tokens: i64::try_from(self.cache_read_tokens).unwrap_or(0),
            cache_write_tokens: i64::try_from(self.cache_write_tokens).unwrap_or(0),
            has_tool_calls: self.tool_calls,
            content,
        })
    }
}

/// Stream-parse only the fields needed for usage collection. Lines that do not
/// mention `Model Selection` skip `content` completely (zero allocation for huge
/// tool outputs). The latest record for each `step_index` wins.
pub(crate) fn usage_steps(path: &Path) -> Result<Vec<UsageStep>, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("Cannot read Antigravity transcript: {e}"))?;
    let mut steps = BTreeMap::new();
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|e| format!("Cannot read Antigravity transcript: {e}"))?;
        if bytes == 0 {
            break;
        }
        let step = if line.contains("Model Selection") {
            serde_json::from_str::<RawUsageStepWithContent>(&line)
                .ok()
                .and_then(|raw| {
                    let keep_content = raw.base.step_type == "USER_INPUT";
                    raw.base
                        .into_usage_step(keep_content.then_some(raw.content).flatten())
                })
        } else {
            serde_json::from_str::<RawUsageStep>(&line)
                .ok()
                .and_then(|raw| raw.into_usage_step(None))
        };
        if let Some(step) = step {
            steps.insert(step.step_index, step);
        }
    }
    Ok(steps.into_values().collect())
}

/// Lightweight transcript file discovery for the usage collector: walks the
/// session roots directly without opening `conversation_summaries.db` or
/// fallback-parsing transcripts to build UI titles.
pub(crate) fn scan_transcript_files() -> Vec<(String, PathBuf)> {
    let mut seen = std::collections::HashSet::new();
    let mut files = Vec::new();
    for (root, _) in session_sources() {
        let canonical_root = root.canonicalize().ok();
        let Ok(entries) = fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(id) = entry
                .file_name()
                .to_str()
                .map(str::to_string)
                .filter(|id| valid_session_id(id))
            else {
                continue;
            };
            let path = entry
                .path()
                .join(".system_generated/logs/transcript_full.jsonl");
            if !path.is_file()
                || !path
                    .canonicalize()
                    .ok()
                    .zip(canonical_root.as_ref())
                    .is_some_and(|(p, r)| p.starts_with(r))
            {
                continue;
            }
            if seen.insert(id.clone()) {
                files.push((id, path));
            }
        }
    }
    files
}

/// Antigravity CLI keeps sessions in `~/.gemini/antigravity-cli/brain`, while the IDE writes the
/// same layout to `~/.gemini/antigravity/brain`. API-key providers run agy with a private
/// `--gemini_dir` (`~/.cc-switch/agy/<provider>/`), so their sessions live under that directory's
/// `antigravity-cli/brain`. All of them are scanned.
pub fn session_roots() -> Vec<PathBuf> {
    session_sources()
        .into_iter()
        .map(|(root, _)| root)
        .collect()
}

/// Each root, with the private `--gemini_dir` its sessions must be resumed with.
fn session_sources() -> Vec<(PathBuf, Option<PathBuf>)> {
    let cli = crate::agy_config::agy_dir();
    let ide = cli.with_file_name("antigravity");
    let mut sources = vec![(cli.join("brain"), None), (ide.join("brain"), None)];
    let mut profiles: Vec<PathBuf> = fs::read_dir(crate::agy_config::key_profiles_root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .map(|entry| entry.path())
        .collect();
    profiles.sort();
    for dir in profiles {
        sources.push((dir.join("antigravity-cli").join("brain"), Some(dir)));
    }
    sources
}

pub(crate) fn records(path: &Path) -> Result<Vec<Value>, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("Cannot read Antigravity transcript: {e}"))?;
    // The export can rewrite steps as they finish. The latest record for a step wins.
    let mut steps = BTreeMap::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|e| format!("Cannot read Antigravity transcript: {e}"))?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(index) = value["step_index"].as_u64() {
            steps.insert(index, value);
        }
    }
    Ok(steps.into_values().collect())
}

pub fn scan_sessions() -> Vec<SessionMeta> {
    scan_roots(&session_sources())
}

/// Missing roots are tolerated; a session id found in several roots is reported once (first wins).
fn scan_roots(sources: &[(PathBuf, Option<PathBuf>)]) -> Vec<SessionMeta> {
    let mut seen = std::collections::HashSet::new();
    let mut sessions = Vec::new();
    for (root, gemini_dir) in sources {
        for meta in scan_root(root, gemini_dir.as_deref()) {
            if seen.insert(meta.session_id.clone()) {
                sessions.push(meta);
            }
        }
    }
    sessions
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn scan_root(root: &Path, gemini_dir: Option<&Path>) -> Vec<SessionMeta> {
    let mut summaries = read_summaries(
        &root
            .parent()
            .unwrap_or(root)
            .join("conversation_summaries.db"),
        gemini_dir,
    );
    let canonical_root = root.canonicalize().ok();
    let mut sessions = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(id) = entry
                .file_name()
                .to_str()
                .map(str::to_string)
                .filter(|id| valid_session_id(id))
            else {
                continue;
            };
            let path = entry
                .path()
                .join(".system_generated/logs/transcript_full.jsonl");
            if !path.is_file()
                || !path
                    .canonicalize()
                    .ok()
                    .zip(canonical_root.as_ref())
                    .is_some_and(|(path, root)| path.starts_with(root))
            {
                continue;
            }
            let summary = summaries.remove(&id);
            // Avoid parsing every transcript on list refresh when agy's own index is available.
            let mut meta = if let Some(meta) = summary {
                meta
            } else {
                let Ok(rows) = records(&path) else { continue };
                let first = rows
                    .iter()
                    .find(|v| v["type"] == "USER_INPUT")
                    .and_then(|v| v["content"].as_str())
                    .map(user_request);
                SessionMeta {
                    provider_id: "agy".into(),
                    session_id: id.clone(),
                    title: first.map(|s| truncate_summary(s, 80)),
                    summary: first.map(|s| truncate_summary(s, 160)),
                    project_dir: None,
                    created_at: rows.iter().find_map(timestamp),
                    last_active_at: rows.iter().filter_map(timestamp).max(),
                    source_path: None,
                    resume_command: Some(crate::agy_config::resume_command(gemini_dir, &id)),
                }
            };
            meta.source_path = Some(path.to_string_lossy().into_owned());
            sessions.push(meta);
        }
    }
    // A binary-only conversation can still be found and resumed, but has no readable transcript.
    sessions.extend(summaries.into_values());
    sessions
}

/// Batch read of `(title, project_dir)` for all Antigravity sessions from
/// `conversation_summaries.db` (one fast SQLite scan, zero transcript file reads).
pub(crate) fn lookup_all_summaries(
) -> std::collections::HashMap<String, (Option<String>, Option<String>)> {
    let mut map = std::collections::HashMap::new();
    for (root, _) in session_sources() {
        let db_path = root
            .parent()
            .unwrap_or(&root)
            .join("conversation_summaries.db");
        let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            continue;
        };
        let Ok(mut stmt) = conn
            .prepare("SELECT conversation_id, title, workspace_uris FROM conversation_summaries")
        else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        }) else {
            continue;
        };
        for (id, title, uris) in rows.flatten() {
            if !valid_session_id(&id) || map.contains_key(&id) {
                continue;
            }
            let project_dir = serde_json::from_str::<Vec<String>>(&uris)
                .ok()
                .and_then(|v| v.into_iter().next())
                .and_then(|uri| url::Url::parse(&uri).ok()?.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned());
            map.insert(id, ((!title.is_empty()).then_some(title), project_dir));
        }
    }
    map
}

/// Fast on-demand lookup of `(title, project_dir)` for a single Antigravity session ID
/// without scanning all sessions or parsing any transcript files.
pub(crate) fn lookup_session_summary(session_id: &str) -> Option<(Option<String>, Option<String>)> {
    if !valid_session_id(session_id) {
        return None;
    }
    for (root, _) in session_sources() {
        let db_path = root
            .parent()
            .unwrap_or(&root)
            .join("conversation_summaries.db");
        let Ok(conn) = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) else {
            continue;
        };
        let row: Result<(String, String), _> = conn.query_row(
            "SELECT title, workspace_uris FROM conversation_summaries WHERE conversation_id = ?1 LIMIT 1",
            [session_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        );
        if let Ok((title, uris)) = row {
            let project_dir = serde_json::from_str::<Vec<String>>(&uris)
                .ok()
                .and_then(|v| v.into_iter().next())
                .and_then(|uri| url::Url::parse(&uri).ok()?.to_file_path().ok())
                .map(|p| p.to_string_lossy().into_owned());
            return Some(((!title.is_empty()).then_some(title), project_dir));
        }
    }
    None
}

fn read_summaries(path: &Path, gemini_dir: Option<&Path>) -> BTreeMap<String, SessionMeta> {
    let mut result = BTreeMap::new();
    let Ok(conn) =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
        return result;
    };
    let Ok(mut query) = conn.prepare("SELECT conversation_id, title, workspace_uris, preview, last_modified_time FROM conversation_summaries") else { return result };
    let Ok(rows) = query.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
        ))
    }) else {
        return result;
    };
    for (id, title, uris, preview, modified) in rows.flatten() {
        if !valid_session_id(&id) {
            continue;
        }
        let project_dir = serde_json::from_str::<Vec<String>>(&uris)
            .ok()
            .and_then(|v| v.into_iter().next())
            .and_then(|uri| url::Url::parse(&uri).ok()?.to_file_path().ok())
            .map(|p| p.to_string_lossy().into_owned());
        let last_active_at =
            parse_timestamp_to_ms(&Value::String(modified.clone())).or_else(|| {
                chrono::NaiveDateTime::parse_from_str(&modified, "%Y-%m-%d %H:%M:%S%.f")
                    .ok()
                    .map(|t| t.and_utc().timestamp_millis())
            });
        result.insert(
            id.clone(),
            SessionMeta {
                provider_id: "agy".into(),
                session_id: id.clone(),
                title: (!title.is_empty()).then_some(title),
                summary: (!preview.is_empty()).then(|| truncate_summary(&preview, 160)),
                project_dir,
                created_at: None,
                last_active_at,
                source_path: None,
                resume_command: Some(crate::agy_config::resume_command(gemini_dir, &id)),
            },
        );
    }
    result
}

/// agy stores each prompt wrapped in `<USER_REQUEST>`, followed by context it adds itself
/// (`<ADDITIONAL_METADATA>`, `<USER_SETTINGS_CHANGE>`, ...). Only the part the user typed is shown.
fn user_request(content: &str) -> &str {
    content
        .split_once("<USER_REQUEST>")
        .and_then(|(_, rest)| rest.split_once("</USER_REQUEST>"))
        .map(|(inner, _)| inner.trim())
        .filter(|inner| !inner.is_empty())
        .unwrap_or(content)
}

pub(crate) fn timestamp(value: &Value) -> Option<i64> {
    parse_timestamp_to_ms(&value["created_at"])
}

pub fn load_messages(path: &Path) -> Result<Vec<SessionMessage>, String> {
    let mut messages = Vec::new();
    for value in records(path)? {
        let role = match value["type"].as_str() {
            Some("USER_INPUT") => "user",
            Some("PLANNER_RESPONSE") => "assistant",
            _ if value["source"] == "MODEL" => "assistant",
            _ => "system",
        };
        let content = value["content"].as_str().unwrap_or("");
        let content = if role == "user" {
            user_request(content)
        } else {
            content
        };
        // Keep unrecognized steps visible instead of silently dropping future tool/event types.
        let content = if content.is_empty() {
            serde_json::to_string_pretty(&value).unwrap_or_default()
        } else {
            content.into()
        };
        let mut message = SessionMessage::from_blocks(
            role,
            timestamp(&value),
            vec![SessionBlock::Text {
                text: content,
                full: None,
            }],
        );
        message.id = value["step_index"].as_u64().map(|n| n.to_string());
        message.meta = Some(MessageMeta {
            model: value["model"].as_str().map(String::from),
            input_tokens: value["input_tokens"].as_u64(),
            output_tokens: value["output_tokens"].as_u64(),
            cache_read_tokens: value["cache_read_tokens"].as_u64(),
            cache_write_tokens: value["cache_write_tokens"].as_u64(),
            reasoning_tokens: value["reasoning_tokens"].as_u64(),
            ..Default::default()
        });
        messages.push(message);
    }
    assign_turn_ids(&mut messages);
    Ok(messages)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agy_binary_only_sessions_keep_index_metadata_without_fabricated_transcripts() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project space");
        let workspace_uris =
            serde_json::to_string(&[url::Url::from_file_path(&project).unwrap().to_string()])
                .unwrap();
        let db = rusqlite::Connection::open(dir.path().join("conversation_summaries.db")).unwrap();
        db.execute_batch("CREATE TABLE conversation_summaries(conversation_id TEXT,title TEXT,workspace_uris TEXT,preview TEXT,last_modified_time TEXT);").unwrap();
        db.execute(
            "INSERT INTO conversation_summaries VALUES (?1,?2,?3,?4,?5)",
            rusqlite::params![
                "abc-123",
                "Test title",
                workspace_uris,
                "Preview",
                "2026-10-13T17:26:12Z"
            ],
        )
        .unwrap();
        let sessions = scan_root(&dir.path().join("brain"), None);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].title.as_deref(), Some("Test title"));
        assert_eq!(
            sessions[0].project_dir.as_deref(),
            Some(project.to_string_lossy().as_ref())
        );
        assert!(sessions[0].source_path.is_none());
        assert!(sessions[0].last_active_at.is_some());
    }

    #[test]
    fn agy_scans_cli_and_ide_roots_deduplicating_and_tolerating_missing() {
        let home = tempfile::tempdir().unwrap();
        let write = |root: &str, id: &str| {
            let logs = home
                .path()
                .join(root)
                .join("brain")
                .join(id)
                .join(".system_generated/logs");
            fs::create_dir_all(&logs).unwrap();
            fs::write(
                logs.join("transcript_full.jsonl"),
                r#"{"step_index":0,"type":"USER_INPUT","content":"hi"}"#,
            )
            .unwrap();
        };
        write("antigravity-cli", "cli-1");
        write("antigravity", "ide-1");
        write("antigravity", "cli-1");
        let roots = vec![
            (home.path().join("antigravity-cli/brain"), None),
            (home.path().join("antigravity/brain"), None),
            (home.path().join("missing/brain"), None),
        ];
        let mut ids: Vec<_> = scan_roots(&roots)
            .into_iter()
            .map(|s| s.session_id)
            .collect();
        ids.sort();
        assert_eq!(ids, ["cli-1", "ide-1"]);
    }

    #[test]
    #[cfg(unix)]
    fn agy_discovery_does_not_follow_transcripts_outside_its_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let session = root.path().join("abc-123");
        fs::create_dir_all(&session).unwrap();
        let logs = outside.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("transcript_full.jsonl"),
            r#"{"step_index":0,"type":"USER_INPUT","content":"outside"}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), session.join(".system_generated")).unwrap();
        assert!(scan_root(root.path(), None).is_empty());
    }

    #[test]
    fn agy_shows_only_the_typed_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("abc-123/.system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        let file = logs.join("transcript_full.jsonl");
        fs::write(&file, concat!(
            "{\"step_index\":0,\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>\\nfix the build\\n</USER_REQUEST>\\n<ADDITIONAL_METADATA>\\nThe current local time is: 2026-10-21T08:40:35+08:00.\\n</ADDITIONAL_METADATA>\"}\n",
            "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"content\":\"<USER_REQUEST> stays in replies\"}\n",
            "{\"step_index\":2,\"type\":\"USER_INPUT\",\"content\":\"plain prompt\"}\n",
        )).unwrap();
        assert_eq!(
            scan_root(dir.path(), None)[0].title.as_deref(),
            Some("fix the build")
        );
        let messages = load_messages(&file).unwrap();
        assert_eq!(messages[0].content, "fix the build");
        assert_eq!(messages[1].content, "<USER_REQUEST> stays in replies");
        assert_eq!(messages[2].content, "plain prompt");
    }

    #[test]
    fn agy_reads_tokens_rewritten_steps_and_partial_tail() {
        let dir = tempfile::tempdir().unwrap();
        let logs = dir.path().join("abc-123/.system_generated/logs");
        fs::create_dir_all(&logs).unwrap();
        let file = logs.join("transcript_full.jsonl");
        fs::write(&file, concat!(
            "{\"step_index\":0,\"type\":\"USER_INPUT\",\"content\":\"hello\",\"created_at\":\"2026-10-13T17:26:12Z\"}\n",
            "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"content\":\"draft\"}\n",
            "{\"step_index\":1,\"type\":\"PLANNER_RESPONSE\",\"content\":\"done\",\"input_tokens\":100,\"output_tokens\":12,\"cache_read_tokens\":20}\n{"
        )).unwrap();
        let sessions = scan_root(dir.path(), None);
        assert_eq!(sessions.len(), 1);
        // The command format is decided by agy_config::resume_command (the slimmed-down
        // Observatory build does not distinguish a private gemini_dir); this only checks
        // that the session ID and the directory are passed through correctly.
        assert_eq!(
            sessions[0].resume_command,
            Some(crate::agy_config::resume_command(None, "abc-123"))
        );
        let private = Path::new("/home/me/.cc-switch/agy/relay");
        assert_eq!(
            scan_root(dir.path(), Some(private))[0].resume_command,
            Some(crate::agy_config::resume_command(Some(private), "abc-123"))
        );
        let messages = load_messages(&file).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].content, "done");
        assert_eq!(messages[1].meta.as_ref().unwrap().input_tokens, Some(100));
        assert_eq!(
            messages[1].meta.as_ref().unwrap().cache_read_tokens,
            Some(20)
        );
    }

    #[test]
    fn agy_usage_steps_streams_minimal_fields_and_skips_generic_content() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("transcript_full.jsonl");
        fs::write(&file, concat!(
            "{\"step_index\":0,\"source\":\"USER_EXPLICIT\",\"type\":\"USER_INPUT\",\"status\":\"DONE\",\"created_at\":\"2026-11-21T13:12:07Z\",\"content\":\"changed setting `Model Selection` from None to Gemini 4 Neon (Medium).\"}\n",
            "{\"step_index\":1,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-11-21T13:12:07Z\",\"input_tokens\":100,\"output_tokens\":250,\"cache_read_tokens\":50,\"thinking\":\"very long thinking\",\"tool_calls\":[{\"name\":\"run_command\",\"args\":{\"huge\":\"payload\"}}]}\n",
            "{\"step_index\":2,\"source\":\"MODEL\",\"type\":\"GENERIC\",\"status\":\"DONE\",\"created_at\":\"2026-11-21T13:12:13Z\",\"content\":\"huge tool output without model switch\"}\n",
            "{\"step_index\":3,\"source\":\"MODEL\",\"type\":\"PLANNER_RESPONSE\",\"status\":\"DONE\",\"created_at\":\"2026-11-21T13:12:14Z\",\"output_tokens\":300,\"tool_calls\":[]}\n",
        )).unwrap();
        let steps = usage_steps(&file).unwrap();
        assert_eq!(steps.len(), 4);
        assert!(steps[0]
            .content
            .as_deref()
            .is_some_and(|c| c.contains("Model Selection")));
        assert!(steps[1].has_tool_calls);
        assert_eq!(steps[1].output_tokens, 250);
        assert!(steps[1].content.is_none());
        assert!(steps[2].content.is_none());
        assert!(!steps[3].has_tool_calls);
    }
}
