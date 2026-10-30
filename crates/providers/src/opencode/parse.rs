//! OpenCode's server events (`GET /api/event`, one JSON object per SSE `data:` line) as
//! [`ProviderEvent`]s. Shapes as `opencode serve` 2.0.19 sends them (checked live).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde_json::{Value, json};

use crate::model::*;
use crate::session_file::{FileContext, FileEntry};
use crate::{OUTPUT_CLIP, clip};

/// Reads one session's events, as a stream: tool rows are tied to the calls they report on.
#[derive(Default)]
pub(crate) struct Parser {
    /// The session whose events count; others on the stream (a sub-agent's) are left out.
    pub(crate) session: Option<String>,
    /// When the running turn started (`session.execution.started`), while one runs.
    turn_started_ms: Option<i64>,
    /// Each tool call so far: its tool name and input.
    calls: HashMap<String, (String, Value)>,
    /// When each running tool call was called, for its duration.
    called_at: HashMap<String, i64>,
    /// The user messages admitted into the session's inbox (`session.inbox.enqueued`) and not
    /// yet delivered, by id: their text, said once OpenCode delivers them.
    inbox: HashMap<String, String>,
    /// Unanswered permission requests by id → the session that asks (this one, or one of its
    /// sub-agents, which ask under their own) and the tool call they are for.
    pub(crate) asked: HashMap<String, (String, String)>,
    /// Tool calls whose permission the user rejected: they read as declined.
    rejected: HashSet<String>,
    /// The model's context window, once a step names the model ([`Self::with_window`]).
    window: Option<i64>,
}

impl Parser {
    pub(crate) fn new(session: Option<String>, window: Option<i64>) -> Self {
        Self {
            session,
            window,
            ..Self::default()
        }
    }

    /// Whether a turn runs now.
    pub(crate) fn running(&self) -> bool {
        self.turn_started_ms.is_some()
    }

