//! Codex's rollout (`~/.codex/sessions/…/rollout-…-<thread id>.jsonl`) as thread items. Each
//! finished item is written as `event_msg`/`item_completed` with the id the app-server streamed
//! it under, so its events are the stream's ([`super::parse`]) with the same ids. A turn's end
//! is `task_complete` or `turn_aborted`; a user message inside a running turn is a steer.

use std::path::Path;

use serde_json::Value;

use crate::model::{
    FileChange, FileChangeKind, ItemStatus, ProviderEvent, Role, TextPhase, TurnStatus,
};
use crate::session_file::{FileEntry, shown};

/// The thread the rollout at `path` holds.
pub fn read(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    // A turn runs, and whether its user message came yet.
    let mut turn: Option<bool> = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if str_of(&value, "type") != Some("event_msg") {
            continue;
        }
        let at_ms = str_of(&value, "timestamp").and_then(crate::time::parse_rfc3339_ms);
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let mut phase = None;
        let event = match str_of(payload, "type") {
            Some("task_started") => {
                turn = Some(false);
                continue;
            }
            Some("task_complete") => {
                turn = None;
                ProviderEvent::TurnCompleted {
                    turn_id: str_of(payload, "turn_id").map(str::to_owned),
                    status: TurnStatus::Completed,
                    duration_ms: payload.get("duration_ms").and_then(Value::as_i64),
                    usage: None,
                }
            }
            Some("turn_aborted") => {
                turn = None;
                ProviderEvent::TurnCompleted {
                    turn_id: str_of(payload, "turn_id").map(str::to_owned),
                    status: match str_of(payload, "reason") {
                        Some("interrupted" | "replaced") => TurnStatus::Interrupted,
                        _ => TurnStatus::Failed,
                    },
                    duration_ms: payload.get("duration_ms").and_then(Value::as_i64),
                    usage: None,
                }
            }
            Some("item_completed") => {
                let Some(item) = payload.get("item") else {
                    continue;
                };
                let Some(event) = item_event(item) else {
                    continue;
                };
                phase = match str_of(item, "phase") {
                    Some("final_answer") => Some(TextPhase::Answer),
                    Some("commentary") => Some(TextPhase::Commentary),
                    _ => None,
                };
                event
            }
            _ => continue,
        };
        if !shown(&event) {
            continue;
        }
        let user = matches!(
            event,
            ProviderEvent::Message {
                role: Role::User,
                ..
            }
        );
        let steer = user && turn == Some(true);
        if user && turn == Some(false) {
            turn = Some(true);
        }
        entries.push(FileEntry {
            steer,
            phase,
            ..FileEntry::new(event, at_ms)
        });
    }
    Ok(entries)
}

