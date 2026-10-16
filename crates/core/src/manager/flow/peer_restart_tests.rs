//! A message between sessions survives the daemon dying at any point of its delivery and
//! reaches its receiver exactly once (SESSION-MESSAGING-PLAN.md §4): recorded but not yet in
//! the inbox, in the inbox but not taken, or taken by the CLI with `seen` not yet recorded,
//! which the CLI's transcript shows.

use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::json;
use tokio::sync::Notify;

use super::peer_kit::{Peers, Sent, is_reply, message_id, only, send, wait};
use super::{FakeBehavior, Flow, Options, Reply};
use crate::board::Board;
use crate::manager::peers::PeerFault;
use crate::model::{ConversationId, PeerState};
use crate::work::OrchestratorStepKind;

fn stands(board: &Board, id: &str, state: &PeerState) -> bool {
    board
        .peer_messages
        .get(id)
        .is_some_and(|record| record.state == *state)
}

/// The id of the one message `board`'s session sent.
fn sent_id(board: &Board) -> String {
    let sent: Vec<_> = board
        .peer_messages
        .values()
        .filter(|record| record.outgoing)
        .collect();
    assert_eq!(sent.len(), 1, "{sent:#?}");
    sent[0].message_id.clone()
}

/// Whether message `id` was taken on `board`: seen, or already answered.
fn taken(board: &Board, id: &str) -> bool {
    stands(board, id, &PeerState::Seen) || stands(board, id, &PeerState::Replied)
}

/// Waits until message `id` is taken on both sides, then that `to`'s turns took it once.
async fn delivered_once(flow: &Flow, peers: &Peers, to: &ConversationId, id: &str) {
    flow.until_in(to, "the message seen", |board| taken(board, id))
        .await;
    flow.until("the sender's record seen", |board| taken(board, id))
        .await;
    // Anything still to come would have come by now.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(peers.count_in(to, &format!("· id {id}]")), 1);
}

/// The daemon dies once the send is recorded, before it reached the inbox: the next launch
/// delivers it.
#[tokio::test]
async fn a_message_recorded_before_a_crash_is_delivered_once_at_the_next_launch() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-crash-1", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    *flow.manager.peer_fault.lock().unwrap() = Some(PeerFault::BeforeEnqueue);
    let sent = peers.sends(&flow.conversation, &api, "Please add a `name` column.");
    flow.say("Ask the API session for a name column.").await;
    while sent.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(only(&sent).1, "the send died with the daemon");
    assert!(peers.inputs_of(&api).is_empty());
    flow.crash().await;
    let id = sent_id(&flow.board().await);
    delivered_once(&flow, &peers, &api, &id).await;
    flow.stop().await;
}

/// The daemon dies with the message in the receiver's inbox, its CLI not having taken it,
/// though a tool's output in that session showed its header: the transcript's inputs alone
/// count, so the next launch delivers it.
#[tokio::test]
async fn a_message_in_the_inbox_at_a_crash_is_delivered_once_even_if_a_tool_showed_it() {
    let peers = Peers::new();
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let mut flow = Flow::start("peer-crash-2", options, peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let sent = peers.sends(&flow.conversation, &api, "Please add a `name` column.");
    // The receiver's turn runs on until the message waits in its inbox, then reads its own
    // session: a tool's output now carries the message's header.
    let (looked, after) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let (gate, done, seen, own) = (looked.clone(), after.clone(), sent.clone(), api.clone());
    peers.on(&api, move |turn| {
        let (gate, done, seen, own) = (gate.clone(), done.clone(), seen.clone(), own.clone());
        async move {
            if turn.input.contains("[message from session") {
                return Reply::text("Added the column.");
            }
            gate.notified().await;
            let id = message_id(&only(&seen).0);
            let read = turn.call("read_session", json!({ "id": own.0 })).await;
            assert!(read.text.contains(&id), "{}", read.text);
            turn.tool_output(&format!(
                "[message from session \"Flow\" in Flow · id {id}]\nPlease add a `name` column."
            ));
            done.notify_one();
            Reply::text("Refactored.")
        }
    });
    flow.say_to(&api, "Refactor the models.").await;
    while peers.inputs_of(&api).is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let (reply, failed) = only(&sent);
    assert!(!failed && reply.contains("\"state\":\"queued\""), "{reply}");
    let id = message_id(&reply);
    looked.notify_one();
    after.notified().await;
    assert!(stands(
        &flow.board_of(&api).await,
        &id,
        &PeerState::Delivered
    ));
    flow.crash().await;
    delivered_once(&flow, &peers, &api, &id).await;
    flow.stop().await;
}

/// The daemon dies after the receiver's CLI took the message, before `seen` was recorded:
/// the next launch finds it in the CLI's transcript and records it seen without sending it
/// again.
#[tokio::test]
async fn a_message_the_cli_took_before_a_crash_is_found_and_not_sent_again() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-crash-3", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    *flow.manager.peer_fault.lock().unwrap() = Some(PeerFault::AfterTaken);
    let sent = peers.sends(&flow.conversation, &api, "Please add a `name` column.");
    flow.say("Ask the API session for a name column.").await;
    // The receiver can take it before the sender's call returns.
    while peers.inputs_of(&api).is_empty()
        || flow.manager.peer_fault.lock().unwrap().is_some()
        || sent.lock().unwrap().is_empty()
    {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let id = message_id(&only(&sent).0);
    // The old daemon's last word: taken, not yet seen.
    assert!(
        flow.board_of(&api)
            .await
            .peer_messages
            .get(&id)
            .is_some_and(|record| record.state.due()),
        "seen wasn't recorded before the crash"
    );
    flow.crash().await;
    delivered_once(&flow, &peers, &api, &id).await;
    flow.stop().await;
}

/// A message waiting in a busy session's inbox when the daemon quits goes at the next
/// launch, and what its turn sends on without `reply_to` stays in its chain: the chain is
/// read from the stored records, not from memory.
#[tokio::test]
async fn a_message_sent_on_after_a_restart_stays_in_its_chain() {
    let peers = Peers::new();
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            refuse_steers: true,
            ..Default::default()
        }),
        ..Options::default()
    };
    let mut flow = Flow::start("peer-chain-restart", options, peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let first = peers.sends(&flow.conversation, &api, "Is `name` required now?");
    let hold = Arc::new(Notify::new());
    let answered = Sent::default();
    let (held, kept, back) = (hold.clone(), answered.clone(), flow.conversation.clone());
    peers.on(&api, move |turn| {
        let (held, kept, back) = (held.clone(), kept.clone(), back.clone());
        async move {
            if !turn.input.contains("[message from session") {
                held.notified().await;
                return Reply::text("Refactored.");
            }
            // An answer, but not sent as a reply.
            let reply = send(&turn, &back, "Yes, `name` is required.", None).await;
            kept.lock().unwrap().push(reply);
            Reply::text("Answered.")
        }
    });
    flow.say_to(&api, "Refactor the models.").await;
    while peers.inputs_of(&api).is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    flow.say("Ask the API session whether `name` is required.")
        .await;
    flow.settled().await;
    let question = message_id(&only(&first).0);
    flow.restart().await;
    hold.notify_one();
    flow.until_in(&api, "the answer sent", |_| {
        answered.lock().unwrap().len() == 1
    })
    .await;
    let (reply, failed) = only(&answered);
    assert!(!failed, "{reply}");
    let answer = message_id(&reply);
    let board = flow
        .until("the answer received", |board| {
            board.peer_messages.contains_key(&answer)
        })
        .await;
    let asked = &board.peer_messages[&question];
    let got = &board.peer_messages[&answer];
    assert!(!got.outgoing && got.reply_to.is_none());
    assert_eq!(got.chain_id, asked.chain_id);
    assert_eq!(got.hop, 2);
    flow.settled_in(&api).await;
    flow.stop().await;
}

