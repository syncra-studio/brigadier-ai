//! Codex's rollout (`~/.codex/sessions/…/rollout-…-<thread id>.jsonl`) as thread items. Each
//! finished item is written as `event_msg`/`item_completed` with the id the app-server streamed
//! it under, so its events are the stream's ([`super::parse`]) with the same ids. A turn's end
//! is `task_complete` or `turn_aborted`; a user message inside a running turn is a steer.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use super::parse::{declined_command, is_decline_steer};
use crate::model::{
    FileChange, FileChangeKind, ItemStatus, ProviderEvent, Role, TextPhase, TurnStatus,
};
use crate::session_file::{FileEntry, shown};

/// What a line of the rollout says of its context ([`crate::session_file::context`]): a
/// `token_count`'s last request, which the live stream reports as the context in use, and the
/// model's window.
pub(crate) fn context_of(value: &Value) -> Option<Option<crate::session_file::FileContext>> {
    if str_of(value, "type") != Some("event_msg") {
        return None;
    }
    let payload = value.get("payload")?;
    if str_of(payload, "type") != Some("token_count") {
        return None;
    }
    let info = payload.get("info").filter(|info| !info.is_null())?;
    let used = info
        .get("last_token_usage")?
        .get("total_tokens")?
        .as_i64()?;
    Some(Some(crate::session_file::FileContext {
        used_tokens: used,
        window_tokens: info.get("model_context_window").and_then(Value::as_i64),
    }))
}

/// How much of a rollout's end [`ends_read`] looks at: a turn's own lines are at its end, and
/// a poll further back than this is no proof, so its command is told as unread.
const ENDS_READ_TAIL: u64 = 4 * 1024 * 1024;

/// Those of the background commands `items` whose end the model read, as the rollout at `path`
/// proves: a script the model ran polled that command's process, and the script's output carries
/// the exit code and the end of what the command printed. Codex's stream says no such thing.
/// Whatever isn't that proof (no file, an unknown format, a line not written yet) reads as unread.
///
/// shortcut: only a poll naming its process by number in a script's `session_id:`, printing its
/// result as JSON, counts; a poll in a variable or a plain `write_stdin` call proves nothing, and
/// its model hears the end again. Upgrade if those show up live.
pub fn ends_read(path: &Path, items: &[String]) -> Vec<String> {
    let Ok(text) = tail(path, ENDS_READ_TAIL) else {
        return Vec::new();
    };
    let mut ends = HashMap::new();
    let mut polls = HashMap::new();
    let mut results = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(payload) = value.get("payload") else {
            continue;
        };
        match str_of(payload, "type") {
            Some("item_completed") => {
                let Some(item) = payload.get("item") else {
                    continue;
                };
                if str_of(item, "type") == Some("CommandExecution")
                    && str_of(item, "id").is_some_and(|id| items.iter().any(|item| item == id))
                    && let Some(process) = str_of(item, "process_id")
                {
                    ends.insert(process.to_owned(), item.clone());
                }
            }
            Some("custom_tool_call") if str_of(payload, "name") == Some("exec") => {
                if let (Some(call), Some(process)) = (
                    str_of(payload, "call_id"),
                    str_of(payload, "input").and_then(polled_process),
                ) {
                    polls.insert(call.to_owned(), process.to_owned());
                }
            }
            Some("custom_tool_call_output") => {
                if let Some(process) = str_of(payload, "call_id").and_then(|call| polls.get(call))
                    && let Some(output) = payload.get("output")
                {
                    results.push((process.clone(), output.clone()));
                }
            }
            _ => {}
        }
    }
    // A poll's output and its command's end come in either order.
    results
        .iter()
        .filter_map(|(process, output)| ends.get(process).filter(|end| shows_end(output, end)))
        .filter_map(|end| str_of(end, "id").map(str::to_owned))
        .collect()
}

