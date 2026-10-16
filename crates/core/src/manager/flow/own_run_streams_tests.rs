//! Real child pipes through the thread's tools, gated by FIFOs instead of timing sleeps.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::ProjectPatch;
use crate::work::{BackgroundRunState, OrchestratorStepKind};

const SECRET: &str = "stream-secret-value";

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn parts(secret: bool) -> [String; 3] {
    [
        format!(
            "first:{}\n{}",
            "a".repeat(5000),
            if secret { "stream-" } else { "" }
        ),
        format!(
            "{}second:{}\nsecond-end\n",
            if secret { "secret-value\n" } else { "" },
            "b".repeat(6000)
        ),
        format!("last:{}\n", "c".repeat(3000)),
    ]
}

fn command(dir: &Path, secret: bool) -> String {
    for (number, part) in parts(secret).iter().enumerate() {
        std::fs::write(dir.join(format!("chunk-{number}")), part).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(dir.join(format!("gate-{number}")))
                .status()
                .unwrap()
                .success()
        );
    }
    let stages = (0..3)
        .map(|n| {
            format!(
                "IFS= read -r gate < {}; cat {}",
                quote(&dir.join(format!("gate-{n}"))),
                quote(&dir.join(format!("chunk-{n}")))
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    // The newer decoy has this same abbreviated detail but a different actual command.
    format!("# {}\n{stages}", "shared command prefix ".repeat(20))
}

fn row<'a>(board: &'a Board, item: &str) -> Option<&'a OrchestratorStepKind> {
    board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            kind @ OrchestratorStepKind::Tool { item_id, .. } if item_id == item => Some(kind),
            _ => None,
        })
}

