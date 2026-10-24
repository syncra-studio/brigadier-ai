//! Claude Code's session file (`<config>/projects/<cwd>/<id>.jsonl`) as thread items. Its
//! lines are the stream's own (`user`, `assistant`, `system`) with the file's extras, so they
//! go through the stream's [`Parser`] and come out with the stream's item ids. What the file
//! adds: queued messages (a steer), the user's Stop ("[Request interrupted by user]"), API
//! errors, outputs too long to keep inline (`tool-results/`, read through their preview) and
//! subagents (`<id>/subagents/agent-*.jsonl`, each beside the `Agent` call that ran it).

use std::collections::HashMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::parse::{Output, Parser, str_of};
use crate::model::{ProviderEvent, Role, TurnStatus};
use crate::session_file::{FileEntry, FileImage, image_blocks, shown};

/// What Claude writes for the user's Stop in place of the rest of the turn.
const INTERRUPTED: &str = "[Request interrupted by user";
/// Claude's own stand-in reply after a Stop.
const NO_RESPONSE: &str = "No response requested.";

/// The thread the session file at `path` holds, its subagents' beside their calls.
pub fn read(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let entries = read_lines(path, None)?;
    Ok(with_subagents(entries, subagents(path)))
}

/// A session file's items with its subagents' (`children`, by the call that ran each)
/// beside their calls.
fn with_subagents(
    mut entries: Vec<FileEntry>,
    mut children: HashMap<String, Vec<FileEntry>>,
) -> Vec<FileEntry> {
    if children.is_empty() {
        return entries;
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
    out
}

/// Each subagent's thread, by the call that ran it.
fn subagents(path: &Path) -> HashMap<String, Vec<FileEntry>> {
    let mut found = HashMap::new();
    for (call, transcript) in subagent_files(path) {
        if let Ok(entries) = read_lines(&transcript, Some(&call)) {
            found.insert(call, entries);
        }
    }
    found
}

/// Each subagent's transcript, with the call that ran it, as the session file at `path`'s
/// directory lists them.
fn subagent_files(path: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
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
        found.push((call, meta.with_file_name(format!("{stem}.jsonl"))));
    }
    found
}

/// A session file and its subagents' transcripts read so far, to read again for only the
/// lines added since: a long session's file runs to tens of MB, and a whole read of it to
/// tens of ms, at each turn's start and end.
pub(crate) struct Reading {
    path: PathBuf,
    main: Option<Part>,
    /// Each subagent's transcript read so far, by its path.
    subagents: HashMap<PathBuf, Part>,
}

impl Reading {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_owned(),
            main: None,
            subagents: HashMap::new(),
        }
    }

    /// The thread the session file holds now, as [`read`] gives it: each file read on from
    /// where this last read it, or whole when it is another file now (replaced, cut short or
    /// rewritten).
    pub(crate) fn read(&mut self) -> std::io::Result<Vec<FileEntry>> {
        let main = Part::read(self.main.take(), &self.path, None)?;
        let mut held = std::mem::take(&mut self.subagents);
        let mut children = HashMap::new();
        for (call, transcript) in subagent_files(&self.path) {
            let Ok(part) = Part::read(held.remove(&transcript), &transcript, Some(&call)) else {
                continue;
            };
            children.insert(call, part.entries.clone());
            self.subagents.insert(transcript, part);
        }
        let entries = with_subagents(main.entries.clone(), children);
        self.main = Some(main);
        Ok(entries)
    }
}

/// One file read so far.
struct Part {
    lines: Lines,
    entries: Vec<FileEntry>,
    /// The bytes read: up to the end of the last whole line. A line still being written
    /// waits for its newline.
    at: u64,
    stamp: Stamp,
}

/// Which file a path was, how long and when it last changed.
#[derive(Clone, Copy, PartialEq)]
struct Stamp {
    identity: u128,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        Self {
            identity: identity(meta),
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }

    /// Whether `now` is this file grown: Claude only ever appends to its files.
    fn grew_to(&self, now: &Stamp) -> bool {
        self.identity == now.identity && now.len > self.len
    }
}

/// The file a path is now: its device and inode.
#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> u128 {
    use std::os::unix::fs::MetadataExt;
    (u128::from(meta.dev()) << 64) | u128::from(meta.ino())
}

/// The file a path is now: when it was made (Windows gives no inode outside nightly Rust).
#[cfg(not(unix))]
fn identity(meta: &std::fs::Metadata) -> u128 {
    meta.created()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |at| at.as_nanos())
}

impl Part {
    /// The file at `path` read on from `held`, or whole when `held` is none or another file.
    fn read(held: Option<Part>, path: &Path, parent: Option<&str>) -> std::io::Result<Part> {
        let mut file = std::fs::File::open(path)?;
        let stamp = Stamp::of(&file.metadata()?);
        let mut part = match held {
            Some(part) if part.stamp == stamp => return Ok(part),
            Some(part) if part.stamp.grew_to(&stamp) => part,
            _ => Part {
                lines: Lines::new(path, parent),
                entries: Vec::new(),
                at: 0,
                stamp,
            },
        };
        file.seek(std::io::SeekFrom::Start(part.at))?;
        let mut bytes = Vec::new();
        file.take(stamp.len.saturating_sub(part.at))
            .read_to_end(&mut bytes)?;
        let whole = bytes
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |end| end + 1);
        let text = std::str::from_utf8(&bytes[..whole])
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        part.lines.feed(text, &mut part.entries);
        part.at += whole as u64;
        part.stamp = stamp;
        Ok(part)
    }
}

