//! Development fault injection (`M/fault.rs`), the continuity battery's seams (BUILD-PLAN
//! D1.0): an injected provider error ends the running turn as that error, and a planted
//! `learned` line reaches the next worker report of the conversation.

use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent, ProviderKind};
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

/// An error injected into the thread's running turn ends that turn as the CLI's own error would.
/// "Prompt is too long" on the thread no longer fails the request: its generation ends with no
/// note and the request goes on in the successor (STRIP-DOWN-PLAN S22).
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
                if turn.is_orchestrator()
                    && turn
                        .input
                        .contains("The previous session ended without a hand-off note")
                {
                    return Reply::text("Read it in the fresh session.");
                }
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
    // Its first turn failed on the error; the fresh session's answer ended it.
    assert_eq!(ended, vec![RequestState::Done]);
    flow.stop().await;
}
