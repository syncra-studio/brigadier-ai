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

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use brigadier_core::digest::BASH_WHOLE_MAX;
use brigadier_core::manager::{HOOK_GRANT_ENV, OUTPUT_MAX_BYTES};
use brigadier_ipc::protocol::{ClientFrame, HookOutput, HookReply};
use brigadier_sandbox::PlatformOptions;
use serde_json::{Map, Value, json};

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
        Some(event) if event == "restore" => return restore(args),
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

/// `hook restore --record <dir> --slot <k>`: a long session's `SessionStart(compact)` hook
/// prints part `k` of its decisions, read from its record as they stand, for Claude to add to
/// the compacted context (run2-D8). Nothing for a slot with no part; never fails the session.
fn restore(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let (mut record, mut slot) = (None, None);
    while let Some(arg) = args.next() {
        if arg == "--record" {
            record = args.next().map(PathBuf::from);
        } else if arg == "--slot" {
            slot = args.next().and_then(|n| n.to_str()?.parse::<usize>().ok());
        }
    }
    let mut stdin = String::new();
    let _ = std::io::stdin().read_to_string(&mut stdin);
    if let Some(printed) = record
        .zip(slot)
        .and_then(|(record, slot)| restore_output(&record, slot))
    {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(printed.as_bytes());
        let _ = stdout.flush();
    }
    ExitCode::SUCCESS
}

/// What `hook restore` prints for part `slot` of the decisions in session record `record`.
fn restore_output(record: &std::path::Path, slot: usize) -> Option<String> {
    let part = brigadier_core::manager::restore_part(record, slot)?;
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": part,
            }
        })
        .to_string(),
    )
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

    /// A long session's put-back hook prints its slot's part of the ledger's active entries,
    /// as Claude takes a `SessionStart` hook's added context, and nothing for a slot past them.
    #[test]
    fn the_restore_hook_prints_its_part_of_the_decisions() {
        let record =
            Temp(std::env::temp_dir().join(format!("hook-restore-{}", std::process::id())));
        std::fs::create_dir_all(&*record).unwrap();
        std::fs::write(
            record.join("ledger.md"),
            "- L1 · 10-24 14:42 · gen 1 · Decision: store it in SQLite\n\
             - L2 · 10-24 14:43 · gen 1 · Rule: never push to main\n\
             - L3 · 10-24 14:44 · gen 1 · Note: the build took 4 minutes\n\
             - L4 · 10-24 14:45 · gen 1 · Open: the flaky login test\n",
        )
        .unwrap();
        let printed: Value = serde_json::from_str(&restore_output(&record, 1).unwrap()).unwrap();
        assert_eq!(
            printed["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        let part = printed["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(
            part.starts_with("[Brigadier · decisions restored after compaction · part 1 of 1]")
        );
        let at = |needle: &str| part.find(needle).unwrap_or_else(|| panic!("{needle}"));
        // Open items first, then rules and decisions, each whole; the rest stays out.
        assert!(at("L4 · ") < at("L1 · ") && at("L1 · ") < at("L2 · "));
        assert!(part.contains("- L1 · 10-24 14:42 · gen 1 · Decision: store it in SQLite"));
        assert!(!part.contains("L3 · "));
        assert!(part.contains("brigadier brain search"));
        assert_eq!(restore_output(&record, 2), None);
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
