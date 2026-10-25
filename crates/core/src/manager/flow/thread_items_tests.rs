//! The thread's own tool calls' rows (BUILD-PLAN B1b, [`super::super::thread_items`]): a turn
//! that edits two files and runs a command printing more than its event keeps, in each CLI's
//! event shapes, and its rows on the board while the command runs. The calls themselves are
//! read from the CLI's own session file (`version_fixture_tests`, #540).

use std::sync::{Arc, Mutex};

use brigadier_providers::{FileChange, FileChangeKind, ItemStatus, ProviderEvent, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::work::{FileStat, OrchestratorStepKind, OutputSource, ThreadItemKind};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// About 30 KB of a build's output.
fn long_output() -> String {
    (0..3000)
        .map(|n| format!("   Compiling crate-{n:04} v0.1.0\n"))
        .collect()
}

/// What a CLI's event keeps of a long output: its first 8 KiB, marked.
fn clipped(output: &str) -> String {
    format!("{}… [{} more bytes]", &output[..8192], output.len() - 8192)
}

fn edit(path: &str, kind: FileChangeKind, diff: &str) -> FileChange {
    FileChange::counted(path.into(), kind, Some(diff), 32 * 1024)
}

/// The thread's command row, by its item.
fn command_row(board: &Board) -> Option<&OrchestratorStepKind> {
    board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            kind @ OrchestratorStepKind::Tool { item_id, .. } if item_id == "cmd-1" => Some(kind),
            _ => None,
        })
}

/// A Claude turn edits two files (two `Edit` calls) and runs a `Bash` whose long output its
/// hook stored (the model got the digest); a Codex turn makes one patch of two files and runs
/// a command whose output streams, longer than its event keeps. Their rows are keyed to the
/// turn and the request, each file with its lines in and out. While the command runs its row
/// shows (Codex's with how much it printed).
#[tokio::test]
async fn a_turns_own_edits_and_long_command_are_rows_of_its_turn() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let full = long_output();
        let digest: Arc<Mutex<String>> = Arc::default();
        let sent = digest.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let held = release.clone();
        let streamed = full.clone();
        let flow = Flow::start(
            "thread-items",
            Options {
                thread: provider,
                ..Options::default()
            },
            script(move |turn| {
                let (digest, held, full) =
                    (sent.lock().unwrap().clone(), held.clone(), streamed.clone());
                async move {
                    let send = |event: ProviderEvent| {
                        let events = turn.events.clone();
                        async move { events.send(event).await.unwrap() }
                    };
                    let a = "-fn a() -> u32 { 1 }\n+fn a() -> u32 { 2 }\n";
                    let b = "+pub mod b;\n+pub mod c;\n";
                    match provider {
                        ProviderKind::Claude => {
                            for (item, path, kind, diff) in [
                                ("edit-1", "/w/src/a.rs", FileChangeKind::Update, a),
                                ("edit-2", "/w/src/lib.rs", FileChangeKind::Add, b),
                            ] {
                                for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                                    send(ProviderEvent::FileChanges {
                                        item_id: item.into(),
                                        changes: vec![edit(path, kind, diff)],
                                        status,
                                    })
                                    .await;
                                }
                            }
                        }
                        _ => {
                            let changes = vec![
                                edit(
                                    "/w/src/a.rs",
                                    FileChangeKind::Update,
                                    &format!("--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n{a}"),
                                ),
                                edit(
                                    "/w/src/lib.rs",
                                    FileChangeKind::Add,
                                    "pub mod b;\npub mod c;\n",
                                ),
                            ];
                            for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                                send(ProviderEvent::FileChanges {
                                    item_id: "patch-1".into(),
                                    changes: changes.clone(),
                                    status,
                                })
                                .await;
                            }
                        }
                    }
                    let (cwd, command) = match provider {
                        ProviderKind::Claude => (None, "cargo build".to_owned()),
                        _ => (
                            Some("/w".to_owned()),
                            "/bin/zsh -lc 'cargo build'".to_owned(),
                        ),
                    };
                    send(ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command: command.clone(),
                        cwd: cwd.clone(),
                        status: ItemStatus::InProgress,
                        exit_code: None,
                        output: None,
                        duration_ms: None,
                        kept: None,
                    })
                    .await;
                    // Codex streams the output as it comes; Claude's arrives at completion.
                    if provider == ProviderKind::Codex {
                        for part in full.as_bytes().chunks(4096) {
                            send(ProviderEvent::CommandOutputDelta {
                                item_id: "cmd-1".into(),
                                text: String::from_utf8(part.to_vec()).unwrap(),
                            })
                            .await;
                        }
                    }
                    held.notified().await;
                    let (output, duration_ms) = match provider {
                        ProviderKind::Claude => (digest, None),
                        _ => (clipped(&full), Some(12_000)),
                    };
                    send(ProviderEvent::Command {
                        item_id: "cmd-1".into(),
                        command,
                        cwd,
                        status: ItemStatus::Completed,
                        exit_code: Some(0),
                        output: Some(output),
                        duration_ms,
                        kept: None,
                    })
                    .await;
                    Reply::text("Built.")
                }
            }),
        )
        .await;
        if provider == ProviderKind::Claude {
            // What the thread's hook does with a long `Bash` output: stored whole, the model
            // gets the digest, which the result then holds.
            let (_, stored) = flow
                .manager
                .store_output(
                    &flow.conversation,
                    OutputSource::Bash,
                    "exit 0",
                    full.clone().into_bytes(),
                    true,
                )
                .await
                .unwrap();
            assert!(stored.len() < full.len());
            *digest.lock().unwrap() = stored;
        }
        flow.say("Bump a and add the modules, then build").await;

        // While the command runs: its row, and (Codex) how much it printed so far.
        let running = flow
            .until("the command runs", |board| {
                matches!(command_row(board), Some(OrchestratorStepKind::Tool {
                    status: ItemStatus::InProgress,
                    output_bytes,
                    ..
                }) if provider == ProviderKind::Claude || output_bytes.is_some_and(|bytes| bytes > 0))
            })
            .await;
        let Some(OrchestratorStepKind::Tool { item, turn, .. }) = command_row(&running) else {
            unreachable!()
        };
        assert_eq!(*item, Some(ThreadItemKind::Command), "{provider:?}");
        let turn = turn.expect("the row names its turn");
        let request = running
            .orchestrator_steps
            .iter()
            .find_map(|step| step.request_id.clone())
            .expect("the rows are the request's");
        release.notify_one();
        let board = flow.settled().await;

        // The rows: each call's kind, the files' lines in and out, all in the one turn.
        let rows: Vec<_> = board
            .orchestrator_steps
            .iter()
            .filter_map(|step| match &step.kind {
                OrchestratorStepKind::Tool {
                    item_id,
                    item,
                    files,
                    turn: of,
                    ..
                } => {
                    assert_eq!(step.request_id.as_deref(), Some(request.as_str()));
                    assert_eq!(*of, Some(turn), "{provider:?}: {item_id}");
                    Some((item_id.clone(), *item, files.clone()))
                }
                _ => None,
            })
            .collect();
        let stat = |path: &str, insertions: u32, deletions: u32, change| FileStat {
            path: path.into(),
            insertions,
            deletions,
            binary: false,
            change: Some(change),
            from: None,
        };
        let files = vec![
            stat("/w/src/a.rs", 1, 1, FileChangeKind::Update),
            stat("/w/src/lib.rs", 2, 0, FileChangeKind::Add),
        ];
        let expected = match provider {
            ProviderKind::Claude => vec![
                (
                    "edit-1".to_owned(),
                    Some(ThreadItemKind::Files),
                    Some(files[..1].to_vec()),
                ),
                (
                    "edit-2".to_owned(),
                    Some(ThreadItemKind::Files),
                    Some(files[1..].to_vec()),
                ),
                ("cmd-1".to_owned(), Some(ThreadItemKind::Command), None),
            ],
            _ => vec![
                (
                    "patch-1".to_owned(),
                    Some(ThreadItemKind::Files),
                    Some(files),
                ),
                ("cmd-1".to_owned(), Some(ThreadItemKind::Command), None),
            ],
        };
        assert_eq!(rows, expected, "{provider:?}");

        // A long output is stored once: a Codex command's whole output in the output store as
        // it streamed, past the CLI's clip; Claude's through the hook.
        let sources: Vec<OutputSource> =
            board.outputs.values().map(|output| output.source).collect();
        match provider {
            ProviderKind::Claude => assert_eq!(sources, [OutputSource::Bash]),
            _ => assert_eq!(sources, [OutputSource::Shell]),
        }
        flow.stop().await;
    }
}

