//! A request's turn ends it, as a plain CLI's (S16b): a failure stays a failure while a CI
//! watch it started still runs.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;
use crate::work::{OrchestratorStepKind, RequestState, RunState};

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
