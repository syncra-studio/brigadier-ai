//! A message from one session reaches another however that one stands: idle, sleeping, busy
//! (steered in, or queued when steering is refused), and never one archived, unknown or the
//! sender itself (SESSION-MESSAGING-PLAN.md §5.3).

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::peer_kit::{Peers, message_id, only};
use super::{FakeBehavior, Flow, Options, Reply};
use crate::board::Board;
use crate::model::{ConversationId, Lifecycle, MessageRole, PeerState};

/// Whether the message `id` stands at `state` on `board`.
fn stands(board: &Board, id: &str, state: &PeerState) -> bool {
    board
        .peer_messages
        .get(id)
        .is_some_and(|record| record.state == *state)
}

/// What the receiver `to` stores of message `id` from `from`: a peer's transcript message
/// holding `text` (never the user's), and a request of its own opened for it.
async fn assert_received(
    flow: &Flow,
    to: &ConversationId,
    from: &ConversationId,
    id: &str,
    text: &str,
) {
    let messages = flow.core.all_messages(to).await.unwrap();
    let message = messages
        .iter()
        .find(|message| message.id == id)
        .expect("the received message is in the transcript");
    assert_eq!(message.role, MessageRole::Peer);
    assert_eq!(message.text, text);
    let peer = message.peer.as_ref().expect("it says who sent it");
    assert_eq!(peer.from, *from);
    assert!(
        messages
            .iter()
            .all(|message| message.role != MessageRole::User || !message.text.contains(text)),
        "nothing of it is the user's"
    );
    let board = flow.board_of(to).await;
    let request = &board.requests[id];
    assert_eq!(request.from_peer.as_ref(), Some(from));
}

/// An idle session in another project takes a message in a turn of its own, whose input is
/// the header naming the sender, then the text; both sides record it seen.
#[tokio::test]
async fn a_message_starts_a_turn_of_an_idle_session_in_another_project() {
    let peers = Peers::new();
    let flow = Flow::start("peer-idle", Options::default(), peers.script()).await;
    let docs = flow
        .add_session("Docs", Some("Website"), ProviderKind::Codex)
        .await;
    let text = "The schema changed: `name` is now required.";
    let sent = peers.sends(&flow.conversation, &docs, text);
    flow.say("Tell the docs session the schema changed.").await;
    flow.settled().await;
    let (reply, failed) = only(&sent);
    assert!(!failed, "{reply}");
    assert!(reply.contains("\"state\":\"delivered\""), "{reply}");
    let id = message_id(&reply);
    flow.until_in(&docs, "the message seen", |board| {
        stands(board, &id, &PeerState::Seen)
    })
    .await;
    // The turn's answer goes back to the sender as the reply.
    flow.until_in(&docs, "the message answered", |board| {
        stands(board, &id, &PeerState::Replied)
    })
    .await;
    flow.settled_in(&docs).await;
    let inputs = peers.inputs_of(&docs);
    assert_eq!(inputs.len(), 1, "{inputs:?}");
    assert!(
        inputs[0].contains(&format!(
            "[message from session \"Flow\" in Flow · session {} · id {id}]\n{text}",
            flow.conversation
        )),
        "{}",
        inputs[0]
    );
    assert_received(&flow, &docs, &flow.conversation, &id, text).await;
    let sender = flow
        .until("the sender's record replied", |board| {
            stands(board, &id, &PeerState::Replied)
        })
        .await;
    let record = &sender.peer_messages[&id];
    assert!(record.outgoing && record.peer == docs);
    assert!(record.sent_at_ms.is_some());
    assert_eq!(record.hop, 1);
    flow.stop().await;
}

/// A sleeping session in the same project wakes for a message, resumes and takes it.
#[tokio::test]
async fn a_message_wakes_a_sleeping_session_in_the_same_project() {
    let peers = Peers::new();
    let flow = Flow::start("peer-sleep", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    flow.say_to(&api, "Look around.").await;
    flow.settled_in(&api).await;
    flow.manager.hibernate(api.clone()).await.unwrap();
    assert_eq!(
        flow.core.conversation(&api).unwrap().lifecycle,
        Lifecycle::Hibernated
    );
    let sent = peers.sends(&flow.conversation, &api, "Please add a `name` column.");
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let id = message_id(&only(&sent).0);
    flow.until_in(&api, "the message seen", |board| {
        stands(board, &id, &PeerState::Seen)
    })
    .await;
    assert_eq!(
        flow.core.conversation(&api).unwrap().lifecycle,
        Lifecycle::Active
    );
    assert_eq!(peers.count_in(&api, &format!("· id {id}]")), 1);
    flow.stop().await;
}

/// A message for a session whose turn runs is steered into that turn: no turn of its own
/// starts, and the turn answers it on its own, inside the running request's block.
#[tokio::test]
async fn a_message_is_steered_into_a_busy_sessions_running_turn() {
    let peers = Peers::new();
    let flow = Flow::start("peer-steer", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let steered: Arc<std::sync::Mutex<Option<String>>> = Arc::default();
    let kept = steered.clone();
    peers.on(&api, move |turn| {
        let kept = kept.clone();
        async move {
            let input = turn.steered().await;
            if let Some(input) = &input {
                let id = super::peer_kit::header_id(input).unwrap();
                let from = super::peer_kit::header_session(input).unwrap();
                let (_, failed) =
                    super::peer_kit::send(&turn, &from, "Added the `name` column.", Some(&id))
                        .await;
                assert!(!failed);
            }
            *kept.lock().unwrap() = input;
            Reply::text("Done, with the column.")
        }
    });
    flow.say_to(&api, "Refactor the models.").await;
    let working = flow
        .until_in(&api, "its turn running", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == crate::work::RequestState::Working)
        })
        .await;
    let user_request = working.requests.keys().next().unwrap().clone();
    // The turn has begun: steering reaches it.
    while peers.inputs_of(&api).is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let text = "Also add a `name` column while you're in there.";
    let sent = peers.sends(&flow.conversation, &api, text);
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let (reply, failed) = only(&sent);
    assert!(
        !failed && reply.contains("\"state\":\"delivered\""),
        "{reply}"
    );
    let id = message_id(&reply);
    let board = flow.settled_in(&api).await;
    let input = steered
        .lock()
        .unwrap()
        .clone()
        .expect("the turn read the steer");
    assert!(input.contains(&format!("· id {id}]\n{text}")), "{input}");
    assert_eq!(peers.inputs_of(&api).len(), 1, "no turn of its own");
    // The turn's own reply to it went back.
    assert!(stands(&board, &id, &PeerState::Replied));
    assert_eq!(
        board.requests[&id].steered_into.as_deref(),
        Some(user_request.as_str())
    );
    assert_received(&flow, &api, &flow.conversation, &id, text).await;
    flow.stop().await;
}

