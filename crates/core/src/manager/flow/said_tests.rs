//! Every message Brigadier sends a session's CLI is noted for whose words it holds (L12): the
//! thread tells the user's own words from Brigadier's notes, frames, steers and nudges by that
//! note, never by how the words begin.
use super::{FakeBehavior, Flow, Options, Reply};
use crate::manager::conversation::Envelope;
use crate::work::InjectionKind;
use brigadier_providers::session_file::Said;
use std::sync::{Arc, Mutex};

type Inputs = Arc<Mutex<Vec<String>>>;

/// A session whose CLI keeps a session file, as Claude Code does. "Hold." takes two steers in
/// its turn; "Work a while." takes two as well, the user's queued message and its withdrawal.
async fn flow(name: &str) -> (Flow, Inputs) {
    let inputs: Inputs = Arc::default();
    let seen = inputs.clone();
    let flow = Flow::start(
        name,
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
                if turn.conversation().is_some() {
                    seen.lock().unwrap().push(turn.input.clone());
                    if turn.input == "Hold." || turn.input == "Work a while." {
                        for _ in 0..2 {
                            if let Some(steered) = turn.steered().await {
                                seen.lock().unwrap().push(steered);
                            }
                        }
                    }
                }
                Reply::text("Done.")
            })
        }),
    )
    .await;
    (flow, inputs)
}

async fn until_given(flow: &Flow, inputs: &Inputs, needle: &str) -> String {
    let needle = needle.to_owned();
    flow.until(&format!("the CLI to get {needle}"), |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains(&needle))
    })
    .await;
    inputs
        .lock()
        .unwrap()
        .iter()
        .find(|input| input.contains(&needle))
        .cloned()
        .unwrap()
}

#[tokio::test]
async fn a_turns_message_is_the_users_and_a_note_after_their_words_is_brigadiers() {
    let (flow, inputs) = flow("said-turns").await;
    flow.say("Fix the test.").await;
    flow.settled().await;
    assert_eq!(flow.said_in_thread("Fix the test."), [Some(Said::User)]);

    // A note that waits for the user's next message goes after their words: only theirs show.
    flow.manager
        .note_next_turn(&flow.conversation, "[answer] The user picked B.".into())
        .await;
    flow.say("Go on.").await;
    flow.settled().await;
    let sent = until_given(&flow, &inputs, "Go on.").await;
    assert!(sent.contains("[answer] The user picked B."), "{sent}");
    assert_eq!(
        flow.said_in_thread("Go on."),
        [Some(Said::Both {
            user: "Go on.".into()
        })]
    );

    // Another session's message on a turn of its own is Brigadier's.
    let peer = "[message from session \"Docs\" (c-1) · sm-0123456789ab] Done with the docs.";
    flow.manager
        .deliver(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::PeerMessage,
                label: "peer".into(),
                task_id: None,
                text: peer.into(),
            },
        )
        .await;
    until_given(&flow, &inputs, "Done with the docs.").await;
    flow.settled().await;
    assert_eq!(
        flow.said_in_thread("Done with the docs."),
        [Some(Said::Brigadier)]
    );
    flow.stop().await;
}

#[tokio::test]
async fn brigadiers_steers_are_brigadiers_and_the_users_steer_is_theirs() {
    let (flow, inputs) = flow("said-steers").await;
    flow.say("Hold.").await;
    until_given(&flow, &inputs, "Hold.").await;
    // The user stopped a worker: Brigadier tells the running turn.
    let stopped = "[worker] The user stopped task-3 \"Fix the flaky test\". You didn't stop it and nothing failed.";
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    flow.manager.tell_thread_now(&conv, stopped.into()).await;
    until_given(&flow, &inputs, "The user stopped task-3").await;
    assert_eq!(flow.noted_said(stopped), Some(Said::Brigadier));
    // The user's own steer, with a note that waited for their next message after it: only
    // their words show.
    flow.manager
        .note_next_turn(&flow.conversation, "[answer] The user picked A.".into())
        .await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "And the docs.".into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    let steered = until_given(&flow, &inputs, "And the docs.").await;
    assert!(steered.contains("[answer] The user picked A."), "{steered}");
    assert_eq!(
        flow.noted_said(&steered),
        Some(Said::Both {
            user: "And the docs.".into()
        })
    );
    flow.settled().await;

    // Another session's message steered in.
    flow.say("Hold.").await;
    flow.until("the second hold", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|input| *input == "Hold.")
            .count()
            == 2
    })
    .await;
    let peer = "[message from session \"Docs\" (c-1) · sm-0123456789ab] Steered in.";
    let taken = flow
        .manager
        .steer_or_queue(
            &flow.conversation,
            Envelope {
                kind: InjectionKind::PeerMessage,
                label: "peer".into(),
                task_id: None,
                text: peer.into(),
            },
        )
        .await;
    assert_eq!(taken, Some(true));
    until_given(&flow, &inputs, "Steered in.").await;
    assert_eq!(flow.noted_said(peer), Some(Said::Brigadier));
    flow.stop().await;
}

#[tokio::test]
async fn a_queued_messages_frame_and_its_withdrawal_are_brigadiers() {
    let (flow, inputs) = flow("said-queued").await;
    flow.say("Work a while.").await;
    until_given(&flow, &inputs, "Work a while.").await;
    // Sent while the turn works: steered in under its frame to judge.
    flow.say("Also the docs.").await;
    let framed = until_given(&flow, &inputs, "Also the docs.").await;
    assert!(framed.starts_with("[queued message "), "{framed}");
    assert_eq!(
        flow.noted_said(&framed),
        Some(Said::Both {
            user: "Also the docs.".into()
        })
    );
    // Deleted once the thread has it: Brigadier tells the thread to leave it.
    let item = flow.board().await.queue.items[0].id.clone();
    flow.manager
        .delete_queued(&flow.conversation, &item)
        .await
        .unwrap();
    let withdrawn = until_given(&flow, &inputs, "withdrawn").await;
    assert_eq!(flow.noted_said(&withdrawn), Some(Said::Brigadier));
    flow.stop().await;
}

#[tokio::test]
async fn a_chats_compact_the_user_asked_for_is_theirs() {
    use crate::model::{ConversationKind, ModelChoice, SetupRequest};
    use brigadier_providers::ProviderKind;
    let (flow, _) = flow("said-chat-compact").await;
    let chat = flow
        .manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Compact".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission: Default::default(),
            }),
        )
        .await
        .unwrap()
        .id;
    flow.say_to(&chat, "Grow.").await;
    flow.settled_in(&chat).await;
    flow.manager.compact(chat.clone()).await.unwrap();
    assert_eq!(flow.manager.noted_said(&chat, "/compact"), Some(Said::User));
    flow.stop().await;
}
