//! The block construction helpers shared by the parsers: tool name normalization (section
//! 3.3), preview truncation and title extraction.
//!
//! Only the rules that are independent of a concrete Agent live here; the refinements that
//! need context (the Codex parsed_cmd, the additions and deletions of apply_patch, the Claude
//! structuredPatch, ...) are done by the individual parsers once they have the result of
//! [`normalize_tool`].

use serde_json::Value;

use crate::session_manager::model::ToolKind;

/// The maximum number of lines of a tool result preview
pub const PREVIEW_LINES: usize = 12;
/// The maximum number of characters of a tool result preview
pub const PREVIEW_CHARS: usize = 1200;
/// The maximum number of characters of a tool argument preview (the title / detail already
/// condense the important points, the arguments only need their beginning;
/// the Bash argument preview once took a fifth of the payload in a big session)
pub const INPUT_PREVIEW_CHARS: usize = 400;
/// The maximum number of characters of a thinking body preview
pub const THINKING_PREVIEW_CHARS: usize = 400;
/// The maximum number of characters of a tool title
pub const TITLE_CHARS: usize = 200;
/// A Text block that exceeds this number of characters is only sent as a preview plus a reference (an injected AGENTS.md / developer / task-notification, ...)
pub const INLINE_TEXT_MAX_CHARS: usize = 8 * 1024;
/// The number of preview characters for a long event description such as a compaction summary
pub const EVENT_PREVIEW_CHARS: usize = 400;

/// Which Agent a tool call comes from; the same tool name can be classified differently per Agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSource {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    /// Pi and OpenClaw (isomorphic)
    Pi,
    /// Hermes / Grok Build and the like: inferred from a keyword in the name
    Generic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTool {
    pub kind: ToolKind,
    /// The name of the MCP server (when kind = Mcp)
    pub server: Option<String>,
}

impl NormalizedTool {
    fn of(kind: ToolKind) -> Self {
        Self { kind, server: None }
    }

    fn mcp(server: impl Into<String>) -> Self {
        Self {
            kind: ToolKind::Mcp,
            server: Some(server.into()),
        }
    }
}

/// Normalize the raw tool name into a [`ToolKind`] with the mapping table of section 3.3.
///
/// `namespace` is only passed by the Codex `function_call.namespace` (e.g. `mcp__cua_repl`).
pub fn normalize_tool(
    source: ToolSource,
    raw_name: &str,
    namespace: Option<&str>,
) -> NormalizedTool {
    // MCP: the Codex looks at the namespace, the others at the `mcp__<server>__<tool>` name
    if let Some(server) = namespace.and_then(|ns| ns.strip_prefix("mcp__")) {
        return NormalizedTool::mcp(server.trim_end_matches('_'));
    }
    if let Some((server, _)) = split_mcp_name(raw_name) {
        return NormalizedTool::mcp(server);
    }

    use ToolKind::*;
    let kind = match source {
        ToolSource::Claude => match raw_name {
            "Bash" => Shell,
            "Read" => Read,
            "Grep" | "Glob" | "ToolSearch" | "ListAgents" => Search,
            "Edit" | "MultiEdit" | "NotebookEdit" => Edit,
            "Write" => Write,
            "WebFetch" | "WebSearch" => Web,
            "Agent" | "Task" | "SendMessage" | "TaskStop" => Agent,
            "AskUserQuestion" => Ask,
            "TodoWrite" => Todo,
            _ => Other,
        },
        ToolSource::Codex => match raw_name {
            "exec" | "exec_command" | "shell" | "local_shell" | "write_stdin"
            | "CommandExecution" => Shell,
            "read_file" => Read,
            // apply_patch / FileChange default to Edit; when everything is a new file the parser changes it to Write
            "apply_patch" | "FileChange" => Edit,
            "write_file" => Write,
            "web_search" | "web_search_call" | "open_page" | "WebSearch" => Web,
            "McpToolCall" => Mcp,
            "spawn_agent" | "send_message" | "agent_message" | "SubAgentActivity"
            | "create_thread" => Agent,
            "request_user_input_async" => Ask,
            "update_plan" => Todo,
            name if name.starts_with("web.") => Web,
            _ => Other,
        },
        ToolSource::Gemini => match raw_name {
            "run_shell_command" => Shell,
            "read_file" | "read_many_files" => Read,
            "grep_search" | "glob" | "list_directory" | "search_file_content" => Search,
            "replace" | "edit" => Edit,
            "write_file" => Write,
            "web_fetch" | "google_web_search" => Web,
            "ask_user" => Ask,
            "write_todos" => Todo,
            // To be verified: the MCP tool names of the Gemini CLI look like `server/tool`
            name if name.contains('/') => {
                let server = name.split('/').next().unwrap_or_default();
                return NormalizedTool::mcp(server);
            }
            _ => Other,
        },
        ToolSource::OpenCode => match raw_name {
            "bash" => Shell,
            "read" => Read,
            "grep" | "glob" | "list" | "ast_grep_search" | "codesearch" => Search,
            "edit" | "patch" => Edit,
            "write" => Write,
            "webfetch" | "websearch" => Web,
            "task" | "background_output" | "background_cancel" => Agent,
            "question" => Ask,
            "todowrite" | "todoread" => Todo,
            "invalid" => Other,
            // To be verified: the MCP tool names of OpenCode look like `server_tool` (a name with a `_` that is not in the built-in table)
            name if name.contains('_') => {
                let server = name.split('_').next().unwrap_or_default();
                return NormalizedTool::mcp(server);
            }
            _ => Other,
        },
        ToolSource::Pi => match raw_name {
            "bash" | "bashExecution" => Shell,
            "read" => Read,
            "grep" | "find" | "ls" => Search,
            "edit" => Edit,
            "write" => Write,
            "fetch" => Web,
            _ => Other,
        },
        ToolSource::Generic => generic_kind(raw_name),
    };
    NormalizedTool::of(kind)
}

