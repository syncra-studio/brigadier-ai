//! Under Ask for approval a message to another session waits for the user's Send on a card
//! (SESSION-MESSAGING-PLAN.md §5.6): Send delivers it exactly as written, Don't send declines
//! it, a restart keeps the card, and a target gone or archived meanwhile refuses it.

use std::time::Duration;

use brigadier_providers::{ApprovalDecision, ProviderKind};

use super::peer_kit::{Peers, Sent, message_id, only, send};
use super::{Flow, Options, Reply};
use crate::board::Board;
use crate::model::{ConversationId, PeerState, PermissionLevel};
use crate::work::{ApprovalSubject, CardId, CardState, RequestState};

fn ask() -> Options {
    Options {
        permission: PermissionLevel::AskForApproval,
        ..Options::default()
    }
}

/// The pending cards of sends waiting for the user's OK, oldest first.
fn send_cards(board: &Board) -> Vec<(CardId, String)> {
    let mut cards: Vec<_> = board
        .approvals
        .values()
        .filter(|approval| approval.state == CardState::Pending)
        .filter_map(|approval| match &approval.subject {
            ApprovalSubject::PeerMessage { message_id, .. } => {
                Some((approval.position, approval.id.clone(), message_id.clone()))
            }
            _ => None,
        })
        .collect();
    cards.sort_by_key(|(position, _, _)| *position);
    cards
        .into_iter()
        .map(|(_, card, message)| (card, message))
        .collect()
}

fn state_of(board: &Board, id: &str) -> PeerState {
    board.peer_messages[id].state.clone()
}

/// A thread whose turn sends one message: its card waits, the request waits on the user, and
/// the other session has nothing yet.
async fn sent_for_ok(flow: &Flow, peers: &Peers, to: &ConversationId) -> (CardId, String) {
    let sent = peers.sends(
        &flow.conversation,
        to,
        "The schema changed: `name` is required.",
    );
    flow.say("Tell the API session the schema changed.").await;
    // The card is written inside the call, before the tool's reply reaches the script.
    let board = flow
        .until("the send's card and the tool's reply", |board| {
            !send_cards(board).is_empty() && !sent.lock().unwrap().is_empty()
        })
        .await;
    let (reply, failed) = only(&sent);
    assert!(!failed, "{reply}");
    assert!(reply.contains("\"state\":\"waiting_for_ok\""), "{reply}");
    let (card, id) = send_cards(&board).remove(0);
    assert_eq!(message_id(&reply), id);
    assert_eq!(state_of(&board, &id), PeerState::WaitingForOk);
    let board = flow
        .until("the request waiting on the user", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Waiting)
        })
        .await;
    assert!(board.peer_messages[&id].sent_at_ms.is_none());
    assert!(flow.board_of(to).await.peer_messages.is_empty());
    assert!(peers.inputs_of(to).is_empty());
    (card, id)
}

/// What the sender's next turn hears about its send.
async fn next_note(flow: &Flow, peers: &Peers) -> String {
    peers.on(&flow.conversation, |_| async { Reply::text("Noted.") });
    let before = peers.inputs_of(&flow.conversation).len();
    flow.say("Did it go?").await;
    // A request still waiting for a reply makes this a follow-up the thread is asked about:
    // either way, a turn takes it.
    flow.until("the next turn", |_| {
        peers.inputs_of(&flow.conversation).len() > before
    })
    .await;
    flow.settled().await;
    peers.inputs_of(&flow.conversation)[before].clone()
}

