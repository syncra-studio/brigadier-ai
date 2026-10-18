//! Previews (THREAD-PLAN.md Q6): started by the thread in its workspace, in a process group of
//! their own under their own cleanup owner; kept across the thread's hibernation and CLI
//! restarts; stopped by `stop_preview`, the user's Stop, a merge, a workspace change, archive,
//! delete and Brigadier's quit; and swept at launch after a crash.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_providers::Artifact;
use serde_json::json;

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::board::Board;
use crate::model::{Environment, Setup};
use crate::work::{Preview, PreviewState};

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
                } else if let Some(command) = input
                    .lines()
                    .find_map(|line| line.strip_prefix("start-app "))
                {
                    (
                        "start_preview",
                        json!({ "command": command, "name": "app", "kind": "app" }),
                    )
                } else if let Some(command) = input
                    .lines()
                    .find_map(|line| line.strip_prefix("start-for-user "))
                {
                    (
                        "start_preview",
                        json!({ "command": command, "name": "site", "purpose": "user" }),
                    )
                } else if input.contains("Please stop it.") {
                    ("stop_preview", json!({}))
                } else if input.contains("Please merge.") {
                    ("finish_session", json!({ "user_words": "Please merge." }))
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

/// Waits until `pid`, already killed, is gone (one outside the daemon's children is reaped by
/// the system a moment later).
async fn gone(flow: &Flow, pid: u32) {
    super::eventually(&format!("process {pid} to go"), || !alive(flow, pid)).await;
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
    start_as(flow, "start", pids).await
}

/// [`start`] with the message's own verb (`start-for-user` asks with `purpose: "user"`).
async fn start_as(flow: &Flow, verb: &str, pids: &Path) -> (Preview, u32) {
    let _ = std::fs::remove_file(pids);
    let before = flow.board().await.previews.len();
    flow.say(&format!("{verb} {}", server(pids))).await;
    let board = flow
        .until("the preview to run", |board| {
            // The shell makes the file before it writes the pid in.
            board.previews.len() > before
                && std::fs::read_to_string(pids).is_ok_and(|pid| !pid.trim().is_empty())
                && !running(board).is_empty()
        })
        .await;
    let preview = running(&board)
        .into_iter()
        .max_by_key(|preview| preview.started_at_ms)
        .unwrap();
    (preview, child_pid(pids))
}

fn scratch(name: &str) -> Scratch {
    Scratch::new(&format!("preview-{name}"))
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
        started.contains("preview-1 \"site\" is starting, not ready yet"),
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
    flow.settled().await;
    flow.say("Show me the log.").await;
    flow.until("the log", |_| replies.lock().unwrap().len() == 2)
        .await;
    let log = replies.lock().unwrap()[1].1.clone();
    assert!(
        log.starts_with("[preview-1 \"site\" starting, not ready yet ("),
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

    flow.settled().await;
    flow.say("Please stop it.").await;
    flow.until("the stop", |_| replies.lock().unwrap().len() == 3)
        .await;
    let stopped = replies.lock().unwrap()[2].1.clone();
    assert!(
        stopped.starts_with(
            "preview-1 \"site\": stopped (stopped by the thread); removed from the user's \
             list, its log: read_artifact out-"
        ),
        "{stopped}"
    );
    gone(&flow, pid).await;
    gone(&flow, child).await;
    // The thread's stop clears it, as the user's Clear finished would.
    assert!(flow.board().await.previews.is_empty());
    assert!(
        !ledger
            .artifacts(&owner)
            .iter()
            .any(|artifact| matches!(artifact, Artifact::Process { .. })),
        "forgotten once ended"
    );
    flow.stop().await;
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

/// A preview the user stopped from its chip reaches the thread as a `[preview]` note with the
/// user's next message, so it never answers "already running"; one that ended on its own after
/// its start answered "running" reaches it at once, with its last output line, to report in
/// the same turn (SESSION-FIXES-PLAN.md Q9).
#[tokio::test]
async fn the_thread_hears_of_a_preview_the_user_stopped_or_that_ended() {
    let replies: Replies = Arc::default();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let inner = thread(replies);
    let heard = inputs.clone();
    let flow = Flow::start(
        "preview-notes",
        Options::default(),
        script(move |turn| {
            if turn.is_orchestrator() {
                heard.lock().unwrap().push(turn.input.clone());
            }
            inner(turn)
        }),
    )
    .await;
    let tmp = scratch("notes");
    let pids = tmp.join("child.pid");

    // The chip's Stop.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    flow.manager
        .stop_preview(flow.conversation.clone(), Some(preview.id.clone()))
        .await
        .unwrap();
    gone(&flow, child).await;
    assert!(!flow.board().await.previews.contains_key(&preview.id));
    flow.say("Is the site up?").await;
    flow.settled().await;
    let last = inputs.lock().unwrap().last().cloned().unwrap();
    assert!(last.contains("Is the site up?"), "{last}");
    assert!(
        last.contains(&format!(
            "[preview] {} \"site\" was stopped by the user",
            preview.id
        )),
        "{last}"
    );

    assert!(
        last.contains("removed; its ID is no longer available"),
        "{last}"
    );
    assert!(last.contains("read_artifact"), "{last}");

    // An end on its own, after the start answered.
    flow.say("start sleep 2; echo 'Error: port 3000 is taken' >&2; exit 4")
        .await;
    let heard = inputs.clone();
    flow.until("the thread to hear the exit", move |_| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[preview ended: exit 4]"))
    })
    .await;
    flow.idle().await;
    let ended = inputs
        .lock()
        .unwrap()
        .iter()
        .find(|input| input.contains("[preview ended: exit 4]"))
        .cloned()
        .unwrap();
    assert!(
        ended.contains("Its last output line: Error: port 3000 is taken"),
        "{ended}"
    );
    assert!(
        ended.contains("Tell the user in this turn what failed")
            && ended.contains("if you already told them, reply [quiet]"),
        "{ended}"
    );
    flow.say("And now?").await;
    flow.settled().await;
    let last = inputs.lock().unwrap().last().cloned().unwrap();
    assert!(
        !last.contains("was stopped by the user") && !last.contains("ended on its own"),
        "told once: {last}"
    );
    let board = flow.board().await;
    let ids = board.previews.keys().cloned().collect::<Vec<_>>();
    let aliases = board
        .previews
        .values()
        .filter_map(|p| p.log.clone())
        .collect::<Vec<_>>();
    flow.manager
        .clear_previews(flow.conversation.clone())
        .await
        .unwrap();
    flow.say("Where are those previews?").await;
    flow.settled().await;
    let last = inputs.lock().unwrap().last().cloned().unwrap();
    assert!(last.contains("The user cleared"), "{last}");
    assert!(
        last.contains("stored logs remain readable through read_artifact"),
        "{last}"
    );
    for id in ids {
        assert!(last.contains(&id), "{last}");
    }
    for alias in aliases {
        assert!(last.contains(&format!("read_artifact {alias}")), "{last}");
    }
    flow.stop().await;
}

/// The user's Stop, a change of the thread's workspace and the merge of the session's work each
/// stop the running previews: the merge's because they showed the work it took (#289), though
/// the workspace stays; its result tells the thread. One the user asked for runs on past the
/// merge (#335).
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
    assert!(!flow.board().await.previews.contains_key(&preview.id));
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

    // The merge: the thread committed in the worktree, and the user asked for the merge. The
    // thread started one preview to show its change and another the user asked for.
    let (preview, child) = start(&flow, &pids).await;
    flow.settled().await;
    let user_pids = tmp.join("user-child.pid");
    let (asked, asked_child) = start_as(&flow, "start-for-user", &user_pids).await;
    flow.settled().await;
    let worktree = workspace(&flow);
    std::fs::write(worktree.join("page.html"), "<h1>Hi</h1>\n").unwrap();
    super::git(&worktree, &["add", "page.html"]);
    super::git(&worktree, &["commit", "-q", "-m", "Add the page"]);
    flow.say("Please merge.").await;
    flow.until("the merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
    })
    .await;
    let board = flow.settled().await;
    gone(&flow, preview.pid.unwrap()).await;
    gone(&flow, child).await;
    assert_eq!(
        board.previews[&preview.id].state,
        PreviewState::Stopped {
            reason: "the work it showed is merged".into()
        }
    );
    let (tool, reply) = replies.lock().unwrap().last().cloned().unwrap();
    assert_eq!(tool, "finish_session");
    assert!(
        reply.contains(&format!(
            "Your preview {} stopped, as the work it showed is merged.",
            preview.id
        )),
        "{reply}"
    );
    assert!(board.previews[&asked.id].state.is_running());
    assert!(
        alive(&flow, asked_child),
        "the asked-for preview's child still runs"
    );
    assert!(!reply.contains(&asked.id), "{reply}");
    flow.manager
        .stop_preview(flow.conversation.clone(), Some(asked.id.clone()))
        .await
        .unwrap();
    gone(&flow, asked_child).await;
    flow.stop().await;
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
            kind: None,
            purpose: None,
        };
        manager.start_preview(&id, args).await
    });
    // It waits for the numbering (its own handle on the lock is the third).
    super::eventually("the late start to wait for the numbering", || {
        Arc::strong_count(&flow.manager.previews.starting) == 3
    })
    .await;
    let stops = flow.manager.previews.stops_of(&flow.conversation);
    let (manager, id) = (flow.manager.clone(), flow.conversation.clone());
    let stop = tokio::spawn(async move {
        manager
            .stop_previews(&id, "stopped by the user")
            .await
            .unwrap()
    });
    super::eventually("the stop to be counted", || {
        flow.manager.previews.stops_of(&flow.conversation) != stops
    })
    .await;
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
        .await
        .unwrap();
    gone(&flow, child).await;
    let ended = flow.board().await.previews[&preview.id].clone();
    assert!(!ended.state.is_running());
    flow.manager.record_log(&live, "out-stale").await;
    assert_eq!(flow.board().await.previews[&preview.id], ended);
    flow.stop().await;
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
        super::eventually(&format!("the log folder to go (delete: {delete})"), || {
            !logs.exists()
        })
        .await;
        if !delete {
            assert_eq!(
                flow.board().await.previews[&preview.id].state,
                PreviewState::Stopped {
                    reason: "the session closed".into()
                }
            );
        }
        flow.stop().await;
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
    super::eventually("the leftover's pid", || {
        std::fs::read_to_string(&pids).is_ok_and(|pid| !pid.trim().is_empty())
    })
    .await;
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
}