/// Hermes / Grok Build: there is no fixed tool table, it is inferred from a keyword in the name.
fn generic_kind(raw_name: &str) -> ToolKind {
    let name = raw_name.to_ascii_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| name.contains(k));
    if name.starts_with("web") {
        ToolKind::Web
    } else if has(&["shell", "bash", "exec", "terminal"]) {
        ToolKind::Shell
    } else if name.starts_with("read") {
        ToolKind::Read
    } else if has(&["grep", "glob", "search"]) {
        ToolKind::Search
    } else if has(&["edit", "patch"]) {
        ToolKind::Edit
    } else if name.starts_with("write") {
        ToolKind::Write
    } else {
        ToolKind::Other
    }
}

/// Split `mcp__<server>__<tool>`; returns `None` when the name does not have that shape.
pub fn split_mcp_name(raw_name: &str) -> Option<(&str, &str)> {
    let rest = raw_name.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    (!server.is_empty() && !tool.is_empty()).then_some((server, tool))
}

/// The shell commands are refined by the name of the first program: a leading `rg`/`grep`/`fd`/`find`/`ls` counts as a search, everything else stays a Shell.
pub fn refine_shell_kind(command: &str) -> ToolKind {
    let program = command
        .split_whitespace()
        .next()
        .map(|p| p.rsplit('/').next().unwrap_or(p))
        .unwrap_or_default();
    match program {
        "rg" | "grep" | "fd" | "find" | "ls" => ToolKind::Search,
        _ => ToolKind::Shell,
    }
}

/// Codex `CommandExecution.parsed_cmd[].type` -> kind (`unknown` returns `None` and stays a Shell)
pub fn kind_from_codex_parsed_cmd(parsed_type: &str) -> Option<ToolKind> {
    match parsed_type {
        "read" => Some(ToolKind::Read),
        "search" | "list" | "list_files" => Some(ToolKind::Search),
        _ => None,
    }
}

// ─── Preview ────────────────────────────────────────────────────────────────

/// The truncated preview and the statistics of the full text (the lengths are counted in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub text: String,
    /// The number of characters of the full text
    pub total_len: u32,
    /// The number of lines of the full text (0 for an empty text, a trailing newline does not count as another line)
    pub line_count: u32,
    /// Whether the preview is shorter than the full text (a trailing newline is ignored)
    pub truncated: bool,
}

/// The tool result preview: the first [`PREVIEW_LINES`] lines and at most [`PREVIEW_CHARS`] characters.
pub fn preview(text: &str) -> Preview {
    preview_with(text, PREVIEW_LINES, PREVIEW_CHARS)
}

/// Truncate by characters only (the arguments, the thinking body).
pub fn preview_chars(text: &str, max_chars: usize) -> Preview {
    preview_with(text, usize::MAX, max_chars)
}

pub fn preview_with(text: &str, max_lines: usize, max_chars: usize) -> Preview {
    let total_len = saturating_u32(text.chars().count());
    let line_count = saturating_u32(text.lines().count());

    let mut end = text.len();
    if max_lines > 0 {
        if let Some((idx, _)) = text.match_indices('\n').nth(max_lines - 1) {
            end = idx;
        }
    } else {
        end = 0;
    }
    if let Some((idx, _)) = text[..end].char_indices().nth(max_chars) {
        end = idx;
    }

    let truncated = !text[end..].trim_end_matches(['\r', '\n']).is_empty();
    let kept = text[..end].trim_end_matches(['\r', '\n']);
    Preview {
        text: kept.to_string(),
        total_len,
        line_count,
        truncated,
    }
}

pub(super) fn saturating_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The base64 length -> an estimate of the decoded number of bytes (`ImageRef.size`)
pub fn estimate_base64_size(base64_len: usize) -> u32 {
    saturating_u32(base64_len / 4 * 3)
}

// ─── Title extraction (section 3.3): the output is at most TITLE_CHARS, a multi line value takes the first line and adds `…` ─────────────────

/// Cut it into a single line title: the first non-empty line, with a `…` when it exceeds [`TITLE_CHARS`] or when there are more lines.
pub fn one_line_title(text: &str) -> String {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next().unwrap_or_default().trim_end();
    let more_lines = lines.any(|line| !line.trim().is_empty());
    let mut title: String = first.chars().take(TITLE_CHARS).collect();
    if more_lines || first.chars().count() > TITLE_CHARS {
        title.push('…');
    }
    title
}

