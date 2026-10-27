//! A CLI's own session file read back as a thread (S9, G11): Claude Code's JSONL
//! ([`crate::claude::session_file`]) and Codex's rollout ([`crate::codex::rollout`]), in the
//! words of the live stream ([`ProviderEvent`]) and with the same item ids, so a running
//! session's stream extends what its file holds and the two meet item by item.

use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{ProviderEvent, ProviderKind};

/// One thing a CLI's session file holds, as its live stream says it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub event: ProviderEvent,
    /// When the CLI wrote it (ms since the epoch), by the file's own timestamps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub at_ms: Option<i64>,
    /// When the CLI last said something of it (ms since the epoch): a call's end, a message's
    /// newest words. None when it said all of it at once (`at_ms`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at_ms: Option<i64>,
    /// The subagent call it was made in (Claude's `Agent` tool call); none in the session's
    /// own thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent: Option<String>,
    /// A user message the CLI took while a turn ran: it joins that turn.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[ts(as = "Option<bool>", optional)]
    pub steer: bool,
    /// The images it carries, read back with [`read_image`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(as = "Option<Vec<FileImage>>", optional)]
    pub images: Vec<FileImage>,
    /// What a reply is, when the file says: a line between the turn's steps, or its final
    /// answer (Codex's `phase`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phase: Option<crate::model::TextPhase>,
}

impl FileEntry {
    pub(crate) fn new(event: ProviderEvent, at_ms: Option<i64>) -> Self {
        Self {
            event,
            at_ms,
            updated_at_ms: None,
            parent: None,
            steer: false,
            images: Vec::new(),
            phase: None,
        }
    }
}

/// An image inside a session file: the `index`th image block of its line `line` (from 0).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileImage {
    pub file: String,
    pub line: u32,
    pub index: u32,
    pub mime: String,
}

/// The thread `kind`'s session file at `path` holds, one entry per item.
pub fn read(kind: ProviderKind, path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let entries = match kind {
        ProviderKind::Claude => crate::claude::session_file::read(path)?,
        ProviderKind::Codex => crate::codex::rollout::read(path)?,
    };
    Ok(merge(Vec::new(), entries))
}

/// How full a session's context is, as its CLI's own file last said (#540).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileContext {
    pub used_tokens: i64,
    /// The model's window, when the file says it (Codex's); Claude's file never does.
    pub window_tokens: Option<i64>,
}

/// The context size `kind`'s session file at `path` last recorded, counted as the live stream
/// counts it ([`ProviderEvent::ContextSize`]): Claude's newest model call in the session's own
/// thread, or what a compaction after it left; Codex's newest `token_count`. Read from the
/// end, so a long session's file isn't read whole. None when the file says none.
pub fn context(kind: ProviderKind, path: &Path) -> std::io::Result<Option<FileContext>> {
    match kind {
        ProviderKind::Claude => newest(path, crate::claude::session_file::context_of),
        ProviderKind::Codex => newest(path, crate::codex::rollout::context_of),
    }
}

/// How much more of a file [`newest`] reads back at a time.
const TAIL_STEP: u64 = 256 * 1024;

/// The answer of the newest line of the JSONL file at `path` that `find` answers for
/// (`Some`); `find` says `None` for a line that tells nothing. Reads back from the end.
pub(crate) fn newest<T>(
    path: &Path,
    find: impl Fn(&serde_json::Value) -> Option<Option<T>>,
) -> std::io::Result<Option<T>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    // Where the lines not read yet end.
    let mut end = file.metadata()?.len();
    let mut span = TAIL_STEP;
    while end > 0 {
        let start = end.saturating_sub(span);
        file.seek(SeekFrom::Start(start))?;
        let mut bytes = vec![0; usize::try_from(end - start).unwrap_or(usize::MAX)];
        file.read_exact(&mut bytes)?;
        // The first line may start before what was read: it is read with the next part. A
        // part with no whole line in it is read again, longer.
        let whole = match start {
            0 => 0,
            _ => match bytes.iter().position(|byte| *byte == b'\n') {
                Some(at) if at + 1 < bytes.len() => at + 1,
                _ => {
                    span = span.saturating_mul(2);
                    continue;
                }
            },
        };
        for line in bytes[whole..].split(|byte| *byte == b'\n').rev() {
            if let Ok(value) = serde_json::from_slice(line)
                && let Some(answer) = find(&value)
            {
                return Ok(answer);
            }
        }
        end = start + whole as u64;
        span = TAIL_STEP;
    }
    Ok(None)
}

