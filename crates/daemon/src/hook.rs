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
//! - **A file tool's success** (`Read`, `Edit`, `Write`, `MultiEdit`, `NotebookEdit`, run by
//!   a second `PostToolUse` entry): the touched file's path goes to the daemon, which answers
//!   with the rules of its folders the thread hasn't had yet (a subfolder's `CLAUDE.md` or
//!   `AGENTS.md`, left out of its instructions); the hook prints them as
//!   `hookSpecificOutput.additionalContext`, which follows the call's result (BUILD-PLAN D1.6).
//!
//! A Codex thread's hook (`post-tool-use --codex`, from its own home's `hooks.json`, #121) runs
//! after its shell calls and patches, the way Codex reads and changes files: the files and
//! folders the command or patch names go to the daemon the same way, for their rules.
//!
//! The grant (in [`HOOK_GRANT_ENV`], from the CLI's environment) can only store this thread's
//! output. Whatever goes wrong, the hook prints nothing and exits 0: the tool call never
//! breaks. No async runtime and no logging, so it starts at once.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use brigadier_core::digest::BASH_WHOLE_MAX;
use brigadier_core::manager::{
    COMMAND_PATHS_MAX, HOOK_GRANT_ENV, OUTPUT_MAX_BYTES, command_paths, lexical,
};
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
    /// A file a `Read`, `Edit` or `Write` call touched: the rules of its folders the thread
    /// hasn't had yet follow the call's result (BUILD-PLAN D1.6).
    Rules { path: String },
}

/// Runs the hook with the arguments after `hook`.
pub fn run(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    // Every path ends here with nothing printed: never break the tool call.
    if args.next().is_none_or(|event| event != "post-tool-use") {
        return ExitCode::SUCCESS;
    }
    let mut data_dir: Option<PathBuf> = None;
    let mut codex = false;
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            data_dir = args.next().map(PathBuf::from);
        } else if arg == "--codex" {
            codex = true;
        }
    }
    let mut stdin = String::new();
    if std::io::stdin().read_to_string(&mut stdin).is_err() {
        return ExitCode::SUCCESS;
    }
    let grant = std::env::var(HOOK_GRANT_ENV).ok();
    let printed = if codex {
        codex_rules(&stdin, |path| {
            let header = HookOutput {
                excerpt: false,
                status: String::new(),
                bytes: 0,
                touched: Some(path.to_owned()),
            };
            send(grant.as_deref(), data_dir.clone(), &header, &[])?.context
        })
    } else {
        handle(&stdin, |output, bytes| {
            send(grant.as_deref(), data_dir.clone(), output, bytes)
        })
    };
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
                touched: None,
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

/// What a Codex thread's hook prints for its `PostToolUse` event `stdin`, given a way to ask
/// the daemon for a path's rules: the rules of the folders of the files and folders its shell
/// command or patch names, as `additionalContext`, which Codex gives the model after the
/// call's result; `None`: nothing new.
fn codex_rules(stdin: &str, mut ask: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let event: Value = serde_json::from_str(stdin).ok()?;
    if event.get("hook_event_name").and_then(Value::as_str)? != "PostToolUse" {
        return None;
    }
    let mut cwd = PathBuf::from(event.get("cwd").and_then(Value::as_str)?);
    let input = event.get("tool_input")?;
    let command = input.get("command")?.as_str()?;
    // A call run in a folder of its own (`workdir`) reads from there.
    if let Some(workdir) = input.get("workdir").and_then(Value::as_str) {
        cwd = lexical(&cwd.join(workdir));
    }
    let paths = match event.get("tool_name").and_then(Value::as_str)? {
        "Bash" => command_paths(command, &cwd),
        "apply_patch" => patch_paths(command, &cwd),
        _ => return None,
    };
    let rules: Vec<String> = paths
        .iter()
        .take(COMMAND_PATHS_MAX)
        .filter_map(|path| ask(&path.display().to_string()))
        .collect();
    if rules.is_empty() {
        return None;
    }
    Some(
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": rules.join("\n\n"),
            }
        })
        .to_string(),
    )
}

/// The files a Codex patch adds, changes, deletes or moves to, from `cwd`.
fn patch_paths(patch: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for line in patch.lines() {
        let named = [
            "*** Add File: ",
            "*** Update File: ",
            "*** Delete File: ",
            "*** Move to: ",
        ]
        .iter()
        .find_map(|head| line.strip_prefix(head))
        .map(str::trim)
        .filter(|path| !path.is_empty());
        if let Some(path) = named.map(|path| lexical(&cwd.join(path)))
            && !found.contains(&path)
        {
            found.push(path);
        }
    }
    found
}