/// shell: the first line of the command text. Removing the Codex shell wrapper, taking the cmd out of an exec JS and the like are added by P1b.
pub fn title_shell(command: &str) -> String {
    one_line_title(command)
}

/// read: the title is the path; the `detail` is the line range `:start-end` (the offset counts from 1, the limit is the number of lines).
pub fn title_read(path: &str, offset: Option<u64>, limit: Option<u64>) -> (String, Option<String>) {
    let detail = match (offset, limit) {
        (Some(start), Some(limit)) if limit > 0 => Some(format!(":{start}-{}", start + limit - 1)),
        (Some(start), None) => Some(format!(":{start}")),
        (None, Some(limit)) if limit > 0 => Some(format!(":1-{limit}")),
        _ => None,
    };
    (one_line_title(path), detail)
}

/// search: `pattern in path` (without a path only the pattern is given).
pub fn title_search(pattern: &str, path: Option<&str>) -> String {
    match path.filter(|p| !p.is_empty()) {
        Some(path) => one_line_title(&format!("{pattern} in {path}")),
        None => one_line_title(pattern),
    }
}

/// edit / write: the path.
pub fn title_path(path: &str) -> String {
    one_line_title(path)
}

/// web: the URL or the query; several are joined with `, `.
pub fn title_web(items: &[&str]) -> String {
    one_line_title(&items.join(", "))
}

/// mcp：`server.tool`。
pub fn title_mcp(server: &str, tool: &str) -> String {
    one_line_title(&format!("{server}.{tool}"))
}

/// agent：description / task_name。
pub fn title_agent(description: &str) -> String {
    one_line_title(description)
}

/// ask: the text of the first question.
pub fn title_ask(question: &str) -> String {
    one_line_title(question)
}

/// todo: the number of entries.
pub fn title_todo(count: usize) -> String {
    format!("{count} 项")
}

/// other: the original name.
pub fn title_other(raw_name: &str) -> String {
    one_line_title(raw_name)
}

/// The first non-empty string field of the arguments (the `detail` of mcp / other).
pub fn first_string_field(input: &Value) -> Option<String> {
    input
        .as_object()?
        .values()
        .find_map(|v| v.as_str().filter(|s| !s.trim().is_empty()))
        .map(one_line_title)
}

// ─── Building a block from the JSON arguments (shared by Gemini / OpenCode / Pi / OpenClaw / Hermes / Grok) ────

use crate::session_manager::model::{
    ContentRef, DiffFile, DiffOp, DiffSummary, EventKind, SessionBlock, SessionMessage, ToolStatus,
};

/// The first non-empty string field that is present in the arguments.
pub fn str_field<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .filter(|s| !s.trim().is_empty())
}

/// The first non-negative integer field that is present in the arguments (numbers written as a string are tolerated).
fn u64_field(input: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        let value = input.get(*key)?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
    })
}

/// The `arguments` in the OpenAI shape are a JSON string: parse them into an object/array when possible, otherwise keep them as they are.
pub fn parse_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed @ (Value::Object(_) | Value::Array(_))) => parsed,
            _ => raw.clone(),
        },
        other => other.clone(),
    }
}

/// The preview of the tool arguments: a string as it is, everything else serialized as compact JSON; empty arguments are the empty string.
pub fn input_preview(input: &Value) -> Preview {
    match input {
        Value::Null => preview_chars("", INPUT_PREVIEW_CHARS),
        Value::String(text) => preview_chars(text, INPUT_PREVIEW_CHARS),
        other => preview_chars(&other.to_string(), INPUT_PREVIEW_CHARS),
    }
}

const PATH_KEYS: &[&str] = &[
    "file_path",
    "filePath",
    "path",
    "absolute_path",
    "file",
    "filename",
    "dir_path",
];