/// The actual start path provisions and tracks a short temporary directory for Chromium.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn a_sandboxed_preview_gets_a_private_writable_temp_directory() {
    let replies = Replies::default();
    let flow = Flow::start(
        "preview-temp",
        Options {
            permission: crate::model::PermissionLevel::ApproveForMe,
            ..Options::default()
        },
        thread(replies.clone()),
    )
    .await;
    flow.say("start test -d \"$MAC_CHROMIUM_TMPDIR\" && touch \"$MAC_CHROMIUM_TMPDIR/probe\" && echo \"$MAC_CHROMIUM_TMPDIR\"").await;
    flow.until("the temporary folder", |_| {
        !replies.lock().unwrap().is_empty()
    })
    .await;
    let reply = replies.lock().unwrap()[0].1.clone();
    let dir = PathBuf::from(reply.lines().last().unwrap());
    assert!(
        dir.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("brigadier-pv-"),
        "{reply}"
    );
    assert!(dir.join("probe").is_file(), "{reply}");
    let owner = format!("preview:{}", flow.conversation);
    let ledger = flow.manager.runtime.ledger();
    assert!(ledger.artifacts(&owner).contains(&Artifact::ScratchDir {
        path: dir.to_string_lossy().into_owned()
    }));
    assert!(ledger.dispose(&owner).await.is_clean());
    assert!(!dir.exists());
    flow.stop().await;
}

