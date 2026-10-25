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
            // A wait says its events were heard once it printed them.
            if let CliRequest::Wait { .. } = call.request
                && let Some(heard) = read_frame(&mut stream)
            {
                firsts.push(heard);
            }
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
        context: Some(ContextSize {
            used_tokens: 120_000,
            window_tokens: Some(1_000_000),
        }),
        git: Some(git_facts()),
        commands: Some(turn_commands()),
    }
}

/// What git shows of a worker: 2 commits not merged, a conflict, a file left uncommitted.
fn git_facts() -> GitFacts {
    GitFacts {
        branch: Some("brigadier/a1b2c3d4/task-2".into()),
        base: Some("brigadier/0000c0de/session".into()),
        base_commit: Some("1111111111111111111111111111111111111111".into()),
        base_checkout_clean: Some(true),
        commits: vec![
            CommitFact {
                hash: "abcdef0123456789abcdef0123456789abcdef01".into(),
                subject: "Add --json".into(),
            },
            CommitFact {
                hash: "0123456789abcdef0123456789abcdef01234567".into(),
                subject: "Add the schema".into(),
            },
        ],
        ahead: 2,
        behind: 1,
        uncommitted: Some(1),
        merge: Some(MergeWord::Conflicts),
        conflicts: vec!["src/render.rs".into()],
    }
}

/// The commands a turn ran: a failed test run, one with no exit known, one in the background.
fn turn_commands() -> TurnCommands {
    TurnCommands {
        turn: Some(3),
        total: 3,
        commands: vec![
            CommandFact {
                command: "cargo test -p brigadier-cli".into(),
                turn: Some(3),
                status: CommandStatus::Failed,
                exit: Some(101),
                head: Some("0123456789abcdef0123456789abcdef01234567".into()),
            },
            CommandFact {
                command: "pnpm dev".into(),
                turn: Some(2),
                status: CommandStatus::Background,
                exit: None,
                head: None,
            },
            CommandFact {
                command: "cat <<'EOF' > notes.md\nline\nEOF".into(),
                turn: Some(3),
                status: CommandStatus::Done,
                exit: Some(0),
                head: Some("abcdef0123456789abcdef0123456789abcdef01".into()),
            },
        ],
    }
}

