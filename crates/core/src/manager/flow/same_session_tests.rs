//! Same-session resume (BUILD-PLAN D1.3, research A12): a worker cut off by a usage limit or a
//! sign-in error resumes its own CLI session on another account of its provider, its whole
//! context kept, where a thin successor used to start from a hand-off; "prompt is too long"
//! goes to a fresh session of the same model from the size hand-off, told it isn't lossless,
//! never to another model; and the thread out of credits moves to another account in the same
//! session.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::fault::{FaultKey, InjectedError};
use crate::model::DomainEvent;
use crate::work::RequestState;

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
    let moments: Vec<String> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::OrchestratorStepped { step } => match step.kind {
                crate::work::OrchestratorStepKind::Moment { text } => Some(text),
                _ => None,
            },
            _ => None,
        })
        .collect();
    assert!(
        moments
            .iter()
            .any(|text| text.contains("ran out of credits")
                && text.contains("continuing on Account acct-b")),
        "{moments:#?}"
    );
    flow.stop().await;
}
