//! The `brigadier` binary end to end: its help, every subcommand's `--json` output against
//! its checked-in schema, and refusal without a valid session grant. A stand-in daemon listens
//! on a temporary data folder's socket and answers with the shapes the real one sends (the
//! real daemon's answers are checked against the same schemas by core's flow test).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use brigadier_cli::commands::{COMMANDS, ERROR_SCHEMA};
use brigadier_cli::protocol::*;
use brigadier_cli::schema::violations;
use interprocess::local_socket::traits::ListenerExt as _;
use interprocess::local_socket::{ListenerOptions, Stream};
use serde_json::Value;

const SESSION: &str = "0199f0aa-0000-7000-8000-00000000c0de";
const GRANT: &str = "brg_good";

/// A temporary data folder, removed when dropped.
struct DataDir(PathBuf);

impl DataDir {
    fn new(name: &str) -> Self {
        // Short: the socket's path must fit a Unix socket address.
        let dir = std::env::temp_dir().join(format!("bcli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for DataDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn brigadier(dir: Option<&Path>, grant: Option<&str>, words: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_brigadier"));
    command
        .args(words)
        .env_remove(SESSION_ENV)
        .env_remove(GRANT_ENV)
        .env_remove("BRIGADIER_DATA_DIR");
    if let Some(dir) = dir {
        command.env("BRIGADIER_DATA_DIR", dir);
    }
    if let Some(grant) = grant {
        command.env(SESSION_ENV, SESSION).env(GRANT_ENV, grant);
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn read_frame(stream: &mut Stream) -> Option<Value> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).ok()?;
    let mut buffer = vec![0u8; u32::from_be_bytes(len) as usize];
    stream.read_exact(&mut buffer).ok()?;
    serde_json::from_slice(&buffer).ok()
}

fn write_frame(stream: &mut Stream, value: &impl serde::Serialize) {
    let json = serde_json::to_vec(value).unwrap();
    stream
        .write_all(&(json.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&json).unwrap();
    stream.flush().unwrap();
}

/// Listens on `dir`'s daemon socket and answers `calls` connections: a wrong grant gets the
/// connection closed without a word, as the daemon does; a right one gets `answer(request)`.
fn stand_in_daemon(
    dir: &Path,
    calls: usize,
    answer: fn(&CliRequest) -> CliReply,
) -> std::thread::JoinHandle<Vec<Value>> {
    let paths = brigadier_sandbox::AppPaths::resolve(dir.to_path_buf()).unwrap();
    std::fs::create_dir_all(&paths.run_dir).unwrap();
    if let Some(socket_dir) = paths.socket_dir() {
        std::fs::create_dir_all(socket_dir).unwrap();
    }
    let name = match &paths.ipc_endpoint {
        #[cfg(unix)]
        brigadier_sandbox::IpcEndpoint::UnixSocket(path) => {
            use interprocess::local_socket::{GenericFilePath, ToFsName};
            path.clone().to_fs_name::<GenericFilePath>().unwrap()
        }
        #[cfg(windows)]
        brigadier_sandbox::IpcEndpoint::NamedPipe(name) => {
            use interprocess::local_socket::{GenericNamespaced, ToNsName};
            name.clone().to_ns_name::<GenericNamespaced>().unwrap()
        }
        #[allow(unreachable_patterns)]
        _ => unreachable!(),
    };
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    std::thread::spawn(move || {
        let mut firsts = Vec::new();
        for stream in listener.incoming().take(calls) {
            let mut stream = stream.unwrap();
            let first = read_frame(&mut stream).unwrap();
            firsts.push(first.clone());
            if first["grant"] != GRANT || first["session"] != SESSION {
                continue;
            }
            let call: CliCall = serde_json::from_value(read_frame(&mut stream).unwrap()).unwrap();
            assert_eq!(call.protocol, PROTOCOL);
            write_frame(&mut stream, &answer(&call.request));
        }
        firsts
    })
}

fn worker_row() -> WorkerRow {
    WorkerRow {
        id: "a1b2c3d4".into(),
        number: 2,
        title: "Add JSON output".into(),
        agent: Agent::Codex,
        model: Some("codex-model-1".into()),
        effort: Some("high".into()),
        state: WorkerState::Working,
        branch: Some("brigadier/a1b2c3d4/task-2".into()),
        changes: Some(Changes {
            files: 3,
            insertions: 40,
            deletions: 2,
        }),
    }
}

/// What the real daemon could send for each request, with every optional part both present
/// and absent across the rows.
fn sample(request: &CliRequest) -> CliReply {
    match request {
        CliRequest::Models => CliReply::ok(&brigadier_cli::models::ModelsOutput {
            models: vec![],
            accounts: vec![],
            guidance: vec![],
            rules: vec![brigadier_cli::models::Rule {
                rule: "forbid: fable".into(),
            }],
        }),
        CliRequest::Sessions => CliReply::ok(&SessionsOutput {
            project: Some("brigadier-ai".into()),
            sessions: vec![
                SessionRow {
                    id: "0000c0de".into(),
                    title: "Strip-down".into(),
                    state: SessionState::Working,
                    branch: Some("brigadier/0000c0de/session".into()),
                    changes: Some(Changes {
                        files: 1,
                        insertions: 12,
                        deletions: 3,
                    }),
                    workers: 2,
                    caller: true,
                },
                SessionRow {
                    id: "0000beef".into(),
                    title: "Docs".into(),
                    state: SessionState::NeedsYou,
                    branch: None,
                    changes: None,
                    workers: 0,
                    caller: false,
                },
            ],
        }),
        CliRequest::Workers => CliReply::ok(&WorkersOutput {
            session: "0000c0de".into(),
            workers: vec![
                worker_row(),
                WorkerRow {
                    model: None,
                    effort: None,
                    branch: None,
                    changes: None,
                    state: WorkerState::Done,
                    ..worker_row()
                },
            ],
        }),
        CliRequest::Show { id } if id == "a1b2" => CliReply::ok(&ShowOutput {
            kind: Kind::Worker,
            id: "a1b2c3d4".into(),
            title: "Add JSON output".into(),
            state: "working".into(),
            agent: Agent::Codex,
            model: None,
            effort: None,
            branch: None,
            changes: None,
            created_at_ms: 1,
            checkout: None,
            base: None,
            waiting: None,
            workers: None,
            parent: Some("0000c0de".into()),
            number: Some(2),
            task: Some("Add --json.".into()),
            report: Some("Done.".into()),
            error: Some("none".into()),
        }),
        CliRequest::Show { .. } => CliReply::ok(&ShowOutput {
            kind: Kind::Session,
            id: "0000c0de".into(),
            title: "Strip-down".into(),
            state: "needs-you".into(),
            agent: Agent::Claude,
            model: Some("claude-opus-5-5".into()),
            effort: Some("high".into()),
            branch: Some("brigadier/0000c0de/session".into()),
            changes: Some(Changes {
                files: 1,
                insertions: 1,
                deletions: 0,
            }),
            created_at_ms: 1,
            checkout: Some(Checkout::Worktree),
            base: Some("main".into()),
            waiting: Some(Waiting {
                questions: 1,
                approvals: 0,
            }),
            workers: Some(vec![worker_row()]),
            parent: None,
            number: None,
            task: None,
            report: None,
            error: None,
        }),
        CliRequest::Read { id, .. } if id == "gone" => CliReply::error(
            ErrorCode::NotFound,
            "no session or worker has the id \"gone\"",
        ),
        CliRequest::Read { last, screen, .. } => CliReply::ok(&ReadOutput {
            kind: Kind::Session,
            id: "0000c0de".into(),
            state: "working".into(),
            view: match (last, screen) {
                (Some(_), _) => ReadView::Items,
                (_, true) => ReadView::Screen,
                _ => ReadView::Last,
            },
            items: vec![
                Item {
                    kind: ItemKind::Message,
                    role: Some(Role::User),
                    text: "Go".into(),
                    output: None,
                    status: None,
                    exit: None,
                    at_ms: 1,
                },
                Item {
                    kind: ItemKind::Command,
                    role: None,
                    text: "cargo test".into(),
                    output: Some("ok".into()),
                    status: Some(ItemStatus::Done),
                    exit: Some(0),
                    at_ms: 2,
                },
            ],
            has_more: true,
        }),
        CliRequest::Start(start) => CliReply::ok(&StartOutput {
            // A just-started worker has no branch yet: its worktree is still being made.
            worker: WorkerRow {
                state: WorkerState::Starting,
                branch: None,
                changes: None,
                ..worker_row()
            },
            permission: start.permission.unwrap_or(Permission::AcceptEdits),
            base: (!start.local).then(|| "brigadier/0000c0de/session".into()),
        }),
        CliRequest::Send { id, answer, .. } => CliReply::ok(&SendOutput {
            id: id.clone(),
            queued: answer.is_none(),
            answered: *answer,
        }),
        CliRequest::Wait { timeout_secs, .. } if timeout_secs.is_some() => {
            CliReply::ok(&WaitOutput {
                events: Vec::new(),
                timed_out: true,
            })
        }
        CliRequest::Wait { .. } => CliReply::ok(&WaitOutput {
            events: vec![
                WaitEvent {
                    id: "a1b2c3d4".into(),
                    event: EventKind::TurnEnded,
                    message: Some("Wrote the file.".into()),
                    status: Some(TurnEnd::Completed),
                    asks: None,
                    resets_at_ms: None,
                    at_ms: 3,
                },
                WaitEvent {
                    id: "e5f6a7b8".into(),
                    event: EventKind::NeedsInput,
                    message: None,
                    status: None,
                    asks: Some("rm -rf build".into()),
                    resets_at_ms: None,
                    at_ms: 4,
                },
                WaitEvent {
                    id: "e5f6a7b8".into(),
                    event: EventKind::UsageLimit,
                    message: Some("weekly limit".into()),
                    status: None,
                    asks: None,
                    resets_at_ms: Some(5),
                    at_ms: 5,
                },
            ],
            timed_out: false,
        }),
        CliRequest::Close { id } => CliReply::ok(&CloseOutput {
            id: id.clone(),
            closed: true,
        }),
        CliRequest::Tool { action, .. } if action == "screenshot" => CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: "The tab browser:agent-1 at https://example.com.".into(),
            // A 1-byte "PNG".
            images: vec!["data:image/png;base64,AA==".into()],
            saved: Vec::new(),
        }),
        CliRequest::Tool { tool, action, args } => CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: format!("{tool:?} {action}: {args}"),
            images: Vec::new(),
            saved: Vec::new(),
        }),
        CliRequest::Ask { options, .. } => CliReply::ok(&AskOutput {
            answer: options
                .first()
                .cloned()
                .unwrap_or_else(|| "In my words".into()),
            option: (!options.is_empty()).then_some(0),
        }),
    }
}

