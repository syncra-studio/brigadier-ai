//! Restart survival (BUILD-PLAN D1.4, research A3): a daemon killed while a worker waits (on
//! its question, its outline's go-ahead, its review, the user's pause or quota) loses no
//! worker: each keeps its task and resumes its own CLI session, so what it read before the
//! kill is still in its context. A landing the kill cut off after the target moved is found
//! landed, and one cut off before is not.

use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use serde_json::json;
use tokio::sync::Notify;

use super::super::phases::WAITING_FOR_GO_AHEAD;
use super::super::review_runs::WAITING_FOR_REVIEW;
use super::super::workers::ASKED_THE_THREAD;
use super::{Flow, Options, Reply, Script, Turn};
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What a worker reads in its first session: still in its context once it resumes.
const PLANTED: &str = "The vendor's ticket is VT-88213.";

/// One turn a worker's CLI was asked to take.
#[derive(Debug, Clone)]
struct WorkerTurn {
    native_id: String,
    input: String,
}

type Log = Arc<Mutex<Vec<WorkerTurn>>>;

/// Logs each worker turn, then answers it with `worker`, given how many turns its CLI session
/// took before (0: a new session's first); the thread's and the reviewer's turns go to
/// `thread`.
fn logged<T, TF, W, WF>(log: &Log, thread: T, worker: W) -> Script
where
    T: Fn(Turn) -> TF + Send + Sync + 'static,
    TF: std::future::Future<Output = Reply> + Send + 'static,
    W: Fn(Turn, usize) -> WF + Send + Sync + 'static,
    WF: std::future::Future<Output = Reply> + Send + 'static,
{
    let (log, thread, worker) = (log.clone(), Arc::new(thread), Arc::new(worker));
    script(move |turn| {
        let (log, thread, worker) = (log.clone(), thread.clone(), worker.clone());
        async move {
            if turn.is_orchestrator() || turn.is_review() {
                return thread(turn).await;
            }
            let earlier = {
                let mut log = log.lock().unwrap();
                let earlier = log
                    .iter()
                    .filter(|logged| logged.native_id == turn.native_id)
                    .count();
                log.push(WorkerTurn {
                    native_id: turn.native_id.clone(),
                    input: turn.input.clone(),
                });
                earlier
            };
            worker(turn, earlier).await
        }
    })
}

