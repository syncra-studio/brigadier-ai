//! Every thread and Chat guesses the user's next message (G18, Q89): its final answer ends with
//! the guess on a `[next: …]` line, which never shows and is kept on the request it follows.
//! No CLI is asked for a guess of its own (Claude Code's `--prompt-suggestions`, Codex's
//! `thread/prediction/request`): each was a request more per turn.

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::NEXT_MESSAGE;
use crate::model::{ConversationId, ConversationKind, MessageRole, ModelChoice, SetupRequest};

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

/// A Session thread on Codex and Chats on either CLI: the instructions ask for the line, the
/// answer is stored without it, and its words are the request's guess.
#[tokio::test]
async fn a_codex_thread_and_chats_on_either_cli_carry_the_guess_on_a_hidden_last_line() {
    for (provider, chat) in [
        (ProviderKind::Codex, false),
        (ProviderKind::Claude, true),
        (ProviderKind::Codex, true),
    ] {
        let flow = Flow::start(
            "20261017-2108-workspaces-rework-w95-next-everywhere",
            Options {
                thread: provider,
                ..Default::default()
            },
            script(|turn| async move {
                if turn.input.contains("Hello") {
                    assert!(turn.prompt.contains(NEXT_MESSAGE), "{}", turn.prompt);
                }
                Reply::text(format!("Answered.\n\n[next: {GUESS}]"))
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
        assert_eq!(board.requests.len(), 1);
        assert_eq!(
            answers(&flow, &id).await,
            ["Answered."],
            "{provider:?} {chat}"
        );
        flow.stop().await;
    }
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
/// of its own: the line never shows in the answer, and the guess is kept on the request. An
/// answer without the line keeps none, and the next request starts without one.
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
