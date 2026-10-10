//! How a session's request ends (THREAD-PARITY-PLAN.md §5 Q6): the user is there, so nothing
//! is listed for them to do. A worker's `needs_user` reaches the thread for its answer, and
//! the thread's own `note_for_user` kind `waiting` is refused; the request ends done.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_sessions_worker_and_thread_list_nothing_for_the_user_and_the_request_ends() {
    let reports: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = reports.clone();
    let flow = Flow::start(
        "ending-nothing-listed",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        log.lock().unwrap().push(turn.input.clone());
                        return Reply::text(
                            "The payment form is wired up.\nYou'll need to: add STRIPE_KEY to .env",
                        );
                    }
                    let waiting = turn
                        .call(
                            "note_for_user",
                            json!({"kind": "waiting", "what": "Add STRIPE_KEY to .env."}),
                        )
                        .await;
                    assert!(
                        waiting.is_error && waiting.text.contains("Say it in your answer instead"),
                        "{}",
                        waiting.text
                    );
                    let decided = turn
                        .call(
                            "note_for_user",
                            json!({"kind": "decided", "what": "Kept the form on one page.", "why": "It has three fields."}),
                        )
                        .await;
                    assert!(!decided.is_error, "{}", decided.text);
                    let delegated = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "low", "title": "Look around", "kind": "scout", "spec": "Find the payment form."}),
                        )
                        .await;
                    assert!(!delegated.is_error, "{}", delegated.text);
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "The form is in README.md.",
                               "needs_user": ["Add STRIPE_KEY to .env"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Wire up the payment form.").await;
    let board = flow.settled().await;
    // The thread heard what the worker needs, to say it in its answer.
    let heard = reports.lock().unwrap().join("\n");
    assert!(
        heard.contains("Needs the user (say it in your answer):\n- Add STRIPE_KEY to .env"),
        "{heard}"
    );
    // Nothing is listed, nothing waits, and the decision is noted.
    assert!(board.waiting.is_empty(), "{:#?}", board.waiting);
    assert!(board.waits_listed.is_empty(), "{:#?}", board.waits_listed);
    let request = board.latest_request().unwrap();
    assert_eq!(request.state, RequestState::Done, "{request:#?}");
    assert!(
        board
            .decisions
            .iter()
            .any(|decision| decision.what == "Kept the form on one page."),
        "{:#?}",
        board.decisions
    );
    flow.stop().await;
}