/// The thread's delegation of task-1 on the user's "Do it.".
async fn delegate(turn: &Turn, kind: &str) -> Reply {
    let reply = turn
        .call(
            "delegate_task",
            json!({"effort": "medium", "title": "Find the ticket", "kind": kind,
                   "spec": "Find the vendor's ticket and add it to ticket.txt.",
                   "provider": "claude"}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("[quiet]")
}

/// A worker's report that names what it read before the restart.
async fn report(turn: &Turn) -> Reply {
    let reply = turn
        .call(
            "submit_report",
            json!({"summary": format!("Found it. {PLANTED}")}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

/// The worker's turns after the first resumed its first session: the same CLI session, started
/// again from its transcript, so nothing it read was lost.
fn assert_resumed_whole(flow: &Flow, log: &Log) {
    let turns = log.lock().unwrap().clone();
    assert!(turns.len() >= 2, "{turns:#?}");
    let first = &turns[0].native_id;
    for turn in &turns[1..] {
        assert_eq!(&turn.native_id, first, "the same session: {turns:#?}");
    }
    let resumed = flow.specs().into_iter().any(
        |(_, spec)| matches!(&spec.origin, Origin::Resume { native_id } if native_id == first),
    );
    assert!(resumed, "its session resumed");
}

fn task_reported(board: &crate::board::Board) -> bool {
    board.tasks.values().any(|task| task.report.is_some())
}

/// A worker that asked the thread a question when the daemon died: the thread hears the
/// question again, and its answer resumes the worker's own session.
#[tokio::test]
async fn a_worker_waiting_on_its_question_is_asked_again_and_resumes_after_a_kill() {
    let log = Log::default();
    let mut flow = Flow::start(
        "restart-question",
        Options::default(),
        logged(
            &log,
            |turn| async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.input.contains("[question from task-1") {
                    if turn.input.contains("before Brigadier restarted") {
                        let reply = turn
                            .call(
                                "answer_worker",
                                json!({"task": "task-1", "answer": "Use the support portal.",
                                       "why": "the brief implies it"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("Do it.") {
                    return delegate(&turn, "scout").await;
                }
                Reply::text("[quiet]")
            },
            |turn, n| async move {
                if n == 0 {
                    turn.tool_output(PLANTED);
                    let _ = turn
                        .call("ask_orchestrator", json!({"question": "Which portal?"}))
                        .await;
                    return Reply::text("Asked.");
                }
                assert!(
                    turn.input.contains("Use the support portal."),
                    "{}",
                    turn.input
                );
                report(&turn).await
            },
        ),
    )
    .await;
    flow.say("Do it.").await;
    flow.until("the worker's question", |board| {
        board.tasks.values().any(|task| {
            task.blocked_reason
                .as_deref()
                .is_some_and(|why| why.starts_with(ASKED_THE_THREAD))
        })
    })
    .await;
    flow.crash().await;
    let board = flow
        .until("the resumed worker's report", task_reported)
        .await;
    let task = Flow::task(&board, 1);
    assert_ne!(task.state, TaskState::Stopped, "{task:#?}");
    assert_resumed_whole(&flow, &log);
    flow.stop().await;
}

/// A lead whose outline waited for its go-ahead when the daemon died: it still waits, and the
/// go-ahead resumes its own session.
#[tokio::test]
async fn a_lead_waiting_for_its_go_ahead_still_waits_after_a_kill() {
    let log = Log::default();
    let mut flow = Flow::start(
        "restart-outline",
        Options::default(),
        logged(
            &log,
            |turn| async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.input.contains("Go ahead with task-1.") {
                    let reply = turn
                        .call("approve_outline", json!({"task": "task-1"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Started it.");
                }
                if turn.input.contains("Do it.") {
                    return delegate(&turn, "implement").await;
                }
                // Its outline, and anything else, waits for the user here.
                Reply::text("[quiet]")
            },
            |turn, n| async move {
                if n == 0 {
                    turn.tool_output(PLANTED);
                    let reply = turn
                        .call("submit_outline", json!({"outline": "1. Write ticket.txt"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Waiting for the go-ahead.");
                }
                assert!(turn.input.contains("Go ahead"), "{}", turn.input);
                turn.write("ticket.txt", "VT-88213\n");
                turn.git(&["add", "ticket.txt"]);
                turn.git(&["commit", "-qm", "Add the ticket"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": format!("Wrote it. {PLANTED}"),
                               "changes": ["ticket.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            },
        ),
    )
    .await;
    flow.say("Do it.").await;
    flow.until("the outline to wait", |board| {
        board
            .tasks
            .values()
            .any(|task| task.blocked_reason.as_deref() == Some(WAITING_FOR_GO_AHEAD))
    })
    .await;
    flow.crash().await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Blocked, "{task:#?}");
    assert_eq!(task.blocked_reason.as_deref(), Some(WAITING_FOR_GO_AHEAD));
    flow.say("Go ahead with task-1.").await;
    flow.until("the lead's report", task_reported).await;
    assert_resumed_whole(&flow, &log);
    flow.stop().await;
}

/// A verifier that waited for its own code review when the daemon died: the review ended with
/// the daemon, so the verifier resumes its own session.
#[tokio::test]
async fn a_verifier_waiting_for_its_review_resumes_after_a_kill() {
    let log = Log::default();
    let mut flow = Flow::start(
        "restart-review",
        Options {
            // The review never ends: the kill cuts it off.
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        logged(
            &log,
            |turn| async move {
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("start_verifier", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("Do it.") {
                    return delegate(&turn, "implement").await;
                }
                Reply::text("[quiet]")
            },
            |turn, earlier| async move {
                if !turn.prompt.contains("You verify this phase") {
                    turn.write("ticket.txt", "VT-88213\n");
                    turn.git(&["add", "ticket.txt"]);
                    turn.git(&["commit", "-qm", "Add the ticket"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Wrote it.", "changes": ["ticket.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                // Its resumed session (told it was interrupted, maybe also of its review's
                // end) reports.
                if earlier > 0 {
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": format!("Verified. {PLANTED}"),
                                   "changes": ["ticket.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                turn.tool_output(PLANTED);
                let started = turn.call("review_code", json!({})).await;
                assert!(
                    started.text.starts_with("Started a review"),
                    "{}",
                    started.text
                );
                Reply::text("Waiting for the review.")
            },
        ),
    )
    .await;
    flow.say("Do it.").await;
    flow.until("the verifier to wait for its review", |board| {
        board
            .tasks
            .values()
            .any(|task| task.blocked_reason.as_deref() == Some(WAITING_FOR_REVIEW))
    })
    .await;
    flow.crash().await;
    let board = flow
        .until("the verifier's report", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 2 && task.report.is_some())
        })
        .await;
    assert_ne!(Flow::task(&board, 2).state, TaskState::Stopped);
    // The verifier's own turns: its first, then the one its resumed session took.
    let turns = log.lock().unwrap().clone();
    let verifier: Vec<WorkerTurn> = turns
        .iter()
        .filter(|turn| turn.native_id != turns[0].native_id)
        .cloned()
        .collect();
    assert!(verifier.len() >= 2, "{verifier:#?}");
    // Its own session resumed (told it was interrupted, or of its review's end, whichever
    // came first).
    assert!(
        verifier
            .iter()
            .all(|turn| turn.native_id == verifier[0].native_id)
    );
    flow.stop().await;
}

/// A worker the user paused when the daemon died stays paused, and the user's resume starts
/// its own session again.
#[tokio::test]
async fn a_paused_worker_stays_paused_after_a_kill_and_resumes_its_own_session() {
    let log = Log::default();
    let started = Arc::new(Notify::new());
    let mut flow = Flow::start(
        "restart-paused",
        Options::default(),
        logged(
            &log,
            |turn| async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.input.contains("Do it.") {
                    return delegate(&turn, "scout").await;
                }
                Reply::text("[quiet]")
            },
            {
                let started = started.clone();
                move |turn, n| {
                    let started = started.clone();
                    async move {
                        if n == 0 {
                            turn.tool_output(PLANTED);
                            started.notify_one();
                            // Until the user's pause interrupts it.
                            turn.stopped().await;
                            return Reply::text("Paused.");
                        }
                        assert!(turn.input.contains("Continue the task."), "{}", turn.input);
                        report(&turn).await
                    }
                }
            },
        ),
    )
    .await;
    flow.say("Do it.").await;
    started.notified().await;
    let id = Flow::task(&flow.board().await, 1).id.clone();
    flow.manager.pause_task(id.clone()).await.unwrap();
    flow.crash().await;
    let board = flow.board().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Paused);
    flow.manager.resume_task(id).await.unwrap();
    flow.until("the resumed worker's report", task_reported)
        .await;
    assert_resumed_whole(&flow, &log);
    flow.stop().await;
}

/// A worker that waited for quota when the daemon died still waits, its task kept.
#[tokio::test]
async fn a_worker_waiting_for_quota_still_waits_after_a_kill() {
    let log = Log::default();
    let mut flow = Flow::start(
        "restart-quota",
        Options::default(),
        logged(
            &log,
            |turn| async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.input.contains("Do it.") {
                    return delegate(&turn, "scout").await;
                }
                Reply::text("[quiet]")
            },
            // Every model is at its limit.
            |_, _| async move { Reply::limited() },
        ),
    )
    .await;
    flow.say("Do it.").await;
    flow.until("the task to wait for quota", |board| {
        board.tasks.values().any(|task| task.quota_wait.is_some())
    })
    .await;
    flow.crash().await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Paused, "{task:#?}");
    assert!(task.quota_wait.is_some());
    flow.stop().await;
}

/// The flow for the landing tests: task-1 writes ticket.txt and reports; the thread leaves it
/// unlanded, and every thread turn's input is logged.
async fn reported_task(name: &str, heard: &Arc<Mutex<Vec<String>>>) -> Flow {
    let heard = heard.clone();
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.is_orchestrator() {
                    heard.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("Do it.") {
                        return delegate(&turn, "implement").await;
                    }
                    return Reply::text("[quiet]");
                }
                turn.write("ticket.txt", "VT-88213\n");
                turn.git(&["add", "ticket.txt"]);
                turn.git(&["commit", "-qm", "Add the ticket"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Wrote it.", "changes": ["ticket.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Do it.").await;
    flow.until("the report", task_reported).await;
    flow
}

/// Marks task-1 as landing from the target's tip as it is now, as `land_commits` records it
/// right before the target moves; with `moved`, the target then fast-forwards to the task's
/// branch, as the git engine does, and the daemon dies before the landing is recorded.
async fn cut_off_landing(flow: &Flow, moved: bool) -> String {
    let task = Flow::task(&flow.board().await, 1).clone();
    let workspace = task.workspace.clone().expect("its workspace");
    let target = workspace.target.clone().expect("its target");
    let branch = workspace.branch.clone().expect("its branch");
    let repo = flow.manager.git.open(&flow.repo).unwrap();
    let from = repo.branch_tip(&target).unwrap().expect("the target");
    let tip = repo.branch_tip(&branch).unwrap().expect("the branch");
    flow.manager
        .update_task(&flow.conversation, &task.id, |t| {
            t.state = TaskState::Landing;
            t.landing = Some(t.title.clone());
            t.landing_from = Some(from.0.clone());
        })
        .await
        .unwrap();
    if moved {
        let landed = repo
            .land(&brigadier_git::LandRequest {
                branch: target,
                expected_tip: from,
                commit: tip.clone(),
            })
            .unwrap();
        assert!(
            matches!(landed, brigadier_git::LandOutcome::Landed { .. }),
            "{landed:?}"
        );
    }
    tip.0
}

/// A landing the kill cut off after the target fast-forwarded: the task is landed at its
/// branch's tip, and the thread hears it landed.
#[tokio::test]
async fn a_landing_cut_off_after_the_target_moved_is_found_landed() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = reported_task("restart-landed", &heard).await;
    let tip = cut_off_landing(&flow, true).await;
    flow.crash().await;
    let board = flow
        .until("the landing recorded", |board| {
            Flow::task(board, 1).state == TaskState::Landed
        })
        .await;
    assert_eq!(Flow::task(&board, 1).landed.as_deref(), Some(tip.as_str()));
    assert_eq!(Flow::task(&board, 1).landing_from, None);
    flow.until("the thread to hear it", |_| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[landed task-1] Brigadier restarted as it landed"))
    })
    .await;
    flow.stop().await;
}

/// A landing the kill cut off before the target moved: nothing landed, and the thread is told
/// to land it again.
#[tokio::test]
async fn a_landing_cut_off_before_the_target_moved_lands_again() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = reported_task("restart-not-landed", &heard).await;
    cut_off_landing(&flow, false).await;
    flow.crash().await;
    let board = flow.board().await;
    let task = Flow::task(&board, 1);
    assert_eq!(task.state, TaskState::Reported, "{task:#?}");
    assert_eq!(task.landing_from, None);
    flow.until("the thread to hear it", |_| {
        heard
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[not landed task-1"))
    })
    .await;
    flow.stop().await;
}