/// What the real daemon could send for each request, with every optional part both present
/// and absent across the rows.
fn sample(request: &CliRequest) -> CliReply {
    match request {
        CliRequest::Models => CliReply::ok(&brigadier_cli::models::ModelsOutput {
            workers_at_once: 3,
            models: vec![],
            agents: vec![brigadier_cli::models::AgentAccounts {
                agent: Agent::Claude,
                switch_accounts: true,
            }],
            switch_providers: false,
            accounts: vec![
                brigadier_cli::models::Account {
                    id: "own".into(),
                    name: String::new(),
                    agent: Agent::Claude,
                    current: true,
                    you: false,
                    logged_in: true,
                    age_s: Some(30),
                    fit: "ok".into(),
                    usage: vec![],
                    place: Some(1),
                    why: "nearest its limit (5-hour 80% used)".into(),
                },
                brigadier_cli::models::Account {
                    id: "acct-b".into(),
                    name: "Work".into(),
                    agent: Agent::Claude,
                    current: false,
                    you: false,
                    logged_in: true,
                    age_s: None,
                    fit: "out until 14:10".into(),
                    usage: vec![],
                    place: None,
                    why: "at its limit until 14:10".into(),
                },
            ],
            guidance: vec![],
            rules: vec![brigadier_cli::models::Rule {
                rule: "forbid: fable".into(),
            }],
        }),
        CliRequest::Sessions { all } => CliReply::ok(&SessionsOutput {
            project: (!all).then(|| "brigadier-ai".into()),
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
                    project: all.then(|| "brigadier-ai".into()),
                },
                SessionRow {
                    id: "0000beef".into(),
                    title: "Docs".into(),
                    state: SessionState::NeedsYou,
                    branch: None,
                    changes: None,
                    workers: 0,
                    caller: false,
                    project: all.then(|| "other".into()),
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
                    context: None,
                    state: WorkerState::Done,
                    git: None,
                    commands: None,
                    ..worker_row()
                },
            ],
            wait_running: false,
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
            git: Some(GitFacts {
                base: None,
                base_commit: None,
                base_checkout_clean: None,
                commits: Vec::new(),
                ahead: 0,
                behind: 0,
                uncommitted: Some(0),
                merge: None,
                conflicts: Vec::new(),
                ..git_facts()
            }),
            commands: Some(turn_commands()),
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
            git: None,
            commands: None,
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
                git: None,
                commands: None,
                ..worker_row()
            },
            permission: start.permission.unwrap_or(Permission::AcceptEdits),
            base: (!start.local).then(|| "brigadier/0000c0de/session".into()),
            // `--work` answers with the row that picked what the flags left out.
            row: start.work.as_ref().map(|work| StartRow {
                work: work.clone(),
                source: "user".into(),
                picked: vec!["agent".into(), "effort".into()],
            }),
        }),
        CliRequest::Send { id, answer, .. } => CliReply::ok(&SendOutput {
            id: id.clone(),
            queued: answer.is_none(),
            answered: *answer,
            message: answer.is_none().then(|| "msg-session-1".into()),
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
                    event_id: "ev-0000aaaa".into(),
                    event: EventKind::TurnEnded,
                    message: Some("Wrote the file.".into()),
                    asks: None,
                    resets_at_ms: None,
                    at_ms: 3,
                    git: Some(git_facts()),
                    commands: Some(turn_commands()),
                },
                WaitEvent {
                    id: "e5f6a7b8".into(),
                    event_id: String::new(),
                    event: EventKind::NeedsInput,
                    message: None,
                    asks: Some("rm -rf build".into()),
                    resets_at_ms: None,
                    at_ms: 4,
                    git: None,
                    commands: None,
                },
                WaitEvent {
                    id: "e5f6a7b8".into(),
                    event_id: String::new(),
                    event: EventKind::UsageLimit,
                    message: Some("weekly limit".into()),
                    asks: None,
                    resets_at_ms: Some(5),
                    at_ms: 5,
                    git: None,
                    commands: None,
                },
            ],
            timed_out: false,
        }),
        CliRequest::Close { id } => CliReply::ok(&CloseOutput {
            id: id.clone(),
            closed: true,
        }),
        CliRequest::Queue { id, judgment } => CliReply::ok(&QueueOutput {
            id: id.clone(),
            state: match judgment {
                Judgment::Join => QueueState::Joined,
                Judgment::Wait => QueueState::Waiting,
                Judgment::Worker { .. } => QueueState::SentToWorker,
                Judgment::NewSession => QueueState::Offered,
            },
            already: false,
            worker: match judgment {
                Judgment::Worker { id } => Some(id.clone()),
                _ => None,
            },
        }),
        CliRequest::ContinueOn { agent, after_turn } => {
            CliReply::ok(&brigadier_cli::protocol::ContinueOnOutput {
                agent: *agent,
                model: None,
                effort: Some("high".into()),
                when: if *after_turn {
                    brigadier_cli::protocol::ContinueWhen::AfterTurn
                } else {
                    brigadier_cli::protocol::ContinueWhen::Now
                },
            })
        }
        CliRequest::Tool { action, .. } if action == "screenshot" => CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: "The tab browser:agent-1 at https://example.com.".into(),
            // A 1-byte "PNG".
            images: vec!["data:image/png;base64,AA==".into()],
            saved: Vec::new(),
        }),
        CliRequest::Tool {
            tool: Tool::Ledger,
            action,
            args,
        } => CliReply::ok(&LedgerOutput {
            action: action.clone(),
            text: format!("Ledger {action}: {args}"),
            check: (action == "check").then(|| LedgerCheck {
                file: args["file"].as_str().unwrap().to_owned(),
                uncited: vec![LedgerEntry {
                    id: "L3".into(),
                    entry: "- L3 · 10-23 08:10 · gen 1 · Decision: keep tabs".into(),
                }],
                ended: vec![LedgerEnded {
                    id: "L2".into(),
                    how: "superseded".into(),
                    by: Some("L5".into()),
                }],
                unknown: vec!["L99".into()],
                no_id: vec![LedgerNoId {
                    file: "ledger.md".into(),
                    line: 10,
                    entry: "- Decision: by hand".into(),
                }],
            }),
        }),
        CliRequest::Tool {
            tool: Tool::Library,
            action,
            args,
        } => CliReply::ok(&LibraryOutput {
            action: action.clone(),
            items: match action.as_str() {
                "list" | "add" => vec![LibraryItem {
                    name: args["name"].as_str().unwrap_or("sentry").to_owned(),
                    kind: "server".into(),
                    detail: "https://mcp.sentry.dev/mcp".into(),
                    everywhere: args.get("project").is_none(),
                    on_in: args["project"]
                        .as_str()
                        .map(str::to_owned)
                        .into_iter()
                        .collect(),
                    off_in: vec![],
                    agents: vec![
                        LibraryAgentState {
                            agent: "claude".into(),
                            state: "ready".into(),
                        },
                        LibraryAgentState {
                            agent: "codex".into(),
                            state: "needsSignIn".into(),
                        },
                    ],
                    attention: Some("Codex needs you to sign in.".into()),
                }],
                _ => vec![],
            },
            agents: match action.as_str() {
                "list" => vec![],
                _ => vec![
                    LibraryAgentResult {
                        agent: "claude".into(),
                        outcome: "done".into(),
                        message: None,
                    },
                    LibraryAgentResult {
                        agent: "codex".into(),
                        outcome: "failed".into(),
                        message: Some(
                            "Codex only reads this project's settings once you trust the project."
                                .into(),
                        ),
                    },
                ],
            },
            warning: None,
        }),
        CliRequest::Tool { tool, action, args } => CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: format!("{tool:?} {action}: {args}"),
            images: Vec::new(),
            saved: Vec::new(),
        }),
        // By line: a turn's (3), a worker's (4), the user's commit (5).
        CliRequest::Why { path, line } => {
            assert!(Path::new(path).is_absolute(), "{path}");
            let turn = WhyTurn {
                number: 2,
                started_at_ms: Some(1_792_783_200_000),
                ended_at_ms: None,
                path: "src/main.rs".into(),
                line: 3,
                message: Some("Count the lines.".into()),
                reply: Some("Counted.".into()),
                reply_cut: true,
                read: Some("brigadier brain read a1b2c3d4 4 6".into()),
                ledger: vec![WhyLedgerEntry {
                    id: Some("L3".into()),
                    text: "- L3 · 10-23 08:10 · gen 1 · Decision: keep tabs".into(),
                }],
                gone: (*line == 4).then(|| "Its CLI session file is gone.".into()),
            };
            let commit = WhyCommit {
                id: "0123456789abcdef0123456789abcdef01234567".into(),
                author: "Test".into(),
                at_ms: 1_792_783_200_000,
                summary: "Start".into(),
                merge: (*line == 4).then(|| "1123456789abcdef0123456789abcdef01234567".into()),
                branch: (*line == 4).then(|| "brigadier/count".into()),
            };
            CliReply::ok(&WhyOutput {
                path: "src/main.rs".into(),
                line: *line,
                text: "fn main() {}".into(),
                by: match line {
                    3 => WhyBy::Turn,
                    4 => WhyBy::Worker,
                    _ => WhyBy::Commit,
                },
                session: (*line != 5).then(|| WhySession {
                    id: "a1b2c3d4".into(),
                    title: "Count".into(),
                    agent: Agent::Claude,
                }),
                turn: (*line != 5).then_some(turn),
                worker: (*line == 4).then(|| WhyWorker {
                    id: "e5f6a7b8".into(),
                    number: 1,
                    title: "Count the lines".into(),
                    agent: Agent::Codex,
                    branch: Some("brigadier/count".into()),
                    task: Some("Count the lines in README.".into()),
                    reply: Some("Three.".into()),
                    reply_cut: false,
                    read: Some("brigadier brain read a1b2c3d4/1-e5f6a7b8 0 9".into()),
                    gone: None,
                }),
                commit: (*line != 3).then_some(commit),
                moved_by: (*line == 3)
                    .then(|| WhyMove {
                        session: "a1b2c3d4".into(),
                        turn: 3,
                    })
                    .into_iter()
                    .collect(),
            })
        }
        CliRequest::Context { part, name, .. } => CliReply::ok(&context_sample(*part, name)),
        CliRequest::Ask { questions, .. } => CliReply::ok(&AskOutput::of(
            questions
                .iter()
                .map(|item| AskAnswer {
                    question: item.question.clone(),
                    answer: item
                        .options
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "In my words".into()),
                    option: (!item.options.is_empty()).then_some(0),
                })
                .collect(),
        )),
    }
}

