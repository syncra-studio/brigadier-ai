//! Previews (THREAD-PLAN.md Q6): started by the thread in its workspace, in a process group of
//! their own under their own cleanup owner; kept across the thread's hibernation and CLI
//! restarts; stopped by `stop_preview`, the user's Stop, a merge, a workspace change, archive,
//! delete and Brigadier's quit; and swept at launch after a crash.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ApprovalDecision, Artifact};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::{Environment, Setup};
use crate::work::{ApprovalSubject, CardState, Preview, PreviewState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A preview that starts a second process in its group and keeps running, printing a line
/// first; the second process's pid goes to `pids`.
fn server(pids: &Path) -> String {
    format!(
        "sleep 600 & echo $! > {}; echo listening on 8123; wait",
        pids.display()
    )
}

/// What the thread was told by each tool call, in order.
type Replies = Arc<Mutex<Vec<(String, String)>>>;

/// A thread that does what the user's message says: `start <command>`, show the log, stop,
/// merge.
fn thread(replies: Replies) -> Script {
    script(move |turn| {
        let replies = replies.clone();
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            let input = turn.input.clone();
            let (tool, args) =
                if let Some(command) = input.lines().find_map(|line| line.strip_prefix("start ")) {
                    (
                        "start_preview",
                        json!({ "command": command, "name": "site" }),
                    )
                } else if input.contains("Please stop it.") {
                    ("stop_preview", json!({}))
                } else if input.contains("Please merge.") {
                    ("finish_session", json!({}))
                } else if input.contains("Show me the log.") {
                    ("preview_log", json!({}))
                } else {
                    return Reply::text("[quiet]");
                };
            let reply = turn.call(tool, args).await;
            replies
                .lock()
                .unwrap()
                .push((tool.to_owned(), reply.text.clone()));
            Reply::text(format!("Called {tool}."))
        }
    })
}

fn running(board: &Board) -> Vec<Preview> {
    board
        .sorted_previews()
        .into_iter()
        .filter(|preview| preview.state.is_running())
        .collect()
}

/// The session's worktree, as the conversation records it.
fn workspace(flow: &Flow) -> PathBuf {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => PathBuf::from(path),
        other => panic!("no session worktree: {other:?}"),
    }
}

fn alive(flow: &Flow, pid: u32) -> bool {
    flow.manager.runtime.platform().processes().is_alive(pid)
}

