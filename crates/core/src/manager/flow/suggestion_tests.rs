//! The thread's CLI guesses the user's next message (G18): kept on the request it follows,
//! never logged as said, and asked for only while the conversation's composer shows.

use std::sync::Arc;

use brigadier_providers::{ProviderEvent, ProviderKind};
use brigadier_store::StreamPage;

use super::{Flow, Options, Reply, Script, Turn};
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
