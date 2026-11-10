//! The unified data model of the session reader page.
//!
//! The provider parsers turn the source records into [`SessionMessage`]: the structured content
//! goes into `blocks` and `content` is the plain text projection derived from the blocks
//! ([`project_content`]), so that the old search / copy / outline logic keeps working. The
//! frontend types live in `src/types.ts` and the fields correspond one to one;
//! `tests/fixtures/sessions/*.messages.json` are the contract samples shared by both sides.

use serde::{Deserialize, Serialize, Serializer};

/// The serialization rules are in the hand written [`Serialize`] implementation below: `content` is not sent when there are blocks.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    /// user | assistant | tool | system (the old values are kept, the frontend already has a roleLabel)
    pub role: String,
    /// The plain text projection (the rules are in [`project_content`]), used inside the backend
    /// (outline previews and the like); it is not serialized when the blocks are not empty, the
    /// frontend derives it from the blocks and only an old backend (without blocks) falls back to it
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    /// The id of the source record (Claude uuid / Codex payload.id / OpenCode message id / Pi entry id)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The identifier of a turn: the Codex turn_id, for the others the parser generates `t{n}` by counting up the user messages
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Injected content (AGENTS.md, environment_context, system-reminder, the developer role, ...)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub injected: bool,
    /// Empty means the parser has not produced any structured content yet, the frontend falls back to `content`
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocks: Vec<SessionBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<MessageMeta>,
}

/// `content` is omitted when there are blocks (it is only a projection of the blocks and takes about a quarter of the payload in a big session).
impl Serialize for SessionMessage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            role: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            content: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ts: Option<i64>,
            #[serde(skip_serializing_if = "Option::is_none")]
            id: Option<&'a str>,
            #[serde(skip_serializing_if = "Option::is_none")]
            turn_id: Option<&'a str>,
            #[serde(skip_serializing_if = "std::ops::Not::not")]
            injected: bool,
            #[serde(skip_serializing_if = "<[SessionBlock]>::is_empty")]
            blocks: &'a [SessionBlock],
            #[serde(skip_serializing_if = "Option::is_none")]
            meta: Option<&'a MessageMeta>,
        }
        Wire {
            role: &self.role,
            content: self.blocks.is_empty().then_some(self.content.as_str()),
            ts: self.ts,
            id: self.id.as_deref(),
            turn_id: self.turn_id.as_deref(),
            injected: self.injected,
            blocks: &self.blocks,
            meta: self.meta.as_ref(),
        }
        .serialize(serializer)
    }
}

impl SessionMessage {
    /// Build the message from the blocks, `content` is derived with [`project_content`].
    pub fn from_blocks(
        role: impl Into<String>,
        ts: Option<i64>,
        blocks: Vec<SessionBlock>,
    ) -> Self {
        Self {
            role: role.into(),
            content: project_content(&blocks),
            ts,
            blocks,
            ..Self::default()
        }
    }

    /// Whether there is nothing displayable at all (replaces the old `content.trim().is_empty()` check)
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty() && self.content.trim().is_empty()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageMeta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
}

