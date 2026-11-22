//! On-demand Agent single-task call-chain trace extraction (read-only).
//!
//! Builds a hierarchical task waterfall (User Task -> LLM calls -> Tool / Subagent calls)
//! directly from local tool transcripts when available, and gracefully falls back to
//! `proxy_request_logs` turn timing for platforms without local tool logs or remote LAN devices.
//! Conversation bodies are never stored in `observatory.db`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::DateTime;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;

use crate::error::AppError;
use crate::insights::{
    clean_title, find_codex_rollout, query_session_turns_on_conn, safe_session_token,
    SessionTurnRow,
};
use crate::session_manager::providers::utils::parse_timestamp_to_ms;

const MAX_TASKS: usize = 80;
const MAX_SPANS_PER_TASK: usize = 200;
const FALLBACK_TASK_GAP_MS: i64 = 90_000;
const MAX_LLM_FALLBACK_MS: i64 = 180_000;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceSpan {
    pub id: String,
    /// `"llm"` | `"tool"` | `"subagent"` | `"gap"`
    pub kind: String,
    /// `0` for top-level LLM call, `1` for tool / subagent / gap child span.
    pub depth: u8,
    pub label: String,
    pub detail: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub offset_ms: u64,
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    pub model: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cost_usd: f64,
    /// `"ok"` | `"error"`
    pub status: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDurationStat {
    pub name: String,
    pub count: u64,
    pub total_ms: u64,
    pub error_count: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTrace {
    pub index: u32,
    pub prompt: Option<String>,
    pub start_ms: i64,
    pub end_ms: i64,
    pub duration_ms: u64,
    pub llm_ms: u64,
    pub tool_ms: u64,
    pub llm_calls: u64,
    pub tool_calls: u64,
    pub error_count: u64,
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub top_tools: Vec<ToolDurationStat>,
    pub spans: Vec<TraceSpan>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionTraceDetail {
    pub session_id: String,
    pub app_type: String,
    pub title: Option<String>,
    pub project_dir: Option<String>,
    pub git_branch: Option<String>,
    pub resume_command: Option<String>,
    /// `"transcript"` when real tool-call spans were extracted locally, `"turns"` otherwise.
    pub trace_source: String,
    pub total_turns: u64,
    pub total_cost_usd: f64,
    pub total_tokens: u64,
    pub tasks: Vec<TaskTrace>,
    pub turns: Vec<SessionTurnRow>,
}

/// Query a session's turns and build its single-task call-chain waterfall.
pub fn query_session_trace_on_conn(
    conn: &Connection,
    session_id: &str,
    app_type: Option<&str>,
    start_date: Option<i64>,
    end_date: Option<i64>,
    is_remote_device: bool,
) -> Result<SessionTraceDetail, AppError> {
    let detail = query_session_turns_on_conn(conn, session_id, app_type, start_date, end_date)?;

    let transcript_tasks = if is_remote_device || !safe_session_token(session_id) {
        None
    } else {
        extract_local_trace(
            &detail.app_type,
            session_id,
            &detail.turns,
            start_date,
            end_date,
        )
    };

    let (trace_source, tasks) = match transcript_tasks {
        Some(tasks) if !tasks.is_empty() => ("transcript".to_string(), tasks),
        _ => (
            "turns".to_string(),
            build_tasks_from_turns(&detail.turns, detail.title.as_deref()),
        ),
    };

    Ok(SessionTraceDetail {
        session_id: detail.session_id,
        app_type: detail.app_type,
        title: detail.title,
        project_dir: detail.project_dir,
        git_branch: detail.git_branch,
        resume_command: detail.resume_command,
        trace_source,
        total_turns: detail.total_turns,
        total_cost_usd: detail.total_cost_usd,
        total_tokens: detail.total_tokens,
        tasks,
        turns: detail.turns,
    })
}

fn extract_local_trace(
    app_type: &str,
    session_id: &str,
    turns: &[SessionTurnRow],
    start_date: Option<i64>,
    end_date: Option<i64>,
) -> Option<Vec<TaskTrace>> {
    let tasks = match app_type {
        "agy" => extract_agy_trace(session_id, turns),
        "claude" | "claude-desktop" => extract_claude_trace(session_id, turns),
        "codex" => extract_codex_trace(session_id, turns),
        "opencode" => extract_opencode_trace(session_id, turns),
        "gemini" => extract_gemini_trace(session_id, turns),
        _ => None,
    }?;
    if tasks.is_empty() {
        return None;
    }
    let filtered = filter_tasks_by_range(tasks, start_date, end_date);
    Some(filtered)
}

fn filter_tasks_by_range(
    tasks: Vec<TaskTrace>,
    start_date: Option<i64>,
    end_date: Option<i64>,
) -> Vec<TaskTrace> {
    if start_date.is_none() && end_date.is_none() {
        return cap_and_reindex(tasks);
    }
    let start_ms = start_date.map(|s| s * 1000);
    let end_ms = end_date.map(|e| e * 1000);
    let kept: Vec<TaskTrace> = tasks
        .iter()
        .filter(|t| {
            let after_start = start_ms.is_none_or(|s| t.end_ms >= s);
            let before_end = end_ms.is_none_or(|e| t.start_ms < e);
            after_start && before_end
        })
        .cloned()
        .collect();
    if kept.is_empty() {
        cap_and_reindex(tasks)
    } else {
        cap_and_reindex(kept)
    }
}

fn cap_and_reindex(mut tasks: Vec<TaskTrace>) -> Vec<TaskTrace> {
    if tasks.len() > MAX_TASKS {
        let start = tasks.len() - MAX_TASKS;
        tasks = tasks.split_off(start);
    }
    for (i, t) in tasks.iter_mut().enumerate() {
        t.index = (i + 1) as u32;
    }
    tasks
}

// ---------------------------------------------------------------------------
// Helpers: tool classification, short labels, union interval math, turn match
// ---------------------------------------------------------------------------

fn is_subagent_tool(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "invoke_subagent" | "swarm" | "run_workflow" | "task" | "agent" | "subagent"
    ) || lower.contains("subagent")
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let trimmed = s.trim();
    let mut out = String::new();
    for (i, ch) in trimmed.chars().enumerate() {
        if ch == '\n' || ch == '\r' {
            out.push(' ');
            continue;
        }
        if i >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out.trim().to_string()
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn short_path_label(raw: &str) -> String {
    let clean = strip_quotes(raw);
    Path::new(clean)
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| truncate_str(s, 56))
        .unwrap_or_else(|| truncate_str(clean, 56))
}

fn extract_tool_detail(args: &Value) -> Option<String> {
    let obj = args.as_object()?;
    for key in ["toolSummary", "description", "Role", "role"] {
        if let Some(s) = obj.get(key).and_then(Value::as_str) {
            let clean = strip_quotes(s);
            if !clean.is_empty() {
                return Some(truncate_str(clean, 64));
            }
        }
    }
    for key in [
        "Path",
        "path",
        "file_path",
        "filePath",
        "target_file",
        "TargetFile",
        "directory",
    ] {
        if let Some(s) = obj.get(key).and_then(Value::as_str) {
            let clean = strip_quotes(s);
            if !clean.is_empty() {
                return Some(short_path_label(clean));
            }
        }
    }
    for key in [
        "CommandLine",
        "command",
        "cmd",
        "query",
        "pattern",
        "Url",
        "url",
    ] {
        if let Some(s) = obj.get(key).and_then(Value::as_str) {
            let clean = strip_quotes(s);
            if !clean.is_empty() {
                return Some(truncate_str(clean, 64));
            }
        }
    }
    None
}

fn union_duration_ms(intervals: &[(i64, i64)]) -> u64 {
    if intervals.is_empty() {
        return 0;
    }
    let mut sorted: Vec<(i64, i64)> = intervals.iter().copied().filter(|(s, e)| e > s).collect();
    if sorted.is_empty() {
        return 0;
    }
    sorted.sort_unstable_by_key(|(s, e)| (*s, *e));
    let mut total: i64 = 0;
    let (mut cur_s, mut cur_e) = sorted[0];
    for (s, e) in sorted.into_iter().skip(1) {
        if s <= cur_e {
            cur_e = cur_e.max(e);
        } else {
            total += cur_e - cur_s;
            cur_s = s;
            cur_e = e;
        }
    }
    total += cur_e - cur_s;
    total.max(0) as u64
}

struct TurnMatcher<'a> {
    turns: &'a [SessionTurnRow],
    used: Vec<bool>,
}

impl<'a> TurnMatcher<'a> {
    fn new(turns: &'a [SessionTurnRow]) -> Self {
        Self {
            turns,
            used: vec![false; turns.len()],
        }
    }

    fn match_turn(&mut self, ts_ms: i64, output_tokens: u64) -> Option<&'a SessionTurnRow> {
        let mut best_idx: Option<usize> = None;
        let mut best_score = i64::MAX;
        for (i, turn) in self.turns.iter().enumerate() {
            if self.used[i] {
                continue;
            }
            let diff_ms = (turn.created_at * 1000 - ts_ms).abs();
            if diff_ms > 180_000 {
                continue;
            }
            let tok_penalty = if output_tokens > 0 && turn.output_tokens == output_tokens {
                0
            } else {
                30_000
            };
            let score = diff_ms + tok_penalty;
            if score < best_score {
                best_score = score;
                best_idx = Some(i);
            }
        }
        let idx = best_idx?;
        self.used[idx] = true;
        Some(&self.turns[idx])
    }
}

fn finalize_task(
    index: u32,
    prompt: Option<String>,
    mut spans: Vec<TraceSpan>,
    task_start_hint: Option<i64>,
) -> Option<TaskTrace> {
    if spans.is_empty() {
        return None;
    }
    if spans.len() > MAX_SPANS_PER_TASK {
        spans.truncate(MAX_SPANS_PER_TASK);
    }

    let min_span_start = spans.iter().map(|s| s.start_ms).min().unwrap_or(0);
    let start_ms = task_start_hint
        .filter(|h| *h <= min_span_start && min_span_start - *h <= 60_000)
        .unwrap_or(min_span_start);
    let end_ms = spans
        .iter()
        .map(|s| s.end_ms)
        .max()
        .unwrap_or(start_ms)
        .max(start_ms);

    let mut llm_intervals = Vec::new();
    let mut tool_intervals = Vec::new();
    let mut llm_calls = 0u64;
    let mut tool_calls = 0u64;
    let mut error_count = 0u64;
    let mut total_tokens = 0u64;
    let mut total_cost_usd = 0.0f64;
    let mut tool_agg: HashMap<String, ToolDurationStat> = HashMap::new();

    for span in &mut spans {
        if span.end_ms < span.start_ms {
            span.end_ms = span.start_ms;
        }
        span.offset_ms = (span.start_ms - start_ms).max(0) as u64;
        span.duration_ms = (span.end_ms - span.start_ms).max(0) as u64;
        if span.status == "error" {
            error_count += 1;
        }
        match span.kind.as_str() {
            "llm" => {
                llm_calls += 1;
                llm_intervals.push((span.start_ms, span.end_ms));
                total_tokens += span.input_tokens + span.output_tokens + span.cache_read_tokens;
                total_cost_usd += span.cost_usd;
            }
            "tool" | "subagent" => {
                tool_calls += 1;
                tool_intervals.push((span.start_ms, span.end_ms));
                let entry =
                    tool_agg
                        .entry(span.label.clone())
                        .or_insert_with(|| ToolDurationStat {
                            name: span.label.clone(),
                            count: 0,
                            total_ms: 0,
                            error_count: 0,
                        });
                entry.count += 1;
                entry.total_ms += span.duration_ms;
                if span.status == "error" {
                    entry.error_count += 1;
                }
            }
            _ => {}
        }
    }

    let mut top_tools: Vec<ToolDurationStat> = tool_agg.into_values().collect();
    top_tools.sort_by(|a, b| {
        b.total_ms
            .cmp(&a.total_ms)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.name.cmp(&b.name))
    });
    top_tools.truncate(8);

    Some(TaskTrace {
        index,
        prompt,
        start_ms,
        end_ms,
        duration_ms: (end_ms - start_ms).max(1) as u64,
        llm_ms: union_duration_ms(&llm_intervals),
        tool_ms: union_duration_ms(&tool_intervals),
        llm_calls,
        tool_calls,
        error_count,
        total_tokens,
        total_cost_usd,
        top_tools,
        spans,
    })
}