/// `kind`'s session file at `path` kept read, to read again for only what was added since:
/// Claude's file and its subagents' are read on from where the last read stopped. Codex's
/// rollout is read whole each time.
pub struct Reading(Kept);

enum Kept {
    Claude(Box<crate::claude::session_file::Reading>),
    Codex(std::path::PathBuf),
}

impl Reading {
    pub fn new(kind: ProviderKind, path: &Path) -> Self {
        Self(match kind {
            ProviderKind::Claude => {
                Kept::Claude(Box::new(crate::claude::session_file::Reading::new(path)))
            }
            ProviderKind::Codex => Kept::Codex(path.to_owned()),
        })
    }

    /// The thread the file holds now, as [`read`] gives it.
    pub fn read(&mut self) -> std::io::Result<Vec<FileEntry>> {
        let entries = match &mut self.0 {
            Kept::Claude(reading) => reading.read()?,
            Kept::Codex(path) => crate::codex::rollout::read(path)?,
        };
        Ok(merge(Vec::new(), entries))
    }
}

/// Where `kind`'s session file recorded at `path` is now. Codex moves a thread's rollout from
/// `<home>/sessions/<y>/<m>/<d>/` to `<home>/archived_sessions/` when it is archived (Brigadier
/// archives a thread it closes, so the user's own Codex apps don't list it), and back when
/// it is resumed. A Claude file that earlier builds recorded under the project folder of a
/// verbatim Windows path (`----C--…`, which Claude never writes) is read where Claude wrote it.
pub fn locate(kind: ProviderKind, path: &Path) -> std::path::PathBuf {
    if path.exists() {
        return path.to_owned();
    }
    let moved = match kind {
        ProviderKind::Codex => path
            .ancestors()
            .find(|dir| dir.file_name().is_some_and(|name| name == "sessions"))
            .and_then(Path::parent)
            .zip(path.file_name())
            .map(|(home, name)| home.join("archived_sessions").join(name)),
        ProviderKind::Claude => crate::claude::files::without_verbatim_project(path),
    };
    moved
        .filter(|moved| moved.exists())
        .unwrap_or_else(|| path.to_owned())
}

/// The item an event is about: its id as the CLI gave it.
pub fn item_id(event: &ProviderEvent) -> Option<&str> {
    match event {
        ProviderEvent::Message { item_id, .. }
        | ProviderEvent::MessageDelta { item_id, .. }
        | ProviderEvent::Reasoning { item_id, .. }
        | ProviderEvent::ReasoningDelta { item_id, .. }
        | ProviderEvent::ToolCall { item_id, .. }
        | ProviderEvent::Command { item_id, .. }
        | ProviderEvent::CommandOutputDelta { item_id, .. }
        | ProviderEvent::FileChanges { item_id, .. }
        | ProviderEvent::Image { item_id, .. }
        | ProviderEvent::TaskNotice { item_id, .. } => Some(item_id),
        _ => None,
    }
}

