//! `brigadierd hook post-tool-use`: a Claude thread's output hook (THREAD-PLAN.md Q4); and
//! `brigadierd hook stop`, a Claude worker's `Stop` hook (BUILD-PLAN E2), at the end.
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
//! - **A `Bash` call's start and end** (`PreToolUse`, from the same settings, and both of the
//!   above): the call goes to the daemon, which reads the files its command names before it
//!   runs and once it ended, so Undo can tell what the call changed (#87). Nothing is printed
//!   for it, and the call runs as it would without the hook. A call sent to the background is
//!   left out: it ends after its hook.
//! - **A file tool's success** (`Read`, `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, run by
//!   a second `PostToolUse` entry): the touched file's path goes to the daemon, which answers
//!   with the rules of its folders the thread hasn't had yet (a subfolder's `CLAUDE.md` or
//!   `AGENTS.md`, left out of its instructions); the hook prints them as
//!   `hookSpecificOutput.additionalContext`, which follows the call's result (BUILD-PLAN D1.6).
//!
//! The grant (in [`HOOK_GRANT_ENV`], from the CLI's environment) can only store this thread's
//! output. Whatever goes wrong, the hook prints nothing and exits 0: the tool call never
//! breaks. Like `brigadierd mcp`, no async runtime and no logging, so it starts at once.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use brigadier_core::digest::BASH_WHOLE_MAX;
use brigadier_core::manager::{HOOK_GRANT_ENV, OUTPUT_MAX_BYTES, STOP_GRANT_ENV};
use brigadier_ipc::protocol::{ClientFrame, HookOutput, HookReply, ShellCall, StopHookReply};
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
    /// A file a `Read`, `Edit` or `Write` call touched: the rules of its folders the thread
    /// hasn't had yet follow the call's result (BUILD-PLAN D1.6).
    Rules { path: String },
}

/// Runs the hook with the arguments after `hook`.
pub fn run(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    // Every path ends here with nothing printed: never break the tool call.
    let stop = match args.next() {
        Some(event) if event == "post-tool-use" => false,
        Some(event) if event == "stop" => true,
        _ => return ExitCode::SUCCESS,
    };
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
    if stop {
        let grant = std::env::var(STOP_GRANT_ENV).ok();
        if let Some(printed) = stop_answer(&stdin, |active| {
            ask_stop(grant.as_deref(), data_dir, active)
        }) {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(printed.as_bytes());
            let _ = stdout.flush();
        }
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
    if let Some(shell) = shell_call(&event) {
        let header = HookOutput {
            excerpt: false,
            status: String::new(),
            bytes: 0,
            touched: None,
            shell: Some(shell),
        };
        send(&header, &[]);
    }
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
                touched: None,
                shell: None,
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
                touched: None,
                shell: None,
            };
            send(&header, &excerpt);
            None
        }
        Plan::Rules { path } => {
            let header = HookOutput {
                excerpt: false,
                status: String::new(),
                bytes: 0,
                touched: Some(path),
                shell: None,
            };
            let context = send(&header, &[])?.context?;
            Some(
                json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PostToolUse",
                        "additionalContext": context,
                    }
                })
                .to_string(),
            )
        }
    }
}

/// A `Bash` call about to run (`PreToolUse`) or just ended, as the daemon reads its files for
/// it; `None` for any other event, and for a call sent to the background.
fn shell_call(event: &Value) -> Option<ShellCall> {
    if event.get("tool_name").and_then(Value::as_str)? != "Bash" {
        return None;
    }
    let ended = match event.get("hook_event_name").and_then(Value::as_str)? {
        "PreToolUse" => false,
        "PostToolUse" | "PostToolUseFailure" => true,
        _ => return None,
    };
    let input = event.get("tool_input")?;
    if input.get("run_in_background").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    Some(ShellCall {
        ended,
        tool_use_id: event.get("tool_use_id")?.as_str()?.to_owned(),
        command: input.get("command")?.as_str()?.to_owned(),
        cwd: event.get("cwd").and_then(Value::as_str).map(str::to_owned),
    })
}