/// Whether a call of `board`'s thread still shows as running.
fn runs_a_call(board: &Board) -> bool {
    board.orchestrator_steps.iter().any(|step| {
        matches!(
            step.kind,
            OrchestratorStepKind::Tool {
                status: ItemStatus::InProgress,
                ..
            }
        )
    })
}

/// The daemon dies while the receiver's turn works on a message and the sender's waits for
/// its reply: nothing resumes the receiver's turn, so the reply says the restart cut it off,
/// and the sender can ask again. Neither side's calls show as running any more.
#[tokio::test]
async fn a_receiver_a_crash_cut_off_replies_that_it_was_cut_off() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-crash-cut-off", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Codex)
        .await;
    let (asked, to) = (Sent::default(), api.clone());
    let kept = asked.clone();
    peers.on(&flow.conversation, move |turn| {
        let (kept, to) = (kept.clone(), to.clone());
        async move {
            if is_reply(&turn.input) {
                return Reply::text("Noted.");
            }
            let sent = send(&turn, &to, "Please add a `name` column.", None).await;
            kept.lock().unwrap().push(sent);
            wait(&turn, json!({})).await;
            Reply::text("Added.")
        }
    });
    // The receiver's turn, in a command, never ends before the crash.
    let hold = Arc::new(Notify::new());
    let held = hold.clone();
    peers.on(&api, move |turn| {
        let held = held.clone();
        async move {
            turn.emit(ProviderEvent::Command {
                item_id: "cmd-1".into(),
                command: "sleep 40".into(),
                cwd: None,
                status: ItemStatus::InProgress,
                exit_code: None,
                output: None,
                duration_ms: None,
                kept: None,
            })
            .await;
            held.notified().await;
            Reply::text("Added.")
        }
    });
    flow.say("Ask the API session for a name column.").await;
    while asked.lock().unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let id = message_id(&only(&asked).0);
    flow.until_in(&api, "the command running", |board| {
        stands(board, &id, &PeerState::Seen) && runs_a_call(board)
    })
    .await;
    flow.until("the wait running", runs_a_call).await;
    flow.crash().await;
    flow.until("the reply received", |board| {
        board
            .peer_messages
            .values()
            .any(|record| !record.outgoing && record.reply_to.as_deref() == Some(id.as_str()))
    })
    .await;
    let board = flow.settled().await;
    let reply = "[\"API work\" was cut off by a restart before it answered]";
    assert_eq!(peers.count_in(&flow.conversation, reply), 1);
    assert_eq!(
        peers.count_in(&flow.conversation, "finished without an answer"),
        0
    );
    assert!(!runs_a_call(&board), "{:#?}", board.orchestrator_steps);
    let receiver = flow.settled_in(&api).await;
    assert!(
        !runs_a_call(&receiver),
        "{:#?}",
        receiver.orchestrator_steps
    );
    hold.notify_waiters();
    flow.stop().await;
}
