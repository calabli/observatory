//! Response Parser - extracts the token usage from an API response
//!
//! Supports several API formats:
//! - Claude API (non-streaming and streaming)
//! - OpenRouter (OpenAI format)
//! - Codex API (non-streaming and streaming)
//! - Gemini API (non-streaming and streaming)

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn openai_cache_read_tokens(usage: &Value) -> u32 {
    // Every step requires "present and not 0": an explicit 0 is treated as "not reported" and the
    // chain keeps looking further back (#8041:
    // some relays hardcode an earlier field as a stub 0 while the true value only lives in a
    // lower priority field). The official endpoints are unaffected because they report the same
    // non-zero value; when a real 0 is hit all the candidates are 0 or missing, so the result
    // does not change.
    fn provided_nonzero(value: Option<&Value>) -> Option<&Value> {
        value.filter(|value| value.as_u64().is_some_and(|tokens| tokens > 0))
    }
    provided_nonzero(usage.get("cache_read_input_tokens"))
        .or_else(|| provided_nonzero(usage.pointer("/input_tokens_details/cached_tokens")))
        .or_else(|| provided_nonzero(usage.pointer("/prompt_tokens_details/cached_tokens")))
        // The documented cache hit field of DeepSeek Chat, the last fallback: the official endpoint
        // currently mirrors the same value into the undocumented prompt_tokens_details.cached_tokens
        // (the standard field above already hit when it is not 0), so this fallback only kicks in
        // when the upstream sends the documented field but not the mirror (some relays do that), and
        // it defends against the undocumented mirror disappearing someday. prompt_tokens itself
        // already contains hits and misses
        // (the miss is in prompt_cache_miss_tokens, which is only a reference and does not have to be
        // subtracted here),
        // so the hit count can be used directly as the cache_read.
        .or_else(|| provided_nonzero(usage.get("prompt_cache_hit_tokens")))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
}

fn openai_cache_write_tokens(usage: &Value) -> u32 {
    usage
        .get("cache_creation_input_tokens")
        .or_else(|| usage.pointer("/input_tokens_details/cache_write_tokens"))
        .or_else(|| usage.pointer("/prompt_tokens_details/cache_write_tokens"))
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
}

/// The cache tokens of the Claude usage that were written with a 1 hour TTL (a subset of `cache_creation_input_tokens`)
pub(crate) fn claude_cache_creation_1h_tokens(usage: &Value) -> u32 {
    usage
        .pointer("/cache_creation/ephemeral_1h_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32
}

/// The request_id prefix of the session logs, kept in sync with the format in `session_usage.rs`
pub const SESSION_REQUEST_ID_PREFIX: &str = "session:";

/// Claude Code and Claude Desktop share Claude message ids with the session
/// importer, so both use the bare `session:{message_id}` namespace. Other
/// apps retain app/provider scoping to avoid collisions between upstreams.
pub fn dedup_scope_for_app<'a>(
    app_type: &'a str,
    provider_id: &'a str,
) -> Option<(&'a str, &'a str)> {
    (!matches!(app_type, "claude" | "claude-desktop")).then_some((app_type, provider_id))
}

fn response_id(body: &Value, field: &str) -> Option<String> {
    body.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

/// The token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cache_read_tokens: u32,
    pub cache_creation_tokens: u32,
    /// The part of `cache_creation_tokens` that was written with a 1 hour TTL (Claude bills it separately)
    pub cache_creation_1h_tokens: u32,
    /// The name of the actual model extracted from the response (if available)
    pub model: Option<String>,
    /// The message id extracted from the response (used for the deduplication across sources)
    ///
    /// Claude API: `msg_xxx`, the same as the `message.id` in the session JSONL
    #[serde(skip)]
    pub message_id: Option<String>,
}

impl TokenUsage {
    /// Generate a stable request_id. Claude gets no scope so that it keeps converging with the
    /// `session:{message_id}` primary key of the session JSONL; the other protocols add an
    /// app/provider scope so that different upstreams reusing an envelope id cannot overwrite
    /// each other.
    pub fn dedup_request_id(&self, scope: Option<(&str, &str)>) -> String {
        self.message_id
            .as_ref()
            .map(|message_id| match scope {
                Some((app_type, provider_id)) => {
                    format!("{SESSION_REQUEST_ID_PREFIX}{app_type}:{provider_id}:{message_id}")
                }
                None => format!("{SESSION_REQUEST_ID_PREFIX}{message_id}"),
            })
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    }

    /// Whether any billable dimension produced tokens.
    ///
    /// It is used to filter out the all-zero empty usages before writing: when an OpenAI compatible
    /// upstream omits the usage while streaming, the converter synthesizes an all-zero terminal
    /// event and without a message_id `dedup_request_id` degrades to a random UUID, which would
    /// insert a meaningless empty row per request and inflate the request count.
    pub fn has_billable_tokens(&self) -> bool {
        self.input_tokens > 0
            || self.output_tokens > 0
            || self.cache_read_tokens > 0
            || self.cache_creation_tokens > 0
    }
}

