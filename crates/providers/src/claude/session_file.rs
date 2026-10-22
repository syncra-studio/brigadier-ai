//! Claude Code's session file (`<config>/projects/<cwd>/<id>.jsonl`) as thread items. Its
//! lines are the stream's own (`user`, `assistant`, `system`) with the file's extras, so they
//! go through the stream's [`Parser`] and come out with the stream's item ids. What the file
//! adds: queued messages (a steer), the user's Stop ("[Request interrupted by user]"), API
//! errors, outputs too long to keep inline (`tool-results/`, read through their preview) and
//! subagents (`<id>/subagents/agent-*.jsonl`, each beside the `Agent` call that ran it).

use std::collections::HashMap;
use std::path::Path;

use serde_json::{Value, json};

use super::parse::{Output, Parser, str_of};
use crate::model::{ProviderEvent, Role, TurnStatus};
use crate::session_file::{FileEntry, FileImage, image_blocks, shown};

/// What Claude writes for the user's Stop in place of the rest of the turn.
const INTERRUPTED: &str = "[Request interrupted by user";

/// The thread the session file at `path` holds, its subagents' beside their calls.
pub fn read(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut entries = read_lines(path, None)?;
    let mut children = subagents(path);
    if children.is_empty() {
        return Ok(entries);
    }
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries.drain(..) {
        let call = match &entry.event {
            ProviderEvent::ToolCall { item_id, .. } => Some(item_id.clone()),
            _ => None,
        };
        out.push(entry);
        if let Some(call) = call
            && let Some(inner) = children.remove(&call)
        {
            out.extend(inner);
        }
    }
    Ok(out)
}

/// Each subagent's thread, by the call that ran it.
fn subagents(path: &Path) -> HashMap<String, Vec<FileEntry>> {
    let mut found = HashMap::new();
    let Ok(dir) = std::fs::read_dir(path.with_extension("").join("subagents")) else {
        return found;
    };
    for meta in dir.flatten().map(|entry| entry.path()) {
        let Some(name) = meta.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".meta.json") else {
            continue;
        };
        let Some(call) = std::fs::read(&meta)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|meta| str_of(&meta, "toolUseId").map(str::to_owned))
        else {
            continue;
        };
        let transcript = meta.with_file_name(format!("{stem}.jsonl"));
        if let Ok(entries) = read_lines(&transcript, Some(&call)) {
            found.insert(call, entries);
        }
    }
    found
}

/// One file's lines as thread items; `parent` is the subagent call they ran in.
fn read_lines(path: &Path, parent: Option<&str>) -> std::io::Result<Vec<FileEntry>> {
    let text = std::fs::read_to_string(path)?;
    let file = path.display().to_string();
    let mut parser = Parser::replay();
    let mut entries = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let at_ms = str_of(&value, "timestamp").and_then(crate::time::parse_rfc3339_ms);
        let mut images: Vec<FileImage> = {
            let mut blocks = Vec::new();
            image_blocks(&value, &mut blocks);
            blocks
                .iter()
                .enumerate()
                .map(|(index, (mime, _))| FileImage {
                    file: file.clone(),
                    line: number as u32,
                    index: index as u32,
                    mime: (*mime).to_owned(),
                })
                .collect()
        };
        let (line, steer) = match translate(value, parent.is_some()) {
            Line::Skip => continue,
            Line::Interrupted => {
                entries.push(FileEntry {
                    parent: parent.map(str::to_owned),
                    ..FileEntry::new(
                        ProviderEvent::TurnCompleted {
                            turn_id: None,
                            status: TurnStatus::Interrupted,
                            duration_ms: None,
                            usage: None,
                        },
                        at_ms,
                    )
                });
                continue;
            }
            Line::Feed { line, steer } => (line, steer),
        };
        for output in parser.feed(&line.to_string()) {
            let Output::Event(event) = output else {
                continue;
            };
            if !shown(&event) {
                continue;
            }
            entries.push(FileEntry {
                parent: parent.map(str::to_owned),
                steer,
                images: std::mem::take(&mut images),
                ..FileEntry::new(event, at_ms)
            });
        }
    }
    Ok(entries)
}