/// Data belongs to the session: restarts retain it, other sessions cannot adopt it,
/// archive sweeps it, and a failed OS spawn rolls back only the newly created folder.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn preview_data_is_session_owned_and_failed_spawns_leave_no_folder() {
    // Under /tmp, as BRIGADIER_DATA_DIR must be, even when the temp directory isn't.
    let tmp = Scratch::in_tmp("preview-data-lifecycle");
    let data = tmp.join("app-data");
    let failed = tmp.join("failed-data");
    let replies = Replies::default();
    let make_thread = |replies: Replies| {
        let data = data.clone();
        let failed = failed.clone();
        script(move |turn| {
            let (replies, data, failed) = (replies.clone(), data.clone(), failed.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let fail = turn.input.contains("Fail the spawn");
                let path = if fail { &failed } else { &data };
                let mut env = json!({ "BRIGADIER_DATA_DIR": path });
                if fail {
                    // An embedded NUL reaches Command::spawn and is rejected by the OS API.
                    env["BAD"] = json!("\0");
                }
                let result = turn.call("start_preview", json!({
                    "command": "if test -f \"$BRIGADIER_DATA_DIR/probe\"; then cat \"$BRIGADIER_DATA_DIR/probe\"; else echo retained > \"$BRIGADIER_DATA_DIR/probe\"; echo created; fi",
                    "env": env,
                })).await;
                replies
                    .lock()
                    .unwrap()
                    .push(("start_preview".into(), result.text));
                Reply::text("Called start_preview.")
            }
        })
    };
    let options = Options {
        permission: if cfg!(target_os = "macos") {
            crate::model::PermissionLevel::ApproveForMe
        } else {
            crate::model::PermissionLevel::FullAccess
        },
        ..Options::default()
    };
    let flow = Flow::start("preview-data", options, make_thread(replies.clone())).await;
    flow.say("Start the preview.").await;
    flow.until("first data start", |_| replies.lock().unwrap().len() == 1)
        .await;
    flow.settled().await;
    assert!(
        data.join("probe").is_file(),
        "{:?}",
        replies.lock().unwrap()
    );
    let ledger = flow.manager.runtime.ledger();
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(&data).unwrap();
    let artifact = Artifact::PreviewDataDir {
        path: data.canonicalize().unwrap().to_string_lossy().into_owned(),
        identity: Some((meta.dev(), meta.ino())),
    };
    assert!(
        ledger
            .artifacts(&format!("preview-data:{}", flow.conversation))
            .contains(&artifact)
    );
    assert!(
        !ledger
            .artifacts(&format!("preview:{}", flow.conversation))
            .contains(&artifact)
    );

    flow.say("Start it again.").await;
    flow.until("reused data", |_| replies.lock().unwrap().len() == 2)
        .await;
    flow.settled().await;
    assert!(
        replies.lock().unwrap()[1].1.contains("retained"),
        "{:?}",
        replies.lock().unwrap()
    );

    let other_replies = Replies::default();
    let other = Flow::start(
        "preview-data-other",
        Options {
            permission: crate::model::PermissionLevel::ApproveForMe,
            ..Options::default()
        },
        make_thread(other_replies.clone()),
    )
    .await;
    other.say("Start the preview.").await;
    other
        .until("refused foreign data", |_| {
            !other_replies.lock().unwrap().is_empty()
        })
        .await;
    assert!(
        other_replies.lock().unwrap()[0]
            .1
            .contains("pick a fresh name")
    );
    other.stop().await;

    flow.say("Fail the spawn.").await;
    flow.until("failed spawn", |_| replies.lock().unwrap().len() == 3)
        .await;
    flow.settled().await;
    assert!(
        replies.lock().unwrap()[2]
            .1
            .contains("could not start the preview"),
        "{:?}",
        replies.lock().unwrap()
    );
    super::eventually("failed claim removed from ledger", || {
        !failed.exists()
            && ledger.artifacts(&format!("preview-data:{}", flow.conversation))
                == vec![artifact.clone()]
    })
    .await;
    assert!(data.join("probe").exists());
    // A second start in the same session reuses the data (a merge keeps the worktree now).
    flow.say("Start it again.").await;
    flow.until("data reused", |_| replies.lock().unwrap().len() == 4)
        .await;
    flow.settled().await;
    let worktree = workspace(&flow);
    let git_file = worktree.join(".git");
    let original = std::fs::read(&git_file).unwrap();
    // Make open_worktree fail, so archive cannot keep the session's changes.
    std::fs::write(&git_file, "not a git worktree").unwrap();
    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    super::eventually("session data swept", || !data.exists()).await;
    assert!(worktree.exists(), "failed keep must retain the worktree");
    assert!(
        !ledger
            .disposing()
            .contains(&format!("session:{}", flow.conversation))
    );
    std::fs::write(git_file, original).unwrap();
    flow.stop().await;
}

/// Full access, and Linux's existing sandbox path, pass the value to the child unchanged.
#[tokio::test]
async fn preview_data_pass_through_does_not_claim_or_create_folders() {
    let tmp = scratch("data-pass-through");
    let missing = tmp.join("missing-parent/data");
    let existing = tmp.join("existing");
    std::fs::create_dir(&existing).unwrap();
    let replies = Replies::default();
    let script = {
        let replies = replies.clone();
        let missing = missing.clone();
        let existing = existing.clone();
        script(move |turn| {
            let (replies, missing, existing) = (replies.clone(), missing.clone(), existing.clone());
            async move {
                if turn.is_orchestrator() {
                    let path = if turn.input.contains("existing") {
                        &existing
                    } else {
                        &missing
                    };
                    let result = turn
                        .call(
                            "start_preview",
                            json!({
                                "command": "printf '%s' \"$BRIGADIER_DATA_DIR\"",
                                "env": { "BRIGADIER_DATA_DIR": path },
                            }),
                        )
                        .await;
                    replies
                        .lock()
                        .unwrap()
                        .push(("start_preview".into(), result.text));
                }
                Reply::text("Done.")
            }
        })
    };
    let flow = Flow::start("data-pass-through", Options::default(), script).await;
    for (index, (message, path)) in [("missing", &missing), ("existing", &existing)]
        .into_iter()
        .enumerate()
    {
        flow.say(message).await;
        flow.until("pass-through preview", |_| {
            replies.lock().unwrap().len() == index + 1
        })
        .await;
        flow.settled().await;
        assert!(
            replies.lock().unwrap()[index]
                .1
                .contains(path.to_str().unwrap())
        );
        assert!(
            flow.manager
                .runtime
                .ledger()
                .artifacts(&format!("preview-data:{}", flow.conversation))
                .is_empty()
        );
    }
    assert!(!missing.parent().unwrap().exists());
    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    flow.manager.cleanup_finished(&flow.conversation).await;
    assert!(existing.exists());
    flow.stop().await;
}