/// A finished item as the stream's event.
fn item_event(item: &Value) -> Option<ProviderEvent> {
    let id = str_of(item, "id")?.to_owned();
    let event = match str_of(item, "type")? {
        "UserMessage" => ProviderEvent::Message {
            item_id: id,
            role: Role::User,
            text: texts(item.get("content")?, "text").join("\n"),
        },
        "AgentMessage" => ProviderEvent::Message {
            item_id: id,
            role: Role::Assistant,
            text: texts(item.get("content")?, "Text").concat(),
        },
        "Reasoning" => {
            let summary: Vec<&str> = item
                .get("summary_text")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            if summary.is_empty() {
                return None;
            }
            ProviderEvent::Reasoning {
                item_id: id,
                text: summary.join("\n\n"),
            }
        }
        "CommandExecution" => ProviderEvent::Command {
            item_id: id,
            command: command_line(item.get("command")?),
            cwd: str_of(item, "cwd").map(|cwd| cwd.trim_start_matches("file://").to_owned()),
            status: status(item),
            exit_code: item
                .get("exit_code")
                .and_then(Value::as_i64)
                .map(|code| code as i32),
            output: str_of(item, "aggregated_output").map(str::to_owned),
            duration_ms: item.get("duration").and_then(duration_ms),
            kept: None,
        },
        "FileChange" => ProviderEvent::FileChanges {
            item_id: id,
            changes: item
                .get("changes")?
                .as_object()?
                .iter()
                .map(|(path, change)| file_change(path, change))
                .collect(),
            status: status(item),
        },
        "McpToolCall" => ProviderEvent::ToolCall {
            item_id: id,
            name: format!(
                "{}/{}",
                str_of(item, "server").unwrap_or("mcp"),
                str_of(item, "tool").unwrap_or("tool")
            ),
            input: item.get("arguments").map(Value::to_string),
            status: status(item),
            output: item
                .get("error")
                .filter(|error| !error.is_null())
                .map(|error| str_of(error, "message").unwrap_or("failed").to_owned())
                .or_else(|| {
                    let content = item.pointer("/result/content")?;
                    Some(texts(content, "text").join("\n"))
                }),
            kept: None,
        },
        "CollabAgentToolCall" => ProviderEvent::ToolCall {
            item_id: id,
            name: format!("agents/{}", str_of(item, "tool").unwrap_or("call")),
            input: str_of(item, "prompt").map(str::to_owned),
            status: status(item),
            output: None,
            kept: None,
        },
        "WebSearch" => ProviderEvent::ToolCall {
            item_id: id,
            name: "web_search".into(),
            input: Some(serde_json::json!({"query": str_of(item, "query")}).to_string()),
            status: ItemStatus::Completed,
            output: None,
            kept: None,
        },
        "ImageView" => ProviderEvent::ToolCall {
            item_id: id,
            name: "view_image".into(),
            input: str_of(item, "path").map(str::to_owned),
            status: ItemStatus::Completed,
            output: None,
            kept: None,
        },
        "Extension" if str_of(item, "kind").is_some_and(|kind| kind.starts_with("image_gen")) => {
            ProviderEvent::Image {
                item_id: id,
                status: if item.get("failure").is_some_and(|f| !f.is_null()) {
                    ItemStatus::Failed
                } else {
                    status(item)
                },
                path: str_of(item, "savedPath").map(str::to_owned),
                prompt: str_of(item, "revisedPrompt").map(str::to_owned),
            }
        }
        "Plan" => ProviderEvent::ToolCall {
            item_id: id,
            name: "plan".into(),
            input: None,
            status: ItemStatus::Completed,
            output: str_of(item, "text").map(str::to_owned),
            kept: None,
        },
        "ContextCompaction" => ProviderEvent::CompactionEnded {
            automatic: true,
            tokens_before: None,
            tokens_after: None,
            error: None,
        },
        _ => return None,
    };
    Some(event)
}

fn str_of<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The `text` of each content part of type `kind`.
fn texts<'a>(content: &'a Value, kind: &str) -> Vec<&'a str> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| str_of(part, "type") == Some(kind))
        .filter_map(|part| str_of(part, "text"))
        .collect()
}

fn status(item: &Value) -> ItemStatus {
    match str_of(item, "status") {
        Some("failed") => ItemStatus::Failed,
        Some("declined") => ItemStatus::Declined,
        Some("inProgress" | "in_progress") => ItemStatus::InProgress,
        _ => ItemStatus::Completed,
    }
}

/// The command as the shell ran it: `zsh -lc '<script>'` shows the script.
fn command_line(command: &Value) -> String {
    match command {
        Value::String(line) => line.clone(),
        Value::Array(parts) => {
            let parts: Vec<&str> = parts.iter().filter_map(Value::as_str).collect();
            match parts.as_slice() {
                [_, flag, script] if flag.starts_with('-') && flag.ends_with('c') => {
                    (*script).to_owned()
                }
                _ => parts.join(" "),
            }
        }
        _ => String::new(),
    }
}

/// `{"secs": 1, "nanos": 5}` or milliseconds.
fn duration_ms(value: &Value) -> Option<i64> {
    if let Some(ms) = value.as_i64() {
        return Some(ms);
    }
    let secs = value.get("secs")?.as_i64()?;
    let nanos = value.get("nanos").and_then(Value::as_i64).unwrap_or(0);
    Some(secs * 1000 + nanos / 1_000_000)
}

fn file_change(path: &str, change: &Value) -> FileChange {
    let (kind, diff) = match str_of(change, "type") {
        Some("add") => (FileChangeKind::Add, str_of(change, "content")),
        Some("delete") => (FileChangeKind::Delete, str_of(change, "content")),
        _ => (FileChangeKind::Update, str_of(change, "unified_diff")),
    };
    let moved_to = str_of(change, "move_path")
        .filter(|moved| *moved != "None")
        .map(str::to_owned);
    FileChange {
        moved_to,
        ..FileChange::counted(path.to_owned(), kind, diff, crate::DIFF_RAW_CLIP)
    }
}