const FILE_TOOLS: &[&str] = &["Read", "Edit", "Write", "MultiEdit", "NotebookEdit"];

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
        };
        assert_eq!(send(None, None, &header, b""), None);
        assert_eq!(send(Some(""), None, &header, b""), None);
    }

    /// A Codex thread's shell call or patch asks for the rules of each file and folder it
    /// names (from its folder, or where a `cd` went), and their rules follow the call's result
    /// together; with none to give, or for another call, nothing is printed (#121).
    #[test]
    fn a_codex_calls_files_bring_the_rules_of_their_folders() {
        let root =
            Temp(std::env::temp_dir().join(format!("brigadier-hook-{}", uuid::Uuid::new_v4())));
        let workspace = root.join("ws");
        std::fs::create_dir_all(workspace.join("apps/web")).unwrap();
        std::fs::create_dir_all(workspace.join("docs")).unwrap();
        std::fs::write(workspace.join("apps/web/page.tsx"), "x").unwrap();
        let own = root.join("thread");
        std::fs::create_dir_all(&own).unwrap();
        let event = |tool: &str, command: &str| {
            json!({
                "hook_event_name": "PostToolUse",
                "tool_name": tool,
                "tool_input": { "command": command },
                "tool_response": "Exit code: 0",
                "cwd": own.display().to_string(),
                "turn_id": "t1",
            })
            .to_string()
        };
        let here = own.display().to_string();
        // The thread's own folder, outside the workspace, has none.
        let rules = |path: &str| (path != here).then(|| format!("rules for {path}"));
        let asked = std::cell::RefCell::new(Vec::new());
        let shell = format!(
            "cd '{}' && sed -n '1,80p' apps/web/page.tsx && ls -la docs \"apps/web/page.tsx\" | head",
            workspace.display()
        );
        let printed = codex_rules(&event("Bash", &shell), |path| {
            asked.borrow_mut().push(path.to_owned());
            rules(path)
        })
        .expect("the rules");
        let ws = workspace.display().to_string();
        // A command's paths come out with the platform's separator (`\` on Windows).
        let under = |dir: &Path, path: &str| {
            path.split('/')
                .fold(dir.to_path_buf(), |dir, part| dir.join(part))
                .display()
                .to_string()
        };
        let page = under(&workspace, "apps/web/page.tsx");
        let docs = workspace.join("docs").display().to_string();
        assert_eq!(
            *asked.borrow(),
            [here.clone(), ws.clone(), page.clone(), docs.clone()]
        );
        let printed: Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(
            printed,
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "PostToolUse",
                    "additionalContext": format!(
                        "rules for {ws}\n\nrules for {page}\n\nrules for {docs}"
                    ),
                }
            })
        );
        // A patch's files, absolute or from its folder; a new file needn't be there yet.
        let patch = format!(
            "*** Begin Patch\n*** Update File: {page}\n@@\n-x\n+y\n*** Add File: notes/new.md\n+hi\n*** End Patch"
        );
        let asked = std::cell::RefCell::new(Vec::new());
        let printed = codex_rules(&event("apply_patch", &patch), |path| {
            asked.borrow_mut().push(path.to_owned());
            None
        });
        assert_eq!(printed, None, "nothing new to give");
        assert_eq!(*asked.borrow(), [page.clone(), under(&own, "notes/new.md")]);
        // A `..` leads out of the folder it follows: `docs` is asked about, not `apps/docs`.
        asked.borrow_mut().clear();
        let up = format!(
            "cd '{}' && ls ../docs ./web/../../apps/web/page.tsx",
            workspace.join("apps").display()
        );
        codex_rules(&event("Bash", &up), |path| {
            asked.borrow_mut().push(path.to_owned());
            None
        });
        let patched = "*** Begin Patch\n*** Update File: ../ws/docs/../apps/web/page.tsx\n@@\n-x\n+y\n*** End Patch";
        codex_rules(&event("apply_patch", patched), |path| {
            asked.borrow_mut().push(path.to_owned());
            None
        });
        let apps = workspace.join("apps").display().to_string();
        assert_eq!(
            *asked.borrow(),
            [here.clone(), apps, docs, page.clone(), page]
        );
        // A search names no path: the folder it runs in (its `workdir`) is asked about.
        let mut search: Value = serde_json::from_str(&event("Bash", "rg TODO")).unwrap();
        search["tool_input"]["workdir"] = json!(workspace.join("apps/web"));
        let asked = std::cell::RefCell::new(Vec::new());
        let printed = codex_rules(&search.to_string(), |path| {
            asked.borrow_mut().push(path.to_owned());
            rules(path)
        });
        let web = under(&workspace, "apps/web");
        // Read from the JSON, which escapes a Windows path's `\`.
        let printed: Value = serde_json::from_str(&printed.expect("the rules")).unwrap();
        let context = printed["hookSpecificOutput"]["additionalContext"].as_str();
        assert!(context.is_some_and(|context| context.contains(&format!("rules for {web}"))));
        assert_eq!(*asked.borrow(), [web]);
        // A command naming nothing there asks about its own folder only; another tool or
        // another event asks nothing.
        let asked = std::cell::RefCell::new(Vec::new());
        let printed = codex_rules(&event("Bash", "echo hi --x ~/a *.rs"), |path| {
            asked.borrow_mut().push(path.to_owned());
            rules(path)
        });
        assert_eq!((printed, asked.into_inner()), (None, vec![here.clone()]));
        let never = |_: &str| -> Option<String> { panic!("asked") };
        assert_eq!(
            codex_rules(&event("mcp__brigadier__run", "ls"), never),
            None
        );
        let started = event("Bash", "ls").replace("PostToolUse", "PreToolUse");
        assert_eq!(codex_rules(&started, never), None);
        assert_eq!(codex_rules("not json", never), None);
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