/// A stopped group includes grandchildren and descendants that start another session. Stop
/// gives each of them the grace to quit, not only the leader: the parent quits at once, and the
/// others take a moment to.
#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn pause_and_resume_stop_the_group_and_detached_grandchildren_and_stop_quits_them() {
    use std::time::Duration;
    let replies = Replies::default();
    let flow = Flow::start("preview-pause", Options::default(), thread(replies)).await;
    let tmp = scratch("pause");
    let program = tmp.join("heartbeat.py");
    std::fs::write(
        &program,
        r#"
import os, signal, sys, time
root = sys.argv[1]
def heartbeat(name):
    def quit(signum, frame):
        if name != 'parent':
            time.sleep(0.5)
        with open(root + '/' + name + '.quit', 'w') as f: f.write('term')
        sys.exit(0)
    signal.signal(signal.SIGTERM, quit)
    with open(root + '/' + name + '.pid', 'w') as f: f.write(str(os.getpid()))
    while True:
        with open(root + '/' + name, 'a') as f: f.write('.')
        time.sleep(0.03)
if os.fork() == 0:
    if os.fork() == 0:
        os.setsid()
        heartbeat('detached')
    heartbeat('child')
heartbeat('parent')
"#,
    )
    .unwrap();
    flow.say(&format!(
        "start python3 '{}' '{}'",
        program.display(),
        tmp.display()
    ))
    .await;
    let board = flow
        .until("the heartbeat preview", |board| {
            !running(board).is_empty() && tmp.join("detached").exists()
        })
        .await;
    let preview = running(&board).pop().unwrap();
    flow.settled().await;
    let size = |name: &str| std::fs::metadata(tmp.join(name)).unwrap().len();
    let names = ["parent", "child", "detached"];
    flow.manager
        .pause_preview(flow.conversation.clone(), preview.id.clone())
        .await
        .unwrap();
    assert_eq!(
        flow.board().await.previews[&preview.id].state,
        PreviewState::Paused
    );
    assert!(!flow.board().await.previews[&preview.id].state.is_running());
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = names.map(size);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        names.map(size),
        before,
        "the parent, child and detached grandchild are stopped"
    );
    let log = flow
        .manager
        .preview_log(&flow.conversation, Some(&preview.id), None)
        .await
        .unwrap();
    assert!(log.contains("paused by the user"), "{log}");
    flow.manager
        .resume_preview(flow.conversation.clone(), preview.id.clone())
        .await
        .unwrap();
    // It never got ready (nothing answers, no window), so it starts again.
    assert_eq!(
        flow.board().await.previews[&preview.id].state,
        PreviewState::Starting
    );
    super::eventually("all heartbeats to continue", || {
        names
            .iter()
            .enumerate()
            .all(|(i, name)| size(name) > before[i])
    })
    .await;
    flow.manager
        .pause_preview(flow.conversation.clone(), preview.id.clone())
        .await
        .unwrap();
    flow.manager
        .stop_preview(flow.conversation.clone(), Some(preview.id.clone()))
        .await
        .unwrap();
    assert!(!flow.board().await.previews.contains_key(&preview.id));
    super::eventually(
        "all processes to handle SIGTERM, including the paused grandchild",
        || {
            names
                .iter()
                .all(|name| tmp.join(format!("{name}.quit")).exists())
        },
    )
    .await;
    for name in names {
        gone(&flow, child_pid(&tmp.join(format!("{name}.pid")))).await;
    }
    assert!(
        flow.manager
            .resume_preview(flow.conversation.clone(), preview.id)
            .await
            .is_err()
    );
    // If its leader dies externally while paused, detached children must not remain frozen.
    let orphaned = tmp.join("orphaned");
    std::fs::create_dir(&orphaned).unwrap();
    flow.say(&format!(
        "start python3 '{}' '{}'",
        program.display(),
        orphaned.display()
    ))
    .await;
    let board = flow
        .until("another heartbeat preview", |board| {
            !running(board).is_empty() && orphaned.join("detached.pid").exists()
        })
        .await;
    let preview = running(&board).pop().unwrap();
    flow.settled().await;
    flow.manager
        .pause_preview(flow.conversation.clone(), preview.id.clone())
        .await
        .unwrap();
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(preview.pid.unwrap() as i32),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    flow.until("the externally killed preview to end", |board| {
        !board.previews[&preview.id].state.is_live()
    })
    .await;
    for name in names {
        gone(&flow, child_pid(&orphaned.join(format!("{name}.pid")))).await;
    }
    flow.stop().await;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
