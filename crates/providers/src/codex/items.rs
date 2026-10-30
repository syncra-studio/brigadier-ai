//! Codex's rollout as exact items ([`crate::adapters::Adapter::items`]).

use std::collections::HashMap;

use serde_json::Value;

use crate::items::{Describe, NativeItem, anonymous, content, from_display, string};
use crate::session_file::FileEntry;

pub(crate) fn items(
    lines: &[(Option<String>, Value)],
    display: &[FileEntry],
    describe: Describe,
) -> Vec<NativeItem> {
    let mut items = Vec::new();
    let mut occurrences = HashMap::<String, usize>::new();
    let mut tools = HashMap::<String, String>::new();
    let mut push = |id: String, role, tool: Option<String>, kind, text: String, at_ms| {
        items.push(NativeItem {
            id,
            role,
            tool,
            kind,
            text,
            saved: None,
            at_ms,
            at: None,
        });
    };
    for (_, value) in lines {
        if string(value, "type") != Some("response_item") {
            continue;
        }
        let at_ms = string(value, "timestamp")
            .and_then(|at| at.parse::<jiff::Timestamp>().ok())
            .map(|at| at.as_millisecond());
        let Some(payload) = value.get("payload") else {
            continue;
        };
        match string(payload, "type") {
            Some("message") => {
                let role = match string(payload, "role") {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => continue,
                };
                let body = content(&payload["content"]);
                // A Stop hook's send-back (harness D49): the CLI's, as Claude's
                // `isMeta` feedback is, not the user's words.
                if role == "user" && body.trim_start().starts_with("<hook_prompt ") {
                    continue;
                }
                let id = string(payload, "id")
                    .map(str::to_owned)
                    .unwrap_or_else(|| anonymous(payload, &mut occurrences));
                push(id, role, None, "message", body, at_ms);
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
                    at_ms,
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
                    at_ms,
                );
            }
            _ => {}
        }
    }
    // Older Codex rollouts store finished app-server items. A resumed file can mix
    // both encodings; preserve its old IDs when newer response items are appended.
    items.extend(from_display(display, describe));
    items
}