/// Waits until `pid` is gone (a killed process outside the daemon's children is reaped by the
/// system a moment later).
async fn gone(flow: &Flow, pid: u32) {
    for _ in 0..100 {
        if !alive(flow, pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} still runs");
}

fn child_pid(pids: &Path) -> u32 {
    std::fs::read_to_string(pids)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Starts the preview with `say`, and waits for it to run; returns it and the pid of the
/// process it started.
async fn start(flow: &Flow, pids: &Path) -> (Preview, u32) {
    let _ = std::fs::remove_file(pids);
    let before = flow.board().await.previews.len();
    flow.say(&format!("start {}", server(pids))).await;
    let board = flow
        .until("the preview to run", |board| {
            board.previews.len() > before && pids.exists() && !running(board).is_empty()
        })
        .await;
    let preview = running(&board).pop().unwrap();
    (preview, child_pid(pids))
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("brigadier-preview-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A preview runs in the session's worktree, leads its own process group and is recorded under
/// its own owner, never the thread's. The thread's hibernation (its CLI closed, and the
/// thread owner's processes ended, its scratch folder swept by `end_in_dir`) and a fresh CLI
/// leave it running; `preview_log` shows its output with a reference to the whole log, and
/// `stop_preview` ends it and what it started.
#[tokio::test]
async fn a_preview_runs_in_the_workspace_and_outlives_the_threads_cli() {
    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-runs", Options::default(), thread(replies.clone())).await;
    let tmp = scratch("runs");
    let pids = tmp.join("child.pid");
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    let pid = preview.pid.unwrap();
    let processes = flow.manager.runtime.platform().processes();
    let worktree = workspace(&flow).canonicalize().unwrap();
    assert_eq!(PathBuf::from(&preview.workdir), worktree);
    assert_eq!(preview.workspace, workspace(&flow).to_string_lossy());
    assert_eq!(processes.group_of(pid), Some(pid), "it leads its own group");
    assert_eq!(
        processes.group_of(child),
        Some(pid),
        "what it starts stays in its group"
    );
    assert!(
        processes.in_dir(&worktree).unwrap().contains(&pid),
        "its cwd is the worktree"
    );
    let started = replies.lock().unwrap()[0].1.clone();
    assert!(
        started.contains("preview-1 \"site\" is running"),
        "{started}"
    );
    assert!(started.contains("listening on 8123"), "{started}");

    // Its own owner records it; the thread's owner records nothing in the workspace.
    let ledger = flow.manager.runtime.ledger();
    let owner = format!("preview:{}", flow.conversation);
    assert!(
        ledger
            .artifacts(&owner)
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Process { pid: p, .. } if *p == pid))
    );
    let thread_owner = format!("orch:{}", flow.conversation);
    let held = ledger.artifacts(&thread_owner);
    assert!(
        held.iter()
            .any(|artifact| matches!(artifact, Artifact::ProcessesIn { .. })),
        "the thread's scratch folder is swept: {held:?}"
    );
    for artifact in &held {
        if let Artifact::ProcessesIn { dir } | Artifact::ScratchDir { path: dir } = artifact {
            assert!(
                !worktree.starts_with(dir) && !Path::new(dir).starts_with(&worktree),
                "{dir}"
            );
        }
    }

    // Hibernation: the thread's CLI closes and its owner's processes end (`end_in_dir` on its
    // scratch folder included); the preview runs on.
    flow.manager
        .hibernate(flow.conversation.clone())
        .await
        .unwrap();
    ledger.end_processes(&thread_owner).await;
    assert!(alive(&flow, pid) && alive(&flow, child));
    assert!(running(&flow.board().await).len() == 1);

    // A fresh CLI (as after a rebirth or a fallback, which close the old one) reads its log.
    flow.say("Show me the log.").await;
    flow.until("the log", |_| replies.lock().unwrap().len() == 2)
        .await;
    let log = replies.lock().unwrap()[1].1.clone();
    assert!(
        log.starts_with("[preview-1 \"site\" running, pid "),
        "{log}"
    );
    assert!(log.contains("full log: read_artifact out-"), "{log}");
    assert!(log.ends_with("listening on 8123"), "{log}");
    let alias = log
        .split("read_artifact ")
        .nth(1)
        .unwrap()
        .split(',')
        .next()
        .unwrap()
        .to_owned();
    let (blob, _) = flow
        .manager
        .output_blob(&flow.conversation, &alias)
        .await
        .unwrap()
        .unwrap();
    let (bytes, _) = flow.core.read_blob_range(blob, 0, 1_000).await.unwrap();
    assert_eq!(bytes, b"listening on 8123\n");
    assert!(alive(&flow, pid));

    flow.say("Please stop it.").await;
    flow.until("the stop", |_| replies.lock().unwrap().len() == 3)
        .await;
    let stopped = replies.lock().unwrap()[2].1.clone();
    assert_eq!(
        stopped,
        "preview-1 \"site\": stopped (stopped by the thread)"
    );
    gone(&flow, pid).await;
    gone(&flow, child).await;
    let board = flow.board().await;
    let ended = &board.previews["preview-1"];
    assert_eq!(
        ended.state,
        PreviewState::Stopped {
            reason: "stopped by the thread".into()
        }
    );
    assert!(ended.ended_at_ms.is_some() && ended.log.is_some());
    assert!(
        !ledger
            .artifacts(&owner)
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Process { .. })),
        "forgotten once ended"
    );
    flow.stop().await;
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// A preview that ends on its own reads as exited with its code, and `start_preview` says so.
#[tokio::test]
async fn a_preview_that_ends_at_once_says_how() {
    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-exits", Options::default(), thread(replies.clone())).await;
    flow.say("start echo port taken >&2; exit 3").await;
    flow.settled().await;
    let reply = replies.lock().unwrap()[0].1.clone();
    assert!(reply.starts_with("preview-1 ended at once."), "{reply}");
    assert!(reply.contains("ended on its own (exit 3)"), "{reply}");
    assert!(reply.ends_with("port taken"), "{reply}");
    let board = flow.board().await;
    assert_eq!(
        board.previews["preview-1"].state,
        PreviewState::Exited {
            code: Some(3),
            status: "exit 3".into()
        }
    );
    flow.stop().await;
}

/// The user's Stop, a change of the thread's workspace, and the merge of the session branch
/// each stop the running previews.
#[tokio::test]
async fn the_users_stop_a_workspace_change_and_a_merge_stop_previews() {
    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-stops", Options::default(), thread(replies.clone())).await;
    let tmp = scratch("stops");
    let pids = tmp.join("child.pid");

    // The user's Stop.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    gone(&flow, preview.pid.unwrap()).await;
    gone(&flow, child).await;
    assert_eq!(
        flow.board().await.previews[&preview.id].state,
        PreviewState::Stopped {
            reason: "stopped by the user".into()
        }
    );
    flow.manager
        .resume_queue(flow.conversation.clone())
        .await
        .unwrap();

    // The workspace moves (as when an overnight run starts): its previews stop at the next
    // turn.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        environment:
            Environment::NewWorktree {
                base,
                branch,
                path,
                start: from,
            },
        permission,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
    }) = conversation.setup.clone()
    else {
        panic!("a new-worktree session");
    };
    let moved = tmp.join("elsewhere");
    std::fs::create_dir_all(&moved).unwrap();
    let setup = |path: Option<String>| Setup::Session {
        repo: repo.clone(),
        environment: Environment::NewWorktree {
            base: base.clone(),
            branch: branch.clone(),
            path,
            start: from.clone(),
        },
        permission,
        orchestrator: orchestrator.clone(),
        workers_see_uncommitted,
        plan_mode,
    };
    flow.core
        .set_setup(
            flow.conversation.clone(),
            setup(Some(moved.to_string_lossy().into_owned())),
        )
        .await
        .unwrap();
    flow.manager.stop_moved_previews(&flow.conversation).await;
    gone(&flow, preview.pid.unwrap()).await;
    gone(&flow, child).await;
    assert_eq!(
        flow.board().await.previews[&preview.id].state,
        PreviewState::Stopped {
            reason: "the thread's workspace changed".into()
        }
    );
    flow.core
        .set_setup(flow.conversation.clone(), setup(path.clone()))
        .await
        .unwrap();

    // The merge: the thread committed in the worktree, asked to merge, the user approved.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    let worktree = workspace(&flow);
    std::fs::write(worktree.join("page.html"), "<h1>Hi</h1>\n").unwrap();
    super::git(&worktree, &["add", "page.html"]);
    super::git(&worktree, &["commit", "-q", "-m", "Add the page"]);
    flow.say("Please merge.").await;
    let board = flow
        .until("the merge card", |board| {
            board.approvals.values().any(|card| {
                card.state == CardState::Pending
                    && matches!(card.subject, ApprovalSubject::FinishSession { .. })
            })
        })
        .await;
    assert!(alive(&flow, preview.pid.unwrap()), "asking doesn't stop it");
    let card = board
        .approvals
        .values()
        .find(|card| card.state == CardState::Pending)
        .unwrap()
        .id
        .clone();
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    gone(&flow, preview.pid.unwrap()).await;
    gone(&flow, child).await;
    let board = flow
        .until("the merge's stop recorded", |board| {
            !board.previews[&preview.id].state.is_running()
        })
        .await;
    assert_eq!(
        board.previews[&preview.id].state,
        PreviewState::Stopped {
            reason: "the session was merged".into()
        }
    );
    flow.stop().await;
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// A Stop that comes while a preview is starting refuses that start rather than leaving it
/// running past the Stop; and a log snapshot that lands after a preview ended keeps its end.
#[tokio::test]
async fn a_stop_during_a_start_refuses_it_and_a_late_log_snapshot_keeps_the_end() {
    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-races", Options::default(), thread(replies.clone())).await;
    let tmp = scratch("races");
    let pids = tmp.join("child.pid");
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;

    // Another start holds the numbering when the thread asks for one more, and the user's
    // Stop comes before it gets its turn.
    let held = flow.manager.previews.starting.clone().lock_owned().await;
    let (manager, id) = (flow.manager.clone(), flow.conversation.clone());
    let late = tokio::spawn(async move {
        let args = crate::tools::StartPreview {
            command: "sleep 600".into(),
            name: None,
            env: None,
            workdir: None,
        };
        manager.start_preview(&id, args).await
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (manager, id) = (flow.manager.clone(), flow.conversation.clone());
    let stop = tokio::spawn(async move { manager.stop_previews(&id, "stopped by the user").await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(held);
    let refused = late.await.unwrap().expect_err("the late start is refused");
    assert!(
        refused
            .to_string()
            .contains("stopped while this one was starting"),
        "{refused}"
    );
    stop.await.unwrap();
    gone(&flow, preview.pid.unwrap()).await;
    gone(&flow, child).await;
    let board = flow.board().await;
    assert!(running(&board).is_empty());
    assert_eq!(board.previews.len(), 1, "no second preview was started");

    // The thread's next start works again.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    let live = flow
        .manager
        .previews
        .get(&flow.conversation, &preview.id)
        .unwrap();
    flow.manager
        .stop_previews(&flow.conversation, "stopped by the user")
        .await;
    gone(&flow, child).await;
    let ended = flow.board().await.previews[&preview.id].clone();
    assert!(!ended.state.is_running());
    flow.manager.record_log(&live, "out-stale").await;
    assert_eq!(flow.board().await.previews[&preview.id], ended);
    flow.stop().await;
    std::fs::remove_dir_all(&tmp).unwrap();
}

/// Archive and delete stop the session's previews before its worktree goes, and remove their
/// log folder.
#[tokio::test]
async fn archive_and_delete_stop_previews() {
    for delete in [false, true] {
        let replies: Replies = Arc::default();
        let flow = Flow::start("preview-closes", Options::default(), thread(replies)).await;
        let tmp = scratch("closes");
        let pids = tmp.join("child.pid");
        let (preview, child) = start(&flow, &pids).await;
        flow.settled().await;
        let logs = flow.manager.owned_dir("previews", &flow.conversation.0);
        assert!(logs.exists());
        if delete {
            flow.manager
                .delete(flow.conversation.clone())
                .await
                .unwrap();
        } else {
            flow.manager
                .archive(flow.conversation.clone())
                .await
                .unwrap();
        }
        gone(&flow, preview.pid.unwrap()).await;
        gone(&flow, child).await;
        for _ in 0..100 {
            if !logs.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(!logs.exists(), "the log folder goes (delete: {delete})");
        if !delete {
            assert_eq!(
                flow.board().await.previews[&preview.id].state,
                PreviewState::Stopped {
                    reason: "the session closed".into()
                }
            );
        }
        flow.stop().await;
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

/// Brigadier's quit stops its previews; after a crash, the launch sweep ends the process group
/// a previous daemon left running, and its record reads as stopped.
#[tokio::test]
async fn quit_stops_previews_and_the_launch_sweep_ends_a_crashs_leftovers() {
    let replies: Replies = Arc::default();
    let mut flow = Flow::start("preview-quit", Options::default(), thread(replies)).await;
    let tmp = scratch("quit");
    let pids = tmp.join("child.pid");
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    flow.restart().await;
    assert!(!alive(&flow, preview.pid.unwrap()));
    gone(&flow, child).await;
    assert_eq!(
        flow.board().await.previews[&preview.id].state,
        PreviewState::Stopped {
            reason: "Brigadier quit".into()
        }
    );

    // A crash: a preview group recorded as running that no daemon watches any more.
    let platform = flow.manager.runtime.platform().clone();
    let mut spec = flow.manager.runtime.cli_env().spec(Path::new("/bin/sh"));
    let _ = std::fs::remove_file(&pids);
    spec.args = vec!["-c".into(), server(&pids).into()];
    spec.cwd = Some(workspace(&flow));
    let mut command = platform.processes().piped_command(&spec);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let leftover = command.spawn().unwrap();
    let pid = leftover.id();
    for _ in 0..100 {
        if pids.exists() && !std::fs::read_to_string(&pids).unwrap().trim().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let child = child_pid(&pids);
    let owner = format!("preview:{}", flow.conversation);
    flow.manager
        .runtime
        .ledger()
        .record(
            &owner,
            Artifact::Process {
                pid,
                started_at_ms: platform.processes().start_time_ms(pid).ok(),
            },
        )
        .await
        .unwrap();
    let mut crashed = preview.clone();
    crashed.id = "preview-2".into();
    crashed.pid = Some(pid);
    crashed.state = PreviewState::Running;
    crashed.ended_at_ms = None;
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::PreviewUpdated { preview: crashed }],
        )
        .await
        .unwrap();
    flow.restart().await;
    // The sweep killed it; reap it here, as its parent.
    let reaped = tokio::task::spawn_blocking(move || {
        let mut leftover = leftover;
        leftover.wait()
    })
    .await
    .unwrap()
    .unwrap();
    assert!(!reaped.success());
    gone(&flow, child).await;
    assert_eq!(
        flow.board().await.previews["preview-2"].state,
        PreviewState::Stopped {
            reason: "Brigadier quit".into()
        }
    );
    assert!(flow.manager.runtime.ledger().artifacts(&owner).is_empty());
    flow.stop().await;
    std::fs::remove_dir_all(&tmp).unwrap();
}
