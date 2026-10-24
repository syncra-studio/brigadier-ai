//! Exact items from the CLI's durable records, before the display reader merges tool calls
//! and results. IDs namespace native IDs by CLI session; records without IDs use content and
//! occurrence, never a position in the merged lineage. Compaction markers are not items.
use std::collections::HashMap;
use std::path::Path;

use brigadier_providers::{ProviderEvent, ProviderKind, session_file};
use serde_json::Value;

use super::{RecordSession, entry_text, read_lineage, saved_text};

#[derive(Debug, Clone)]
pub(super) struct Item {
    pub id: String,
    pub role: String,
    pub tool: Option<String>,
    pub kind: &'static str,
    pub text: String,
    pub saved: Option<String>,
    pub at_ms: Option<i64>,
    pub position: i64,
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

fn content(value: &Value) -> String {
    if let Some(text) = value.as_str() {
        return text.to_owned();
    }
    if let Some(blocks) = value.as_array() {
        return blocks
            .iter()
            .filter_map(|block| string(block, "text"))
            .collect::<Vec<_>>()
            .join("\n");
    }
    if value.is_null() {
        String::new()
    } else {
        value.to_string()
    }
}

/// Reads originals on every exact read; an index is never the authority for the bytes.
pub(super) fn read(session: &RecordSession) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut known = HashMap::new();
    let mut position = 0i64;
    for generation in read_lineage(&session.record) {
        let Some(file) = generation.file else {
            continue;
        };
        let path = session_file::locate(generation.provider, Path::new(&file));
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let lines: Vec<Value> = text
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        let display = session_file::read(generation.provider, &path).unwrap_or_default();
        let positions: HashMap<&str, i64> = display
            .iter()
            .enumerate()
            .filter_map(|(n, entry)| {
                Some((session_file::item_id(&entry.event)?, position + n as i64))
            })
            .collect();
        let mut occurrences = HashMap::<String, usize>::new();
        let mut tools = HashMap::<String, String>::new();
        let mut push = |native: String,
                        role: &str,
                        tool: Option<String>,
                        kind,
                        body: String,
                        saved: Option<String>,
                        at_ms,
                        at: Option<i64>| {
            let key = format!(
                "{:?}\0{}\0{native}\0{kind}",
                generation.provider, generation.session_id
            );
            let id = format!("i-{}", &blake3::hash(key.as_bytes()).to_hex()[..32]);
            let item = Item {
                id: id.clone(),
                role: role.into(),
                tool,
                kind,
                text: body,
                saved,
                at_ms,
                position: at.unwrap_or(position),
            };
            if let Some(&index) = known.get(&id) {
                items[index] = item;
            } else {
                known.insert(id, items.len());
                items.push(item);
            }
        };
        let canonical_codex = lines
            .iter()
            .any(|v| string(v, "type") == Some("response_item"));
        for value in &lines {
            let at_ms = string(value, "timestamp")
                .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                .map(|at| at.as_millisecond());
            if generation.provider == ProviderKind::Claude {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let Some(role @ ("user" | "assistant")) = string(message, "role") else {
                    continue;
                };
                let native = string(value, "uuid")
                    .or_else(|| string(message, "id"))
                    .map(str::to_owned)
                    .unwrap_or_else(|| anonymous(value, &mut occurrences));
                let at = positions.get(native.as_str()).copied();
                let blocks = message.get("content").unwrap_or(&Value::Null);
                if blocks.is_string() {
                    push(
                        native,
                        role,
                        None,
                        "message",
                        content(blocks),
                        None,
                        at_ms,
                        at,
                    );
                    continue;
                }
                let Some(blocks) = blocks.as_array() else {
                    continue;
                };
                let words = blocks
                    .iter()
                    .filter(|b| string(b, "type") == Some("text"))
                    .filter_map(|b| string(b, "text"))
                    .collect::<Vec<_>>();
                if !words.is_empty() {
                    push(
                        native.clone(),
                        role,
                        None,
                        "message",
                        words.join("\n"),
                        None,
                        at_ms,
                        at,
                    );
                }
                for block in blocks {
                    match string(block, "type") {
                        Some("tool_use") => {
                            let Some(id) = string(block, "id") else {
                                continue;
                            };
                            let name = string(block, "name").unwrap_or("unknown");
                            tools.insert(id.into(), name.into());
                            push(
                                id.into(),
                                "tool",
                                Some(name.into()),
                                "call",
                                content(block.get("input").unwrap_or(&Value::Null)),
                                None,
                                at_ms,
                                positions.get(id).copied(),
                            );
                        }
                        Some("tool_result") => {
                            let Some(id) = string(block, "tool_use_id") else {
                                continue;
                            };
                            let body = content(block.get("content").unwrap_or(&Value::Null));
                            let saved = value
                                .pointer("/toolUseResult/persistedOutputPath")
                                .and_then(Value::as_str)
                                .map(str::to_owned)
                                .or_else(|| persisted_path(&body))
                                .filter(|p| brigadier_providers::is_tool_results_file(p))
                                .or_else(|| super::kept_blob(&body, &session.blobs));
                            push(
                                id.into(),
                                "tool",
                                tools.get(id).cloned(),
                                "output",
                                body,
                                saved,
                                at_ms,
                                positions.get(id).copied(),
                            );
                        }
                        _ => {}
                    }
                }
            } else if string(value, "type") == Some("response_item") {
                let Some(payload) = value.get("payload") else {
                    continue;
                };
                match string(payload, "type") {
                    Some("message") => {
                        let Some(role @ ("user" | "assistant")) = string(payload, "role") else {
                            continue;
                        };
                        let id = string(payload, "id")
                            .map(str::to_owned)
                            .unwrap_or_else(|| anonymous(payload, &mut occurrences));
                        push(
                            id,
                            role,
                            None,
                            "message",
                            content(&payload["content"]),
                            None,
                            at_ms,
                            None,
                        );
                    }
                    Some("function_call" | "custom_tool_call") => {
                        let Some(id) = string(payload, "call_id") else {
                            continue;
                        };
                        let name = string(payload, "name").unwrap_or("unknown");
                        tools.insert(id.into(), name.into());
                        let body = payload
                            .get("arguments")
                            .or_else(|| payload.get("input"))
                            .unwrap_or(&Value::Null);
                        push(
                            id.into(),
                            "tool",
                            Some(name.into()),
                            "call",
                            content(body),
                            None,
                            at_ms,
                            None,
                        );
                    }
                    Some("function_call_output" | "custom_tool_call_output") => {
                        let Some(id) = string(payload, "call_id") else {
                            continue;
                        };
                        push(
                            id.into(),
                            "tool",
                            tools.get(id).cloned(),
                            "output",
                            content(&payload["output"]),
                            None,
                            at_ms,
                            None,
                        );
                    }
                    _ => {}
                }
            }
        }
        // Older Codex rollouts store finished app-server items instead of response items.
        if generation.provider == ProviderKind::Codex && !canonical_codex {
            for entry in &display {
                let Some(id) = session_file::item_id(&entry.event) else {
                    continue;
                };
                let at = positions.get(id).copied();
                match &entry.event {
                    ProviderEvent::Message { role, text, .. } => {
                        let role = if *role == brigadier_providers::Role::User {
                            "user"
                        } else {
                            "assistant"
                        };
                        push(
                            id.into(),
                            role,
                            None,
                            "message",
                            text.clone(),
                            None,
                            entry.at_ms,
                            at,
                        );
                    }
                    ProviderEvent::Command {
                        command, output, ..
                    } => {
                        push(
                            id.into(),
                            "tool",
                            Some("exec_command".into()),
                            "call",
                            command.clone(),
                            None,
                            entry.at_ms,
                            at,
                        );
                        if let Some(output) = output {
                            push(
                                id.into(),
                                "tool",
                                Some("exec_command".into()),
                                "output",
                                output.clone(),
                                None,
                                entry.at_ms,
                                at,
                            );
                        }
                    }
                    ProviderEvent::ToolCall {
                        name,
                        input,
                        output,
                        ..
                    } => {
                        push(
                            id.into(),
                            "tool",
                            Some(name.clone()),
                            "call",
                            input.clone().unwrap_or_default(),
                            None,
                            entry.at_ms,
                            at,
                        );
                        if let Some(output) = output {
                            push(
                                id.into(),
                                "tool",
                                Some(name.clone()),
                                "output",
                                output.clone(),
                                None,
                                entry.at_ms,
                                at,
                            );
                        }
                    }
                    _ => {
                        if let Some((_, body, saved)) = entry_text(entry, &session.blobs) {
                            push(
                                id.into(),
                                "tool",
                                None,
                                "call",
                                body,
                                saved,
                                entry.at_ms,
                                at,
                            );
                        }
                    }
                }
            }
        }
        position += display.len() as i64;
    }
    for item in &mut items {
        if let Some(saved) = &item.saved
            && let Some(whole) = saved_text(saved, &session.blobs)
        {
            item.text = whole;
        }
    }
    items
}

fn anonymous(value: &Value, occurrences: &mut HashMap<String, usize>) -> String {
    let hash = blake3::hash(value.to_string().as_bytes())
        .to_hex()
        .to_string();
    let n = occurrences.entry(hash.clone()).or_default();
    let id = format!("{hash}:{n}");
    *n += 1;
    id
}

fn persisted_path(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        line.split_once("Full output saved to: ")
            .map(|(_, path)| path.trim().to_owned())
    })
}