/// `more` added to `entries`, one entry per item: a later event of an item already there (a
/// call's end after its start, the stream's after the file's) takes its place, which stays
/// where the item began. A call's end keeps what only its start said (its input).
pub fn merge(mut entries: Vec<FileEntry>, more: Vec<FileEntry>) -> Vec<FileEntry> {
    let mut at: std::collections::HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| Some((item_id(&entry.event)?.to_owned(), index)))
        .collect();
    for mut entry in more {
        let Some(id) = item_id(&entry.event).map(str::to_owned) else {
            entries.push(entry);
            continue;
        };
        match at.get(&id) {
            Some(&index) => {
                let earlier = &mut entries[index];
                // A running command's output so far grows its row.
                if let (
                    ProviderEvent::Command { output, .. },
                    ProviderEvent::CommandOutputDelta { text, .. },
                ) = (&mut earlier.event, &entry.event)
                {
                    output.get_or_insert_with(String::new).push_str(text);
                    earlier.updated_at_ms = entry.at_ms.max(earlier.updated_at_ms);
                    continue;
                }
                keep_input(&earlier.event, &mut entry.event);
                if entry.images.is_empty() {
                    entry.images = std::mem::take(&mut earlier.images);
                }
                entry.updated_at_ms = entry
                    .updated_at_ms
                    .or(entry.at_ms)
                    .max(earlier.updated_at_ms);
                entry.at_ms = earlier.at_ms.or(entry.at_ms);
                entry.steer |= earlier.steer;
                entry.parent = entry.parent.take().or(earlier.parent.take());
                *earlier = entry;
            }
            None => {
                at.insert(id, entries.len());
                entries.push(entry);
            }
        }
    }
    entries
}

/// What a call's start said that its end leaves out.
fn keep_input(earlier: &ProviderEvent, later: &mut ProviderEvent) {
    match (earlier, later) {
        (
            ProviderEvent::ToolCall {
                input: Some(input), ..
            },
            ProviderEvent::ToolCall {
                input: later @ None,
                ..
            },
        ) => *later = Some(input.clone()),
        (ProviderEvent::Command { command, .. }, ProviderEvent::Command { command: later, .. })
            if later.is_empty() =>
        {
            *later = command.clone()
        }
        (
            ProviderEvent::Message { text, .. } | ProviderEvent::MessageDelta { text, .. },
            ProviderEvent::MessageDelta { text: later, .. },
        ) => *later = format!("{text}{later}"),
        (
            ProviderEvent::Reasoning { text, .. } | ProviderEvent::ReasoningDelta { text, .. },
            ProviderEvent::ReasoningDelta { text: later, .. },
        ) => *later = format!("{text}{later}"),
        _ => {}
    }
}

/// The bytes of an image a session file holds (base64 in the file).
pub fn read_image(image: &FileImage) -> std::io::Result<Vec<u8>> {
    use base64::Engine as _;
    use std::io::BufRead as _;
    let invalid =
        |what: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, what.to_owned());
    let file = std::io::BufReader::new(std::fs::File::open(&image.file)?);
    let line = file
        .lines()
        .nth(image.line as usize)
        .ok_or_else(|| invalid("no such line"))??;
    let value: serde_json::Value =
        serde_json::from_str(&line).map_err(|err| invalid(&err.to_string()))?;
    let mut found = Vec::new();
    image_blocks(&value, &mut found);
    let data = found
        .get(image.index as usize)
        .ok_or_else(|| invalid("no such image"))?;
    base64::engine::general_purpose::STANDARD
        .decode(data.1.as_bytes())
        .map_err(|err| invalid(&err.to_string()))
}

/// Every base64 image block in `value`, in order: (media type, data).
pub(crate) fn image_blocks<'a>(value: &'a serde_json::Value, out: &mut Vec<(&'a str, &'a str)>) {
    match value {
        serde_json::Value::Object(map) => {
            if map.get("type").and_then(serde_json::Value::as_str) == Some("image")
                && let Some(source) = map.get("source")
                && let Some(data) = source.get("data").and_then(serde_json::Value::as_str)
            {
                let mime = source
                    .get("media_type")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("image/png");
                out.push((mime, data));
                return;
            }
            for child in map.values() {
                image_blocks(child, out);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                image_blocks(child, out);
            }
        }
        _ => {}
    }
}

