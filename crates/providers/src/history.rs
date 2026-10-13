//! Where the user's own CLI sessions ran: the working folders of their past sessions, read from
//! each CLI's session files, so the first run can offer those folders as projects. Only the
//! first lines of each file are read, and nothing is changed.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde_json::Value;

/// A folder the user's sessions of one CLI ran in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PastFolder {
    pub cwd: PathBuf,
    pub sessions: u32,
    pub last_active_ms: i64,
}

/// Lines of a Claude transcript read to find its working folder (the first few are metadata).
const CLAUDE_LINES: usize = 64;
/// Transcripts of one Claude project folder tried before giving up on its working folder.
const CLAUDE_FILES: usize = 3;
/// A line longer than this is not read on (Codex's first line carries its instructions).
const LINE_LIMIT: u64 = 4 * 1024 * 1024;

/// Claude Code keeps each working folder's transcripts in `<config>/projects/<encoded cwd>/`.
/// The folder name is lossy, so the working folder comes from the transcripts' `cwd` field.
pub(crate) fn claude(config: &Path) -> Vec<PastFolder> {
    let Ok(entries) = fs::read_dir(config.join("projects")) else {
        return Vec::new();
    };
    let mut folders = Vec::new();
    for entry in entries.flatten() {
        let mut transcripts: Vec<(PathBuf, i64)> = fs::read_dir(entry.path())
            .into_iter()
            .flatten()
            .flatten()
            .map(|file| file.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .filter_map(|path| {
                let ms = modified_ms(&path)?;
                Some((path, ms))
            })
            .collect();
        if transcripts.is_empty() {
            continue;
        }
        transcripts.sort_by_key(|(_, ms)| std::cmp::Reverse(*ms));
        let cwd = transcripts
            .iter()
            .take(CLAUDE_FILES)
            .find_map(|(path, _)| claude_cwd(path));
        if let Some(cwd) = cwd {
            folders.push(PastFolder {
                cwd,
                sessions: u32::try_from(transcripts.len()).unwrap_or(u32::MAX),
                last_active_ms: transcripts[0].1,
            });
        }
    }
    merge(folders)
}

fn claude_cwd(path: &Path) -> Option<PathBuf> {
    let mut reader = BufReader::new(File::open(path).ok()?);
    for _ in 0..CLAUDE_LINES {
        let line = read_line(&mut reader)?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(Value::as_str) {
            return Some(PathBuf::from(cwd));
        }
    }
    None
}

/// Codex keeps one rollout per thread in `<CODEX_HOME>/sessions/YYYY/MM/DD/rollout-*.jsonl`,
/// its first line the thread's `session_meta` with the working folder.
pub(crate) fn codex(home: &Path) -> Vec<PastFolder> {
    let mut rollouts = Vec::new();
    collect_rollouts(&home.join("sessions"), 0, &mut rollouts);
    let folders = rollouts
        .into_iter()
        .filter_map(|path| {
            let last_active_ms = modified_ms(&path)?;
            let mut reader = BufReader::new(File::open(&path).ok()?);
            let meta: Value = serde_json::from_str(&read_line(&mut reader)?).ok()?;
            let cwd = meta.pointer("/payload/cwd").and_then(Value::as_str)?;
            Some(PastFolder {
                cwd: PathBuf::from(cwd),
                sessions: 1,
                last_active_ms,
            })
        })
        .collect();
    merge(folders)
}

/// Whether Claude session `id`'s transcript (`<config>/projects/*/<id>.jsonl`) holds
/// `needle` in an input the CLI took: a user line's text, or a message queued while a turn ran
/// that the turn took in (a `queued_command` attachment), never a tool result or the model's
/// own words.
pub(crate) fn claude_took(config: &Path, id: &str, needle: &str) -> bool {
    let Ok(entries) = fs::read_dir(config.join("projects")) else {
        return false;
    };
    let name = format!("{id}.jsonl");
    entries
        .flatten()
        .map(|entry| entry.path().join(&name))
        .filter(|path| path.is_file())
        .any(|path| {
            any_line(&path, needle, |line| {
                let content = match line.get("type").and_then(Value::as_str) {
                    Some("user") => line.pointer("/message/content"),
                    Some("attachment")
                        if line.pointer("/attachment/type").and_then(Value::as_str)
                            == Some("queued_command") =>
                    {
                        line.pointer("/attachment/prompt")
                    }
                    _ => return false,
                };
                match content {
                    Some(Value::String(text)) => text.contains(needle),
                    Some(Value::Array(parts)) => parts.iter().any(|part| {
                        part.get("type").and_then(Value::as_str) == Some("text")
                            && part
                                .get("text")
                                .and_then(Value::as_str)
                                .is_some_and(|text| text.contains(needle))
                    }),
                    _ => false,
                }
            })
        })
}

/// Whether Codex thread `id`'s rollout (`<CODEX_HOME>/sessions/**/rollout-*-<id>.jsonl`)
/// holds `needle` in an input the CLI took: a user message, never a tool's output or the
/// model's own words.
pub(crate) fn codex_took(home: &Path, id: &str, needle: &str) -> bool {
    let mut rollouts = Vec::new();
    collect_rollouts(&home.join("sessions"), 0, &mut rollouts);
    let suffix = format!("-{id}.jsonl");
    rollouts
        .iter()
        .filter(|path| path.to_string_lossy().ends_with(&suffix))
        .any(|path| {
            any_line(path, needle, |line| {
                let payload = line.get("payload").unwrap_or(&Value::Null);
                let kind = payload.get("type").and_then(Value::as_str);
                let texts = |parts: Option<&Value>, part_kind: &str| {
                    parts.and_then(Value::as_array).is_some_and(|parts| {
                        parts.iter().any(|part| {
                            part.get("type").and_then(Value::as_str) == Some(part_kind)
                                && part
                                    .get("text")
                                    .and_then(Value::as_str)
                                    .is_some_and(|text| text.contains(needle))
                        })
                    })
                };
                match (line.get("type").and_then(Value::as_str), kind) {
                    (Some("response_item"), Some("message")) => {
                        payload.get("role").and_then(Value::as_str) == Some("user")
                            && texts(payload.get("content"), "input_text")
                    }
                    (Some("event_msg"), Some("user_message")) => payload
                        .get("message")
                        .and_then(Value::as_str)
                        .is_some_and(|text| text.contains(needle)),
                    (Some("event_msg"), Some("item_completed")) => {
                        let item = payload.get("item").unwrap_or(&Value::Null);
                        item.get("type").and_then(Value::as_str) == Some("UserMessage")
                            && texts(item.get("content"), "text")
                    }
                    _ => false,
                }
            })
        })
}

/// Whether a line of the JSONL file at `path` that mentions `needle` is one `takes`.
fn any_line(path: &Path, needle: &str, takes: impl Fn(&Value) -> bool) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return false,
            Ok(_) => {}
        }
        if line.contains(needle)
            && serde_json::from_str::<Value>(&line).is_ok_and(|value| takes(&value))
        {
            return true;
        }
    }
}