/// Extract `(title, detail)` from the arguments by kind (section 3.3). When the matching field
/// cannot be found the title is empty and
/// the projection degrades to `[Tool: name]`.
pub fn title_from_input(
    kind: ToolKind,
    raw_name: &str,
    server: Option<&str>,
    input: &Value,
) -> (String, Option<String>) {
    let detail_of = |keys: &[&str]| str_field(input, keys).map(one_line_title);
    match kind {
        ToolKind::Shell => (
            str_field(input, &["command", "cmd", "script"])
                .map(title_shell)
                .unwrap_or_default(),
            detail_of(&["description"]),
        ),
        ToolKind::Read => {
            if let Some(path) = str_field(input, PATH_KEYS) {
                title_read(
                    path,
                    u64_field(input, &["offset", "start_line"]),
                    u64_field(input, &["limit"]),
                )
            } else {
                let paths: Vec<&str> = input
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|items| items.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                (title_web(&paths), None)
            }
        }
        ToolKind::Search => {
            let path = str_field(input, &["path", "dir_path", "directory", "cwd"]);
            match str_field(input, &["pattern", "query", "regex", "glob", "include"]) {
                Some(pattern) => (title_search(pattern, path), None),
                None => (path.map(title_path).unwrap_or_default(), None),
            }
        }
        ToolKind::Edit | ToolKind::Write => (
            str_field(input, PATH_KEYS)
                .map(title_path)
                .unwrap_or_default(),
            None,
        ),
        ToolKind::Web => {
            let mut items: Vec<&str> = ["urls", "queries"]
                .iter()
                .filter_map(|key| input.get(*key).and_then(Value::as_array))
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            if items.is_empty() {
                items.extend(str_field(input, &["url", "query", "prompt"]));
            }
            (title_web(&items), None)
        }
        ToolKind::Mcp => {
            let server = server.unwrap_or_default();
            let tool = split_mcp_name(raw_name)
                .map(|(_, tool)| tool)
                .or_else(|| {
                    raw_name
                        .strip_prefix(server)
                        .map(|rest| rest.trim_start_matches(['_', '/', '.']))
                })
                .filter(|tool| !tool.is_empty())
                .unwrap_or(raw_name);
            (title_mcp(server, tool), first_string_field(input))
        }
        ToolKind::Agent => (
            str_field(input, &["description", "task_name", "prompt", "message"])
                .map(title_agent)
                .unwrap_or_default(),
            detail_of(&["subagent_type", "agent", "model"]),
        ),
        ToolKind::Ask => {
            let question = input
                .get("questions")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|first| {
                    first
                        .get("question")
                        .and_then(Value::as_str)
                        .or_else(|| first.as_str())
                })
                .or_else(|| str_field(input, &["question", "prompt"]));
            (question.map(title_ask).unwrap_or_default(), None)
        }
        ToolKind::Todo => {
            let count = input.get("todos").and_then(Value::as_array).map(Vec::len);
            (count.map(title_todo).unwrap_or_default(), None)
        }
        ToolKind::Other => (title_other(raw_name), first_string_field(input)),
    }
}

/// The number of lines of the text (0 for an empty text).
fn text_lines(text: &str) -> u32 {
    saturating_u32(text.lines().count())
}

/// Count the added and removed lines of a unified diff (or of the `+12 xxx` form with line numbers), skipping the `+++`/`---` file headers.
pub fn count_diff_lines(diff: &str) -> (u32, u32) {
    let (mut added, mut removed) = (0u32, 0u32);
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added = added.saturating_add(1);
        } else if line.starts_with('-') {
            removed = removed.saturating_add(1);
        }
    }
    (added, removed)
}

/// The summary of the changes of a single file.
pub fn single_file_diff(path: &str, op: DiffOp, added: u32, removed: u32) -> DiffSummary {
    DiffSummary {
        files: vec![DiffFile {
            path: path.to_string(),
            op,
            added,
            removed,
        }],
        added,
        removed,
        full: None,
    }
}

/// Estimate the changes of an Edit/Write from the arguments alone (the `old`/`new` strings are
/// counted by lines, `edits[]` is summed up, `content` counts as an addition).
/// When the tool provides its own diff, that one wins.
pub fn diff_from_input(kind: ToolKind, input: &Value) -> Option<DiffSummary> {
    let path = str_field(input, PATH_KEYS)?;
    let pair = |item: &Value| -> Option<(u32, u32)> {
        let old = str_field_allow_empty(item, &["old_string", "oldString", "old_str", "oldText"])?;
        let new = str_field_allow_empty(item, &["new_string", "newString", "new_str", "newText"])?;
        Some((text_lines(new), text_lines(old)))
    };
    match kind {
        ToolKind::Edit => {
            let (added, removed) = if let Some(edits) = input.get("edits").and_then(Value::as_array)
            {
                edits
                    .iter()
                    .filter_map(pair)
                    .fold((0u32, 0u32), |(a, r), (x, y)| {
                        (a.saturating_add(x), r.saturating_add(y))
                    })
            } else {
                pair(input)?
            };
            Some(single_file_diff(path, DiffOp::Update, added, removed))
        }
        ToolKind::Write => {
            let content = str_field_allow_empty(input, &["content", "contents", "text"])?;
            Some(single_file_diff(path, DiffOp::Add, text_lines(content), 0))
        }
        _ => None,
    }
}

fn str_field_allow_empty<'a>(input: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
}

/// The `tool_calls[]` in the OpenAI shape (`{id, function{name, arguments}}`, the flat
/// `{id, name, arguments}` is tolerated as well)
/// -> ToolCall. `arguments_ref(pointer)` builds a reference to the full arguments, `pointer` is
/// the JSON Pointer of the arguments inside the `tool_calls`
/// array (`/{i}/function/arguments` or `/{i}/arguments` for the flat shape).
pub(super) fn openai_tool_calls(
    calls: Option<&Value>,
    arguments_ref: impl Fn(String) -> Option<ContentRef>,
) -> Vec<SessionBlock> {
    calls
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(i, call)| {
            let name = call
                .pointer("/function/name")
                .or_else(|| call.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let id = call.get("id").and_then(Value::as_str).unwrap_or_default();
            let (pointer, raw) = match call.pointer("/function/arguments") {
                Some(raw) => (format!("/{i}/function/arguments"), Some(raw)),
                None => (format!("/{i}/arguments"), call.get("arguments")),
            };
            let input = raw.map(parse_arguments).unwrap_or(Value::Null);
            tool_call_block(ToolSource::Generic, id, name, &input, || {
                arguments_ref(pointer)
            })
        })
        .collect()
}

