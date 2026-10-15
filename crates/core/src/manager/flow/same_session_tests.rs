//! Same-session resume (BUILD-PLAN D1.3, research A12): a worker cut off by a usage limit or a
//! sign-in error resumes its own CLI session on another account of its provider, its whole
//! context kept, where a thin successor used to start from a hand-off; "prompt is too long"
//! goes to a fresh session of the same model from the size hand-off, told it isn't lossless,
//! never to another model; and the thread out of credits moves to another account in the same
//! session.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use brigadier_providers::model::Origin;
use serde_json::json;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::fallback::CONTINUE_SAME_SESSION;
use crate::manager::fault::{FaultKey, InjectedError};
use crate::model::DomainEvent;
use crate::work::{RequestState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// One turn a worker's CLI was asked to take.
#[derive(Debug, Clone)]
struct WorkerTurn {
    account: Option<String>,
    native_id: String,
    input: String,
}

/// A planted detail the worker learns in its first session and must still know after the seam.
const PLANTED: &str = "The vendor's ticket is VT-88213.";

/// The thread delegates one scout; the scout's turns are logged and answered by `answer`.
fn delegating<F>(log: &Arc<Mutex<Vec<WorkerTurn>>>, answer: F) -> Script
where
    F: Fn(&Turn, &[WorkerTurn]) -> Option<Reply> + Send + Sync + 'static,
{
    let (log, answer) = (log.clone(), Arc::new(answer));
    script(move |turn| {
        let (log, answer) = (log.clone(), answer.clone());
        async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if turn.is_orchestrator() {
                if turn.input.contains("Find the ticket.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Find the ticket",
                                   "kind": "scout", "spec": "Find the vendor's ticket.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            let earlier = log.lock().unwrap().clone();
            log.lock().unwrap().push(WorkerTurn {
                account: turn.account.clone(),
                native_id: turn.native_id.clone(),
                input: turn.input.clone(),
            });
            if let Some(reply) = answer(&turn, &earlier) {
                return reply;
            }
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": format!("Found it. {PLANTED}")}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    })
}

/// A worker at its account's limit resumes its own session on the other account, told to
/// carry on: the same native session, so everything it had read is still there.
#[tokio::test]
async fn a_worker_at_its_limit_resumes_its_own_session_on_another_account() {
    let log: Arc<Mutex<Vec<WorkerTurn>>> = Arc::default();
    let flow = Flow::start(
        "same-session-limit",
        Options::default(),
        delegating(&log, |turn, earlier| {
            // Its first session reads the planted detail, then hits the limit.
            (earlier.is_empty() && turn.account.is_none()).then(|| {
                turn.tool_output(PLANTED);
                Reply::limited()
            })
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("Find the ticket.").await;
    let board = flow
        .until("the scout reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    let turns = log.lock().unwrap().clone();
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[0].account, None);
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].native_id, turns[0].native_id, "the same session");
    assert_eq!(turns[1].input, CONTINUE_SAME_SESSION);
    let resumed = flow.specs().into_iter().any(|(_, spec)| {
        matches!(&spec.origin, Origin::Resume { native_id } if *native_id == turns[0].native_id)
    });
    assert!(
        resumed,
        "{:#?}",
        flow.specs()
            .iter()
            .map(|(_, s)| &s.origin)
            .collect::<Vec<_>>()
    );
    let task = board.tasks.values().next().unwrap();
    assert_eq!(task.attempts.len(), 2);
    flow.stop().await;
}

/// The thread delegates one scout. The scout's first turn reads the planted detail, says it is
/// ready, waits until the test armed a fault, then emits one event (which fires it); later
/// turns report. Every scout turn is logged.
fn held_scout(
    log: &Arc<Mutex<Vec<WorkerTurn>>>,
    ready: &Arc<Notify>,
    armed: &Arc<Notify>,
) -> Script {
    let (log, ready, armed) = (log.clone(), ready.clone(), armed.clone());
    script(move |turn| {
        let (log, ready, armed) = (log.clone(), ready.clone(), armed.clone());
        async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Find the ticket.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Find the ticket",
                                   "kind": "scout", "spec": "Find the vendor's ticket.",
                                   "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            let first = log.lock().unwrap().is_empty();
            log.lock().unwrap().push(WorkerTurn {
                account: turn.account.clone(),
                native_id: turn.native_id.clone(),
                input: turn.input.clone(),
            });
            if first {
                turn.tool_output(PLANTED);
                ready.notify_one();
                armed.notified().await;
                turn.emit(brigadier_providers::ProviderEvent::Notice {
                    level: brigadier_providers::NoticeLevel::Info,
                    message: "reading".into(),
                })
                .await;
                return Reply::text("Still reading.");
            }
            let reply = turn
                .call("submit_report", json!({"summary": "Found it."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    })
}

/// Starts the scout, arms `error` on it once its first turn is under way, and waits for its
/// report: the board then, and every scout turn.
async fn seam(
    flow: &Flow,
    log: &Arc<Mutex<Vec<WorkerTurn>>>,
    ready: &Notify,
    armed: &Notify,
    error: InjectedError,
) -> (crate::board::Board, Vec<WorkerTurn>, crate::work::TaskId) {
    flow.say("Find the ticket.").await;
    ready.notified().await;
    let task = flow
        .board()
        .await
        .tasks
        .keys()
        .next()
        .cloned()
        .expect("the scout");
    flow.manager
        .debug_inject_error(FaultKey::Task(task.clone()), ProviderKind::Claude, error, 0)
        .await
        .unwrap();
    armed.notify_one();
    let board = flow
        .until("the scout reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    (board, log.lock().unwrap().clone(), task)
}

/// A sign-in error mid-turn is a cut-off the same model survives on the other account: the
/// same session resumes there.
#[tokio::test]
async fn a_worker_whose_sign_in_fails_resumes_its_own_session_on_another_account() {
    let log: Arc<Mutex<Vec<WorkerTurn>>> = Arc::default();
    let (ready, armed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let flow = Flow::start(
        "same-session-auth",
        Options::default(),
        held_scout(&log, &ready, &armed),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let (_, turns, _) = seam(&flow, &log, &ready, &armed, InjectedError::Auth).await;
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[1].native_id, turns[0].native_id, "the same session");
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].input, CONTINUE_SAME_SESSION);
    flow.stop().await;
}

/// "Prompt is too long" on a worker: a fresh session of the same model goes on from the size
/// hand-off, told that its transcript is clipped and to say unknown rather than guess, and no
/// other model is routed.
#[tokio::test]
async fn a_worker_whose_prompt_is_too_long_goes_on_in_a_fresh_session_of_the_same_model() {
    let log: Arc<Mutex<Vec<WorkerTurn>>> = Arc::default();
    let (ready, armed) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let flow = Flow::start(
        "same-session-context",
        Options::default(),
        held_scout(&log, &ready, &armed),
    )
    .await;
    let (board, turns, task) =
        seam(&flow, &log, &ready, &armed, InjectedError::ContextWindow).await;
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_ne!(turns[1].native_id, turns[0].native_id, "a fresh session");
    assert!(
        turns[1].input.contains("prompt is too long")
            && turns[1].input.contains("This hand-off is not lossless")
            && turns[1]
                .input
                .contains("say it is unknown rather than guess it"),
        "{}",
        turns[1].input
    );
    // Same model, same attempt: no reroute.
    let task = &board.tasks[&task];
    assert_eq!(task.attempts.len(), 1, "{:?}", task.attempts);
    assert_ne!(task.state, TaskState::Failed);
    flow.stop().await;
}

/// The thread out of credits moves to the other account in the same session, and the note
/// says why.
#[tokio::test]
async fn the_thread_out_of_credits_goes_on_in_its_session_on_another_account() {
    // Each thread turn's account, native session and input.
    type Seen = Vec<(Option<String>, String, String)>;
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let (started, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (log, on_start, on_go) = (seen.clone(), started.clone(), go.clone());
    let flow = Flow::start(
        "same-session-credits",
        Options::default(),
        script(move |turn| {
            let (log, on_start, on_go) = (log.clone(), on_start.clone(), on_go.clone());
            async move {
                let first = log.lock().unwrap().is_empty();
                log.lock().unwrap().push((
                    turn.account.clone(),
                    turn.native_id.clone(),
                    turn.input.clone(),
                ));
                if first {
                    on_start.notify_one();
                    on_go.notified().await;
                    turn.emit(brigadier_providers::ProviderEvent::Notice {
                        level: brigadier_providers::NoticeLevel::Info,
                        message: "thinking".into(),
                    })
                    .await;
                    return Reply::text("Partly.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("Do the thing.").await;
    started.notified().await;
    flow.manager
        .debug_inject_error(
            FaultKey::Conversation(flow.conversation.clone()),
            ProviderKind::Claude,
            InjectedError::Billing,
            0,
        )
        .await
        .unwrap();
    go.notify_one();
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:#?}");
    assert_eq!(seen[1].0.as_deref(), Some("acct-b"));
    assert_eq!(seen[1].1, seen[0].1, "the same session");
    let notices: Vec<String> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::ConversationNotice { notice, .. } => Some(notice.text),
            _ => None,
        })
        .collect();
    assert!(
        notices
            .iter()
            .any(|text| text.contains("ran out of credits")
                && text.contains("continuing on Account acct-b")),
        "{notices:#?}"
    );
    flow.stop().await;
}
