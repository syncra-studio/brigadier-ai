//! A worker's turn ends only with its report, or rightly waiting (BUILD-PLAN E2, research
//! A5(c), (e)).
//!
//! - **The `Stop` hook** refuses a Claude worker's turn that ends with no report, once per stop;
//!   it never refuses one that waits: for its outline's go-ahead, its background job or its
//!   question.
//! - **A question nobody answers** goes on after the wait only with its recorded safe default,
//!   and only when it is operational; a consent or product question keeps waiting, its worker
//!   ends its turn, and the answer starts its next one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::ProviderEvent;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};

type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn seen(log: &Log, what: &str) -> Option<String> {
    log.lock()
        .unwrap()
        .iter()
        .find(|(name, _)| name == what)
        .unwrap_or_else(|| panic!("{what} was looked at: {:?}", log.lock().unwrap()))
        .1
        .clone()
}

async fn report(turn: &Turn) -> Reply {
    let reply = turn
        .call("submit_report", json!({"summary": "Done."}))
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

/// The thread's turn: it delegates `title` on its first, and stays quiet after.
async fn delegate_once(turn: &Turn, title: &str) -> Reply {
    if turn.earlier == 0 {
        let reply = turn
            .call(
                "delegate_task",
                json!({"effort": "high", "title": title, "kind": "implement",
                       "spec": "Do it.", "provider": "claude"}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
    }
    Reply::text("")
}

#[tokio::test]
async fn a_turn_without_a_report_is_held_once_and_a_reported_one_ends() {
    let log: Log = Arc::default();
    let seen_by = log.clone();
    let flow = Flow::start(
        "stop-hook-report",
        Options::default(),
        script(move |turn| {
            let log = seen_by.clone();
            async move {
                if turn.is_orchestrator() {
                    return delegate_once(&turn, "Write it").await;
                }
                let first = turn.stop_decision(false).await;
                log.lock().unwrap().push(("no report".into(), first));
                let again = turn.stop_decision(true).await;
                log.lock().unwrap().push(("no report, again".into(), again));
                let reply = turn
                    .call("submit_report", json!({"summary": "Done."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                let reported = turn.stop_decision(false).await;
                log.lock().unwrap().push(("reported".into(), reported));
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Write it.").await;
    flow.until("task-1 to report", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.report.is_some())
    })
    .await;
    flow.settled().await;
    let held = seen(&log, "no report").expect("held");
    assert!(held.contains("without calling submit_report"), "{held}");
    // Once per stop: the CLI went on with the reason, and the next stop ends.
    assert_eq!(seen(&log, "no report, again"), None);
    assert_eq!(seen(&log, "reported"), None);
    flow.stop().await;
}

#[tokio::test]
async fn a_worker_waiting_on_its_background_job_or_its_outline_ends_its_turn() {
    let log: Log = Arc::default();
    let seen_by = log.clone();
    let flow = Flow::start(
        "stop-hook-waits",
        Options::default(),
        script(move |turn| {
            let log = seen_by.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[outline task-") {
                        let reply = turn
                            .call("approve_outline", json!({"task": "task-1"}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
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
                    return Reply::text("");
                }
                if turn.earlier == 0 {
                    let alone = turn.stop_decision(false).await;
                    log.lock().unwrap().push(("no job".into(), alone));
                    // The job's count reaches the task through its event pump.
                    let t = &turn;
                    turn.emit(ProviderEvent::BackgroundJobs {
                        running: vec!["job".into()],
                    })
                    .await;
                    super::eventually_async("the job to let the turn end", || async move {
                        t.stop_decision(false).await.is_none()
                    })
                    .await;
                    turn.emit(ProviderEvent::BackgroundJobs {
                        running: Vec::new(),
                    })
                    .await;
                    super::eventually_async(
                        "the job's end to hold the turn again",
                        || async move { t.stop_decision(false).await.is_some() },
                    )
                    .await;
                    let reply = turn
                        .call("submit_outline", json!({"outline": "1. Build it."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    let outline = turn.stop_decision(false).await;
                    log.lock().unwrap().push(("outline".into(), outline));
                    return Reply::text("Waiting for the go-ahead.");
                }
                report(&turn).await
            }
        }),
    )
    .await;
    flow.say("Build it.").await;
    flow.until("task-1 to report", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.report.is_some())
    })
    .await;
    flow.settled().await;
    assert!(seen(&log, "no job").is_some(), "no job: the turn is held");
    assert_eq!(seen(&log, "outline"), None);
    flow.stop().await;
}

/// No answer within the wait: an operational question goes on with its safe default, and the
/// default is recorded; a consent question keeps waiting whatever default it gives, its worker
/// ends its turn (the Stop hook lets it), and the thread's later answer starts its next turn.
#[tokio::test]
async fn only_an_operational_question_goes_on_with_its_safe_default() {
    let log: Log = Arc::default();
    let seen_by = log.clone();
    let flow = Flow::start(
        "safe-default",
        Options::default(),
        script(move |turn| {
            let log = seen_by.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Answer task-2 now.") {
                        let reply = turn
                            .call(
                                "answer_worker",
                                json!({"task": "task-2", "answer": "Don't merge it; report.",
                                       "why": "The user decides merges."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Answered.");
                    }
                    if turn.earlier == 0 {
                        for title in ["Pick a file", "Ask to merge"] {
                            let reply = turn
                                .call(
                                    "delegate_task",
                                    json!({"effort": "high", "title": title, "kind": "implement",
                                           "spec": title, "provider": "claude"}),
                                )
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                        }
                    }
                    // Nobody answers a question here.
                    return Reply::text("");
                }
                if turn.input.contains("[answer to your question]") {
                    log.lock()
                        .unwrap()
                        .push(("answered".into(), Some(turn.input.clone())));
                    return report(&turn).await;
                }
                match turn.task_number() {
                    Some(1) => {
                        let reply = turn
                            .call(
                                "ask_orchestrator",
                                json!({"question": "Which file holds the helper?",
                                       "about": "operational",
                                       "safe_default": "put it in src/util.ts"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock()
                            .unwrap()
                            .push(("operational".into(), Some(reply.text)));
                        report(&turn).await
                    }
                    _ => {
                        let reply = turn
                            .call(
                                "ask_orchestrator",
                                json!({"question": "May I merge my branch into main?",
                                       "about": "consent",
                                       "safe_default": "merge it"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        let waiting = turn.stop_decision(false).await;
                        log.lock()
                            .unwrap()
                            .push(("consent".into(), Some(reply.text)));
                        log.lock().unwrap().push(("consent stop".into(), waiting));
                        Reply::text("Waiting for the answer.")
                    }
                }
            }
        }),
    )
    .await;
    flow.manager
        .set_question_timeout(Duration::from_millis(300));
    flow.say("Do both.").await;
    flow.until("task-1 to report", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.report.is_some())
    })
    .await;
    let operational = seen(&log, "operational").unwrap();
    assert!(
        operational.contains("Go on with your safe default: put it in src/util.ts"),
        "{operational}"
    );
    super::eventually("the consent question to keep waiting", || {
        log.lock()
            .unwrap()
            .iter()
            .any(|(name, _)| name == "consent stop")
    })
    .await;
    let consent = seen(&log, "consent").unwrap();
    assert!(consent.contains("this question waits for one"), "{consent}");
    assert!(!consent.contains("merge it"), "{consent}");
    assert_eq!(
        seen(&log, "consent stop"),
        None,
        "a waiting worker's turn may end"
    );
    // The request stays open while the question waits: nothing settles it.
    let board = flow
        .until("task-2 to wait for its answer", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 2 && task.state == crate::work::TaskState::Blocked)
        })
        .await;
    let task_2 = Flow::task(&board, 2);
    assert!(task_2.report.is_none(), "it waits: {task_2:#?}");
    assert!(!task_2.state.is_final());

    flow.say("Answer task-2 now.").await;
    flow.until("task-2 to report", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 2 && task.report.is_some())
    })
    .await;
    let answered = seen(&log, "answered").unwrap();
    assert!(answered.contains("Don't merge it; report."), "{answered}");
    flow.stop().await;
}
