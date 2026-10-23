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
            let (tool, args) = if let Some(command) =
                input.lines().find_map(|line| line.strip_prefix("start "))
            {
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
            } else if let Some(command) = input
                .lines()
                .find_map(|line| line.strip_prefix("start-with-env "))
            {
                (
                    "start_preview",
                    json!({ "command": command, "name": "site", "env": {"GREETING": "hi"} }),
                )
            } else if let Some(command) = input
                .lines()
                .find_map(|line| line.strip_prefix("start-test "))
            {
                (
                    "start_preview",
                    json!({ "command": command, "name": "tests", "kind": "test" }),
                )
            } else if let Some(command) = input
                .lines()
                .find_map(|line| line.strip_prefix("start-pdf "))
            {
                (
                    "start_preview",
                    json!({ "command": command, "name": "report", "kind": "pdf", "target": "out.pdf" }),
                )
            } else if input.contains("Please stop it.") {
                ("stop_preview", json!({}))
            } else if input.contains("Please merge.") {
                ("finish_session", json!({}))
            } else if input.contains("Show me the log.") {
                ("preview_log", json!({}))
            } else {
                return Reply::text("");
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
    assert!(log.contains("full log: /"), "{log}");
    assert!(log.ends_with("listening on 8123"), "{log}");
    assert!(alive(&flow, pid));

    flow.settled().await;
    flow.say("Please stop it.").await;
    flow.until("the stop", |_| replies.lock().unwrap().len() == 3)
        .await;
    let stopped = replies.lock().unwrap()[2].1.clone();
    assert!(
        stopped.starts_with(
            "preview-1 \"site\": stopped (stopped by the thread); removed from the user's \
             list, its log: /"
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
    assert!(
        last.contains("Its stored log remains readable: /"),
        "{last}"
    );

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
            && ended.contains("unless you already told them."),
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
    let files = board
        .previews
        .values()
        .filter_map(|p| p.log.as_ref())
        .map(|alias| {
            let hash = board.outputs[alias].blob.parse().unwrap();
            flow.core
                .store()
                .blobs()
                .file_of(&hash)
                .display()
                .to_string()
        })
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
        last.contains("their stored logs remain readable: "),
        "{last}"
    );
    for id in ids {
        assert!(last.contains(&id), "{last}");
    }
    for file in files {
        assert!(last.contains(&file), "{last}");
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
            target: None,
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
        .preview_log(&flow.conversation, Some(&preview.id), None, None)
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
        .preview_log(&flow.conversation, None, None, None)
        .await
        .unwrap();
    assert!(log.contains("starting, not ready yet"), "{log}");
    assert!(
        started.contains("run `brigadier preview log --wait <seconds>`"),
        "{started}"
    );
    // A wait ends after its seconds while it still starts (#510)...
    let log = flow
        .manager
        .preview_log(&flow.conversation, None, None, Some(1))
        .await
        .unwrap();
    assert!(log.contains("starting, not ready yet"), "{log}");
    // ...and the moment it runs.
    let waiting = {
        let manager = flow.manager.clone();
        let conversation = flow.conversation.clone();
        tokio::spawn(async move {
            manager
                .preview_log(&conversation, None, None, Some(300))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!waiting.is_finished());

    answer.send(true).unwrap();
    let waited = tokio::time::timeout(Duration::from_secs(30), waiting)
        .await
        .expect("the wait ends once it runs")
        .unwrap()
        .unwrap();
    assert!(
        waited.starts_with("[preview-1 \"site\" running, pid "),
        "{waited}"
    );
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
        .preview_log(&flow.conversation, None, None, None)
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
/// no longer the preview's descendant, but still in its process group). Its address is that
/// port's, so its page opens, not its output (#406).
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
    let url = preview
        .url
        .as_deref()
        .expect("the address of the port that answered");
    let port = url
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.strip_suffix('/'))
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("a local address with a port: {url}"));
    assert!(
        crate::manager::preview_ready::port_answers(port).await,
        "{url} is the server's"
    );
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

/// Start again on an ended preview runs its own command again from its record: the same id,
/// folder, variables and request, for the user, and the thread hears of it. A running one, an unknown
/// one and a second start of one that runs again are refused.
#[tokio::test]
async fn start_again_runs_an_ended_previews_own_command_from_its_record() {
    let flow = Flow::start(
        "preview-restart",
        Options::default(),
        thread(Arc::default()),
    )
    .await;
    let tmp = scratch("restart");
    let runs = tmp.join("runs.txt");
    let id = flow.conversation.clone();
    flow.say(&format!(
        "start-with-env echo \"run $GREETING $(pwd)\" >> {}; exit 3",
        runs.display()
    ))
    .await;
    flow.settled().await;
    let ended = flow.board().await.sorted_previews().pop().unwrap();
    assert!(matches!(
        ended.state,
        PreviewState::Exited { code: Some(3), .. }
    ));
    assert_eq!(ended.env.get("GREETING").map(String::as_str), Some("hi"));
    // It keeps the request whose turn started it (#435).
    let requests: Vec<String> = flow.board().await.requests.keys().cloned().collect();
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(ended.request_id.as_ref(), requests.first());

    flow.manager
        .restart_preview(id.clone(), ended.id.clone())
        .await
        .unwrap();
    let board = flow
        .until("it ran and ended again", |board| {
            let again = &board.previews[&ended.id];
            again.ended_at_ms > ended.ended_at_ms && !again.state.is_live()
        })
        .await;
    let again = &board.previews[&ended.id];
    assert_eq!(board.previews.len(), 1, "the same preview, not a new one");
    // Still its turn's, though it started again after it.
    assert_eq!(again.request_id, ended.request_id);
    assert!(again.started_at_ms >= ended.started_at_ms);
    assert_eq!(
        (&again.command, &again.workdir),
        (&ended.command, &ended.workdir)
    );
    let lines: Vec<String> = std::fs::read_to_string(&runs)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0], lines[1], "same variables, same folder");
    assert!(lines[1].starts_with("run hi "), "{lines:?}");

    let err = flow
        .manager
        .restart_preview(id.clone(), "preview-9".into())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no preview-9"), "{err}");

    // A running one isn't started twice.
    let (running, child) = start(&flow, &tmp.join("server.pid")).await;
    let err = flow
        .manager
        .restart_preview(id.clone(), running.id.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("running already"), "{err}");
    flow.manager
        .stop_preview(id.clone(), Some(running.id.clone()))
        .await
        .unwrap();
    gone(&flow, child).await;
    flow.stop().await;
}

/// Besides web pages and apps, a preview is a test run (running once started, its command its
/// target, done when it ends well), a rendered PDF (ready once its file is there) or an iOS
/// simulator run, read from its command with its device.
#[tokio::test]
async fn test_runs_pdfs_and_simulators_are_previews_with_their_targets() {
    use crate::work::PreviewKind;
    let flow = Flow::start("preview-kinds", Options::default(), thread(Arc::default())).await;
    let latest = |board: &Board| board.sorted_previews().pop().unwrap();

    flow.say("start-test echo '3 passed, 1 skipped in 0.01s'")
        .await;
    let board = flow
        .until("the test run ended", |board| {
            !board.previews.is_empty() && !latest(board).state.is_live()
        })
        .await;
    let tests = latest(&board);
    assert_eq!(
        (tests.kind, tests.target.as_deref()),
        (
            PreviewKind::Test,
            Some("echo '3 passed, 1 skipped in 0.01s'")
        )
    );
    assert!(
        matches!(tests.state, PreviewState::Exited { code: Some(0), .. }),
        "{:?}",
        tests.state
    );

    let pages = |n: usize| crate::documents::tests::pdf(&vec![&["a page"][..]; n]);
    std::fs::write(workspace(&flow).join("two.pdf"), pages(2)).unwrap();
    std::fs::write(workspace(&flow).join("three.pdf"), pages(3)).unwrap();
    flow.say("start-pdf sleep 1; cp two.pdf out.pdf; sleep 600")
        .await;
    let board = flow
        .until("the PDF is there", |board| {
            latest(board).kind == PreviewKind::Pdf && latest(board).state == PreviewState::Running
        })
        .await;
    let pdf = latest(&board);
    assert_eq!(pdf.target.as_deref(), Some("out.pdf"));
    assert!(
        workspace(&flow).join("out.pdf").is_file(),
        "ready once it is there"
    );
    // Its page count, read when it is ready.
    assert_eq!(pdf.pages, Some(2));
    flow.manager
        .stop_preview(flow.conversation.clone(), Some(pdf.id.clone()))
        .await
        .unwrap();
    // Rendered anew, its count is read again when the render ends: none while the file isn't
    // a PDF it can read.
    flow.say("start-pdf rm out.pdf; sleep 1; echo pdf > out.pdf; sleep 1; cp three.pdf out.pdf")
        .await;
    let board = flow
        .until("the PDF rendered anew", |board| {
            latest(board).kind == PreviewKind::Pdf && !latest(board).state.is_live()
        })
        .await;
    assert_eq!(latest(&board).pages, Some(3));

    flow.say("start true simctl boot 'iPhone 16'; sleep 600")
        .await;
    let board = flow
        .until("the simulator run", |board| {
            latest(board).kind == PreviewKind::Simulator
                && latest(board).state == PreviewState::Running
        })
        .await;
    let simulator = latest(&board);
    assert_eq!(
        (simulator.target.as_deref(), simulator.url.as_deref()),
        (Some("iPhone 16"), None)
    );
    flow.manager
        .stop_preview(flow.conversation.clone(), None)
        .await
        .unwrap();
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