#[tokio::test]
async fn clear_keeps_live_previews_and_artifacts_and_reloads_without_reusing_ids() {
    let replies = Replies::default();
    let mut flow = Flow::start("preview-clear", Options::default(), thread(replies)).await;
    let tmp = scratch("clear");
    flow.say("start printf 'http://localhost:\\033[1m5173\\033[22m/\\nfirst\\nlast\\n'")
        .await;
    flow.settled().await;
    let board = flow
        .until("an ended preview", |board| {
            board.previews.values().any(|p| !p.state.is_live())
        })
        .await;
    let ended = board.sorted_previews().pop().unwrap();
    let (tail, url) = flow
        .manager
        .preview_log_tail(flow.conversation.clone(), ended.id.clone(), 1)
        .await
        .unwrap();
    assert!(tail.ends_with("last"), "{tail}");
    assert!(!tail.contains("first"), "{tail}");
    assert_eq!(url.as_deref(), Some("http://localhost:5173/"));
    assert_eq!(ended.url, url);
    let artifact = ended.log.clone().unwrap();
    let (live, _) = start(&flow, &tmp.join("live.pid")).await;
    flow.settled().await;
    // Polling discovers and persists a URL while the preview is still running. Later
    // polls use the stored URL even if the beginning of its log changes.
    let live_log = flow
        .manager
        .owned_dir("previews", &flow.conversation.0)
        .join(format!("{}.log", live.id));
    std::fs::write(
        &live_log,
        "http://localhost:\x1b[1m8123\x1b[22m/\nlistening on 8123\n",
    )
    .unwrap();
    let (_, url) = flow
        .manager
        .preview_log_tail(flow.conversation.clone(), live.id.clone(), 1)
        .await
        .unwrap();
    assert_eq!(url.as_deref(), Some("http://localhost:8123/"));
    assert_eq!(flow.board().await.previews[&live.id].url, url);
    std::fs::write(&live_log, "listening on 8123\n").unwrap();
    let (paused, _) = start(&flow, &tmp.join("paused.pid")).await;
    flow.settled().await;
    flow.manager
        .pause_preview(flow.conversation.clone(), paused.id.clone())
        .await
        .unwrap();
    let before = flow.board().await.outputs.len();
    for _ in 0..3 {
        let (tail, cached_url) = flow
            .manager
            .preview_log_tail(flow.conversation.clone(), live.id.clone(), 1)
            .await
            .unwrap();
        assert!(tail.contains("listening on 8123"));
        assert_eq!(cached_url, url);
    }
    assert_eq!(
        flow.board().await.outputs.len(),
        before,
        "polls do not create artifacts"
    );
    flow.manager
        .clear_previews(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow.board().await;
    assert_eq!(board.previews.len(), 2);
    assert!(board.previews.contains_key(&live.id) && board.previews.contains_key(&paused.id));
    assert!(board.outputs.contains_key(&artifact));
    // Shutdown must also stop the paused row. Reload folds the clear event, including its ID watermark.
    flow.restart().await;
    let board = flow.board().await;
    assert!(!board.previews.contains_key(&ended.id));
    assert!(matches!(
        board.previews[&paused.id].state,
        PreviewState::Stopped { .. }
    ));
    assert!(board.outputs.contains_key(&artifact));
    flow.manager
        .clear_previews(flow.conversation.clone())
        .await
        .unwrap();
    flow.restart().await;
    assert!(flow.board().await.previews.is_empty());
    let (next, _) = start(&flow, &tmp.join("next.pid")).await;
    assert_eq!(next.id, "preview-4");
    flow.stop().await;
}

fn state_of(board: &Board, id: &str) -> PreviewState {
    board.previews[id].state.clone()
}

/// A web preview reads Starting until the local URL it printed answers an HTTP request (a
/// server that accepts but doesn't answer yet is not enough), then Running with that URL; the
/// thread hears it is starting, not running, and `preview_log` gives its state.
#[tokio::test]
async fn a_web_preview_runs_once_its_url_answers() {
    use std::time::Duration;
    use tokio::io::AsyncWriteExt as _;

    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-url", Options::default(), thread(replies.clone())).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    let (answer, mut answering) = tokio::sync::watch::channel(false);
    let server = tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            if *answering.borrow_and_update() {
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .await;
            } else {
                held.push(stream);
            }
        }
    });
    flow.say(&format!("start echo '  Local: {url}'; sleep 600"))
        .await;
    flow.settled().await;
    let started = replies.lock().unwrap()[0].1.clone();
    assert!(
        started.contains("preview-1 \"site\" is starting, not ready yet"),
        "{started}"
    );
    assert!(started.contains("Don't tell the user it runs"), "{started}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        state_of(&flow.board().await, "preview-1"),
        PreviewState::Starting
    );
    let log = flow
        .manager
        .preview_log(&flow.conversation, None, None)
        .await
        .unwrap();
    assert!(log.contains("starting, not ready yet"), "{log}");

    answer.send(true).unwrap();
    let board = flow
        .until("the preview to be ready", |board| {
            state_of(board, "preview-1") == PreviewState::Running
        })
        .await;
    assert_eq!(
        board.previews["preview-1"].url.as_deref(),
        Some(url.as_str())
    );
    let log = flow
        .manager
        .preview_log(&flow.conversation, None, None)
        .await
        .unwrap();
    assert!(
        log.starts_with("[preview-1 \"site\" running, pid "),
        "{log}"
    );
    server.abort();
    flow.stop().await;
}

/// A web preview that prints no URL is ready once a port a process of its group listens on
/// answers, even when the launcher that started that process has exited (it is reparented,
/// no longer the preview's descendant, but still in its process group).
#[tokio::test]
async fn a_web_preview_runs_once_a_port_its_group_listens_on_answers() {
    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-port", Options::default(), thread(replies.clone())).await;
    let tmp = scratch("port");
    let pids = tmp.join("server.pid");
    flow.say(&format!(
        "start sh -c 'python3 -m http.server 0 --bind 127.0.0.1 >/dev/null 2>&1 & echo $! > {}'; \
         echo launched; sleep 600",
        pids.display()
    ))
    .await;
    let board = flow
        .until("the preview to be ready", |board| {
            board
                .previews
                .get("preview-1")
                .is_some_and(|preview| preview.state == PreviewState::Running)
        })
        .await;
    let preview = &board.previews["preview-1"];
    assert_eq!(preview.url, None, "it printed none");
    let server = child_pid(&pids);
    let processes = flow.manager.runtime.platform().processes();
    let leader = preview.pid.unwrap();
    assert_eq!(processes.group_of(server), Some(leader));
    assert!(
        !processes.descendants(leader).unwrap().contains(&server),
        "its launcher exited, so it was reparented"
    );
    // The start's turn over: a message sent into it would join it, and this script reads none.
    flow.settled().await;
    flow.say("Please stop it.").await;
    flow.until("the stop", |_| replies.lock().unwrap().len() == 2)
        .await;
    gone(&flow, server).await;
    flow.stop().await;
}