/// Whether the thread shows `event`: what was said and done, not the session's bookkeeping.
pub(crate) fn shown(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Message { .. }
            | ProviderEvent::Reasoning { .. }
            | ProviderEvent::ToolCall { .. }
            | ProviderEvent::Command { .. }
            | ProviderEvent::FileChanges { .. }
            | ProviderEvent::Image { .. }
            | ProviderEvent::Looked { .. }
            | ProviderEvent::Error { .. }
            | ProviderEvent::TurnCompleted { .. }
            | ProviderEvent::CompactionEnded { .. }
            | ProviderEvent::TaskNotice { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Role, TextPhase};
    use serde_json::{Value, json};

    /// A scratch session file, removed when dropped.
    struct Scratch(std::path::PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A Claude file earlier builds recorded under a verbatim Windows path's project folder is
    /// read where Claude wrote it; one recorded where it is stays.
    #[test]
    fn a_claude_file_recorded_under_a_verbatim_project_is_located() {
        let home = std::env::temp_dir().join(format!("brigadier-locate-{}", uuid::Uuid::new_v4()));
        let written = home.join("projects").join("C--repo").join("s.jsonl");
        std::fs::create_dir_all(written.parent().unwrap()).unwrap();
        std::fs::write(&written, "").unwrap();
        let recorded = home.join("projects").join("----C--repo").join("s.jsonl");
        assert_eq!(locate(ProviderKind::Claude, &recorded), written);
        assert_eq!(locate(ProviderKind::Claude, &written), written);
        let missing = home.join("projects").join("----D--other").join("s.jsonl");
        assert_eq!(locate(ProviderKind::Claude, &missing), missing);
        std::fs::remove_dir_all(&home).unwrap();
    }

    fn context_file(lines: &[Value]) -> (Scratch, std::path::PathBuf) {
        let path =
            std::env::temp_dir().join(format!("brigadier-context-{}.jsonl", uuid::Uuid::new_v4()));
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&path, text.join("\n") + "\n").unwrap();
        (Scratch(path.clone()), path)
    }

    fn claude_call(read: i64, sidechain: bool) -> Value {
        json!({"type": "assistant", "isSidechain": sidechain, "message": {"usage": {
            "input_tokens": 2, "cache_read_input_tokens": read,
            "cache_creation_input_tokens": 10, "output_tokens": 5}}})
    }

    /// Claude's newest call of its own thread counts all it read and wrote; a subagent's and a
    /// stand-in reply's don't count, and a compaction after it leaves what it says (#540).
    #[test]
    fn claudes_context_is_its_newest_own_call_or_what_a_compaction_left() {
        let stand_in = json!({"type": "assistant", "message": {"usage": {
            "input_tokens": 0, "output_tokens": 0}}});
        let (_dir, path) = context_file(&[
            claude_call(100, false),
            claude_call(900, true),
            stand_in,
            json!({"type": "user", "message": {"content": "next"}}),
        ]);
        assert_eq!(
            context(ProviderKind::Claude, &path).unwrap(),
            Some(FileContext {
                used_tokens: 117,
                window_tokens: None
            })
        );
        let (_dir, path) = context_file(&[
            claude_call(100, false),
            json!({"type": "system", "subtype": "compact_boundary",
                "compactMetadata": {"preTokens": 117, "postTokens": 40}}),
        ]);
        assert_eq!(
            context(ProviderKind::Claude, &path)
                .unwrap()
                .map(|c| c.used_tokens),
            Some(40)
        );
        let (_dir, path) = context_file(&[json!({"type": "user"})]);
        assert_eq!(context(ProviderKind::Claude, &path).unwrap(), None);
    }

    /// Codex's newest `token_count` gives its last request and the window.
    #[test]
    fn codexs_context_is_its_newest_token_count() {
        let count = |total: i64| {
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": {
                "last_token_usage": {"total_tokens": total}, "model_context_window": 1000}}})
        };
        let (_dir, path) = context_file(&[
            count(10),
            count(20),
            json!({"type": "event_msg", "payload": {"type": "token_count", "info": null}}),
        ]);
        assert_eq!(
            context(ProviderKind::Codex, &path).unwrap(),
            Some(FileContext {
                used_tokens: 20,
                window_tokens: Some(1000)
            })
        );
    }

    /// The file is read back from its end in parts; a line longer than a part is read whole.
    #[test]
    fn the_newest_line_is_found_past_lines_longer_than_a_part() {
        let long = "x".repeat(usize::try_from(TAIL_STEP * 3).unwrap());
        let (_dir, path) = context_file(&[
            claude_call(100, false),
            json!({"type": "user", "message": {"content": long}}),
            json!({"type": "user", "message": {"content": long}}),
        ]);
        assert_eq!(
            context(ProviderKind::Claude, &path)
                .unwrap()
                .map(|c| c.used_tokens),
            Some(117)
        );
    }

    /// A session file's items, one line each (`BRIGADIER_SESSION_FILE=claude:<path>` or
    /// `codex:<path>`): for comparing a real session with its CLI's own resume view.
    #[test]
    #[ignore = "reads a session file named in the environment"]
    fn print_a_session_file() {
        let Ok(named) = std::env::var("BRIGADIER_SESSION_FILE") else {
            return;
        };
        let (kind, path) = named.split_once(':').expect("<claude|codex>:<path>");
        let kind = if kind == "codex" {
            ProviderKind::Codex
        } else {
            ProviderKind::Claude
        };
        for entry in read(kind, Path::new(path)).unwrap() {
            println!("{}", summary(&entry));
        }
    }

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/session-files")
            .join(name)
    }

    /// A file's items, one line each, against what the CLI shows for it (`<name>.expected`,
    /// checked by hand against the CLI's own resume view of the scrubbed sessions).
    fn reads_as_expected(kind: ProviderKind, name: &str) -> Vec<FileEntry> {
        let entries = read(kind, &fixture(&format!("{name}.jsonl"))).unwrap();
        let shown: Vec<String> = entries.iter().map(summary).collect();
        let expected = std::fs::read_to_string(fixture(&format!("{name}.expected"))).unwrap();
        assert_eq!(shown, expected.lines().collect::<Vec<_>>());
        entries
    }

    /// Tool rows, a steer, a stopped turn, a failed turn, a tool's images and a subagent
    /// beside its call, from real Claude Code sessions (scrubbed).
    #[test]
    fn a_claude_session_file_reads_as_its_thread() {
        let entries = reads_as_expected(ProviderKind::Claude, "claude-session");
        let images = entries
            .iter()
            .find(|entry| !entry.images.is_empty())
            .map(|entry| entry.images.clone())
            .unwrap();
        assert_eq!(images.len(), 4);
        let png = read_image(&images[3]).unwrap();
        assert!(png.starts_with(b"\x89PNG"), "{png:?}");
        // Each item once, where it began, as it ended.
        let mut ids: Vec<&str> = entries.iter().filter_map(|e| item_id(&e.event)).collect();
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all);
        // A subagent's items follow its call and name it.
        let call = entries
            .iter()
            .position(
                |e| matches!(&e.event, ProviderEvent::ToolCall { name, .. } if name == "Agent"),
            )
            .unwrap();
        assert!(
            entries[call + 1..]
                .iter()
                .all(|e| e.parent.as_deref() == item_id(&entries[call].event))
        );
    }

    /// Commands, file changes, MCP calls, images, a steer and a stopped turn, from real Codex
    /// rollouts (scrubbed).
    #[test]
    fn a_codex_rollout_reads_as_its_thread() {
        let entries = reads_as_expected(ProviderKind::Codex, "codex-rollout");
        assert!(entries.iter().all(|entry| entry.at_ms.is_some()));
        // A reply keeps what Codex calls it: the turn's final answer, or a line between steps.
        let answers = entries
            .iter()
            .filter(|entry| entry.phase == Some(TextPhase::Answer))
            .count();
        let lines = std::fs::read_to_string(fixture("codex-rollout.jsonl")).unwrap();
        assert_eq!(answers, lines.matches(r#""phase":"final_answer""#).count());
        assert!(answers > 0);
        assert!(
            entries
                .iter()
                .filter(|entry| entry.phase.is_some())
                .all(|entry| matches!(
                    &entry.event,
                    ProviderEvent::Message {
                        role: Role::Assistant,
                        ..
                    }
                ))
        );
    }

    /// A background job's end Claude took up reads as the CLI's notice line before its reply,
    /// in a turn of its own or folded into a running one, as the live stream's echo of it.
    #[test]
    fn a_background_jobs_end_reads_as_the_clis_notice_line() {
        let notice = |id: &str| {
            format!(
                "<task-notification>\n<task-id>{id}</task-id>\n<status>completed</status>\n<summary>Background command &quot;sleep {id}&quot; completed (exit code 0)</summary>\n</task-notification>"
            )
        };
        let lines = [
            json!({"type":"user", "uuid":"u1", "message":{"role":"user", "content":"Wait for it."}}),
            json!({"type":"attachment", "uuid":"a1", "attachment":{"type":"queued_command",
                "prompt": notice("b1"), "source_uuid":"n1", "commandMode":"task-notification"}}),
            json!({"type":"assistant", "uuid":"r1", "message":{"id":"m1", "role":"assistant",
                "content":[{"type":"text", "text":"Done waiting."}]}}),
            json!({"type":"user", "uuid":"n2", "origin":{"kind":"task-notification"},
                "message":{"role":"user", "content": notice("b2")}}),
            json!({"type":"assistant", "uuid":"r2", "message":{"id":"m2", "role":"assistant",
                "content":[{"type":"text", "text":"The second one ended."}]}}),
        ];
        let path = std::env::temp_dir().join(format!(
            "brigadier-task-notice-{}.jsonl",
            std::process::id()
        ));
        let text: Vec<String> = lines.iter().map(Value::to_string).collect();
        std::fs::write(&path, text.join("\n")).unwrap();
        let entries = read(ProviderKind::Claude, &path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let shown: Vec<String> = entries.iter().map(summary).collect();
        assert_eq!(
            shown,
            [
                "User: Wait for it.",
                r#"notice: Background command "sleep b1" completed (exit code 0)"#,
                "Assistant: Done waiting.",
                r#"notice: Background command "sleep b2" completed (exit code 0)"#,
                "Assistant: The second one ended.",
            ]
        );
        // The live echo of the same line is the same item.
        let echo = json!({"type":"user", "isReplay":true, "uuid":"n2",
            "message":{"role":"user", "content": notice("b2")}});
        let live = crate::claude::parse::Parser::live().feed(&echo.to_string());
        let file = entries
            .iter()
            .find(|entry| item_id(&entry.event) == Some("n2"));
        assert!(
            live.iter().any(|output| matches!(output,
                crate::claude::parse::Output::Event(event) if Some(event) == file.map(|entry| &entry.event))),
            "{live:?}"
        );
    }

    /// The stream extends what the file holds: an item it already has changes in place, and
    /// deltas grow a running message and a command's output.
    #[test]
    fn the_stream_extends_the_file_item_by_item() {
        let message = |id: &str, text: &str| {
            FileEntry::new(
                ProviderEvent::Message {
                    item_id: id.into(),
                    role: crate::model::Role::Assistant,
                    text: text.into(),
                },
                Some(1),
            )
        };
        let command = |status| {
            FileEntry::new(
                ProviderEvent::Command {
                    item_id: "c".into(),
                    command: "ls".into(),
                    cwd: None,
                    status,
                    exit_code: None,
                    output: None,
                    duration_ms: None,
                    kept: None,
                },
                Some(2),
            )
        };
        let file = vec![
            message("a", "Hello."),
            command(crate::model::ItemStatus::InProgress),
        ];
        let stream = vec![
            FileEntry::new(
                ProviderEvent::CommandOutputDelta {
                    item_id: "c".into(),
                    text: "one\n".into(),
                },
                Some(3),
            ),
            message("a", "Hello."),
            FileEntry::new(
                ProviderEvent::MessageDelta {
                    item_id: "b".into(),
                    text: "Wor".into(),
                    phase: None,
                },
                Some(4),
            ),
            FileEntry::new(
                ProviderEvent::MessageDelta {
                    item_id: "b".into(),
                    text: "king".into(),
                    phase: None,
                },
                Some(5),
            ),
        ];
        let merged = merge(file, stream);
        let shown: Vec<String> = merged.iter().map(summary).collect();
        assert_eq!(
            shown,
            [
                "Assistant: Hello.",
                "command ls InProgress",
                "MessageDelta { item_id: \"b\", text: \"Working\", phase: None }"
            ]
        );
        assert!(matches!(
            &merged[1].event,
            ProviderEvent::Command { output: Some(output), .. } if output == "one\n"
        ));
        assert_eq!(merged[2].at_ms, Some(4));
        // Each item says when it was last heard of: the command's output, the message's words.
        assert_eq!(merged[1].updated_at_ms, Some(3));
        assert_eq!(merged[2].updated_at_ms, Some(5));
    }

    /// An archived Codex thread's rollout is found where Codex moved it.
    #[test]
    fn an_archived_rollout_is_found_where_codex_moved_it() {
        let home = std::env::temp_dir().join(format!("session-file-locate-{}", std::process::id()));
        let recorded = home.join("sessions/2026/10/22/rollout-x.jsonl");
        let archived = home.join("archived_sessions/rollout-x.jsonl");
        std::fs::create_dir_all(archived.parent().unwrap()).unwrap();
        std::fs::write(&archived, "").unwrap();
        assert_eq!(locate(ProviderKind::Codex, &recorded), archived);
        assert_eq!(locate(ProviderKind::Claude, &recorded), recorded);
        std::fs::create_dir_all(recorded.parent().unwrap()).unwrap();
        std::fs::write(&recorded, "").unwrap();
        assert_eq!(locate(ProviderKind::Codex, &recorded), recorded);
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A Claude session file kept read gives what a whole read gives as it grows: in chunks,
    /// one ending inside a line, with a subagent appearing later and growing too, and read
    /// whole again once it is replaced by a shorter file.
    #[test]
    fn a_growing_claude_session_file_reads_on_as_a_whole_read_would() {
        let dir = std::env::temp_dir().join(format!(
            "session-file-reading-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        let path = dir.join("session.jsonl");
        let subagents = dir.join("session/subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        let main = std::fs::read(fixture("claude-session.jsonl")).unwrap();
        let agent = "agent-a4796ccd4e8be31af";
        let from = fixture("claude-session/subagents");
        let child = std::fs::read(from.join(format!("{agent}.jsonl"))).unwrap();
        let meta = std::fs::read(from.join(format!("{agent}.meta.json"))).unwrap();
        // A cut inside a line, at a plain character (a whole read of a file cut inside a
        // character fails).
        let inside = |bytes: &[u8], near: usize| {
            (near..bytes.len())
                .find(|&at| bytes[at].is_ascii() && bytes[at] != b'\n' && bytes[at - 1] != b'\n')
                .unwrap()
        };
        let line_end = |bytes: &[u8], near: usize| {
            near + bytes[near..].iter().position(|&b| b == b'\n').unwrap() + 1
        };
        let cuts = [
            inside(&main, main.len() / 5),
            line_end(&main, main.len() / 3),
            inside(&main, main.len() / 2),
            line_end(&main, main.len() * 2 / 3),
            main.len(),
        ];
        let child_cut = inside(&child, child.len() / 2);
        let append = |file: &Path, bytes: &[u8]| {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
                .unwrap()
                .write_all(bytes)
                .unwrap();
        };
        let mut reading = Reading::new(ProviderKind::Claude, &path);
        let mut at = 0;
        for (step, &cut) in cuts.iter().enumerate() {
            append(&path, &main[at..cut]);
            at = cut;
            match step {
                // The subagent's meta first, its transcript not yet written.
                1 => std::fs::write(subagents.join(format!("{agent}.meta.json")), &meta).unwrap(),
                2 => append(
                    &subagents.join(format!("{agent}.jsonl")),
                    &child[..child_cut],
                ),
                3 => append(
                    &subagents.join(format!("{agent}.jsonl")),
                    &child[child_cut..],
                ),
                _ => {}
            }
            let whole = read(ProviderKind::Claude, &path).unwrap();
            assert_eq!(reading.read().unwrap(), whole, "after chunk {step}");
            assert_eq!(
                reading.read().unwrap(),
                whole,
                "read again after chunk {step}"
            );
        }
        assert!(
            reading
                .read()
                .unwrap()
                .iter()
                .any(|entry| entry.parent.is_some()),
            "the subagent's items are in"
        );
        // Replaced by a shorter file: read whole.
        let shorter = dir.join("shorter.jsonl");
        std::fs::write(&shorter, &main[..cuts[1]]).unwrap();
        std::fs::rename(&shorter, &path).unwrap();
        let whole = read(ProviderKind::Claude, &path).unwrap();
        assert_eq!(
            reading.read().unwrap(),
            whole,
            "after the file was replaced"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A whole read of a long Claude session file (about 60 MB) against a kept reading's read
    /// after a few lines were added; prints both.
    #[test]
    #[ignore = "writes and times a 60 MB file"]
    fn time_a_long_session_file_read_whole_and_read_on() {
        let dir = Path::new("/tmp/dlg-lsm-w30");
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("long-session.jsonl");
        let base = std::fs::read_to_string(fixture("claude-session.jsonl")).unwrap();
        // Each copy's items get their own ids, so the thread holds them all.
        let copy = |k: usize| {
            base.replace("toolu_", &format!("toolu_{k}_"))
                .replace("\"msg_", &format!("\"msg_{k}_"))
                .replace("\"uuid\":\"", &format!("\"uuid\":\"{k}-"))
        };
        // A real session file's copy when one is named (`BRIGADIER_LONG_SESSION=<path>`).
        let mut text = std::env::var("BRIGADIER_LONG_SESSION")
            .map(|named| std::fs::read_to_string(named).unwrap())
            .unwrap_or_default();
        let mut k = 0;
        while text.len() < 60_000_000 {
            text.push_str(&copy(k));
            k += 1;
        }
        std::fs::write(&path, &text).unwrap();
        let started = std::time::Instant::now();
        let whole = read(ProviderKind::Claude, &path).unwrap();
        let whole_ms = started.elapsed().as_secs_f64() * 1000.0;
        let mut reading = Reading::new(ProviderKind::Claude, &path);
        reading.read().unwrap();
        // A turn's few lines.
        let added: String = copy(k)
            .lines()
            .take(4)
            .map(|line| format!("{line}\n"))
            .collect();
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(added.as_bytes()).unwrap();
        }
        let started = std::time::Instant::now();
        let on = reading.read().unwrap();
        let on_ms = started.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{} bytes, {} items: whole read {whole_ms:.1} ms; read on after {} bytes added {on_ms:.1} ms ({} items)",
            text.len(),
            whole.len(),
            added.len(),
            on.len()
        );
        std::fs::remove_file(&path).unwrap();
    }

    /// One line for an entry: what it is, then its first words.
    pub(crate) fn summary(entry: &FileEntry) -> String {
        let clip = |text: &str| text.chars().take(60).collect::<String>().replace('\n', " ");
        let what = match &entry.event {
            ProviderEvent::Message { role, text, .. } => format!("{role:?}: {}", clip(text)),
            ProviderEvent::Reasoning { text, .. } => format!("thinking: {}", clip(text)),
            ProviderEvent::ToolCall { name, status, .. } => format!("tool {name} {status:?}"),
            ProviderEvent::Command {
                command, status, ..
            } => format!("command {} {status:?}", clip(command)),
            ProviderEvent::FileChanges { changes, .. } => format!("files {}", changes.len()),
            ProviderEvent::Image { status, .. } => format!("image {status:?}"),
            ProviderEvent::Looked { .. } => "looked".into(),
            ProviderEvent::Error { error } => format!("error: {}", clip(&error.message)),
            ProviderEvent::TurnCompleted { status, .. } => format!("turn end {status:?}"),
            ProviderEvent::CompactionEnded { .. } => "compacted".into(),
            ProviderEvent::TaskNotice { text, .. } => format!("notice: {}", clip(text)),
            other => format!("{other:?}"),
        };
        let parent = entry.parent.as_ref().map_or("", |_| "  ↳ ");
        let steer = if entry.steer { " (steer)" } else { "" };
        let images = if entry.images.is_empty() {
            String::new()
        } else {
            format!(" [{} image]", entry.images.len())
        };
        format!("{parent}{what}{steer}{images}")
    }
}
