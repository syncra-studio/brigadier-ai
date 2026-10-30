//! A CLI's durable record read as exact items ([`crate::adapters::Adapter::items`]): what each
//! adapter's reader gives core, which ids them, places them and finds their saved outputs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::model::{ProviderEvent, Role};
use crate::session_file::{FileEntry, item_id};

/// One item of a CLI's record, before core ids it.
#[derive(Debug, Clone)]
pub struct NativeItem {
    /// Its id as the CLI gave it (or made from its content), namespaced by subagent.
    pub id: String,
    pub role: &'static str,
    pub tool: Option<String>,
    /// `message`, `call` or `output`.
    pub kind: &'static str,
    pub text: String,
    /// The file the CLI saved its whole output to.
    pub saved: Option<String>,
    pub at_ms: Option<i64>,
    /// The display entry it sits at, by index into the display reader's entries.
    pub at: Option<usize>,
}

/// A display entry as core describes it: its text, and the file its whole output was saved to.
pub type Describe<'a> = &'a dyn Fn(&FileEntry) -> Option<(String, Option<String>)>;

/// A record's exact items, from its lines (each with its subagent scope, as
/// [`crate::adapters::Adapter::item_sources`] lists them) and its display reader's entries.
pub type Items = fn(&[(Option<String>, Value)], &[FileEntry], Describe) -> Vec<NativeItem>;

/// The files a session file's exact items are read from, each with its subagent scope.
pub type Sources = fn(&Path) -> Vec<(Option<String>, PathBuf)>;

/// The files a record's lines are read from: its own, alone.
pub(crate) fn own_file(path: &Path) -> Vec<(Option<String>, PathBuf)> {
    vec![(None, path.to_owned())]
}

pub(crate) fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

pub(crate) fn content(value: &Value) -> String {
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

/// An id for a record without one: its content, and how often that content came before.
pub(crate) fn anonymous(value: &Value, occurrences: &mut HashMap<String, usize>) -> String {
    let hash = blake3::hash(value.to_string().as_bytes())
        .to_hex()
        .to_string();
    let n = occurrences.entry(hash.clone()).or_default();
    let id = format!("{hash}:{n}");
    *n += 1;
    id
}

/// Where each display item sits (its last entry).
pub(crate) fn positions(display: &[FileEntry]) -> HashMap<&str, usize> {
    display
        .iter()
        .enumerate()
        .filter_map(|(n, entry)| Some((item_id(&entry.event)?, n)))
        .collect()
}

/// The display reader's items as exact items, for a record the display reader reads 1:1.
pub(crate) fn from_display(display: &[FileEntry], describe: Describe) -> Vec<NativeItem> {
    let positions = positions(display);
    let mut items = Vec::new();
    for entry in display {
        let Some(id) = item_id(&entry.event) else {
            continue;
        };
        let item = |role, tool: Option<&String>, kind, text: String, saved| NativeItem {
            id: id.into(),
            role,
            tool: tool.cloned(),
            kind,
            text,
            saved,
            at_ms: entry.at_ms,
            at: positions.get(id).copied(),
        };
        let exec = "exec_command".to_owned();
        match &entry.event {
            ProviderEvent::Message { role, text, .. } => {
                let role = if *role == Role::User {
                    "user"
                } else {
                    "assistant"
                };
                items.push(item(role, None, "message", text.clone(), None));
            }
            ProviderEvent::Command {
                command, output, ..
            } => {
                items.push(item("tool", Some(&exec), "call", command.clone(), None));
                if let Some(output) = output {
                    items.push(item("tool", Some(&exec), "output", output.clone(), None));
                }
            }
            ProviderEvent::ToolCall {
                name,
                input,
                output,
                ..
            } => {
                let input = input.clone().unwrap_or_default();
                items.push(item("tool", Some(name), "call", input, None));
                if let Some(output) = output {
                    items.push(item("tool", Some(name), "output", output.clone(), None));
                }
            }
            _ => {
                if let Some((body, saved)) = describe(entry) {
                    items.push(item("tool", None, "call", body, saved));
                }
            }
        }
    }
    items
}
