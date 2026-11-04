//! Try again on a session turn that failed to launch answers the user's own message again
//! (#660): the retry is that request's turn, the CLI reads the prompt once, and the thread has
//! one user message for it, not a "Try again." of its own. Every platform: the CLI is the
//! flow's in-process fake.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderEvent;

use super::{FakeBehavior, Flow, Options, Reply};
use crate::model::MessageRole;
use crate::work::{RequestState, RunState};

#[tokio::test]
async fn try_again_after_a_failed_launch_answers_the_original_prompt_once() {
    let inputs = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = inputs.clone();
    let flow = Flow::start(
        "retry-failed-launch",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(turn.input.clone());
                Reply::text("Added.")
            })
        }),
    )
    .await;
    let id = flow.conversation.clone();
    flow.behavior.fail_starts.store(true, Ordering::SeqCst);
    flow.say("Add a test.").await;
    let board = flow
        .until("the launch to fail", |board| board.run == RunState::Failed)
        .await;
    let request = board.latest_request().expect("its request").id.clone();
    assert!(inputs.lock().unwrap().is_empty(), "the CLI never started");

    flow.behavior.fail_starts.store(false, Ordering::SeqCst);
    flow.manager
        .regenerate(id.clone(), request.clone())
        .await
        .unwrap();
    flow.until("the retry to answer", |board| {
        board.run != RunState::Failed && board.requests[&request].state == RequestState::Done
    })
    .await;
    flow.settled().await;

    // The CLI read the user's prompt once, in one turn, and no "Try again." with it.
    let all = inputs.lock().unwrap().clone();
    assert_eq!(all.len(), 1, "{all:#?}");
    assert_eq!(all[0].matches("Add a test.").count(), 1, "{all:#?}");
    assert!(!all[0].contains("Try again."), "{all:#?}");
    // One user message is stored, the original.
    let users: Vec<_> = flow
        .manager
        .core
        .all_messages(&id)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::User)
        .map(|message| (message.id, message.text))
        .collect();
    assert_eq!(users, [(request.clone(), "Add a test.".to_owned())]);
    // The thread's one turn is the original request's, and shows its prompt once.
    let view = flow.manager.thread_view(&id).unwrap();
    assert_eq!(view.turns.len(), 1, "{view:#?}");
    assert!(
        view.turns[0].requests.contains(&request) || view.turns[0].user_message == request,
        "{view:#?}"
    );
    let prompts = view
        .entries
        .iter()
        .filter(|entry| {
            matches!(&entry.event, ProviderEvent::Message { text, .. } if text.contains("Add a test."))
        })
        .count();
    assert_eq!(prompts, 1, "{view:#?}");
    flow.stop().await;
}