/// The schema file named after `command`.
fn schema_of(command: &str) -> &'static str {
    COMMANDS
        .iter()
        .find(|c| c.name == command)
        .unwrap_or_else(|| panic!("no command {command}"))
        .schema
}

fn assert_fits(schema: &str, output: &Output, what: &str) {
    let text = stdout(output);
    let value: Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("{what}: stdout is not JSON ({err}):\n{text}"));
    let found = violations(schema, &value);
    assert!(found.is_empty(), "{what}:\n{}\n{text}", found.join("\n"));
}

#[test]
fn help_lists_every_subcommand_and_every_schema_has_one() {
    let output = brigadier(None, None, &["--help"]);
    assert!(output.status.success());
    let help = stdout(&output);
    let listed: Vec<&str> = help
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    let names: Vec<&str> = COMMANDS.iter().map(|c| c.name).collect();
    assert_eq!(listed, names, "{help}");
    for name in [
        "sessions", "workers", "show", "read", "start", "send", "wait", "close",
    ] {
        assert!(listed.contains(&name), "{name} missing from:\n{help}");
    }

    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas");
    let mut files = 0;
    for entry in std::fs::read_dir(&schemas).unwrap() {
        let file = entry.unwrap().file_name().into_string().unwrap();
        let name = file.strip_suffix(".schema.json").expect(&file);
        files += 1;
        if name == "error" {
            continue;
        }
        assert!(listed.contains(&name), "{file} has no subcommand in --help");
        let on_disk = std::fs::read_to_string(schemas.join(&file)).unwrap();
        assert_eq!(on_disk, schema_of(name), "{file} is the one compiled in");
    }
    assert_eq!(
        files,
        COMMANDS.len() + 1,
        "one schema per command, and error's"
    );

    // Each command's own help names its arguments.
    let read = stdout(&brigadier(None, None, &["read", "--help"]));
    assert!(read.contains("Usage: brigadier read <id> [--last N] [--screen] [--json]"));
    let start = stdout(&brigadier(None, None, &["help", "start"]));
    assert!(!start.contains("Not available yet"), "{start}");
    assert!(
        start.contains("--lean") && start.contains("--skill NAME"),
        "{start}"
    );
    let models = stdout(&brigadier(None, None, &["help", "models"]));
    assert!(!models.contains("Not available yet"), "{models}");
}