/// What the daemon answers `brigadier context [<part>]`, in the shapes it sends.
fn context_sample(part: Option<ContextPart>, name: &Option<String>) -> ContextOutput {
    let mut output = ContextOutput {
        part: part.map_or("overview", ContextPart::word).into(),
        text: format!("{} text\n", part.map_or("overview", ContextPart::word)),
        parts: Vec::new(),
        entries: Vec::new(),
        items: Vec::new(),
        files: Vec::new(),
        models: None,
    };
    match part {
        None => {
            output.parts = ContextPart::ALL
                .into_iter()
                .map(|part| ContextPartSize {
                    part: part.word().into(),
                    summary: "What it holds".into(),
                    entries: (part != ContextPart::Brief).then_some(2),
                    lines: 4,
                    tokens: 30,
                    command: format!("brigadier context {}", part.word()),
                })
                .collect();
        }
        Some(ContextPart::Decisions | ContextPart::Open | ContextPart::Ledger) => {
            output.entries = vec![
                ContextEntry {
                    id: Some("L3".into()),
                    kind: "rule".into(),
                    source: "session".into(),
                    text: "- L3 · 10-25 11:00 · gen 1 · Rule from the user: \"no tabs\"".into(),
                },
                ContextEntry {
                    id: None,
                    kind: "other".into(),
                    source: "own".into(),
                    text: "- Ruled out: the cache".into(),
                },
                ContextEntry {
                    id: Some("card-1".into()),
                    kind: "question".into(),
                    source: "card".into(),
                    text: "Ship it?".into(),
                },
            ];
        }
        Some(ContextPart::Skills) if name.is_none() => {
            output.items = vec![ContextItem {
                name: "tools:review".into(),
                kind: "skill".into(),
                detail: "Reviews a diff.".into(),
                has: true,
                status: None,
            }];
        }
        Some(ContextPart::Mcps) => {
            output.items = vec![ContextItem {
                name: "pencil".into(),
                kind: "server".into(),
                detail: "Claude Code (user)".into(),
                has: false,
                status: Some("connected".into()),
            }];
        }
        Some(ContextPart::Project) => {
            output.files = vec![ContextFile {
                path: "CLAUDE.md".into(),
                lines: 1,
                text: "Use tabs.\n".into(),
            }];
        }
        Some(ContextPart::Routing) => {
            let CliReply::Ok { output: models } = sample(&CliRequest::Models) else {
                unreachable!()
            };
            output.models = Some(serde_json::from_value(models).unwrap());
        }
        Some(_) => {}
    }
    output
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
        start.contains("--mcp NAME") && start.contains("--with WHAT"),
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
        &["sessions", "--all", "--json"],
        &["workers", "--json"],
        &["show", "0000", "--json"],
        &["show", "a1b2", "--json"],
        &["read", "0000", "--json"],
        &["read", "0000", "--last", "2", "--json"],
        &["read", "0000", "--screen", "--json"],
        &["start", "--json", "write", "a", "file"],
        &["start", "--local", "--permission", "ask", "--json", "go"],
        &["start", "--work", "Independent review", "--json", "go"],
        &["send", "a1b2", "keep", "going", "--json"],
        &["send", "a1b2", "--allow", "--json"],
        &["wait", "--json"],
        &["wait", "a1b2", "--timeout", "1", "--json"],
        &["close", "a1b2", "--json"],
        &["queue", "9f8e7d6c", "join", "--json"],
        &["queue", "9f8e7d6c", "wait", "--json"],
        &["queue", "9f8e7d6c", "worker", "a1b2", "--json"],
        &["queue", "9f8e7d6c", "new-session", "--json"],
        &["continue-on", "codex", "--after-turn", "--json"],
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
        &[
            "ask",
            "--question",
            "Ship it?",
            "--option",
            "Yes",
            "--option",
            "No",
            "--question",
            "What name?",
            "--json",
        ],
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
            "ledger",
            "add",
            "Decision: keep tabs (user message 1)",
            "--json",
        ],
        &["ledger", "show", "--json"],
        &["ledger", "show", "--open", "--rules", "--json"],
        &["ledger", "close", "L3", "asked", "again", "--json"],
        &["checkpoint", "set", "Goal:", "ship.", "--json"],
        &["checkpoint", "show", "--json"],
        &["context", "--json"],
        &["context", "brief", "--json"],
        &["context", "decisions", "--json"],
        &["context", "open", "--json"],
        &["context", "ledger", "--json"],
        &["context", "checkpoint", "--json"],
        &["context", "transcript", "--json"],
        &["context", "transcript", "--grep", "cache", "--json"],
        &["context", "transcript", "--last", "5", "--json"],
        &["context", "transcript", "--item", "u-1", "--json"],
        &["context", "skills", "--json"],
        &["context", "skills", "tools:review", "--json"],
        &["context", "mcps", "--json"],
        &["context", "project", "--json"],
        &["context", "routing", "--json"],
        &[
            "ledger",
            "check",
            concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
            "--json",
        ],
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
        &["library", "list", "--json"],
        &["library", "list", "--project", ".", "--json"],
        &[
            "library",
            "add",
            "--project",
            ".",
            "--json",
            "--",
            "claude",
            "mcp",
            "add",
            "--transport",
            "http",
            "sentry",
            "https://mcp.sentry.dev/mcp",
        ],
        &["library", "remove", "sentry", "--json"],
        &["library", "login", "sentry", "--json"],
        &["why", "src/main.rs:3", "--json"],
        &["why", "src/main.rs", "--line", "4", "--json"],
        &["why", "src/main.rs:5", "--json"],
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
    let daemon = stand_in_daemon(&dir.0, 17, sample);
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
    let library = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["library", "list"]));
    assert_eq!(
        library,
        "sentry  server  every project  claude ready, codex needsSignIn\n\
         sentry: Codex needs you to sign in.\n"
    );
    let login = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["library", "login", "sentry"],
    ));
    assert_eq!(
        login,
        "claude: done\ncodex: failed — Codex only reads this project's settings once you trust the project.\n"
    );
    let workers = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["workers"]));
    assert!(workers.contains("codex-model-1 (high)"), "{workers}");
    // How full each worker's context is, to weigh reusing it (run2-D42).
    assert!(workers.contains("CONTEXT"), "{workers}");
    assert!(workers.contains("120k of 1M"), "{workers}");
    // What git and the CLI showed, in a few words, and no wait running while one works.
    assert!(
        workers.contains("conflicts · 2 commits · 1 uncommitted  3, 1 failed"),
        "{workers}"
    );
    assert!(
        workers.contains("No `brigadier wait` is running while worker 2 is at work"),
        "{workers}"
    );
    let read = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["read", "0000", "--last", "2"],
    ));
    assert!(read.contains("$ cargo test\n  ok\n  (exit 0)"), "{read}");
    let start = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["start", "go"]));
    assert!(
        start.starts_with(
            "a1b2c3d4\nStarted worker 2 (codex, codex-model-1, high effort), in a new \
             worktree from brigadier/0000c0de/session, "
        ),
        "{start}"
    );
    // The receipt names the row `--work` followed and what it picked.
    let by_work = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &[
            "start",
            "--model",
            "codex-model-1",
            "--work",
            "Independent review",
            "go",
        ],
    ));
    assert!(
        by_work.contains("\nYour row \"Independent review\" picked its agent and effort.\n"),
        "{by_work}"
    );
    let local = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["start", "--local", "go"],
    ));
    assert!(local.contains(", in this session's checkout, "), "{local}");
    let wait = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["wait"]));
    assert!(
        wait.starts_with("a1b2c3d4 turn-ended (ev-0000aaaa)\nWrote the file.\n"),
        "{wait}"
    );
    assert!(
        wait.contains("e5f6a7b8 needs-input\nasks: rm -rf build\n"),
        "{wait}"
    );
    // The facts Brigadier read, after the worker's words, and one next step (harness P1, P3).
    assert!(
        wait.contains(
            "Wrote the file.\ngit: 2 commits not in brigadier/0000c0de/session · merge would \
             conflict in src/render.rs · 1 uncommitted file\n  abcdef01 Add --json\n  \
             01234567 Add the schema\nran in turn 3: 3 commands\n  $ cargo test -p \
             brigadier-cli → exit 101 · at 01234567\n  $ pnpm dev → in the background, exit \
             unknown · at unknown commit (from turn 2)\n  $ cat <<'EOF' > notes.md… → exit 0 · \
             at abcdef01\nNext: merge into brigadier/0000c0de/session would conflict; it left \
             uncommitted files; not merged into brigadier/0000c0de/session yet.\n"
        ),
        "{wait}"
    );
    let show = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["show", "a1b2"]));
    assert!(
        show.contains("\ngit: on brigadier/a1b2c3d4/task-2 · nothing uncommitted\n"),
        "{show}"
    );
    assert!(show.contains("ran in turn 3: 3 commands\n"), "{show}");
    let send = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["send", "e5f6", "--deny"],
    ));
    assert_eq!(send, "Denied what e5f6 asked.\n");
    let close = stdout(&brigadier(Some(&dir.0), Some(GRANT), &["close", "a1b2"]));
    assert_eq!(close, "Closed a1b2.\n");
    let queued = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &["queue", "9f8e7d6c", "worker", "a1b2"],
    ));
    assert_eq!(
        queued,
        "9f8e7d6c went to worker a1b2; the user sees where it went.\n"
    );
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
    // A round: each question numbered, its answer under it.
    let round = stdout(&brigadier(
        Some(&dir.0),
        Some(GRANT),
        &[
            "ask",
            "--question",
            "Ship it?",
            "--option",
            "Yes",
            "--option",
            "No",
            "--question",
            "What name?",
        ],
    ));
    assert_eq!(
        round,
        "1. Ship it?\n   Yes\n2. What name?\n   In my words\n"
    );
    let heard = daemon.join().unwrap();
    assert!(
        heard.contains(&serde_json::json!({"heard": true})),
        "the wait said its events were heard: {heard:#?}"
    );
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

