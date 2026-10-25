//! `brigadierd hook post-tool-use`: a Claude thread's output hook (THREAD-PLAN.md Q4).
//!
//! The thread's CLI runs it after every `Bash` call (`PostToolUse` and `PostToolUseFailure`,
//! from Brigadier's `--settings`), with the call's event on stdin
//! (docs/evidence/2026-10-07-thread-phase2-contracts.md §1–3):
//!
//! - **Success** over [`BASH_WHOLE_MAX`] bytes: the whole output (the CLI's own copy at
//!   `tool_response.persistedOutputPath` when it made one, else `stdout` and `stderr`) goes to
//!   the daemon, which stores it and answers with its digest. The hook prints the same
//!   `tool_response` object with `stdout` replaced by the digest, `stderr` emptied and the
//!   `persisted*` fields left out, as `hookSpecificOutput.updatedToolOutput`; the CLI then
//!   gives the model exactly that. Shorter outputs: nothing printed, nothing stored.
//! - **Failure**: the event holds only the CLI's excerpt of the output (`error`), which the
//!   daemon stores as such. Nothing is printed, so the model gets the excerpt untrimmed; a
//!   failure's hook can't replace the output anyway.
//!
//! The grant (in [`HOOK_GRANT_ENV`], from the CLI's environment) can only store this thread's
//! output. Whatever goes wrong, the hook prints nothing and exits 0: the tool call never
//! breaks. No async runtime and no logging, so it starts at once.
//!
//! `brigadierd hook stop` is a thread's `Stop` hook, Claude's and Codex's (harness D43, D45,
//! D49; Codex sends the same event fields and takes the same answer): when the turn's
//! last reply ends with a round of questions written as text ([`crate::text_round`]), it sends
//! the turn back once with [`STOP_REASON`], and the model asks the round on the question card
//! itself; Brigadier rewrites nothing. A stop that is already such a send-back
//! (`stop_hook_active`) always ends, so never twice in one turn. No daemon, no grant.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use brigadier_core::digest::BASH_WHOLE_MAX;
use brigadier_core::manager::{HOOK_GRANT_ENV, OUTPUT_MAX_BYTES};
use brigadier_ipc::protocol::{ClientFrame, HookOutput, HookReply};
use brigadier_sandbox::PlatformOptions;
use serde_json::{Map, Value, json};

/// What the thread is told when its turn ends with a round of questions written as text.
const STOP_REASON: &str =
    "Put these questions in your question tool (or brigadier ask) instead of the reply.";

/// What the hook does with one event.
#[derive(Debug, PartialEq)]
enum Plan {
    /// A successful call's whole output, long enough to trim; `response` is its
    /// `tool_response`, which the reply replaces.
    Trim {
        output: Vec<u8>,
        status: String,
        response: Map<String, Value>,
    },
    /// A failing call's excerpt, stored as it is.
    Store { excerpt: Vec<u8>, status: String },
}

/// Runs the hook with the arguments after `hook`.
pub fn run(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    // Every path ends here with nothing printed: never break the tool call.
    match args.next() {
        Some(event) if event == "post-tool-use" => {}
        Some(event) if event == "stop" => {
            let mut stdin = String::new();
            if std::io::stdin().read_to_string(&mut stdin).is_ok()
                && let Some(printed) = stop(&stdin)
            {
                let mut stdout = std::io::stdout().lock();
                let _ = stdout.write_all(printed.as_bytes());
                let _ = stdout.flush();
            }
            return ExitCode::SUCCESS;
        }
        _ => return ExitCode::SUCCESS,
    }
    let mut data_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            data_dir = args.next().map(PathBuf::from);
        }
    }
    let mut stdin = String::new();
    if std::io::stdin().read_to_string(&mut stdin).is_err() {
        return ExitCode::SUCCESS;
    }
    let grant = std::env::var(HOOK_GRANT_ENV).ok();
    let printed = handle(&stdin, |output, bytes| {
        send(grant.as_deref(), data_dir.clone(), output, bytes)
    });
    if let Some(printed) = printed {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(printed.as_bytes());
        let _ = stdout.flush();
    }
    ExitCode::SUCCESS
}

