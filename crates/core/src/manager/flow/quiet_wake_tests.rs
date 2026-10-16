//! Batched wake-ups (BUILD-PLAN C3): news that needs nothing from the thread now (a status
//! due, a landing done, checks passed, a clean review) waits a short window while more of its
//! request still runs, so several wake the thread once. A worker's question ends the wait at
//! once, and the window's end delivers what waited: nothing is dropped.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::Envelope;
use crate::work::{InjectionKind, RunState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A session whose thread delegates one scout and ends on [quiet]. The scout waits for `ask`,
/// then asks the thread a question, which the thread answers, and reports. Every thread turn's
/// input goes into `inputs`.
async fn scouting(name: &str, ask: Arc<Notify>, inputs: Arc<Mutex<Vec<String>>>) -> Flow {
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (ask, inputs) = (ask.clone(), inputs.clone());
            async move {
                if turn.is_orchestrator() {
                    inputs.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Hello it is.");
                    }
                    if turn.input.contains("Which greeting?") {
                        let reply = turn
                            .call(
                                "answer_worker",
                                json!({"task": "task-1", "answer": "Hello.",
                                       "why": "your recommendation fits the brief"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("Pick our greeting") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "high", "title": "Pick a greeting",
                                       "kind": "scout", "spec": "Pick the greeting."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                ask.notified().await;
                let answer = turn
                    .call(
                        "ask_orchestrator",
                        json!({"question": "Which greeting? I recommend Hello."}),
                    )
                    .await;
                assert_eq!(answer.text, "Hello.");
                let reply = turn
                    .call("submit_report", json!({"summary": "Hello."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Pick our greeting.").await;
    flow.until("the scout to run with the thread idle", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.state == TaskState::Running)
            && !matches!(board.run, RunState::Running | RunState::Starting)
    })
    .await;
    flow
}

/// A status due, as the status clock sends it.
async fn status(flow: &Flow, request: &str, minutes: u32) {
    flow.manager
        .deliver_for(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Reminder,
                label: "status".into(),
                task_id: None,
                text: format!("[status due] {minutes} min with nothing new for the user."),
            },
            Some(request.to_owned()),
        )
        .await;
}

/// A status due while the scout works waits: no turn starts for it. The scout's question
/// then wakes the thread at once, though the window has an hour to go, and that one turn reads
/// both.
#[tokio::test]
async fn quiet_news_waits_while_a_worker_runs_and_a_question_wakes_the_thread_at_once() {
    let (ask, inputs) = (Arc::new(Notify::new()), Arc::default());
    let flow = scouting("quiet-question", ask.clone(), Arc::clone(&inputs)).await;
    *flow.manager.quiet_window.lock().unwrap() = Some(Duration::from_secs(3600));
    let request = flow.board().await.latest_request().unwrap().id.clone();
    status(&flow, &request, 5).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        inputs.lock().unwrap().len(),
        1,
        "a status alone woke the thread"
    );
    assert_eq!(flow.board().await.run, RunState::Idle);
    ask.notify_one();
    // The question's turn, its answer and the report can all be over between two looks at the
    // board, so what the thread was given says it woke. With the window an hour long, only the
    // question can have woken it.
    tokio::time::timeout(
        Duration::from_secs(30),
        flow.until("the question's turn", |_| inputs.lock().unwrap().len() >= 2),
    )
    .await
    .expect("the question woke the thread at once");
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Done);
    let inputs = inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 3, "{inputs:#?}");
    assert!(
        inputs[1].contains("[status due] 5 min") && inputs[1].contains("Which greeting?"),
        "{}",
        inputs[1]
    );
    flow.stop().await;
}

/// Two statuses due a moment apart while the scout works, with nothing urgent after them: the
/// window's end delivers both in one turn, and the request still ends on the scout's report.
#[tokio::test]
async fn quiet_news_goes_in_one_turn_when_its_window_ends() {
    let (ask, inputs) = (Arc::new(Notify::new()), Arc::default());
    let flow = scouting("quiet-window", ask.clone(), Arc::clone(&inputs)).await;
    *flow.manager.quiet_window.lock().unwrap() = Some(Duration::from_secs(3));
    let request = flow.board().await.latest_request().unwrap().id.clone();
    status(&flow, &request, 5).await;
    // Long enough for a turn of its own to start, well inside the window.
    tokio::time::sleep(Duration::from_millis(200)).await;
    status(&flow, &request, 10).await;
    super::eventually("the window's turn", || inputs.lock().unwrap().len() == 2).await;
    {
        let inputs = inputs.lock().unwrap();
        assert!(
            inputs[1].contains("[status due] 5 min") && inputs[1].contains("[status due] 10 min"),
            "{}",
            inputs[1]
        );
    }
    ask.notify_one();
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Done);
    // Its start, the statuses, the question and the report.
    assert_eq!(inputs.lock().unwrap().len(), 4);
    flow.stop().await;
}

/// With nothing else running for its request, quiet news wakes the thread at once: there is
/// nothing to wait for, and the answer may wait on it.
#[tokio::test]
async fn quiet_news_with_nothing_else_running_wakes_the_thread_at_once() {
    let (ask, inputs) = (Arc::new(Notify::new()), Arc::default());
    let flow = scouting("quiet-alone", ask.clone(), Arc::clone(&inputs)).await;
    *flow.manager.quiet_window.lock().unwrap() = Some(Duration::from_secs(3600));
    ask.notify_one();
    flow.settled().await;
    let request = flow.board().await.latest_request().unwrap().id.clone();
    let before = inputs.lock().unwrap().len();
    status(&flow, &request, 5).await;
    super::eventually("the status's turn", || {
        inputs.lock().unwrap().len() == before + 1
    })
    .await;
    flow.stop().await;
}