/// A session's thread that reads a page and searches the web leaves a page read and a search
/// on the board, as a Chat's does: the context card's Sources and the Sources tab read them
/// (#415).
#[tokio::test]
async fn a_sessions_web_lookups_reach_its_board() {
    let flow = Flow::start(
        "session-web",
        Options::default(),
        script(|turn| async move {
            for (item, name, input) in [
                (
                    "fetch-1",
                    "WebFetch",
                    r#"{"url":"https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/date","prompt":"value format"}"#,
                ),
                ("search-1", "WebSearch", r#"{"query":"input type=date value"}"#),
            ] {
                for status in [ItemStatus::InProgress, ItemStatus::Completed] {
                    turn.events
                        .send(ProviderEvent::ToolCall {
                            item_id: item.into(),
                            name: name.into(),
                            input: Some(input.into()),
                            status,
                            output: (status == ItemStatus::Completed).then(|| "ok".into()),
                            kept: None,
                        })
                        .await
                        .unwrap();
                }
            }
            Reply::text("Looked it up.")
        }),
    )
    .await;
    flow.say("How does a date field report its value?").await;
    let board = flow
        .until("both lookups are on the board", |board| {
            board
                .orchestrator_steps
                .iter()
                .filter(|step| {
                    matches!(
                        step.kind,
                        OrchestratorStepKind::ReadPage { .. }
                            | OrchestratorStepKind::SearchedWeb { .. }
                    )
                })
                .count()
                == 2
        })
        .await;
    let web: Vec<&OrchestratorStepKind> = board
        .orchestrator_steps
        .iter()
        .map(|step| &step.kind)
        .filter(|kind| {
            matches!(
                kind,
                OrchestratorStepKind::ReadPage { .. } | OrchestratorStepKind::SearchedWeb { .. }
            )
        })
        .collect();
    assert_eq!(
        web,
        [
            &OrchestratorStepKind::ReadPage {
                url: "https://developer.mozilla.org/en-US/docs/Web/HTML/Element/input/date".into()
            },
            &OrchestratorStepKind::SearchedWeb {
                query: "input type=date value".into()
            },
        ]
    );
    flow.stop().await;
}