/// A block of the message content. The `type` tag is snake_case, the fields inside a block are camelCase (like the frontend).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionBlock {
    Text {
        /// The body; with `full` it is only a preview (a huge injected text, see `blocks::text_block`)
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full: Option<ContentRef>,
    },
    Thinking {
        /// The preview of the visible body (it may be empty: Claude only has a signature / Codex only has encrypted_content)
        text: String,
        /// A short summary like the Codex summary_text or the Gemini thoughts.subject
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// The body is not visible (encrypted / only a signature left)
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        redacted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// A reference is given when the body exceeds the preview length, the full text is fetched on demand
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full: Option<ContentRef>,
    },
    ToolCall {
        /// The pairing key: Claude tool_use.id / Codex call_id / OpenCode callID / Pi toolCall.id
        id: String,
        raw_name: String,
        kind: ToolKind,
        /// The title body (command, path, search term, URL, question, ...), already condensed by kind, at most 200 characters
        title: String,
        /// Secondary information (workdir, line range, description, agent description, ...)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// The name of the MCP server (when kind = mcp)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server: Option<String>,
        /// The preview of the arguments (at most 1200 characters) and the reference to the full text; `input_total_len` is the character count of the full text
        input_preview: String,
        input_total_len: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_full: Option<ContentRef>,
        /// A summary of the file changes (Edit/Write/apply_patch/FileChange)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<DiffSummary>,
        /// A command the user ran themselves (Pi bashExecution, Codex "You ran")
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        by_user: bool,
    },
    ToolResult {
        /// The matching ToolCall.id; empty means the source data has no pairing information (the frontend shows it as a generic "tool output" step)
        call_id: String,
        status: ToolStatus,
        /// The preview: the first 12 lines and at most 1200 characters
        preview: String,
        /// The character count and the line count of the full text
        total_len: u32,
        line_count: u32,
        truncated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full: Option<ContentRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        /// The images that came with the result (MCP screenshots, images of a Read)
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        images: Vec<ImageRef>,
        /// The full path of the file the tool wrote to disk (Claude persistedOutputPath / Pi fullOutputPath)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        saved_path: Option<String>,
    },
    Image {
        image: ImageRef,
    },
    Event {
        kind: EventKind,
        /// The explanatory text; with `full` it is only a preview (a compressed summary and other long texts)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        full: Option<ContentRef>,
    },
    /// OpenCode step-start / step-finish; the other agents do not produce them
    Step {
        phase: StepPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cost_usd: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl SessionBlock {
    /// A Text block of the full body
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text {
            text: text.into(),
            full: None,
        }
    }

    /// An Event block without a reference
    pub fn event(kind: EventKind, text: Option<String>, url: Option<String>) -> Self {
        Self::Event {
            kind,
            text,
            url,
            full: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolKind {
    Shell,
    Read,
    Search,
    Edit,
    Write,
    Web,
    Mcp,
    Agent,
    Ask,
    Todo,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Success,
    Error,
    Interrupted,
    Pending,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// The user interrupted / turn_aborted
    Aborted,
    /// The context was compacted (Codex compaction/compacted, Pi compaction, Claude /compact)
    Compaction,
    /// Pi model_change、Codex thread_settings_applied
    ModelChange,
    /// Pi thinking_level_change
    ThinkingLevel,
    /// The Claude system.stop_hook_summary (only produced when there is an error)
    Hook,
    /// Claude pr-link
    PrLink,
    /// Claude <command-name>
    SlashCommand,
    /// Gemini info
    Info,
    /// Gemini error
    Error,
    /// Codex agent_message / SubAgentActivity
    SubAgent,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepPhase {
    Start,
    Finish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffSummary {
    pub files: Vec<DiffFile>,
    pub added: u32,
    pub removed: u32,
    /// A reference to the full unified diff (it can be expanded to see the diff)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<ContentRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffFile {
    pub path: String,
    pub op: DiffOp,
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffOp {
    Add,
    Update,
    Delete,
    Rename,
}

/// A "fetch on demand" reference for large content. The frontend passes it back unchanged and the backend validates it before reading.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ContentRef {
    /// JSONL: the byte range of the line plus a JSON Pointer (RFC 6901) inside the line
    Jsonl {
        offset: u64,
        len: u32,
        pointer: String,
    },
    /// SQLite: table + primary key + column + a JSON Pointer inside the column (an empty string means the whole column, used when the column itself is not JSON)
    Sqlite {
        table: String,
        id: String,
        column: String,
        pointer: String,
    },
    /// A standalone JSON file (the OpenCode file storage part): the path relative to the storage root the session sourcePath belongs to
    File { rel_path: String, pointer: String },
    /// The full output file the tool wrote to disk (only inside the directory attached to the session is allowed)
    Sidecar { rel_path: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageRef {
    pub source: ImageSource,
    pub media_type: String,
    /// An estimate of the number of decoded bytes (base64 length x 3/4); for a local file the file size, unknown is 0
    pub size: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum ImageSource {
    /// base64 inlined in the source record (Claude image / tool_result image, Codex input_image data URL, Pi toolResult image)
    Inline { content: ContentRef },
    /// A local file (Codex ImageView, Claude tool-results/*.jpg, a file:// inside Markdown)
    LocalFile { path: String },
}

/// Chunked transfer (section 5): `stream_session_messages` sends them one after another through a Channel.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum TranscriptChunk {
    /// The first packet: the total count and the turn index, so that the frontend can draw the skeleton first
    Header {
        total: usize,
        turns: Vec<TurnIndex>,
        cached: bool,
        parse_ms: u64,
    },
    Messages {
        start: usize,
        messages: Vec<SessionMessage>,
    },
    Done {
        payload_bytes: u64,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnIndex {
    pub turn_id: String,
    pub first_message_index: usize,
    pub last_message_index: usize,
    /// At most 80 characters
    pub question_preview: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ts: Option<i64>,
    pub step_count: u32,
    pub error_count: u32,
    pub has_final_reply: bool,
    pub aborted: bool,
}

/// Derive the plain text `content` from the blocks (section 3.4):
///
/// - `Text` -> the original text
/// - `ToolCall` -> `[Tool: {raw_name}] {title}` (the old prefix is kept so that old frontends
///   still recognize it as a tool line)
/// - `ToolResult` -> the preview (not the full text)
/// - `Image` -> `[Image: {media_type} {size}]`
/// - `Event` -> `text` (skipped when there is none)
/// - `Thinking` / `Step` -> do not end up in it
///
/// The parts are joined with a blank line; empty parts are skipped.
pub fn project_content(blocks: &[SessionBlock]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        let part = match block {
            SessionBlock::Text { text, .. } => text.clone(),
            SessionBlock::ToolCall {
                raw_name, title, ..
            } => {
                if title.is_empty() {
                    format!("[Tool: {raw_name}]")
                } else {
                    format!("[Tool: {raw_name}] {title}")
                }
            }
            SessionBlock::ToolResult { preview, .. } => preview.clone(),
            SessionBlock::Image { image } => {
                format!("[Image: {} {}]", image.media_type, image.size)
            }
            SessionBlock::Event { text, .. } => text.clone().unwrap_or_default(),
            SessionBlock::Thinking { .. } | SessionBlock::Step { .. } => continue,
        };
        if !part.trim().is_empty() {
            parts.push(part);
        }
    }
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(s: &str) -> SessionBlock {
        SessionBlock::text(s)
    }

    fn tool_call(raw_name: &str, title: &str) -> SessionBlock {
        SessionBlock::ToolCall {
            id: "call_1".into(),
            raw_name: raw_name.into(),
            kind: ToolKind::Shell,
            title: title.into(),
            detail: None,
            server: None,
            input_preview: "{}".into(),
            input_total_len: 2,
            input_full: None,
            diff: None,
            by_user: false,
        }
    }

    #[test]
    fn project_content_follows_projection_rules() {
        let blocks = vec![
            SessionBlock::Thinking {
                text: "secret plan".into(),
                summary: None,
                redacted: false,
                duration_ms: None,
                full: None,
            },
            text("先看看构建输出。"),
            tool_call("Bash", "cargo build"),
            tool_call("Write", ""),
            SessionBlock::ToolResult {
                call_id: "call_1".into(),
                status: ToolStatus::Error,
                preview: "error[E0433]".into(),
                total_len: 12,
                line_count: 1,
                truncated: false,
                full: None,
                exit_code: Some(101),
                duration_ms: None,
                images: vec![],
                saved_path: None,
            },
            SessionBlock::Image {
                image: ImageRef {
                    source: ImageSource::LocalFile {
                        path: "/tmp/a.png".into(),
                    },
                    media_type: "image/png".into(),
                    size: 2048,
                    alt: None,
                },
            },
            SessionBlock::event(EventKind::Aborted, None, None),
            SessionBlock::Step {
                phase: StepPhase::Finish,
                tokens: Some(10),
                cost_usd: None,
                reason: None,
            },
        ];
        assert_eq!(
            project_content(&blocks),
            "先看看构建输出。\n\n[Tool: Bash] cargo build\n\n[Tool: Write]\n\nerror[E0433]\n\n[Image: image/png 2048]"
        );
    }

    #[test]
    fn serializes_with_snake_case_tags_and_camel_case_fields() {
        let msg = SessionMessage::from_blocks(
            "assistant",
            Some(1),
            vec![SessionBlock::ToolResult {
                call_id: "c".into(),
                status: ToolStatus::Interrupted,
                preview: String::new(),
                total_len: 0,
                line_count: 0,
                truncated: false,
                full: Some(ContentRef::Sidecar {
                    rel_path: "tool-results/c.txt".into(),
                }),
                exit_code: None,
                duration_ms: Some(5),
                images: vec![],
                saved_path: None,
            }],
        );
        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            json!({
                "role": "assistant",
                "ts": 1,
                "blocks": [{
                    "type": "tool_result",
                    "callId": "c",
                    "status": "interrupted",
                    "preview": "",
                    "totalLen": 0,
                    "lineCount": 0,
                    "truncated": false,
                    "full": { "kind": "sidecar", "relPath": "tool-results/c.txt" },
                    "durationMs": 5
                }]
            })
        );

        let chunk = TranscriptChunk::Header {
            total: 0,
            turns: vec![],
            cached: true,
            parse_ms: 3,
        };
        assert_eq!(
            serde_json::to_value(&chunk).unwrap(),
            json!({ "type": "header", "total": 0, "turns": [], "cached": true, "parseMs": 3 })
        );
    }

    #[test]
    fn legacy_message_serializes_like_before() {
        let legacy = |content: &str| SessionMessage {
            role: "user".into(),
            content: content.into(),
            ..SessionMessage::default()
        };
        let msg = legacy("hi");
        assert_eq!(
            serde_json::to_value(&msg).unwrap(),
            json!({ "role": "user", "content": "hi" })
        );
        assert!(!msg.is_empty());
        assert!(legacy("  ").is_empty());
    }
}