#[test]
fn every_subcommands_json_output_fits_its_schema() {
    let dir = DataDir::new("schemas");
    let read_calls: &[&[&str]] = &[
        &["sessions", "--json"],
        &["workers", "--json"],
        &["show", "0000", "--json"],
        &["show", "a1b2", "--json"],
        &["read", "0000", "--json"],
        &["read", "0000", "--last", "2", "--json"],
        &["read", "0000", "--screen", "--json"],
        &["start", "--json", "write", "a", "file"],
        &["start", "--local", "--permission", "ask", "--json", "go"],
        &["send", "a1b2", "keep", "going", "--json"],
        &["send", "a1b2", "--allow", "--json"],
        &["wait", "--json"],
        &["wait", "a1b2", "--timeout", "1", "--json"],
        &["close", "a1b2", "--json"],
        &["preview", "start", "--name", "web", "--json", "pnpm", "dev"],
        &["preview", "log", "preview-1", "--tail", "5", "--json"],
        &["preview", "stop", "--json"],
        &["browser", "open", "https://example.com", "--keep", "--json"],
        &[
            "browser",
            "screenshot",
            "--tab",
            "browser:agent-1",
            "--json",
        ],
        &[
            "browser",
            "click",
            "--tab",
            "browser:agent-1",
            "e12",
            "--json",
        ],
        &[
            "ask", "--option", "Yes", "--option", "No", "--json", "Ship it?",
        ],
        &["ask", "--json", "What", "name?"],
        &["app", "state", "--json"],
        &[
            "app",
            "open",
            "file",
            "src/main.rs",
            "--line",
            "3",
            "--json",
        ],
        &["page", "read", "page-1", "--json"],
        &[
            "page",
            "write",
            "page-1",
            "--revision",
            "r1",
            "--json",
            "New",
            "text.",
        ],
        &["page", "file-read", "--json"],
        &[
            "page",
            "file-write",
            "--revision",
            "r1",
            "--json",
            "fn",
            "main()",
        ],
        &[
            "brain",
            "search",
            "--session",
            "a1b2c3d4",
            "--json",
            "sidebar",
        ],
        &["brain", "read", "a1b2c3d4", "40", "44", "--json"],
        &["transcript", "search", "--limit", "3", "--json", "budget"],
        &["transcript", "read", "a1b2c3d4", "0", "2", "--json"],
        &["code", "search", "--kind", "symbol", "--json", "ToolHost"],
        &["code", "refs", "cli_call", "--json"],
        &["code", "map", "--json"],
        &["memory", "save", "--json", "Lives", "in", "Lisbon."],
        &[
            "report", "save", "--title", "Printers", "--json", "Bambu", "A1.",
        ],
        &[
            "feedback",
            "offer",
            "--kind",
            "bug",
            "--title",
            "Stop spins",
            "--json",
            "It",
            "spins.",
        ],
        &[
            "approval",
            "request",
            "--action",
            "Buy a domain",
            "--json",
            "12",
            "euros.",
        ],
    ];
    let daemon = stand_in_daemon(&dir.0, read_calls.len() + 2, sample);
    let output = brigadier(Some(&dir.0), Some(GRANT), &["models", "--json"]);
    assert!(output.status.success());
    assert_fits(schema_of("models"), &output, "models");
    for words in read_calls {
        let output = brigadier(Some(&dir.0), Some(GRANT), words);
        assert!(output.status.success(), "{words:?}: {output:?}");
        assert_fits(schema_of(words[0]), &output, &format!("{words:?}"));
    }
    // A failed call's output fits the error schema.
    let output = brigadier(Some(&dir.0), Some(GRANT), &["read", "gone", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    assert_fits(ERROR_SCHEMA, &output, "not found");
    daemon.join().unwrap();

    // The commands not built yet answer with an error of the error schema, exit 4, and never
    // call the daemon.
    for command in COMMANDS.iter().filter(|c| !c.available) {
        let mut words = vec![command.name];
        words.push("--json");
        let output = brigadier(None, Some(GRANT), &words);
        assert_eq!(output.status.code(), Some(4), "{words:?}");
        assert_fits(ERROR_SCHEMA, &output, &format!("{words:?}"));
        assert!(stdout(&output).contains("not-available"));
    }
}

#[test]
fn text_output_reads_well() {
    let dir = DataDir::new("text");
    let daemon = stand_in_daemon(&dir.0, 11, sample);
    let models = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["models"]));
    assert!(
        models.contains("Your rules") && models.contains("forbid: fable"),
        "{models}"
    );
    assert!(models.lines().count() <= 25);
    let sessions = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["sessions"]));
    assert!(sessions.starts_with("brigadier-ai\nID"), "{sessions}");
    assert!(sessions.contains("Strip-down (this session)"), "{sessions}");
    assert!(sessions.contains("+12 −3"), "{sessions}");
    let workers = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["workers"]));
    assert!(workers.contains("codex-model-1 (high)"), "{workers}");
    let read = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["read", "0000", "--last", "2"],
    ));
    assert!(read.contains("$ cargo test\n  ok\n  (exit 0)"), "{read}");
    let start = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["start", "go"]));
    assert!(
        start.starts_with(
            "a1b2c3d4\nStarted worker 2 (codex, codex-model-1), in a new worktree from \
             brigadier/0000c0de/session, "
        ),
        "{start}"
    );
    let local = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["start", "--local", "go"],
    ));
    assert!(local.contains(", in this session's checkout, "), "{local}");
    let wait = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["wait"]));
    assert!(
        wait.starts_with("a1b2c3d4 turn-ended (completed)\nWrote the file.\n"),
        "{wait}"
    );
    assert!(
        wait.contains("e5f6a7b8 needs-input\nasks: rm -rf build\n"),
        "{wait}"
    );
    let send = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["send", "e5f6", "--deny"],
    ));
    assert_eq!(send, "Denied what e5f6 asked.\n");
    let close = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["close", "a1b2"]));
    assert_eq!(close, "Closed a1b2.\n");
    // A screenshot is saved where `--out` says, and the text says where.
    let shot = dir.0.join("shot.png");
    let shown = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &[
            "browser",
            "screenshot",
            "--tab",
            "browser:agent-1",
            "--out",
            shot.to_str().unwrap(),
        ],
    ));
    assert_eq!(std::fs::read(&shot).unwrap(), [0]);
    assert!(
        shown.ends_with(&format!("Screenshot saved to {}\n", shot.display())),
        "{shown}"
    );
    let asked = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["ask", "--option", "Yes", "--option", "No", "Ship", "it?"],
    ));
    assert_eq!(asked, "Yes\n");
    daemon.join().unwrap();
}

