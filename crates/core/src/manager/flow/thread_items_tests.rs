//! The thread's own tool calls as transcript items (BUILD-PLAN B1b,
//! [`super::super::thread_items`]): a turn that edits two files and runs a command printing
//! more than its event keeps, in each CLI's event shapes, read back through
//! `listThreadItems`, and its rows on the board while the command runs.

use std::sync::{Arc, Mutex};

use brigadier_providers::{FileChange, FileChangeKind, ItemStatus, ProviderEvent, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::work::{
    FileStat, OrchestratorEntry, OrchestratorStepKind, OutputSource, ThreadItemKind,
};

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
/// a command whose output streams, longer than its event keeps. Both come back through
/// `listThreadItems` keyed to the turn and the request: the command with its whole output
/// and exit code, each file with its lines in and out and its diff. While the command runs
/// its row shows (Codex's with how much it printed) and the page returns it running, a Codex
/// command with its output so far. What streamed is not stored as it streams.
#[tokio::test]
async fn a_turns_own_edits_and_long_command_come_back_whole_as_transcript_items() {
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
        let live = flow
            .manager
            .thread_items(&flow.conversation, &request, None, None, 50)
            .await
            .unwrap();
        let so_far: Option<&str> = live.entries.iter().find_map(|entry| match &entry.event {
            ProviderEvent::CommandOutputDelta { item_id, text } if item_id == "cmd-1" => {
                Some(text.as_str())
            }
            _ => None,
        });
        assert!(
            live.entries.iter().any(|entry| matches!(&entry.event,
                ProviderEvent::Command { item_id, status: ItemStatus::InProgress, .. } if item_id == "cmd-1")),
            "{provider:?}: {live:?}"
        );
        match provider {
            ProviderKind::Claude => assert_eq!(so_far, None),
            _ => assert_eq!(so_far, Some(full.as_str())),
        }
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
                    log_seq,
                    ..
                } => {
                    assert_eq!(step.request_id.as_deref(), Some(request.as_str()));
                    assert_eq!(*of, Some(turn), "{provider:?}: {item_id}");
                    assert!(log_seq.is_some());
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

        // The items, as a worker's transcript holds them, keyed to the request and the turn.
        let page = flow
            .manager
            .thread_items(&flow.conversation, &request, Some(turn), None, 50)
            .await
            .unwrap();
        assert_eq!(page.next, None);
        assert_eq!(page.entries.len(), expected.len(), "{provider:?}: {page:?}");
        let mut diffs = Vec::new();
        for entry in &page.entries {
            match &entry.event {
                ProviderEvent::FileChanges {
                    changes, status, ..
                } => {
                    assert_eq!(*status, ItemStatus::Completed);
                    for change in changes {
                        diffs.push((
                            change.path.clone(),
                            change.added,
                            change.removed,
                            change.diff.is_some(),
                        ));
                    }
                }
                ProviderEvent::Command {
                    status,
                    exit_code,
                    output,
                    duration_ms,
                    cwd,
                    ..
                } => {
                    assert_eq!(*status, ItemStatus::Completed);
                    assert_eq!(*exit_code, Some(0));
                    // Whole, from the output store, past the event's clip or digest.
                    assert_eq!(output.as_deref(), Some(full.as_str()), "{provider:?}");
                    assert!(duration_ms.is_some_and(|ms| ms >= 0));
                    assert_eq!(cwd.is_some(), provider == ProviderKind::Codex);
                }
                other => panic!("{provider:?}: unexpected {other:?}"),
            }
        }
        assert_eq!(
            diffs,
            [
                ("/w/src/a.rs".to_owned(), Some(1), Some(1), true),
                ("/w/src/lib.rs".to_owned(), Some(2), Some(0), true),
            ],
            "{provider:?}"
        );
        // Another turn's are none of these; the pages follow on from each other.
        let other = flow
            .manager
            .thread_items(
                &flow.conversation,
                &request,
                Some(turn + 1_000_000),
                None,
                50,
            )
            .await
            .unwrap();
        assert!(other.entries.is_empty());
        let first = flow
            .manager
            .thread_items(&flow.conversation, &request, None, None, 1)
            .await
            .unwrap();
        assert_eq!(first.entries.len(), 1);
        let rest = flow
            .manager
            .thread_items(&flow.conversation, &request, None, first.next, 50)
            .await
            .unwrap();
        assert_eq!(rest.entries.len() + 1, expected.len());
        assert_eq!(rest.next, None);

        // Stored once: the streamed output never as it streamed; a Codex command's whole
        // output in the output store as it streamed, the event keeping the CLI's clip.
        let log = flow
            .core
            .list_orchestrator_log(&flow.conversation, None, 500)
            .await
            .unwrap();
        assert!(!log.entries.iter().any(|entry| matches!(
            &entry.entry,
            OrchestratorEntry::Provider {
                event: ProviderEvent::CommandOutputDelta { .. },
                ..
            }
        )));
        let sources: Vec<OutputSource> =
            board.outputs.values().map(|output| output.source).collect();
        match provider {
            ProviderKind::Claude => assert_eq!(sources, [OutputSource::Bash]),
            _ => assert_eq!(sources, [OutputSource::Shell]),
        }
        flow.stop().await;
    }
}
