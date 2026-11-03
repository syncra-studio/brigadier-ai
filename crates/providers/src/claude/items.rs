//! Claude Code's session file and its registered subagents' as exact items
//! ([`crate::adapters::Adapter::items`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::items::{Describe, NativeItem, anonymous, content, positions, string};
use crate::model::{ProviderEvent, Role};
use crate::session_file::{FileEntry, item_id};

pub(crate) fn items(
    lines: &[(Option<String>, Value)],
    display: &[FileEntry],
    _: Describe,
) -> Vec<NativeItem> {
    let positions = positions(display);
    let mut items = Vec::new();
    let mut occurrences = HashMap::<String, usize>::new();
    let mut tools = HashMap::<String, String>::new();
    let mut push = |id: String,
                    role: &'static str,
                    tool: Option<String>,
                    kind,
                    text: String,
                    saved: Option<String>,
                    at_ms,
                    at: Option<usize>| {
        items.push(NativeItem {
            id,
            role,
            tool,
            kind,
            text,
            saved,
            at_ms,
            at,
        });
    };
    let mut previous_scope = None;
    for (scope, value) in lines {
        if previous_scope != scope.as_deref() {
            occurrences.clear();
            previous_scope = scope.as_deref();
        }
        let at_ms = string(value, "timestamp")
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .map(|at| at.as_millisecond());
        if value.get("isMeta").and_then(Value::as_bool) == Some(true)
            || value.get("isCompactSummary").and_then(Value::as_bool) == Some(true)
            || (scope.is_none() && value.get("isSidechain").and_then(Value::as_bool) == Some(true))
        {
            continue;
        }
        // A message the user sent while a turn ran: Claude keeps it as an attachment,
        // read as the user's as the display reader does (`session_file`). A background
        // job's notice there is the CLI's line, not the user's.
        if string(value, "type") == Some("attachment")
            && value.pointer("/attachment/type").and_then(Value::as_str) == Some("queued_command")
        {
            let text = content(value.pointer("/attachment/prompt").unwrap_or(&Value::Null));
            if text.is_empty() || text.starts_with('<') {
                continue;
            }
            let native = value
                .pointer("/attachment/source_uuid")
                .or_else(|| value.get("uuid"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| anonymous(value, &mut occurrences));
            let at = positions.get(native.as_str()).copied();
            push(
                scoped(scope.as_deref(), &native),
                "user",
                None,
                "message",
                text,
                None,
                at_ms,
                at,
            );
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = match string(message, "role") {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let native = string(value, "uuid")
            .or_else(|| string(message, "id"))
            .map(str::to_owned)
            .unwrap_or_else(|| anonymous(value, &mut occurrences));
        let at = positions
            .get(native.as_str())
            .copied()
            .or_else(|| string(message, "id").and_then(|id| positions.get(id).copied()))
            .or_else(|| {
                let prefix = format!("{}:", string(message, "id")?);
                display.iter().enumerate().find_map(|(n, entry)| {
                    matches!(
                        &entry.event,
                        ProviderEvent::Message {
                            role: Role::Assistant,
                            ..
                        }
                    )
                    .then(|| item_id(&entry.event))
                    .flatten()
                    .filter(|id| id.starts_with(&prefix))
                    .map(|_| n)
                })
            });
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
                    // Core looks for the blob Brigadier's output hook kept it in otherwise.
                    let saved = value
                        .pointer("/toolUseResult/persistedOutputPath")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| persisted_path(&body))
                        .filter(|p| crate::is_tool_results_file(p));
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
    }
    items
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

/// The session file and the display reader's registered subagents. Include metadata in the
/// stamp too: a newly attached transcript must become searchable even if the parent did not
/// grow.
pub(crate) fn sources(path: &Path) -> Vec<(Option<String>, PathBuf)> {
    let mut sources = vec![(None, path.to_owned())];
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
