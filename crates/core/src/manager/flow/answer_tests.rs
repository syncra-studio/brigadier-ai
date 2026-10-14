//! A turn's final answer as the app reads it: when its CLI said it started, kept on the stored
//! message so the work folds there after a reload, and when the request finished, kept on the
//! conversation for its sidebar row's unread dot.

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script};
use crate::model::MessageRole;

fn answering() -> Script {
    Arc::new(|_| {
        Box::pin(async {
            Reply {
                text: "It has 3 lines.".into(),
                answer: true,
                ..Reply::default()
            }
        })
    })
}

#[tokio::test]
async fn a_final_answer_keeps_when_it_started_and_the_request_when_it_finished() {
    let flow = Flow::start(
        "final-answer",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        answering(),
    )
    .await;
    flow.say("How many lines?").await;
    flow.settled().await;

    let view = flow
        .core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap();
    let answer = view
        .messages
        .messages
        .iter()
        .find(|message| message.role == MessageRole::Assistant)
        .expect("the answer");
    let started = answer.answer_at_ms.expect("its start");
    assert!(
        started <= answer.created_at_ms,
        "{started} vs {}",
        answer.created_at_ms
    );
    let finished = view.conversation.finished_at_ms.expect("its finish");
    assert!(
        finished >= answer.created_at_ms,
        "{finished} vs {}",
        answer.created_at_ms
    );

    // A daemon started again on the same store knows both.
    let reloaded = crate::Core::load(flow.core.store().clone()).await.unwrap();
    assert_eq!(
        reloaded
            .conversation(&flow.conversation)
            .unwrap()
            .finished_at_ms,
        Some(finished)
    );
    let again = reloaded
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap();
    assert!(
        again
            .messages
            .messages
            .iter()
            .any(|message| message.answer_at_ms == Some(started))
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_reply_its_cli_didnt_call_the_answer_has_no_start() {
    let flow = Flow::start(
        "plain-answer",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Hi there.") })),
    )
    .await;
    flow.say("Say hi.").await;
    flow.settled().await;
    let view = flow
        .core
        .conversation_view(flow.conversation.clone(), 50)
        .await
        .unwrap();
    assert!(
        view.messages
            .messages
            .iter()
            .filter(|message| message.role == MessageRole::Assistant)
            .all(|message| message.answer_at_ms.is_none())
    );
    assert!(view.conversation.finished_at_ms.is_some());
    flow.stop().await;
}
