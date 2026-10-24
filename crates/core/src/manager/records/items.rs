//! Exact items from the CLI's durable records, before the display reader merges tool calls
//! and results. IDs namespace native IDs by CLI session; records without IDs use content and
//! occurrence, never a position in the merged lineage. Compaction markers are not items.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
        let lines: Vec<(Option<String>, Value)> = sources(generation.provider, &path)
            .into_iter()
            .flat_map(|(scope, path)| {
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .map(|value| (scope.clone(), value))
                    .collect::<Vec<_>>()
            })
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
            .any(|(_, v)| string(v, "type") == Some("response_item"));
        let mut previous_scope = None;
        for (scope, value) in &lines {
            if previous_scope != scope.as_deref() {
                occurrences.clear();
                previous_scope = scope.as_deref();
            }
            let at_ms = string(value, "timestamp")
                .and_then(|at| at.parse::<jiff::Timestamp>().ok())
                .map(|at| at.as_millisecond());
            if generation.provider == ProviderKind::Claude {
                if value.get("isMeta").and_then(Value::as_bool) == Some(true)
                    || value.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
                    || (scope.is_none()
                        && value.get("isSidechain").and_then(Value::as_bool) == Some(true))
                {
                    continue;
                }
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
                let native = scoped(scope.as_deref(), &native);
                let blocks = message.get("content").unwrap_or(&Value::Null);
                if role == "user"
                    && !blocks.as_array().is_some_and(|blocks| {
                        blocks
                            .iter()
                            .any(|block| string(block, "type") == Some("tool_result"))
                    })
                    && (content(blocks).starts_with("[Request interrupted by user")
                        || (scope.is_some() && value.get("parentUuid").is_none_or(Value::is_null)))
                {
                    continue;
                }
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
                            tools.insert(scoped(scope.as_deref(), id), name.into());
                            push(
                                scoped(scope.as_deref(), id),
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
                                scoped(scope.as_deref(), id),
                                "tool",
                                tools.get(&scoped(scope.as_deref(), id)).cloned(),
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

fn scoped(scope: Option<&str>, id: &str) -> String {
    scope.map_or_else(|| id.to_owned(), |scope| format!("subagent:{scope}\0{id}"))
}

/// Match the display reader's registered Claude subagents. Include metadata in the stamp
/// too: a newly attached transcript must become searchable even if the parent did not grow.
pub(super) fn sources(provider: ProviderKind, path: &Path) -> Vec<(Option<String>, PathBuf)> {
    let mut sources = vec![(None, path.to_owned())];
    if provider != ProviderKind::Claude {
        return sources;
    }
    let Ok(dir) = std::fs::read_dir(path.with_extension("").join("subagents")) else {
        return sources;
    };
    let mut files: Vec<_> = dir.flatten().map(|entry| entry.path()).collect();
    files.sort();
    for meta in files {
        let Some(stem) = meta
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(".meta.json"))
        else {
            continue;
        };
        let attached = std::fs::read(&meta)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| string(&value, "toolUseId").map(str::to_owned))
            .is_some_and(|id| !id.is_empty());
        if !attached {
            continue;
        }
        let file = meta.with_file_name(format!("{stem}.jsonl"));
        let scope = Some(stem.to_owned());
        sources.push((scope.clone(), meta));
        sources.push((scope, file));
    }
    sources
}
