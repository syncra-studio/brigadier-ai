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
    let flow = Flow::start(
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
    flow.stop().await;
}