/// One file's lines as thread items; `parent` is the subagent call they ran in.
fn read_lines(path: &Path, parent: Option<&str>) -> std::io::Result<Vec<FileEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    Lines::new(path, parent).feed(&text, &mut entries);
    Ok(entries)
}

/// A session file's lines read so far, to read on from: the parser's state and the line
/// count (an image is found again by its line).
struct Lines {
    parser: Parser,
    /// The lines read.
    number: usize,
    file: String,
    parent: Option<String>,
}

impl Lines {
    fn new(path: &Path, parent: Option<&str>) -> Self {
        Self {
            parser: Parser::replay(),
            number: 0,
            file: path.display().to_string(),
            parent: parent.map(str::to_owned),
        }
    }

    /// Reads on through `text`, whole lines (the last may lack its newline only at the
    /// file's end), adding their items to `entries`.
    fn feed(&mut self, text: &str, entries: &mut Vec<FileEntry>) {
        for line in text.lines() {
            let number = self.number;
            self.number += 1;
            self.line(number, line, entries);
        }
    }

    fn line(&mut self, number: usize, line: &str, entries: &mut Vec<FileEntry>) {
        let (file, parser, parent) = (&self.file, &mut self.parser, self.parent.as_deref());
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            return;
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
            Line::Skip => return,
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
                return;
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
            // Written in place of a reply when the next message follows a Stop; its resume
            // view hides it.
            if value.pointer("/message/model").and_then(Value::as_str) == Some("<synthetic>")
                && value.pointer("/message/content").map(text_of).as_deref() == Some(NO_RESPONSE)
            {
                return Line::Skip;
            }
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
        // background job's notice ("<task-notification>…") is the CLI's line, not the user's.
        Some("attachment")
            if value.pointer("/attachment/type").and_then(Value::as_str)
                == Some("queued_command") =>
        {
            let prompt = value
                .pointer("/attachment/prompt")
                .cloned()
                .unwrap_or(Value::Null);
            let text = text_of(&prompt);
            let notice = text.starts_with("<task-notification>");
            if text.is_empty() || (text.starts_with('<') && !notice) {
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
                steer: !notice,
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

/// What the session's `SessionStart(compact)` hooks added after the compaction whose
/// `compact_boundary` is `boundary`, up to the next compaction, in session file `text`: each
/// hook's output whole, as Claude keeps it (`hook_additional_context`). A put-back's receipt.
pub fn restored_after(text: &str, boundary: &str) -> Vec<String> {
    let mut after = false;
    let mut out = Vec::new();
    for value in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if str_of(&value, "subtype") == Some("compact_boundary") {
            if after {
                break;
            }
            after = str_of(&value, "uuid") == Some(boundary);
            continue;
        }
        if after
            && str_of(&value, "type") == Some("attachment")
            && value.pointer("/attachment/type").and_then(Value::as_str)
                == Some("hook_additional_context")
        {
            match value.pointer("/attachment/content") {
                Some(Value::Array(parts)) => {
                    out.extend(parts.iter().filter_map(Value::as_str).map(str::to_owned));
                }
                Some(Value::String(part)) => out.push(part.clone()),
                _ => {}
            }
        }
    }
    out
}

/// The compact summary Claude wrote after the compaction whose `compact_boundary` is
/// `boundary`, in session file `text`: the user line marked `isCompactSummary` that follows it.
pub fn summary_after(text: &str, boundary: &str) -> Option<String> {
    let mut after = false;
    for value in text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        if str_of(&value, "subtype") == Some("compact_boundary") {
            if after {
                return None;
            }
            after = str_of(&value, "uuid") == Some(boundary);
            continue;
        }
        if after && value.get("isCompactSummary").and_then(Value::as_bool) == Some(true) {
            return match value.pointer("/message/content") {
                Some(Value::String(summary)) => Some(summary.clone()),
                Some(Value::Array(parts)) => Some(
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                _ => None,
            };
        }
    }
    None
}

#[cfg(test)]
mod restore_tests {
    use super::{restored_after, summary_after};

    /// A steered `/compact` with eight `SessionStart(compact)` hooks of 9,476 characters each
    /// (step 0, Claude Code 2.1.310): every one arrived, whole.
    const EIGHT: &str = include_str!("../../fixtures/long-sessions/claude-compact-8-slices.jsonl");

    #[test]
    fn the_compact_summary_after_its_boundary_is_read_whole() {
        let summary =
            summary_after(EIGHT, "b63e9690-174c-4cdb-84c0-fc70f4b05098").expect("a summary");
        assert!(summary.starts_with("This session is being continued"));
        assert_eq!(summary_after(EIGHT, "no-such-boundary"), None);
    }

    #[test]
    fn each_hooks_output_after_its_compaction_is_its_receipt_whole() {
        let parts = restored_after(EIGHT, "b63e9690-174c-4cdb-84c0-fc70f4b05098");
        assert_eq!(parts.len(), 8);
        for k in 1..=8 {
            let part = parts
                .iter()
                .find(|part| part.contains(&format!("part {k} of 8]")))
                .unwrap_or_else(|| panic!("part {k}"));
            assert!(part.ends_with(&format!("SLICE-{k}-END")), "part {k} whole");
            assert!(part.chars().count() > 9_000);
        }
        assert!(restored_after(EIGHT, "another-compaction").is_empty());
    }
}