impl TokenUsage {
    /// Parse a non-streaming Claude API response
    pub fn from_claude_response(body: &Value) -> Option<Self> {
        let usage = body.get("usage")?;
        // Extract the model name from the response
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let message_id = response_id(body, "id");

        Some(Self {
            input_tokens: usage.get("input_tokens")?.as_u64()? as u32,
            output_tokens: usage.get("output_tokens")?.as_u64()? as u32,
            cache_read_tokens: usage
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: usage
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_1h_tokens: claude_cache_creation_1h_tokens(usage),
            model,
            message_id,
        })
    }

    /// Parse a streaming Claude API response
    #[allow(dead_code)]
    pub fn from_claude_stream_events(events: &[Value]) -> Option<Self> {
        let mut usage = Self::default();
        let mut model: Option<String> = None;
        let mut message_id: Option<String> = None;
        let mut input_from_delta = false;

        for event in events {
            if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                match event_type {
                    "message_start" => {
                        if let Some(message) = event.get("message") {
                            if model.is_none() {
                                if let Some(m) = message.get("model").and_then(|v| v.as_str()) {
                                    model = Some(m.to_string());
                                }
                            }
                            if message_id.is_none() {
                                if let Some(id) = response_id(message, "id") {
                                    message_id = Some(id);
                                }
                            }
                        }
                        if let Some(msg_usage) = event.get("message").and_then(|m| m.get("usage")) {
                            // Get the input_tokens from the message_start (the native Claude API)
                            if let Some(input) =
                                msg_usage.get("input_tokens").and_then(|v| v.as_u64())
                            {
                                usage.input_tokens = input as u32;
                            }
                            usage.cache_read_tokens = msg_usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                                as u32;
                            usage.cache_creation_tokens = msg_usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                                as u32;
                            usage.cache_creation_1h_tokens =
                                claude_cache_creation_1h_tokens(msg_usage);
                        }
                    }
                    "message_delta" => {
                        if let Some(delta_usage) = event.get("usage") {
                            // Get the output_tokens from the message_delta
                            if let Some(output) =
                                delta_usage.get("output_tokens").and_then(|v| v.as_u64())
                            {
                                usage.output_tokens = output as u32;
                            }

                            let delta_input = delta_usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);
                            let delta_cache_read = delta_usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);
                            let delta_cache_creation = delta_usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32);

                            // Some Anthropic-compatible SSE providers report the full context in the message_start but
                            // the corrected fresh input in the message_delta. When a smaller positive delta input is
                            // seen the delta is used; if the same usage block also carries the cache counts, they are
                            // adopted as well to avoid a double count.
                            // When the delta lacks the cache fields, the cache values of the start are kept as a
                            // best-effort fallback.
                            if let Some(input) = delta_input {
                                let should_use_delta_input = input > 0
                                    && (usage.input_tokens == 0
                                        || input < usage.input_tokens
                                        || (input_from_delta && input <= usage.input_tokens));

                                if should_use_delta_input {
                                    usage.input_tokens = input;
                                    input_from_delta = true;
                                    if let Some(cache_read) = delta_cache_read {
                                        usage.cache_read_tokens = cache_read;
                                    }
                                    if let Some(cache_creation) = delta_cache_creation {
                                        usage.cache_creation_tokens = cache_creation;
                                        usage.cache_creation_1h_tokens =
                                            claude_cache_creation_1h_tokens(delta_usage);
                                    }
                                }
                            }
                            // Handle the cache hits (cache_read_input_tokens) of the message_delta
                            if usage.cache_read_tokens == 0 {
                                if let Some(cache_read) = delta_cache_read {
                                    usage.cache_read_tokens = cache_read;
                                }
                            }
                            // Handle the cache creation (cache_creation_input_tokens) of the message_delta
                            // Note: zhipu does not return the cache_creation_input_tokens field at the moment
                            if usage.cache_creation_tokens == 0 {
                                if let Some(cache_creation) = delta_cache_creation {
                                    usage.cache_creation_tokens = cache_creation;
                                    usage.cache_creation_1h_tokens =
                                        claude_cache_creation_1h_tokens(delta_usage);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Use has_billable_tokens instead of only looking at input/output: a fully cached streaming
        // request without output
        // (input==0 && output==0 but cache_read>0) is a real cache-read billing and has to be kept.
        // The Gemini->Anthropic path especially produces this all-cached case once the input was
        // changed to fresh(promptTokenCount - cachedContentTokenCount); the old gate dropped it as
        //"no usage".
        if usage.has_billable_tokens() {
            usage.model = model;
            usage.message_id = message_id;
            Some(usage)
        } else {
            None
        }
    }

    /// Parse a non-streaming Codex API response
    pub fn from_codex_response(body: &Value) -> Option<Self> {
        let usage = body.get("usage");
        if usage.is_none() {
            log::debug!(
                "[Codex] 响应中没有 usage 字段，body keys: {:?}",
                body.as_object().map(|o| o.keys().collect::<Vec<_>>())
            );
            return None;
        }
        let usage = usage?;

        let input_tokens = usage.get("input_tokens").and_then(|v| v.as_u64());
        let output_tokens = usage.get("output_tokens").and_then(|v| v.as_u64());

        if input_tokens.is_none() || output_tokens.is_none() {
            log::debug!("[Codex] usage 字段缺少 input_tokens 或 output_tokens，usage: {usage:?}");
            return None;
        }

        // Extract the model name from the response
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let cached_tokens = openai_cache_read_tokens(usage);
        let cache_write_tokens = openai_cache_write_tokens(usage);

        Some(Self {
            input_tokens: input_tokens? as u32,
            output_tokens: output_tokens? as u32,
            cache_read_tokens: cached_tokens,
            cache_creation_tokens: cache_write_tokens,
            cache_creation_1h_tokens: 0,
            model,
            message_id: response_id(body, "id"),
        })
    }

    /// Smart Codex response parsing - detects the OpenAI or the Codex format automatically
    ///
    /// Codex supports two API formats:
    /// - `/v1/responses`: uses input_tokens/output_tokens
    /// - `/v1/chat/completions`: uses prompt_tokens/completion_tokens (the OpenAI format)
    ///
    /// Note: the original input_tokens are recorded, the cached_tokens are subtracted when the cost
    /// is calculated.
    pub fn from_codex_response_auto(body: &Value) -> Option<Self> {
        let usage = body.get("usage")?;

        // Detect the format: OpenAI uses prompt_tokens, Codex uses input_tokens
        if usage.get("prompt_tokens").is_some() {
            log::debug!("[Codex] 检测到 OpenAI 格式 (prompt_tokens)");
            Self::from_openai_response(body)
        } else if usage.get("input_tokens").is_some() {
            log::debug!("[Codex] 检测到 Codex 格式 (input_tokens)");
            // Use the non-adjusted version and record the original input_tokens
            Self::from_codex_response(body)
        } else {
            log::debug!("[Codex] 无法识别响应格式，usage: {usage:?}");
            None
        }
    }

    /// Smart Codex streaming response parsing - detects the Codex Responses / Images / OpenAI format automatically
    pub fn from_codex_stream_events_auto(events: &[Value]) -> Option<Self> {
        log::debug!("[Codex] 智能解析流式事件，共 {} 个事件", events.len());

        // Try the Codex Responses API format first (the response.completed event)
        for event in events {
            if let Some(event_type) = event.get("type").and_then(|v| v.as_str()) {
                if event_type == "response.completed" {
                    if let Some(response) = event.get("response") {
                        log::debug!("[Codex] 找到 response.completed 事件");
                        return Self::from_codex_response_auto(response);
                    }
                }
            }
        }

        // The Images API streaming format (the image_generation.completed event): the usage hangs
        // directly on the top level of the event and the field shape is the same as in a non-streaming
        // Codex response; walk it backwards and take the last event that can be parsed in that shape,
        // skipping the partial_image events in front that carry no usage. When the parsing does not
        // hold
        // it continues with the OpenAI fallback below and does not change the existing path
        if let Some(usage) = events
            .iter()
            .rev()
            .filter(|event| event.pointer("/usage/input_tokens").is_some())
            .find_map(Self::from_codex_response)
        {
            log::debug!("[Codex] 找到顶层 usage.input_tokens 事件");
            return Some(usage);
        }

        // Fall back to the OpenAI Chat Completions format (the last chunk contains the usage)
        log::debug!("[Codex] 尝试 OpenAI 流式格式");
        Self::from_openai_stream_events(events)
    }

    /// Parse an OpenAI Chat Completions API response (prompt_tokens, completion_tokens)
    pub fn from_openai_response(body: &Value) -> Option<Self> {
        let usage = body.get("usage")?;

        // OpenAI uses prompt_tokens and completion_tokens
        let prompt_tokens = usage.get("prompt_tokens").and_then(|v| v.as_u64())?;
        let completion_tokens = usage.get("completion_tokens").and_then(|v| v.as_u64())?;

        // Get the cached_tokens (they may be inside prompt_tokens_details)
        let cached_tokens = openai_cache_read_tokens(usage);
        let cache_write_tokens = openai_cache_write_tokens(usage);

        // Extract the model name from the response
        let model = body
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        Some(Self {
            input_tokens: prompt_tokens as u32,
            output_tokens: completion_tokens as u32,
            cache_read_tokens: cached_tokens,
            cache_creation_tokens: cache_write_tokens,
            cache_creation_1h_tokens: 0,
            model,
            message_id: response_id(body, "id"),
        })
    }

    /// Parse a streaming OpenAI Chat Completions API response
    pub fn from_openai_stream_events(events: &[Value]) -> Option<Self> {
        log::debug!("[Codex] 解析 OpenAI 流式事件，共 {} 个事件", events.len());
        // An OpenAI streaming response contains the usage in the last chunk
        for event in events.iter().rev() {
            if let Some(usage) = event.get("usage") {
                if !usage.is_null() {
                    log::debug!("[Codex] 找到 usage: {usage:?}");
                    let mut parsed = Self::from_openai_response(event)?;
                    if parsed.message_id.is_none() {
                        parsed.message_id =
                            events.iter().find_map(|chunk| response_id(chunk, "id"));
                    }
                    return Some(parsed);
                }
            }
        }
        log::debug!("[Codex] 未找到 usage 信息");
        None
    }

    /// Parse a non-streaming Gemini API response
    pub fn from_gemini_response(body: &Value) -> Option<Self> {
        let usage = body.get("usageMetadata")?;
        // Extract the name of the actually used model (the modelVersion field)
        let model = body
            .get("modelVersion")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        let prompt_tokens = usage.get("promptTokenCount")?.as_u64()? as u32;
        let total_tokens = usage.get("totalTokenCount")?.as_u64()? as u32;

        // The output tokens = the total tokens - the input tokens
        // This includes candidatesTokenCount + thoughtsTokenCount
        let output_tokens = total_tokens.saturating_sub(prompt_tokens);

        Some(Self {
            input_tokens: prompt_tokens,
            output_tokens,
            cache_read_tokens: usage
                .get("cachedContentTokenCount")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32,
            cache_creation_tokens: 0,
            cache_creation_1h_tokens: 0,
            model,
            message_id: response_id(body, "responseId"),
        })
    }

    /// Parse a streaming Gemini API response
    #[allow(dead_code)]
    pub fn from_gemini_stream_chunks(chunks: &[Value]) -> Option<Self> {
        let mut total_input = 0u32;
        let mut total_tokens = 0u32;
        let mut total_cache_read = 0u32;
        let mut model: Option<String> = None;
        let mut message_id: Option<String> = None;

        for chunk in chunks {
            if let Some(usage) = chunk.get("usageMetadata") {
                // The input tokens (usually stay the same in all the chunks)
                total_input = usage
                    .get("promptTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                // The total tokens (input + output + thoughts)
                total_tokens = usage
                    .get("totalTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;

                // The cache read tokens
                total_cache_read = usage
                    .get("cachedContentTokenCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
            }

            // Extract the name of the actually used model (the modelVersion field)
            if model.is_none() {
                if let Some(model_version) = chunk.get("modelVersion").and_then(|v| v.as_str()) {
                    model = Some(model_version.to_string());
                }
            }
            if message_id.is_none() {
                message_id = response_id(chunk, "responseId");
            }
        }

        // The output tokens = the total tokens - the input tokens
        let total_output = total_tokens.saturating_sub(total_input);

        if total_input > 0 || total_output > 0 {
            Some(Self {
                input_tokens: total_input,
                output_tokens: total_output,
                cache_read_tokens: total_cache_read,
                cache_creation_tokens: 0,
                cache_creation_1h_tokens: 0,
                model,
                message_id,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn response_ids_produce_scoped_dedup_keys_and_empty_ids_fall_back() {
        let response = json!({
            "id": "resp_123",
            "model": "gpt-5.6",
            "usage": { "input_tokens": 10, "output_tokens": 2 }
        });
        let usage = TokenUsage::from_codex_response(&response).unwrap();
        assert_eq!(usage.message_id.as_deref(), Some("resp_123"));
        assert_eq!(
            usage.dedup_request_id(Some(("codex", "provider-a"))),
            "session:codex:provider-a:resp_123"
        );

        let empty = json!({
            "id": "",
            "usage": { "input_tokens": 10, "output_tokens": 2 }
        });
        let empty_usage = TokenUsage::from_codex_response(&empty).unwrap();
        assert!(empty_usage.message_id.is_none());
        assert!(!empty_usage
            .dedup_request_id(Some(("codex", "provider-a")))
            .starts_with("session:"));
    }

    #[test]
    fn claude_apps_share_the_session_request_id_namespace() {
        let usage = TokenUsage {
            message_id: Some("msg_123".to_string()),
            ..Default::default()
        };

        for app_type in ["claude", "claude-desktop"] {
            assert_eq!(
                usage.dedup_request_id(dedup_scope_for_app(app_type, "provider-a")),
                "session:msg_123"
            );
        }
        assert_eq!(
            usage.dedup_request_id(dedup_scope_for_app("codex", "provider-a")),
            "session:codex:provider-a:msg_123"
        );
    }

    #[test]
    fn stream_parsers_recover_ids_from_envelope_chunks() {
        let openai = vec![
            json!({"id": "chatcmpl_123", "choices": []}),
            json!({
                "usage": { "prompt_tokens": 10, "completion_tokens": 2 },
                "choices": []
            }),
        ];
        assert_eq!(
            TokenUsage::from_openai_stream_events(&openai)
                .unwrap()
                .message_id
                .as_deref(),
            Some("chatcmpl_123")
        );

        let gemini = vec![json!({
            "responseId": "gemini_123",
            "usageMetadata": { "promptTokenCount": 10, "totalTokenCount": 12 }
        })];
        assert_eq!(
            TokenUsage::from_gemini_stream_chunks(&gemini)
                .unwrap()
                .message_id
                .as_deref(),
            Some("gemini_123")
        );
    }

    #[test]
    fn test_claude_response_parsing() {
        let response = json!({
            "model": "claude-sonnet-4-20250514",
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 20,
                "cache_creation_input_tokens": 10
            }
        });

        let usage = TokenUsage::from_claude_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 10);
        assert_eq!(usage.model, Some("claude-sonnet-4-20250514".to_string()));
    }

    #[test]
    fn test_has_billable_tokens_gates_empty_usage() {
        // An all-zero usage (like the all-zero terminal event that is synthesized when the upstream
        // omits the usage) must not be billed --
        // this is the basis of the gate of the Codex streaming empty row over-counting fix (D).
        assert!(!TokenUsage::default().has_billable_tokens());
        // A cache_read on its own is a real billable token and has to be counted.
        let only_cache = TokenUsage {
            cache_read_tokens: 100,
            ..Default::default()
        };
        assert!(only_cache.has_billable_tokens());
        let normal = TokenUsage {
            input_tokens: 10,
            output_tokens: 5,
            ..Default::default()
        };
        assert!(normal.has_billable_tokens());
    }

    #[test]
    fn test_claude_stream_cache_only_request_is_recorded() {
        // P2 regression: a fully cached streaming request without output (input==0 && output==0 but
        // cache_read>0)
        // is real billing and has to be kept -- the old gate `input>0 || output>0` dropped it.
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "id": "msg_cacheonly",
                    "model": "claude-opus-4-8",
                    "usage": {
                        "input_tokens": 0,
                        "cache_read_input_tokens": 50000,
                        "cache_creation_input_tokens": 0
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": { "output_tokens": 0 }
            }),
        ];
        let usage = TokenUsage::from_claude_stream_events(&events)
            .expect("cache-only 流式请求必须被记录，不能被 input/output gate 丢弃");
        assert_eq!(usage.input_tokens, 0);
        assert_eq!(usage.output_tokens, 0);
        assert_eq!(usage.cache_read_tokens, 50000);
        assert_eq!(usage.message_id, Some("msg_cacheonly".to_string()));
    }

    #[test]
    fn test_codex_response_auto_returns_some_for_synthetic_all_zero() {
        // P3 regression: when the upstream omits the usage in a non-streaming chat the converter
        // synthesizes an all-zero usage and from_codex_response_auto
        // still returns Some (the fields are there, there is no positivity check) -- that proves that
        // the handlers have to use the has_billable_tokens
        // gate to block the empty row, `if let Some` alone is not enough.
        let synthetic = json!({
            "usage": { "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 }
        });
        let usage = TokenUsage::from_codex_response_auto(&synthetic)
            .expect("全 0 usage 字段存在时 from_codex_response_auto 返回 Some");
        assert!(
            !usage.has_billable_tokens(),
            "全 0 usage 必须被 has_billable_tokens 判为非计费，由 handlers 闸门跳过"
        );
    }

    #[test]
    fn test_claude_response_parsing_no_model() {
        let response = json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 50,
                "cache_read_input_tokens": 20,
                "cache_creation_input_tokens": 10
            }
        });

        let usage = TokenUsage::from_claude_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 10);
        assert_eq!(usage.model, None);
    }

    #[test]
    fn test_claude_stream_parsing() {
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "model": "claude-sonnet-4-20250514",
                    "usage": {
                        "input_tokens": 100,
                        "cache_read_input_tokens": 20,
                        "cache_creation_input_tokens": 10
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "output_tokens": 50
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 10);
        assert_eq!(usage.model, Some("claude-sonnet-4-20250514".to_string()));
    }

    #[test]
    fn test_claude_stream_parsing_no_model() {
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 100,
                        "cache_read_input_tokens": 20,
                        "cache_creation_input_tokens": 10
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "output_tokens": 50
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 10);
        assert_eq!(usage.model, None);
    }

    #[test]
    fn test_gemini_response_parsing() {
        let response = json!({
            "modelVersion": "gemini-3-pro-high",
            "usageMetadata": {
                "promptTokenCount": 8383,
                "candidatesTokenCount": 50,
                "thoughtsTokenCount": 114,
                "totalTokenCount": 8547,
                "cachedContentTokenCount": 20
            }
        });

        let usage = TokenUsage::from_gemini_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 8383);
        // output_tokens = totalTokenCount - promptTokenCount = 8547 - 8383 = 164
        assert_eq!(usage.output_tokens, 164);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.model, Some("gemini-3-pro-high".to_string()));
    }

