//! Claude Code stream-json output → normalized events.
//!
//! With `--include-partial-messages` every content block arrives twice: as a `stream_event`
//! lifecycle (`content_block_start`, deltas, `content_block_stop`), and as an `assistant`
//! message holding just that block, which Claude prints before the block's stop. Deltas are
//! forwarded under the item id `<message id>:<block index>`; the final message reuses the id of
//! the block still open, so it replaces the streamed text instead of repeating it.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::model::*;
use crate::policy;
use crate::{clip, now_ms};

/// Tool output kept in events; the full output stays in the CLI's transcript.
const OUTPUT_CLIP: usize = 8 * 1024;

/// Something the session has to act on besides forwarding events.
#[derive(Debug)]
pub enum Control {
    /// The answer to one of our control requests.
    Response {
        request_id: String,
        result: Result<Value, String>,
    },
    /// A permission request the session must answer (also reported as an event).
    Approval { request_id: String, input: Value },
    /// A control request Brigadier does not serve; the session answers with an error.
    Unsupported { request_id: String, subtype: String },
    /// Claude withdrew a pending request (the turn was interrupted).
    Cancelled { request_id: String },
}

#[derive(Debug)]
pub enum Output {
    Event(ProviderEvent),
    Control(Control),
}

#[derive(Debug, Clone)]
enum BlockKind {
    Text,
    Thinking,
    Tool { id: String, name: String },
    Other,
}

#[derive(Debug)]
struct OpenBlock {
    kind: BlockKind,
    item_id: String,
    /// Tool input as streamed by `input_json_delta`.
    json: String,
    /// The final `assistant` message for this block was seen.
    finalized: bool,
}

#[derive(Debug, Clone)]
struct ToolUse {
    name: String,
    input: Value,
}

/// Latest `rate_limit_info`, used to tell a usage limit from throttling.
#[derive(Debug, Clone, Default)]
struct LimitState {
    rejected: bool,
    window: Option<String>,
    resets_at_ms: Option<i64>,
}

#[derive(Default)]
pub struct Parser {
    /// A session reported its own start; `system/init` is then not a start.
    started: bool,
    message_id: Option<String>,
    blocks: HashMap<u64, OpenBlock>,
    /// Final messages seen per message id, for streams without partial messages.
    finals: HashMap<String, u64>,
    tools: HashMap<String, ToolUse>,
    limit: LimitState,
    quota: Vec<QuotaWindow>,
    context_window: Option<i64>,
    /// The context size last reported, to repeat once the window is known.
    context_used: Option<i64>,
    totals: TokenUsage,
    interrupting: bool,
    turn_active: bool,
    /// Messages written to Claude that it has not taken yet (echoed back).
    unechoed: u32,
    /// An error was already reported for the running turn.
    turn_error: bool,
    turn_started_ms: Option<i64>,
    /// `/compact` was written and Claude has not started compacting yet.
    compact_asked: bool,
    /// The compaction running now: whether Claude started it on its own.
    compacting: Option<bool>,
    /// A compaction failed: Claude says why once more, as a reply of its own, which is dropped.
    compact_failed: bool,
}

impl Parser {
    /// A parser for a live session that announced its own start.
    pub fn live() -> Self {
        Self {
            started: true,
            ..Self::default()
        }
    }

    /// A parser for a recording.
    pub fn replay() -> Self {
        Self::default()
    }

    /// Whether Claude is working on a turn.
    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    /// A message was written to Claude; it echoes it when it takes it.
    pub fn wrote_message(&mut self) {
        self.unechoed += 1;
    }

    /// Writing a message failed: Claude never saw it.
    pub fn write_failed(&mut self) {
        self.unechoed = self.unechoed.saturating_sub(1);
    }

    /// `/compact` is about to be written: the compaction it starts was asked for.
    pub fn compact_requested(&mut self) {
        self.compact_asked = true;
    }

    pub fn interrupt_requested(&mut self) {
        if self.turn_active {
            self.interrupting = true;
        }
    }

