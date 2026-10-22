//! A request isn't over while Brigadier still owes it a turn (w43 bug 2): a CI watch with no
//! result. It works on between turns and is done once, at its real ending; a stop or a failure
//! still ends it, and the user's Stop ends the watch.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::Envelope;
use crate::model::DomainEvent;
use crate::work::{InjectionKind, OrchestratorStepKind, RequestState, RunState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// How many times `request` became done, from its stored updates.
async fn times_done(flow: &Flow, request: &str) -> usize {
    let mut done = false;
    let mut times = 0;
    for event in flow.events().await {
        if let DomainEvent::RequestUpdated { request: updated } = event
            && updated.id == request
        {
            let now = updated.state == RequestState::Done;
            if now && !done {
                times += 1;
            }
            done = now;
        }
    }
    times
}

/// The request's state now, and still a moment later: nothing settles it otherwise meanwhile.
async fn stays(flow: &Flow, request: &str) -> RequestState {
    let before = flow.board().await.requests[request].state.clone();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let after = flow.board().await.requests[request].state.clone();
    assert_eq!(before, after, "it changed meanwhile");
    after
}

/// A session that pushes and watches CI: its turn waits at `push` while the test files the
/// watch's row for it (as `watch_ci` does), then ends on [quiet]. The watch's result turn
/// answers in one line.
async fn watching(name: &str, ends: Reply) -> (Flow, String) {
    let push = Arc::new(tokio::sync::Notify::new());
    let gate = push.clone();
    let ends = Arc::new(Mutex::new(Some(ends)));
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (gate, ends) = (gate.clone(), ends.clone());
            async move {
                if turn.input.contains("[ci 97c298d5 · passed") {
                    return Reply::text("CI passed on `main`.");
                }
                if turn.input.contains("Push main") {
                    gate.notified().await;
                    return ends.lock().unwrap().take().unwrap();
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Push main to GitHub.").await;
    let board = flow
        .until("the push's turn", |board| board.run == RunState::Running)
        .await;
    let request = board.latest_request().unwrap().id.clone();
    flow.manager
        .orchestrator_step(&flow.conversation, ci_row(None))
        .await;
    push.notify_one();
    flow.until("the push's turn to end", |board| {
        !matches!(board.run, RunState::Running | RunState::Starting)
    })
    .await;
    (flow, request)
}

fn ci_row(result: Option<&str>) -> OrchestratorStepKind {
    OrchestratorStepKind::CiWatch {
        repo: "o/n".into(),
        branch: "main".into(),
        sha: "97c298d5aaaa".into(),
        waiting: result.is_none(),
        runs: Vec::new(),
        result: result.map(str::to_owned),
    }
}

/// How the watch's row ended ("" while it checks).
async fn ci_result(flow: &Flow) -> Option<String> {
    let board = flow.board().await;
    board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            OrchestratorStepKind::CiWatch { result, .. } => {
                Some(result.clone().unwrap_or_default())
            }
            _ => None,
        })
}

/// Push, watch, a turn that ends on [quiet] (w43 B1): the request works until the watch's
/// result turn answers, then is done once, with that answer.
#[tokio::test]
async fn a_ci_watch_keeps_its_request_working_until_its_result_turn() {
    let (flow, request) = watching("owed-ci", Reply::text("")).await;
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    assert_eq!(times_done(&flow, &request).await, 0);
    // The watch ends as `watch_ci`'s does: its row's result, then the message.
    flow.manager
        .orchestrator_step(&flow.conversation, ci_row(Some("passed")))
        .await;
    flow.manager
        .deliver_for(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::Report,
                label: "ci 97c298d5".into(),
                task_id: None,
                text: "[ci 97c298d5 · passed · 1 job]".into(),
            },
            Some(request.clone()),
        )
        .await;
    let board = flow.settled().await;
    assert_eq!(board.requests[&request].state, RequestState::Done);
    assert_eq!(times_done(&flow, &request).await, 1, "done once");
    flow.stop().await;
}

/// The user's Stop while only the CI watch keeps the request working: it ends Stopped, the
/// watch's task ends and its row reads "stopped", and nothing wakes the thread for it later.
#[tokio::test]
async fn stop_during_a_ci_watch_stops_the_request_and_ends_the_watch() {
    let (flow, request) = watching("owed-ci-stop", Reply::text("")).await;
    assert_eq!(stays(&flow, &request).await, RequestState::Working);
    // The watch's own task, as `watch_ci` keeps it.
    let task = tokio::spawn(tokio::time::sleep(Duration::from_secs(600)));
    flow.manager
        .conv(&flow.conversation)
        .unwrap()
        .ci_watches
        .lock()
        .unwrap()
        .push(task.abort_handle());
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow
        .until("the request stopped", |board| {
            board.requests[&request].state == RequestState::Stopped
        })
        .await;
    assert!(task.await.unwrap_err().is_cancelled(), "the watch ended");
    assert_eq!(ci_result(&flow).await.as_deref(), Some("stopped"));
    assert_ne!(board.run, RunState::Running);
    assert_eq!(stays(&flow, &request).await, RequestState::Stopped);
    assert_eq!(times_done(&flow, &request).await, 0);
    flow.stop().await;
}

/// A turn that fails while the CI watch runs: the request reads failed, not working; what
/// the watch brings later works it again as any late event does.
#[tokio::test]
async fn a_failed_turn_stays_failed_while_its_ci_watch_runs() {
    let (flow, request) = watching("owed-ci-failed", Reply::failed("API error 500")).await;
    // The turn's end settles it a moment later (a tab's ready check reads git first).
    flow.until("the request to end", |board| {
        board.requests[&request].state != RequestState::Working
    })
    .await;
    let state = stays(&flow, &request).await;
    assert!(matches!(state, RequestState::Failed { .. }), "{state:?}");
    assert_eq!(ci_result(&flow).await.as_deref(), Some(""), "still watched");
    flow.stop().await;
}