/// Build a `ToolCall` from the arguments: normalize the kind, extract the title, preview the
/// arguments; when the arguments are too long `input_full` builds a reference.
/// For an Edit/Write the diff is estimated from the arguments first and the parser overwrites
/// it with more accurate data.
pub fn tool_call_block(
    source: ToolSource,
    id: impl Into<String>,
    raw_name: &str,
    input: &Value,
    input_full: impl FnOnce() -> Option<ContentRef>,
) -> SessionBlock {
    let NormalizedTool { kind, server } = normalize_tool(source, raw_name, None);
    let (title, detail) = title_from_input(kind, raw_name, server.as_deref(), input);
    let preview = input_preview(input);
    SessionBlock::ToolCall {
        id: id.into(),
        raw_name: raw_name.to_string(),
        kind,
        title,
        detail,
        server,
        input_preview: preview.text,
        input_total_len: preview.total_len,
        input_full: if preview.truncated {
            input_full()
        } else {
            None
        },
        diff: diff_from_input(kind, input),
        by_user: false,
    }
}

/// Build a `ToolResult`: a preview of 12 lines / 1200 characters, beyond that `full` builds a
/// reference.
/// `exit_code` / `duration_ms` / `images` / `saved_path` are added by the parser as needed.
pub fn tool_result_block(
    call_id: impl Into<String>,
    status: ToolStatus,
    text: &str,
    full: impl FnOnce() -> Option<ContentRef>,
) -> SessionBlock {
    let p = preview(text);
    SessionBlock::ToolResult {
        call_id: call_id.into(),
        status,
        preview: p.text,
        total_len: p.total_len,
        line_count: p.line_count,
        truncated: p.truncated,
        full: if p.truncated { full() } else { None },
        exit_code: None,
        duration_ms: None,
        images: Vec::new(),
        saved_path: None,
    }
}

/// Build a `Thinking`: the body is truncated to [`THINKING_PREVIEW_CHARS`], beyond that `full`
/// builds a reference;
/// an empty body counts as not visible (redacted).
pub fn thinking_block(
    text: &str,
    summary: Option<String>,
    duration_ms: Option<u64>,
    full: impl FnOnce() -> Option<ContentRef>,
) -> SessionBlock {
    let p = preview_chars(text.trim_end(), THINKING_PREVIEW_CHARS);
    SessionBlock::Thinking {
        redacted: text.trim().is_empty(),
        text: p.text,
        summary,
        duration_ms,
        full: if p.truncated { full() } else { None },
    }
}

/// Build a `Text`: when it exceeds [`INLINE_TEXT_MAX_CHARS`] and a reference can be given only
/// the preview (12 lines / 1200 characters) plus `full` is placed.
/// It is only used for the content that is collapsed by default (the injected texts); a real
/// question and the final reply render the full Markdown and do not go through here.
pub fn large_text_block(
    text: impl Into<String>,
    full: impl FnOnce() -> Option<ContentRef>,
) -> SessionBlock {
    let text = text.into();
    if text.chars().nth(INLINE_TEXT_MAX_CHARS).is_none() {
        return SessionBlock::text(text);
    }
    match full() {
        Some(full) => SessionBlock::Text {
            text: preview(&text).text,
            full: Some(full),
        },
        None => SessionBlock::text(text),
    }
}

/// Build an `Event` with a long description (a compaction summary): when it exceeds
/// [`EVENT_PREVIEW_CHARS`] and a reference can be given only the preview plus `full` is placed;
/// without a reference it falls back to a preview with a `…`, so that the whole summary does
/// not end up in the payload.
pub fn summary_event_block(
    kind: EventKind,
    text: &str,
    full: impl FnOnce() -> Option<ContentRef>,
) -> SessionBlock {
    let text = text.trim();
    let p = preview_chars(text, EVENT_PREVIEW_CHARS);
    if !p.truncated {
        return SessionBlock::event(kind, Some(p.text), None);
    }
    match full() {
        Some(full) => SessionBlock::Event {
            kind,
            text: Some(p.text),
            url: None,
            full: Some(full),
        },
        None => SessionBlock::event(kind, Some(format!("{}…", p.text)), None),
    }
}

/// Whether this message opens a new turn: a user message that is not injected and carries real
/// input -- non-empty text, an image or a slash command.
/// A user message that is only an interruption event, a command the user ran themselves
/// (the Claude `!cmd`, the Pi bashExecution) or a tool result
/// does not open a new turn and belongs to the current turn. The messages of the old format
/// (without blocks) are judged by whether the `content` is non-empty.
pub fn starts_turn(message: &SessionMessage) -> bool {
    if message.role != "user" || message.injected {
        return false;
    }
    if message.blocks.is_empty() {
        return !message.content.trim().is_empty();
    }
    message.blocks.iter().any(|block| match block {
        SessionBlock::Text { text, .. } => !text.trim().is_empty(),
        SessionBlock::Image { .. } => true,
        SessionBlock::Event { kind, .. } => *kind == EventKind::SlashCommand,
        _ => false,
    })
}