/// The file tools whose calls bring the rules of the touched file's folders (BUILD-PLAN D1.6).
const FILE_TOOLS: [&str; 5] = ["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

/// What to do with a hook event: a `Bash` call's success when its whole output is longer than
/// [`BASH_WHOLE_MAX`] bytes, its failure always; a file tool's success, for its file's rules.
fn plan(event: &Value) -> Option<Plan> {
    let tool = event.get("tool_name").and_then(Value::as_str)?;
    if FILE_TOOLS.contains(&tool) {
        if event.get("hook_event_name").and_then(Value::as_str)? != "PostToolUse" {
            return None;
        }
        let input = event.get("tool_input")?;
        let path = input
            .get("file_path")
            .or_else(|| input.get("notebook_path"))?
            .as_str()
            .filter(|path| !path.is_empty())?;
        return Some(Plan::Rules {
            path: path.to_owned(),
        });
    }
    if tool != "Bash" {
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

/// What a worker's `Stop` hook prints for the stop event `stdin`: `decision: "block"` with the
/// daemon's reason when the turn may not end yet; `None` (the turn ends) otherwise, and on any
/// failure: a hook that can't reach the daemon never keeps a worker going.
fn stop_answer(stdin: &str, ask: impl FnOnce(bool) -> Option<StopHookReply>) -> Option<String> {
    let event: Value = serde_json::from_str(stdin).ok()?;
    let active = event
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let reason = ask(active)?.block?;
    Some(json!({ "decision": "block", "reason": reason }).to_string())
}

/// Asks the daemon for `data_dir` whether the worker of `grant` may end its turn.
fn ask_stop(grant: Option<&str>, data_dir: Option<PathBuf>, active: bool) -> Option<StopHookReply> {
    let grant = grant.filter(|grant| !grant.is_empty())?.to_owned();
    let platform = brigadier_sandbox::native(PlatformOptions { data_dir }).ok()?;
    let mut stream = brigadier_ipc::connect_blocking(
        platform.paths(),
        &ClientFrame::StopHook {
            grant,
            stop_hook_active: active,
        },
    )
    .ok()?;
    brigadier_ipc::read_frame_blocking::<StopHookReply>(&mut stream)
        .ok()
        .flatten()
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

    /// A worker's `Stop` hook blocks with the daemon's reason, passes `stop_hook_active` on,
    /// and lets the turn end when the daemon has no reason or can't be reached.
    #[test]
    fn a_stop_hook_blocks_only_with_the_daemons_reason() {
        let event = r#"{"hook_event_name":"Stop","stop_hook_active":true}"#;
        let printed = stop_answer(event, |active| {
            assert!(active);
            Some(StopHookReply {
                block: Some("Report first.".into()),
            })
        })
        .unwrap();
        let printed: Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(
            printed,
            json!({"decision": "block", "reason": "Report first."})
        );
        assert_eq!(
            stop_answer(event, |_| Some(StopHookReply { block: None })),
            None
        );
        assert_eq!(stop_answer(event, |_| None), None);
        assert_eq!(stop_answer("not json", |_| unreachable!()), None);
        // An event without the flag is a first stop.
        let _ = stop_answer(r#"{"hook_event_name":"Stop"}"#, |active| {
            assert!(!active);
            None
        });
    }

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
            // The call's files go first, on a connection of their own.
            if header.shell.is_some() {
                return None;
            }
            *sent.borrow_mut() = Some((header.clone(), bytes.to_vec()));
            Some(HookReply {
                replacement: Some("exit 0 [full output: read_artifact out-1, …]".into()),
                context: None,
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
                        "stdout": "exit 0 [full output: read_artifact out-1, …]",
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
    /// background, or another tool, sends nothing (#87).
    #[test]
    fn a_bash_calls_start_and_end_go_to_the_daemon_for_its_files() {
        let event = |name: &str, input: Value| {
            json!({
                "hook_event_name": name,
                "tool_name": "Bash",
                "tool_use_id": "toolu_1",
                "cwd": "/repo",
                "tool_input": input,
                "tool_response": { "stdout": "", "stderr": "" },
                "error": "Exit code 1",
            })
            .to_string()
        };
        let command = json!({ "command": "echo x >> notes.md" });
        for (name, ended) in [
            ("PreToolUse", false),
            ("PostToolUse", true),
            ("PostToolUseFailure", true),
        ] {
            let sent = std::cell::RefCell::new(Vec::new());
            let printed = handle(&event(name, command.clone()), |header, bytes| {
                // A failure's excerpt follows on its own connection.
                if header.shell.is_some() {
                    assert!(bytes.is_empty());
                    sent.borrow_mut().push(header.clone());
                }
                None
            });
            assert_eq!(printed, None, "{name}");
            let shell = sent.borrow()[0].shell.clone().expect("the call");
            assert_eq!(
                shell,
                ShellCall {
                    ended,
                    tool_use_id: "toolu_1".into(),
                    command: "echo x >> notes.md".into(),
                    cwd: Some("/repo".into()),
                }
            );
        }
        let background = json!({ "command": "sleep 9", "run_in_background": true });
        assert_eq!(
            handle(&event("PreToolUse", background), |_, _| panic!(
                "a background call's changes stay unowned"
            )),
            None
        );
    }

    #[test]
    fn a_short_success_is_left_alone_without_reaching_the_daemon() {
        let printed = handle(
            &success(json!({ "stdout": "x".repeat(BASH_WHOLE_MAX), "stderr": "" })),
            |header, _| {
                assert!(header.shell.is_some(), "nothing to store");
                None
            },
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
            handle(&long, |_, _| {
                Some(HookReply {
                    replacement: None,
                    context: None,
                })
            }),
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
            touched: None,
            shell: None,
        };
        assert_eq!(send(None, None, &header, b""), None);
        assert_eq!(send(Some(""), None, &header, b""), None);
    }

    /// A file tool's call asks for the rules of its file's folders, and the daemon's rules
    /// follow the call's result; with none to give, nothing is printed (BUILD-PLAN D1.6).
    #[test]
    fn a_file_tools_call_brings_the_rules_of_its_folders() {
        let event = |tool: &str, input: Value| {
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": tool,
                "tool_input": input,
                "tool_response": {},
            })
            .to_string()
        };
        let sent = std::cell::RefCell::new(Vec::new());
        let printed = handle(
            &event("Edit", json!({ "file_path": "/repo/apps/web/page.tsx" })),
            |header, bytes| {
                assert!(bytes.is_empty());
                sent.borrow_mut().push(header.clone());
                Some(HookReply {
                    replacement: None,
                    context: Some("### apps/web/CLAUDE.md\nUse pnpm.".into()),
                })
            },
        )
        .expect("the rules");
        let header = sent.borrow()[0].clone();
        assert_eq!(header.touched.as_deref(), Some("/repo/apps/web/page.tsx"));
        assert_eq!(header.bytes, 0);
        let printed: Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(
            printed,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": "### apps/web/CLAUDE.md\nUse pnpm.",
                }
            })
        );
        // A notebook's path counts too; nothing new to give prints nothing.
        let printed = handle(
            &event("NotebookEdit", json!({ "notebook_path": "/repo/a.ipynb" })),
            |header, _| {
                assert_eq!(header.touched.as_deref(), Some("/repo/a.ipynb"));
                Some(HookReply {
                    replacement: None,
                    context: None,
                })
            },
        );
        assert_eq!(printed, None);
        // A failed call, or another tool, asks nothing.
        let failed = json!({
            "hook_event_name": "PostToolUseFailure",
            "tool_name": "Write",
            "tool_input": { "file_path": "/repo/x" },
            "error": "denied",
        });
        assert_eq!(handle(&failed.to_string(), |_, _| panic!("failed")), None);
        assert_eq!(
            handle(&event("Glob", json!({ "pattern": "*" })), |_, _| panic!(
                "Glob"
            )),
            None
        );
    }
}