/// The last `limit` bytes of the file at `path`. Its first line may come cut, so unreadable.
fn tail(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let from = file.metadata()?.len().saturating_sub(limit);
    file.seek(SeekFrom::Start(from))?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The one process a code-mode script polls, named by number: `write_stdin({session_id:123,…`.
fn polled_process(script: &str) -> Option<&str> {
    if !script.contains("write_stdin") {
        return None;
    }
    let mut named = script.split("session_id:").skip(1);
    let rest = named.next()?.trim_start();
    if named.next().is_some() {
        return None;
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    (digits > 0).then(|| &rest[..digits])
}

/// A script's `output` holds a result with the command's exit code and the end of its output.
/// Part of it may have been read by an earlier poll; none of it, of one that printed, is no proof.
fn shows_end(output: &Value, end: &Value) -> bool {
    let printed = str_of(end, "aggregated_output").unwrap_or_default();
    output.as_array().into_iter().flatten().any(|part| {
        let Some(result) =
            str_of(part, "text").and_then(|text| serde_json::from_str::<Value>(text).ok())
        else {
            return false;
        };
        result
            .get("exit_code")
            .is_some_and(|code| Some(code) == end.get("exit_code"))
            && str_of(&result, "output").is_some_and(|seen| {
                printed.ends_with(seen) && seen.is_empty() == printed.is_empty()
            })
    })
}

/// The thread the rollout at `path` holds.
pub fn read(path: &Path) -> std::io::Result<Vec<FileEntry>> {
    let text = std::fs::read_to_string(path)?;
    let mut entries = Vec::new();
    let mut previous_at_ms = None;
    // A turn runs, and whether its user message came yet.
    let mut turn: Option<bool> = None;
    // Codex can write both its native marker and a finished ContextCompaction item for
    // the same compaction. The marker alone is enough; the following item is no new divider.
    let mut compacted = false;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let mut at_ms = str_of(&value, "timestamp").and_then(crate::time::parse_rfc3339_ms);
        if str_of(&value, "type") == Some("compacted") {
            previous_at_ms = at_ms.or(previous_at_ms);
            entries.push((previous_at_ms, FileEntry::new(compaction(), at_ms)));
            compacted = true;
            continue;
        }
        if str_of(&value, "type") != Some("event_msg") {
            continue;
        }
        let Some(payload) = value.get("payload") else {
            continue;
        };
        let mut phase = None;
        let event = match str_of(payload, "type") {
            Some("task_started") => {
                turn = Some(false);
                compacted = false;
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
                if str_of(item, "type") == Some("ContextCompaction")
                    && std::mem::take(&mut compacted)
                {
                    continue;
                }
                let Some(event) = item_event(item) else {
                    continue;
                };
                at_ms = payload
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .or(at_ms);
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
        previous_at_ms = at_ms.or(previous_at_ms);
        entries.push((
            previous_at_ms,
            FileEntry {
                steer,
                phase,
                ..FileEntry::new(event, at_ms)
            },
        ));
    }
    // D44/D47: a finished item keeps its start position, as in the live stream. Turn and
    // steer state above follows file order; ties and untimed entries keep that order too.
    entries.sort_by_key(|(at_ms, _)| *at_ms);
    Ok(entries.into_iter().map(|(_, entry)| entry).collect())
}

/// A finished item as the stream's event.
fn item_event(item: &Value) -> Option<ProviderEvent> {
    let id = str_of(item, "id")?.to_owned();
    let event = match str_of(item, "type")? {
        "UserMessage" => {
            let text = texts(item.get("content")?, "text").join("\n");
            // Brigadier's word on a decline is no message of the user's: Codex keeps no item
            // of a declined command, so the word stands for it, where Codex took it (#560).
            if let Some((item_id, command)) = declined_command(&text) {
                return Some(ProviderEvent::Command {
                    item_id,
                    command,
                    cwd: None,
                    status: ItemStatus::Declined,
                    exit_code: None,
                    output: None,
                    duration_ms: None,
                    kept: None,
                });
            }
            if is_decline_steer(&text) {
                return None;
            }
            ProviderEvent::Message {
                item_id: id,
                role: Role::User,
                text,
            }
        }
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
        // A sub-agent's start and end: the row the live session shows (D56).
        "SubAgentActivity" => {
            let status = match str_of(item, "kind")? {
                "started" => ItemStatus::InProgress,
                "completed" => ItemStatus::Completed,
                "interrupted" => ItemStatus::Failed,
                _ => return None,
            };
            super::parse::agent_row(
                str_of(item, "agent_thread_id")?.to_owned(),
                super::parse::agent_name(str_of(item, "agent_path")?),
                status,
            )
        }
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
        "ContextCompaction" => compaction(),
        _ => return None,
    };
    Some(event)
}

fn compaction() -> ProviderEvent {
    ProviderEvent::CompactionEnded {
        automatic: true,
        tokens_before: None,
        tokens_after: None,
        error: None,
        native_id: None,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A command's end, a poll's script and its output, each as the rollout records them.
    fn end_line(id: &str, process: &str, printed: &str) -> String {
        serde_json::json!({"type": "event_msg", "payload": {"type": "item_completed", "item": {
            "type": "CommandExecution", "id": id, "process_id": process, "exit_code": 0,
            "aggregated_output": printed
        }}})
        .to_string()
    }

    fn poll_lines(call: &str, script: &str, result: &str) -> [String; 2] {
        [
            serde_json::json!({"type": "response_item", "payload": {
                "type": "custom_tool_call", "name": "exec", "call_id": call, "input": script
            }})
            .to_string(),
            serde_json::json!({"type": "response_item", "payload": {
                "type": "custom_tool_call_output", "call_id": call, "output": [
                    {"type": "input_text", "text": "Script completed\nOutput:\n"},
                    {"type": "input_text", "text": result}
                ]
            }})
            .to_string(),
        ]
    }

    fn ends_read_of(name: &str, lines: &[String], items: &[&str]) -> Vec<String> {
        let path = std::env::temp_dir().join(format!(
            "brigadier-ends-read-{name}-{}.jsonl",
            std::process::id()
        ));
        std::fs::write(&path, lines.join("\n")).unwrap();
        let items: Vec<String> = items.iter().map(|item| (*item).to_owned()).collect();
        let read = ends_read(&path, &items);
        std::fs::remove_file(&path).unwrap();
        read
    }

    const POLL_1: &str = "text(await tools.write_stdin({session_id:111,chars:\"\"}));";
    const ENDED: &str = r#"{"exit_code":0,"output":"one\n"}"#;

    /// Two commands in a turn: a poll that read the first's end proves nothing of the second's,
    /// though both ended, and whichever way round its end and output are recorded.
    #[test]
    fn a_polled_end_proves_only_its_own_command() {
        let [call, output] = poll_lines("c1", POLL_1, ENDED);
        for lines in [
            [
                end_line("a", "111", "one\n"),
                end_line("b", "222", "one\n"),
                call.clone(),
                output.clone(),
            ],
            [
                call.clone(),
                output.clone(),
                end_line("a", "111", "one\n"),
                end_line("b", "222", "one\n"),
            ],
        ] {
            assert_eq!(ends_read_of("own", &lines, &["a", "b"]), ["a"]);
        }
    }

    #[test]
    fn a_poll_that_saw_it_running_proves_nothing() {
        let [call, output] = poll_lines("c1", POLL_1, r#"{"session_id":111,"output":""}"#);
        let lines = [call, output, end_line("a", "111", "one\n")];
        assert!(ends_read_of("running", &lines, &["a"]).is_empty());
    }

    /// The answer's own words can't pass for the proof, nor another command's result for its.
    #[test]
    fn only_a_result_of_this_commands_end_is_proof() {
        for (script, result) in [
            (POLL_1, r#"{"exit_code":0,"output":"other\n"}"#),
            (POLL_1, r#"{"exit_code":3,"output":"one\n"}"#),
            (POLL_1, r#"{"exit_code":0,"output":""}"#),
            (POLL_1, r#""exit_code":0"#),
            ("text(await tools.exec_command({cmd:\"echo one\"}));", ENDED),
            (
                "for (const s of [1]) await tools.write_stdin({session_id:s});",
                ENDED,
            ),
            (
                "await tools.write_stdin({session_id:111});await tools.write_stdin({session_id:222});",
                ENDED,
            ),
        ] {
            let [call, output] = poll_lines("c1", script, result);
            let lines = [call, output, end_line("a", "111", "one\n")];
            assert!(
                ends_read_of("other", &lines, &["a"]).is_empty(),
                "{script} {result}"
            );
        }
    }

    #[test]
    fn a_missing_or_foreign_file_proves_nothing() {
        let items = ["a".to_owned()];
        assert!(ends_read(Path::new("/nonexistent/rollout.jsonl"), &items).is_empty());
        assert!(ends_read_of("foreign", &["not json".into(), "{}".into()], &["a"]).is_empty());
    }

    /// A poll further back than the read reaches is no proof, and the read stays that small.
    #[test]
    fn only_the_files_end_is_read() {
        let [call, output] = poll_lines("c1", POLL_1, ENDED);
        let path = std::env::temp_dir().join(format!(
            "brigadier-ends-read-tail-{}.jsonl",
            std::process::id()
        ));
        let filler = "x".repeat(usize::try_from(ENDS_READ_TAIL).unwrap());
        let lines = [end_line("a", "111", "one\n"), call, output, filler];
        std::fs::write(&path, lines.join("\n")).unwrap();
        assert!(ends_read(&path, &["a".to_owned()]).is_empty());
        std::fs::remove_file(&path).unwrap();
    }

    /// Codex keeps no item of a declined command; Brigadier's word on it stands for it, in
    /// its place, and is no message of the user's (#560). An older word names no command and
    /// shows nothing.
    #[test]
    fn a_declined_command_reads_where_codex_took_the_decline() {
        let line = |at: &str, payload: serde_json::Value| {
            serde_json::json!({"timestamp": at, "type": "event_msg", "payload": payload})
                .to_string()
        };
        let item = |item: serde_json::Value| {
            serde_json::json!({"type": "item_completed", "thread_id": "t", "turn_id": "u",
                "item": item})
        };
        let user = |id: &str, text: &str| {
            item(serde_json::json!({"type": "UserMessage", "id": id,
                "content": [{"type": "text", "text": text, "text_elements": []}]}))
        };
        let agent = |id: &str, text: &str, phase: &str| {
            item(serde_json::json!({"type": "AgentMessage", "id": id,
                "content": [{"type": "Text", "text": text}], "phase": phase}))
        };
        let steer = super::super::parse::decline_steer(
            Some(("exec-1", "/bin/zsh -lc \"curl -sI x\"")),
            "The user declined this.",
        );
        let text = [
            line(
                "2026-10-23T16:02:44.000Z",
                serde_json::json!({"type": "task_started", "turn_id": "u"}),
            ),
            line("2026-10-23T16:02:46.000Z", user("m1", "Run curl.")),
            line(
                "2026-10-23T16:02:48.000Z",
                agent("a1", "Running it.", "commentary"),
            ),
            line("2026-10-23T16:02:50.000Z", user("m2", &steer)),
            line(
                "2026-10-23T16:02:51.000Z",
                user("m3", "[Declined] Older word."),
            ),
            line(
                "2026-10-23T16:02:52.000Z",
                agent("a2", "I didn't run it.", "final_answer"),
            ),
            line(
                "2026-10-23T16:02:53.000Z",
                serde_json::json!({"type": "task_complete", "turn_id": "u"}),
            ),
        ]
        .join("\n");
        let path = std::env::temp_dir().join(format!(
            "brigadier-rollout-declined-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, text).unwrap();
        let entries = read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let kinds: Vec<String> = entries
            .iter()
            .map(|entry| match &entry.event {
                ProviderEvent::Message { role, item_id, .. } => format!("{role:?} {item_id}"),
                ProviderEvent::Command {
                    item_id,
                    status,
                    command,
                    ..
                } => format!("command {item_id} {status:?} {command}"),
                ProviderEvent::TurnCompleted { .. } => "end".into(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "User m1",
                "Assistant a1",
                "command exec-1 Declined /bin/zsh -lc \"curl -sI x\"",
                "Assistant a2",
                "end"
            ]
        );
        assert!(!entries[2].steer);
    }

    /// D44/D47: the recorded commands finish after the answer or the turn's interruption,
    /// but reload keeps them where the live stream started them, with their own start times.
    #[test]
    fn recorded_rollout_items_keep_their_start_order() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/versions/codex-0.164.0/lifecycle.native.jsonl");
        let entries = read(&path).unwrap();
        let command = |text: &str| {
            entries.iter().position(|entry| {
                matches!(&entry.event, ProviderEvent::Command { command, .. } if command == text)
            }).unwrap()
        };
        let sleep = command("sleep 2");
        let answer = entries
            .iter()
            .position(
                |entry| matches!(&entry.event, ProviderEvent::Message { text, .. } if text == "OK"),
            )
            .unwrap();
        assert!(sleep < answer);
        let interrupted = entries
            .iter()
            .position(|entry| {
                matches!(
                    &entry.event,
                    ProviderEvent::TurnCompleted {
                        status: TurnStatus::Interrupted,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(command("sleep 30") < interrupted);
        let text = std::fs::read_to_string(path).unwrap();
        let mut checked = 0;
        for line in text.lines() {
            let value: Value = serde_json::from_str(line).unwrap();
            let Some(payload) = value.get("payload") else {
                continue;
            };
            if str_of(payload, "type") != Some("item_completed") {
                continue;
            }
            let Some(id) = payload.pointer("/item/id").and_then(Value::as_str) else {
                continue;
            };
            if let Some(entry) = entries
                .iter()
                .find(|entry| crate::session_file::item_id(&entry.event) == Some(id))
            {
                assert_eq!(
                    entry.at_ms,
                    payload.get("started_at_ms").and_then(Value::as_i64)
                );
                checked += 1;
            }
        }
        assert_eq!(checked, 8);
        assert!(
            entries
                .windows(2)
                .all(|pair| pair[0].at_ms <= pair[1].at_ms)
        );
    }

    /// A turn the Stop hook sent back once (harness D49), as codex-cli 0.164.0 wrote it: the
    /// send-back is a `HookPrompt` item and a user-role response item, and neither reads as a
    /// message of the user's.
    #[test]
    fn a_stop_hooks_send_back_reads_as_no_user_message() {
        let completed = |at: i64, item: Value| {
            serde_json::json!({"type": "event_msg", "payload": {
                "type": "item_completed", "started_at_ms": at, "item": item}})
        };
        let prompt = "<hook_prompt hook_run_id=\"stop:0:/&lt;session-flags&gt;/config.toml\">Say the word PINEAPPLE and stop.</hook_prompt>";
        let lines = [
            serde_json::json!({"type": "event_msg", "payload": {"type": "task_started"}}),
            completed(
                1,
                serde_json::json!({"type": "UserMessage", "id": "u1",
                "content": [{"type": "text", "text": "Reply with just: hello"}]}),
            ),
            completed(
                2,
                serde_json::json!({"type": "AgentMessage", "id": "a1",
                "content": [{"type": "Text", "text": "hello"}], "phase": "final_answer"}),
            ),
            serde_json::json!({"type": "response_item", "payload": {"type": "message",
                "id": "msg_1", "role": "user",
                "content": [{"type": "input_text", "text": prompt}]}}),
            completed(
                3,
                serde_json::json!({"type": "HookPrompt", "id": "msg_1",
                "fragments": [{"text": "Say the word PINEAPPLE and stop.",
                    "hookRunId": "stop:0:/<session-flags>/config.toml"}]}),
            ),
            completed(
                4,
                serde_json::json!({"type": "AgentMessage", "id": "a2",
                "content": [{"type": "Text", "text": "PINEAPPLE"}], "phase": "final_answer"}),
            ),
        ];
        let path = std::env::temp_dir().join(format!(
            "20261025-1046-harness-research-w30-rollout-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let entries = read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        let messages: Vec<(Role, &str)> = entries
            .iter()
            .filter_map(|entry| match &entry.event {
                ProviderEvent::Message { role, text, .. } => Some((*role, text.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            [
                (Role::User, "Reply with just: hello"),
                (Role::Assistant, "hello"),
                (Role::Assistant, "PINEAPPLE"),
            ]
        );
    }

    #[test]
    fn equal_and_missing_times_keep_file_order_and_steers_follow_file_state() {
        let user = |id: &str, start: Option<i64>| {
            serde_json::json!({"type": "event_msg", "payload": {
                "type": "item_completed", "started_at_ms": start,
                "item": {"type": "UserMessage", "id": id,
                    "content": [{"type": "text", "text": id}]}
            }})
        };
        // Deliberately inverted starts: classification uses file order, even after sorting.
        let lines = [
            serde_json::json!({"type": "event_msg", "payload": {"type": "task_started"}}),
            user("first", Some(30)),
            user("steer", Some(10)),
            user("untimed", None),
            user("equal", Some(10)),
        ];
        let path = std::env::temp_dir().join(format!(
            "20261025-1046-harness-research-w25-rollout-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let entries = read(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| crate::session_file::item_id(&entry.event).unwrap())
                .collect::<Vec<_>>(),
            ["steer", "untimed", "equal", "first"]
        );
        assert!(entries[..3].iter().all(|entry| entry.steer));
        assert!(!entries[3].steer);
        assert_eq!(entries[1].at_ms, None);
    }

    #[test]
    fn native_compaction_and_its_finished_item_make_one_divider_in_rollout_order() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/session-files/codex-compacted.jsonl");
        let entries = read(&path).unwrap();
        let compacted: Vec<_> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| matches!(entry.event, ProviderEvent::CompactionEnded { .. }))
            .collect();
        // Native marker + item, native marker alone, and item alone; none is lost or doubled.
        assert_eq!(
            compacted.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
            [1, 3, 5]
        );
        assert_eq!(
            compacted[0].1.at_ms,
            crate::time::parse_rfc3339_ms("2026-10-23T08:11:27.390Z")
        );
        assert!(matches!(
            &entries[0].event,
            ProviderEvent::Message {
                role: Role::User,
                ..
            }
        ));
        assert!(matches!(
            &entries[2].event,
            ProviderEvent::Message {
                role: Role::Assistant,
                ..
            }
        ));
        assert!(matches!(
            &entries[6].event,
            ProviderEvent::TurnCompleted { .. }
        ));
        for (_, entry) in compacted {
            assert!(matches!(
                entry.event,
                ProviderEvent::CompactionEnded {
                    automatic: true,
                    error: None,
                    ..
                }
            ));
        }
    }
}