    #[test]
    fn test_gemini_response_parsing_no_model() {
        // Test the case without a modelVersion field
        let response = json!({
            "usageMetadata": {
                "promptTokenCount": 100,
                "totalTokenCount": 150,
                "cachedContentTokenCount": 20
            }
        });

        let usage = TokenUsage::from_gemini_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 100);
        // output_tokens = totalTokenCount - promptTokenCount = 150 - 100 = 50
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 20);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.model, None);
    }

    #[test]
    fn test_gemini_response_with_thoughts() {
        // Test a real response that contains thoughtsTokenCount
        // This is a real scenario that was reported by a user
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "",
                                "thoughtSignature": "EvcECvQE..."
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ],
            "modelVersion": "gemini-3-pro-high",
            "responseId": "yupTafqLDu-PjMcPhrOx4QQ",
            "usageMetadata": {
                "candidatesTokenCount": 50,
                "promptTokenCount": 8383,
                "thoughtsTokenCount": 114,
                "totalTokenCount": 8547
            }
        });

        let usage = TokenUsage::from_gemini_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 8383);
        // output_tokens = totalTokenCount - promptTokenCount
        // = 8547 - 8383 = 164 (includes candidatesTokenCount 50 + thoughtsTokenCount 114)
        assert_eq!(usage.output_tokens, 164);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.model, Some("gemini-3-pro-high".to_string()));
    }

    #[test]
    fn test_codex_response_parsing_cached_tokens_in_details() {
        let response = json!({
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "input_tokens_details": {
                    "cached_tokens": 300
                }
            }
        });

        let usage = TokenUsage::from_codex_response(&response).unwrap();
        // In the non-adjusted mode the input_tokens keep their value, but the cache hits should be recorded
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.cache_read_tokens, 300);
    }

    #[test]
    fn test_codex_response_parsing_cache_write_tokens_in_details() {
        let response = json!({
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "input_tokens_details": {
                    "cached_tokens": 300,
                    "cache_write_tokens": 200
                }
            }
        });

        let usage = TokenUsage::from_codex_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.cache_read_tokens, 300);
        assert_eq!(usage.cache_creation_tokens, 200);
    }

    #[test]
    fn test_openrouter_stream_parsing() {
        // Test the parsing of a streaming response after the OpenRouter conversion
        // After the conversion of an OpenRouter streaming response the input_tokens are in the message_delta
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "model": "claude-sonnet-4-20250514",
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "delta": {
                    "stop_reason": "end_turn"
                },
                "usage": {
                    "input_tokens": 150,
                    "output_tokens": 75
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 75);
        assert_eq!(usage.model, Some("claude-sonnet-4-20250514".to_string()));
    }

    #[test]
    fn test_claude_stream_prefers_smaller_delta_input_and_cache_pair() {
        // Some Anthropic-compatible providers give the total context including the cache in the
        // message_start
        // and the corrected fresh input in the message_delta, so the delta usage has to win.
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "model": "qwen-max",
                    "usage": {
                        "input_tokens": 200_000,
                        "cache_read_input_tokens": 180_000,
                        "cache_creation_input_tokens": 2_000
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 80_000,
                    "output_tokens": 1_000,
                    "cache_read_input_tokens": 120_000,
                    "cache_creation_input_tokens": 500
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 80_000);
        assert_eq!(usage.output_tokens, 1_000);
        assert_eq!(usage.cache_read_tokens, 120_000);
        assert_eq!(usage.cache_creation_tokens, 500);
        assert_eq!(usage.model, Some("qwen-max".to_string()));
    }

    #[test]
    fn test_claude_stream_updates_cache_pair_from_later_delta_input() {
        // Some providers send a message_delta with an input several times; once a delta input has been
        // adopted, a later delta with the same or a smaller input should keep updating the cache counts
        // of the same block.
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "model": "qwen-max",
                    "usage": {
                        "input_tokens": 200_000,
                        "cache_read_input_tokens": 180_000,
                        "cache_creation_input_tokens": 2_000
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 80_000,
                    "output_tokens": 100,
                    "cache_read_input_tokens": 110_000,
                    "cache_creation_input_tokens": 300
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 80_000,
                    "output_tokens": 1_000,
                    "cache_read_input_tokens": 120_000,
                    "cache_creation_input_tokens": 500
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 80_000);
        assert_eq!(usage.output_tokens, 1_000);
        assert_eq!(usage.cache_read_tokens, 120_000);
        assert_eq!(usage.cache_creation_tokens, 500);
        assert_eq!(usage.model, Some("qwen-max".to_string()));
    }

    #[test]
    fn test_claude_stream_keeps_start_when_delta_input_is_larger() {
        // Under the normal Anthropic semantics the input_tokens of the message_start are already
        // trustworthy;
        // if the delta input is bigger, it must not overwrite the start input/cache.
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "usage": {
                        "input_tokens": 100,
                        "cache_read_input_tokens": 20
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "input_tokens": 150,
                    "output_tokens": 75,
                    "cache_read_input_tokens": 30
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 75);
        assert_eq!(usage.cache_read_tokens, 20);
    }

    #[test]
    fn test_native_claude_stream_parsing() {
        // Test the parsing of a native Claude API streaming response
        // The input_tokens of the native Claude API are in the message_start
        let events = vec![
            json!({
                "type": "message_start",
                "message": {
                    "model": "claude-sonnet-4-20250514",
                    "usage": {
                        "input_tokens": 200,
                        "cache_read_input_tokens": 50
                    }
                }
            }),
            json!({
                "type": "message_delta",
                "usage": {
                    "output_tokens": 100
                }
            }),
        ];

        let usage = TokenUsage::from_claude_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 200);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 50);
        assert_eq!(usage.model, Some("claude-sonnet-4-20250514".to_string()));
    }

    // ============================================================================
    // Smart Codex parsing tests
    // ============================================================================

    #[test]
    fn test_codex_response_auto_openai_format() {
        // The OpenAI format (prompt_tokens/completion_tokens)
        let response = json!({
            "model": "gpt-4o",
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 500,
                "prompt_tokens_details": {
                    "cached_tokens": 200
                }
            }
        });

        let usage = TokenUsage::from_codex_response_auto(&response).unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.cache_read_tokens, 200);
        assert_eq!(usage.model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_openai_response_deepseek_cache_hit_fields() {
        // The DeepSeek Chat format (related to issue #6073): the cache hits and misses are listed
        // separately in the documented
        // prompt_cache_hit_tokens / prompt_cache_miss_tokens, and prompt_tokens contains both.
        // When the upstream only sends these documented fields and does not mirror
        // prompt_tokens_details.cached_tokens
        // (some relays do that), without this fallback the cache hits would be recorded as 0 and the
        // cost would be inflated compared to the official price.
        let response = json!({
            "model": "deepseek-v4-flash",
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 100,
                "prompt_cache_hit_tokens": 600,
                "prompt_cache_miss_tokens": 400,
                "total_tokens": 1100
            }
        });

        let usage = TokenUsage::from_openai_response(&response).unwrap();
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 100);
        assert_eq!(usage.cache_read_tokens, 600);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.model, Some("deepseek-v4-flash".to_string()));
    }

    #[test]
    fn openai_cache_read_prefers_nonzero_standard_field_over_deepseek_specific() {
        // When both sets of fields are present the non-zero standard field is still authoritative: the priority is not changed by the "0 gives way" rule of #8041.
        let response = json!({
            "model": "deepseek-v4-flash",
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 10,
                "prompt_tokens_details": { "cached_tokens": 200 },
                "prompt_cache_hit_tokens": 600
            }
        });
        let usage = TokenUsage::from_openai_response(&response).unwrap();
        assert_eq!(usage.cache_read_tokens, 200);
    }

    #[test]
    fn openai_cache_read_falls_through_explicit_zero_top_field() {
        // #8041: some relays hardcode the cache_read_input_tokens of the first step as a stub 0 and
        // the true value
        // only lives in prompt_cache_hit_tokens. An explicit 0 is treated as "not reported" and the
        // search continues, it must not short-circuit the whole or_else chain with Some(0).
        let response = json!({
            "model": "deepseek-v4-flash",
            "usage": {
                "prompt_tokens": 6650,
                "completion_tokens": 16,
                "cache_read_input_tokens": 0,
                "prompt_cache_hit_tokens": 6400
            }
        });
        let usage = TokenUsage::from_openai_response(&response).unwrap();
        assert_eq!(usage.cache_read_tokens, 6400);
    }

    #[test]
    fn openai_cache_read_falls_through_explicit_zero_mirror_field() {
        // The same mechanism as #8041 (added by the reporter): the stub 0 can also be on the mirror
        // field,
        // so an explicit 0 of prompt_tokens_details.cached_tokens gives way as well.
        let response = json!({
            "model": "deepseek-v4-flash",
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 10,
                "prompt_tokens_details": { "cached_tokens": 0 },
                "prompt_cache_hit_tokens": 600
            }
        });
        let usage = TokenUsage::from_openai_response(&response).unwrap();
        assert_eq!(usage.cache_read_tokens, 600);
    }

    #[test]
    fn test_openai_stream_deepseek_cache_hit_fields() {
        // The streaming path: the usage is on the last chunk, the DeepSeek cache hits have to be extracted there as well.
        let events = vec![
            json!({
                "id": "chatcmpl-ds",
                "model": "deepseek-v4-flash",
                "choices": [{"delta": {"content": "Hi"}}]
            }),
            json!({
                "id": "chatcmpl-ds",
                "model": "deepseek-v4-flash",
                "choices": [],
                "usage": {
                    "prompt_tokens": 800,
                    "completion_tokens": 50,
                    "prompt_cache_hit_tokens": 512,
                    "prompt_cache_miss_tokens": 288,
                    "total_tokens": 850
                }
            }),
        ];

        let usage = TokenUsage::from_openai_stream_events(&events).unwrap();
        assert_eq!(usage.input_tokens, 800);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.cache_read_tokens, 512);
        assert_eq!(usage.message_id.as_deref(), Some("chatcmpl-ds"));
    }

    #[test]
    fn test_codex_response_auto_codex_format() {
        // The Codex format (input_tokens/output_tokens)
        let response = json!({
            "model": "o3",
            "usage": {
                "input_tokens": 1000,
                "output_tokens": 500,
                "input_tokens_details": {
                    "cached_tokens": 300
                }
            }
        });

        let usage = TokenUsage::from_codex_response_auto(&response).unwrap();
        // The original input_tokens are recorded, not adjusted
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.cache_read_tokens, 300);
        assert_eq!(usage.model, Some("o3".to_string()));
    }

    #[test]
    fn test_codex_stream_events_auto_codex_format() {
        // The Codex Responses API streaming format (the response.completed event)
        let events = vec![
            json!({
                "type": "response.created",
                "response": {
                    "id": "resp_123"
                }
            }),
            json!({
                "type": "response.completed",
                "response": {
                    "model": "o3",
                    "usage": {
                        "input_tokens": 1000,
                        "output_tokens": 500,
                        "input_tokens_details": {
                            "cached_tokens": 200
                        }
                    }
                }
            }),
        ];

        let usage = TokenUsage::from_codex_stream_events_auto(&events).unwrap();
        // The original input_tokens are recorded, not adjusted
        assert_eq!(usage.input_tokens, 1000);
        assert_eq!(usage.output_tokens, 500);
        assert_eq!(usage.cache_read_tokens, 200);
        assert_eq!(usage.model, Some("o3".to_string()));
    }

    #[test]
    fn test_codex_stream_events_auto_openai_format() {
        // The OpenAI Chat Completions streaming format (the last chunk contains the usage)
        let events = vec![
            json!({
                "id": "chatcmpl-123",
                "model": "gpt-4o",
                "choices": [{"delta": {"content": "Hello"}}]
            }),
            json!({
                "id": "chatcmpl-123",
                "model": "gpt-4o",
                "choices": [{"delta": {}}],
                "usage": {
                    "prompt_tokens": 100,
                    "completion_tokens": 50
                }
            }),
        ];

        let usage = TokenUsage::from_codex_stream_events_auto(&events).unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
        assert_eq!(usage.model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_codex_stream_events_auto_image_generation_completed() {
        // The Images API streaming format: the usage hangs on the top level of the
        // image_generation.completed event,
        // the field shape is the same as in a non-streaming Codex response (input_tokens / output_tokens)
        let events = vec![
            json!({
                "type": "image_generation.partial_image",
                "b64_json": "cGFydGlhbA==",
                "partial_image_index": 0
            }),
            json!({
                "type": "image_generation.completed",
                "b64_json": "aW1hZ2U=",
                "created_at": 1778832973,
                "usage": {
                    "input_tokens": 1474,
                    "input_tokens_details": {
                        "image_tokens": 1457,
                        "text_tokens": 17
                    },
                    "output_tokens": 1372,
                    "output_tokens_details": {
                        "image_tokens": 1372,
                        "text_tokens": 0
                    },
                    "total_tokens": 2846
                }
            }),
        ];

        let usage = TokenUsage::from_codex_stream_events_auto(&events)
            .expect("image_generation.completed usage should be parsed");
        assert_eq!(usage.input_tokens, 1474);
        assert_eq!(usage.output_tokens, 1372);
        assert_eq!(usage.cache_read_tokens, 0);
        assert_eq!(usage.cache_creation_tokens, 0);
        assert_eq!(usage.model, None);
    }
}
