//! Claude Code stream-json output → normalized events.
//!
//! With `--include-partial-messages` every content block arrives twice: as a `stream_event`
//! lifecycle (`content_block_start`, deltas, `content_block_stop`), and as an `assistant`
//! message holding just that block, which Claude prints before the block's stop. Deltas are
//! forwarded under the item id `<message id>:<block index>`; the final message reuses the id of
//! the block still open, so it replaces the streamed text instead of repeating it.

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::model::*;
use crate::policy;
use crate::{OUTPUT_CLIP, clip, now_ms};

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
    /// Subagents' (the Agent tool's) messages since the last result, each one's last usage by
    /// message id. Claude's `result` leaves them out of its `usage` (only its cumulative
    /// `modelUsage` has them), so the next result's turn adds them.
    subagent_usage: HashMap<String, TokenUsage>,
    /// Subagent messages already added to a turn: a line repeated after its result adds nothing.
    subagent_counted: HashSet<String>,
    /// The model calls since the last result: its assistant messages' ids, a subagent's
    /// included.
    turn_calls: HashSet<String>,
    /// Each model call's prompt since the last result, as first reported (research P0.1).
    turn_cache: Vec<CallCache>,
    /// The calls already in `turn_cache` or an earlier result's: a call's usage repeats on
    /// each of its lines.
    cache_seen: HashSet<String>,
    /// The last call's prompt per chain: the main agent's (`None`) and each subagent's.
    chain_prompts: HashMap<Option<String>, i64>,
    /// A compaction ended: the main agent's next call starts from its summary.
    compacted: bool,
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
    /// The session's `PreCompact` hook refuses compaction: one Claude starts on its own is
    /// reported only once it is done, or as refused.
    pub(crate) refuses_compaction: bool,
    /// Claude began compacting on its own in a session that refuses it: not reported yet.
    refusing: bool,
    /// A compaction failed: Claude says why once more, as a reply of its own, which is dropped.
    compact_failed: bool,
    /// The session's working directory (`system/init`), which its searches' relative paths
    /// start from.
    cwd: Option<String>,
    /// The background jobs Claude last said still run: each `task_notification` of one ends
    /// it alone, in the order Claude starts their turns (#221).
    background: Vec<String>,
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
                // Claude answers in a turn it started on its own (a background job of its own
                // ended): nothing of ours was echoed to start it.
                if str_of(&value, "parent_tool_use_id").is_none()
                    && value.pointer("/event/type").and_then(Value::as_str) == Some("message_start")
                {
                    self.begin_turn(&mut out);
                }
                if let (Some(id), Some(usage)) = (
                    value.pointer("/event/message/id").and_then(Value::as_str),
                    value.pointer("/event/message/usage"),
                ) {
                    self.call_seen(id, usage, str_of(&value, "parent_tool_use_id"));
                }
                if let Some(event) = value.get("event") {
                    self.stream_event(event, &mut out);
                }
            }
            Some("assistant") => self.assistant(&value, &mut out),
            Some("user") => self.user(&value, &mut out),
            Some("system") => self.system(&value, &mut out),
            Some("result") => self.result(&value, &mut out),
            Some("rate_limit_event") => self.rate_limit(&value, &mut out),
            Some("prompt_suggestion") => {
                if let Some(text) = str_of(&value, "suggestion")
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                {
                    out.push(Output::Event(ProviderEvent::PromptSuggestion {
                        text: text.to_owned(),
                    }));
                }
            }
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
                                phase: None,
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

    /// A model call's usage seen (on its `message_start` or a line of its message), in the
    /// main agent's chain or a subagent's (`parent`): its prompt is kept once, against the
    /// chain's call before it (research P0.1). A usage with no prompt yet is not its call's.
    fn call_seen(&mut self, id: &str, usage: &Value, parent: Option<&str>) {
        if self.cache_seen.contains(id) {
            return;
        }
        let used = token_usage(usage);
        let prompt = used.input_tokens + used.cached_input_tokens + used.cache_write_tokens;
        if prompt == 0 {
            return;
        }
        self.cache_seen.insert(id.to_owned());
        let previous_prompt = self.chain_prompts.insert(parent.map(str::to_owned), prompt);
        self.turn_cache.push(CallCache {
            prompt,
            cache_read: used.cached_input_tokens,
            cache_write: used.cache_write_tokens,
            previous_prompt,
            after_compaction: parent.is_none() && std::mem::take(&mut self.compacted),
            subagent: parent.is_some(),
        });
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
        // alive. Its tokens count towards the next result's turn.
        if value
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
        {
            if let (Some(id), Some(usage)) = (str_of(&message, "id"), message.get("usage"))
                && !self.subagent_counted.contains(id)
            {
                self.subagent_usage
                    .insert(id.to_owned(), token_usage(usage));
                self.turn_calls.insert(id.to_owned());
                self.call_seen(id, usage, str_of(value, "parent_tool_use_id"));
            }
            progress(value, out);
            return;
        }
        // A turn Claude started on its own, without partial messages.
        self.begin_turn(out);
        if let Some(id) = str_of(&message, "id") {
            self.turn_calls.insert(id.to_owned());
            if let Some(usage) = message.get("usage") {
                self.call_seen(id, usage, None);
            }
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
                kept: None,
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
                kept: None,
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
            self.begin_turn(out);
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
        // The structured result (`tool_use_result`) is the message's one tool result's.
        let results = blocks
            .iter()
            .filter(|block| str_of(block, "type") == Some("tool_result"))
            .count();
        let structured = value.get("tool_use_result").filter(|_| results == 1);
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
            // Whole: the event sender cuts it once it is redacted and kept (`crate::events`).
            let output = content_text(block);
            let kept = kept_output(&output, structured, failed);
            let event = match tool_kind(&tool.name) {
                // Claude's command output arrives at completion; Codex streams. A running
                // `Bash` gets no output on the way (claude 2.1.300, measured 2026-10-15): a
                // `task_started` line, a `tool_progress` heartbeat every 30 s with only the
                // time elapsed, a `task_notification`, then this result with all of it. So a
                // Claude command has no `CommandOutputDelta`: its output is here, once.
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
                    kept,
                },
                ToolKind::FileChange => {
                    let mut changes = file_changes(&tool.name, &tool.input);
                    if let [change] = changes.as_mut_slice() {
                        finish_change(change, structured, &tool.input);
                    }
                    ProviderEvent::FileChanges {
                        item_id: id.to_owned(),
                        changes,
                        status,
                    }
                }
                ToolKind::Other => ProviderEvent::ToolCall {
                    item_id: id.to_owned(),
                    name: tool.name.clone(),
                    input: Some(clip(&tool.input.to_string(), OUTPUT_CLIP)),
                    status,
                    output: Some(output),
                    kept,
                },
            };
            out.push(Output::Event(event));
            if !failed && let Some(looked) = self.looked(id, &tool, structured, block) {
                out.push(Output::Event(looked));
            }
        }
    }

    /// What a finished `Read`, `Grep`, `Glob` or `Bash` call read or searched (a command line's
    /// simple reads and searches, see [`crate::looked::shell_looked`]). Read: the lines the
    /// result holds (`file.startLine`, `numLines` against `totalLines`), else the ones asked
    /// for. Grep: its pattern, folder and file filter, and the files it names (`filenames`,
    /// or the `path:line:` prefixes of its content mode). Glob: its pattern and the files.
    fn looked(
        &self,
        id: &str,
        tool: &ToolUse,
        result: Option<&Value>,
        block: &Value,
    ) -> Option<ProviderEvent> {
        let input = &tool.input;
        let result = result.filter(|result| result.is_object());
        let filenames = || {
            result
                .and_then(|result| result.get("filenames"))
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .take(crate::looked::HITS_KEPT)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        let (reads, searches) = match tool.name.as_str() {
            "Read" => {
                let path = str_of(input, "file_path")?.to_owned();
                (
                    vec![FileRead {
                        path,
                        lines: read_lines(input, result),
                    }],
                    Vec::new(),
                )
            }
            "Grep" => {
                let mut hits = filenames();
                if hits.is_empty() {
                    let content = result
                        .and_then(|result| str_of(result, "content"))
                        .map(str::to_owned)
                        .unwrap_or_else(|| content_text(block));
                    let mode = result.and_then(|result| str_of(result, "mode"));
                    if matches!(mode, Some("content" | "count")) {
                        hits = crate::looked::output_hits(&content, SearchKind::Content);
                    }
                }
                let glob = str_of(input, "glob")
                    .map(str::to_owned)
                    .or_else(|| str_of(input, "type").map(|kind| format!("type:{kind}")));
                (
                    Vec::new(),
                    vec![FileSearch {
                        kind: SearchKind::Content,
                        pattern: str_of(input, "pattern").map(str::to_owned),
                        scope: str_of(input, "path").map(str::to_owned),
                        glob,
                        hits,
                    }],
                )
            }
            "Glob" => (
                Vec::new(),
                vec![FileSearch {
                    kind: SearchKind::Files,
                    pattern: str_of(input, "pattern").map(str::to_owned),
                    scope: str_of(input, "path").map(str::to_owned),
                    glob: None,
                    hits: filenames(),
                }],
            ),
            "Bash" => {
                let output = result
                    .and_then(|result| str_of(result, "stdout"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| content_text(block));
                crate::looked::shell_looked(str_of(input, "command")?, &output)
            }
            _ => return None,
        };
        if reads.is_empty() && searches.is_empty() {
            return None;
        }
        Some(ProviderEvent::Looked {
            item_id: id.to_owned(),
            cwd: self.cwd.clone(),
            reads,
            searches,
        })
    }

    fn system(&mut self, value: &Value, out: &mut Vec<Output>) {
        match str_of(value, "subtype") {
            Some("init") => {
                if let Some(cwd) = str_of(value, "cwd") {
                    self.cwd = Some(cwd.to_owned());
                }
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
                if self.refuses_compaction && !self.compact_asked {
                    self.refusing = true;
                } else {
                    self.compaction_started(out)
                }
            }
            // The refused compaction's status ends with nothing compacted.
            Some("status")
                if self.refusing
                    && value.get("status").is_none_or(Value::is_null)
                    && value.get("compact_result").is_none() =>
            {
                self.refusing = false;
                out.push(Output::Event(ProviderEvent::CompactionRefused));
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
                // Compacted after all, though the session refuses it: said as it happened.
                if std::mem::take(&mut self.refusing) {
                    self.compaction_started(out);
                }
                let metadata = value.get("compact_metadata").unwrap_or(&Value::Null);
                let automatic = self
                    .compacting
                    .take()
                    .unwrap_or_else(|| str_of(metadata, "trigger") == Some("auto"));
                let tokens_after = metadata.get("post_tokens").and_then(Value::as_i64);
                self.compacted = true;
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
            // Its background jobs (commands, sub-agents) still running, all of them each time.
            // One that ends starts a turn of its own.
            Some("background_tasks_changed") => {
                let running = value
                    .get("tasks")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(at, task)| {
                        str_of(task, "task_id").map_or_else(|| format!("#{at}"), str::to_owned)
                    })
                    .collect();
                self.background.clone_from(&running);
                out.push(Output::Event(ProviderEvent::BackgroundJobs { running }));
            }
            // A job's end, before the word of those still running: said one at a time, so each
            // end's turn is matched to its own job even when several end at once.
            Some("task_notification") => {
                let ended = str_of(value, "task_id");
                if let Some(at) = self
                    .background
                    .iter()
                    .position(|job| Some(job.as_str()) == ended)
                {
                    self.background.remove(at);
                    out.push(Output::Event(ProviderEvent::BackgroundJobs {
                        running: self.background.clone(),
                    }));
                }
            }
            _ => {}
        }
    }

    /// A turn starts, unless one already runs.
    fn begin_turn(&mut self, out: &mut Vec<Output>) {
        if self.turn_active {
            return;
        }
        self.turn_active = true;
        self.turn_error = false;
        self.interrupting = false;
        self.turn_started_ms = Some(now_ms());
        out.push(Output::Event(ProviderEvent::TurnStarted { turn_id: None }));
    }

    /// Claude began compacting: in the running turn when it compacts on its own, else in a
    /// turn of its own (Claude echoes the `/compact` only once it is done).
    fn compaction_started(&mut self, out: &mut Vec<Output>) {
        let automatic = !std::mem::take(&mut self.compact_asked);
        self.begin_turn(out);
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
        let calls = i64::try_from(std::mem::take(&mut self.turn_calls).len()).unwrap_or(i64::MAX);
        let usage = value.get("usage").map(|usage| {
            let mut turn = token_usage(usage);
            turn.cost_usd = value.get("total_cost_usd").and_then(Value::as_f64);
            turn.model_calls = calls;
            for (id, sub) in self.subagent_usage.drain() {
                turn.input_tokens += sub.input_tokens;
                turn.cached_input_tokens += sub.cached_input_tokens;
                turn.cache_write_tokens += sub.cache_write_tokens;
                turn.output_tokens += sub.output_tokens;
                turn.reasoning_tokens += sub.reasoning_tokens;
                turn.cache_write_1h_tokens += sub.cache_write_1h_tokens;
                turn.cache_write_5m_tokens += sub.cache_write_5m_tokens;
                self.subagent_counted.insert(id);
            }
            turn
        });
        if let Some(turn) = &usage {
            self.totals.input_tokens += turn.input_tokens;
            self.totals.cached_input_tokens += turn.cached_input_tokens;
            self.totals.cache_write_tokens += turn.cache_write_tokens;
            self.totals.output_tokens += turn.output_tokens;
            self.totals.reasoning_tokens += turn.reasoning_tokens;
            self.totals.model_calls += turn.model_calls;
            self.totals.cache_write_1h_tokens += turn.cache_write_1h_tokens;
            self.totals.cache_write_5m_tokens += turn.cache_write_5m_tokens;
            // `total_cost_usd` is cumulative for the session.
            self.totals.cost_usd = turn.cost_usd.or(self.totals.cost_usd);
            out.push(Output::Event(ProviderEvent::Usage {
                total: self.totals.clone(),
                last: None,
                calls: std::mem::take(&mut self.turn_cache),
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
        // Brigadier keeps "Allow … for this session" itself, for the whole conversation: the
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

/// The lines a `Read` got: what its result holds (the whole file when it starts at line 1 and
/// holds all of them), else what it asked for (`offset`, `limit`; neither: the whole file).
fn read_lines(input: &Value, result: Option<&Value>) -> Option<LineRange> {
    let number = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64);
    if let Some(file) = result.and_then(|result| result.get("file"))
        && let (Some(start), Some(count)) = (number(file, "startLine"), number(file, "numLines"))
    {
        let total = number(file, "totalLines");
        if start <= 1 && total.is_some_and(|total| count >= total) {
            return None;
        }
        return Some(LineRange {
            start: start.max(1),
            end: Some(start.max(1) + count.max(1) - 1),
        });
    }
    let offset = number(input, "offset");
    let limit = number(input, "limit");
    if offset.is_none() && limit.is_none() {
        return None;
    }
    let start = offset.unwrap_or(1).max(1);
    Some(LineRange {
        start,
        end: limit.map(|limit| start + limit.max(1) - 1),
    })
}

fn file_changes(tool: &str, input: &Value) -> Vec<FileChange> {
    let path = str_of(input, "file_path").or_else(|| str_of(input, "notebook_path"));
    let kind = match tool {
        "Write" => FileChangeKind::Add,
        "Edit" | "MultiEdit" | "NotebookEdit" => FileChangeKind::Update,
        _ => return Vec::new(),
    };
    path.map(|path| {
        vec![FileChange::counted(
            path.to_owned(),
            kind,
            edit_diff(tool, input).as_deref(),
            crate::DIFF_RAW_CLIP,
        )]
    })
    .unwrap_or_default()
}

/// The result carries the patch actually applied, including its position in the file.
fn finish_change(change: &mut FileChange, result: Option<&Value>, input: &Value) {
    let Some(result) = result else { return };
    change.original = str_of(result, "originalFile").map(str::to_owned);
    change.kind = match str_of(result, "type") {
        Some("create") => FileChangeKind::Add,
        Some("update") => FileChangeKind::Update,
        _ => change.kind,
    };
    let mut diff = String::new();
    if let Some(hunks) = result.get("structuredPatch").and_then(Value::as_array) {
        for hunk in hunks {
            let number = |key| hunk.get(key).and_then(Value::as_u64).unwrap_or(0);
            diff.push_str(&format!(
                "@@ -{},{} +{},{} @@\n",
                number("oldStart"),
                number("oldLines"),
                number("newStart"),
                number("newLines")
            ));
            for line in hunk
                .get("lines")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                diff.push_str(line);
                diff.push('\n');
            }
        }
    }
    if diff.is_empty()
        && change.kind == FileChangeKind::Update
        && let Some(original) = &change.original
        && let Some(content) = str_of(result, "content").or_else(|| str_of(input, "content"))
    {
        // A replacement Write sometimes supplies the two whole versions without hunks.
        diff.push_str(&format!(
            "@@ -1,{} +1,{} @@\n",
            original.lines().count(),
            content.lines().count()
        ));
        for (text, sign) in [(original.as_str(), '-'), (content, '+')] {
            for line in text.lines() {
                diff.push(sign);
                diff.push_str(line);
                diff.push('\n');
            }
            if !text.is_empty() && !text.ends_with('\n') {
                diff.push_str("\\ No newline at end of file\n");
            }
        }
    }
    if !diff.is_empty() {
        let counted = FileChange::counted(
            change.path.clone(),
            change.kind,
            Some(&diff),
            crate::DIFF_RAW_CLIP,
        );
        change.diff = counted.diff;
        change.added = counted.added;
        change.removed = counted.removed;
    }
}

/// An edit's lines out and in, from its input: `Edit`'s old and new text, each of `MultiEdit`'s
/// edits, `Write`'s content.
fn edit_diff(tool: &str, input: &Value) -> Option<String> {
    let lines = |text: &str, sign: char, out: &mut String| {
        for line in text.strip_suffix('\n').unwrap_or(text).lines() {
            out.push(sign);
            out.push_str(line);
            out.push('\n');
        }
    };
    let pair = |edit: &Value, out: &mut String| {
        lines(str_of(edit, "old_string").unwrap_or_default(), '-', out);
        lines(str_of(edit, "new_string").unwrap_or_default(), '+', out);
    };
    let mut out = String::new();
    match tool {
        "Edit" => pair(input, &mut out),
        "MultiEdit" => {
            for (index, edit) in input
                .get("edits")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                if index > 0 {
                    out.push_str("@@\n");
                }
                pair(edit, &mut out);
            }
        }
        "Write" => lines(str_of(input, "content").unwrap_or_default(), '+', &mut out),
        _ => return None,
    }
    (!out.is_empty()).then_some(out)
}

/// What a tool result says of its whole output (P5 in docs/research/long-sessions/PROBES.md):
/// a result too big for the model is a preview naming the file Claude saved it to (a command's
/// also in `persistedOutputPath`), and a failing command's long output is only a head and tail
/// around a "[N characters truncated]" mark.
fn kept_output(text: &str, structured: Option<&Value>, failed: bool) -> Option<Box<KeptOutput>> {
    let path = structured
        .and_then(|result| str_of(result, "persistedOutputPath"))
        .map(str::to_owned)
        .or_else(|| saved_file(text));
    let excerpt = failed && text.contains(" characters truncated]");
    (path.is_some() || excerpt).then(|| {
        Box::new(KeptOutput {
            file: path.map(|path| KeptFile {
                path,
                ..KeptFile::default()
            }),
            excerpt,
            ..KeptOutput::default()
        })
    })
}

/// The file a preview says the whole output was saved to: "Full output saved to: <path>" (a
/// command's) or "Output has been saved to <path>." (an MCP tool's). The preview's text is a
/// command's own output too, so only a path shaped as the CLI writes them counts: a file right
/// in a session's `tool-results` folder (`…/projects/<project>/<session>/tool-results/<file>`),
/// with no `..` or `.` in it.
pub(crate) fn saved_file(text: &str) -> Option<String> {
    let head = &text[..text.floor_char_boundary(4 * 1024)];
    let (_, rest) = head
        .split_once("Full output saved to: ")
        .or_else(|| head.split_once("Output has been saved to "))?;
    let line = rest.lines().next()?.trim();
    let path = line.strip_suffix('.').unwrap_or(line);
    crate::model::is_tool_results_file(path).then(|| path.to_owned())
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
        model_calls: 0,
        cache_write_1h_tokens: usage
            .pointer("/cache_creation/ephemeral_1h_input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        cache_write_5m_tokens: usage
            .pointer("/cache_creation/ephemeral_5m_input_tokens")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
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
    fn file_and_network_approvals_keep_their_session_scopes() {
        let workspace =
            std::env::temp_dir().join(format!("claude-file-grant-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace).unwrap();
        let ask =
            |tool: &str, input: Value| {
                Parser::live().feed(&json!({
                "type": "control_request", "request_id": "a1",
                "request": { "subtype": "can_use_tool", "tool_name": tool, "input": input }
            }).to_string()).into_iter().find_map(|out| match out {
                Output::Event(ProviderEvent::ApprovalRequested { request }) => Some(request),
                _ => None,
            }).unwrap()
            };
        let edit =
            |path: &std::path::Path| ask("Write", json!({ "file_path": path, "content": "hello" }));
        let mut first = edit(&workspace.join("a.txt"));
        assert_eq!(first.kind, ApprovalKind::FileChange);
        first.grant = policy::workspace_grant(&first, &workspace);
        let mut similar = policy::Similar::default();
        similar.allow(&first);
        assert!(similar.covers(&edit(&workspace.join("new/b.txt")), Some(&workspace)));
        assert!(!similar.covers(
            &edit(&workspace.parent().unwrap().join("outside.txt")),
            Some(&workspace)
        ));
        let network = ask("SandboxNetworkAccess", json!({ "host": "example.com" }));
        assert_eq!(network.kind, ApprovalKind::Tool);
        similar.allow(&network);
        assert!(similar.covers(&network, Some(&workspace)));
        std::fs::remove_dir_all(workspace).unwrap();
    }

    /// The CLI's guess at the user's next message, trimmed; an empty one is no guess.
    #[test]
    fn a_prompt_suggestion_is_the_cli_s_guess_at_the_next_message() {
        let guesses = |suggestion: &str| {
            Parser::live()
                .feed(
                    &json!({"type": "prompt_suggestion", "suggestion": suggestion, "uuid": "u", "session_id": "s"})
                        .to_string(),
                )
                .into_iter()
                .filter_map(|out| match out {
                    Output::Event(ProviderEvent::PromptSuggestion { text }) => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            guesses("  Run the tests once more.\n"),
            ["Run the tests once more."]
        );
        assert!(guesses("   ").is_empty());
    }

    #[test]
    fn an_edit_carries_its_lines_out_and_in() {
        let edit = json!({"file_path": "/r/a.ts", "old_string": "a\nb\n", "new_string": "a\nc"});
        let [change] = file_changes("Edit", &edit).try_into().unwrap();
        assert_eq!(change.diff.as_deref(), Some("-a\n-b\n+a\n+c\n"));
        let multi = json!({"file_path": "f", "edits": [
            {"old_string": "x", "new_string": "y"},
            {"old_string": "", "new_string": "z"},
        ]});
        assert_eq!(
            edit_diff("MultiEdit", &multi).as_deref(),
            Some("-x\n+y\n@@\n+z\n")
        );
        assert_eq!(
            edit_diff("Write", &json!({"file_path": "n", "content": "hi"})).as_deref(),
            Some("+hi\n")
        );
        assert_eq!(edit_diff("Read", &json!({"file_path": "n"})), None);
        // Counted from the edit while it runs.
        assert_eq!((change.added, change.removed), (Some(2), Some(2)));
    }

    /// A finished edit's lines in and out are its result's patch, without the unchanged lines
    /// its old and new text repeat; a new file's (no patch) are all of its lines in.
    #[test]
    fn a_finished_edit_counts_the_lines_its_patch_changed() {
        let mut parser = Parser::live();
        let changes = |outputs: Vec<Output>| -> Vec<FileChange> {
            outputs
                .into_iter()
                .find_map(|output| match output {
                    Output::Event(ProviderEvent::FileChanges { changes, .. }) => Some(changes),
                    _ => None,
                })
                .expect("a file change")
        };
        let edit = json!({"file_path": "/w/a.rs", "old_string": "fn a() {\n    1\n}", "new_string": "fn a() {\n    2\n}"});
        let write = json!({"file_path": "/w/new.rs", "content": "one\ntwo\nthree\n"});
        let started = parser.feed(
            &json!({"type": "assistant", "message": {"id": "m1", "content": [
                {"type": "tool_use", "id": "toolu_e", "name": "Edit", "input": edit},
                {"type": "tool_use", "id": "toolu_w", "name": "Write", "input": write},
            ]}})
            .to_string(),
        );
        let running: Vec<(Option<u32>, Option<u32>)> = started
            .into_iter()
            .filter_map(|output| match output {
                Output::Event(ProviderEvent::FileChanges { changes, .. }) => {
                    Some((changes[0].added, changes[0].removed))
                }
                _ => None,
            })
            .collect();
        assert_eq!(running, [(Some(3), Some(3)), (Some(3), Some(0))]);
        let edited = changes(parser.feed(
            &json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_e", "content": "The file was updated"},
            ]}, "tool_use_result": {"filePath": "/w/a.rs", "structuredPatch": [
                {"oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
                 "lines": [" fn a() {", "-    1", "+    2", " }"]},
            ]}})
            .to_string(),
        ));
        assert_eq!((edited[0].added, edited[0].removed), (Some(1), Some(1)));
        assert_eq!(
            edited[0].diff.as_deref(),
            Some("@@ -1,3 +1,3 @@\n fn a() {\n-    1\n+    2\n }\n")
        );
        let written = changes(parser.feed(
            &json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "toolu_w", "content": "File created"},
            ]}, "tool_use_result": {"type": "create", "filePath": "/w/new.rs", "structuredPatch": []}})
            .to_string(),
        ));
        assert_eq!((written[0].added, written[0].removed), (Some(3), Some(0)));
    }

    #[test]
    fn request_changes_claude_applied_patches_keep_real_lines_and_replaced_contents() {
        for (tool, input, result, expected) in [
            (
                "Write",
                json!({"file_path": "/w/a", "content": "new\n"}),
                json!({"type": "update", "originalFile": "old\n", "content": "new\n", "structuredPatch": []}),
                "@@ -1,1 +1,1 @@\n-old\n+new\n",
            ),
            (
                "Edit",
                json!({"file_path": "/w/a", "old_string": "old", "new_string": "new"}),
                json!({"type": "update", "originalFile": "one\ntwo\nold\n", "structuredPatch": [
                 {"oldStart": 3, "oldLines": 1, "newStart": 3, "newLines": 1, "lines": ["-old", "+new"]}]}),
                "@@ -3,1 +3,1 @@\n-old\n+new\n",
            ),
        ] {
            let mut parser = Parser::live();
            parser.feed(
                &json!({"type": "assistant", "message": {"id": "msg_1", "content": [
                    {"type": "tool_use", "id": "tool_1", "name": tool, "input": input}
                ]}})
                .to_string(),
            );
            let events = parser.feed(
                &json!({"type": "user", "message": {"content": [
                {"type": "tool_result", "tool_use_id": "tool_1", "content": "Updated"}
            ]}, "tool_use_result": result})
                .to_string(),
            );
            let change = events
                .into_iter()
                .find_map(|output| match output {
                    Output::Event(ProviderEvent::FileChanges { mut changes, .. }) => changes.pop(),
                    _ => None,
                })
                .unwrap();
            assert_eq!(change.kind, FileChangeKind::Update);
            assert_eq!(change.diff.as_deref(), Some(expected));
            assert_eq!((change.added, change.removed), (Some(1), Some(1)));
            assert!(change.original.is_some());
        }
    }

    /// Claude's command output arrives at completion; Codex streams (C5). The lines a real
    /// foreground `Bash` of 40 s printed (claude 2.1.300, 2026-10-15): it started, a heartbeat
    /// came at 30 s with the time elapsed and no output, it ended, and the result held all ten
    /// lines. No output comes on the way, so the command's one output is its result's.
    #[test]
    fn a_claude_commands_output_arrives_at_completion() {
        let lines = [
            json!({"type": "assistant", "message": {"id": "msg_1", "content": [
                {"type": "tool_use", "id": "toolu_01Vk", "name": "Bash", "input": {
                    "command": "for i in 1 2 3 4 5 6 7 8 9 10; do echo line$i; sleep 4; done",
                    "description": "Print ten lines with pauses", "run_in_background": false}},
            ]}}),
            json!({"type": "system", "subtype": "task_started", "task_id": "btbhmgdpp",
                "tool_use_id": "toolu_01Vk", "description": "Print ten lines with pauses",
                "is_backgrounded": false, "task_type": "local_bash"}),
            json!({"type": "tool_progress", "tool_use_id": "toolu_01Vk-heartbeat-0",
                "tool_name": "Bash", "parent_tool_use_id": "toolu_01Vk",
                "elapsed_time_seconds": 30, "heartbeat": true}),
            json!({"type": "system", "subtype": "task_notification", "task_id": "btbhmgdpp",
                "tool_use_id": "toolu_01Vk", "status": "completed", "output_file": "",
                "summary": "Print ten lines with pauses"}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"tool_use_id": "toolu_01Vk", "type": "tool_result",
                 "content": "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10",
                 "is_error": false}]},
             "tool_use_result": {"stdout": "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10",
                 "stderr": "", "interrupted": false, "isImage": false}}),
        ];
        let mut parser = Parser::live();
        let events: Vec<ProviderEvent> = lines
            .iter()
            .flat_map(|line| parser.feed(&line.to_string()))
            .filter_map(|output| match output {
                Output::Event(event) => Some(event),
                Output::Control(_) => None,
            })
            .collect();
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, ProviderEvent::CommandOutputDelta { .. })),
            "{events:?}"
        );
        let commands: Vec<(ItemStatus, Option<&str>, Option<i32>)> = events
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::Command {
                    status,
                    output,
                    exit_code,
                    ..
                } => Some((*status, output.as_deref(), *exit_code)),
                _ => None,
            })
            .collect();
        let whole = (1..=10)
            .map(|n| format!("line{n}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            commands,
            [
                (ItemStatus::InProgress, None, None),
                (ItemStatus::Completed, Some(whole.as_str()), Some(0)),
            ]
        );
    }

    /// The results of P5 (docs/research/long-sessions/PROBES.md), claude 2.1.301: a 200 KiB
    /// `Bash` output and a 200 KiB MCP result reach the model as a preview naming the file the
    /// whole output was saved to; a failing command's long output as a head and tail.
    #[test]
    fn a_preview_names_the_saved_file_and_a_failure_excerpt_is_marked() {
        let dir = "/Users/u/Library/Application Support/x/projects/-tmp-p/d8d2/tool-results";
        let bash = format!(
            "<persisted-output>\nOutput too large (200KB). Full output saved to: {dir}/bs15nclkg.txt\n\n\
             Preview (first 2KB):\nline 000000 filler\n...\n</persisted-output>"
        );
        let mcp = format!(
            "Error: result (204,802 characters across 3,414 lines) exceeds maximum allowed tokens. \
             Output has been saved to {dir}/mcp-probe-big_dump-1792107958316.txt.\nFormat: Plain text\n"
        );
        let failing = format!(
            "Exit code 1\n{}... [182345 characters truncated] ...{}",
            "x".repeat(5_000),
            "y".repeat(5_000)
        );
        let lines = [
            json!({"type": "assistant", "message": {"id": "m1", "content": [
                {"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "gen"}},
                {"type": "tool_use", "id": "t2", "name": "mcp__probe__big_dump", "input": {}},
                {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "make test"}},
            ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"tool_use_id": "t1", "type": "tool_result", "content": bash, "is_error": false}]},
             "tool_use_result": {"stdout": "line 000000 filler", "stderr": "",
                 "persistedOutputPath": format!("{dir}/bs15nclkg.txt"), "persistedOutputSize": 204802}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"tool_use_id": "t2", "type": "tool_result", "content": mcp, "is_error": false}]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"tool_use_id": "t3", "type": "tool_result", "content": failing, "is_error": true}]}}),
        ];
        let mut parser = Parser::live();
        let kept: Vec<(String, Option<Box<KeptOutput>>, usize)> = lines
            .iter()
            .flat_map(|line| parser.feed(&line.to_string()))
            .filter_map(|output| match output {
                Output::Event(
                    ProviderEvent::Command {
                        item_id,
                        status: ItemStatus::Completed | ItemStatus::Failed,
                        output,
                        kept,
                        ..
                    }
                    | ProviderEvent::ToolCall {
                        item_id,
                        status: ItemStatus::Completed | ItemStatus::Failed,
                        output,
                        kept,
                        ..
                    },
                ) => Some((item_id, kept, output.map_or(0, |output| output.len()))),
                _ => None,
            })
            .collect();
        let file = |name: &str| {
            Some(Box::new(KeptOutput {
                file: Some(KeptFile {
                    path: format!("{dir}/{name}"),
                    ..KeptFile::default()
                }),
                ..KeptOutput::default()
            }))
        };
        assert_eq!(kept[0].1, file("bs15nclkg.txt"));
        assert_eq!(kept[1].1, file("mcp-probe-big_dump-1792107958316.txt"));
        assert_eq!(
            kept[2].1,
            Some(Box::new(KeptOutput {
                excerpt: true,
                ..KeptOutput::default()
            }))
        );
        // The parser no longer cuts: the event sender does, once it is redacted and kept.
        assert!(kept[2].2 > 10_000, "{}", kept[2].2);
    }

    /// P5 (docs/research/long-sessions/PROBES.md), claude 2.1.301: a compaction Claude starts on
    /// its own, refused by the session's `PreCompact` hook, shows only a `compacting` status and
    /// an empty one. A session that refuses compaction reports it as refused, never as started;
    /// one that compacts after all is reported as it happened.
    #[test]
    fn a_refused_compaction_is_reported_as_refused_not_started() {
        let lines = [
            json!({"type": "system", "subtype": "status", "status": "compacting",
                   "compact_output_chars": 0, "session_id": "s"}),
            json!({"type": "system", "subtype": "status", "status": null, "session_id": "s"}),
        ];
        let events = |refuses: bool, lines: &[Value]| {
            let mut parser = Parser::live();
            parser.refuses_compaction = refuses;
            lines
                .iter()
                .flat_map(|line| parser.feed(&line.to_string()))
                .filter_map(|output| match output {
                    Output::Event(
                        event @ (ProviderEvent::CompactionStarted { .. }
                        | ProviderEvent::CompactionEnded { .. }
                        | ProviderEvent::CompactionRefused),
                    ) => Some(event),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(events(true, &lines), [ProviderEvent::CompactionRefused]);
        // A session that may compact sees it start, as before.
        assert_eq!(
            events(false, &lines[..1]),
            [ProviderEvent::CompactionStarted { automatic: true }]
        );
        let compacted = [
            lines[0].clone(),
            json!({"type": "system", "subtype": "compact_boundary", "session_id": "s",
                   "compact_metadata": {"trigger": "auto", "pre_tokens": 90000, "post_tokens": 9000}}),
        ];
        let seen = events(true, &compacted);
        assert!(
            matches!(
                &seen[..],
                [
                    ProviderEvent::CompactionStarted { automatic: true },
                    ProviderEvent::CompactionEnded { error: None, .. }
                ]
            ),
            "{seen:?}"
        );
    }

    #[test]
    fn a_saved_file_is_only_a_tool_results_path() {
        assert_eq!(saved_file("Full output saved to: /etc/passwd\n"), None);
        assert_eq!(
            saved_file("Output has been saved to relative/tool-results/a.txt."),
            None
        );
        assert_eq!(
            saved_file("Output has been saved to /a b/tool-results/c.txt.\nmore"),
            None
        );
        // A command's own output can forge the words: a path out of the folder is no saved file.
        assert_eq!(
            saved_file(
                "Full output saved to: /h/.claude/projects/p/s/tool-results/../../../../../etc/passwd\n"
            ),
            None
        );
        assert_eq!(
            saved_file(
                "Output has been saved to /h/x b/projects/-tmp-p/s1/tool-results/c.txt.\nmore"
            ),
            Some("/h/x b/projects/-tmp-p/s1/tool-results/c.txt".into())
        );
        // Long output whose 4 KiB mark falls inside a character is read, not a panic.
        let wide = "界".repeat(2_000);
        assert_eq!(saved_file(&wide), None);
    }

    /// The `Looked` events a recording's output makes.
    pub(crate) fn looked_in(recording: &str) -> Vec<ProviderEvent> {
        let recording = crate::record::Recording::parse(recording).unwrap();
        let mut parser = Parser::replay();
        recording
            .lines
            .iter()
            .filter(|line| line.dir == crate::record::Direction::Out)
            .flat_map(|line| parser.feed(&line.line))
            .filter_map(|output| match output {
                Output::Event(event @ ProviderEvent::Looked { .. }) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn read(path: &str, lines: Option<(u64, Option<u64>)>) -> FileRead {
        FileRead {
            path: path.into(),
            lines: lines.map(|(start, end)| LineRange { start, end }),
        }
    }

    #[test]
    fn a_thread_s_reads_and_searches_are_told_from_its_tool_results() {
        let looked = looked_in(include_str!("../../fixtures/claude-thread-reads.jsonl"));
        let repo = "/tmp/brg-reads-probe/repo";
        let file = |name: &str| format!("{repo}/{name}");
        let mut reads = Vec::new();
        let mut searches = Vec::new();
        for event in looked {
            let ProviderEvent::Looked {
                cwd,
                reads: r,
                searches: s,
                ..
            } = event
            else {
                unreachable!()
            };
            assert_eq!(cwd.as_deref(), Some("/private/tmp/brg-reads-probe/cwd"));
            reads.extend(r);
            searches.extend(s);
        }
        assert_eq!(
            reads,
            [
                // What the result holds: offset 35, limit 10.
                read(&file("src.rs"), Some((35, Some(44)))),
                // From line 110 to the file's end (121 lines).
                read(&file("src.rs"), Some((110, Some(121)))),
                read(&file("notes.md"), None),
                read("/tmp/brg-reads-probe/outside.txt", None),
            ]
        );
        let search = |kind, pattern: &str, glob: Option<&str>, hits: &[&str]| FileSearch {
            kind,
            pattern: Some(pattern.into()),
            scope: Some(repo.into()),
            glob: glob.map(Into::into),
            hits: hits.iter().map(|name| file(name)).collect(),
        };
        assert_eq!(
            searches,
            [
                search(
                    SearchKind::Content,
                    "compute_answer",
                    None,
                    &["other.rs", "src.rs"]
                ),
                // Content mode names its files in its lines.
                search(
                    SearchKind::Content,
                    "compute_answer",
                    None,
                    &["src.rs", "other.rs"]
                ),
                search(SearchKind::Content, "Notes", Some("*.md"), &["notes.md"]),
                search(SearchKind::Files, "**/*.rs", None, &["src.rs", "other.rs"]),
            ]
        );
    }

    #[test]
    fn a_read_without_its_structured_result_keeps_what_it_asked_for() {
        let mut parser = Parser::live();
        let call = |id: &str, input: Value| {
            json!({"type":"assistant", "message":{"id":"m", "content":[
                {"type":"tool_use", "id": id, "name":"Read", "input": input}]}})
        };
        let result = |id: &str, failed: bool| {
            json!({"type":"user", "message":{"content":[
                {"type":"tool_result", "tool_use_id": id, "content":"1\tx", "is_error": failed}]}})
        };
        let mut looked = Vec::new();
        for (id, input, failed) in [
            (
                "a",
                json!({"file_path":"/r/a.rs", "offset": 5, "limit": 3}),
                false,
            ),
            ("b", json!({"file_path":"/r/b.rs", "offset": 9}), false),
            ("c", json!({"file_path":"/r/c.rs"}), false),
            ("d", json!({"file_path":"/r/missing.rs"}), true),
        ] {
            parser.feed(&call(id, input).to_string());
            looked.extend(
                parser
                    .feed(&result(id, failed).to_string())
                    .into_iter()
                    .filter_map(|output| match output {
                        Output::Event(ProviderEvent::Looked { reads, .. }) => Some(reads),
                        _ => None,
                    })
                    .flatten(),
            );
        }
        assert_eq!(
            looked,
            [
                read("/r/a.rs", Some((5, Some(7)))),
                read("/r/b.rs", Some((9, None))),
                read("/r/c.rs", None),
            ]
        );
    }

    #[test]
    fn a_tool_is_visible_before_its_arguments_finish_and_updates_once() {
        let mut parser = Parser::live();
        // In a turn, as every tool call is.
        let echo = json!({"type":"user", "isReplay":true, "message":{"role":"user", "content":"Delegate it."}});
        parser.feed(&echo.to_string());
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

    /// What each line said of the background jobs still running.
    fn jobs_of(outputs: Vec<Output>) -> Vec<Vec<String>> {
        outputs
            .into_iter()
            .filter_map(|output| match output {
                Output::Event(ProviderEvent::BackgroundJobs { running }) => Some(running),
                _ => None,
            })
            .collect()
    }

    /// Claude's word of the background jobs still running.
    fn jobs(parser: &mut Parser, tasks: Value) -> Vec<Vec<String>> {
        let line =
            json!({ "type": "system", "subtype": "background_tasks_changed", "tasks": tasks });
        jobs_of(parser.feed(&line.to_string()))
    }

    /// Two jobs that end together are said to end one at a time, in the order of Claude's
    /// notifications, which is the order of the turns their ends start: each turn is matched
    /// to its own job, not to the order they started in (#221).
    #[test]
    fn jobs_that_end_together_end_in_the_order_claude_notifies_them() {
        let mut parser = Parser::live();
        let tasks = json!([{ "task_id": "a1", "task_type": "local_bash" },
                           { "task_id": "b2", "task_type": "local_bash" }]);
        assert_eq!(
            jobs(&mut parser, tasks),
            [vec!["a1".to_owned(), "b2".to_owned()]]
        );
        let notified = |parser: &mut Parser, id: &str| {
            let line = json!({ "type": "system", "subtype": "task_notification",
                               "task_id": id, "status": "completed" });
            jobs_of(parser.feed(&line.to_string()))
        };
        assert_eq!(notified(&mut parser, "b2"), [vec!["a1".to_owned()]]);
        assert_eq!(notified(&mut parser, "a1"), [Vec::<String>::new()]);
        assert_eq!(jobs(&mut parser, json!([])), [Vec::<String>::new()]);
        // A foreground command's notification ends no job.
        assert!(notified(&mut parser, "fg9").is_empty());
    }

    /// A turn can end while Claude's own background job still runs: Claude says how many run,
    /// and the job's end starts a turn of its own, with no message of ours echoed. Shapes as
    /// Claude Code 2.1.296 streams them (`run_in_background` Bash).
    #[test]
    fn a_background_jobs_end_starts_a_turn_of_its_own() {
        let mut parser = Parser::live();
        let task = json!({ "task_id": "b3dwmq6fl", "task_type": "local_bash",
                           "description": "Sleep 8 seconds then echo finished" });
        assert_eq!(
            jobs(&mut parser, json!([task])),
            [vec!["b3dwmq6fl".to_owned()]]
        );
        let result = json!({ "type": "result", "subtype": "success", "is_error": false, "result": "waiting" });
        parser.feed(&result.to_string());
        assert!(!parser.turn_active());
        let notification = json!({ "type": "system", "subtype": "task_notification",
                                   "task_id": "b3dwmq6fl", "status": "completed" });
        assert_eq!(
            jobs_of(parser.feed(&notification.to_string())),
            [Vec::<String>::new()]
        );
        assert_eq!(jobs(&mut parser, json!([])), [Vec::<String>::new()]);
        assert!(!parser.turn_active());
        let reply = json!({ "type": "assistant", "parent_tool_use_id": null,
                            "message": { "id": "m9", "content": [{ "type": "text", "text": "seen" }] } });
        let outputs = parser.feed(&reply.to_string());
        assert!(matches!(
            outputs.as_slice(),
            [
                Output::Event(ProviderEvent::TurnStarted { .. }),
                Output::Event(ProviderEvent::Message { text, .. }),
            ] if text == "seen"
        ));
        assert!(parser.turn_active());
        // Partial messages start it at the first one.
        parser.feed(&result.to_string());
        let start = json!({ "type": "stream_event", "parent_tool_use_id": null,
                            "event": { "type": "message_start", "message": { "id": "m10" } } });
        let outputs = parser.feed(&start.to_string());
        assert!(matches!(
            outputs.as_slice(),
            [Output::Event(ProviderEvent::TurnStarted { .. })]
        ));
    }

    /// Claude's `result` leaves its subagents (the Agent tool) out of `usage`: a turn's usage
    /// adds each subagent message's last usage once, and one that arrives after a result (a
    /// subagent run in the background) goes to the next turn. Shapes as Claude Code 2.1.296
    /// streams them.
    #[test]
    fn a_turns_usage_counts_its_subagents_messages_once() {
        let mut parser = Parser::live();
        let usage = |input: i64, read: i64, write: i64, output: i64| {
            json!({ "input_tokens": input, "cache_read_input_tokens": read,
                    "cache_creation_input_tokens": write, "output_tokens": output })
        };
        let sub = |id: &str, usage: Value| {
            json!({ "type": "assistant", "parent_tool_use_id": "toolu_1",
                    "message": { "id": id, "content": [{ "type": "text", "text": "…" }],
                                 "usage": usage } })
            .to_string()
        };
        let turn_usage = |outputs: Vec<Output>| {
            outputs
                .into_iter()
                .find_map(|output| match output {
                    Output::Event(ProviderEvent::TurnCompleted { usage, .. }) => usage,
                    _ => None,
                })
                .expect("a turn's usage")
        };
        parser.feed(&sub("m1", usage(2, 0, 13_171, 16)));
        // The same message again, as each of its blocks streams it.
        parser.feed(&sub("m1", usage(2, 0, 13_171, 16)));
        parser.feed(&sub("m2", usage(2, 13_171, 2_455, 2)));
        // The main agent's own call, its blocks streamed as two lines (BUILD-PLAN A0: calls).
        let main = json!({ "type": "assistant", "parent_tool_use_id": null,
                           "message": { "id": "a1", "content": [{ "type": "text", "text": "Done." }] } })
        .to_string();
        parser.feed(&main);
        parser.feed(&main);
        let result = |main: Value| {
            json!({ "type": "result", "subtype": "success", "is_error": false, "usage": main })
                .to_string()
        };
        let first = turn_usage(parser.feed(&result(usage(4, 36_377, 14_851, 318))));
        assert_eq!(
            (
                first.input_tokens,
                first.cached_input_tokens,
                first.cache_write_tokens,
                first.output_tokens
            ),
            (4 + 4, 36_377 + 13_171, 14_851 + 13_171 + 2_455, 318 + 18)
        );
        assert_eq!(first.model_calls, 3);
        // A line of a counted message after its result adds nothing; a new one counts next.
        parser.feed(&sub("m2", usage(2, 13_171, 2_455, 2)));
        parser.feed(&sub("m3", usage(2, 15_626, 160, 4)));
        let second = turn_usage(parser.feed(&result(usage(4, 25_905, 639, 4))));
        assert_eq!(
            (second.cached_input_tokens, second.output_tokens),
            (25_905 + 15_626, 4 + 4)
        );
        assert_eq!(second.model_calls, 1);
    }

    /// Each model call's prompt is kept once, before the turn sums them, against the call
    /// before it in its own chain, with the lifetime of what it wrote (research P0.1). Shapes
    /// as Claude Code streams them: `message_start` carries the prompt's usage, and each of the
    /// message's lines repeats it.
    #[test]
    fn each_calls_prompt_is_kept_against_the_call_before_it() {
        let mut parser = Parser::live();
        let usage = |input: i64, read: i64, write: i64, hour: i64| {
            json!({ "input_tokens": input, "cache_read_input_tokens": read,
                    "cache_creation_input_tokens": write, "output_tokens": 50,
                    "cache_creation": { "ephemeral_1h_input_tokens": hour,
                                        "ephemeral_5m_input_tokens": write - hour } })
        };
        let start = |id: &str, parent: Value, usage: Value| {
            json!({ "type": "stream_event", "parent_tool_use_id": parent,
                    "event": { "type": "message_start",
                               "message": { "id": id, "usage": usage } } })
            .to_string()
        };
        let line = |id: &str, usage: Value| {
            json!({ "type": "assistant", "parent_tool_use_id": null,
                    "message": { "id": id, "content": [{ "type": "text", "text": "…" }],
                                 "usage": usage } })
            .to_string()
        };
        let report = |outputs: Vec<Output>| {
            outputs
                .into_iter()
                .find_map(|output| match output {
                    Output::Event(ProviderEvent::Usage { total, calls, .. }) => {
                        Some((total, calls))
                    }
                    _ => None,
                })
                .expect("a usage report")
        };
        let result = |main: Value| {
            json!({ "type": "result", "subtype": "success", "is_error": false, "usage": main })
                .to_string()
        };
        // A first call writes the whole prompt for an hour; the second reads it back and
        // writes its new tail; a subagent's call is its own chain.
        parser.feed(&start("a1", Value::Null, usage(3, 0, 20_000, 20_000)));
        parser.feed(&line("a1", usage(3, 0, 20_000, 20_000)));
        parser.feed(&start("s1", json!("toolu_1"), usage(2, 0, 5_000, 0)));
        parser.feed(&start("a2", Value::Null, usage(2, 20_003, 1_500, 1_500)));
        parser.feed(&line("a2", usage(2, 20_003, 1_500, 1_500)));
        let (total, calls) = report(parser.feed(&result(usage(5, 20_003, 21_500, 21_500))));
        assert_eq!(
            (total.cache_write_1h_tokens, total.cache_write_5m_tokens),
            (21_500, 0)
        );
        let main = |prompt: i64, read: i64, write: i64, previous: Option<i64>| CallCache {
            prompt,
            cache_read: read,
            cache_write: write,
            previous_prompt: previous,
            after_compaction: false,
            subagent: false,
        };
        assert_eq!(
            calls,
            [
                main(20_003, 0, 20_000, None),
                CallCache {
                    subagent: true,
                    ..main(5_002, 0, 5_000, None)
                },
                main(21_505, 20_003, 1_500, Some(20_003)),
            ]
        );
        // The next turn's call missed part of the prefix: kept as reported, against the last.
        parser.feed(&start("a3", Value::Null, usage(2, 4_000, 18_000, 18_000)));
        let (_, calls) = report(parser.feed(&result(usage(2, 4_000, 18_000, 18_000))));
        assert_eq!(calls, [main(22_002, 4_000, 18_000, Some(21_505))]);
        // After a compaction the next call is reported apart.
        parser.feed(
            &json!({ "type": "system", "subtype": "compact_boundary",
                     "compact_metadata": { "trigger": "auto", "pre_tokens": 22_002,
                                           "post_tokens": 3_000 } })
            .to_string(),
        );
        parser.feed(&start("a4", Value::Null, usage(3_000, 0, 0, 0)));
        let (_, calls) = report(parser.feed(&result(usage(3_000, 0, 0, 0))));
        assert_eq!(
            calls,
            [CallCache {
                after_compaction: true,
                ..main(3_000, 0, 0, Some(22_002))
            }]
        );
    }
}