/// Shared by the parsers that have no native turn concept (section 4): `t{n}` is generated by
/// counting up [`starts_turn`],
/// the content before the first question belongs to `t0`. The messages that already have a
/// `turn_id` stay unchanged.
pub fn assign_turn_ids(messages: &mut [SessionMessage]) {
    let mut turn = 0u32;
    for message in messages {
        if starts_turn(message) {
            turn += 1;
        }
        if message.turn_id.is_none() {
            message.turn_id = Some(format!("t{turn}"));
        }
    }
}

// ─── Diff statistics ───────────────────────────────────────────────────────────

/// Estimate the `(+added, -removed)` line counts from the text before and after the
/// replacement: after the identical leading and trailing lines are removed,
/// the remaining old lines count as removals and the new ones as additions. It is used when
/// there is no structured patch (for example for the arguments of a Claude Edit).
pub fn line_change_counts(old: &str, new: &str) -> (u32, u32) {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let prefix = old_lines
        .iter()
        .zip(&new_lines)
        .take_while(|(a, b)| a == b)
        .count();
    let suffix = old_lines[prefix..]
        .iter()
        .rev()
        .zip(new_lines[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    (
        saturating_u32(new_lines.len() - prefix - suffix),
        saturating_u32(old_lines.len() - prefix - suffix),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn line_change_counts_ignores_shared_context() {
        assert_eq!(line_change_counts("a\nb\nc", "a\nb\nx\nc"), (1, 0));
        assert_eq!(line_change_counts("a\nb", "a\nc\nd"), (2, 1));
        assert_eq!(line_change_counts("", "x\ny"), (2, 0));
        assert_eq!(line_change_counts("same", "same"), (0, 0));
    }

    fn kind(source: ToolSource, name: &str) -> ToolKind {
        normalize_tool(source, name, None).kind
    }

    #[test]
    fn normalize_tool_claude() {
        use ToolKind::*;
        let cases = [
            ("Bash", Shell),
            ("Read", Read),
            ("Grep", Search),
            ("Glob", Search),
            ("ToolSearch", Search),
            ("Edit", Edit),
            ("MultiEdit", Edit),
            ("NotebookEdit", Edit),
            ("Write", Write),
            ("WebFetch", Web),
            ("WebSearch", Web),
            ("Agent", Agent),
            ("Task", Agent),
            ("AskUserQuestion", Ask),
            ("TodoWrite", Todo),
            ("Skill", Other),
            ("EnterWorktree", Other),
        ];
        for (name, expected) in cases {
            assert_eq!(kind(ToolSource::Claude, name), expected, "{name}");
        }
        assert_eq!(
            normalize_tool(ToolSource::Claude, "mcp__claude-in-chrome__computer", None),
            NormalizedTool::mcp("claude-in-chrome")
        );
    }

    #[test]
    fn normalize_tool_codex() {
        use ToolKind::*;
        for (name, expected) in [
            ("exec", Shell),
            ("exec_command", Shell),
            ("write_stdin", Shell),
            ("read_file", Read),
            ("apply_patch", Edit),
            ("write_file", Write),
            ("web_search", Web),
            ("web.search", Web),
            ("spawn_agent", Agent),
            ("request_user_input_async", Ask),
            ("update_plan", Todo),
            ("sleep", Other),
            ("js", Other),
        ] {
            assert_eq!(kind(ToolSource::Codex, name), expected, "{name}");
        }
        // A namespace that starts with mcp__ -> MCP, the server is the name without the prefix
        assert_eq!(
            normalize_tool(ToolSource::Codex, "js", Some("mcp__cua_repl")),
            NormalizedTool::mcp("cua_repl")
        );
        // A non-MCP namespace does not matter
        assert_eq!(kind(ToolSource::Codex, "sleep"), Other);
        assert_eq!(
            normalize_tool(ToolSource::Codex, "send_message", Some("collaboration")).kind,
            Agent
        );
    }

    #[test]
    fn normalize_tool_gemini_opencode_pi_generic() {
        use ToolKind::*;
        assert_eq!(kind(ToolSource::Gemini, "run_shell_command"), Shell);
        assert_eq!(kind(ToolSource::Gemini, "read_many_files"), Read);
        assert_eq!(kind(ToolSource::Gemini, "list_directory"), Search);
        assert_eq!(kind(ToolSource::Gemini, "replace"), Edit);
        assert_eq!(kind(ToolSource::Gemini, "google_web_search"), Web);
        assert_eq!(kind(ToolSource::Gemini, "write_todos"), Todo);
        assert_eq!(
            normalize_tool(ToolSource::Gemini, "github/search_issues", None),
            NormalizedTool::mcp("github")
        );

        assert_eq!(kind(ToolSource::OpenCode, "bash"), Shell);
        assert_eq!(kind(ToolSource::OpenCode, "ast_grep_search"), Search);
        assert_eq!(kind(ToolSource::OpenCode, "patch"), Edit);
        assert_eq!(kind(ToolSource::OpenCode, "background_output"), Agent);
        assert_eq!(kind(ToolSource::OpenCode, "question"), Ask);
        assert_eq!(kind(ToolSource::OpenCode, "todowrite"), Todo);
        assert_eq!(kind(ToolSource::OpenCode, "invalid"), Other);
        assert_eq!(
            normalize_tool(ToolSource::OpenCode, "context7_resolve-library-id", None),
            NormalizedTool::mcp("context7")
        );

        assert_eq!(kind(ToolSource::Pi, "bashExecution"), Shell);
        assert_eq!(kind(ToolSource::Pi, "ls"), Search);
        assert_eq!(kind(ToolSource::Pi, "fetch"), Web);
        assert_eq!(kind(ToolSource::Pi, "custom"), Other);

        assert_eq!(kind(ToolSource::Generic, "terminal"), Shell);
        assert_eq!(kind(ToolSource::Generic, "read_file"), Read);
        assert_eq!(kind(ToolSource::Generic, "search_files"), Search);
        assert_eq!(kind(ToolSource::Generic, "web_search"), Web);
        assert_eq!(kind(ToolSource::Generic, "patch"), Edit);
        assert_eq!(kind(ToolSource::Generic, "write_file"), Write);
        assert_eq!(kind(ToolSource::Generic, "vision_analyze"), Other);
    }

    #[test]
    fn split_mcp_name_requires_server_and_tool() {
        assert_eq!(split_mcp_name("mcp__slack__post"), Some(("slack", "post")));
        assert_eq!(split_mcp_name("mcp__slack"), None);
        assert_eq!(split_mcp_name("mcp____x"), None);
        assert_eq!(split_mcp_name("Bash"), None);
    }

    #[test]
    fn refine_shell_kind_detects_search_programs() {
        assert_eq!(refine_shell_kind("rg -n foo src"), ToolKind::Search);
        assert_eq!(
            refine_shell_kind("/usr/bin/find . -name x"),
            ToolKind::Search
        );
        assert_eq!(refine_shell_kind("cargo build"), ToolKind::Shell);
        assert_eq!(refine_shell_kind(""), ToolKind::Shell);
        assert_eq!(kind_from_codex_parsed_cmd("read"), Some(ToolKind::Read));
        assert_eq!(kind_from_codex_parsed_cmd("list"), Some(ToolKind::Search));
        assert_eq!(kind_from_codex_parsed_cmd("unknown"), None);
    }

    #[test]
    fn preview_keeps_short_text_untouched() {
        let p = preview("a\nb\n");
        assert_eq!(p.text, "a\nb");
        assert_eq!((p.total_len, p.line_count, p.truncated), (4, 2, false));

        let empty = preview("");
        assert_eq!(
            (empty.text.as_str(), empty.line_count, empty.truncated),
            ("", 0, false)
        );
    }

    #[test]
    fn preview_cuts_at_line_limit() {
        let exact: String = (1..=12).map(|i| format!("l{i}\n")).collect();
        let p = preview(&exact);
        assert!(!p.truncated, "恰好 12 行不算截断");
        assert_eq!(p.line_count, 12);

        let over: String = (1..=13).map(|i| format!("l{i}\n")).collect();
        let p = preview(&over);
        assert!(p.truncated);
        assert_eq!(p.text.lines().count(), PREVIEW_LINES);
        assert!(p.text.ends_with("l12"));
        assert_eq!(p.line_count, 13);
    }

    #[test]
    fn preview_cuts_at_char_limit_on_char_boundary() {
        let exact = "字".repeat(PREVIEW_CHARS);
        let p = preview(&exact);
        assert!(!p.truncated);
        assert_eq!(p.total_len as usize, PREVIEW_CHARS);

        let over = "字".repeat(PREVIEW_CHARS + 1);
        let p = preview(&over);
        assert!(p.truncated);
        assert_eq!(p.text.chars().count(), PREVIEW_CHARS);
        assert_eq!(p.total_len as usize, PREVIEW_CHARS + 1);
        assert_eq!(p.line_count, 1);

        let p = preview_chars("abcdef", 3);
        assert_eq!((p.text.as_str(), p.truncated), ("abc", true));
    }

    #[test]
    fn titles_are_single_line_and_bounded() {
        assert_eq!(title_shell("cargo build"), "cargo build");
        assert_eq!(title_shell("cd a\ncargo test"), "cd a…");
        let long = "x".repeat(TITLE_CHARS + 5);
        let t = title_shell(&long);
        assert_eq!(t.chars().count(), TITLE_CHARS + 1);
        assert!(t.ends_with('…'));

        assert_eq!(
            title_read("/a/b.rs", Some(10), Some(31)),
            ("/a/b.rs".to_string(), Some(":10-40".to_string()))
        );
        assert_eq!(title_read("/a/b.rs", None, None).1, None);
        assert_eq!(title_search("foo", Some("src")), "foo in src");
        assert_eq!(title_search("foo", None), "foo");
        assert_eq!(title_web(&["a", "b"]), "a, b");
        assert_eq!(title_mcp("slack", "post"), "slack.post");
        assert_eq!(title_todo(3), "3 项");
        assert_eq!(
            first_string_field(&json!({"n": 1, "q": "", "url": "https://x"})),
            Some("https://x".to_string())
        );
        assert_eq!(estimate_base64_size(8), 6);
    }

    #[test]
    fn title_from_input_covers_each_kind() {
        use ToolKind::*;
        let t = |kind, name: &str, input: Value| title_from_input(kind, name, None, &input);
        assert_eq!(
            t(
                Shell,
                "bash",
                json!({"command": "ls -la", "description": "List"})
            ),
            ("ls -la".into(), Some("List".into()))
        );
        assert_eq!(
            t(
                Read,
                "read",
                json!({"filePath": "/a.rs", "offset": 10, "limit": 31})
            ),
            ("/a.rs".into(), Some(":10-40".into()))
        );
        assert_eq!(
            t(Read, "read_many_files", json!({"paths": ["a", "b"]})).0,
            "a, b"
        );
        assert_eq!(
            t(Search, "grep", json!({"pattern": "foo", "path": "src"})).0,
            "foo in src"
        );
        assert_eq!(
            t(Search, "list_directory", json!({"dir_path": "/p"})).0,
            "/p"
        );
        assert_eq!(t(Edit, "edit", json!({"file_path": "/p/a"})).0, "/p/a");
        assert_eq!(
            t(Web, "webfetch", json!({"url": "https://x"})).0,
            "https://x"
        );
        assert_eq!(
            t(
                Agent,
                "task",
                json!({"description": "Find", "subagent_type": "explore"})
            ),
            ("Find".into(), Some("explore".into()))
        );
        assert_eq!(
            t(
                Ask,
                "question",
                json!({"questions": [{"question": "删吗？"}]})
            )
            .0,
            "删吗？"
        );
        assert_eq!(t(Todo, "todowrite", json!({"todos": [1, 2]})).0, "2 项");
        assert_eq!(
            t(Other, "skill", json!({"name": "commit"})),
            ("skill".into(), Some("commit".into()))
        );
        assert_eq!(t(Shell, "bash", json!({})).0, "");
        assert_eq!(
            title_from_input(
                Mcp,
                "context7_resolve-library-id",
                Some("context7"),
                &json!({})
            )
            .0,
            "context7.resolve-library-id"
        );
    }

    #[test]
    fn diff_helpers_count_lines() {
        assert_eq!(
            count_diff_lines("--- a\n+++ b\n@@\n-x\n+y\n+z\n ctx"),
            (2, 1)
        );
        assert_eq!(count_diff_lines(" 10 a\n-11 x\n+11 y"), (1, 1));

        let edit = diff_from_input(
            ToolKind::Edit,
            &json!({"path": "/a", "edits": [{"oldText": "a\nb", "newText": "c"}, {"oldText": "d", "newText": "e\nf"}]}),
        )
        .unwrap();
        assert_eq!((edit.added, edit.removed), (3, 3));
        let write = diff_from_input(
            ToolKind::Write,
            &json!({"file_path": "/a", "content": "1\n2\n"}),
        )
        .unwrap();
        assert_eq!(
            (write.added, write.removed, write.files[0].op),
            (2, 0, DiffOp::Add)
        );
        assert!(diff_from_input(ToolKind::Edit, &json!({"path": "/a"})).is_none());
    }

    #[test]
    fn arguments_and_turns() {
        assert_eq!(parse_arguments(&json!("{\"a\":1}")), json!({"a": 1}));
        assert_eq!(parse_arguments(&json!("not json")), json!("not json"));
        assert_eq!(parse_arguments(&json!("42")), json!("42"));

        let msg = |role: &str, injected: bool, blocks: Vec<SessionBlock>| SessionMessage {
            role: role.into(),
            injected,
            blocks,
            ..SessionMessage::default()
        };
        let q = || vec![SessionBlock::text("q")];
        let by_user = tool_call_block(ToolSource::Pi, "u1", "bash", &json!({}), || None);
        let mut messages = vec![
            msg("system", false, q()),
            msg("user", false, q()),
            msg("assistant", false, q()),
            msg("user", true, q()),
            // An interruption and a command the user ran themselves do not open a new turn
            msg(
                "user",
                false,
                vec![SessionBlock::event(EventKind::Aborted, None, None)],
            ),
            msg("user", false, vec![by_user]),
            msg("user", false, q()),
            msg(
                "user",
                false,
                vec![SessionBlock::event(
                    EventKind::SlashCommand,
                    Some("/compact".into()),
                    None,
                )],
            ),
        ];
        assign_turn_ids(&mut messages);
        let turns: Vec<_> = messages
            .iter()
            .map(|m| m.turn_id.clone().unwrap())
            .collect();
        assert_eq!(turns, ["t0", "t1", "t1", "t1", "t1", "t1", "t2", "t3"]);
    }
}