    pub fn feed(&mut self, line: &str) -> Vec<Output> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => {
                return vec![notice(
                    NoticeLevel::Warning,
                    format!("unparsed Claude output: {}", clip(line, 300)),
                )];
            }
        };
        let mut out = Vec::new();
        match str_of(&value, "type") {
            Some("stream_event") => {
                if let Some(event) = value.get("event") {
                    self.stream_event(event, &mut out);
                }
            }
            Some("assistant") => self.assistant(&value, &mut out),
            Some("user") => self.user(&value, &mut out),
            Some("system") => self.system(&value, &mut out),
            Some("result") => self.result(&value, &mut out),
            Some("rate_limit_event") => self.rate_limit(&value, &mut out),
            Some("control_request") => self.control_request(&value, &mut out),
            Some("control_response") => {
                if let Some(response) = value.get("response") {
                    let request_id = str_of(response, "request_id")
                        .unwrap_or_default()
                        .to_owned();
                    let result = match str_of(response, "subtype") {
                        Some("success") => {
                            Ok(response.get("response").cloned().unwrap_or(Value::Null))
                        }
                        _ => Err(str_of(response, "error")
                            .unwrap_or("request failed")
                            .to_owned()),
                    };
                    out.push(Output::Control(Control::Response { request_id, result }));
                }
            }
            Some("control_cancel_request") => {
                if let Some(request_id) = str_of(&value, "request_id") {
                    out.push(Output::Control(Control::Cancelled {
                        request_id: request_id.to_owned(),
                    }));
                }
            }
            // A long tool still runs: nothing to show, but the session is alive. A heartbeat
            // (sent every 30 s while a foreground command runs) only says the command has not
            // ended, not that anything moves: a hung command must still look silent to the
            // stall watchdog, which allows a running command longer.
            Some("tool_progress") if !is_heartbeat(&value) => progress(&value, &mut out),
            // Heartbeats, prompt suggestions: nothing to report.
            _ => {}
        }
        out
    }

    fn stream_event(&mut self, event: &Value, out: &mut Vec<Output>) {
        match str_of(event, "type") {
            Some("message_start") => {
                self.message_id = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.blocks.clear();
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                let kind = match str_of(&block, "type") {
                    Some("text") => BlockKind::Text,
                    Some("thinking") => BlockKind::Thinking,
                    Some("tool_use" | "server_tool_use" | "mcp_tool_use") => BlockKind::Tool {
                        id: str_of(&block, "id").unwrap_or_default().to_owned(),
                        name: str_of(&block, "name").unwrap_or("tool").to_owned(),
                    },
                    _ => BlockKind::Other,
                };
                let item_id = match &kind {
                    BlockKind::Tool { id, .. } => id.clone(),
                    _ => self.block_item_id(index),
                };
                // Tool input (especially a delegation spec) may take seconds to generate.
                // Show the call immediately; its final block fills in the input once ready.
                if let BlockKind::Tool { id, name } = &kind
                    && matches!(tool_kind(name), ToolKind::Other)
                {
                    self.tool_started(id.clone(), name.clone(), Value::Null, out);
                }
                self.blocks.insert(
                    index,
                    OpenBlock {
                        kind,
                        item_id,
                        json: String::new(),
                        finalized: false,
                    },
                );
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let Some(block) = self.blocks.get_mut(&index) else {
                    return;
                };
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                match str_of(&delta, "type") {
                    Some("text_delta") => {
                        let text = str_of(&delta, "text").unwrap_or_default();
                        if !text.is_empty() {
                            out.push(Output::Event(ProviderEvent::MessageDelta {
                                item_id: block.item_id.clone(),
                                text: text.to_owned(),
                            }));
                        }
                    }
                    Some("thinking_delta") => {
                        let text = str_of(&delta, "thinking").unwrap_or_default();
                        if !text.is_empty() {
                            out.push(Output::Event(ProviderEvent::ReasoningDelta {
                                item_id: block.item_id.clone(),
                                text: text.to_owned(),
                            }));
                        }
                    }
                    Some("input_json_delta") => {
                        block
                            .json
                            .push_str(str_of(&delta, "partial_json").unwrap_or_default());
                    }
                    _ => {}
                }
            }
            Some("content_block_stop") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                // A tool block whose final message never came: report it from the stream.
                if let Some(block) = self.blocks.remove(&index)
                    && !block.finalized
                    && let BlockKind::Tool { id, name } = block.kind
                {
                    let input = serde_json::from_str(&block.json).unwrap_or(Value::Null);
                    self.tool_started(id, name, input, out);
                }
            }
            Some("message_delta") => {
                if let Some(usage) = event.get("usage") {
                    self.context_from_usage(usage, out);
                }
            }
            _ => {}
        }
    }

    fn block_item_id(&self, index: u64) -> String {
        format!(
            "{}:{index}",
            self.message_id.as_deref().unwrap_or("message")
        )
    }

    fn assistant(&mut self, value: &Value, out: &mut Vec<Output>) {
        let message = value.get("message").cloned().unwrap_or(Value::Null);
        if let Some(code) = str_of(value, "error") {
            let text = content_text(&message);
            self.turn_error = true;
            out.push(Output::Event(ProviderEvent::Error {
                error: self.classify(code, text),
            }));
            return;
        }
        // Why a compaction failed, said again as a reply: already reported with it.
        if std::mem::take(&mut self.compact_failed)
            && str_of(&message, "model") == Some("<synthetic>")
        {
            return;
        }
        // Subagent output is summarized by its tool result; meanwhile it shows the session is
        // alive.
        if value
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            progress(value, out);
            return;
        }
        let message_id = str_of(&message, "id").unwrap_or("message").to_owned();
        let Some(blocks) = message.get("content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            let item_id = self.final_item_id(&message_id, block);
            match str_of(block, "type") {
                Some("text") => {
                    let text = str_of(block, "text").unwrap_or_default();
                    if !text.is_empty() {
                        out.push(Output::Event(ProviderEvent::Message {
                            item_id,
                            role: Role::Assistant,
                            text: text.to_owned(),
                        }));
                    }
                }
                Some("thinking") => {
                    let text = str_of(block, "thinking").unwrap_or_default();
                    if !text.is_empty() {
                        out.push(Output::Event(ProviderEvent::Reasoning {
                            item_id,
                            text: text.to_owned(),
                        }));
                    }
                }
                Some("tool_use" | "server_tool_use" | "mcp_tool_use") => {
                    let name = str_of(block, "name").unwrap_or("tool").to_owned();
                    let input = block.get("input").cloned().unwrap_or(Value::Null);
                    self.tool_started(item_id, name, input, out);
                }
                _ => {}
            }
        }
    }

    /// The item id for a block in a final `assistant` message: the open streamed block's, so
    /// the final text replaces the deltas.
    fn final_item_id(&mut self, message_id: &str, block: &Value) -> String {
        if let Some(id) = str_of(block, "id")
            && matches!(
                str_of(block, "type"),
                Some("tool_use" | "server_tool_use" | "mcp_tool_use")
            )
        {
            if let Some(open) = self.blocks.values_mut().find(|open| open.item_id == id) {
                open.finalized = true;
            }
            return id.to_owned();
        }
        if self.message_id.as_deref() == Some(message_id)
            && let Some((_, open)) = self
                .blocks
                .iter_mut()
                .filter(|(_, open)| !open.finalized && !matches!(open.kind, BlockKind::Tool { .. }))
                .min_by_key(|(index, _)| **index)
        {
            open.finalized = true;
            return open.item_id.clone();
        }
        let count = self.finals.entry(message_id.to_owned()).or_default();
        let id = format!("{message_id}:{count}");
        *count += 1;
        id
    }

    fn tool_started(&mut self, id: String, name: String, input: Value, out: &mut Vec<Output>) {
        if let Some(known) = self.tools.get(&id)
            && (known.input == input || !known.input.is_null())
        {
            return;
        }
        let event = match tool_kind(&name) {
            ToolKind::Command => ProviderEvent::Command {
                item_id: id.clone(),
                command: str_of(&input, "command").unwrap_or_default().to_owned(),
                cwd: None,
                status: ItemStatus::InProgress,
                exit_code: None,
                output: None,
                duration_ms: None,
            },
            ToolKind::FileChange => ProviderEvent::FileChanges {
                item_id: id.clone(),
                changes: file_changes(&name, &input),
                status: ItemStatus::InProgress,
            },
            ToolKind::Other => ProviderEvent::ToolCall {
                item_id: id.clone(),
                name: name.clone(),
                input: (!input.is_null()).then(|| clip(&input.to_string(), OUTPUT_CLIP)),
                status: ItemStatus::InProgress,
                output: None,
            },
        };
        self.tools.insert(id, ToolUse { name, input });
        out.push(Output::Event(event));
    }

    fn user(&mut self, value: &Value, out: &mut Vec<Output>) {
        if value
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            progress(value, out);
            return;
        }
        // With `--replay-user-messages` Claude echoes each message when it takes it: while idle
        // that starts a turn, during a turn it was folded into the running one (a steer).
        if value.get("isReplay").and_then(Value::as_bool) == Some(true) {
            self.unechoed = self.unechoed.saturating_sub(1);
            let text = content_text(value.get("message").unwrap_or(&Value::Null));
            // What a slash command printed ("Compacted"), not something the user said.
            if text.starts_with("<local-command-") {
                return;
            }
            if !self.turn_active {
                self.turn_active = true;
                self.turn_error = false;
                self.interrupting = false;
                self.turn_started_ms = Some(now_ms());
                out.push(Output::Event(ProviderEvent::TurnStarted { turn_id: None }));
            }
            out.push(Output::Event(ProviderEvent::Message {
                item_id: str_of(value, "uuid").unwrap_or_default().to_owned(),
                role: Role::User,
                text,
            }));
            return;
        }
        let Some(blocks) = value.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            if str_of(block, "type") != Some("tool_result") {
                continue;
            }
            let Some(id) = str_of(block, "tool_use_id") else {
                continue;
            };
            let Some(tool) = self.tools.get(id).cloned() else {
                continue;
            };
            let failed = block.get("is_error").and_then(Value::as_bool) == Some(true);
            let status = if failed {
                ItemStatus::Failed
            } else {
                ItemStatus::Completed
            };
            let output = clip(&content_text(block), OUTPUT_CLIP);
            let event = match tool_kind(&tool.name) {
                ToolKind::Command => ProviderEvent::Command {
                    item_id: id.to_owned(),
                    command: str_of(&tool.input, "command")
                        .unwrap_or_default()
                        .to_owned(),
                    cwd: None,
                    status,
                    exit_code: exit_code(&output, failed),
                    output: Some(output),
                    duration_ms: None,
                },
                ToolKind::FileChange => ProviderEvent::FileChanges {
                    item_id: id.to_owned(),
                    changes: file_changes(&tool.name, &tool.input),
                    status,
                },
                ToolKind::Other => ProviderEvent::ToolCall {
                    item_id: id.to_owned(),
                    name: tool.name.clone(),
                    input: Some(clip(&tool.input.to_string(), OUTPUT_CLIP)),
                    status,
                    output: Some(output),
                },
            };
            out.push(Output::Event(event));
        }
    }

    fn system(&mut self, value: &Value, out: &mut Vec<Output>) {
        match str_of(value, "subtype") {
            Some("init") => {
                if !self.started {
                    self.started = true;
                    out.push(Output::Event(ProviderEvent::SessionStarted {
                        native_id: str_of(value, "session_id").unwrap_or_default().to_owned(),
                        model: str_of(value, "model").map(str::to_owned),
                        cwd: str_of(value, "cwd").map(str::to_owned),
                        cli_version: str_of(value, "claude_code_version").map(str::to_owned),
                    }));
                }
            }
            Some("api_retry") => {
                let code = str_of(value, "error").unwrap_or("unknown");
                let attempt = value.get("attempt").and_then(Value::as_i64).unwrap_or(0);
                let max = value
                    .get("max_retries")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let delay = value
                    .get("retry_delay_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let mut error = self.classify(
                    code,
                    format!("API request failed ({code}); retry {attempt} of {max} in {delay} ms"),
                );
                error.will_retry = true;
                out.push(Output::Event(ProviderEvent::Error { error }));
            }
            Some("status") if str_of(value, "status") == Some("compacting") => {
                self.compaction_started(out)
            }
            Some("status") if str_of(value, "compact_result") == Some("failed") => {
                let automatic = self.compacting.take().unwrap_or(false);
                // No echo comes for the `/compact` that failed.
                if !automatic {
                    self.unechoed = self.unechoed.saturating_sub(1);
                }
                self.compact_failed = true;
                out.push(Output::Event(ProviderEvent::CompactionEnded {
                    automatic,
                    tokens_before: None,
                    tokens_after: None,
                    error: Some(
                        str_of(value, "compact_error")
                            .unwrap_or("Claude could not compact the conversation")
                            .to_owned(),
                    ),
                }));
            }
            Some("compact_boundary") => {
                let metadata = value.get("compact_metadata").unwrap_or(&Value::Null);
                let automatic = self
                    .compacting
                    .take()
                    .unwrap_or_else(|| str_of(metadata, "trigger") == Some("auto"));
                let tokens_after = metadata.get("post_tokens").and_then(Value::as_i64);
                out.push(Output::Event(ProviderEvent::CompactionEnded {
                    automatic,
                    tokens_before: metadata.get("pre_tokens").and_then(Value::as_i64),
                    tokens_after,
                    error: None,
                }));
                if let Some(used) = tokens_after {
                    self.context_used = Some(used);
                    out.push(Output::Event(ProviderEvent::ContextSize {
                        used_tokens: used,
                        window_tokens: self.context_window,
                    }));
                }
            }
            Some(
                subtype @ ("model_fallback"
                | "model_refusal_fallback"
                | "model_consent_fallback"
                | "model_refusal_no_fallback"),
            ) => {
                let detail = ["message", "reason", "fallback_model"]
                    .iter()
                    .find_map(|key| str_of(value, key))
                    .unwrap_or_default();
                out.push(notice(
                    NoticeLevel::Warning,
                    format!("Claude: {} {detail}", subtype.replace('_', " "))
                        .trim_end()
                        .to_owned(),
                ));
            }
            // A background task or sub-agent at work.
            Some("task_started" | "task_progress") => progress(value, out),
            _ => {}
        }
    }

    /// Claude began compacting: in the running turn when it compacts on its own, else in a
    /// turn of its own (Claude echoes the `/compact` only once it is done).
    fn compaction_started(&mut self, out: &mut Vec<Output>) {
        let automatic = !std::mem::take(&mut self.compact_asked);
        if !self.turn_active {
            self.turn_active = true;
            self.turn_error = false;
            self.interrupting = false;
            self.turn_started_ms = Some(now_ms());
            out.push(Output::Event(ProviderEvent::TurnStarted { turn_id: None }));
        }
        self.compacting = Some(automatic);
        out.push(Output::Event(ProviderEvent::CompactionStarted {
            automatic,
        }));
    }

    fn result(&mut self, value: &Value, out: &mut Vec<Output>) {
        let subtype = str_of(value, "subtype").unwrap_or("success");
        let is_error = value.get("is_error").and_then(Value::as_bool) == Some(true);
        let interrupted = std::mem::take(&mut self.interrupting);
        // A message steered in too late for this step is still to come: Claude answers it in
        // a turn of its own right after, which carries on this one.
        let continuing = self.unechoed > 0 && !interrupted && subtype == "success" && !is_error;
        if !continuing {
            self.turn_active = false;
        }

        if let Some(models) = value.get("modelUsage").and_then(Value::as_object) {
            let known = self.context_window;
            self.context_window = models
                .values()
                .filter_map(|model| model.get("contextWindow").and_then(Value::as_i64))
                .max()
                .or(self.context_window);
            // The first turn learns the window only now: say the size again with it.
            if known.is_none()
                && let (Some(window), Some(used)) = (self.context_window, self.context_used)
            {
                out.push(Output::Event(ProviderEvent::ContextSize {
                    used_tokens: used,
                    window_tokens: Some(window),
                }));
            }
        }
        let usage = value.get("usage").map(|usage| {
            let mut turn = token_usage(usage);
            turn.cost_usd = value.get("total_cost_usd").and_then(Value::as_f64);
            turn
        });
        if let Some(turn) = &usage {
            self.totals.input_tokens += turn.input_tokens;
            self.totals.cached_input_tokens += turn.cached_input_tokens;
            self.totals.cache_write_tokens += turn.cache_write_tokens;
            self.totals.output_tokens += turn.output_tokens;
            self.totals.reasoning_tokens += turn.reasoning_tokens;
            // `total_cost_usd` is cumulative for the session.
            self.totals.cost_usd = turn.cost_usd.or(self.totals.cost_usd);
            out.push(Output::Event(ProviderEvent::Usage {
                total: self.totals.clone(),
                last: None,
            }));
        }

        if continuing {
            return;
        }
        let status = if interrupted {
            TurnStatus::Interrupted
        } else if subtype == "success" && !is_error {
            TurnStatus::Completed
        } else {
            TurnStatus::Failed
        };
        if status == TurnStatus::Failed && !std::mem::take(&mut self.turn_error) {
            let text = str_of(value, "result")
                .map(str::to_owned)
                .or_else(|| {
                    value.get("errors").and_then(Value::as_array).map(|errors| {
                        errors
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                })
                .unwrap_or_else(|| subtype.replace('_', " "));
            let code = match subtype {
                "success" => "unknown",
                other => other,
            };
            out.push(Output::Event(ProviderEvent::Error {
                error: self.classify(code, text),
            }));
        }
        let duration_ms = value
            .get("duration_ms")
            .and_then(Value::as_i64)
            .or_else(|| {
                self.turn_started_ms
                    .take()
                    .map(|started| now_ms() - started)
            });
        self.turn_error = false;
        out.push(Output::Event(ProviderEvent::TurnCompleted {
            turn_id: None,
            status,
            duration_ms,
            usage,
        }));
    }

    fn rate_limit(&mut self, value: &Value, out: &mut Vec<Output>) {
        let Some(info) = value.get("rate_limit_info") else {
            return;
        };
        let status = str_of(info, "status").unwrap_or("allowed");
        // Only the subscription's own windows; overage and other kinds are not usage windows.
        let window = str_of(info, "rateLimitType")
            .filter(|id| is_window(id))
            .map(str::to_owned);
        let resets_at_ms = info
            .get("resetsAt")
            .and_then(Value::as_i64)
            .map(|s| s * 1000);

        if let Some(windows) = info.get("unifiedWindows").and_then(Value::as_object) {
            self.quota = windows
                .iter()
                .filter(|(id, _)| is_window(id))
                .map(|(id, window)| {
                    quota_window(
                        id,
                        // Reported as a fraction here and as a percentage by `get_usage`.
                        window
                            .get("utilization")
                            .and_then(Value::as_f64)
                            .map(|used| if used <= 1.0 { used * 100.0 } else { used })
                            .unwrap_or_default(),
                        window
                            .get("resetsAt")
                            .and_then(Value::as_i64)
                            .map(|s| s * 1000),
                    )
                })
                .collect();
        } else if let (Some(id), Some(used)) =
            (&window, info.get("utilization").and_then(Value::as_f64))
        {
            let used = if used <= 1.0 { used * 100.0 } else { used };
            match self.quota.iter_mut().find(|quota| &quota.id == id) {
                Some(quota) => quota.used_percent = used,
                None => self.quota.push(quota_window(id, used, resets_at_ms)),
            }
        }

        let rejected = status == "rejected";
        // A per-model weekly window running out refuses that model only: it shows as that
        // window used up, not as the provider at its limit.
        let scoped = window.as_deref().and_then(window_model);
        if rejected
            && scoped.is_some()
            && let Some(id) = &window
        {
            match self.quota.iter_mut().find(|quota| &quota.id == id) {
                Some(quota) => quota.used_percent = quota.used_percent.max(100.0),
                None => self.quota.push(quota_window(id, 100.0, resets_at_ms)),
            }
        }
        let provider_wide = rejected && scoped.is_none();
        // A scoped rejection is still a usage limit for this session's error (naming its
        // window, so the hand-off keeps the limit to that model).
        self.limit = LimitState {
            rejected,
            window: window.clone(),
            resets_at_ms,
        };
        out.push(Output::Event(ProviderEvent::RateLimits {
            quota: QuotaSnapshot {
                provider: ProviderKind::Claude,
                windows: self.quota.clone(),
                limit: provider_wide.then_some(LimitHit {
                    window,
                    resets_at_ms,
                    kind: LimitKind::UsageWindow,
                }),
                observed_at_ms: now_ms(),
                source: QuotaSource::Event,
            },
        }));
    }

    fn control_request(&mut self, value: &Value, out: &mut Vec<Output>) {
        let request_id = str_of(value, "request_id").unwrap_or_default().to_owned();
        let request = value.get("request").cloned().unwrap_or(Value::Null);
        let subtype = str_of(&request, "subtype").unwrap_or_default().to_owned();
        if subtype != "can_use_tool" {
            out.push(Output::Control(Control::Unsupported {
                request_id,
                subtype,
            }));
            return;
        }
        let tool = str_of(&request, "tool_name").unwrap_or("tool").to_owned();
        let input = request.get("input").cloned().unwrap_or(Value::Null);
        let escalation = input
            .get("dangerouslyDisableSandbox")
            .and_then(Value::as_bool)
            == Some(true);
        let kind = match tool_kind(&tool) {
            ToolKind::Command => ApprovalKind::Command,
            ToolKind::FileChange => ApprovalKind::FileChange,
            ToolKind::Other => ApprovalKind::Tool,
        };
        let reason = ["decision_reason", "description"]
            .iter()
            .find_map(|key| str_of(&request, key))
            .map(str::to_owned)
            .or_else(|| {
                str_of(&request, "blocked_path").map(|path| format!("needs access to {path}"))
            });
        // Brigadier keeps "Allow similar commands" itself, for the whole conversation: the
        // command's first words, or the host Claude's sandbox asks to reach.
        let grant = if tool == policy::NETWORK_TOOL {
            str_of(&input, "host").map(str::to_owned)
        } else {
            str_of(&input, "command")
                .filter(|_| tool == "Bash")
                .and_then(policy::command_prefix)
        };
        out.push(Output::Event(ProviderEvent::ApprovalRequested {
            request: ApprovalRequest {
                id: request_id.clone(),
                kind,
                tool: tool.clone(),
                command: str_of(&input, "command").map(str::to_owned),
                cwd: None,
                paths: file_changes(&tool, &input)
                    .into_iter()
                    .map(|change| change.path)
                    .chain(str_of(&request, "blocked_path").map(str::to_owned))
                    .collect(),
                reason,
                escalation,
                input: Some(clip(&input.to_string(), OUTPUT_CLIP)),
                grant,
            },
        }));
        out.push(Output::Control(Control::Approval { request_id, input }));
    }

    fn context_from_usage(&mut self, usage: &Value, out: &mut Vec<Output>) {
        let usage = token_usage(usage);
        let used = usage.input_tokens
            + usage.cached_input_tokens
            + usage.cache_write_tokens
            + usage.output_tokens;
        if used > 0 {
            self.context_used = Some(used);
            out.push(Output::Event(ProviderEvent::ContextSize {
                used_tokens: used,
                window_tokens: self.context_window,
            }));
        }
    }

    /// Classifies a Claude error code (the assistant `error` field, `api_retry.error`, a
    /// `result` subtype) together with the latest rate-limit state.
    fn classify(&self, code: &str, message: String) -> ProviderError {
        let lower = message.to_lowercase();
        let (kind, limit) = match code {
            "rate_limit" if self.limit.rejected || lower.contains("hit your") => (
                ErrorKind::UsageLimit,
                Some(LimitHit {
                    window: self.limit.window.clone(),
                    resets_at_ms: self.limit.resets_at_ms,
                    kind: LimitKind::UsageWindow,
                }),
            ),
            "rate_limit" => (ErrorKind::RateLimit, None),
            "overloaded" => (ErrorKind::Overloaded, None),
            "authentication_failed" | "oauth_org_not_allowed" => (ErrorKind::Auth, None),
            "billing_error" => (ErrorKind::Billing, None),
            "invalid_request"
                if lower.contains("prompt is too long") || lower.contains("context") =>
            {
                (ErrorKind::ContextWindow, None)
            }
            "invalid_request" => (ErrorKind::InvalidRequest, None),
            "server_error" | "error_during_execution" => (ErrorKind::Server, None),
            "error_max_budget_usd" => (ErrorKind::Billing, None),
            _ if lower.contains("usage limit") => (ErrorKind::UsageLimit, None),
            _ => (ErrorKind::Other, None),
        };
        ProviderError {
            kind,
            message: if message.trim().is_empty() {
                code.replace('_', " ")
            } else {
                clip(message.trim(), 2_000)
            },
            will_retry: false,
            limit,
            code: Some(code.to_owned()),
        }
    }
}

enum ToolKind {
    Command,
    FileChange,
    Other,
}

fn tool_kind(name: &str) -> ToolKind {
    match name {
        "Bash" | "PowerShell" => ToolKind::Command,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => ToolKind::FileChange,
        _ => ToolKind::Other,
    }
}

fn file_changes(tool: &str, input: &Value) -> Vec<FileChange> {
    let path = str_of(input, "file_path").or_else(|| str_of(input, "notebook_path"));
    let kind = match tool {
        "Write" => FileChangeKind::Add,
        "Edit" | "MultiEdit" | "NotebookEdit" => FileChangeKind::Update,
        _ => return Vec::new(),
    };
    path.map(|path| {
        vec![FileChange {
            path: path.to_owned(),
            kind,
        }]
    })
    .unwrap_or_default()
}

/// Claude reports a failed command's status as `Exit code N` at the top of its output.
fn exit_code(output: &str, failed: bool) -> Option<i32> {
    let first = output.lines().next().unwrap_or_default();
    first
        .strip_prefix("Exit code ")
        .and_then(|code| code.trim().parse().ok())
        .or((!failed).then_some(0))
}

fn token_usage(usage: &Value) -> TokenUsage {
    let int = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or_default();
    TokenUsage {
        input_tokens: int("input_tokens"),
        cached_input_tokens: int("cache_read_input_tokens"),
        cache_write_tokens: int("cache_creation_input_tokens"),
        output_tokens: int("output_tokens"),
        reasoning_tokens: usage
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        cost_usd: None,
    }
}

/// Text of a message or tool result: a string, or the text blocks of a content array (a tool
/// search's result names the tools it loaded).
fn content_text(value: &Value) -> String {
    match value.get("content") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| match block {
                Value::String(text) => Some(text.clone()),
                _ if str_of(block, "type") == Some("tool_reference") => {
                    str_of(block, "tool_name").map(|name| format!("Loaded tool: {name}"))
                }
                _ => str_of(block, "text").map(str::to_owned),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Claude's usage windows: the 5-hour session window, the weekly one, and weekly windows that
/// meter one model family (`seven_day_opus`). Other `rate_limits` entries (overage, extra
/// usage, internal buckets) are not windows work waits on.
fn is_window(id: &str) -> bool {
    id == "five_hour" || id == "seven_day" || window_model(id).is_some()
}

/// The model family a per-model weekly window meters (`seven_day_opus` → `opus`).
fn window_model(id: &str) -> Option<&str> {
    id.strip_prefix("seven_day_").filter(|family| {
        !family.is_empty()
            && family.chars().all(|c| c.is_ascii_lowercase())
            && !matches!(*family, "overage" | "oauth")
    })
}

fn quota_window(id: &str, used_percent: f64, resets_at_ms: Option<i64>) -> QuotaWindow {
    QuotaWindow {
        id: id.to_owned(),
        label: window_label(id),
        used_percent,
        resets_at_ms,
        window_minutes: window_minutes(id),
        bucket: None,
        model: window_model(id).map(str::to_owned),
    }
}

pub(crate) fn window_label(id: &str) -> String {
    match id {
        "five_hour" => "5-hour".into(),
        "seven_day" => "Weekly".into(),
        other => match window_model(other) {
            Some(family) => {
                let mut name = family.to_owned();
                if let Some(first) = name.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                format!("Weekly ({name})")
            }
            None => other.replace('_', " "),
        },
    }
}

pub(crate) fn window_minutes(id: &str) -> Option<i64> {
    if id == "five_hour" {
        Some(5 * 60)
    } else if id.starts_with("seven_day") {
        Some(7 * 24 * 60)
    } else {
        None
    }
}

/// Normalizes a `get_usage` answer. Its `limits` list is read first: the session window, the
/// weekly one and weekly windows scoped to one model (surface-scoped ones are left out); older
/// versions without it give the named windows (`five_hour`, `seven_day`,
/// `seven_day_<family>`). A provider-wide window at 100% is a limit.
pub(crate) fn usage_snapshot(answer: &Value) -> QuotaSnapshot {
    let rates = answer.get("rate_limits").filter(|rates| !rates.is_null());
    let available = answer
        .get("rate_limits_available")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let listed = rates
        .and_then(|rates| rates.get("limits"))
        .and_then(Value::as_array);
    let mut windows: Vec<QuotaWindow> = match (available, rates, listed) {
        (false, _, _) | (_, None, _) => Vec::new(),
        (true, _, Some(limits)) => limits.iter().filter_map(listed_window).collect(),
        (true, Some(rates), None) => rates
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(id, _)| is_window(id))
            .filter_map(|(id, window)| {
                let used = window.get("utilization").and_then(Value::as_f64)?;
                Some(quota_window(id, used, iso_ms(window.get("resets_at"))))
            })
            .collect(),
    };
    windows.sort_by_key(|window| {
        (
            window.model.is_some(),
            window.window_minutes.unwrap_or(i64::MAX),
        )
    });
    windows.dedup_by(|a, b| a.id == b.id);
    let limit = windows
        .iter()
        .find(|window| window.model.is_none() && window.used_percent >= 100.0)
        .map(|window| LimitHit {
            window: Some(window.id.clone()),
            resets_at_ms: window.resets_at_ms,
            kind: LimitKind::UsageWindow,
        });
    QuotaSnapshot {
        provider: ProviderKind::Claude,
        windows,
        limit,
        observed_at_ms: now_ms(),
        source: QuotaSource::Read,
    }
}

/// One entry of `get_usage`'s `limits` list as a window, if it is one work waits on.
fn listed_window(limit: &Value) -> Option<QuotaWindow> {
    let used = limit.get("percent").and_then(Value::as_f64)?;
    let resets_at_ms = iso_ms(limit.get("resets_at"));
    let scope = limit.get("scope").filter(|scope| !scope.is_null());
    match str_of(limit, "kind")? {
        "session" if scope.is_none() => Some(quota_window("five_hour", used, resets_at_ms)),
        "weekly_all" if scope.is_none() => Some(quota_window("seven_day", used, resets_at_ms)),
        "weekly_scoped" => {
            let model = scope?.get("model").filter(|model| !model.is_null())?;
            let name = str_of(model, "display_name").or_else(|| str_of(model, "id"))?;
            // The family word: "Fable" in "Fable", "Claude Fable 5.1" or "claude-fable-5-1".
            let family: String = name
                .split(|c: char| !c.is_ascii_alphanumeric())
                .find(|word| {
                    word.chars().any(|c| c.is_ascii_alphabetic())
                        && !word.eq_ignore_ascii_case("claude")
                })?
                .to_ascii_lowercase();
            Some(quota_window(
                &format!("seven_day_{family}"),
                used,
                resets_at_ms,
            ))
        }
        _ => None,
    }
}

fn iso_ms(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_str)
        .and_then(crate::time::parse_rfc3339_ms)
}

/// Work under way the transcript doesn't show (a sub-agent's message, a tool's or a task's
/// progress), under the tool call it belongs to.
fn progress(value: &Value, out: &mut Vec<Output>) {
    let item_id = ["parent_tool_use_id", "tool_use_id", "task_id"]
        .iter()
        .find_map(|key| str_of(value, key))
        .unwrap_or("progress")
        .to_owned();
    out.push(Output::Event(ProviderEvent::Progress { item_id }));
}

/// Whether a `tool_progress` line is only the CLI's heartbeat for a running tool.
fn is_heartbeat(value: &Value) -> bool {
    value.get("heartbeat").and_then(Value::as_bool) == Some(true)
        || str_of(value, "tool_use_id").is_some_and(|id| id.contains("-heartbeat-"))
}

fn notice(level: NoticeLevel, message: String) -> Output {
    Output::Event(ProviderEvent::Notice { level, message })
}

pub(crate) fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Builds a control request line.
pub fn control_request(request_id: &str, request: Map<String, Value>) -> String {
    serde_json::json!({
        "type": "control_request",
        "request_id": request_id,
        "request": request,
    })
    .to_string()
}

/// Builds a user message line from its content blocks.
pub fn user_message(content: Vec<Value>) -> String {
    serde_json::json!({
        "type": "user",
        "message": {"role": "user", "content": content},
        "parent_tool_use_id": null,
        "uuid": uuid::Uuid::new_v4().to_string(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tool_is_visible_before_its_arguments_finish_and_updates_once() {
        let mut parser = Parser::live();
        let start = json!({"type":"stream_event", "event":{"type":"content_block_start", "index":1,
            "content_block":{"type":"tool_use", "id":"call", "name":"mcp__brigadier__delegate_task", "input":{}}}});
        let events = parser.feed(&start.to_string());
        assert!(
            matches!(&events[..], [Output::Event(ProviderEvent::ToolCall { item_id, input: None, status: ItemStatus::InProgress, .. })] if item_id == "call")
        );
        let partial = json!({"type":"stream_event", "event":{"type":"content_block_delta", "index":1,
            "delta":{"type":"input_json_delta", "partial_json":"{\"title\":\"Read code\"}"}}});
        assert!(parser.feed(&partial.to_string()).is_empty());
        let final_block = json!({"type":"assistant", "message":{"id":"msg", "content":[
            {"type":"tool_use", "id":"call", "name":"mcp__brigadier__delegate_task", "input":{"title":"Read code"}}]}});
        let events = parser.feed(&final_block.to_string());
        assert!(
            matches!(&events[..], [Output::Event(ProviderEvent::ToolCall { input: Some(input), status: ItemStatus::InProgress, .. })] if input.contains("Read code"))
        );
        assert!(parser.feed(&final_block.to_string()).is_empty());
        let stop = json!({"type":"stream_event", "event":{"type":"content_block_stop", "index":1}});
        assert!(parser.feed(&stop.to_string()).is_empty());
        let result = json!({"type":"user", "message":{"content":[{"type":"tool_result", "tool_use_id":"call", "content":"Created task-1"}]}});
        let events = parser.feed(&result.to_string());
        assert!(
            matches!(&events[..], [Output::Event(ProviderEvent::ToolCall { input: Some(input), status: ItemStatus::Completed, .. })] if input.contains("Read code"))
        );
    }

    #[test]
    fn a_streamed_tool_without_a_final_message_keeps_its_input() {
        let mut parser = Parser::live();
        parser.feed(
            &json!({"type":"stream_event", "event":{"type":"content_block_start", "index":0,
            "content_block":{"type":"tool_use", "id":"read", "name":"Read", "input":{}}}})
            .to_string(),
        );
        parser.feed(
            &json!({"type":"stream_event", "event":{"type":"content_block_delta", "index":0,
            "delta":{"type":"input_json_delta", "partial_json":"{\"file_path\":\"README.md\"}"}}})
            .to_string(),
        );
        let events = parser.feed(
            &json!({"type":"stream_event", "event":{"type":"content_block_stop", "index":0}})
                .to_string(),
        );
        assert!(
            matches!(&events[..], [Output::Event(ProviderEvent::ToolCall { input: Some(input), .. })] if input.contains("README.md"))
        );
    }

    #[test]
    fn a_tool_search_result_names_the_tools_it_loaded() {
        let result = json!({ "content": [
            { "type": "tool_reference", "tool_name": "mcp__brigadier__code_search" },
            { "type": "text", "text": "done" },
        ]});
        assert_eq!(
            content_text(&result),
            "Loaded tool: mcp__brigadier__code_search\ndone"
        );
    }

    #[test]
    fn sub_agent_work_and_tool_progress_show_the_session_is_alive() {
        let mut parser = Parser::live();
        let progress = |parser: &mut Parser, line: Value| -> Vec<String> {
            parser
                .feed(&line.to_string())
                .into_iter()
                .filter_map(|output| match output {
                    Output::Event(ProviderEvent::Progress { item_id }) => Some(item_id),
                    _ => None,
                })
                .collect()
        };
        let tool = json!({
            "type": "tool_progress",
            "tool_use_id": "toolu_1",
            "tool_name": "Bash",
            "parent_tool_use_id": null,
            "elapsed_time_seconds": 30,
        });
        assert_eq!(progress(&mut parser, tool), ["toolu_1"]);
        // A heartbeat for a foreground command only says it has not ended: a hung command
        // stays silent.
        let heartbeat = json!({
            "type": "tool_progress",
            "tool_use_id": "toolu_5-heartbeat-0",
            "tool_name": "Bash",
            "parent_tool_use_id": "toolu_5",
            "elapsed_time_seconds": 30,
            "heartbeat": true,
        });
        assert!(parser.feed(&heartbeat.to_string()).is_empty());
        let unmarked = json!({
            "type": "tool_progress",
            "tool_use_id": "toolu_5-heartbeat-1",
            "tool_name": "Bash",
            "elapsed_time_seconds": 60,
        });
        assert!(parser.feed(&unmarked.to_string()).is_empty());
        let sub_agent = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_2",
            "message": { "id": "m1", "content": [{ "type": "text", "text": "Reading files" }] },
        });
        let outputs = parser.feed(&sub_agent.to_string());
        assert!(matches!(
            outputs.as_slice(),
            [Output::Event(ProviderEvent::Progress { item_id })] if item_id == "toolu_2"
        ));
        let result = json!({
            "type": "user",
            "parent_tool_use_id": "toolu_2",
            "message": { "content": [{ "type": "tool_result", "tool_use_id": "toolu_3", "content": "ok" }] },
        });
        assert_eq!(progress(&mut parser, result), ["toolu_2"]);
        let task = json!({
            "type": "system",
            "subtype": "task_progress",
            "task_id": "task_1",
            "tool_use_id": "toolu_4",
        });
        assert_eq!(progress(&mut parser, task), ["toolu_4"]);
    }
}
