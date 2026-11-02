//! Development fault injection (`M/fault.rs`), the continuity battery's seams (BUILD-PLAN
//! D1.0): an injected provider error ends the running turn as that error, and a planted
//! `learned` line reaches the next worker report of the conversation.

use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent, ProviderKind};
use tokio::sync::Notify;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::fault::{FaultKey, InjectedError};
use crate::thread_view::TurnEnd;
use crate::work::RequestState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// An error injected into the thread's running turn ends that turn as the CLI's own error would.
/// "Prompt is too long" on the thread doesn't fail the request: its CLI compacts in place and the
/// request goes again by itself (D15).
#[tokio::test]
async fn an_injected_context_window_error_ends_the_threads_turn() {
    let (started, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (on_start, on_go) = (started.clone(), go.clone());
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flow = Flow::start(
        "fault-context-window",
        Options::default(),
        script(move |turn| {
            let (on_start, on_go, first) = (on_start.clone(), on_go.clone(), first.clone());
            async move {
                // The request again, after the compaction.
                if turn.is_orchestrator()
                    && turn.input.contains("Read the file.")
                    && !first.swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    return Reply::text("Read it after compacting.");
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
    // Its first turn failed on the error; its answer after the compaction ended it, in the
    // same CLI session.
    assert_eq!(ended, vec![RequestState::Done]);
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 1);
    flow.stop().await;
}

/// The thread's model declines its turn (D143): the turn fails as declined, is never asked
/// again by itself, and its view names the next model of the routing for "Continue on".
#[tokio::test]
async fn an_injected_refusal_names_the_next_model_and_never_retries() {
    let (started, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (on_start, on_go) = (started.clone(), go.clone());
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = asked.clone();
    let flow = Flow::start(
        "fault-refusal",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let (on_start, on_go, count) = (on_start.clone(), on_go.clone(), count.clone());
            async move {
                if turn.is_orchestrator() && turn.input.contains("Answer this.") {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    on_start.notify_one();
                    on_go.notified().await;
                    // Its next event fires the armed fault.
                    turn.emit(ProviderEvent::Notice {
                        level: NoticeLevel::Info,
                        message: "answering".into(),
                    })
                    .await;
                    return Reply::text("Partly.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Answer this.").await;
    started.notified().await;
    flow.manager
        .debug_inject_error(
            FaultKey::Conversation(flow.conversation.clone()),
            ProviderKind::Claude,
            InjectedError::Refusal,
            0,
        )
        .await
        .unwrap();
    go.notify_one();
    flow.until("the request ended", |board| {
        board
            .requests
            .values()
            .any(|request| request.state != RequestState::Working)
    })
    .await;
    flow.settled().await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let turn = view.turns.last().unwrap();
    assert_eq!(turn.end, Some(TurnEnd::Failed), "{turn:?}");
    assert_eq!(
        turn.error_kind,
        Some(brigadier_providers::ErrorKind::Policy)
    );
    let from = flow.manager.core.conversation(&flow.conversation).unwrap();
    let next = view.next_model.expect("the routing's next model");
    let Some(crate::model::Setup::Session { orchestrator, .. }) = from.setup else {
        panic!("a session: {:?}", from.setup);
    };
    assert_ne!(next, orchestrator);
    assert_eq!(
        asked.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "asked again by itself"
    );
    flow.stop().await;
}

/// A limit injected into the thread's running turn ends that turn as a CLI at its limit does:
/// failed, never as the user's Stop, though the CLI itself says it was interrupted. Every reader
/// of the CLI's events, the thread's turn recorder too, gets them with the fault applied.
#[tokio::test]
async fn an_injected_limit_ends_the_threads_turn_failed_never_stopped() {
    let (started, go) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (on_start, on_go) = (started.clone(), go.clone());
    let flow = Flow::start(
        "fault-limit-turn-end",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(move |turn| {
            let (on_start, on_go) = (on_start.clone(), on_go.clone());
            async move {
                if turn.is_orchestrator() && turn.input.contains("Write the marks.") {
                    on_start.notify_one();
                    on_go.notified().await;
                    // Its next event fires the armed limit, which interrupts the CLI.
                    turn.emit(ProviderEvent::Notice {
                        level: NoticeLevel::Info,
                        message: "writing".into(),
                    })
                    .await;
                    assert!(turn.stopped().await, "the injected limit interrupts it");
                    return Reply::text("");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Write the marks.").await;
    started.notified().await;
    flow.manager
        .debug_inject_limit(
            FaultKey::Conversation(flow.conversation.clone()),
            ProviderKind::Claude,
            "five_hour".into(),
            1,
            0,
        )
        .await
        .unwrap();
    go.notify_one();
    let first_end = || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.first().and_then(|turn| turn.end)
    };
    super::eventually("the limited turn ends", || first_end().is_some()).await;
    assert_eq!(first_end(), Some(TurnEnd::Failed));
    flow.stop().await;
}
