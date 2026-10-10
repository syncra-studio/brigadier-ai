//! The pre-warmed worker (THREAD-PLAN.md Q8 lever 6): the user's message makes the next
//! task's worktree; the task delegated next starts in it under its own id, at the session's tip
//! even when that moved on; the user's Stop, a changed permission level and expiry each leave
//! nothing behind.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{PermissionLevel, Setup, TaskId};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A thread that delegates one implement task when asked to add a greeting, and a worker that writes
/// hello.txt and reports; each worker's worktree is kept in `seen`.
fn delegating(seen: Arc<Mutex<Vec<PathBuf>>>) -> Script {
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Please add a greeting") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                                   "spec": "Create hello.txt."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Done.");
            }
            seen.lock().unwrap().push(turn.cwd.clone());
            turn.write("hello.txt", "hello\n");
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    })
}

/// Waits until `owner` has nothing left in the cleanup ledger and `path` is gone.
async fn removed(flow: &Flow, owner: &str, path: &Path) {
    super::eventually(&format!("{owner}'s pre-warm to go"), || {
        flow.manager.runtime.ledger().artifacts(owner).is_empty() && !path.exists()
    })
    .await;
}

#[tokio::test]
async fn a_delegated_task_starts_in_the_pre_warmed_worktree_under_its_reserved_id() {
    let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let flow = Flow::start(
        "prewarm-adopt",
        Options::default(),
        delegating(seen.clone()),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let (reserved, worktree) = flow
        .manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("a pre-warm");
    assert!(worktree.join("README.md").exists());
    flow.say("Please add a greeting").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.id, reserved, "the task took the pre-warm's id");
    let recorded = task.workspace.as_ref().expect("a workspace");
    assert_eq!(
        recorded.worktree.as_deref(),
        Some(worktree.to_str().unwrap())
    );
    assert_eq!(*seen.lock().unwrap(), std::slice::from_ref(&worktree));
    // The worker worked on the task's own branch, from the session's tip.
    let branch = recorded.branch.clone().expect("a branch");
    assert!(branch.starts_with("brigadier/"), "{branch}");
    assert!(
        git(&flow.repo, &["log", "--format=%s", &branch]).contains("Start"),
        "{branch}"
    );
    // The adopted pre-warm is the task's; taking it started the session's next one.
    let next = flow
        .manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("the next pre-warm");
    assert_ne!(next.0, reserved);
    assert_ne!(next.1, worktree);
    flow.stop().await;
}

#[tokio::test]
async fn a_pre_warm_whose_base_moved_on_is_moved_to_the_new_tip_and_still_used() {
    let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let flow = Flow::start(
        "prewarm-moved",
        Options::default(),
        delegating(seen.clone()),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let (reserved, worktree) = flow
        .manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("a pre-warm");
    // What the pre-warm copied in (untracked, like a build cache) stays through the move.
    std::fs::create_dir_all(worktree.join("warm-cache")).unwrap();
    std::fs::write(worktree.join("warm-cache/built"), "built\n").unwrap();
    // The session's branch moves on after the pre-warm was made.
    let workspace = flow.thread_specs()[0].1.add_dirs[0].clone();
    std::fs::write(workspace.join("moved.txt"), "moved\n").unwrap();
    git(&workspace, &["add", "-A"]);
    git(&workspace, &["commit", "-q", "-m", "Move on"]);
    flow.say("Please add a greeting").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.id, reserved);
    let started = seen.lock().unwrap().clone();
    assert_eq!(
        started,
        std::slice::from_ref(&worktree),
        "the pre-warmed worktree"
    );
    assert!(worktree.join("moved.txt").exists(), "from the new tip");
    assert!(worktree.join("warm-cache/built").exists(), "the copy stays");
    let recorded = task.workspace.as_ref().expect("a workspace");
    assert_eq!(
        recorded.base.as_deref(),
        Some(git(&workspace, &["rev-parse", "HEAD"]).trim())
    );
    flow.stop().await;
}

#[tokio::test]
async fn the_user_s_stop_while_the_pre_warm_is_made_leaves_nothing() {
    let flow = Flow::start(
        "prewarm-stop",
        Options::default(),
        delegating(Arc::default()),
    )
    .await;
    flow.say("Hello").await;
    // Stopped at once: the pre-warm is most likely still being made.
    let reserved: TaskId = flow
        .manager
        .prewarm_id(&flow.conversation)
        .expect("a pre-warm under way");
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    assert!(flow.manager.prewarm_id(&flow.conversation).is_none());
    let owner = format!("task:{reserved}");
    let made = flow
        .dir
        .join("data")
        .join("worktrees")
        .read_dir()
        .ok()
        .and_then(|mut projects| projects.next())
        .and_then(|project| project.ok())
        .map(|project| {
            project
                .path()
                .join(format!("task-next-{}", &reserved.0[reserved.0.len() - 8..]))
        })
        .unwrap_or_else(|| flow.dir.join("none"));
    removed(&flow, &owner, &made).await;
    assert!(!flow.manager.prewarms.owns(&reserved.0));
    flow.stop().await;
}

#[tokio::test]
async fn an_unused_pre_warm_is_removed_when_it_expires() {
    let flow = Flow::start(
        "prewarm-expiry",
        Options::default(),
        delegating(Arc::default()),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let (reserved, worktree) = flow
        .manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("a pre-warm");
    flow.manager.prewarms.expire();
    removed(&flow, &format!("task:{reserved}"), &worktree).await;
    assert!(flow.manager.prewarm_id(&flow.conversation).is_none());
    flow.stop().await;
}

#[tokio::test]
async fn a_task_after_a_permission_change_does_not_take_the_pre_warm() {
    let seen: Arc<Mutex<Vec<PathBuf>>> = Arc::default();
    let flow = Flow::start(
        "prewarm-permission",
        Options::default(),
        delegating(seen.clone()),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let (reserved, worktree) = flow
        .manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("a pre-warm");
    let conversation = flow.manager.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        environment,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
        ..
    }) = conversation.setup
    else {
        panic!("a session");
    };
    flow.manager
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo,
                environment,
                permission: PermissionLevel::ApproveForMe,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    // The thread delegates without a new user message (a queued follow-up, say).
    let claimed = flow
        .manager
        .claim_prewarm(&flow.conversation, crate::work::TaskKind::Implement);
    assert_eq!(claimed, None);
    removed(&flow, &format!("task:{reserved}"), &worktree).await;
    flow.stop().await;
}
