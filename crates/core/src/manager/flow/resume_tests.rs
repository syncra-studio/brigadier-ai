//! A worker whose CLI dies before it reports resumes itself in the same session; a second
//! death before a report stops it with both exits named (SESSION-FIXES-PLAN.md Q4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use brigadier_providers::model::Origin;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::worker_resume::INTERRUPTED;
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The thread delegates one task and stays quiet about what comes back.
async fn thread(turn: &Turn) -> Reply {
    if turn.input.contains("Add a.") {
        let reply = turn
            .call(
                "delegate_task",
                json!({"effort": "high", "title": "Add a", "kind": "implement",
                       "spec": "Add a.txt, then report."}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
    }
    Reply::text("[quiet]")
}

/// The worker sessions started, in order: (origin, model, worktree).
fn worker_starts(flow: &Flow) -> Vec<(Origin, Option<String>, Vec<std::path::PathBuf>)> {
    flow.specs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, spec)| {
            spec.append_system_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.starts_with("You are a Brigadier worker."))
        })
        .map(|(_, spec)| {
            (
                spec.origin.clone(),
                spec.model.clone(),
                spec.add_dirs.clone(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_worker_killed_mid_turn_resumes_its_own_session_and_reports() {
    let turns = Arc::new(AtomicU32::new(0));
    let counted = turns.clone();
    let flow = Flow::start(
        "worker-resume",
        Options::default(),
        script(move |turn| {
            let counted = counted.clone();
            async move {
                if turn.is_orchestrator() {
                    return thread(&turn).await;
                }
                if counted.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Reply::crashed(9);
                }
                assert!(turn.input.contains(INTERRUPTED), "{}", turn.input);
                // It still knows its task: the resumed session has its brief.
                assert!(turn.prompt.contains("Add a.txt"), "{}", turn.prompt);
                turn.write("a.txt", "a\n");
                turn.git(&["add", "a.txt"]);
                turn.git(&["commit", "-qm", "Add a.txt"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added a.txt.", "changes": ["a.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add a.").await;
    let board = flow
        .until("the resumed worker to report", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    let task = board.tasks.values().next().expect("the task").clone();
    assert!(task.deaths.is_empty(), "a report clears its deaths");
    assert!(!task.resuming);
    // One attempt on one model: a crash isn't a hand-off.
    assert_eq!(task.attempts.len(), 1, "{:#?}", task.attempts);
    let starts = worker_starts(&flow);
    assert_eq!(starts.len(), 2, "{starts:#?}");
    assert_eq!(starts[0].0, Origin::New);
    assert_eq!(
        starts[1].0,
        Origin::Resume {
            native_id: task.native_session.clone().expect("its session"),
        },
        "the same CLI session resumes"
    );
    assert_eq!(starts[0].1, starts[1].1, "the same model");
    assert_eq!(starts[0].2, starts[1].2, "the same worktree");
    flow.stop().await;
}

#[tokio::test]
async fn a_worker_that_dies_twice_before_reporting_stops_with_both_exits_named() {
    let told = Arc::new(std::sync::Mutex::new(String::new()));
    let heard = told.clone();
    let mut flow = Flow::start(
        "worker-dies-twice",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("died twice") {
                        heard.lock().unwrap().clone_from(&turn.input);
                    }
                    return thread(&turn).await;
                }
                Reply::crashed(9)
            }
        }),
    )
    .await;
    flow.say("Add a.").await;
    let board = flow
        .until("the task to stop", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Blocked)
        })
        .await;
    let task = board.tasks.values().next().expect("the task").clone();
    let reason = task.blocked_reason.clone().expect("why it stopped");
    assert_eq!(
        reason,
        "Its CLI ended twice before it reported: signal 9: Killed; then signal 9: Killed."
    );
    assert_eq!(task.deaths.len(), 2);
    // Resumed once, never handed to another model.
    assert_eq!(worker_starts(&flow).len(), 2);
    assert_eq!(task.attempts.len(), 1, "{:#?}", task.attempts);
    flow.until("the thread to hear it", |_| {
        told.lock()
            .unwrap()
            .contains("[task-1 stopped: died twice]")
    })
    .await;
    assert!(told.lock().unwrap().contains(&reason));
    // A restart keeps it as it stopped, for message_worker: blocked, its worktree there.
    let worktree = task
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.worktree.clone())
        .expect("its worktree");
    flow.restart().await;
    let tasks = flow.core.tasks(&flow.conversation).await.unwrap();
    let kept = tasks.iter().find(|t| t.id == task.id).expect("the task");
    assert_eq!(kept.state, TaskState::Blocked);
    assert_eq!(kept.deaths.len(), 2);
    assert!(std::path::Path::new(&worktree).is_dir(), "{worktree}");
    assert_eq!(worker_starts(&flow).len(), 2, "nothing resumed it");
    flow.stop().await;
}

/// The ref check counts a worker live only while its CLI runs: a rewrite of its branch is
/// recorded as its own amend while its first CLI and its resumed one run, and put back once
/// it stopped after dying twice, its task still held (PLAN bug 6 step 5).
#[tokio::test]
async fn a_worker_counts_live_for_the_ref_check_only_while_its_cli_runs() {
    use crate::manager::refcheck::Caller;
    use crate::model::ConversationId;
    use crate::work::TaskId;
    let conversation = Arc::new(std::sync::Mutex::new(None::<ConversationId>));
    let shared = conversation.clone();
    let starts = Arc::new(AtomicU32::new(0));
    // A commit on no parent: no rewrite to it is a forward move.
    let rewrite = |dir: &std::path::Path, message: &str| -> String {
        let tree = super::git(dir, &["rev-parse", "HEAD^{tree}"]);
        let commit = super::git(dir, &["commit-tree", &tree, "-m", message]);
        let branch = super::git(dir, &["symbolic-ref", "--short", "HEAD"]);
        super::git(
            dir,
            &["update-ref", &format!("refs/heads/{branch}"), &commit],
        );
        commit
    };
    let flow = Flow::start(
        "worker-live-ref-check",
        Options::default(),
        script(move |turn| {
            let (shared, starts) = (shared.clone(), starts.clone());
            async move {
                if turn.is_orchestrator() {
                    return thread(&turn).await;
                }
                let start = starts.fetch_add(1, Ordering::SeqCst);
                let conversation = shared.lock().unwrap().clone().unwrap();
                let branch = turn.git(&["symbolic-ref", "--short", "HEAD"]);
                let task = turn.host.core.board(&conversation).await.unwrap();
                let task = Flow::task(&task, 1).id.clone();
                turn.host
                    .record_branch(&turn.cwd, &branch, &conversation, Some(&task));
                // Another worker's landing check, while this one's CLI runs.
                let amended = rewrite(&turn.cwd, &format!("Amended {start}"));
                turn.host
                    .ref_check(&turn.cwd, Caller::Task(TaskId("other".into())))
                    .await;
                assert_eq!(turn.git(&["rev-parse", "HEAD"]), amended);
                Reply::crashed(9)
            }
        }),
    )
    .await;
    *conversation.lock().unwrap() = Some(flow.conversation.clone());
    flow.say("Add a.").await;
    let board = flow
        .until("the task to stop", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Blocked)
        })
        .await;
    assert_eq!(worker_starts(&flow).len(), 2, "resumed once");
    let task = board.tasks.values().next().expect("the task").clone();
    let worktree = std::path::PathBuf::from(task.workspace.unwrap().worktree.unwrap());
    let kept = super::git(&worktree, &["rev-parse", "HEAD"]);
    rewrite(&worktree, "Rewound");
    flow.manager
        .ref_check(&worktree, Caller::Task(TaskId("other".into())))
        .await;
    assert_eq!(super::git(&worktree, &["rev-parse", "HEAD"]), kept);
    flow.stop().await;
}