fn collect_rollouts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth < 3 {
            collect_rollouts(&path, depth + 1, out);
        } else if kind.is_file()
            && path.extension().is_some_and(|ext| ext == "jsonl")
            && entry.file_name().to_string_lossy().starts_with("rollout-")
        {
            out.push(path);
        }
    }
}

/// One entry per folder, most recent first.
fn merge(folders: Vec<PastFolder>) -> Vec<PastFolder> {
    let mut by_cwd: HashMap<PathBuf, PastFolder> = HashMap::new();
    for folder in folders {
        match by_cwd.get_mut(&folder.cwd) {
            Some(known) => {
                known.sessions = known.sessions.saturating_add(folder.sessions);
                known.last_active_ms = known.last_active_ms.max(folder.last_active_ms);
            }
            None => {
                by_cwd.insert(folder.cwd.clone(), folder);
            }
        }
    }
    let mut merged: Vec<_> = by_cwd.into_values().collect();
    merged.sort_by(|a, b| {
        b.last_active_ms
            .cmp(&a.last_active_ms)
            .then_with(|| a.cwd.cmp(&b.cwd))
    });
    merged
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut line = String::new();
    let read = reader.take(LINE_LIMIT).read_line(&mut line).ok()?;
    (read > 0).then_some(line)
}

