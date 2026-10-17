//! The engine's loop rules for messages between sessions (SESSION-MESSAGING-PLAN.md §5.5):
//! every message carries the chain of the input that led to it, eight go back and forth with
//! no one's word before the ninth is refused, and one session sends another at most twenty a
//! minute.

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::peer_kit::{Peers, Sent, is_reply, message_id, only, send};
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
/// the plan's words, and nothing more is sent but the replies: the refused turn's answer to the
/// eighth, then each message's own reply as the requests waiting on it end, back to the first.
/// The user's next message starts a new chain, whose first message goes again.
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
                // A reply only informs: the turn it starts sends nothing.
                if is_reply(&turn.input) {
                    return Reply::text("Noted.");
                }
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
    // Each message is owed one reply, sent as the request waiting on the next one's reply
    // ends: they go back one by one, in turns of their own, so wait for the last on each side.
    for id in [&flow.conversation, &api] {
        flow.until_in(id, "every message replied and every reply taken", |board| {
            board
                .peer_messages
                .values()
                .all(|record| match record.reply_to {
                    None => record.state == PeerState::Replied,
                    Some(_) => record.outgoing || record.state == PeerState::Seen,
                })
        })
        .await;
    }
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
    let (board, at_api) = (flow.board().await, flow.board_of(&api).await);
    let sent: Vec<_> = board
        .peer_messages
        .values()
        .chain(at_api.peer_messages.values())
        .filter(|record| record.outgoing)
        .collect();
    let mut asked: Vec<&PeerRecord> = sent
        .iter()
        .copied()
        .filter(|record| record.reply_to.is_none())
        .collect();
    asked.sort_by_key(|record| record.hop);
    let hops: Vec<u32> = asked.iter().map(|record| record.hop).collect();
    assert_eq!(hops, (1..=8).collect::<Vec<_>>());
    // Each message gets one reply, Brigadier's, one hop on and exempt from the hop rule; a
    // reply starts no reply of its own. The turn whose send was refused still answers the
    // eighth message; each earlier one is answered by the turn its reply woke.
    let replies: Vec<&PeerRecord> = sent
        .iter()
        .copied()
        .filter(|record| record.reply_to.is_some())
        .collect();
    assert_eq!(replies.len(), 8, "{sent:#?}");
    for message in &asked {
        let reply = replies
            .iter()
            .find(|reply| reply.reply_to.as_deref() == Some(message.message_id.as_str()))
            .unwrap_or_else(|| panic!("no reply to hop {}: {sent:#?}", message.hop));
        assert!(reply.automatic, "{reply:#?}");
        assert_eq!(reply.hop, message.hop + 1, "{reply:#?}");
        let answer = if message.hop == 8 { "Asked." } else { "Noted." };
        assert_eq!(reply.text, answer, "{reply:#?}");
    }
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

/// A worker's report wakes a request a reply came back to: what that turn sends carries the
/// reply's chain on, one hop more, never counting from zero again.
#[tokio::test]
async fn a_turn_a_workers_report_wakes_keeps_the_chain_of_its_requests_newest_message() {
    use super::peer_kit::header_id;
    use serde_json::json;
    let peers = Peers::new();
    let inner = peers.script();
    let script: super::Script = Arc::new(move |turn: super::Turn| {
        if turn.is_orchestrator() {
            return inner(turn);
        }
        Box::pin(async move {
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "`name` is in the users table."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        })
    });
    // Each message waits for a turn of its own, so the reply wakes the thread anew.
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("peer-report-chain", options, script).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let back = flow.conversation.clone();
    peers.on(&api, move |turn| {
        let back = back.clone();
        async move {
            if turn.input.contains("Which column holds names?")
                && let Some(original) = header_id(&turn.input)
            {
                send(&turn, &back, "Ask a worker: I don't know.", Some(&original)).await;
            }
            Reply::text("Done.")
        }
    });
    let all = Sent::default();
    let (kept, to) = (all.clone(), api.clone());
    peers.on(&flow.conversation, move |turn| {
        let (kept, to) = (kept.clone(), to.clone());
        async move {
            if header_id(&turn.input).is_some() {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Find names", "kind": "research",
                               "spec": "Find where names live.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            let text = if turn.input.contains("Ask the API session") {
                "Which column holds names?"
            } else if kept.lock().unwrap().len() == 1 {
                "A worker found it: the users table."
            } else {
                return Reply::text("Done.");
            };
            let reply = send(&turn, &to, text, None).await;
            kept.lock().unwrap().push(reply);
            Reply::text("Told them.")
        }
    });
    flow.say("Ask the API session where names live.").await;
    flow.until("the report's turn's message", |_| {
        all.lock().unwrap().len() == 2
    })
    .await;
    let (reply, failed) = all.lock().unwrap()[1].clone();
    assert!(!failed, "{reply}");
    let board = flow.board().await;
    let record = &board.peer_messages[&message_id(&reply)];
    let user = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .find(|message| message.role == crate::model::MessageRole::User)
        .unwrap();
    assert_eq!(record.chain_id, user.id);
    // The question went at hop 1 and its reply came at 2.
    assert_eq!(record.hop, 3);
    flow.settled().await;
    flow.settled_in(&api).await;
    flow.stop().await;
}