/// What the hook prints for the event `stdin`, given a way to reach the daemon; `None`: nothing.
fn handle(
    stdin: &str,
    mut send: impl FnMut(&HookOutput, &[u8]) -> Option<HookReply>,
) -> Option<String> {
    let event: Value = serde_json::from_str(stdin).ok()?;
    match plan(&event)? {
        Plan::Trim {
            output,
            status,
            mut response,
        } => {
            let header = HookOutput {
                excerpt: false,
                status,
                bytes: output.len() as u64,
            };
            let replacement = send(&header, &output)?.replacement?;
            response.insert("stdout".into(), Value::String(replacement));
            response.insert("stderr".into(), Value::String(String::new()));
            // Kept, the CLI would wrap the replacement in its own persisted-output preview.
            response.remove("persistedOutputPath");
            response.remove("persistedOutputSize");
            Some(
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PostToolUse",
                        "updatedToolOutput": response,
                    }
                })
                .to_string(),
            )
        }
        Plan::Store { excerpt, status } => {
            let header = HookOutput {
                excerpt: true,
                status,
                bytes: excerpt.len() as u64,
            };
            send(&header, &excerpt);
            None
        }
    }
}

/// What the `Stop` hook prints for the event `stdin`: a send-back when the reply ends with a
/// round of questions written as text; `None`: the turn ends.
fn stop(stdin: &str) -> Option<String> {
    let event: Value = serde_json::from_str(stdin).ok()?;
    if event.get("stop_hook_active").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let reply = event.get("last_assistant_message")?.as_str()?;
    crate::text_round::is_text_round(reply)
        .then(|| json!({ "decision": "block", "reason": STOP_REASON }).to_string())
}

fn plan(event: &Value) -> Option<Plan> {
    if event.get("tool_name").and_then(Value::as_str)? != "Bash" {
        return None;
    }
    match event.get("hook_event_name").and_then(Value::as_str)? {
        "PostToolUse" => {
            let response = event.get("tool_response")?.as_object()?.clone();
            let output = whole_output(&response)?;
            if output.len() <= BASH_WHOLE_MAX {
                return None;
            }
            // A success has no exit code: exit 0, or a benign non-zero exit the CLI
            // interprets ("No matches found").
            let status = if response.get("interrupted") == Some(&Value::Bool(true)) {
                "interrupted".to_owned()
            } else {
                response
                    .get("returnCodeInterpretation")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
                    .unwrap_or("exit 0")
                    .to_owned()
            };
            Some(Plan::Trim {
                output,
                status,
                response,
            })
        }
        "PostToolUseFailure" => {
            let error = event.get("error")?.as_str()?;
            let status = if event.get("is_interrupt") == Some(&Value::Bool(true)) {
                "interrupted".to_owned()
            } else {
                // "Exit code 3\n…"
                error
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("Exit code "))
                    .map_or_else(
                        || "failed".to_owned(),
                        |code| format!("exit {}", code.trim()),
                    )
            };
            Some(Plan::Store {
                excerpt: capped(error.as_bytes().to_vec(), error.len() as u64),
                status,
            })
        }
        _ => None,
    }
}