/// A desktop app preview never gets the frontend address it prints, and without a window of
/// its own it stays Starting (where windows can be seen). The user's Stop removes it from the list.
#[tokio::test]
async fn an_app_preview_has_no_url_and_waits_for_its_window() {
    use std::time::Duration;

    let replies: Replies = Arc::default();
    let flow = Flow::start("preview-app", Options::default(), thread(replies.clone())).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!(
        "http://localhost:{}/",
        listener.local_addr().unwrap().port()
    );
    flow.say(&format!("start-app echo '  Local: {url}'; sleep 600"))
        .await;
    flow.settled().await;
    let started = replies.lock().unwrap()[0].1.clone();
    assert!(started.contains("It is a desktop app"), "{started}");
    // Where windows can't be seen, an app reads as running once started.
    let sees = crate::manager::preview_ready::SEES_WINDOWS;
    if sees {
        assert!(
            started.contains(
                "preview-1 \"app\" is starting, not ready yet: it reads Starting until it \
                 opens its window"
            ),
            "{started}"
        );
    }
    // Something listens on the address it printed: that isn't the app.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let (_, url_seen) = flow
        .manager
        .preview_log_tail(flow.conversation.clone(), "preview-1".into(), 10)
        .await
        .unwrap();
    assert_eq!(url_seen, None);
    let board = flow.board().await;
    assert_eq!(
        board.previews["preview-1"].kind,
        crate::work::PreviewKind::App
    );
    let expected = if sees {
        PreviewState::Starting
    } else {
        PreviewState::Running
    };
    assert_eq!(state_of(&board, "preview-1"), expected);
    flow.manager
        .stop_preview(flow.conversation.clone(), Some("preview-1".into()))
        .await
        .unwrap();
    assert!(!flow.board().await.previews.contains_key("preview-1"));

    drop(listener);
    flow.stop().await;
}

/// An app preview runs in the background (#142): its launch, and a relaunch of the same app,
/// carry the marker that keeps a Brigadier build from taking the focus; a web preview's doesn't.
#[tokio::test]
async fn an_app_preview_and_its_relaunch_launch_in_the_background() {
    let replies: Replies = Arc::default();
    let flow = Flow::start(
        "preview-background",
        Options::default(),
        thread(replies.clone()),
    )
    .await;
    let env = brigadier_sandbox::BACKGROUND_LAUNCH_ENV;
    let log = |preview: &str| {
        let (flow, preview) = (&flow, preview.to_owned());
        async move {
            super::eventually_async("the preview's first line", || {
                let preview = preview.clone();
                async move {
                    flow.manager
                        .preview_log_tail(flow.conversation.clone(), preview, 10)
                        .await
                        .is_ok_and(|(text, _)| text.contains("bg="))
                }
            })
            .await;
            flow.manager
                .preview_log_tail(flow.conversation.clone(), preview, 10)
                .await
                .unwrap()
                .0
        }
    };
    let app = format!("start-app echo \"bg=${env}\"; sleep 600");
    flow.say(&app).await;
    flow.settled().await;
    assert!(log("preview-1").await.contains("bg=1"));
    flow.manager
        .stop_preview(flow.conversation.clone(), Some("preview-1".into()))
        .await
        .unwrap();
    // Launched again: still in the background.
    flow.say(&app).await;
    flow.settled().await;
    assert!(log("preview-2").await.contains("bg=1"));
    flow.say(&format!("start echo \"bg=${env}.\"; sleep 600"))
        .await;
    flow.settled().await;
    assert!(log("preview-3").await.contains("bg=."));
    flow.stop().await;
}

// Row Stop, Stop all and the composer's Stop remove only user-stopped work.
// Natural exits remain inspectable, even if Stop arrives after the process ended.
#[tokio::test]
async fn user_stops_remove_previews_but_keep_natural_exits() {
    let flow = Flow::start(
        "preview-user-removal",
        Options::default(),
        thread(Arc::default()),
    )
    .await;
    let tmp = scratch("user-removal");
    flow.say("start echo failed >&2; exit 3").await;
    flow.settled().await;
    let ended = flow.board().await.sorted_previews().pop().unwrap();
    assert!(matches!(
        ended.state,
        PreviewState::Exited { code: Some(3), .. }
    ));
    flow.manager
        .stop_preview(flow.conversation.clone(), Some(ended.id.clone()))
        .await
        .unwrap();
    assert_eq!(flow.board().await.previews[&ended.id].state, ended.state);

    for mode in ["row", "all", "composer"] {
        let (preview, child) = start(&flow, &tmp.join(format!("{mode}.pid"))).await;
        flow.settled().await;
        match mode {
            "row" => flow
                .manager
                .stop_preview(flow.conversation.clone(), Some(preview.id.clone()))
                .await
                .unwrap(),
            "all" => flow
                .manager
                .stop_preview(flow.conversation.clone(), None)
                .await
                .unwrap(),
            _ => flow
                .manager
                .interrupt(flow.conversation.clone())
                .await
                .unwrap(),
        }
        gone(&flow, child).await;
        let board = flow.board().await;
        assert!(
            !board.previews.contains_key(&preview.id),
            "{mode} kept its stopped preview"
        );
        assert_eq!(
            board.previews[&ended.id].state, ended.state,
            "{mode} removed a natural exit"
        );
    }
    flow.stop().await;
}