    pub(crate) fn feed(&mut self, line: &str) -> Vec<ProviderEvent> {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        let data = &event["data"];
        let kind = event["type"].as_str().unwrap_or_default();
        let of_session = data["sessionID"].as_str();
        // A sub-agent's session asks under its own id and works out of sight: its requests
        // reach the user, its steps keep this session alive.
        if self.session.is_some()
            && of_session.is_some()
            && of_session != self.session.as_deref()
            && kind != "permission.asked"
        {
            return match kind {
                "session.step.started"
                | "session.tool.called"
                | "session.tool.success"
                | "session.tool.failed"
                | "session.text.ended" => vec![ProviderEvent::Progress {
                    item_id: of_session.unwrap_or_default().to_owned(),
                }],
                _ => Vec::new(),
            };
        }
        let text = |key: &str| data[key].as_str().unwrap_or_default().to_owned();
        let created = event["created"].as_i64();
        let item = |kind: &str| {
            format!(
                "{}:{kind}{}",
                data["assistantMessageID"].as_str().unwrap_or_default(),
                data["ordinal"].as_i64().unwrap_or_default()
            )
        };
        match kind {
            "session.execution.started" => {
                self.turn_started_ms = Some(created.unwrap_or_else(crate::now_ms));
                vec![ProviderEvent::TurnStarted { turn_id: None }]
            }
            "session.execution.succeeded" => vec![self.turn_end(TurnStatus::Completed, created)],
            "session.execution.interrupted" => {
                vec![self.turn_end(TurnStatus::Interrupted, created)]
            }
            // The user's message joins the session when it is delivered: after its turn started,
            // or, a steer, at the running turn's next step. Its id is its message's.
            "session.inbox.enqueued" => {
                if data.pointer("/item/type") == Some(&json!("user")) {
                    let said = data.pointer("/item/payload/text").and_then(Value::as_str);
                    self.inbox
                        .insert(text("inboxID"), said.unwrap_or_default().to_owned());
                }
                Vec::new()
            }
            "session.inbox.delivered" => match self.inbox.remove(&text("inboxID")) {
                Some(said) => vec![ProviderEvent::Message {
                    item_id: text("inboxID"),
                    role: Role::User,
                    text: said,
                }],
                None => Vec::new(),
            },
            "session.execution.failed" => vec![
                ProviderEvent::Error {
                    error: classify(&data["error"], false),
                },
                self.turn_end(TurnStatus::Failed, created),
            ],
            // OpenCode retries on its own: a limit it meets now is said, nothing ends.
            "session.retry.scheduled" => vec![ProviderEvent::Error {
                error: classify(&data["error"], true),
            }],
            "session.text.delta" => vec![ProviderEvent::MessageDelta {
                item_id: item("t"),
                text: text("delta"),
                phase: None,
            }],
            "session.text.ended" => vec![ProviderEvent::Message {
                item_id: item("t"),
                role: Role::Assistant,
                text: text("text"),
            }],
            "session.reasoning.delta" => vec![ProviderEvent::ReasoningDelta {
                item_id: item("r"),
                text: text("delta"),
            }],
            "session.reasoning.ended" => vec![ProviderEvent::Reasoning {
                item_id: item("r"),
                text: text("text"),
            }],
            "session.tool.input.started" => {
                self.calls.insert(text("id"), (text("name"), Value::Null));
                Vec::new()
            }
            "session.tool.called" => {
                let id = text("id");
                if let Some(at) = created {
                    self.called_at.insert(id.clone(), at);
                }
                let call = self.calls.entry(id.clone()).or_default();
                call.1 = data["input"].clone();
                vec![tool_event(
                    &id,
                    call,
                    ItemStatus::InProgress,
                    None,
                    None,
                    data,
                )]
            }
            "session.tool.progress" => vec![ProviderEvent::Progress {
                item_id: text("id"),
            }],
            "session.tool.success" | "session.tool.failed" => {
                let id = text("id");
                let failed = kind == "session.tool.failed";
                let status = match () {
                    _ if self.rejected.remove(&id) => ItemStatus::Declined,
                    _ if failed => ItemStatus::Failed,
                    _ => ItemStatus::Completed,
                };
                let output = if failed {
                    data.pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                } else {
                    Some(content_text(&data["content"]))
                };
                let call = self.calls.remove(&id).unwrap_or_default();
                let took = self
                    .called_at
                    .remove(&id)
                    .zip(created)
                    .map(|(from, to)| to - from);
                vec![tool_event(&id, &call, status, output, took, data)]
            }
            "session.step.ended" => {
                let tokens = &data["tokens"];
                let count = |path: &str| tokens.pointer(path).and_then(Value::as_i64).unwrap_or(0);
                vec![ProviderEvent::ContextSize {
                    used_tokens: count("/input")
                        + count("/cache/read")
                        + count("/cache/write")
                        + count("/output"),
                    window_tokens: self.window,
                }]
            }
            "session.usage.updated" => {
                let tokens = &data["tokens"];
                let count = |path: &str| tokens.pointer(path).and_then(Value::as_i64).unwrap_or(0);
                vec![ProviderEvent::Usage {
                    total: TokenUsage {
                        input_tokens: count("/input"),
                        cached_input_tokens: count("/cache/read"),
                        cache_write_tokens: count("/cache/write"),
                        output_tokens: count("/output"),
                        reasoning_tokens: count("/reasoning"),
                        cost_usd: data["cost"].as_f64(),
                        ..TokenUsage::default()
                    },
                    last: None,
                    calls: Vec::new(),
                }]
            }
            "session.compaction.started" => vec![ProviderEvent::CompactionStarted {
                automatic: data["reason"] == "auto",
            }],
            "session.compaction.ended" | "session.compaction.failed" => {
                let error = (kind == "session.compaction.failed").then(|| {
                    data.pointer("/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("OpenCode couldn't compact the conversation")
                        .to_owned()
                });
                vec![ProviderEvent::CompactionEnded {
                    automatic: data["reason"] == "auto",
                    tokens_before: None,
                    tokens_after: None,
                    error,
                    native_id: None,
                }]
            }
            "permission.asked" => {
                let call = data.pointer("/source/id").and_then(Value::as_str);
                let called = call.and_then(|call| self.calls.get(call));
                let request = approval(data, called.map(|(_, input)| input));
                self.asked.insert(
                    request.id.clone(),
                    (
                        of_session.unwrap_or_default().to_owned(),
                        call.unwrap_or_default().to_owned(),
                    ),
                );
                vec![ProviderEvent::ApprovalRequested { request }]
            }
            "permission.replied" => {
                if let Some((_, call)) = self.asked.remove(&text("requestID"))
                    && data["reply"] == "reject"
                {
                    self.rejected.insert(call);
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn turn_end(&mut self, status: TurnStatus, at: Option<i64>) -> ProviderEvent {
        let started = self.turn_started_ms.take();
        // A request a stopped turn left unanswered is gone with it.
        self.asked.clear();
        self.rejected.clear();
        ProviderEvent::TurnCompleted {
            turn_id: None,
            status,
            duration_ms: started.zip(at).map(|(start, end)| end - start),
            usage: None,
        }
    }
}

/// The text a tool's result holds (`content: [{type: "text", text}]`).
fn content_text(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A tool call as the row it shows as: a command for `shell`, a file change for `edit` and
/// `write`, any other tool as itself. `duration_ms`: how long it ran, once it ended.
fn tool_event(
    id: &str,
    (name, input): &(String, Value),
    status: ItemStatus,
    output: Option<String>,
    duration_ms: Option<i64>,
    data: &Value,
) -> ProviderEvent {
    match name.as_str() {
        "shell" => ProviderEvent::Command {
            item_id: id.to_owned(),
            command: input["command"].as_str().unwrap_or_default().to_owned(),
            cwd: input["workdir"].as_str().map(str::to_owned),
            status: match data.pointer("/metadata/exit").and_then(Value::as_i64) {
                Some(code) if code != 0 && status == ItemStatus::Completed => ItemStatus::Failed,
                _ => status,
            },
            exit_code: data
                .pointer("/metadata/exit")
                .and_then(Value::as_i64)
                .and_then(|code| i32::try_from(code).ok()),
            output,
            duration_ms,
            kept: None,
        },
        "edit" | "write" => {
            // The finished edit names its files with their diffs; before, the input's path.
            let files = data.pointer("/metadata/files").and_then(Value::as_array);
            let changes = match files {
                Some(files) => files
                    .iter()
                    .map(|file| {
                        let kind = match file["status"].as_str() {
                            Some("added") => FileChangeKind::Add,
                            Some("deleted") => FileChangeKind::Delete,
                            _ => FileChangeKind::Update,
                        };
                        FileChange::counted(
                            file["file"].as_str().unwrap_or_default().to_owned(),
                            kind,
                            // Its patch starts with an `Index:` header before the diff's own.
                            file["patch"].as_str().map(|patch| {
                                patch.find("\n--- ").map_or(patch, |at| &patch[at + 1..])
                            }),
                            crate::DIFF_RAW_CLIP,
                        )
                    })
                    .collect(),
                None => vec![FileChange::counted(
                    input["path"].as_str().unwrap_or_default().to_owned(),
                    if name == "write" {
                        FileChangeKind::Add
                    } else {
                        FileChangeKind::Update
                    },
                    None,
                    0,
                )],
            };
            ProviderEvent::FileChanges {
                item_id: id.to_owned(),
                changes,
                status,
            }
        }
        _ => ProviderEvent::ToolCall {
            item_id: id.to_owned(),
            name: name.clone(),
            input: (!input.is_null()).then(|| clip(&input.to_string(), OUTPUT_CLIP)),
            status,
            output,
            kept: None,
        },
    }
}

/// A permission request (`permission.asked`) as an approval card, `called` its tool call's input:
/// a command's request holds only its parts (`a; b || c` asks for `a`, `b`, `c`). OpenCode's "always" answer
/// saves the rule for good, so none is offered ([`ApprovalRequest::grant`] stays empty).
fn approval(data: &Value, called: Option<&Value>) -> ApprovalRequest {
    let action = data["action"].as_str().unwrap_or_default();
    let resources: Vec<String> = data["resources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|resource| resource.as_str().map(str::to_owned))
        .collect();
    let (kind, command, paths) = match action {
        "shell" => (
            ApprovalKind::Command,
            // A sub-agent's call isn't followed: its parts, then.
            Some(
                called
                    .and_then(|input| input["command"].as_str())
                    .map_or_else(|| resources.join(" && "), str::to_owned),
            ),
            Vec::new(),
        ),
        "edit" => (ApprovalKind::FileChange, None, resources),
        "external_directory" => (ApprovalKind::Permissions, None, resources),
        _ => (ApprovalKind::Tool, None, resources),
    };
    let metadata = &data["metadata"];
    ApprovalRequest {
        id: data["id"].as_str().unwrap_or_default().to_owned(),
        kind,
        tool: action.to_owned(),
        command,
        cwd: None,
        paths,
        reason: data["message"].as_str().map(str::to_owned),
        escalation: action == "external_directory",
        input: (metadata.as_object().is_some_and(|m| !m.is_empty()))
            .then(|| clip(&metadata.to_string(), OUTPUT_CLIP)),
        grant: None,
        network: matches!(action, "webfetch" | "websearch" | "codesearch"),
    }
}

/// OpenCode's error (`{type, message, status?}`, its types `provider.quota`,
/// `provider.rate-limit`, `provider.auth`, …) in Brigadier's words. A used-up quota is a usage
/// limit, as the other CLIs report one; OpenCode tells no window or reset (D17).
pub(crate) fn classify(error: &Value, will_retry: bool) -> ProviderError {
    let code = error["type"].as_str().unwrap_or("unknown").to_owned();
    let message = error["message"].as_str().unwrap_or(&code).to_owned();
    let kind = match code.as_str() {
        "provider.quota" => ErrorKind::UsageLimit,
        _ if message.to_lowercase().contains("usage limit") => ErrorKind::UsageLimit,
        "provider.rate-limit" => ErrorKind::RateLimit,
        "provider.auth" => ErrorKind::Auth,
        "provider.content-filter" => ErrorKind::Policy,
        "provider.transport" | "provider.timeout" => ErrorKind::Network,
        "provider.internal" | "provider.invalid-output" => ErrorKind::Server,
        "provider.invalid-request" | "provider.unsupported-operation" => ErrorKind::InvalidRequest,
        _ => ErrorKind::Other,
    };
    ProviderError {
        kind,
        message: clip(&message, 2_000),
        will_retry,
        limit: (kind == ErrorKind::UsageLimit).then_some(LimitHit {
            window: None,
            resets_at_ms: None,
            kind: LimitKind::UsageWindow,
        }),
        code: Some(code),
    }
}

/// The permission rules a session runs under, after its agent's own (OpenCode's later rule
/// wins), so each level means what it means on the other CLIs (see the module docs).
/// `folders`: the session's extra folders, which it reads and works in without asking.
pub(crate) fn permissions(access: Access, tools: ToolSet, folders: &[String]) -> Value {
    let rule = |action: &str, resource: &str, effect: &str| json!({ "action": action, "resource": resource, "effect": effect });
    // Commands that only read run unasked below Full access, as Claude's read-only commands and
    // Codex's safe ones do; one that redirects or writes `--output` still asks. OpenCode matches
    // each command of a line (`a && b`, `$(b)`) on its own.
    let commands = |mut rules: Vec<Value>| {
        rules.push(rule("shell", "*", "ask"));
        rules.extend(
            crate::policy::READ_ONLY
                .iter()
                .map(|command| rule("shell", &format!("{command} *"), "allow")),
        );
        rules.push(rule("shell", "*>*", "ask"));
        rules.push(rule("shell", "*--output*", "ask"));
        rules
    };
    let mut rules = match access {
        Access::Full => vec![rule("*", "*", "allow")],
        // Brigadier's own commands too, as Claude's `Bash(brigadier:*)` at this level.
        Access::AcceptEdits => {
            let mut rules = commands(Vec::new());
            rules.push(rule("shell", "brigadier *", "allow"));
            rules
        }
        Access::Ask => commands(vec![rule("edit", "*", "ask")]),
        // Whatever the agent would ask about is declined: a reader never asks.
        Access::ReadOnly => vec![
            rule("edit", "*", "deny"),
            rule("shell", "*", "deny"),
            rule("subagent", "*", "deny"),
            rule("external_directory", "*", "deny"),
            rule("read", "*.env", "deny"),
            rule("read", "*.env.*", "deny"),
            rule("read", "*.env.example", "allow"),
        ],
    };
    // A side chat's and a tab chat's shell runs `brigadier` and nothing else, as Claude's
    // `Bash(brigadier:*)` under `dontAsk` there: no redirect.
    if access == Access::ReadOnly && matches!(tools, ToolSet::SideChat | ToolSet::TabChat) {
        rules.push(rule("shell", "brigadier *", "allow"));
        rules.push(rule("shell", "*>*", "deny"));
    }
    // Its own question tool waits on a form Brigadier doesn't show: the model asks in words.
    rules.push(rule("question", "*", "deny"));
    for folder in folders {
        rules.push(rule("external_directory", &format!("{folder}/*"), "allow"));
    }
    Value::Array(rules)
}

/// The thread a session's messages hold (`GET /api/session/{id}/message`, oldest first, one
/// JSON object per line, as [`super::write_transcript`] keeps them), in the live stream's words
/// and item ids, so a running session's stream extends what the file holds.
pub(crate) fn read_transcript(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    // When the running turn's first message came, while one runs.
    let mut turn: Option<i64> = None;
    for line in text.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let id = message["id"].as_str().unwrap_or_default();
        let at = message.pointer("/time/created").and_then(Value::as_i64);
        match message["type"].as_str().unwrap_or_default() {
            "user" => {
                let mut entry = FileEntry::new(
                    ProviderEvent::Message {
                        item_id: id.to_owned(),
                        role: Role::User,
                        text: message["text"].as_str().unwrap_or_default().to_owned(),
                    },
                    at,
                );
                // Delivered while a turn ran: it joined that turn (a steer).
                entry.steer = turn.is_some();
                turn = turn.or(at);
                entries.push(entry);
            }
            "assistant" => {
                let (mut texts, mut thoughts) = (0, 0);
                for part in message["content"].as_array().into_iter().flatten() {
                    let part_at = part.pointer("/time/created").and_then(Value::as_i64).or(at);
                    let event = match part["type"].as_str().unwrap_or_default() {
                        "text" => {
                            texts += 1;
                            ProviderEvent::Message {
                                item_id: format!("{id}:t{}", texts - 1),
                                role: Role::Assistant,
                                text: part["text"].as_str().unwrap_or_default().to_owned(),
                            }
                        }
                        "reasoning" => {
                            thoughts += 1;
                            ProviderEvent::Reasoning {
                                item_id: format!("{id}:r{}", thoughts - 1),
                                text: part["text"].as_str().unwrap_or_default().to_owned(),
                            }
                        }
                        "tool" => {
                            let state = &part["state"];
                            let status = match state["status"].as_str() {
                                Some("completed") => ItemStatus::Completed,
                                Some("error") => ItemStatus::Failed,
                                _ => ItemStatus::InProgress,
                            };
                            let output = match status {
                                ItemStatus::Failed => state
                                    .pointer("/error/message")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned),
                                _ => Some(content_text(&state["content"])),
                            };
                            let call = (
                                part["name"].as_str().unwrap_or_default().to_owned(),
                                state["input"].clone(),
                            );
                            let time = |key: &str| part["time"][key].as_i64();
                            let took = time("completed")
                                .filter(|_| status != ItemStatus::InProgress)
                                .zip(time("created"))
                                .map(|(to, from)| to - from);
                            tool_event(
                                part["id"].as_str().unwrap_or_default(),
                                &call,
                                status,
                                output,
                                took,
                                state,
                            )
                        }
                        _ => continue,
                    };
                    entries.push(FileEntry::new(event, part_at));
                }
            }
            "compaction" if message["status"] == "completed" => entries.push(FileEntry::new(
                ProviderEvent::CompactionEnded {
                    automatic: message["reason"] == "auto",
                    tokens_before: None,
                    tokens_after: None,
                    error: None,
                    native_id: None,
                },
                at,
            )),
            "idle" => {
                let status = match message["outcome"].as_str() {
                    Some("succeeded") => TurnStatus::Completed,
                    Some("interrupted") => TurnStatus::Interrupted,
                    _ => TurnStatus::Failed,
                };
                entries.push(FileEntry::new(
                    ProviderEvent::TurnCompleted {
                        turn_id: None,
                        status,
                        duration_ms: turn.take().zip(at).map(|(start, end)| end - start),
                        usage: None,
                    },
                    at,
                ));
            }
            _ => {}
        }
    }
    Ok(entries)
}

/// The context size a transcript line says: an assistant step's tokens, counted as the live
/// stream counts them (`session.step.ended`).
pub(crate) fn context_of(message: &Value) -> Option<Option<FileContext>> {
    if message["type"] != "assistant" {
        return None;
    }
    let tokens = message.get("tokens")?;
    let count = |path: &str| tokens.pointer(path).and_then(Value::as_i64).unwrap_or(0);
    Some(Some(FileContext {
        used_tokens: count("/input")
            + count("/cache/read")
            + count("/cache/write")
            + count("/output"),
        window_tokens: None,
    }))
}

/// For a streamed delta line: its stream key, its item and where its text is
/// ([`crate::record`]).
pub(crate) fn delta_location(line: &Value) -> Option<(String, String, &'static str)> {
    let kind = line.get("type")?.as_str()?;
    let data = line.get("data")?;
    data.get("delta")?.as_str()?;
    let item = part_of(data);
    Some((format!("{kind}|{item}"), item, "/data/delta"))
}

/// The delta streams a line ends: `Some(item)` for one part's, `None` for all of them.
pub(crate) fn stream_end(line: &Value) -> Option<Option<String>> {
    match line.get("type")?.as_str()? {
        "session.text.ended" | "session.reasoning.ended" => Some(Some(part_of(line.get("data")?))),
        kind if kind.starts_with("session.execution.") => Some(None),
        _ => None,
    }
}

/// A text or reasoning part: its message and place in it.
fn part_of(data: &Value) -> String {
    format!(
        "{}:{}",
        data["assistantMessageID"].as_str().unwrap_or_default(),
        data["ordinal"].as_u64().unwrap_or(0)
    )
}

/// A turn that ends because the usage ran out ([`crate::simulate`]): OpenCode tells no window
/// or reset, its provider's quota error ends the turn (D17).
pub(crate) fn usage_limit(now_s: i64) -> Vec<Value> {
    let session = format!("ses_{}", uuid::Uuid::new_v4().simple());
    let event = |kind: &str, mut data: Value| {
        data["sessionID"] = session.clone().into();
        json!({"id": format!("evt_{}", uuid::Uuid::new_v4().simple()), "created": now_s * 1000, "type": kind, "data": data})
    };
    vec![
        event("session.execution.started", json!({})),
        event(
            "session.execution.failed",
            json!({"error": {
                "type": "provider.quota",
                "message": "You've hit your usage limit. Try again in 2 hours.",
                "status": 429,
            }}),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "ses_1";

    fn line(kind: &str, data: Value) -> String {
        json!({ "id": "evt_1", "created": 1_000, "type": kind, "data": data }).to_string()
    }

    fn feed(parser: &mut Parser, lines: &[String]) -> Vec<ProviderEvent> {
        lines.iter().flat_map(|line| parser.feed(line)).collect()
    }

    /// Recorded from `opencode serve` 2.0.19: a turn that streams text, runs a command, and
    /// ends.
    #[test]
    fn a_turn_streams_text_runs_a_command_and_ends() {
        let mut parser = Parser::new(Some(S.into()), Some(262_144));
        let msg = "msg_a";
        let lines = [
            line("session.execution.started", json!({"sessionID": S})),
            line("session.text.delta", json!({"sessionID": S, "assistantMessageID": msg, "ordinal": 0, "delta": "DO"})),
            line("session.text.delta", json!({"sessionID": S, "assistantMessageID": msg, "ordinal": 0, "delta": "NE"})),
            line("session.text.ended", json!({"sessionID": S, "assistantMessageID": msg, "ordinal": 0, "text": "DONE"})),
            line("session.tool.input.started", json!({"sessionID": S, "assistantMessageID": msg, "id": "call_1", "name": "shell"})),
            line("session.tool.called", json!({"sessionID": S, "assistantMessageID": msg, "id": "call_1", "input": {"command": "echo hi > a.txt", "workdir": "/w"}, "executed": false})),
            line("session.tool.success", json!({"sessionID": S, "assistantMessageID": msg, "id": "call_1", "content": [{"type": "text", "text": "(no output)"}], "metadata": {"status": "completed", "truncated": false, "exit": 0}})),
            line("session.step.ended", json!({"sessionID": S, "assistantMessageID": msg, "tokens": {"input": 100, "output": 10, "reasoning": 0, "cache": {"read": 5, "write": 0}}})),
            line("session.usage.updated", json!({"sessionID": S, "cost": 0.5, "tokens": {"input": 100, "output": 10, "reasoning": 1, "cache": {"read": 5, "write": 2}}})),
            // A sub-agent's text is not this session's; its steps keep it alive.
            line("session.text.delta", json!({"sessionID": "ses_sub", "assistantMessageID": msg, "ordinal": 0, "delta": "x"})),
            line("session.tool.called", json!({"sessionID": "ses_sub", "id": "call_9", "input": {}})),
            json!({"id": "evt_9", "created": 1_250, "type": "session.execution.succeeded", "data": {"sessionID": S}}).to_string(),
        ];
        let events = feed(&mut parser, &lines);
        assert_eq!(events[0], ProviderEvent::TurnStarted { turn_id: None });
        assert!(
            matches!(&events[1], ProviderEvent::MessageDelta { item_id, text, .. } if item_id == "msg_a:t0" && text == "DO")
        );
        assert!(
            matches!(&events[3], ProviderEvent::Message { item_id, role: Role::Assistant, text } if item_id == "msg_a:t0" && text == "DONE")
        );
        assert!(
            matches!(&events[4], ProviderEvent::Command { item_id, command, cwd: Some(cwd), status: ItemStatus::InProgress, .. } if item_id == "call_1" && command == "echo hi > a.txt" && cwd == "/w")
        );
        assert!(
            matches!(&events[5], ProviderEvent::Command { status: ItemStatus::Completed, exit_code: Some(0), output: Some(output), .. } if output == "(no output)")
        );
        assert_eq!(
            events[6],
            ProviderEvent::ContextSize {
                used_tokens: 115,
                window_tokens: Some(262_144)
            }
        );
        assert!(
            matches!(&events[7], ProviderEvent::Usage { total, .. } if total.input_tokens == 100 && total.cached_input_tokens == 5 && total.cache_write_tokens == 2 && total.cost_usd == Some(0.5))
        );
        assert_eq!(
            events[8],
            ProviderEvent::Progress {
                item_id: "ses_sub".into()
            }
        );
        assert_eq!(
            events[9],
            ProviderEvent::TurnCompleted {
                turn_id: None,
                status: TurnStatus::Completed,
                duration_ms: Some(250),
                usage: None
            }
        );
        assert_eq!(events.len(), 10);
        assert!(!parser.running());
    }

    /// As `opencode serve` 2.0.19 said them: the user's message is admitted, its turn starts,
    /// then it is delivered; a steer is delivered at the running turn's next step; a stopped
    /// command ends failed. The user's messages come as the other CLIs say theirs, after the
    /// turn's start, and the stopped command says how long it ran.
    #[test]
    fn the_users_messages_come_after_their_turns_start_and_a_stopped_command_has_its_time() {
        let mut parser = Parser::new(Some(S.into()), None);
        let at = |created: i64, kind: &str, data: Value| {
            json!({"id": "evt", "created": created, "type": kind, "data": data}).to_string()
        };
        let events = feed(
            &mut parser,
            &[
                at(
                    1_000,
                    "session.inbox.enqueued",
                    json!({"inboxID": "msg_u1", "sessionID": S, "item": {"type": "user", "payload": {"text": "Sleep 30s.", "files": []}, "delivery": "steer"}}),
                ),
                at(1_001, "session.execution.started", json!({"sessionID": S})),
                at(
                    1_010,
                    "session.inbox.delivered",
                    json!({"sessionID": S, "inboxID": "msg_u1"}),
                ),
                at(
                    2_000,
                    "session.tool.input.started",
                    json!({"sessionID": S, "id": "call_1", "name": "shell"}),
                ),
                at(
                    2_020,
                    "session.tool.called",
                    json!({"sessionID": S, "id": "call_1", "input": {"command": "sleep 30"}}),
                ),
                at(
                    3_000,
                    "session.inbox.enqueued",
                    json!({"inboxID": "msg_u2", "sessionID": S, "item": {"type": "user", "payload": {"text": "Also say hi.", "files": []}, "delivery": "steer"}}),
                ),
                // A sub-agent's own messages are not this session's.
                at(
                    3_001,
                    "session.inbox.enqueued",
                    json!({"inboxID": "msg_x", "sessionID": "ses_sub", "item": {"type": "user", "payload": {"text": "sub"}}}),
                ),
                at(
                    3_002,
                    "session.inbox.delivered",
                    json!({"sessionID": "ses_sub", "inboxID": "msg_x"}),
                ),
                at(
                    4_000,
                    "session.inbox.delivered",
                    json!({"sessionID": S, "inboxID": "msg_u2"}),
                ),
                at(
                    23_020,
                    "session.tool.failed",
                    json!({"sessionID": S, "id": "call_1", "error": {"type": "aborted", "message": "Tool execution interrupted"}}),
                ),
                at(
                    23_050,
                    "session.execution.interrupted",
                    json!({"sessionID": S, "reason": "user"}),
                ),
            ],
        );
        assert_eq!(events[0], ProviderEvent::TurnStarted { turn_id: None });
        assert_eq!(
            events[1],
            ProviderEvent::Message {
                item_id: "msg_u1".into(),
                role: Role::User,
                text: "Sleep 30s.".into()
            }
        );
        assert!(matches!(
            &events[2],
            ProviderEvent::Command {
                status: ItemStatus::InProgress,
                duration_ms: None,
                ..
            }
        ));
        assert!(
            matches!(&events[3], ProviderEvent::Message { item_id, role: Role::User, .. } if item_id == "msg_u2")
        );
        assert!(
            matches!(
                &events[4],
                ProviderEvent::Command {
                    status: ItemStatus::Failed,
                    duration_ms: Some(21_000),
                    ..
                }
            ),
            "{:?}",
            events[4]
        );
        assert!(matches!(
            events[5],
            ProviderEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                duration_ms: Some(22_049),
                ..
            }
        ));
        assert_eq!(events.len(), 6, "{events:?}");
    }

    #[test]
    fn an_edit_shows_its_diff_and_a_rejected_call_reads_declined() {
        let mut parser = Parser::new(Some(S.into()), None);
        let patch = "Index: b.txt\n===\n--- b.txt\n+++ b.txt\n@@ -1,1 +1,1 @@\n-x\n+y\n";
        let events = feed(
            &mut parser,
            &[
                line(
                    "session.tool.input.started",
                    json!({"sessionID": S, "id": "call_e", "name": "edit"}),
                ),
                line(
                    "session.tool.called",
                    json!({"sessionID": S, "id": "call_e", "input": {"path": "/w/b.txt", "oldString": "x", "newString": "y"}}),
                ),
                line(
                    "permission.asked",
                    json!({"id": "per_1", "sessionID": S, "action": "edit", "resources": ["b.txt"], "save": ["*"], "metadata": {"files": [{"file": "b.txt", "patch": patch, "status": "modified"}]}, "source": {"type": "tool", "messageID": "msg_a", "id": "call_e"}}),
                ),
                line(
                    "permission.replied",
                    json!({"sessionID": S, "requestID": "per_1", "reply": "once"}),
                ),
                line(
                    "session.tool.success",
                    json!({"sessionID": S, "id": "call_e", "content": [{"type": "text", "text": "Edited b.txt"}], "metadata": {"files": [{"file": "b.txt", "patch": patch, "status": "modified"}]}}),
                ),
                line(
                    "session.tool.input.started",
                    json!({"sessionID": S, "id": "call_s", "name": "shell"}),
                ),
                line(
                    "session.tool.called",
                    json!({"sessionID": S, "id": "call_s", "input": {"command": "sleep 1; echo hi 2>/dev/null || true"}, "executed": false}),
                ),
                line(
                    "permission.asked",
                    // As OpenCode 2.2.3 asks it (checked live): the command's parts only.
                    json!({"id": "per_2", "sessionID": S, "action": "shell", "resources": ["sleep 1", "echo hi 2>/dev/null", "true"], "save": ["sleep *", "echo *", "true *"], "source": {"type": "tool", "messageID": "msg_a", "id": "call_s"}}),
                ),
                line(
                    "permission.replied",
                    json!({"sessionID": S, "requestID": "per_2", "reply": "reject"}),
                ),
                line(
                    "session.tool.failed",
                    json!({"sessionID": S, "id": "call_s", "error": {"type": "tool.execution", "message": "Unable to execute command: cat a.txt"}}),
                ),
            ],
        );
        assert!(
            matches!(&events[0], ProviderEvent::FileChanges { status: ItemStatus::InProgress, changes, .. } if changes[0].path == "/w/b.txt")
        );
        let ProviderEvent::ApprovalRequested { request } = &events[1] else {
            panic!("no approval: {events:?}")
        };
        assert_eq!(
            (request.kind, request.tool.as_str(), request.id.as_str()),
            (ApprovalKind::FileChange, "edit", "per_1")
        );
        assert_eq!(request.paths, ["b.txt"]);
        assert!(request.grant.is_none());
        let ProviderEvent::FileChanges {
            changes, status, ..
        } = &events[2]
        else {
            panic!("no file change: {events:?}")
        };
        assert_eq!(*status, ItemStatus::Completed);
        assert_eq!(
            (changes[0].kind, changes[0].added, changes[0].removed),
            (FileChangeKind::Update, Some(1), Some(1))
        );
        let ProviderEvent::ApprovalRequested { request } = &events[4] else {
            panic!("no approval: {events:?}")
        };
        assert_eq!(request.kind, ApprovalKind::Command);
        // The command as it runs, not its parts joined.
        assert_eq!(
            request.command.as_deref(),
            Some("sleep 1; echo hi 2>/dev/null || true")
        );
        assert!(
            matches!(&events[5], ProviderEvent::Command { status: ItemStatus::Declined, output: Some(why), .. } if why.contains("Unable"))
        );
        assert_eq!(events.len(), 6);
        // A sub-agent asks under its own session: the card shows, answered there.
        let asked = parser.feed(&line("permission.asked", json!({"id": "per_3", "sessionID": "ses_sub", "action": "edit", "resources": ["c.txt"], "source": {"type": "tool", "messageID": "m", "id": "call_c"}})));
        assert!(
            matches!(&asked[..], [ProviderEvent::ApprovalRequested { request }] if request.id == "per_3")
        );
        assert_eq!(parser.asked["per_3"].0, "ses_sub");
    }

    #[test]
    fn a_tool_row_carries_its_input_and_result() {
        let mut parser = Parser::new(Some(S.into()), None);
        let events = feed(
            &mut parser,
            &[
                line(
                    "session.tool.input.started",
                    json!({"sessionID": S, "id": "call_g", "name": "glob"}),
                ),
                line(
                    "session.tool.called",
                    json!({"sessionID": S, "id": "call_g", "input": {"pattern": "*.txt"}}),
                ),
                line(
                    "session.tool.progress",
                    json!({"sessionID": S, "id": "call_g", "metadata": {}}),
                ),
                line(
                    "session.tool.success",
                    json!({"sessionID": S, "id": "call_g", "content": [{"type": "text", "text": "a.txt"}, {"type": "text", "text": "b.txt"}]}),
                ),
            ],
        );
        assert!(
            matches!(&events[0], ProviderEvent::ToolCall { name, input: Some(input), status: ItemStatus::InProgress, .. } if name == "glob" && input.contains("*.txt"))
        );
        assert_eq!(
            events[1],
            ProviderEvent::Progress {
                item_id: "call_g".into()
            }
        );
        assert!(
            matches!(&events[2], ProviderEvent::ToolCall { status: ItemStatus::Completed, output: Some(out), .. } if out == "a.txt\nb.txt")
        );
    }

    #[test]
    fn a_used_up_quota_is_a_usage_limit_and_a_failed_turn_ends() {
        let mut parser = Parser::new(Some(S.into()), None);
        let events = feed(
            &mut parser,
            &[
                line("session.execution.started", json!({"sessionID": S})),
                line(
                    "session.retry.scheduled",
                    json!({"sessionID": S, "attempt": 1, "at": 5_000, "error": {"type": "provider.rate-limit", "message": "Too many requests", "status": 429}}),
                ),
                line(
                    "session.execution.failed",
                    json!({"sessionID": S, "error": {"type": "provider.quota", "message": "Quota exceeded", "status": 429}}),
                ),
            ],
        );
        let ProviderEvent::Error { error } = &events[1] else {
            panic!("{events:?}")
        };
        assert_eq!((error.kind, error.will_retry), (ErrorKind::RateLimit, true));
        let ProviderEvent::Error { error } = &events[2] else {
            panic!("{events:?}")
        };
        assert_eq!(error.kind, ErrorKind::UsageLimit);
        assert!(
            error
                .limit
                .as_ref()
                .is_some_and(|limit| limit.window.is_none())
        );
        assert_eq!(error.code.as_deref(), Some("provider.quota"));
        assert!(matches!(
            events[3],
            ProviderEvent::TurnCompleted {
                status: TurnStatus::Failed,
                ..
            }
        ));
        let no_route = classify(
            &json!({"type": "provider.no-route", "message": "Model unavailable: opencode/x"}),
            false,
        );
        assert_eq!(no_route.kind, ErrorKind::Other);
    }

    #[test]
    fn compaction_starts_and_ends_and_an_interrupt_ends_the_turn() {
        let mut parser = Parser::new(Some(S.into()), None);
        let events = feed(
            &mut parser,
            &[
                line("session.execution.started", json!({"sessionID": S})),
                line(
                    "session.compaction.started",
                    json!({"sessionID": S, "reason": "manual", "recent": "", "inputID": "msg_c"}),
                ),
                line(
                    "session.compaction.delta",
                    json!({"sessionID": S, "text": "## Objective"}),
                ),
                line(
                    "session.compaction.ended",
                    json!({"sessionID": S, "reason": "manual", "text": "## Objective"}),
                ),
                line(
                    "session.compaction.started",
                    json!({"sessionID": S, "reason": "auto"}),
                ),
                line(
                    "session.compaction.failed",
                    json!({"sessionID": S, "reason": "auto", "error": {"type": "provider.internal", "message": "boom"}}),
                ),
                line(
                    "session.execution.interrupted",
                    json!({"sessionID": S, "reason": "user"}),
                ),
            ],
        );
        assert_eq!(
            events[1],
            ProviderEvent::CompactionStarted { automatic: false }
        );
        assert!(matches!(
            events[2],
            ProviderEvent::CompactionEnded {
                automatic: false,
                error: None,
                ..
            }
        ));
        assert_eq!(
            events[3],
            ProviderEvent::CompactionStarted { automatic: true }
        );
        assert!(
            matches!(&events[4], ProviderEvent::CompactionEnded { automatic: true, error: Some(error), .. } if error == "boom")
        );
        assert!(matches!(
            events[5],
            ProviderEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                ..
            }
        ));
        assert_eq!(events.len(), 6);
    }

    /// Shapes as `GET /api/session/{id}/message` gave them (2.0.19, trimmed): a turn that runs
    /// a command, takes a steer, and ends. Read back, its rows carry the live stream's ids.
    #[test]
    fn a_transcript_reads_back_as_the_live_rows_with_the_steer_in_its_turn() {
        let messages = [
            json!({"id": "msg_u1", "time": {"created": 1_000}, "text": "Run echo two, then reply DONE.", "type": "user"}),
            json!({"id": "msg_a1", "time": {"created": 1_100}, "type": "assistant", "content": [
                {"type": "reasoning", "text": "Run it.", "time": {"created": 1_150}},
                {"type": "tool", "id": "call_1", "name": "shell", "state": {"status": "completed", "input": {"command": "echo two"}, "content": [{"type": "text", "text": "two\n"}], "metadata": {"exit": 0}}, "time": {"created": 1_160, "ran": 1_170, "completed": 1_190}},
            ], "tokens": {"input": 134, "output": 55, "reasoning": 0, "cache": {"read": 12_416, "write": 0}}}),
            json!({"id": "msg_u2", "time": {"created": 1_200}, "text": "Also end with BANANA.", "type": "user"}),
            json!({"id": "msg_a2", "time": {"created": 1_300}, "type": "assistant", "content": [
                {"type": "reasoning", "text": "Done."},
                {"type": "text", "text": "DONE\n\nBANANA"},
            ], "tokens": {"input": 66, "output": 63, "cache": {"read": 12_544, "write": 0}}}),
            json!({"id": "msg_i", "time": {"created": 1_900}, "type": "idle", "outcome": "succeeded"}),
            json!({"id": "msg_m", "time": {"created": 1_950}, "type": "model-switched", "model": {"id": "x", "providerID": "y"}}),
            // The next turn, as 2.0.19 keeps it: an instructions notice may come before its
            // user message; a stopped command ran until the stop.
            json!({"id": "msg_s", "time": {"created": 2_000}, "type": "system", "text": "The Code Mode tool catalog has changed."}),
            json!({"id": "msg_u3", "time": {"created": 2_001}, "text": "Sleep 30s.", "type": "user"}),
            json!({"id": "msg_a3", "time": {"created": 2_010}, "type": "assistant", "content": [
                {"type": "tool", "id": "call_2", "name": "shell", "executed": false, "state": {"status": "error", "error": {"type": "aborted", "message": "Tool execution interrupted"}, "input": {"command": "sleep 30"}, "metadata": {"shellID": "sh_1"}}, "time": {"created": 2_020, "ran": 2_030, "completed": 23_020}},
            ]}),
            json!({"id": "msg_i2", "time": {"created": 23_050}, "type": "idle", "outcome": "interrupted"}),
        ];
        let path = std::env::temp_dir().join(format!(
            "opencode-transcript-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let lines: Vec<String> = messages.iter().map(Value::to_string).collect();
        std::fs::write(&path, lines.join("\n")).unwrap();
        let entries = read_transcript(&path).unwrap();
        let context = crate::session_file::newest(&path, context_of).unwrap();
        std::fs::remove_file(&path).unwrap();

        let ids: Vec<_> = entries
            .iter()
            .map(|entry| crate::session_file::item_id(&entry.event))
            .collect();
        assert_eq!(
            ids,
            [
                Some("msg_u1"),
                Some("msg_a1:r0"),
                Some("call_1"),
                Some("msg_u2"),
                Some("msg_a2:r0"),
                Some("msg_a2:t0"),
                None,
                Some("msg_u3"),
                Some("call_2"),
                None
            ]
        );
        assert!(
            !entries[0].steer && entries[3].steer,
            "the second message joined the turn"
        );
        assert!(
            !entries[7].steer,
            "the turn after the idle line is a new one"
        );
        assert!(
            matches!(&entries[2].event, ProviderEvent::Command { command, status: ItemStatus::Completed, exit_code: Some(0), output: Some(out), duration_ms: Some(30), .. } if command == "echo two" && out == "two\n")
        );
        assert!(
            matches!(
                &entries[8].event,
                ProviderEvent::Command {
                    status: ItemStatus::Failed,
                    duration_ms: Some(21_000),
                    ..
                }
            ),
            "the stopped command says how long it ran: {:?}",
            entries[8].event
        );
        assert!(matches!(
            entries[9].event,
            ProviderEvent::TurnCompleted {
                status: TurnStatus::Interrupted,
                duration_ms: Some(21_049),
                ..
            }
        ));
        assert_eq!(entries[1].at_ms, Some(1_150));
        assert!(
            matches!(&entries[5].event, ProviderEvent::Message { role: Role::Assistant, text, .. } if text == "DONE\n\nBANANA")
        );
        assert_eq!(
            entries[6].event,
            ProviderEvent::TurnCompleted {
                turn_id: None,
                status: TurnStatus::Completed,
                duration_ms: Some(900),
                usage: None
            }
        );
        assert_eq!(
            context,
            Some(FileContext {
                used_tokens: 66 + 12_544 + 63,
                window_tokens: None
            })
        );

        // The live stream names the same parts the same way: its rows replace the file's.
        let mut parser = Parser::new(Some(S.into()), None);
        let live = parser.feed(&line(
            "session.text.ended",
            json!({"sessionID": S, "assistantMessageID": "msg_a2", "ordinal": 0, "text": "DONE"}),
        ));
        assert_eq!(crate::session_file::item_id(&live[0]), Some("msg_a2:t0"));
    }

    fn effect(rules: &Value, action: &str, resource: &str) -> Option<String> {
        // OpenCode's agent defaults first, the session's rules after: the last match wins.
        let defaults = json!([
            {"action": "*", "resource": "*", "effect": "allow"},
            {"action": "external_directory", "resource": "*", "effect": "ask"},
            {"action": "read", "resource": "*.env", "effect": "ask"},
            {"action": "question", "resource": "*", "effect": "allow"},
        ]);
        // OpenCode's wildcard: `*` matches anything, and `ls *` a bare `ls` too (checked live).
        fn glob(pattern: &str, value: &str) -> bool {
            match pattern.split_once('*') {
                None => pattern == value,
                Some((head, rest)) => {
                    pattern.strip_suffix(" *") == Some(value)
                        || value
                            .strip_prefix(head)
                            .is_some_and(|tail| (0..=tail.len()).any(|at| glob(rest, &tail[at..])))
                }
            }
        }
        defaults
            .as_array()
            .unwrap()
            .iter()
            .chain(rules.as_array().unwrap())
            .rfind(|rule| {
                glob(rule["action"].as_str().unwrap(), action)
                    && glob(rule["resource"].as_str().unwrap(), resource)
            })
            .map(|rule| rule["effect"].as_str().unwrap().to_owned())
    }

    /// Each level means on OpenCode what it means on the other CLIs.
    #[test]
    fn each_level_is_opencodes_own_permission_rules() {
        let folders = ["/extra".to_owned()];
        let at = |access| permissions(access, ToolSet::Default, &folders);
        let effect_of = effect;
        let effect = |access, action: &str, resource: &str| effect(&at(access), action, resource);
        let ask = Some("ask".to_owned());
        let allow = Some("allow".to_owned());
        let deny = Some("deny".to_owned());
        // Ask: asks before an edit or a command; reads go ahead.
        assert_eq!(effect(Access::Ask, "edit", "a.rs"), ask);
        assert_eq!(effect(Access::Ask, "shell", "rm a"), ask);
        assert_eq!(effect(Access::Ask, "read", "a.rs"), allow);
        assert_eq!(effect(Access::Ask, "external_directory", "/etc/*"), ask);
        assert_eq!(effect(Access::Ask, "shell", "ls -la"), allow);
        assert_eq!(effect(Access::Ask, "shell", "brigadier start x"), ask);
        // Accept edits: edits, commands that only read and Brigadier's own go ahead; any other
        // command asks, and so does one that redirects or writes `--output`.
        assert_eq!(effect(Access::AcceptEdits, "edit", "a.rs"), allow);
        for command in [
            "ls",
            "ls -la",
            "git status",
            "cat a.rs",
            "brigadier wait 2>&1",
        ] {
            assert_eq!(
                effect(Access::AcceptEdits, "shell", command),
                allow,
                "{command}"
            );
        }
        for command in [
            "rm a",
            "touch t",
            "lsof",
            "git commit -m x",
            "cat a > b",
            "echo x >> ~/.zshrc",
            "git log --output=x",
        ] {
            assert_eq!(
                effect(Access::AcceptEdits, "shell", command),
                ask,
                "{command}"
            );
        }
        // Full access: nothing asks, outside the folder neither.
        for (action, resource) in [
            ("edit", "a"),
            ("shell", "rm x"),
            ("external_directory", "/etc/*"),
        ] {
            assert_eq!(effect(Access::Full, action, resource), allow);
        }
        // Read-only: no edit, no command, never asks.
        assert_eq!(effect(Access::ReadOnly, "edit", "a.rs"), deny);
        assert_eq!(effect(Access::ReadOnly, "shell", "ls"), deny);
        assert_eq!(
            effect(Access::ReadOnly, "external_directory", "/etc/*"),
            deny
        );
        assert_eq!(effect(Access::ReadOnly, "read", ".env"), deny);
        assert_eq!(effect(Access::ReadOnly, "read", "a.rs"), allow);
        assert_eq!(
            effect(Access::ReadOnly, "shell", "brigadier app state"),
            deny
        );
        // A side chat's and a tab chat's: `brigadier` too, nothing else, no redirect.
        for tools in [ToolSet::SideChat, ToolSet::TabChat] {
            let rules = permissions(Access::ReadOnly, tools, &folders);
            for command in ["brigadier app state", "brigadier report save --title P"] {
                assert_eq!(effect_of(&rules, "shell", command), allow);
            }
            for command in ["ls", "rm a", "brigadier app state > out", "python3 -c 1"] {
                assert_eq!(effect_of(&rules, "shell", command), deny, "{command}");
            }
            assert_eq!(effect_of(&rules, "edit", "a.rs"), deny);
        }
        // Every level: the extra folders are the session's; OpenCode's form question never.
        for access in [
            Access::Ask,
            Access::AcceptEdits,
            Access::Full,
            Access::ReadOnly,
        ] {
            assert_eq!(effect(access, "external_directory", "/extra/x/*"), allow);
            assert_eq!(effect(access, "question", "*"), deny);
        }
    }
}
