//! Development fault injection (`M/fault.rs`), the continuity battery's seams (BUILD-PLAN
//! D1.0): an injected provider error ends the running turn as that error, and a planted
//! `learned` line reaches the next worker report of the conversation.

use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent, ProviderKind};
use serde_json::json;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::fault::{FaultKey, InjectedError};
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// An error injected into the thread's running turn ends that turn as the CLI's own error would:
/// the request fails with "Prompt is too long".
#[tokio::test]
async fn an_injected_context_window_error_ends_the_threads_turn() {
    let (started, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (on_start, on_go) = (started.clone(), go.clone());
    let flow = Flow::start(
        "fault-context-window",
        Options::default(),
        script(move |turn| {
            let (on_start, on_go) = (on_start.clone(), on_go.clone());
            async move {
                if turn.is_orchestrator() && turn.input.contains("Read the file.") {
                    on_start.notify_one();
                    on_go.notified().await;
                    // Its next event fires the armed fault.
                    turn.emit(ProviderEvent::Notice {
                        level: NoticeLevel::Info,
                        message: "reading".into(),
                    })
                    .await;
                    return Reply::text("Read it.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Read the file.").await;
    started.notified().await;
    flow.manager
        .debug_inject_error(
            FaultKey::Conversation(flow.conversation.clone()),
            ProviderKind::Claude,
            InjectedError::ContextWindow,
            0,
        )
        .await
        .unwrap();
    go.notify_one();
    let board = flow
        .until("the request ended", |board| {
            board
                .requests
                .values()
                .any(|request| request.state != RequestState::Working)
        })
        .await;
    let ended: Vec<_> = board.requests.values().map(|r| r.state.clone()).collect();
    assert!(
        ended.iter().any(|state| matches!(
            state,
            RequestState::Failed { error } if error.contains("Prompt is too long")
        )),
        "{ended:?}"
    );
    flow.stop().await;
}

/// A planted `learned` line is added to the next report a worker of the conversation submits,
/// once.
#[tokio::test]
async fn a_planted_learned_line_reaches_the_next_worker_report_once() {
    const PLANTED: &str = "The data file is written atomically.";
    let flow = Flow::start(
        "fault-planted-learned",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("List the files.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "List the files",
                                   "kind": "scout", "spec": "List the files."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "One file.", "learned": "There is one file."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.manager
        .debug_plant_learned(flow.conversation.clone(), PLANTED.into());
    flow.say("List the files.").await;
    let board = flow
        .until("the scout reported", |board| {
            board.tasks.values().any(|task| task.report.is_some())
        })
        .await;
    let report = board
        .tasks
        .values()
        .find_map(|task| task.report.clone())
        .unwrap();
    assert_eq!(
        report.learned,
        vec!["There is one file.".to_owned(), PLANTED.to_owned()]
    );
    // Taken once: nothing is left for a later report.
    assert!(flow.manager.planted_learned(&flow.conversation).is_empty());
    flow.stop().await;
}