/// The preview `id` a start reply names (`preview-N "name" …`).
fn started_id(reply: &str) -> String {
    let at = reply.find("preview-").unwrap_or_else(|| panic!("{reply}"));
    let digits: String = reply[at + "preview-".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    format!("preview-{digits}")
}

/// A worker's previews (issue #203): each is recorded with its task, runs past the worker's
/// report and the thread's next turn, and ends with its task's folder, while another
/// worker's keep running. A worker stops its own and reaches no other's. A build and its app
/// launched as one command is an app preview that reads Building while the build runs.
#[tokio::test]
async fn a_workers_preview_carries_its_task_and_ends_with_its_folder() {
    let tmp = scratch("workers");
    let pids = tmp.join("child.pid");
    let replies: Replies = Arc::default();
    let log = replies.clone();
    let child = pids.clone();
    let flow = Flow::start(
        "preview-workers",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            let child = child.clone();
            async move {
                let call = |tool: &'static str, args: serde_json::Value| {
                    let turn = &turn;
                    let log = log.clone();
                    async move {
                        let reply = turn.call(tool, args).await;
                        assert!(!reply.is_error, "{tool}: {}", reply.text);
                        log.lock()
                            .unwrap()
                            .push((tool.to_owned(), reply.text.clone()));
                        reply.text
                    }
                };
                if turn.is_orchestrator() {
                    if turn.earlier == 0 {
                        for (title, spec) in [("Show the site", "site"), ("Show the app", "app")] {
                            call(
                                "delegate_task",
                                json!({"effort": "high", "title": title, "kind": "implement",
                                       "spec": spec, "provider": "claude"}),
                            )
                            .await;
                        }
                    } else if turn.input.contains("Land task-1.") {
                        call("land_phase", json!({"task": "task-1"})).await;
                    }
                    return Reply::text("[quiet]");
                }
                let number = turn.task_number().unwrap();
                if number == 1 {
                    call(
                        "start_preview",
                        json!({"command": server(&child), "name": "site"}),
                    )
                    .await;
                } else {
                    let app = turn.cwd.join("target/Demo.app/Contents/MacOS/demo");
                    std::fs::create_dir_all(app.parent().unwrap()).unwrap();
                    std::fs::write(&app, "#!/bin/sh\nexec sleep 600\n").unwrap();
                    std::fs::set_permissions(
                        &app,
                        std::os::unix::fs::PermissionsExt::from_mode(0o755),
                    )
                    .unwrap();
                    let command = format!(
                        "echo '   Compiling demo v0.1.0' && sleep 1 && \"{}\"",
                        app.display()
                    );
                    call(
                        "start_preview",
                        json!({"command": command, "name": "dev app"}),
                    )
                    .await;
                    let extra = call(
                        "start_preview",
                        json!({"command": "echo listening on 8124; sleep 600", "name": "extra"}),
                    )
                    .await;
                    call("stop_preview", json!({"id": started_id(&extra)})).await;
                }
                turn.write(&format!("task-{number}.txt"), "shown\n");
                turn.git(&["add", &format!("task-{number}.txt")]);
                turn.git(&["commit", "-qm", "Show it"]);
                call(
                    "submit_report",
                    json!({"summary": "Shown.", "changes": [format!("task-{number}.txt")]}),
                )
                .await;
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Show the site and the app.").await;
    let board = flow
        .until("both workers' previews and reports", |board| {
            running(board).len() == 2
                && std::fs::read_to_string(&pids).is_ok_and(|pid| !pid.trim().is_empty())
                && replies
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(tool, _)| tool == "submit_report")
                    .count()
                    == 2
        })
        .await;
    let (one, two) = (
        Flow::task(&board, 1).id.clone(),
        Flow::task(&board, 2).id.clone(),
    );
    let of = |board: &Board, task: &crate::model::TaskId| -> Preview {
        board
            .sorted_previews()
            .into_iter()
            .find(|preview| preview.task_id.as_ref() == Some(task))
            .unwrap()
    };
    let site = of(&board, &one);
    let app = of(&board, &two);
    assert_eq!(site.name, "site");
    assert!(site.workspace.contains("task-1"), "{}", site.workspace);
    assert_eq!(app.kind, crate::work::PreviewKind::App);
    // A worker has no read_artifact: its preview replies never point it there.
    for (tool, text) in replies.lock().unwrap().iter() {
        if tool.contains("preview") {
            assert!(!text.contains("read_artifact"), "{tool}: {text}");
        }
    }
    // The worker's own Stop removed its extra preview; it reached none of the other's.
    assert_eq!(board.previews.len(), 2, "{:?}", board.previews.keys());
    let err = flow
        .manager
        .stop_preview_tool(&flow.conversation, Some(&site.id), Some(&two))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not one of your previews"),
        "{err}"
    );
    if crate::manager::preview_ready::SEES_WINDOWS {
        // Still building (its log's last line compiles), and no window: it reads Building.
        flow.until("the app to read building", |board| {
            of(board, &two).building && of(board, &two).state == PreviewState::Starting
        })
        .await;
    }
    let child = child_pid(&pids);

    // The thread's next turn checks its own workspace: a worker's preview isn't the thread's.
    flow.say("Hello.").await;
    flow.settled().await;
    assert_eq!(running(&flow.board().await).len(), 2);

    // Landing task-1 removes its worktree: its preview ends, and task-2's runs on.
    flow.say("Land task-1.").await;
    let board = flow
        .until("task-1's preview to end", |board| {
            running(board)
                .iter()
                .all(|preview| preview.task_id.as_ref() != Some(&one))
        })
        .await;
    gone(&flow, child).await;
    assert_eq!(running(&board).len(), 1);
    assert_eq!(of(&board, &two).id, app.id);
    flow.stop().await;
}