enum Line {
    Skip,
    Interrupted,
    /// A line in the stream's own shape.
    Feed {
        line: Value,
        steer: bool,
    },
}

/// A file line in the shape the stream gives it, or what else it is.
fn translate(mut value: Value, subagent: bool) -> Line {
    let meta = value.get("isMeta").and_then(Value::as_bool) == Some(true);
    let sidechain = value.get("isSidechain").and_then(Value::as_bool) == Some(true);
    if meta || (sidechain && !subagent) {
        return Line::Skip;
    }
    match str_of(&value, "type") {
        Some("user") => {
            if value.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
                return Line::Skip;
            }
            value["parent_tool_use_id"] = Value::Null;
            let content = value.pointer("/message/content");
            let results = content.and_then(Value::as_array).is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| str_of(block, "type") == Some("tool_result"))
            });
            if results {
                if let Some(structured) = value.get("toolUseResult").cloned() {
                    value["tool_use_result"] = structured;
                }
                return Line::Feed {
                    line: value,
                    steer: false,
                };
            }
            let text = content.map(text_of).unwrap_or_default();
            if text.starts_with(INTERRUPTED) {
                return Line::Interrupted;
            }
            // A subagent's brief is its call's input, shown with the call.
            if subagent && value.get("parentUuid").is_none_or(Value::is_null) {
                return Line::Skip;
            }
            value["isReplay"] = Value::Bool(true);
            Line::Feed {
                line: value,
                steer: false,
            }
        }
        Some("assistant") => {
            value["parent_tool_use_id"] = Value::Null;
            if value.get("isApiErrorMessage").and_then(Value::as_bool) == Some(true)
                && value.get("error").is_none_or(Value::is_null)
            {
                value["error"] = json!("unknown");
            }
            Line::Feed {
                line: value,
                steer: false,
            }
        }
        Some("system") if str_of(&value, "subtype") == Some("compact_boundary") => {
            if let Some(metadata) = value.get("compactMetadata") {
                value["compact_metadata"] = json!({
                    "trigger": metadata.get("trigger"),
                    "pre_tokens": metadata.get("preTokens"),
                    "post_tokens": metadata.get("postTokens"),
                });
            }
            Line::Feed {
                line: value,
                steer: false,
            }
        }
        // A message the user sent while a turn ran: Claude took it into that turn. A
        // background job's notice ("<task-notification>…") is the CLI's, not the user's.
        Some("attachment")
            if value.pointer("/attachment/type").and_then(Value::as_str)
                == Some("queued_command") =>
        {
            let prompt = value
                .pointer("/attachment/prompt")
                .cloned()
                .unwrap_or(Value::Null);
            let text = text_of(&prompt);
            if text.is_empty() || text.starts_with('<') {
                return Line::Skip;
            }
            let uuid = value
                .pointer("/attachment/source_uuid")
                .or_else(|| value.get("uuid"))
                .cloned()
                .unwrap_or(Value::Null);
            Line::Feed {
                line: json!({
                    "type": "user",
                    "isReplay": true,
                    "parent_tool_use_id": null,
                    "uuid": uuid,
                    "message": {"role": "user", "content": prompt},
                }),
                steer: true,
            }
        }
        _ => Line::Skip,
    }
}

/// The text of message content: a string, or its text blocks joined.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| str_of(block, "type") == Some("text"))
            .filter_map(|block| str_of(block, "text"))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Whether an entry is the user's own message (not a tool's result).
pub fn user_message(entry: &FileEntry) -> bool {
    matches!(
        entry.event,
        ProviderEvent::Message {
            role: Role::User,
            ..
        }
    )
}