// ---------------------------------------------------------------------------
// 1. Antigravity (`agy`) trace parser
// ---------------------------------------------------------------------------

fn find_agy_transcript(session_id: &str) -> Option<PathBuf> {
    let roots = [
        crate::gemini_config::get_gemini_dir().join("antigravity/brain"),
        crate::agy_config::agy_dir().join("brain"),
    ];
    for root in roots {
        let logs = root.join(session_id).join(".system_generated/logs");
        for name in ["transcript.jsonl", "transcript_full.jsonl"] {
            let candidate = logs.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn parse_agy_user_request(content: &str) -> Option<String> {
    let inner = content
        .split_once("<USER_REQUEST>")
        .and_then(|(_, rest)| rest.split_once("</USER_REQUEST>"))
        .map(|(s, _)| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(content);
    clean_title(inner)
}

fn parse_agy_tool_result_timing(content: &str) -> (Option<i64>, Option<i64>, bool) {
    let mut created = None;
    let mut completed = None;
    let mut has_error = false;
    for (idx, line) in content.lines().take(12).enumerate() {
        let trimmed = line.trim();
        if let Some(ts) = trimmed.strip_prefix("Created At:") {
            if created.is_none() {
                created = DateTime::parse_from_rfc3339(ts.trim())
                    .ok()
                    .map(|dt| dt.timestamp_millis());
            }
        } else if let Some(ts) = trimmed.strip_prefix("Completed At:") {
            if completed.is_none() {
                completed = DateTime::parse_from_rfc3339(ts.trim())
                    .ok()
                    .map(|dt| dt.timestamp_millis());
            }
        } else if idx < 8
            && (trimmed.starts_with("Error:")
                || trimmed.starts_with("Tool error")
                || (trimmed.starts_with("Command exited with code ")
                    && !trimmed.starts_with("Command exited with code 0")))
        {
            has_error = true;
        }
    }
    (created, completed, has_error)
}

struct AgyStep {
    step_index: u64,
    step_type: String,
    source: String,
    status: String,
    created_at_ms: i64,
    model: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    tool_calls: Vec<(String, Option<String>)>,
    prompt: Option<String>,
    tool_created_ms: Option<i64>,
    tool_completed_ms: Option<i64>,
    tool_content_err: bool,
}

fn extract_agy_trace(session_id: &str, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let path = find_agy_transcript(session_id)?;
    let steps = parse_agy_steps(&path)?;
    Some(build_agy_tasks(&steps, turns))
}

fn parse_agy_steps(path: &Path) -> Option<Vec<AgyStep>> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut by_index: BTreeMap<u64, AgyStep> = BTreeMap::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(step_index) = v["step_index"].as_u64() else {
            continue;
        };
        let Some(created_at_ms) = parse_timestamp_to_ms(&v["created_at"]) else {
            continue;
        };
        let step_type = v["type"].as_str().unwrap_or("").to_string();
        let source = v["source"].as_str().unwrap_or("").to_string();
        let status = v["status"].as_str().unwrap_or("DONE").to_string();
        let model = v["model"].as_str().map(str::to_string);
        let input_tokens = v["input_tokens"].as_u64().unwrap_or(0);
        let output_tokens = v["output_tokens"].as_u64().unwrap_or(0);
        let cache_read_tokens = v["cache_read_tokens"].as_u64().unwrap_or(0);

        let mut tool_calls = Vec::new();
        if let Some(arr) = v["tool_calls"].as_array() {
            for tc in arr {
                if let Some(name) = tc["name"]
                    .as_str()
                    .or_else(|| tc["function"]["name"].as_str())
                {
                    let detail = extract_tool_detail(&tc["args"])
                        .or_else(|| extract_tool_detail(&tc["arguments"]));
                    tool_calls.push((name.to_string(), detail));
                }
            }
        }

        let prompt = if step_type == "USER_INPUT" {
            v["content"].as_str().and_then(parse_agy_user_request)
        } else {
            None
        };
        let (tool_created_ms, tool_completed_ms, tool_content_err) = if step_type == "GENERIC" {
            v["content"]
                .as_str()
                .map(parse_agy_tool_result_timing)
                .unwrap_or((None, None, false))
        } else {
            (None, None, false)
        };

        by_index.insert(
            step_index,
            AgyStep {
                step_index,
                step_type,
                source,
                status,
                created_at_ms,
                model,
                input_tokens,
                output_tokens,
                cache_read_tokens,
                tool_calls,
                prompt,
                tool_created_ms,
                tool_completed_ms,
                tool_content_err,
            },
        );
    }

    Some(by_index.into_values().collect())
}

fn build_agy_tasks(steps: &[AgyStep], turns: &[SessionTurnRow]) -> Vec<TaskTrace> {
    let mut matcher = TurnMatcher::new(turns);
    let mut tasks = Vec::new();
    let mut cur_prompt: Option<String> = None;
    let mut cur_start: Option<i64> = None;
    let mut cur_spans: Vec<TraceSpan> = Vec::new();
    let mut i = 0usize;

    while i < steps.len() {
        let step = &steps[i];
        if step.step_type == "USER_INPUT" && step.source == "USER_EXPLICIT" {
            if let Some(t) = finalize_task(
                (tasks.len() + 1) as u32,
                cur_prompt.take(),
                std::mem::take(&mut cur_spans),
                cur_start.take(),
            ) {
                tasks.push(t);
            }
            cur_prompt = step.prompt.clone();
            cur_start = Some(step.created_at_ms);
            i += 1;
            continue;
        }

        if step.step_type == "PLANNER_RESPONSE" {
            // Collect immediately following GENERIC tool result steps up to the next planner/user step.
            let mut result_steps: Vec<&AgyStep> = Vec::new();
            let mut j = i + 1;
            while j < steps.len() {
                let next = &steps[j];
                if next.step_type == "PLANNER_RESPONSE"
                    || (next.step_type == "USER_INPUT" && next.source == "USER_EXPLICIT")
                {
                    break;
                }
                if next.step_type == "GENERIC" {
                    result_steps.push(next);
                }
                j += 1;
            }
            let next_boundary_ms = steps.get(j).map(|s| s.created_at_ms);

            let matched = matcher.match_turn(step.created_at_ms, step.output_tokens);
            let llm_start = step.created_at_ms;
            let earliest_tool_start = result_steps
                .iter()
                .filter_map(|r| r.tool_created_ms.or(Some(r.created_at_ms)))
                .min();
            let llm_end = if let Some(t_start) = earliest_tool_start {
                if t_start > llm_start && t_start - llm_start <= MAX_LLM_FALLBACK_MS {
                    t_start
                } else {
                    llm_start + matched.map(|m| m.latency_ms as i64).unwrap_or(1_000)
                }
            } else if let Some(m) = matched {
                if m.latency_ms > 0 {
                    llm_start + m.latency_ms as i64
                } else {
                    llm_start + 1_000
                }
            } else if let Some(nb) = next_boundary_ms {
                if nb > llm_start && nb - llm_start <= MAX_LLM_FALLBACK_MS {
                    nb
                } else {
                    llm_start + 1_000
                }
            } else {
                llm_start + 1_000
            };

            let model_name = step
                .model
                .clone()
                .or_else(|| matched.map(|m| m.model.clone()))
                .unwrap_or_else(|| "model".to_string());
            let is_err = step.status == "ERROR"
                || matched.is_some_and(|m| m.status_code != 0 && m.status_code >= 400);

            let llm_detail = if !step.tool_calls.is_empty() {
                Some(format!("调用 {} 个工具", step.tool_calls.len()))
            } else {
                Some("文本回复".to_string())
            };

            cur_spans.push(TraceSpan {
                id: format!("agy-llm-{}", step.step_index),
                kind: "llm".to_string(),
                depth: 0,
                label: model_name.clone(),
                detail: llm_detail,
                start_ms: llm_start,
                end_ms: llm_end.max(llm_start + 100),
                offset_ms: 0,
                duration_ms: 0,
                ttft_ms: matched.and_then(|m| m.first_token_ms),
                model: Some(model_name),
                input_tokens: step.input_tokens,
                output_tokens: step.output_tokens,
                cache_read_tokens: step.cache_read_tokens,
                cost_usd: matched.map(|m| m.total_cost_usd).unwrap_or(0.0),
                status: if is_err { "error" } else { "ok" }.to_string(),
                error_message: matched.and_then(|m| m.error_message.clone()),
            });

            for (tc_idx, (tc_name, tc_detail)) in step.tool_calls.iter().enumerate() {
                let res = result_steps.get(tc_idx).copied();
                let t_start = res
                    .and_then(|r| r.tool_created_ms.or(Some(r.created_at_ms)))
                    .unwrap_or(llm_end);
                let t_end = res
                    .and_then(|r| r.tool_completed_ms)
                    .or_else(|| {
                        next_boundary_ms
                            .filter(|&nb| nb > t_start && nb - t_start <= MAX_LLM_FALLBACK_MS)
                    })
                    .unwrap_or(t_start + 300);
                let tc_err = res.is_some_and(|r| r.status == "ERROR" || r.tool_content_err);

                cur_spans.push(TraceSpan {
                    id: format!("agy-tool-{}-{tc_idx}", step.step_index),
                    kind: if is_subagent_tool(tc_name) {
                        "subagent"
                    } else {
                        "tool"
                    }
                    .to_string(),
                    depth: 1,
                    label: tc_name.clone(),
                    detail: tc_detail.clone(),
                    start_ms: t_start,
                    end_ms: t_end.max(t_start + 100),
                    offset_ms: 0,
                    duration_ms: 0,
                    ttft_ms: None,
                    model: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                    status: if tc_err { "error" } else { "ok" }.to_string(),
                    error_message: tc_err.then(|| "工具执行失败或非零退出".to_string()),
                });
            }
            i = j;
            continue;
        }
        i += 1;
    }

    if let Some(t) = finalize_task(
        (tasks.len() + 1) as u32,
        cur_prompt.take(),
        cur_spans,
        cur_start,
    ) {
        tasks.push(t);
    }
    tasks
}

// ---------------------------------------------------------------------------
// 2. Claude Code (`claude` / `claude-desktop`) trace parser
// ---------------------------------------------------------------------------

fn find_claude_session_file(session_id: &str) -> Option<PathBuf> {
    let projects_dir = crate::config::get_home_dir().join(".claude/projects");
    let entries = fs::read_dir(projects_dir).ok()?;
    let filename = format!("{session_id}.jsonl");
    for entry in entries.flatten() {
        let candidate = entry.path().join(&filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn extract_claude_trace(session_id: &str, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let path = find_claude_session_file(session_id)?;
    parse_claude_trace_file(&path, turns)
}

fn parse_claude_trace_file(path: &Path, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let file = fs::File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let mut matcher = TurnMatcher::new(turns);
    let mut tasks = Vec::new();
    let mut cur_prompt: Option<String> = None;
    let mut cur_start: Option<i64> = None;
    let mut cur_spans: Vec<TraceSpan> = Vec::new();
    let mut open_tools: HashMap<String, usize> = HashMap::new();
    let mut last_event_ms: Option<i64> = None;
    let mut seq = 0u64;

    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(ts_ms) = parse_timestamp_to_ms(&v["timestamp"]) else {
            continue;
        };
        let rec_type = v["type"].as_str().unwrap_or("");

        if rec_type == "user" {
            let content = &v["message"]["content"];
            let mut has_tool_result = false;
            if let Some(arr) = content.as_array() {
                for item in arr {
                    if item["type"] == "tool_result" {
                        has_tool_result = true;
                        if let Some(id) = item["tool_use_id"].as_str() {
                            if let Some(span_idx) = open_tools.remove(id) {
                                if let Some(span) = cur_spans.get_mut(span_idx) {
                                    span.end_ms = ts_ms.max(span.start_ms + 50);
                                    if item["is_error"].as_bool().unwrap_or(false) {
                                        span.status = "error".to_string();
                                        span.error_message = Some("工具返回错误".to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !has_tool_result {
                // New user task prompt
                if let Some(t) = finalize_task(
                    (tasks.len() + 1) as u32,
                    cur_prompt.take(),
                    std::mem::take(&mut cur_spans),
                    cur_start.take(),
                ) {
                    tasks.push(t);
                }
                open_tools.clear();
                cur_prompt = content.as_str().and_then(clean_title).or_else(|| {
                    content.as_array().and_then(|arr| {
                        arr.iter()
                            .find(|b| b["type"] == "text")
                            .and_then(|b| b["text"].as_str())
                            .and_then(clean_title)
                    })
                });
                cur_start = Some(ts_ms);
            }
            last_event_ms = Some(ts_ms);
            continue;
        }

        if rec_type == "assistant" {
            let msg = &v["message"];
            let usage = &msg["usage"];
            let content = msg["content"].as_array();
            let mut tool_uses: Vec<(String, String, Option<String>)> = Vec::new();
            if let Some(arr) = content {
                for item in arr {
                    if item["type"] == "tool_use" {
                        let id = item["id"].as_str().unwrap_or("").to_string();
                        let name = item["name"].as_str().unwrap_or("tool").to_string();
                        let detail = extract_tool_detail(&item["input"]);
                        tool_uses.push((id, name, detail));
                    }
                }
            }
            let input_tokens = usage["input_tokens"].as_u64().unwrap_or(0);
            let output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
            let cache_read = usage["cache_read_input_tokens"].as_u64().unwrap_or(0);
            if output_tokens == 0 && input_tokens == 0 && tool_uses.is_empty() {
                last_event_ms = Some(ts_ms);
                continue;
            }
            seq += 1;
            let matched = matcher.match_turn(ts_ms, output_tokens);
            let llm_start = matched
                .filter(|m| m.latency_ms > 0 && m.latency_ms <= MAX_LLM_FALLBACK_MS as u64)
                .map(|m| ts_ms - m.latency_ms as i64)
                .or_else(|| {
                    last_event_ms
                        .filter(|&prev| ts_ms > prev && ts_ms - prev <= MAX_LLM_FALLBACK_MS)
                })
                .unwrap_or(ts_ms - 800);
            let model_name = msg["model"]
                .as_str()
                .map(str::to_string)
                .or_else(|| matched.map(|m| m.model.clone()))
                .unwrap_or_else(|| "claude".to_string());

            cur_spans.push(TraceSpan {
                id: format!("claude-llm-{seq}"),
                kind: "llm".to_string(),
                depth: 0,
                label: model_name.clone(),
                detail: if tool_uses.is_empty() {
                    Some("文本回复".to_string())
                } else {
                    Some(format!("调用 {} 个工具", tool_uses.len()))
                },
                start_ms: llm_start,
                end_ms: ts_ms.max(llm_start + 100),
                offset_ms: 0,
                duration_ms: 0,
                ttft_ms: matched.and_then(|m| m.first_token_ms),
                model: Some(model_name),
                input_tokens,
                output_tokens,
                cache_read_tokens: cache_read,
                cost_usd: matched.map(|m| m.total_cost_usd).unwrap_or(0.0),
                status: if matched.is_some_and(|m| m.status_code != 0 && m.status_code >= 400) {
                    "error"
                } else {
                    "ok"
                }
                .to_string(),
                error_message: matched.and_then(|m| m.error_message.clone()),
            });

            for (idx, (tid, tname, tdetail)) in tool_uses.into_iter().enumerate() {
                let span_idx = cur_spans.len();
                cur_spans.push(TraceSpan {
                    id: format!("claude-tool-{seq}-{idx}"),
                    kind: if is_subagent_tool(&tname) {
                        "subagent"
                    } else {
                        "tool"
                    }
                    .to_string(),
                    depth: 1,
                    label: tname,
                    detail: tdetail,
                    start_ms: ts_ms,
                    end_ms: ts_ms + 300,
                    offset_ms: 0,
                    duration_ms: 0,
                    ttft_ms: None,
                    model: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                    status: "ok".to_string(),
                    error_message: None,
                });
                if !tid.is_empty() {
                    open_tools.insert(tid, span_idx);
                }
            }
            last_event_ms = Some(ts_ms);
        }
    }

    if let Some(t) = finalize_task(
        (tasks.len() + 1) as u32,
        cur_prompt.take(),
        cur_spans,
        cur_start,
    ) {
        tasks.push(t);
    }
    Some(tasks)
}

// ---------------------------------------------------------------------------
// 3. Codex (`codex`) trace parser
// ---------------------------------------------------------------------------

fn extract_codex_trace(session_id: &str, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let sessions_dir = crate::codex_config::get_codex_config_dir().join("sessions");
    let path = find_codex_rollout(&sessions_dir, session_id, 0)?;
    let mut reader = crate::codex_rollout_file::open_reader(&path).ok()?;
    let mut matcher = TurnMatcher::new(turns);
    let mut tasks = Vec::new();
    let mut cur_prompt: Option<String> = None;
    let mut cur_start: Option<i64> = None;
    let mut cur_spans: Vec<TraceSpan> = Vec::new();
    let mut open_tools: HashMap<String, usize> = HashMap::new();
    let mut last_event_ms: Option<i64> = None;
    let mut current_model = String::from("codex");
    let mut seq = 0u64;
    let mut line = String::new();

    loop {
        line.clear();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let ts_ms = parse_timestamp_to_ms(&v["timestamp"]);
        let rec_type = v["type"].as_str().unwrap_or("");
        let payload = &v["payload"];

        if rec_type == "turn_context" {
            if let Some(m) = payload["model"].as_str().filter(|s| !s.is_empty()) {
                current_model = m.to_string();
            }
            continue;
        }
        let Some(ts_ms) = ts_ms else {
            continue;
        };

        if rec_type == "event_msg" && payload["type"] == "user_message" {
            if let Some(t) = finalize_task(
                (tasks.len() + 1) as u32,
                cur_prompt.take(),
                std::mem::take(&mut cur_spans),
                cur_start.take(),
            ) {
                tasks.push(t);
            }
            open_tools.clear();
            cur_prompt = payload["message"].as_str().and_then(clean_title);
            cur_start = Some(ts_ms);
            last_event_ms = Some(ts_ms);
            continue;
        }

        if rec_type == "event_msg" && payload["type"] == "token_count" {
            let info = &payload["info"]["last_token_usage"];
            let out_tok = info["output_tokens"].as_u64().unwrap_or(0);
            let in_tok = info["input_tokens"].as_u64().unwrap_or(0);
            let cache_tok = info["cached_input_tokens"].as_u64().unwrap_or(0);
            if out_tok > 0 || in_tok > 0 {
                seq += 1;
                let matched = matcher.match_turn(ts_ms, out_tok);
                let llm_start = matched
                    .filter(|m| m.latency_ms > 0 && m.latency_ms <= MAX_LLM_FALLBACK_MS as u64)
                    .map(|m| ts_ms - m.latency_ms as i64)
                    .or_else(|| {
                        last_event_ms
                            .filter(|&prev| ts_ms > prev && ts_ms - prev <= MAX_LLM_FALLBACK_MS)
                    })
                    .unwrap_or(ts_ms - 1_000);
                let model = matched
                    .map(|m| m.model.clone())
                    .unwrap_or_else(|| current_model.clone());
                cur_spans.push(TraceSpan {
                    id: format!("codex-llm-{seq}"),
                    kind: "llm".to_string(),
                    depth: 0,
                    label: model.clone(),
                    detail: Some("模型推理".to_string()),
                    start_ms: llm_start,
                    end_ms: ts_ms.max(llm_start + 100),
                    offset_ms: 0,
                    duration_ms: 0,
                    ttft_ms: matched.and_then(|m| m.first_token_ms),
                    model: Some(model),
                    input_tokens: in_tok.saturating_sub(cache_tok),
                    output_tokens: out_tok,
                    cache_read_tokens: cache_tok,
                    cost_usd: matched.map(|m| m.total_cost_usd).unwrap_or(0.0),
                    status: "ok".to_string(),
                    error_message: None,
                });
                last_event_ms = Some(ts_ms);
            }
            continue;
        }

        if rec_type == "response_item" {
            let ptype = payload["type"].as_str().unwrap_or("");
            if ptype == "function_call" || ptype == "custom_tool_call" {
                seq += 1;
                let call_id = payload["call_id"].as_str().unwrap_or("").to_string();
                let name = payload["name"].as_str().unwrap_or("tool").to_string();
                let detail = payload["arguments"]
                    .as_str()
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .and_then(|val| extract_tool_detail(&val))
                    .or_else(|| {
                        payload["input"]
                            .as_str()
                            .map(|s| truncate_str(s, 64))
                            .filter(|s| !s.is_empty())
                    });
                let span_idx = cur_spans.len();
                cur_spans.push(TraceSpan {
                    id: format!("codex-tool-{seq}"),
                    kind: if is_subagent_tool(&name) {
                        "subagent"
                    } else {
                        "tool"
                    }
                    .to_string(),
                    depth: 1,
                    label: name,
                    detail,
                    start_ms: ts_ms,
                    end_ms: ts_ms + 300,
                    offset_ms: 0,
                    duration_ms: 0,
                    ttft_ms: None,
                    model: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                    status: "ok".to_string(),
                    error_message: None,
                });
                if !call_id.is_empty() {
                    open_tools.insert(call_id, span_idx);
                }
                last_event_ms = Some(ts_ms);
            } else if ptype == "function_call_output" || ptype == "custom_tool_call_output" {
                if let Some(call_id) = payload["call_id"].as_str() {
                    if let Some(idx) = open_tools.remove(call_id) {
                        if let Some(span) = cur_spans.get_mut(idx) {
                            span.end_ms = ts_ms.max(span.start_ms + 50);
                        }
                    }
                }
                last_event_ms = Some(ts_ms);
            }
        }
    }

    if let Some(t) = finalize_task(
        (tasks.len() + 1) as u32,
        cur_prompt.take(),
        cur_spans,
        cur_start,
    ) {
        tasks.push(t);
    }
    Some(tasks)
}

// ---------------------------------------------------------------------------
// 4. OpenCode (`opencode.db`) trace parser
// ---------------------------------------------------------------------------

fn extract_opencode_trace(session_id: &str, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let db_path = crate::opencode_config::get_opencode_db_path();
    let conn =
        Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;

    let has_v1 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='message')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        != 0;
    let has_parts = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='part')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0)
        != 0;
    if !has_v1 || !has_parts {
        return None;
    }

    let mut msg_stmt = conn
        .prepare("SELECT id, time_created, data FROM message WHERE session_id = ?1 ORDER BY time_created ASC")
        .ok()?;
    let messages: Vec<(String, i64, Value)> = msg_stmt
        .query_map([session_id], |r| {
            let id: String = r.get(0)?;
            let ts: i64 = r.get(1)?;
            let raw: String = r.get(2)?;
            Ok((id, ts, raw))
        })
        .ok()?
        .flatten()
        .filter_map(|(id, ts, raw)| {
            serde_json::from_str::<Value>(&raw)
                .ok()
                .map(|v| (id, ts, v))
        })
        .collect();
    if messages.is_empty() {
        return None;
    }

    let mut parts_by_msg: HashMap<String, Vec<Value>> = HashMap::new();
    if let Ok(mut p_stmt) = conn.prepare(
        "SELECT message_id, data FROM part WHERE session_id = ?1 ORDER BY time_created ASC",
    ) {
        if let Ok(rows) = p_stmt.query_map([session_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for (mid, raw) in rows.flatten() {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    parts_by_msg.entry(mid).or_default().push(v);
                }
            }
        }
    }

    let mut matcher = TurnMatcher::new(turns);
    let mut tasks = Vec::new();
    let mut cur_prompt: Option<String> = None;
    let mut cur_start: Option<i64> = None;
    let mut cur_spans: Vec<TraceSpan> = Vec::new();

    for (msg_id, ts_ms, data) in messages {
        let role = data["role"].as_str().unwrap_or("");
        if role == "user" {
            if let Some(t) = finalize_task(
                (tasks.len() + 1) as u32,
                cur_prompt.take(),
                std::mem::take(&mut cur_spans),
                cur_start.take(),
            ) {
                tasks.push(t);
            }
            let prompt = parts_by_msg.get(&msg_id).and_then(|parts| {
                parts
                    .iter()
                    .find(|p| p["type"] == "text")
                    .and_then(|p| p["text"].as_str())
                    .and_then(clean_title)
            });
            cur_prompt = prompt;
            cur_start = Some(ts_ms);
            continue;
        }
        if role == "assistant" {
            let start = data["time"]["created"].as_i64().unwrap_or(ts_ms);
            let end = data["time"]["streamed"]
                .as_i64()
                .or_else(|| data["time"]["completed"].as_i64())
                .unwrap_or(start + 1_000);
            let tokens = &data["tokens"];
            let input_tokens = tokens["input"].as_u64().unwrap_or(0);
            let output_tokens = tokens["output"].as_u64().unwrap_or(0);
            let cache_read = tokens["cache"]["read"].as_u64().unwrap_or(0);
            let model = data["modelID"].as_str().unwrap_or("opencode").to_string();
            let matched = matcher.match_turn(start, output_tokens);
            let cost = data["cost"]
                .as_f64()
                .filter(|c| *c > 0.0)
                .or_else(|| matched.map(|m| m.total_cost_usd))
                .unwrap_or(0.0);

            cur_spans.push(TraceSpan {
                id: format!("oc-llm-{msg_id}"),
                kind: "llm".to_string(),
                depth: 0,
                label: model.clone(),
                detail: Some("模型回复".to_string()),
                start_ms: start,
                end_ms: end.max(start + 100),
                offset_ms: 0,
                duration_ms: 0,
                ttft_ms: None,
                model: Some(model),
                input_tokens,
                output_tokens,
                cache_read_tokens: cache_read,
                cost_usd: cost,
                status: if data["error"].is_object() {
                    "error"
                } else {
                    "ok"
                }
                .to_string(),
                error_message: data["error"]["message"].as_str().map(str::to_string),
            });

            if let Some(parts) = parts_by_msg.get(&msg_id) {
                for (idx, p) in parts.iter().enumerate() {
                    if p["type"] == "tool" {
                        let name = p["tool"].as_str().unwrap_or("tool").to_string();
                        let state = &p["state"];
                        let t_start = state["time"]["start"].as_i64().unwrap_or(end);
                        let t_end = state["time"]["end"].as_i64().unwrap_or(t_start + 300);
                        let detail = state["title"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                            .map(|s| truncate_str(s, 64))
                            .or_else(|| extract_tool_detail(&state["input"]));
                        let err = state["status"].as_str() == Some("error");
                        cur_spans.push(TraceSpan {
                            id: format!("oc-tool-{msg_id}-{idx}"),
                            kind: if is_subagent_tool(&name) {
                                "subagent"
                            } else {
                                "tool"
                            }
                            .to_string(),
                            depth: 1,
                            label: name,
                            detail,
                            start_ms: t_start,
                            end_ms: t_end.max(t_start + 80),
                            offset_ms: 0,
                            duration_ms: 0,
                            ttft_ms: None,
                            model: None,
                            input_tokens: 0,
                            output_tokens: 0,
                            cache_read_tokens: 0,
                            cost_usd: 0.0,
                            status: if err { "error" } else { "ok" }.to_string(),
                            error_message: state["error"].as_str().map(str::to_string),
                        });
                    }
                }
            }
        }
    }

    if let Some(t) = finalize_task(
        (tasks.len() + 1) as u32,
        cur_prompt.take(),
        cur_spans,
        cur_start,
    ) {
        tasks.push(t);
    }
    Some(tasks)
}

// ---------------------------------------------------------------------------
// 5. Gemini CLI (`gemini`) trace parser
// ---------------------------------------------------------------------------

fn extract_gemini_trace(session_id: &str, turns: &[SessionTurnRow]) -> Option<Vec<TaskTrace>> {
    let meta = crate::session_manager::providers::gemini::scan_sessions()
        .into_iter()
        .find(|s| s.session_id == session_id)?;
    let path = meta.source_path?;
    let raw = fs::read_to_string(path).ok()?;
    let doc = crate::session_manager::providers::gemini::parse_session_document(&raw)?;
    let messages = doc["messages"].as_array()?;

    let mut matcher = TurnMatcher::new(turns);
    let mut tasks = Vec::new();
    let mut cur_prompt: Option<String> = None;
    let mut cur_start: Option<i64> = None;
    let mut cur_spans: Vec<TraceSpan> = Vec::new();
    let mut last_ts: Option<i64> = None;

    for (idx, msg) in messages.iter().enumerate() {
        let Some(ts_ms) = parse_timestamp_to_ms(&msg["timestamp"]) else {
            continue;
        };
        let mtype = msg["type"].as_str().unwrap_or("");
        if mtype == "user" {
            if let Some(t) = finalize_task(
                (tasks.len() + 1) as u32,
                cur_prompt.take(),
                std::mem::take(&mut cur_spans),
                cur_start.take(),
            ) {
                tasks.push(t);
            }
            cur_prompt = msg["content"].as_str().and_then(clean_title).or_else(|| {
                msg["content"].as_array().and_then(|arr| {
                    arr.iter()
                        .filter_map(|v| v["text"].as_str())
                        .find_map(clean_title)
                })
            });
            cur_start = Some(ts_ms);
            last_ts = Some(ts_ms);
            continue;
        }
        if mtype == "gemini" {
            let tokens = &msg["tokens"];
            let input_tokens = tokens["input"].as_u64().unwrap_or(0);
            let output_tokens = tokens["output"].as_u64().unwrap_or(0);
            let cache_read = tokens["cached"].as_u64().unwrap_or(0);
            let matched = matcher.match_turn(ts_ms, output_tokens);
            let llm_start = last_ts
                .filter(|&prev| ts_ms > prev && ts_ms - prev <= MAX_LLM_FALLBACK_MS)
                .unwrap_or(ts_ms - matched.map(|m| m.latency_ms as i64).unwrap_or(1_000));
            let model = msg["model"]
                .as_str()
                .map(str::to_string)
                .or_else(|| matched.map(|m| m.model.clone()))
                .unwrap_or_else(|| "gemini".to_string());
            let tool_calls = msg["toolCalls"].as_array();

            cur_spans.push(TraceSpan {
                id: format!("gem-llm-{idx}"),
                kind: "llm".to_string(),
                depth: 0,
                label: model.clone(),
                detail: tool_calls
                    .filter(|a| !a.is_empty())
                    .map(|a| format!("调用 {} 个工具", a.len()))
                    .or_else(|| Some("文本回复".to_string())),
                start_ms: llm_start,
                end_ms: ts_ms.max(llm_start + 100),
                offset_ms: 0,
                duration_ms: 0,
                ttft_ms: matched.and_then(|m| m.first_token_ms),
                model: Some(model),
                input_tokens: input_tokens.saturating_sub(cache_read),
                output_tokens,
                cache_read_tokens: cache_read,
                cost_usd: matched.map(|m| m.total_cost_usd).unwrap_or(0.0),
                status: "ok".to_string(),
                error_message: None,
            });

            if let Some(arr) = tool_calls {
                for (tc_idx, tc) in arr.iter().enumerate() {
                    let name = tc["name"].as_str().unwrap_or("tool").to_string();
                    let detail = extract_tool_detail(&tc["args"]);
                    let t_start = parse_timestamp_to_ms(&tc["timestamp"]).unwrap_or(ts_ms);
                    let err = matches!(
                        tc["status"].as_str(),
                        Some("error" | "failed" | "cancelled")
                    );
                    cur_spans.push(TraceSpan {
                        id: format!("gem-tool-{idx}-{tc_idx}"),
                        kind: if is_subagent_tool(&name) {
                            "subagent"
                        } else {
                            "tool"
                        }
                        .to_string(),
                        depth: 1,
                        label: name,
                        detail,
                        start_ms: t_start,
                        end_ms: t_start + 300,
                        offset_ms: 0,
                        duration_ms: 0,
                        ttft_ms: None,
                        model: None,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cost_usd: 0.0,
                        status: if err { "error" } else { "ok" }.to_string(),
                        error_message: err.then(|| "工具调用异常".to_string()),
                    });
                }
            }
            last_ts = Some(ts_ms);
        }
    }

    if let Some(t) = finalize_task(
        (tasks.len() + 1) as u32,
        cur_prompt.take(),
        cur_spans,
        cur_start,
    ) {
        tasks.push(t);
    }
    Some(tasks)
}

// ---------------------------------------------------------------------------
// 6. Universal fallback: build Task Waterfalls from `proxy_request_logs` turns
// ---------------------------------------------------------------------------

pub(crate) fn build_tasks_from_turns(
    turns: &[SessionTurnRow],
    session_title: Option<&str>,
) -> Vec<TaskTrace> {
    if turns.is_empty() {
        return Vec::new();
    }
    let mut tasks = Vec::new();
    let mut cur_spans: Vec<TraceSpan> = Vec::new();
    let mut prev_end_ms: Option<i64> = None;

    for (idx, turn) in turns.iter().enumerate() {
        let dur = (turn.latency_ms as i64).clamp(200, MAX_LLM_FALLBACK_MS);
        let end_ms = turn.created_at * 1000;
        let start_ms = end_ms - dur;

        if let Some(pe) = prev_end_ms {
            let gap = start_ms - pe;
            if gap > FALLBACK_TASK_GAP_MS {
                let prompt = if tasks.is_empty() {
                    session_title.map(str::to_string)
                } else {
                    Some(format!("任务段 #{}", tasks.len() + 1))
                };
                if let Some(t) = finalize_task(
                    (tasks.len() + 1) as u32,
                    prompt,
                    std::mem::take(&mut cur_spans),
                    None,
                ) {
                    tasks.push(t);
                }
            } else if gap >= 500 {
                cur_spans.push(TraceSpan {
                    id: format!("gap-{idx}"),
                    kind: "gap".to_string(),
                    depth: 1,
                    label: "工具执行 / 轮次间隔".to_string(),
                    detail: Some("估算间隙".to_string()),
                    start_ms: pe,
                    end_ms: start_ms,
                    offset_ms: 0,
                    duration_ms: 0,
                    ttft_ms: None,
                    model: None,
                    input_tokens: 0,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cost_usd: 0.0,
                    status: "ok".to_string(),
                    error_message: None,
                });
            }
        }

        let is_err = turn.status_code != 0 && turn.status_code >= 400;
        cur_spans.push(TraceSpan {
            id: turn.request_id.clone(),
            kind: "llm".to_string(),
            depth: 0,
            label: turn.model.clone(),
            detail: Some(format!("第 {} 轮", idx + 1)),
            start_ms,
            end_ms,
            offset_ms: 0,
            duration_ms: 0,
            ttft_ms: turn.first_token_ms,
            model: Some(turn.model.clone()),
            input_tokens: turn.fresh_input,
            output_tokens: turn.output_tokens,
            cache_read_tokens: turn.cache_read_tokens,
            cost_usd: turn.total_cost_usd,
            status: if is_err { "error" } else { "ok" }.to_string(),
            error_message: turn.error_message.clone(),
        });
        prev_end_ms = Some(end_ms);
    }

    let prompt = if tasks.is_empty() {
        session_title.map(str::to_string)
    } else {
        Some(format!("任务段 #{}", tasks.len() + 1))
    };
    if let Some(t) = finalize_task((tasks.len() + 1) as u32, prompt, cur_spans, None) {
        tasks.push(t);
    }
    cap_and_reindex(tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agy_trace_extracts_tasks_and_parallel_tools_with_union_duration() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("transcript.jsonl");
        let lines = [
            r#"{"step_index":0,"source":"USER_EXPLICIT","type":"USER_INPUT","status":"DONE","created_at":"2026-11-21T13:12:07Z","content":"<USER_REQUEST>\n探索项目\n</USER_REQUEST>"}"#,
            r#"{"step_index":1,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:07Z","model":"gemini-4","input_tokens":1000,"cache_read_tokens":500,"output_tokens":120,"tool_calls":[{"name":"read_file","args":{"Path":"\"/repo/README.md\""}},{"name":"run_command","args":{"toolSummary":"\"Check git status\""}}]}"#,
            r#"{"step_index":2,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:12:12Z","content":"Created At: 2026-11-21T13:12:12Z\nCompleted At: 2026-11-21T13:12:13Z\n# README"}"#,
            r#"{"step_index":3,"source":"MODEL","type":"GENERIC","status":"DONE","created_at":"2026-11-21T13:12:12Z","content":"Created At: 2026-11-21T13:12:12Z\nCompleted At: 2026-11-21T13:12:15Z\nCommand exited with code 0."}"#,
            r#"{"step_index":4,"source":"MODEL","type":"PLANNER_RESPONSE","status":"DONE","created_at":"2026-11-21T13:12:15Z","model":"gemini-4","input_tokens":2000,"cache_read_tokens":1500,"output_tokens":400,"content":"完成"}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();
        let steps = parse_agy_steps(&file).unwrap();
        let tasks = build_agy_tasks(&steps, &[]);
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.prompt.as_deref(), Some("探索项目"));
        assert_eq!(t.llm_calls, 2);
        assert_eq!(t.tool_calls, 2);
        // Parallel tools ran 12->13 (1s) and 12->15 (3s); union must be 3000ms, not 4000ms.
        assert_eq!(t.tool_ms, 3000);
        assert_eq!(t.spans.len(), 4);
        assert_eq!(t.spans[1].detail.as_deref(), Some("README.md"));
        assert_eq!(t.spans[2].detail.as_deref(), Some("Check git status"));
    }

    #[test]
    fn claude_trace_matches_tool_use_and_tool_result() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s1.jsonl");
        let lines = [
            r#"{"type":"user","timestamp":"2026-11-21T10:00:00Z","message":{"role":"user","content":"Fix bug"}}"#,
            r#"{"type":"assistant","timestamp":"2026-11-21T10:00:04Z","message":{"model":"claude-sonnet-4","usage":{"input_tokens":500,"output_tokens":80,"cache_read_input_tokens":2000},"content":[{"type":"tool_use","id":"tu-1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-11-21T10:00:09Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tu-1","is_error":true}]}}"#,
        ];
        fs::write(&file, lines.join("\n")).unwrap();
        let tasks = parse_claude_trace_file(&file, &[]).unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].prompt.as_deref(), Some("Fix bug"));
        assert_eq!(tasks[0].llm_calls, 1);
        assert_eq!(tasks[0].tool_calls, 1);
        assert_eq!(tasks[0].error_count, 1);
        assert_eq!(tasks[0].spans[1].label, "Bash");
        assert_eq!(tasks[0].spans[1].duration_ms, 5000);
        assert_eq!(tasks[0].spans[1].status, "error");
    }

    #[test]
    fn fallback_splits_turns_by_idle_gap() {
        let mk = |id: &str, ts: i64| SessionTurnRow {
            request_id: id.to_string(),
            created_at: ts,
            model: "deepseek-chat".to_string(),
            fresh_input: 100,
            output_tokens: 50,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            total_cost_usd: 0.01,
            latency_ms: 2000,
            first_token_ms: Some(400),
            status_code: 200,
            data_source: "dsh".to_string(),
            error_message: None,
        };
        let turns = vec![mk("r1", 1000), mk("r2", 1010), mk("r3", 1300)];
        let tasks = build_tasks_from_turns(&turns, Some("重构模块"));
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].llm_calls, 2);
        assert_eq!(tasks[1].llm_calls, 1);
    }
}