/// Send: the stored text goes exactly as written, the model isn't asked again, and the
/// sender's next turn hears it went.
#[tokio::test]
async fn send_on_the_card_delivers_the_message_as_written() {
    let peers = Peers::new();
    let flow = Flow::start("peer-ask-send", ask(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let (card, id) = sent_for_ok(&flow, &peers, &api).await;
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    flow.until_in(&api, "the message seen", |board| {
        board
            .peer_messages
            .get(&id)
            .is_some_and(|record| record.state == PeerState::Seen)
    })
    .await;
    let input = peers.inputs_of(&api).remove(0);
    assert!(
        input.contains(&format!(
            "· id {id}]\nThe schema changed: `name` is required."
        )),
        "{input}"
    );
    let board = flow.board().await;
    assert!(board.peer_messages[&id].sent_at_ms.is_some());
    let note = next_note(&flow, &peers).await;
    assert!(note.contains("Sent to API work"), "{note}");
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// Don't send: the message is declined, the other session never hears of it, and the
/// sender's next turn is told.
#[tokio::test]
async fn dont_send_on_the_card_declines_the_message() {
    let peers = Peers::new();
    let flow = Flow::start("peer-ask-deny", ask(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (card, id) = sent_for_ok(&flow, &peers, &api).await;
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card,
            ApprovalDecision::Deny {
                message: String::new(),
            },
        )
        .await
        .unwrap();
    let board = flow.settled().await;
    assert_eq!(state_of(&board, &id), PeerState::Declined);
    let note = next_note(&flow, &peers).await;
    assert!(note.contains("The user didn't send it."), "{note}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(flow.board_of(&api).await.peer_messages.is_empty());
    assert!(peers.inputs_of(&api).is_empty());
    flow.stop().await;
}

/// A restart while the card waits: the card comes back, nothing was delivered, and its Send
/// delivers the message then.
#[tokio::test]
async fn a_card_waiting_across_a_restart_still_sends() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-ask-restart", ask(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (card, id) = sent_for_ok(&flow, &peers, &api).await;
    flow.restart().await;
    let board = flow.board().await;
    assert_eq!(send_cards(&board), vec![(card.clone(), id.clone())]);
    assert_eq!(state_of(&board, &id), PeerState::WaitingForOk);
    assert!(peers.inputs_of(&api).is_empty());
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    flow.until_in(&api, "the message seen", |board| {
        board
            .peer_messages
            .get(&id)
            .is_some_and(|record| record.state == PeerState::Seen)
    })
    .await;
    assert_eq!(peers.count_in(&api, &format!("· id {id}]")), 1);
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// A target archived, or deleted, before the user's OK: Send refuses it with the reason, and
/// nothing reaches anyone.
#[tokio::test]
async fn send_to_a_target_archived_or_deleted_meanwhile_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-ask-gone", ask(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (card, id) = sent_for_ok(&flow, &peers, &api).await;
    flow.manager.archive(api.clone()).await.unwrap();
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    let board = flow.settled().await;
    assert_eq!(
        state_of(&board, &id),
        PeerState::Refused {
            reason: "API work is archived. Nothing was sent.".into()
        }
    );
    assert!(flow.board_of(&api).await.peer_messages.is_empty());
    let note = next_note(&flow, &peers).await;
    assert!(
        note.contains("It wasn't sent: API work is archived."),
        "{note}"
    );

    let docs = flow.add_session("Docs", None, ProviderKind::Codex).await;
    let sent = peers.sends(&flow.conversation, &docs, "Update the docs.");
    flow.say("Tell the docs session to update.").await;
    let board = flow
        .until("the second card and the tool's reply", |board| {
            !send_cards(board).is_empty() && !sent.lock().unwrap().is_empty()
        })
        .await;
    let (card, second) = send_cards(&board).remove(0);
    assert_eq!(message_id(&only(&sent).0), second);
    flow.manager.delete(docs.clone()).await.unwrap();
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    let board = flow.settled().await;
    assert_eq!(
        state_of(&board, &second),
        PeerState::Refused {
            reason: "The session was deleted. Nothing was sent.".into()
        }
    );
    assert!(peers.inputs_of(&docs).is_empty() && peers.inputs_of(&api).is_empty());
    flow.stop().await;
}

/// Twenty-one cards approved in a burst: the rate counts when each was allowed to go, not
/// when it was written, so the twenty-first is refused.
#[tokio::test]
async fn the_twenty_first_approval_in_a_minute_is_refused() {
    let peers = Peers::new();
    let flow = Flow::start("peer-ask-rate", ask(), peers.script()).await;
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
            Reply::text("Asked to send them.")
        }
    });
    flow.say("Send the API session my notes.").await;
    let board = flow
        .until("21 cards and their replies", |board| {
            send_cards(board).len() == 21 && all.lock().unwrap().len() == 21
        })
        .await;
    assert!(
        all.lock().unwrap().iter().all(|(_, failed)| !failed),
        "the cards are written whatever the rate"
    );
    let cards = send_cards(&board);
    for (card, _) in &cards {
        flow.manager
            .answer_card(
                flow.conversation.clone(),
                card.clone(),
                ApprovalDecision::Allow,
            )
            .await
            .unwrap();
    }
    let board = flow.settled().await;
    let states: Vec<PeerState> = cards.iter().map(|(_, id)| state_of(&board, id)).collect();
    assert!(
        states[..20]
            .iter()
            .all(|state| state.due() || *state == PeerState::Seen),
        "{states:#?}"
    );
    assert_eq!(
        states[20],
        PeerState::Refused {
            reason: "Too many messages to API work this minute.".into()
        }
    );
    flow.settled_in(&api).await;
    flow.stop().await;
}