#[test]
fn a_call_without_a_valid_session_grant_is_refused() {
    // No session in the environment: refused before anything is sent.
    let output = brigadier(None, None, &["sessions", "--json"]);
    assert_eq!(output.status.code(), Some(3));
    assert_fits(ERROR_SCHEMA, &output, "no grant");
    assert!(stdout(&output).contains("\"refused\""));
    assert!(String::from_utf8_lossy(&output.stderr).contains("BRIGADIER_SESSION"));

    // A grant the daemon doesn't know: it closes the connection, and the call is refused.
    let dir = DataDir::new("refused");
    let daemon = stand_in_daemon(&dir.0, 1, sample);
    let output = brigadier(Some(&dir.0), Some("brg_wrong"), &["workers", "--json"]);
    assert_eq!(output.status.code(), Some(3), "{output:?}");
    assert_fits(ERROR_SCHEMA, &output, "wrong grant");
    let firsts = daemon.join().unwrap();
    assert_eq!(firsts[0]["type"], "cli");
    assert_eq!(firsts[0]["grant"], "brg_wrong");

    // No daemon at all: unreachable, exit 5.
    let empty = DataDir::new("nodaemon");
    let output = brigadier(Some(&empty.0), Some(GRANT), &["sessions", "--json"]);
    assert_eq!(output.status.code(), Some(5));
    assert_fits(ERROR_SCHEMA, &output, "no daemon");
}

#[test]
fn wrong_arguments_exit_2_with_the_usage() {
    let output = brigadier(None, None, &["read", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    assert_fits(ERROR_SCHEMA, &output, "usage");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing <id>") && stderr.contains("usage: brigadier read"));
}