fn streaming(tool: &'static str, background: bool) -> Script {
    Arc::new(move |turn: Turn| {
        Box::pin(async move {
            if turn.earlier > 0 {
                return Reply::text("The background command finished.");
            }
            let mut words = command(&turn.cwd, !background);
            if background {
                let (workspace, _) = turn
                    .host
                    .changes_checkout(&turn.conversation().unwrap())
                    .unwrap();
                words.push_str(&format!(
                    "; printf 'written by the background command\\n' > {}",
                    quote(&workspace.join("background-written.txt"))
                ));
            }
            let prefix = match turn.provider {
                ProviderKind::Claude => "mcp__brigadier__",
                _ => "brigadier/",
            };
            let name = format!("{prefix}{tool}");
            let decoy = format!("{}\nprintf decoy", words.lines().next().unwrap());
            for (item, command) in [("own-run", &words), ("decoy", &decoy)] {
                turn.emit(ProviderEvent::ToolCall {
                    kept: None,
                    item_id: item.into(),
                    name: name.clone(),
                    input: Some(json!({"command": command}).to_string()),
                    status: ItemStatus::InProgress,
                    output: None,
                })
                .await;
            }
            let id = turn.conversation().unwrap();
            tokio::time::timeout(super::PATIENCE, async {
                loop {
                    if row(&turn.host.core.board(&id).await.unwrap(), "decoy").is_some() {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let mut args = json!({"command": words, "timeout_secs": 120});
            if tool == "run_check" {
                args["rerun"] = json!(true);
            }
            let reply = turn.call(tool, args).await;
            assert!(!reply.is_error, "{}", reply.text);
            assert_eq!(
                reply.text.contains("[run-1 still runs"),
                background,
                "{}",
                reply.text
            );
            for (item, command, output) in [
                ("own-run", &words, reply.text.as_str()),
                ("decoy", &decoy, "not run"),
            ] {
                turn.emit(ProviderEvent::ToolCall {
                    kept: None,
                    item_id: item.into(),
                    name: name.clone(),
                    input: Some(json!({"command": command}).to_string()),
                    status: ItemStatus::Completed,
                    output: Some(output.into()),
                })
                .await;
            }
            Reply::text(if background {
                "[quiet]"
            } else {
                "The command finished."
            })
        })
    })
}

/// Nonblocking FIFO opens wait for the child to reach its read, without a sleeping test or
/// a blocked helper thread that could outlive a failed assertion.
async fn release(path: PathBuf) {
    tokio::time::timeout(super::PATIENCE, async {
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(nix::libc::O_NONBLOCK)
                .open(&path)
            {
                Ok(mut file) => {
                    file.write_all(b"continue\n").unwrap();
                    break;
                }
                Err(err) if err.raw_os_error() == Some(nix::libc::ENXIO) => {
                    tokio::task::yield_now().await
                }
                Err(err) => panic!("{}: {err}", path.display()),
            }
        }
    })
    .await
    .unwrap();
}

async fn live_text(flow: &Flow, request: &str, contains: &str) -> String {
    tokio::time::timeout(super::PATIENCE, async {
        loop {
            let page = flow
                .manager
                .thread_items(&flow.conversation, request, None, None, 100)
                .await
                .unwrap();
            if let Some(text) = page
                .entries
                .into_iter()
                .find_map(|entry| match entry.event {
                    ProviderEvent::CommandOutputDelta { item_id, text }
                        if item_id == "own-run" && text.contains(contains) =>
                    {
                        Some(text)
                    }
                    _ => None,
                })
            {
                return text;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}

async fn hide_secret(flow: &Flow) {
    std::fs::write(flow.repo.join(".env"), format!("TOKEN={SECRET}\n")).unwrap();
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let mut prefs = flow.core.project(&project).unwrap().prefs;
    prefs.secret_files = vec![".env".into()];
    flow.core
        .update_project(
            project,
            ProjectPatch {
                name: None,
                repos: None,
                prefs: Some(prefs),
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn own_run_streams_real_pipes_for_both_providers_and_checks() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        for tool in ["run", "run_check"] {
            let flow = Flow::start(
                "20261015-0226-rebuild-wow-w69-own-stream",
                Options {
                    thread: provider,
                    ..Default::default()
                },
                streaming(tool, false),
            )
            .await;
            hide_secret(&flow).await;
            super::super::background_runs::hold_foreground(
                &flow.conversation,
                Arc::new(tokio::sync::Notify::new()),
            );
            flow.say("Run the gated command.").await;
            let board = flow
                .until("the two command rows", |b| row(b, "decoy").is_some())
                .await;
            let request = board.latest_request().unwrap().id.clone();
            assert!(matches!(
                row(&board, "own-run"),
                Some(OrchestratorStepKind::Tool {
                    status: ItemStatus::InProgress,
                    output_bytes: None,
                    ..
                })
            ));
            let scratch = flow.manager.owned_dir("orch", &flow.conversation.0);
            release(scratch.join("gate-0")).await;
            let board = flow.until("live byte count", |b| matches!(row(b, "own-run"), Some(OrchestratorStepKind::Tool { output_bytes: Some(bytes), .. }) if *bytes > 0)).await;
            assert!(matches!(
                row(&board, "decoy"),
                Some(OrchestratorStepKind::Tool {
                    output_bytes: None,
                    ..
                })
            ));
            let first = live_text(&flow, &request, "first:").await;
            assert!(first.contains(&"a".repeat(5000)));
            assert!(!first.contains("stream-"), "a split secret prefix escaped");
            release(scratch.join("gate-1")).await;
            let mid = live_text(&flow, &request, "second-end").await;
            assert!(mid.len() > 8192);
            assert!(!mid.contains(SECRET));
            assert!(mid.contains("[redacted]"));
            release(scratch.join("gate-2")).await;
            let board = flow
                .until("the completed tool row", |b| {
                    matches!(
                        row(b, "own-run"),
                        Some(OrchestratorStepKind::Tool {
                            status: ItemStatus::Completed,
                            ..
                        })
                    )
                })
                .await;
            let all = parts(true).concat();
            assert!(
                matches!(row(&board, "own-run"), Some(OrchestratorStepKind::Tool { output_bytes: Some(bytes), .. }) if *bytes == all.len() as u64)
            );
            let page = flow
                .manager
                .thread_items(&flow.conversation, &request, None, None, 100)
                .await
                .unwrap();
            let whole = page
                .entries
                .into_iter()
                .find_map(|entry| match entry.event {
                    ProviderEvent::ToolCall {
                        item_id, output, ..
                    } if item_id == "own-run" => output,
                    _ => None,
                })
                .unwrap();
            assert!(whole.contains(&all.replace(SECRET, "[redacted]")));
            assert!(!whole.contains(SECRET));
            flow.stop().await;
        }
    }
}

#[tokio::test]
async fn own_run_streams_keep_one_buffer_through_background_handoff() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w69-background-stream",
            Options {
                thread: provider,
                ..Default::default()
            },
            streaming("run", true),
        )
        .await;
        flow.say("Run in the background.").await;
        flow.until("the starting rows", |b| row(b, "decoy").is_some())
            .await;
        let scratch = flow.manager.owned_dir("orch", &flow.conversation.0);
        release(scratch.join("gate-0")).await;
        let board = flow
            .until("the background handoff", |b| {
                b.background_runs
                    .get("run-1")
                    .is_some_and(|run| run.item_id.as_deref() == Some("own-run"))
                    && matches!(
                        row(b, "own-run"),
                        Some(OrchestratorStepKind::Tool {
                            status: ItemStatus::Completed,
                            ..
                        })
                    )
            })
            .await;
        assert_eq!(
            board.background_runs["run-1"].state,
            BackgroundRunState::Running
        );
        let first = parts(false)[0].clone();
        let (text, bytes) = flow
            .manager
            .read_background_run(&flow.conversation, "run-1")
            .await
            .unwrap();
        assert_eq!(text, first);
        assert_eq!(bytes, first.len() as u64);
        release(scratch.join("gate-1")).await;
        let (text, bytes) = tokio::time::timeout(super::PATIENCE, async {
            loop {
                let output = flow
                    .manager
                    .read_background_run(&flow.conversation, "run-1")
                    .await
                    .unwrap();
                if output.0.contains("second-end") {
                    return output;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let mid = parts(false)[..2].concat();
        assert_eq!(text, mid);
        assert_eq!(bytes, mid.len() as u64);
        assert!(bytes > first.len() as u64);
        release(scratch.join("gate-2")).await;
        flow.until("the final stored output", |b| {
            b.background_runs
                .get("run-1")
                .is_some_and(|run| matches!(run.state, BackgroundRunState::Finished { .. }))
        })
        .await;
        let board = flow.board().await;
        let changes = board.latest_request().unwrap().changes.as_ref().unwrap();
        assert_eq!(changes.stat.files.len(), 1);
        assert_eq!(changes.stat.files[0].path, "background-written.txt");
        assert_eq!(changes.held[0].path, "background-written.txt");
        let all = parts(false).concat();
        assert_eq!(
            flow.manager
                .read_background_run(&flow.conversation, "run-1")
                .await
                .unwrap(),
            (all.clone(), all.len() as u64)
        );
        flow.stop().await;
    }
}

/// What a background run printed before the user's Stop stays readable from its row.
#[tokio::test]
async fn own_run_streams_keep_a_stopped_background_runs_output() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w77-stopped-stream",
        Options::default(),
        streaming("run", true),
    )
    .await;
    flow.say("Run in the background.").await;
    flow.until("the starting rows", |b| row(b, "decoy").is_some())
        .await;
    let scratch = flow.manager.owned_dir("orch", &flow.conversation.0);
    release(scratch.join("gate-0")).await;
    flow.until("the background handoff", |b| {
        b.background_runs
            .get("run-1")
            .is_some_and(|run| run.item_id.as_deref() == Some("own-run"))
    })
    .await;
    let first = parts(false)[0].clone();
    tokio::time::timeout(super::PATIENCE, async {
        while flow
            .manager
            .read_background_run(&flow.conversation, "run-1")
            .await
            .unwrap()
            .0
            != first
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    flow.manager
        .stop_background_run(flow.conversation.clone(), "run-1".into())
        .await
        .unwrap();
    flow.until("the stopped run", |b| {
        b.background_runs
            .get("run-1")
            .is_some_and(|run| run.state == BackgroundRunState::Stopped)
    })
    .await;
    assert_eq!(
        flow.manager
            .read_background_run(&flow.conversation, "run-1")
            .await
            .unwrap(),
        (first.clone(), first.len() as u64)
    );
    flow.stop().await;
}