/// When the busy session's CLI refuses the steer, the message waits in its inbox and the
/// next turn takes it: the sender hears `queued`.
#[tokio::test]
async fn a_message_waits_for_the_next_turn_when_steering_is_refused() {
    let peers = Peers::new();
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("peer-queue", options, peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    peers.on(&api, move |turn| {
        let held = held.clone();
        async move {
            if turn.input.contains("[message from session") {
                return Reply::text("Added the column.");
            }
            held.notified().await;
            Reply::text("Refactored.")
        }
    });
    flow.say_to(&api, "Refactor the models.").await;
    while peers.inputs_of(&api).is_empty() {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let text = "Please add a `name` column.";
    let sent = peers.sends(&flow.conversation, &api, text);
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let (reply, failed) = only(&sent);
    assert!(!failed && reply.contains("\"state\":\"queued\""), "{reply}");
    let id = message_id(&reply);
    assert!(stands(
        &flow.board_of(&api).await,
        &id,
        &PeerState::Delivered
    ));
    release.notify_one();
    flow.until_in(&api, "the message seen", |board| {
        stands(board, &id, &PeerState::Seen)
    })
    .await;
    flow.settled_in(&api).await;
    let inputs = peers.inputs_of(&api);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert!(
        inputs[1].contains(&format!("· id {id}]\n{text}")),
        "{}",
        inputs[1]
    );
    assert_received(&flow, &api, &flow.conversation, &id, text).await;
    flow.stop().await;
}

/// An archived session, an id that names no session, and the sender itself are refused in
/// plain words, and nothing is recorded or woken.
#[tokio::test]
async fn a_message_to_an_archived_unknown_or_own_session_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-refused", Options::default(), peers.script()).await;
    let old = flow.add_session("Old", None, ProviderKind::Claude).await;
    flow.manager.archive(old.clone()).await.unwrap();
    let own = flow.conversation.clone();
    let replies: super::peer_kit::Sent = Arc::default();
    let kept = replies.clone();
    let targets = [old.clone(), ConversationId("nope".into()), own];
    peers.on(&flow.conversation, move |turn| {
        let (kept, targets) = (kept.clone(), targets.clone());
        async move {
            for to in &targets {
                let reply = super::peer_kit::send(&turn, to, "Hello.", None).await;
                kept.lock().unwrap().push(reply);
            }
            Reply::text("Done.")
        }
    });
    flow.say("Say hello around.").await;
    let board = flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(
        replies,
        vec![
            ("Old is archived. Nothing was sent.".to_owned(), true),
            ("No session with that id.".to_owned(), true),
            ("That's this session.".to_owned(), true),
        ]
    );
    assert!(board.peer_messages.is_empty());
    assert!(peers.inputs_of(&old).is_empty());
    assert_eq!(
        flow.core.conversation(&old).unwrap().lifecycle,
        Lifecycle::Archived
    );
    assert!(flow.board_of(&old).await.peer_messages.is_empty());
    flow.stop().await;
}

/// The receiver's turn that took a message hits its usage limit: the message goes again with
/// the turn handed over to the stand-in, which does the work.
#[tokio::test]
async fn a_message_goes_on_with_a_turn_a_usage_limit_hands_over() {
    let peers = Peers::new();
    let flow = Flow::start("peer-limit", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    peers.on(&api, |turn| async move {
        match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Added the column."),
        }
    });
    let text = "Please add a `name` column.";
    let sent = peers.sends(&flow.conversation, &api, text);
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let id = message_id(&only(&sent).0);
    let board = flow
        .until_in(&api, "the stand-in's turn done", |board| {
            board
                .requests
                .get(&id)
                .is_some_and(|request| request.state == crate::work::RequestState::Done)
        })
        .await;
    assert!(!stands(&board, &id, &PeerState::Delivered));
    // The stand-in's answer goes back as the reply.
    flow.until_in(&api, "the message answered", |board| {
        stands(board, &id, &PeerState::Replied)
    })
    .await;
    let inputs = peers.inputs_of(&api);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert!(
        inputs
            .iter()
            .all(|input| input.contains(&format!("· id {id}]\n{text}"))),
        "{inputs:?}"
    );
    assert_eq!(
        flow.core
            .conversation(&api)
            .unwrap()
            .fallback
            .expect("Codex stands in")
            .choice
            .provider,
        ProviderKind::Codex
    );
    flow.stop().await;
}
