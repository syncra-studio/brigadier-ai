//! Codex app-server messages → normalized events.
//!
//! Every stdout line is a JSON-RPC message: a response to one of our requests, a notification,
//! or a request from the server (approvals). Notifications and approval requests are decoded
//! with the generated [`protocol`] types; a message the bindings cannot read is reported as a
//! notice rather than dropped silently.

use std::collections::HashMap;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::protocol::{self as p, ThreadItem};
use crate::model::*;
use crate::{clip, now_ms};

const OUTPUT_CLIP: usize = 8 * 1024;

#[derive(Debug)]
pub enum Control {
    /// The answer to one of our requests.
    Response {
        id: i64,
        result: Result<Value, String>,
    },
    /// An approval the session must answer, keyed by the approval id in the event.
    Approval {
        approval_id: String,
        rpc_id: Value,
        kind: PendingKind,
    },
    /// An MCP server asks the user something (`mcpServer/elicitation/request`).
    Elicitation {
        rpc_id: Value,
        server: String,
        /// Codex asks whether a tool call may run (`_meta.codex_approval_kind` is
        /// `mcp_tool_call`), rather than for information.
        tool_approval: bool,
        message: String,
    },
    /// A server request Brigadier does not serve.
    Unsupported { rpc_id: Value, method: String },
}

/// What an approval answer has to look like.
#[derive(Debug, Clone)]
pub enum PendingKind {
    /// `grant`: Codex can allow the same command again for the rest of its session.
    Command {
        grant: bool,
    },
    FileChange,
    /// The requested permission profile, echoed back when granted.
    Permissions(Value),
}

#[derive(Debug)]
pub enum Output {
    Event(ProviderEvent),
    Control(Control),
}

#[derive(Default)]
pub struct Parser {
    /// A session reported its own start; `thread/started` is then not a start.
    started: bool,
    turn_id: Option<String>,
    /// An error was already reported for the running turn.
    turn_error: bool,
    /// Files of file-change items, which their approval requests do not repeat.
    file_items: HashMap<String, Vec<FileChange>>,
    quota: Option<QuotaSnapshot>,
    /// The context size last reported.
    context_used: Option<i64>,
    /// `thread/compact/start` was sent and Codex has not started compacting yet.
    compact_asked: bool,
    /// The compaction running now: whether Codex started it on its own, the context before
    /// it, and the size reported since it started.
    compacting: Option<(bool, Option<i64>, Option<i64>)>,
}

impl Parser {
    pub fn live() -> Self {
        Self {
            started: true,
            ..Self::default()
        }
    }

    pub fn replay() -> Self {
        Self::default()
    }

    /// The turn Codex is running, if any.
    pub fn turn_id(&self) -> Option<&str> {
        self.turn_id.as_deref()
    }

    /// `thread/compact/start` is about to be sent: the compaction it starts was asked for.
    pub fn compact_requested(&mut self) {
        self.compact_asked = true;
    }

    /// The latest rate limits, to classify limit errors.
    pub fn quota(&self) -> Option<&QuotaSnapshot> {
        self.quota.as_ref()
    }