fn modified_ms(path: &Path) -> Option<i64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    let ms = modified.duration_since(UNIX_EPOCH).ok()?.as_millis();
    i64::try_from(ms).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-history-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    fn write_lines(path: &Path, values: &[Value]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let text: String = values.iter().map(|value| format!("{value}\n")).collect();
        fs::write(path, text).unwrap();
    }

    const HEADER: &str = "[message from session \"API\" in Flow · id m-1]";

    /// A Claude session took a message sent or steered in, not one only a tool's output or
    /// the model's own words carry.
    #[test]
    fn a_claude_input_counts_and_tool_output_doesnt() {
        let home = scratch("claude");
        let path = home.0.join("projects/-repo/s-1.jsonl");
        write_lines(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t", "content": HEADER}]}}),
                json!({"type": "assistant", "message": {"role": "assistant", "content": [
                    {"type": "text", "text": HEADER}]}}),
            ],
        );
        assert!(!claude_took(&home.0, "s-1", "id m-1"));
        assert!(!claude_took(&home.0, "s-2", "id m-1"));
        write_lines(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "text", "text": format!("{HEADER}\nHello.")}]}}),
            ],
        );
        assert!(claude_took(&home.0, "s-1", "id m-1"));
        write_lines(
            &path,
            &[
                json!({"type": "user", "message": {"role": "user", "content": format!("{HEADER}\nHello.")}}),
            ],
        );
        assert!(claude_took(&home.0, "s-1", "id m-1"));
        // Steered in while a turn ran: queued, then taken in by the turn.
        write_lines(
            &path,
            &[
                json!({"type": "attachment", "attachment": {"type": "queued_command",
                    "prompt": format!("{HEADER}\nHello.")}}),
            ],
        );
        assert!(claude_took(&home.0, "s-1", "id m-1"));
        write_lines(
            &path,
            &[
                json!({"type": "attachment", "attachment": {"type": "queued_command",
                    "prompt": [{"type": "text", "text": HEADER}]}}),
            ],
        );
        assert!(claude_took(&home.0, "s-1", "id m-1"));
        // Any other attachment is not an input.
        write_lines(
            &path,
            &[
                json!({"type": "attachment", "attachment": {"type": "edited_text_file",
                    "prompt": HEADER}}),
            ],
        );
        assert!(!claude_took(&home.0, "s-1", "id m-1"));
    }

    /// A message steered into a running Claude turn, as Claude Code 2.1.296 wrote it (a live
    /// run's record, its ids and paths replaced): a user line of the same prompt, its text
    /// the envelope, which counts as taken.
    #[test]
    fn a_claude_steer_as_the_cli_records_it_counts() {
        let home = scratch("claude-steer");
        let path = home.0.join("projects/-tmp-orch-c-1/s-1.jsonl");
        write_lines(
            &path,
            &[json!({
                "parentUuid": "u-0",
                "isSidechain": false,
                "promptId": "p-1",
                "type": "user",
                "message": {"role": "user", "content": [{
                    "text": "[message from session \"Math helper\" in Alpha · session c-2 · id m-1]\nPlease end your current answer with the line: steered OK.",
                    "type": "text"
                }]},
                "uuid": "u-1",
                "timestamp": "2026-10-13T22:41:54.687Z",
                "permissionMode": "bypassPermissions",
                "promptSource": "sdk",
                "turnOrigin": "sdk",
                "turnPosition": {"promptIndex": 0, "turnIndex": 3},
                "userType": "external",
                "entrypoint": "sdk-cli",
                "cwd": "/tmp/orch/c-1",
                "sessionId": "s-1",
                "version": "2.1.296",
                "gitBranch": "HEAD"
            })],
        );
        assert!(claude_took(&home.0, "s-1", "id m-1"));
        assert!(!claude_took(&home.0, "s-1", "id m-2"));
    }

    /// The same for a Codex thread: its user messages count, a tool's output (also
    /// `input_text`) doesn't.
    #[test]
    fn a_codex_input_counts_and_tool_output_doesnt() {
        let home = scratch("codex");
        let path = home
            .0
            .join("sessions/2026/10/13/rollout-2026-10-13T04-42-32-t-1.jsonl");
        write_lines(
            &path,
            &[
                json!({"type": "response_item", "payload": {"type": "custom_tool_call_output",
                    "output": [{"type": "input_text", "text": HEADER}]}}),
                json!({"type": "response_item", "payload": {"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": HEADER}]}}),
            ],
        );
        assert!(!codex_took(&home.0, "t-1", "id m-1"));
        for line in [
            json!({"type": "response_item", "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": HEADER}]}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": HEADER}}),
            json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {
                "type": "UserMessage", "content": [{"type": "text", "text": HEADER}]}}}),
        ] {
            write_lines(&path, &[line]);
            assert!(codex_took(&home.0, "t-1", "id m-1"));
            assert!(!codex_took(&home.0, "t-2", "id m-1"));
        }
    }
}
