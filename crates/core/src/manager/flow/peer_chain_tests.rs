//! The engine's loop rules for messages between sessions (SESSION-MESSAGING-PLAN.md §5.5):
//! every message carries the chain of the input that led to it, eight go back and forth with
//! no one's word before the ninth is refused, and one session sends another at most twenty a
//! minute.

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::peer_kit::{Peers, Sent, message_id, only, send};
use super::{FakeBehavior, Flow, Options, Reply};
use crate::board::PeerRecord;
use crate::model::PeerState;

/// The record of message `id` the receiver keeps.
async fn received(flow: &Flow, to: &crate::model::ConversationId, id: &str) -> PeerRecord {
    flow.until_in(to, "the message received", |board| {
        board.peer_messages.contains_key(id)
    })
    .await
    .peer_messages[id]
        .clone()
}

/// A message forwarded on keeps the chain of the user's message that started it and counts
/// one hop more at each session: a peer's message never starts a chain of its own.
#[tokio::test]
async fn a_forwarded_message_keeps_its_chain_and_counts_hops() {
    let peers = Peers::new();
    let flow = Flow::start("peer-forward", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let docs = flow
        .add_session("Docs", Some("Website"), ProviderKind::Claude)
        .await;
    let first = peers.sends(&flow.conversation, &api, "The schema changed.");
    let second = peers.sends(&api, &docs, "Forwarded: the schema changed.");
    flow.say("Tell the API session the schema changed.").await;
    flow.settled().await;
    let one = message_id(&only(&first).0);
    flow.until_in(&api, "the forward sent", |_| {
        second.lock().unwrap().len() == 1
    })
    .await;
    let two = message_id(&only(&second).0);
    flow.settled_in(&api).await;
    let user = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.role == crate::model::MessageRole::User)
        .unwrap();
    let at_api = received(&flow, &api, &one).await;
    let at_docs = received(&flow, &docs, &two).await;
    assert_eq!(at_api.chain_id, user.id);
    assert_eq!(at_api.hop, 1);
    assert_eq!(
        at_docs.chain_id, user.id,
        "the peer's message started no chain"
    );
    assert_eq!(at_docs.hop, 2);
    // The sender's record says which input led to it.
    let sent = flow.board_of(&api).await.peer_messages[&two].clone();
    assert!(sent.outgoing);
    assert_eq!(sent.request_id.as_deref(), Some(one.as_str()));
    flow.settled_in(&docs).await;
    flow.stop().await;
}

/// Two sessions that answer each other forever: eight messages go, the ninth is refused in
/// the plan's words, and nothing more is sent. The user's next message starts a new chain,
/// whose first message goes again.
#[tokio::test]
async fn the_ninth_message_with_no_ones_word_is_refused_until_the_user_writes() {
    let peers = Peers::new();
    // Every message waits for the next turn, so each turn takes one and sends one.
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("peer-hops", options, peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let all = Sent::default();
    for (from, to) in [
        (flow.conversation.clone(), api.clone()),
        (api.clone(), flow.conversation.clone()),
    ] {
        let all = all.clone();
        peers.on(&from, move |turn| {
            let (all, to) = (all.clone(), to.clone());
            async move {
                let reply = send(&turn, &to, "What do you think?", None).await;
                all.lock().unwrap().push(reply);
                Reply::text("Asked.")
            }
        });
    }
    flow.say("Talk it over with the API session.").await;
    flow.until("the ninth refused", |_| {
        all.lock().unwrap().iter().any(|(_, failed)| *failed)
    })
    .await;
    flow.settled().await;
    flow.settled_in(&api).await;
    let sends = all.lock().unwrap().clone();
    assert_eq!(sends.len(), 9, "{sends:#?}");
    assert!(sends[..8].iter().all(|(_, failed)| !failed), "{sends:#?}");
    assert_eq!(
        sends[8],
        (
            "Eight messages have gone back and forth with no one's input. Ask your user before \
             sending more."
                .to_owned(),
            true
        )
    );
    let mut hops: Vec<u32> = flow
        .board()
        .await
        .peer_messages
        .values()
        .chain(flow.board_of(&api).await.peer_messages.values())
        .filter(|record| record.outgoing)
        .map(|record| record.hop)
        .collect();
    hops.sort_unstable();
    assert_eq!(hops, (1..=8).collect::<Vec<_>>());
    // The user's word: a new chain, at hop 1 again.
    flow.say("One more question for them.").await;
    flow.until("the new chain's first message", |_| {
        all.lock().unwrap().len() >= 10
    })
    .await;
    let (reply, failed) = all.lock().unwrap()[9].clone();
    assert!(!failed, "{reply}");
    let id = message_id(&reply);
    let record = flow.board().await.peer_messages[&id].clone();
    assert_eq!(record.hop, 1);
    let user = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .rfind(|message| message.role == crate::model::MessageRole::User)
        .unwrap();
    assert_eq!(record.chain_id, user.id);
    flow.stop().await;
}

/// One session sends another twenty messages in a minute; the twenty-first is refused.
#[tokio::test]
async fn the_twenty_first_message_in_a_minute_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-rate", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let all = Sent::default();
    let (kept, to) = (all.clone(), api.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, to) = (kept.clone(), to.clone());
        async move {
            for n in 1..=21 {
                let reply = send(&turn, &to, &format!("Note {n}."), None).await;
                kept.lock().unwrap().push(reply);
            }
            Reply::text("Sent them.")
        }
    });
    flow.say("Send the API session my notes.").await;
    flow.settled().await;
    let sends = all.lock().unwrap().clone();
    assert!(sends[..20].iter().all(|(_, failed)| !failed), "{sends:#?}");
    assert_eq!(
        sends[20],
        (
            "Too many messages to API work this minute.".to_owned(),
            true
        )
    );
    let board = flow.board().await;
    assert_eq!(board.peer_messages.len(), 20);
    assert!(
        board
            .peer_messages
            .values()
            .all(|record| record.state != PeerState::WaitingForOk)
    );
    flow.settled_in(&api).await;
    flow.stop().await;
}