/// A task's folder going while its worker starts a preview: the start is refused, so no
/// preview outlives the folder it runs in.
#[tokio::test]
async fn a_start_racing_its_tasks_end_is_refused() {
    let go = Arc::new(tokio::sync::Notify::new());
    let replies: Replies = Arc::default();
    let (log, wait) = (replies.clone(), go.clone());
    let flow = Flow::start(
        "preview-worker-race",
        Options::default(),
        script(move |turn| {
            let (log, wait) = (log.clone(), wait.clone());
            async move {
                if turn.is_orchestrator() {
                    if turn.earlier == 0 {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": "Show it", "kind": "scout",
                                       "spec": "Show it."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                wait.notified().await;
                let reply = turn
                    .call(
                        "start_preview",
                        json!({"command": "sleep 600", "name": "late"}),
                    )
                    .await;
                log.lock()
                    .unwrap()
                    .push((format!("{}", reply.is_error), reply.text));
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Show it.").await;
    let board = flow
        .until("the worker", |board| !board.tasks.is_empty())
        .await;
    let task = Flow::task(&board, 1).id.clone();
    flow.manager
        .stop_task_previews(&flow.conversation, &task)
        .await;
    go.notify_one();
    flow.until("the refused start", |_| !replies.lock().unwrap().is_empty())
        .await;
    let (error, text) = replies.lock().unwrap()[0].clone();
    assert_eq!(error, "true", "{text}");
    assert!(text.contains("your task has ended"), "{text}");
    assert!(flow.board().await.previews.is_empty());
    flow.stop().await;
}

/// A preview recorded before workers had previews (no `taskId`, no `building`) still loads.
#[test]
fn a_preview_recorded_before_worker_previews_loads() {
    let preview: Preview = serde_json::from_value(json!({
        "id": "preview-1",
        "conversationId": "conv-1",
        "name": "site",
        "command": "pnpm dev",
        "workdir": "",
        "workspace": "/work/repo",
        "state": { "type": "running" },
        "startedAtMs": 0,
    }))
    .unwrap();
    assert_eq!(preview.task_id, None);
    assert!(!preview.building);
}

/// A stopped task's previews stop before its unfinished work is kept (review R21.1): what a
/// preview writes as it stops is in the kept commit, and nothing writes after it.
#[tokio::test]
async fn a_stopped_tasks_preview_stops_before_its_work_is_kept() {
    let flow = Flow::start(
        "preview-worker-kept",
        Options::default(),
        script(move |turn| async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Build it", "kind": "implement",
                                   "spec": "Build it.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            if turn.earlier == 0 {
                let reply = turn
                    .call(
                        "start_preview",
                        json!({"command": "trap 'echo stopped > stopped.txt; exit 0' TERM; \
                                           echo building > building.txt; \
                                           while :; do sleep 1; done",
                               "name": "build", "kind": "app"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            Reply::text("Still building.")
        }),
    )
    .await;
    flow.say("Build it.").await;
    // Stopped once its shell set its trap: a SIGTERM before that ends it writing nothing.
    let board = flow
        .until("the worker's preview to set its trap", |board| {
            board
                .sorted_previews()
                .iter()
                .any(|preview| preview.task_id.is_some() && preview.state.is_live())
                && Flow::task(board, 1)
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.worktree.as_ref())
                    .is_some_and(|worktree| Path::new(worktree).join("building.txt").exists())
        })
        .await;
    let task = Flow::task(&board, 1).id.clone();
    flow.manager.stop_task(task.clone()).await.unwrap();
    let board = flow
        .until("task-1 stopped", |board| {
            Flow::task(board, 1).state == crate::work::TaskState::Stopped
        })
        .await;
    let Some(crate::work::KeptWork::Branch { commit, .. }) = &Flow::task(&board, 1).kept else {
        panic!("kept on its branch: {:?}", Flow::task(&board, 1).kept);
    };
    assert_eq!(
        super::git(&flow.repo, &["show", &format!("{commit}:stopped.txt")]).trim(),
        "stopped"
    );
    assert!(running(&board).is_empty());
    // Its folder is gone with its worker: the start guard forgets it (review R21.4).
    super::eventually("the ended task to be forgotten", || {
        !flow.manager.previews.task_ended(&task)
    })
    .await;
    flow.stop().await;
}

/// A worker's preview that ends on its own after its worker reported: the thread hears of it as
/// the worker's, in the worker's folder, and is not invited to start it again in its own
/// (review R21.3).
#[tokio::test]
async fn the_thread_hears_a_workers_preview_ended_without_restarting_it() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let inputs = heard.clone();
    let flow = Flow::start(
        "preview-worker-ended",
        Options::default(),
        script(move |turn| {
            let inputs = inputs.clone();
            async move {
                if turn.is_orchestrator() {
                    inputs.lock().unwrap().push(turn.input.clone());
                    if turn.earlier == 0 {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": "Show it", "kind": "implement",
                                       "spec": "Show it.", "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "start_preview",
                            json!({"command": "echo listening on 8126; sleep 4; exit 3",
                                   "name": "short"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    turn.write("shown.txt", "shown\n");
                    turn.git(&["add", "shown.txt"]);
                    turn.git(&["commit", "-qm", "Show it"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Shown.", "changes": ["shown.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Show it.").await;
    flow.until("the worker's preview to end", |board| {
        board.sorted_previews().iter().any(|preview| {
            preview.task_id.is_some() && matches!(preview.state, PreviewState::Exited { .. })
        })
    })
    .await;
    flow.settled().await;
    flow.say("Hello.").await;
    flow.settled().await;
    let all = heard.lock().unwrap().join("\n---\n");
    assert!(
        all.contains("\"short\", task-1's preview in its own folder, ended on its own (exit 3)"),
        "{all}"
    );
    assert!(!all.contains("Start it again"), "{all}");
    flow.stop().await;
}

/// Don't trust (issue #235): an idle worker's preview, started with the worker's own access,
/// stops with its CLI instead of running on with the access the folder no longer allows.
#[tokio::test]
async fn an_untrusted_folder_stops_an_idle_workers_preview() {
    let flow = Flow::start(
        "preview-worker-untrusted",
        Options::default(),
        script(move |turn| async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Show it", "kind": "implement",
                                   "spec": "Show it.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            if turn.earlier == 0 {
                let reply = turn
                    .call(
                        "start_preview",
                        json!({"command": "echo listening on 8125; sleep 600", "name": "site"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                turn.write("shown.txt", "shown\n");
                turn.git(&["add", "shown.txt"]);
                turn.git(&["commit", "-qm", "Show it"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Shown.", "changes": ["shown.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Show it.").await;
    let board = flow
        .until("the reported worker's preview", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == crate::work::TaskState::Reported)
                && running(board)
                    .iter()
                    .any(|preview| preview.task_id.is_some())
        })
        .await;
    let preview = running(&board).pop().unwrap();
    let pid = preview.pid.unwrap();
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    flow.manager
        .set_folder_trust(project, None, false)
        .await
        .unwrap();
    let board = flow
        .until("the worker's preview to stop", |board| {
            !board.previews[&preview.id].state.is_live()
        })
        .await;
    assert!(
        matches!(&board.previews[&preview.id].state, PreviewState::Stopped { reason } if reason.contains("trust")),
        "{:?}",
        board.previews[&preview.id].state
    );
    gone(&flow, pid).await;
    // The worker is idle, not stopped: it resumes under Ask for approval.
    assert_eq!(
        Flow::task(&board, 1).state,
        crate::work::TaskState::Reported
    );
    flow.stop().await;
}
