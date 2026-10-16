//! The thread's CLI guesses the user's next message (G18): kept on the request it follows,
//! never logged as said, and asked for only while the conversation's composer shows.

use std::sync::Arc;

use brigadier_providers::{ProviderEvent, ProviderKind};
use brigadier_store::StreamPage;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::model::streams;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const GUESS: &str = "Run the tests once more.";

#[tokio::test]
async fn the_thread_s_cli_guesses_the_next_message_only_while_its_composer_shows() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w70-suggestion",
            Options {
                thread: provider,
                ..Default::default()
            },
            script(|turn| async move {
                turn.emit(ProviderEvent::PromptSuggestion { text: GUESS.into() })
                    .await;
                Reply::text("Answered.")
            }),
        )
        .await;
        flow.say("Hello").await;
        let board = flow
            .until("the request over with its guess", |board| {
                board.latest_request().is_some_and(|request| {
                    request.suggestion.as_deref() == Some(GUESS)
                        && request.state == crate::work::RequestState::Done
                })
            })
            .await;
        // A newer request starts without one.
        assert_eq!(board.requests.len(), 1);

        // Never logged as something said: no transcript, digest or rebirth reads it.
        let logged = flow
            .core
            .store()
            .read_stream(
                streams::orchestrator(&flow.conversation),
                StreamPage {
                    limit: 1000,
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(
            logged
                .iter()
                .all(|event| !event.payload.get().contains(GUESS)),
            "the guess was logged"
        );

        // Only the thread's CLI guesses, held from its start while no composer shows it.
        // (Other one-shot runs, a title or a Brain pass, never guess.)
        let suggesting = flow.behavior.suggesting.lock().unwrap().clone();
        assert_eq!(
            suggesting.iter().filter(|asks| **asks).count(),
            1,
            "{suggesting:?}"
        );
        assert_eq!(*flow.behavior.suggestion_pauses.lock().unwrap(), [true]);
        flow.manager
            .set_composer_shown(flow.conversation.clone(), true)
            .await;
        flow.manager
            .set_composer_shown(flow.conversation.clone(), false)
            .await;
        assert_eq!(
            *flow.behavior.suggestion_pauses.lock().unwrap(),
            [true, false, true]
        );
        flow.stop().await;
    }
}

/// Each guess reads the whole conversation from the cache again, so a turn that leaves work
/// running (a worker that will wake the thread with its report) gets none: the hold lifts once
/// nothing is left running, before the turn that ends the request.
#[tokio::test]
async fn a_turn_that_starts_a_worker_gets_no_guess_until_nothing_is_left_running() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let behavior = Arc::new(FakeBehavior::default());
        let watched = behavior.clone();
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w70-suggestion-held",
            Options {
                thread: provider,
                behavior: behavior.clone(),
                ..Default::default()
            },
            script(move |turn| {
                let watched = watched.clone();
                async move {
                    if !turn.is_orchestrator() {
                        // While the worker runs, its thread guesses nothing.
                        assert_eq!(watched.suggestion_pauses.lock().unwrap().last(), Some(&true));
                        let reply = turn
                            .call(
                                "submit_report",
                                serde_json::json!({"summary": "Notes live in notes/.", "changes": []}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Reported.");
                    }
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Notes live in notes/.");
                    }
                    if turn.earlier > 0 {
                        return Reply::text("Finished.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            serde_json::json!({"effort": "medium", "title": "Find where notes are kept",
                                               "kind": "scout", "spec": "Find where the notes are kept."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    Reply::text("[quiet]")
                }
            }),
        )
        .await;
        // The user is looking at it from the start.
        flow.manager
            .set_composer_shown(flow.conversation.clone(), true)
            .await;
        flow.say("Where do notes go?").await;
        flow.until("the request over", |board| {
            board.latest_request().is_some_and(|request| {
                request.state == crate::work::RequestState::Done
                    && board.tasks.values().all(|task| task.state.is_final())
            })
        })
        .await;
        // Started free, held while the worker ran, free again for the turn that answered.
        assert_eq!(
            *behavior.suggestion_pauses.lock().unwrap(),
            [false, true, false]
        );
        flow.stop().await;
    }
}