    pub fn feed(&mut self, line: &str) -> Vec<Output> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            return vec![notice(
                NoticeLevel::Warning,
                format!("unparsed Codex output: {}", clip(line, 300)),
            )];
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let mut out = Vec::new();
        match (method, id) {
            (Some(method), Some(rpc_id)) => self.server_request(method, rpc_id, params, &mut out),
            (Some(method), None) => self.notification(method, params, &mut out),
            (None, Some(id)) => {
                let result = match message.get("error") {
                    Some(error) => Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("request failed")
                        .to_owned()),
                    None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                };
                if let Some(id) = id.as_i64() {
                    out.push(Output::Control(Control::Response { id, result }));
                }
            }
            (None, None) => {}
        }
        out
    }

    fn notification(&mut self, method: &str, params: Value, out: &mut Vec<Output>) {
        match method {
            "thread/started" => {
                let Some(started) = decode::<p::ThreadStartedNotification>(method, params, out)
                else {
                    return;
                };
                if !self.started {
                    self.started = true;
                    let thread = started.thread;
                    out.push(Output::Event(ProviderEvent::SessionStarted {
                        native_id: thread.id,
                        model: thread.model,
                        cwd: Some(thread.cwd.to_string()),
                        cli_version: Some(thread.cli_version),
                    }));
                }
            }
            "turn/started" => {
                let Some(started) = decode::<p::TurnStartedNotification>(method, params, out)
                else {
                    return;
                };
                self.turn_id = Some(started.turn.id.clone());
                self.turn_error = false;
                out.push(Output::Event(ProviderEvent::TurnStarted {
                    turn_id: Some(started.turn.id),
                }));
            }
            "turn/completed" => {
                let Some(completed) = decode::<p::TurnCompletedNotification>(method, params, out)
                else {
                    return;
                };
                let turn = completed.turn;
                self.turn_id = None;
                let status = match turn.status {
                    p::TurnStatus::Completed | p::TurnStatus::InProgress => TurnStatus::Completed,
                    p::TurnStatus::Interrupted => TurnStatus::Interrupted,
                    p::TurnStatus::Failed => TurnStatus::Failed,
                };
                if let Some(error) = &turn.error
                    && status == TurnStatus::Failed
                    && !std::mem::take(&mut self.turn_error)
                {
                    out.push(Output::Event(ProviderEvent::Error {
                        error: self.classify(error, false),
                    }));
                }
                out.push(Output::Event(ProviderEvent::TurnCompleted {
                    turn_id: Some(turn.id),
                    status,
                    duration_ms: turn.duration_ms,
                    usage: None,
                }));
            }
            "item/started" => {
                if let Some(started) = decode::<p::ItemStartedNotification>(method, params, out) {
                    self.item(started.item, false, out);
                }
            }
            "item/completed" => {
                if let Some(completed) = decode::<p::ItemCompletedNotification>(method, params, out)
                {
                    self.item(completed.item, true, out);
                }
            }
            "item/agentMessage/delta" => {
                if let Some(delta) = decode::<p::AgentMessageDeltaNotification>(method, params, out)
                {
                    out.push(Output::Event(ProviderEvent::MessageDelta {
                        item_id: delta.item_id,
                        text: delta.delta,
                    }));
                }
            }
            "item/reasoning/summaryTextDelta" => {
                if let Some(delta) =
                    decode::<p::ReasoningSummaryTextDeltaNotification>(method, params, out)
                {
                    out.push(Output::Event(ProviderEvent::ReasoningDelta {
                        item_id: delta.item_id,
                        text: delta.delta,
                    }));
                }
            }
            "item/commandExecution/outputDelta" => {
                if let Some(delta) =
                    decode::<p::CommandExecutionOutputDeltaNotification>(method, params, out)
                {
                    out.push(Output::Event(ProviderEvent::CommandOutputDelta {
                        item_id: delta.item_id,
                        text: delta.delta,
                    }));
                }
            }
            "thread/tokenUsage/updated" => {
                let Some(update) =
                    decode::<p::ThreadTokenUsageUpdatedNotification>(method, params, out)
                else {
                    return;
                };
                let usage = update.token_usage;
                out.push(Output::Event(ProviderEvent::Usage {
                    total: token_usage(&usage.total),
                    last: Some(token_usage(&usage.last)),
                }));
                // The last request is what the model saw: the context in use. (What a
                // compaction left counts only in its total.)
                let used = usage.last.total_tokens;
                self.context_used = Some(used);
                if let Some((_, _, after)) = &mut self.compacting {
                    *after = Some(used);
                }
                out.push(Output::Event(ProviderEvent::ContextSize {
                    used_tokens: used,
                    window_tokens: usage.model_context_window,
                }));
            }
            "account/rateLimits/updated" => {
                if let Some(update) =
                    decode::<p::AccountRateLimitsUpdatedNotification>(method, params, out)
                {
                    let quota = quota_event(&update.rate_limits);
                    match &mut self.quota {
                        Some(known) => known.merge(&quota),
                        None => self.quota = Some(quota.clone()),
                    }
                    out.push(Output::Event(ProviderEvent::RateLimits { quota }));
                }
            }
            "error" => {
                if let Some(error) = decode::<p::ErrorNotification>(method, params, out) {
                    self.turn_error |= !error.will_retry;
                    out.push(Output::Event(ProviderEvent::Error {
                        error: self.classify(&error.error, error.will_retry),
                    }));
                }
            }
            // Told by the `contextCompaction` item as well; this notification is deprecated.
            "thread/compacted" => {}
            "warning" | "configWarning" | "deprecationNotice" | "guardianWarning" => {
                let message = ["message", "summary", "details"]
                    .iter()
                    .find_map(|key| params.get(key).and_then(Value::as_str))
                    .unwrap_or(method);
                // The auto-reviewer's verdict on each request it settles (Approve for me) is
                // routine, not a warning.
                let level = if method == "guardianWarning" {
                    NoticeLevel::Info
                } else {
                    NoticeLevel::Warning
                };
                out.push(notice(level, format!("Codex: {message}")));
            }
            "model/rerouted" => out.push(notice(
                NoticeLevel::Warning,
                format!(
                    "Codex rerouted the model: {}",
                    clip(&params.to_string(), 300)
                ),
            )),
            // Status changes, MCP startup, diffs and plans the normalized model has no use
            // for (yet).
            _ => {}
        }
    }

    fn item(&mut self, item: ThreadItem, completed: bool, out: &mut Vec<Output>) {
        let event = match item {
            ThreadItem::UserMessage { id, content, .. } if completed => ProviderEvent::Message {
                item_id: id,
                role: Role::User,
                text: content
                    .iter()
                    .filter_map(|input| match input {
                        p::UserInput::TextUserInput { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            },
            ThreadItem::AgentMessage { id, text, .. } if completed => ProviderEvent::Message {
                item_id: id,
                role: Role::Assistant,
                text,
            },
            ThreadItem::Reasoning { id, summary, .. } if completed && !summary.is_empty() => {
                ProviderEvent::Reasoning {
                    item_id: id,
                    text: summary.join("\n\n"),
                }
            }
            ThreadItem::CommandExecution {
                id,
                command,
                cwd,
                status,
                exit_code,
                aggregated_output,
                duration_ms,
                ..
            } => ProviderEvent::Command {
                item_id: id,
                command,
                cwd: Some(cwd.to_string()),
                status: match status {
                    p::CommandExecutionStatus::InProgress => ItemStatus::InProgress,
                    p::CommandExecutionStatus::Completed => ItemStatus::Completed,
                    p::CommandExecutionStatus::Failed => ItemStatus::Failed,
                    p::CommandExecutionStatus::Declined => ItemStatus::Declined,
                },
                exit_code,
                output: aggregated_output.map(|output| clip(&output, OUTPUT_CLIP)),
                duration_ms,
            },
            ThreadItem::FileChange {
                id,
                changes,
                status,
            } => {
                let changes: Vec<FileChange> = changes
                    .into_iter()
                    .map(|change| FileChange {
                        path: change.path,
                        kind: match change.kind {
                            p::PatchChangeKind::Add => FileChangeKind::Add,
                            p::PatchChangeKind::Delete => FileChangeKind::Delete,
                            p::PatchChangeKind::Update { .. } => FileChangeKind::Update,
                        },
                    })
                    .collect();
                self.file_items.insert(id.clone(), changes.clone());
                ProviderEvent::FileChanges {
                    item_id: id,
                    changes,
                    status: match status {
                        p::PatchApplyStatus::InProgress => ItemStatus::InProgress,
                        p::PatchApplyStatus::Completed => ItemStatus::Completed,
                        p::PatchApplyStatus::Failed => ItemStatus::Failed,
                        p::PatchApplyStatus::Declined => ItemStatus::Declined,
                    },
                }
            }
            ThreadItem::McpToolCall {
                id,
                server,
                tool,
                arguments,
                status,
                result,
                error,
                ..
            } => ProviderEvent::ToolCall {
                item_id: id,
                name: format!("{server}/{tool}"),
                input: Some(clip(&arguments.to_string(), OUTPUT_CLIP)),
                status: match status {
                    p::McpToolCallStatus::InProgress => ItemStatus::InProgress,
                    p::McpToolCallStatus::Completed => ItemStatus::Completed,
                    p::McpToolCallStatus::Failed => ItemStatus::Failed,
                },
                output: error
                    .map(|error| error.message)
                    .or_else(|| {
                        result.and_then(|result| serde_json::to_string(&result.content).ok())
                    })
                    .map(|output| clip(&output, OUTPUT_CLIP)),
            },
            ThreadItem::DynamicToolCall {
                id,
                tool,
                arguments,
                status,
                ..
            } => ProviderEvent::ToolCall {
                item_id: id,
                name: tool,
                input: Some(clip(&arguments.to_string(), OUTPUT_CLIP)),
                status: match status {
                    p::DynamicToolCallStatus::InProgress => ItemStatus::InProgress,
                    p::DynamicToolCallStatus::Completed => ItemStatus::Completed,
                    p::DynamicToolCallStatus::Failed => ItemStatus::Failed,
                },
                output: None,
            },
            // A sub-agent call (spawning one, waiting for it…) is a tool call of the worker's;
            // what the sub-agent does meanwhile only shows the session is alive.
            ThreadItem::CollabAgentToolCall {
                id,
                tool,
                prompt,
                status,
                ..
            } => ProviderEvent::ToolCall {
                item_id: id,
                name: format!("agents/{tool}"),
                input: prompt.map(|prompt| clip(&prompt, OUTPUT_CLIP)),
                status: match status {
                    p::CollabAgentToolCallStatus::InProgress => ItemStatus::InProgress,
                    p::CollabAgentToolCallStatus::Completed => ItemStatus::Completed,
                    p::CollabAgentToolCallStatus::Failed
                    | p::CollabAgentToolCallStatus::Interrupted => ItemStatus::Failed,
                },
                output: None,
            },
            ThreadItem::SubAgentActivity {
                agent_thread_id, ..
            } => ProviderEvent::Progress {
                item_id: agent_thread_id,
            },
            ThreadItem::WebSearch { id, query, .. } => ProviderEvent::ToolCall {
                item_id: id,
                name: "web_search".into(),
                input: Some(query),
                status: item_status(completed),
                output: None,
            },
            ThreadItem::ImageView { id, path } => ProviderEvent::ToolCall {
                item_id: id,
                name: "view_image".into(),
                input: Some(path.to_string()),
                status: item_status(completed),
                output: None,
            },
            ThreadItem::ImageGeneration {
                id,
                status,
                saved_path,
                revised_prompt,
                failure,
                ..
            } => ProviderEvent::Image {
                item_id: id,
                status: if failure.is_some() || status == "failed" {
                    ItemStatus::Failed
                } else {
                    item_status(completed)
                },
                path: saved_path.map(|path| path.to_string()),
                prompt: revised_prompt,
            },
            ThreadItem::Plan { id, text } if completed => ProviderEvent::ToolCall {
                item_id: id,
                name: "plan".into(),
                input: None,
                status: ItemStatus::Completed,
                output: Some(text),
            },
            ThreadItem::ContextCompaction { .. } if !completed => {
                let automatic = !std::mem::take(&mut self.compact_asked);
                self.compacting = Some((automatic, self.context_used, None));
                ProviderEvent::CompactionStarted { automatic }
            }
            ThreadItem::ContextCompaction { .. } => {
                let (automatic, tokens_before, tokens_after) =
                    self.compacting.take().unwrap_or((true, None, None));
                ProviderEvent::CompactionEnded {
                    automatic,
                    tokens_before,
                    tokens_after,
                    error: None,
                }
            }
            _ => return,
        };
        out.push(Output::Event(event));
    }

    fn server_request(
        &mut self,
        method: &str,
        rpc_id: Value,
        params: Value,
        out: &mut Vec<Output>,
    ) {
        let approval_id = format!("codex-{}", rpc_id);
        if method == "mcpServer/elicitation/request" {
            let text = |key: &str| {
                params
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            out.push(Output::Control(Control::Elicitation {
                server: text("serverName"),
                tool_approval: params
                    .pointer("/_meta/codex_approval_kind")
                    .and_then(Value::as_str)
                    == Some("mcp_tool_call"),
                message: clip(&text("message"), 300),
                rpc_id,
            }));
            return;
        }
        let (request, kind) = match method {
            "item/commandExecution/requestApproval" => {
                let Some(ask) =
                    decode::<p::CommandExecutionRequestApprovalParams>(method, params, out)
                else {
                    return self.unsupported(rpc_id, method, out);
                };
                // An approved command runs outside the sandbox (see the adapter's docs).
                let reason = ask.reason.clone();
                // "Allow similar commands": Brigadier allows later commands with the same first
                // words itself; Codex's session cache (`acceptForSession`) holds this one.
                let grant = ask
                    .command
                    .as_deref()
                    .filter(|_| ask.kind == p::CommandExecutionApprovalKind::Command)
                    .and_then(crate::policy::command_prefix);
                (
                    ApprovalRequest {
                        id: approval_id.clone(),
                        kind: ApprovalKind::Command,
                        tool: "commandExecution".into(),
                        command: ask.command.clone(),
                        cwd: ask.cwd.map(|cwd| cwd.to_string()),
                        paths: Vec::new(),
                        reason: reason.or_else(|| {
                            ask.network_approval_context
                                .map(|network| format!("network access to {}", network.host))
                        }),
                        escalation: true,
                        input: ask.command,
                        grant: grant.clone(),
                    },
                    PendingKind::Command {
                        grant: grant.is_some(),
                    },
                )
            }
            "item/fileChange/requestApproval" => {
                let Some(ask) = decode::<p::FileChangeRequestApprovalParams>(method, params, out)
                else {
                    return self.unsupported(rpc_id, method, out);
                };
                let paths = self
                    .file_items
                    .get(&ask.item_id)
                    .map(|changes| changes.iter().map(|change| change.path.clone()).collect())
                    .unwrap_or_default();
                (
                    ApprovalRequest {
                        id: approval_id.clone(),
                        kind: ApprovalKind::FileChange,
                        tool: "fileChange".into(),
                        command: None,
                        cwd: None,
                        paths,
                        escalation: true,
                        reason: ask.reason.or_else(|| {
                            ask.grant_root.map(|root| format!("write access to {root}"))
                        }),
                        input: None,
                        grant: None,
                    },
                    PendingKind::FileChange,
                )
            }
            "item/permissions/requestApproval" => {
                let requested = params.get("permissions").cloned().unwrap_or(Value::Null);
                let Some(ask) = decode::<p::PermissionsRequestApprovalParams>(method, params, out)
                else {
                    return self.unsupported(rpc_id, method, out);
                };
                (
                    ApprovalRequest {
                        id: approval_id.clone(),
                        kind: ApprovalKind::Permissions,
                        tool: "permissions".into(),
                        command: None,
                        cwd: Some(ask.cwd.to_string()),
                        paths: Vec::new(),
                        reason: ask.reason,
                        escalation: true,
                        input: Some(clip(&requested.to_string(), OUTPUT_CLIP)),
                        grant: None,
                    },
                    PendingKind::Permissions(requested),
                )
            }
            _ => return self.unsupported(rpc_id, method, out),
        };
        out.push(Output::Event(ProviderEvent::ApprovalRequested { request }));
        out.push(Output::Control(Control::Approval {
            approval_id,
            rpc_id,
            kind,
        }));
    }

    fn unsupported(&self, rpc_id: Value, method: &str, out: &mut Vec<Output>) {
        out.push(Output::Control(Control::Unsupported {
            rpc_id,
            method: method.to_owned(),
        }));
    }

    fn classify(&self, error: &p::TurnError, will_retry: bool) -> ProviderError {
        use p::CodexErrorInfo as I;
        let code = error
            .codex_error_info
            .as_ref()
            .and_then(|info| serde_json::to_value(info).ok())
            .map(|value| match value {
                Value::String(code) => code,
                Value::Object(map) => map.keys().next().cloned().unwrap_or_default(),
                other => other.to_string(),
            });
        let kind = match &error.codex_error_info {
            Some(I::UsageLimitExceeded) => ErrorKind::UsageLimit,
            Some(I::RateLimitExceeded) => ErrorKind::RateLimit,
            // The flex service tier has no capacity right now: temporary, like an overload.
            Some(I::ServerOverloaded | I::FlexUnavailable) => ErrorKind::Overloaded,
            Some(I::ContextWindowExceeded) => ErrorKind::ContextWindow,
            Some(I::Unauthorized) => ErrorKind::Auth,
            Some(I::SessionBudgetExceeded) => ErrorKind::Billing,
            Some(I::BadRequest | I::ThreadRollbackFailed | I::ActiveTurnNotSteerable { .. }) => {
                ErrorKind::InvalidRequest
            }
            Some(I::CyberPolicy | I::MisalignmentPolicyViolation) => ErrorKind::Policy,
            Some(I::InternalServerError) => ErrorKind::Server,
            Some(I::SandboxError) => ErrorKind::Sandbox,
            Some(
                I::HttpConnectionFailed { .. }
                | I::ResponseStreamConnectionFailed { .. }
                | I::ResponseStreamDisconnected { .. }
                | I::ResponseTooManyFailedAttempts { .. },
            ) => ErrorKind::Network,
            _ if error.message.to_lowercase().contains("usage limit") => ErrorKind::UsageLimit,
            _ => ErrorKind::Other,
        };
        let limit = (kind == ErrorKind::UsageLimit).then(|| self.limit_hit());
        let message = match &error.additional_details {
            Some(details) if !details.is_empty() => format!("{} ({details})", error.message),
            _ => error.message.clone(),
        };
        ProviderError {
            kind,
            message: clip(&message, 2_000),
            will_retry,
            limit,
            code,
        }
    }

    /// The window that ran out, from the latest rate limits: the fullest provider-wide one,
    /// unless only a model's own bucket is used up (then that bucket's window, so the limit
    /// stays with that model).
    fn limit_hit(&self) -> LimitHit {
        if let Some(limit) = self.quota.as_ref().and_then(|quota| quota.limit.clone()) {
            return limit;
        }
        let fullest = |scoped: bool| {
            self.quota.as_ref().and_then(|quota| {
                quota
                    .windows
                    .iter()
                    .filter(|window| window.model.is_some() == scoped)
                    .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
            })
        };
        let main = fullest(false);
        let window = match fullest(true) {
            Some(scoped)
                if scoped.used_percent >= 100.0
                    && main.is_none_or(|main| main.used_percent < 100.0) =>
            {
                Some(scoped)
            }
            _ => main,
        };
        LimitHit {
            window: window.map(|window| window.id.clone()),
            resets_at_ms: window.and_then(|window| window.resets_at_ms),
            kind: LimitKind::UsageWindow,
        }
    }
}

/// The bucket every Codex model draws on (`limitId` of the legacy single-bucket view).
pub(crate) const MAIN_BUCKET: &str = "codex";

/// Normalizes what `account/rateLimits/read` answers: every metered bucket, the main one's
/// windows as `primary` and `secondary`, the others' prefixed with their bucket and tied to the
/// model they meter. A spend control (or ordinary usage not being allowed) stops the provider.
pub fn quota_read(read: &p::GetAccountRateLimitsResponse) -> QuotaSnapshot {
    // Each bucket under its map key (its own `limitId` may be left out).
    let mut buckets: Vec<(&str, &p::RateLimitSnapshot)> = match &read.rate_limits_by_limit_id {
        Some(by_id) if !by_id.is_empty() => by_id
            .iter()
            .map(|(id, bucket)| (id.as_str(), bucket))
            .collect(),
        _ => vec![(bucket_id(&read.rate_limits), &read.rate_limits)],
    };
    // The main bucket first, then the others by id, so the order is stable.
    buckets.sort_by_key(|(id, _)| (*id != MAIN_BUCKET, id.to_owned()));
    let mut quota = QuotaSnapshot {
        provider: ProviderKind::Codex,
        windows: buckets
            .iter()
            .flat_map(|(id, bucket)| bucket_windows(id, bucket))
            .collect(),
        limit: None,
        observed_at_ms: now_ms(),
        source: QuotaSource::Read,
    };
    let main = buckets
        .iter()
        .find(|(id, _)| *id == MAIN_BUCKET)
        .map_or(&read.rate_limits, |(_, bucket)| *bucket);
    quota.limit = bucket_limit(main, &quota.windows);
    let stopped = read.ordinary_usage_allowed == Some(false)
        || buckets
            .iter()
            .any(|(_, bucket)| bucket.spend_control_reached == Some(true));
    if stopped {
        quota.limit = Some(LimitHit {
            window: None,
            resets_at_ms: None,
            kind: LimitKind::SpendControl,
        });
    }
    quota
}

/// Normalizes an `account/rateLimits/updated` notification: one bucket, possibly partial, to be
/// merged into what is known ([`QuotaSnapshot::merge`]).
pub fn quota_event(update: &p::RateLimitSnapshot) -> QuotaSnapshot {
    let windows = bucket_windows(bucket_id(update), update);
    let main = bucket_id(update) == MAIN_BUCKET;
    let mut limit = if main {
        bucket_limit(update, &windows)
    } else {
        None
    };
    if update.spend_control_reached == Some(true) {
        limit = Some(LimitHit {
            window: None,
            resets_at_ms: None,
            kind: LimitKind::SpendControl,
        });
    }
    QuotaSnapshot {
        provider: ProviderKind::Codex,
        windows,
        limit,
        observed_at_ms: now_ms(),
        source: QuotaSource::Event,
    }
}

fn bucket_id(bucket: &p::RateLimitSnapshot) -> &str {
    bucket.limit_id.as_deref().unwrap_or(MAIN_BUCKET)
}

/// A bucket's windows. A bucket other than the main one that reports it was reached has its
/// fullest window marked used up, since only that model is refused.
fn bucket_windows(id: &str, bucket: &p::RateLimitSnapshot) -> Vec<QuotaWindow> {
    let main = id == MAIN_BUCKET;
    let name = bucket
        .limit_name
        .as_deref()
        .or(bucket.normal_model_slug.as_deref())
        .unwrap_or(id);
    let mut windows: Vec<QuotaWindow> = [
        ("primary", &bucket.primary),
        ("secondary", &bucket.secondary),
    ]
    .into_iter()
    .filter_map(|(slot, window)| {
        let window = window.as_ref()?;
        let label = window_label(slot, window.window_duration_mins);
        Some(QuotaWindow {
            id: if main {
                slot.to_owned()
            } else {
                format!("{id}.{slot}")
            },
            label: if main {
                label
            } else {
                format!("{label} ({name})")
            },
            used_percent: f64::from(window.used_percent),
            resets_at_ms: window.resets_at.map(|seconds| seconds * 1_000),
            window_minutes: window.window_duration_mins,
            bucket: Some(id.to_owned()),
            // Another bucket meters one model; one that doesn't say which is scoped to its
            // own id (it limits no model), never read as provider-wide.
            model: if main {
                None
            } else {
                Some(
                    bucket
                        .normal_model_slug
                        .clone()
                        .unwrap_or_else(|| id.to_owned()),
                )
            },
        })
    })
    .collect();
    if !main
        && bucket.rate_limit_reached_type.is_some()
        && let Some(fullest) = windows
            .iter_mut()
            .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
    {
        fullest.used_percent = fullest.used_percent.max(100.0);
    }
    windows
}

/// The provider-wide limit the main bucket reports, with the window that ran out (the fullest).
fn bucket_limit(bucket: &p::RateLimitSnapshot, windows: &[QuotaWindow]) -> Option<LimitHit> {
    let reached = bucket.rate_limit_reached_type.as_ref()?;
    let kind = match reached {
        p::RateLimitReachedType::WorkspaceOwnerCreditsDepleted
        | p::RateLimitReachedType::WorkspaceMemberCreditsDepleted => LimitKind::Credits,
        p::RateLimitReachedType::RateLimitReached
        | p::RateLimitReachedType::WorkspaceOwnerUsageLimitReached
        | p::RateLimitReachedType::WorkspaceMemberUsageLimitReached => LimitKind::UsageWindow,
    };
    let full = windows
        .iter()
        .filter(|window| window.model.is_none())
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent));
    Some(LimitHit {
        window: full.map(|window| window.id.clone()),
        resets_at_ms: full.and_then(|window| window.resets_at_ms),
        kind,
    })
}

fn window_label(id: &str, minutes: Option<i64>) -> String {
    match minutes {
        Some(300) => "5-hour".into(),
        Some(10_080) => "Weekly".into(),
        Some(minutes) if minutes % 1_440 == 0 => format!("{}-day", minutes / 1_440),
        Some(minutes) if minutes % 60 == 0 => format!("{}-hour", minutes / 60),
        Some(minutes) => format!("{minutes}-minute"),
        None => id.into(),
    }
}

fn token_usage(usage: &p::TokenUsageBreakdown) -> TokenUsage {
    TokenUsage {
        input_tokens: usage.input_tokens - usage.cached_input_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_tokens: usage.cache_write_input_tokens,
        output_tokens: usage.output_tokens,
        reasoning_tokens: usage.reasoning_output_tokens,
        cost_usd: None,
    }
}

fn item_status(completed: bool) -> ItemStatus {
    if completed {
        ItemStatus::Completed
    } else {
        ItemStatus::InProgress
    }
}

fn decode<T: DeserializeOwned>(method: &str, params: Value, out: &mut Vec<Output>) -> Option<T> {
    match serde_json::from_value(params) {
        Ok(value) => Some(value),
        Err(err) => {
            out.push(notice(
                NoticeLevel::Warning,
                format!("Codex sent a {method} these bindings cannot read: {err}"),
            ));
            None
        }
    }
}

fn notice(level: NoticeLevel, message: String) -> Output {
    Output::Event(ProviderEvent::Notice { level, message })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn events(parser: &mut Parser, line: &Value) -> Vec<ProviderEvent> {
        parser
            .feed(&line.to_string())
            .into_iter()
            .filter_map(|output| match output {
                Output::Event(event) => Some(event),
                Output::Control(_) => None,
            })
            .collect()
    }

    #[test]
    fn sub_agent_calls_and_their_work_are_reported() {
        let mut parser = Parser::live();
        let call = |status: &str| {
            json!({
                "id": "c1",
                "type": "collabAgentToolCall",
                "tool": "wait",
                "status": status,
                "senderThreadId": "t1",
                "receiverThreadIds": ["t2"],
                "agentsStates": {},
            })
        };
        let started = json!({ "method": "item/started", "params": {
            "item": call("inProgress"), "startedAtMs": 0, "threadId": "t1", "turnId": "u1",
        }});
        assert_eq!(
            events(&mut parser, &started),
            [ProviderEvent::ToolCall {
                item_id: "c1".into(),
                name: "agents/wait".into(),
                input: None,
                status: ItemStatus::InProgress,
                output: None,
            }]
        );
        let activity = json!({ "method": "item/completed", "params": {
            "item": {
                "id": "a1",
                "type": "subAgentActivity",
                "kind": "interacted",
                "agentPath": "worker/1",
                "agentThreadId": "t2",
            },
            "completedAtMs": 1, "threadId": "t1", "turnId": "u1",
        }});
        assert_eq!(
            events(&mut parser, &activity),
            [ProviderEvent::Progress {
                item_id: "t2".into()
            }]
        );
        let interrupted = json!({ "method": "item/completed", "params": {
            "item": call("interrupted"), "completedAtMs": 2, "threadId": "t1", "turnId": "u1",
        }});
        assert!(matches!(
            events(&mut parser, &interrupted).as_slice(),
            [ProviderEvent::ToolCall {
                status: ItemStatus::Failed,
                ..
            }]
        ));
    }
}
