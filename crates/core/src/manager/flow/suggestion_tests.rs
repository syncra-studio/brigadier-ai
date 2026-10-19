//! The thread's CLI guesses the user's next message (G18): kept on the request it follows,
//! never logged as said, and asked for only while the conversation's composer shows. A Claude
//! Session's CLI never guesses (Claude Code 2.1.301 acknowledges the pause and guesses anyway):
//! its final answer ends with the guess on a `[next: …]` line instead, which never shows.

use std::sync::Arc;

use brigadier_providers::{ProviderEvent, ProviderKind};
use brigadier_store::StreamPage;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::model::{
    ConversationId, ConversationKind, MessageRole, ModelChoice, SetupRequest, streams,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const GUESS: &str = "Run the tests once more.";

async fn chat_on(flow: &Flow, provider: ProviderKind) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Guesses".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
            }),
        )
        .await
        .unwrap()
        .id
}

/// A Session on Codex and a Chat on Claude guess.
#[tokio::test]
async fn the_thread_s_cli_guesses_the_next_message_only_while_its_composer_shows() {
    for (provider, chat) in [(ProviderKind::Codex, false), (ProviderKind::Claude, true)] {
        let flow = Flow::start(
            "20261015-0226-rebuild-wow-w70-suggestion",
            Options {
                thread: provider,
                ..Default::default()
            },
            script(|_| async move {
                Reply {
                    suggestion: Some(GUESS.into()),
                    ..Reply::text("Answered.")
                }
            }),
        )
        .await;
        let id = if chat {
            chat_on(&flow, provider).await
        } else {
            flow.conversation.clone()
        };
        flow.say_to(&id, "Hello").await;
        let board = flow
            .until_in(&id, "the request over with its guess", |board| {
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
                streams::orchestrator(&id),
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
        flow.manager.set_composer_shown(id.clone(), true).await;
        flow.manager.set_composer_shown(id.clone(), false).await;
        assert_eq!(
            *flow.behavior.suggestion_pauses.lock().unwrap(),
            [true, false, true]
        );
        flow.stop().await;
    }
}

/// Claude Code 2.1.301 keeps guessing while paused, each guess reading the whole context from
/// the cache again, so a Claude Session thread is never asked to guess at all.
#[tokio::test]
async fn a_claude_session_thread_is_never_asked_to_guess() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w75-suggestion-off",
        Options::default(),
        script(|_| async move { Reply::text("Answered.") }),
    )
    .await;
    flow.manager
        .set_composer_shown(flow.conversation.clone(), true)
        .await;
    flow.say("Hello").await;
    flow.until("the request over", |board| {
        board
            .latest_request()
            .is_some_and(|request| request.state == crate::work::RequestState::Done)
    })
    .await;
    flow.manager
        .set_composer_shown(flow.conversation.clone(), false)
        .await;
    let suggesting = flow.behavior.suggesting.lock().unwrap().clone();
    assert!(!suggesting.contains(&true), "{suggesting:?}");
    assert!(flow.behavior.suggestion_pauses.lock().unwrap().is_empty());
    flow.stop().await;
}

/// Each guess reads the whole conversation from the cache again, so a turn that leaves work
/// running (a worker that will wake the thread with its report) gets none: the hold lifts once
/// nothing is left running, before the turn that ends the request. (A Session on Codex: a
/// Claude Session never guesses.)
#[tokio::test]
async fn a_turn_that_starts_a_worker_gets_no_guess_until_nothing_is_left_running() {
    let provider = ProviderKind::Codex;
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

/// A guess that comes while a newer turn runs (Codex answers its request apart, so one can
/// arrive after the user's next message started a turn) follows nothing on screen: dropped,
/// and the newer request ends without it.
#[tokio::test]
async fn a_guess_that_comes_during_a_newer_turn_is_dropped() {
    let flow = Flow::start(
        "20261015-0226-rebuild-wow-w77-suggestion-late",
        Options {
            thread: ProviderKind::Codex,
            ..Default::default()
        },
        script(|turn| async move {
            if turn.input.contains("Second") {
                turn.emit(ProviderEvent::PromptSuggestion { text: GUESS.into() })
                    .await;
                return Reply::text("Answered again.");
            }
            Reply::text("Answered.")
        }),
    )
    .await;
    flow.say("First").await;
    let first = flow
        .until("the first request over", |board| {
            board
                .latest_request()
                .is_some_and(|request| request.state == crate::work::RequestState::Done)
        })
        .await
        .latest_request()
        .unwrap()
        .id
        .clone();
    flow.say("Second").await;
    let board = flow
        .until("the second request over", |board| {
            board.latest_request().is_some_and(|request| {
                request.id != first && request.state == crate::work::RequestState::Done
            })
        })
        .await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.suggestion.is_none()),
        "{:?}",
        board.requests
    );
    flow.stop().await;
}

/// The answers' texts in `id`, as stored.
async fn answers(flow: &Flow, id: &ConversationId) -> Vec<String> {
    flow.core
        .conversation_view(id.clone(), 50)
        .await
        .unwrap()
        .messages
        .messages
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect()
}

/// A Claude Session thread's final answer carries the guess on its last line, at no request
/// of its own: the line never shows in the answer, and the guess is kept on the request as a
/// CLI's would be. An answer without the line keeps none, as before.
#[tokio::test]
async fn a_claude_session_thread_s_answer_carries_its_guess_on_a_hidden_last_line() {
    let flow = Flow::start(
        "20261017-2108-workspaces-rework-w89-next-line",
        Options::default(),
        script(|turn| async move {
            if turn.input.contains("Second") {
                return Reply::text("Answered again.");
            }
            Reply::text(format!("Answered.\n\n[next: {GUESS}]"))
        }),
    )
    .await;
    flow.say("First").await;
    let first = flow
        .until("the request over with its guess", |board| {
            board.latest_request().is_some_and(|request| {
                request.suggestion.as_deref() == Some(GUESS)
                    && request.state == crate::work::RequestState::Done
            })
        })
        .await
        .latest_request()
        .unwrap()
        .id
        .clone();
    assert_eq!(answers(&flow, &flow.conversation).await, ["Answered."]);
    // Its CLI was never asked to guess.
    let suggesting = flow.behavior.suggesting.lock().unwrap().clone();
    assert!(!suggesting.contains(&true), "{suggesting:?}");

    flow.say("Second").await;
    let board = flow
        .until("the second request over", |board| {
            board.latest_request().is_some_and(|request| {
                request.id != first && request.state == crate::work::RequestState::Done
            })
        })
        .await;
    assert_eq!(board.latest_request().unwrap().suggestion, None);
    assert_eq!(
        answers(&flow, &flow.conversation).await,
        ["Answered.", "Answered again."]
    );
    flow.stop().await;
}

/// A Codex thread keeps its CLI's guess: a `[next: …]` line it writes anyway is hidden, and
/// its words are no guess.
#[tokio::test]
async fn a_codex_thread_s_next_line_is_hidden_and_no_guess() {
    let flow = Flow::start(
        "20261017-2108-workspaces-rework-w89-next-codex",
        Options {
            thread: ProviderKind::Codex,
            ..Default::default()
        },
        script(|_| async move { Reply::text("Answered.\n[next: Something else]") }),
    )
    .await;
    flow.say("Hello").await;
    let board = flow
        .until("the request over", |board| {
            board
                .latest_request()
                .is_some_and(|request| request.state == crate::work::RequestState::Done)
        })
        .await;
    assert_eq!(board.latest_request().unwrap().suggestion, None);
    assert_eq!(answers(&flow, &flow.conversation).await, ["Answered."]);
    flow.stop().await;
}