#[test]
fn checkpoint_set_reads_stdin_and_show_prints_it() {
    fn checkpoint(request: &CliRequest) -> CliReply {
        let CliRequest::Tool {
            tool: Tool::Checkpoint,
            action,
            args,
        } = request
        else {
            panic!("expected a checkpoint call: {request:?}");
        };
        let text = match action.as_str() {
            "set" => {
                assert_eq!(args["text"], "Goal: ship.\nNext: test.\n");
                "Checkpoint saved.\n"
            }
            "show" => {
                assert_eq!(args, &serde_json::json!({}));
                "Goal: ship.\nNext: test.\n"
            }
            _ => panic!("unexpected action"),
        };
        CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: text.into(),
            images: vec![],
            saved: vec![],
        })
    }
    let dir = DataDir::new("checkpoint");
    let daemon = stand_in_daemon(&dir.0, 3, checkpoint);
    for words in [vec!["checkpoint", "set"], vec!["checkpoint", "set", "-"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brigadier"))
            .args(words)
            .env("BRIGADIER_DATA_DIR", &dir.0)
            .env(SESSION_ENV, SESSION)
            .env(GRANT_ENV, GRANT)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Goal: ship.\nNext: test.\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(stdout(&output), "Checkpoint saved.\n");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    let output = brigadier(Some(&dir.0), Some(GRANT), &["checkpoint", "show"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(stdout(&output), "Goal: ship.\nNext: test.\n");
    daemon.join().unwrap();

    for words in [
        vec!["checkpoint"],
        vec!["checkpoint", "show", "extra"],
        vec!["checkpoint", "set", "  "],
        vec!["checkpoint", "unknown"],
    ] {
        let output = brigadier(None, None, &words);
        assert_eq!(output.status.code(), Some(2), "{words:?}");
        assert!(output.stdout.is_empty());
    }
}

#[test]
fn ledger_add_prints_its_id_reads_stdin_and_show_close_and_check_pass_their_words() {
    fn ledger(request: &CliRequest) -> CliReply {
        let CliRequest::Tool {
            tool: Tool::Ledger,
            action,
            args,
        } = request
        else {
            panic!("expected a ledger call: {request:?}");
        };
        let text = match action.as_str() {
            "add" => {
                assert_eq!(args["text"], "Decision: keep tabs\n(source: message 1)\n");
                "L1\n"
            }
            "show" if args.get("open").is_some() => {
                assert_eq!(args, &serde_json::json!({"open": true}));
                "Open items (0):\n"
            }
            "show" => "- Decision: keep tabs (source: message 1)\n\n",
            "close" => {
                assert_eq!(args, &serde_json::json!({"id": "L1", "how": "asked again"}));
                "L2\n"
            }
            "check" => {
                assert!(args["file"].as_str().unwrap().ends_with("Cargo.toml"));
                assert!(args["text"].as_str().unwrap().contains("[package]"));
                "Cargo.toml against the ledger:\n"
            }
            _ => panic!("unexpected action"),
        };
        CliReply::ok(&ToolOutput {
            action: action.clone(),
            text: text.into(),
            images: vec![],
            saved: vec![],
        })
    }
    let dir = DataDir::new("ledger");
    let daemon = stand_in_daemon(&dir.0, 6, ledger);
    for words in [vec!["ledger", "add"], vec!["ledger", "add", "-"]] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_brigadier"))
            .args(words)
            .env("BRIGADIER_DATA_DIR", &dir.0)
            .env(SESSION_ENV, SESSION)
            .env(GRANT_ENV, GRANT)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"Decision: keep tabs\n(source: message 1)\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(stdout(&output), "L1\n");
        assert!(output.stderr.is_empty(), "{output:?}");
    }
    for (words, printed) in [
        (
            &["ledger", "show"][..],
            "- Decision: keep tabs (source: message 1)\n\n",
        ),
        (&["ledger", "show", "--open"][..], "Open items (0):\n"),
        (&["ledger", "close", "L1", "asked", "again"][..], "L2\n"),
        (
            &[
                "ledger",
                "check",
                concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"),
            ][..],
            "Cargo.toml against the ledger:\n",
        ),
    ] {
        let output = brigadier(Some(&dir.0), Some(GRANT), words);
        assert!(output.status.success(), "{output:?}");
        assert_eq!(stdout(&output), printed);
        assert!(output.stderr.is_empty());
    }
    daemon.join().unwrap();

    for words in [
        vec!["ledger"],
        vec!["ledger", "show", "extra"],
        vec!["ledger", "add", "  "],
        vec!["ledger", "unknown"],
        vec!["ledger", "close", "L1"],
        vec!["ledger", "check"],
        vec!["ledger", "check", "/nowhere/plan.md"],
    ] {
        let output = brigadier(None, None, &words);
        assert!(!output.status.success(), "{words:?}");
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8_lossy(&output.stderr).lines().count(),
            1,
            "{output:?}"
        );
    }
}

/// `brigadier context` is one front door (D13): each part goes to the daemon as one request
/// with its own arguments and prints the text the daemon wrote; arguments of another part are
/// refused before any call.
#[test]
fn context_sends_each_part_and_prints_its_text() {
    fn answer(request: &CliRequest) -> CliReply {
        let CliRequest::Context {
            part,
            name,
            grep,
            last,
            item,
        } = request
        else {
            panic!("expected a context call: {request:?}");
        };
        let mut output = context_sample(*part, name);
        output.text = format!(
            "{} name={name:?} grep={grep:?} last={last:?} item={item:?}\n",
            output.part
        );
        CliReply::ok(&output)
    }
    let cases: &[(&[&str], &str)] = &[
        (
            &["context"],
            "overview name=None grep=None last=None item=None\n",
        ),
        (
            &["context", "decisions"],
            "decisions name=None grep=None last=None item=None\n",
        ),
        (
            &["context", "skills", "tools:review"],
            "skills name=Some(\"tools:review\") grep=None last=None item=None\n",
        ),
        (
            &["context", "transcript", "--grep", "the cache"],
            "transcript name=None grep=Some(\"the cache\") last=None item=None\n",
        ),
        (
            &["context", "transcript", "--last", "5"],
            "transcript name=None grep=None last=Some(5) item=None\n",
        ),
        (
            &["context", "transcript", "--item", "u-1"],
            "transcript name=None grep=None last=None item=Some(\"u-1\")\n",
        ),
    ];
    let dir = DataDir::new("context");
    let daemon = stand_in_daemon(&dir.0, cases.len(), answer);
    for (words, printed) in cases {
        let output = brigadier(Some(&dir.0), Some(GRANT), words);
        assert!(output.status.success(), "{words:?}: {output:?}");
        assert_eq!(stdout(&output), *printed, "{words:?}");
    }
    daemon.join().unwrap();

    for words in [
        vec!["context", "nothing"],
        vec!["context", "brief", "extra"],
        vec!["context", "skills", "a", "b"],
        vec!["context", "ledger", "--grep", "x"],
        vec!["context", "--last", "3"],
        vec!["context", "transcript", "--last", "0"],
        vec!["context", "transcript", "--grep", "x", "--item", "u-1"],
    ] {
        let output = brigadier(None, None, &words);
        assert_eq!(output.status.code(), Some(2), "{words:?}: {output:?}");
        assert!(output.stdout.is_empty(), "{words:?}");
    }

    let help = stdout(&brigadier(None, None, &["context", "--help"]));
    for word in [
        "brief",
        "decisions",
        "open",
        "ledger",
        "checkpoint",
        "transcript",
        "skills",
        "mcps",
        "project",
        "routing",
        "after any compaction",
        "--grep TEXT",
        "--last N",
        "--item ID",
    ] {
        assert!(help.contains(word), "{word} missing from:\n{help}");
    }
}