/// A successful call's whole output: the CLI's own copy of a long one, else what it read back.
fn whole_output(response: &Map<String, Value>) -> Option<Vec<u8>> {
    if let Some(path) = response.get("persistedOutputPath").and_then(Value::as_str) {
        let file = std::fs::File::open(path).ok()?;
        let size = file.metadata().ok()?.len();
        let mut bytes = Vec::new();
        file.take(OUTPUT_MAX_BYTES as u64)
            .read_to_end(&mut bytes)
            .ok()?;
        return Some(capped(bytes, size));
    }
    let text = |key: &str| {
        response
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    let (stdout, stderr) = (text("stdout"), text("stderr"));
    let mut output = stdout.as_bytes().to_vec();
    if !stderr.is_empty() {
        if !output.is_empty() && !output.ends_with(b"\n") {
            output.push(b'\n');
        }
        output.extend_from_slice(stderr.as_bytes());
    }
    let size = output.len() as u64;
    Some(capped(output, size))
}

/// At most [`OUTPUT_MAX_BYTES`] of an output of `size` bytes, saying what was left out.
fn capped(mut bytes: Vec<u8>, size: u64) -> Vec<u8> {
    if bytes.len() > OUTPUT_MAX_BYTES {
        bytes.truncate(OUTPUT_MAX_BYTES);
    }
    let left = size.saturating_sub(bytes.len() as u64);
    if left > 0 {
        bytes.extend_from_slice(format!("\n[… {left} more bytes not kept]\n").as_bytes());
    }
    bytes
}

/// Sends `output` to the daemon for `data_dir` with the grant from the environment, and reads
/// its answer; `None` on any failure.
fn send(
    grant: Option<&str>,
    data_dir: Option<PathBuf>,
    header: &HookOutput,
    output: &[u8],
) -> Option<HookReply> {
    let grant = grant.filter(|grant| !grant.is_empty())?.to_owned();
    let platform = brigadier_sandbox::native(PlatformOptions { data_dir }).ok()?;
    let mut stream = brigadier_ipc::connect_blocking(
        platform.paths(),
        &ClientFrame::Hook {
            grant,
            output: header.clone(),
        },
    )
    .ok()?;
    stream.write_all(output).ok()?;
    stream.flush().ok()?;
    brigadier_ipc::read_frame_blocking::<HookReply>(&mut stream)
        .ok()
        .flatten()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn success(response: Value) -> String {
        json!({
            "session_id": "s",
            "hook_event_name": "PostToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" },
            "tool_use_id": "t",
            "tool_response": response,
        })
        .to_string()
    }

    fn digest_reply(
        sent: &std::cell::RefCell<Option<(HookOutput, Vec<u8>)>>,
    ) -> impl FnMut(&HookOutput, &[u8]) -> Option<HookReply> + '_ {
        move |header, bytes| {
            *sent.borrow_mut() = Some((header.clone(), bytes.to_vec()));
            Some(HookReply {
                replacement: Some("exit 0 [full output: out-1 in /data/blobs/ab/abcd, …]".into()),
            })
        }
    }

    #[test]
    fn a_long_success_is_replaced_by_its_digest_in_the_same_shape() {
        let stdout = "ok\n".repeat(12_000);
        let sent = std::cell::RefCell::default();
        let printed = handle(
            &success(json!({
                "stdout": stdout,
                "stderr": "warning: slow\n",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false,
            })),
            digest_reply(&sent),
        )
        .expect("a replacement");
        let (header, bytes) = sent.take().expect("sent");
        assert_eq!(header.status, "exit 0");
        assert!(!header.excerpt);
        assert_eq!(bytes, format!("{stdout}warning: slow\n").into_bytes());
        assert_eq!(header.bytes, bytes.len() as u64);
        let printed: Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(
            printed,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "updatedToolOutput": {
                        "stdout": "exit 0 [full output: out-1 in /data/blobs/ab/abcd, …]",
                        "stderr": "",
                        "interrupted": false,
                        "isImage": false,
                        "noOutputExpected": false,
                    }
                }
            })
        );
    }

    #[test]
    fn the_clis_own_copy_of_a_long_output_is_read_whole() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-hook-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let file = dir.join("b1.txt");
        let whole: String = (1..=20_000).map(|n| format!("{n}\n")).collect();
        std::fs::write(&file, &whole).unwrap();
        let sent = std::cell::RefCell::default();
        let printed = handle(
            &success(json!({
                "stdout": &whole[..30_000],
                "stderr": "",
                "interrupted": false,
                "isImage": false,
                "noOutputExpected": false,
                "returnCodeInterpretation": "No matches found",
                "persistedOutputPath": file.display().to_string(),
                "persistedOutputSize": whole.len(),
            })),
            digest_reply(&sent),
        )
        .expect("a replacement");
        let (header, bytes) = sent.take().unwrap();
        assert_eq!(
            bytes,
            whole.as_bytes(),
            "the whole output, not the read-back window"
        );
        assert_eq!(header.status, "No matches found");
        let printed: Value = serde_json::from_str(&printed).unwrap();
        let replaced = &printed["hookSpecificOutput"]["updatedToolOutput"];
        assert!(replaced.get("persistedOutputPath").is_none());
        assert!(replaced.get("persistedOutputSize").is_none());
        assert_eq!(replaced["returnCodeInterpretation"], "No matches found");
    }

    /// A `Bash` call goes to the daemon before it runs and once it ended (a failure too), with
    /// its command and folder, and the hook prints nothing for it; a call sent to the
    /// background only before it runs, marked so, and another tool nothing (#87).
    #[test]
    fn a_short_success_is_left_alone_without_reaching_the_daemon() {
        let printed = handle(
            &success(json!({ "stdout": "x".repeat(BASH_WHOLE_MAX), "stderr": "" })),
            |_, _| panic!("nothing to store"),
        );
        assert_eq!(printed, None);
    }

    #[test]
    fn a_failure_is_stored_as_the_clis_excerpt_and_reaches_the_model_untrimmed() {
        let error = format!("Exit code 3\n{}", "error: boom\n".repeat(900));
        let event = json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Bash",
            "tool_input": { "command": "make" },
            "error": error,
            "is_interrupt": false,
        });
        let sent = std::cell::RefCell::default();
        let printed = handle(&event.to_string(), digest_reply(&sent));
        assert_eq!(printed, None, "a failure's output is never replaced");
        let (header, bytes) = sent.take().expect("stored");
        assert!(header.excerpt);
        assert_eq!(header.status, "exit 3");
        assert_eq!(bytes, error.as_bytes());
    }

    /// A round written as text is sent back once; the send-back's own stop, and an ordinary
    /// reply, end the turn (harness D43, D45).
    #[test]
    fn a_text_round_is_sent_back_once_per_turn() {
        let event = |reply: &str, active: bool| {
            json!({
                "session_id": "s",
                "hook_event_name": "Stop",
                "stop_hook_active": active,
                "last_assistant_message": reply,
            })
            .to_string()
        };
        let round = "1. Which branch should this land on?\n2. Keep the old flag?\n";
        let printed: Value =
            serde_json::from_str(&stop(&event(round, false)).expect("sent back")).unwrap();
        assert_eq!(
            printed,
            json!({ "decision": "block", "reason": STOP_REASON })
        );
        assert_eq!(stop(&event(round, true)), None, "never twice in one turn");
        assert_eq!(stop(&event("Done. Want me to push it?", false)), None);
        assert_eq!(
            stop(r#"{"hook_event_name":"Stop","stop_hook_active":false}"#),
            None
        );
        assert_eq!(stop("not json"), None);
    }

    /// Codex's Stop event, as codex-cli 0.164.0 sent it to a trusted hook (harness D49): the
    /// same two fields the check reads, and Codex takes the same `decision: "block"` answer.
    #[test]
    fn codex_s_stop_event_gets_the_same_send_back() {
        let event = |reply: &str, active: bool| {
            json!({
                "session_id": "01a1739e-c970-7652-ab58-ebd836dac836",
                "turn_id": "01a1739e-cb5a-7070-8257-bf866b1e7fcd",
                "transcript_path": "/Users/me/.codex/sessions/2026/10/25/rollout-2026-10-25T17-12-28-01a1739e-c970-7652-ab58-ebd836dac836.jsonl",
                "cwd": "/Users/me/code/app",
                "hook_event_name": "Stop",
                "model": "gpt-5.6-luna",
                "permission_mode": "bypassPermissions",
                "stop_hook_active": active,
                "last_assistant_message": reply,
            })
            .to_string()
        };
        let round = "Before I start:\n\n1. Which branch should this land on?\n2. Keep the old flag?\n3. Ship it today?\n";
        let printed: Value =
            serde_json::from_str(&stop(&event(round, false)).expect("sent back")).unwrap();
        assert_eq!(
            printed,
            json!({ "decision": "block", "reason": STOP_REASON })
        );
        assert_eq!(stop(&event(round, true)), None, "never twice in one turn");
        assert_eq!(stop(&event("hello", false)), None);
        // Codex sends a null when the turn ended without a reply.
        let none = json!({ "hook_event_name": "Stop", "stop_hook_active": false, "last_assistant_message": null });
        assert_eq!(stop(&none.to_string()), None);
    }

    #[test]
    fn an_unreachable_daemon_or_another_tool_prints_nothing() {
        let long = success(json!({ "stdout": "y\n".repeat(20_000), "stderr": "" }));
        assert_eq!(handle(&long, |_, _| None), None);
        assert_eq!(
            handle(&long, |_, _| Some(HookReply { replacement: None })),
            None
        );
        let read = json!({
            "hook_event_name": "PostToolUse",
            "tool_name": "Read",
            "tool_response": { "stdout": "z".repeat(20_000) },
        });
        assert_eq!(handle(&read.to_string(), |_, _| panic!("not Bash")), None);
        assert_eq!(handle("not json", |_, _| panic!("no event")), None);
        // No grant (or an empty one): nothing is sent, nothing printed.
        let header = HookOutput {
            excerpt: false,
            status: String::new(),
            bytes: 0,
        };
        assert_eq!(send(None, None, &header, b""), None);
        assert_eq!(send(Some(""), None, &header, b""), None);
    }
}
