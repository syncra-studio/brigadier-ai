//! Messages never break running work: another session's message to a turn whose command runs
//! waits for the command's end (the CLI would read it only then, and a Stop meanwhile would
//! leave it to start a turn of its own, or close the CLI with it unread). It waits in the inbox,
//! recorded delivered, not seen; once the command ended it goes into the running turn and is
//! seen then. A Stop meanwhile leaves it to the next turn, which takes it once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::json;
use tokio::sync::Notify;

use super::peer_kit::{Peers, message_id, only};
use super::{Flow, Options, Reply};
use crate::board::Board;
use crate::model::PeerState;

fn stands(board: &Board, id: &str, state: &PeerState) -> bool {
    board
        .peer_messages
        .get(id)
        .is_some_and(|record| record.state == *state)
}

fn bash(status: ItemStatus) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: "bash-1".into(),
        name: "Bash".into(),
        input: Some(json!({ "command": "npm test" }).to_string()),
        status,
        output: (status != ItemStatus::InProgress).then(|| "[exit 0]\nok".into()),
        kept: None,
    }
}

const TEXT: &str = "Please add a `name` column.";

#[tokio::test]
async fn a_message_waits_for_the_running_command_then_goes_into_the_turn() {
    let peers = Peers::new();
    let flow = Flow::start("peer-boundary", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let (running, finish) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let early: Arc<Mutex<Option<String>>> = Arc::default();
    let steered: Arc<Mutex<Option<String>>> = Arc::default();
    let (ran, done, before, after) = (
        running.clone(),
        finish.clone(),
        early.clone(),
        steered.clone(),
    );
    peers.on(&api, move |turn| {
        let (ran, done, before, after) = (ran.clone(), done.clone(), before.clone(), after.clone());
        async move {
            turn.emit(bash(ItemStatus::InProgress)).await;
            ran.notify_one();
            done.notified().await;
            // Nothing came in while the command ran.
            *before.lock().unwrap() =
                tokio::time::timeout(Duration::from_millis(300), turn.steered())
                    .await
                    .ok()
                    .flatten();
            turn.emit(bash(ItemStatus::Completed)).await;
            let input = turn.steered().await;
            if let Some(input) = &input {
                let id = super::peer_kit::header_id(input).unwrap();
                let from = super::peer_kit::header_session(input).unwrap();
                let (_, failed) =
                    super::peer_kit::send(&turn, &from, "Added the `name` column.", Some(&id))
                        .await;
                assert!(!failed);
            }
            *after.lock().unwrap() = input;
            Reply::text("Tests pass; the column is added.")
        }
    });
    flow.say_to(&api, "Run the tests.").await;
    running.notified().await;
    let sent = peers.sends(&flow.conversation, &api, TEXT);
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
    finish.notify_one();
    flow.until_in(&api, "the message answered", |board| {
        stands(board, &id, &PeerState::Replied)
    })
    .await;
    flow.settled_in(&api).await;
    assert_eq!(
        *early.lock().unwrap(),
        None,
        "nothing steered behind the command"
    );
    let input = steered
        .lock()
        .unwrap()
        .clone()
        .expect("steered once the command ended");
    assert!(input.contains(&format!("· id {id}]\n{TEXT}")), "{input}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let inputs = peers.inputs_of(&api);
    assert_eq!(inputs.len(), 1, "no turn of its own: {inputs:?}");
    flow.stop().await;
}

#[tokio::test]
async fn a_stop_while_a_message_waits_leaves_it_to_the_next_turn_once() {
    let peers = Peers::new();
    let flow = Flow::start("peer-boundary-stop", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let running = Arc::new(Notify::new());
    let steers: Arc<Mutex<Vec<String>>> = Arc::default();
    let (ran, kept) = (running.clone(), steers.clone());
    peers.on(&api, move |turn| {
        let (ran, kept) = (ran.clone(), kept.clone());
        async move {
            if turn.input.contains("[message from session") {
                return Reply::text("Added the column.");
            }
            turn.emit(bash(ItemStatus::InProgress)).await;
            ran.notify_one();
            assert!(turn.stopped().await, "the user stops it");
            if let Ok(Some(steer)) =
                tokio::time::timeout(Duration::from_millis(300), turn.steered()).await
            {
                kept.lock().unwrap().push(steer);
            }
            Reply::text("")
        }
    });
    flow.say_to(&api, "Run the tests.").await;
    running.notified().await;
    let sent = peers.sends(&flow.conversation, &api, TEXT);
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let id = message_id(&only(&sent).0);
    flow.manager.interrupt(api.clone()).await.unwrap();
    flow.until_in(&api, "the message seen", |board| {
        stands(board, &id, &PeerState::Seen) || stands(board, &id, &PeerState::Replied)
    })
    .await;
    flow.settled_in(&api).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        steers.lock().unwrap().is_empty(),
        "nothing steered into the stopped turn"
    );
    let inputs = peers.inputs_of(&api);
    assert_eq!(inputs.len(), 2, "{inputs:?}");
    assert_eq!(peers.count_in(&api, &format!("· id {id}]")), 1);
    assert!(
        inputs[1].contains(&format!("· id {id}]\n{TEXT}")),
        "{}",
        inputs[1]
    );
    flow.stop().await;
}

/// The daemon dies while a message waits for the running command: the next launch delivers
/// it once.
#[tokio::test]
async fn a_message_waiting_for_a_command_at_a_crash_is_delivered_once() {
    let peers = Peers::new();
    let mut flow = Flow::start("peer-boundary-crash", Options::default(), peers.script()).await;
    let api = flow
        .add_session("API work", None, ProviderKind::Claude)
        .await;
    let running = Arc::new(Notify::new());
    let ran = running.clone();
    peers.on(&api, move |turn| {
        let ran = ran.clone();
        async move {
            if turn.input.contains("[message from session") {
                return Reply::text("Added the column.");
            }
            turn.emit(bash(ItemStatus::InProgress)).await;
            ran.notify_one();
            std::future::pending::<()>().await;
            Reply::text("")
        }
    });
    flow.say_to(&api, "Run the tests.").await;
    running.notified().await;
    let sent = peers.sends(&flow.conversation, &api, TEXT);
    flow.say("Ask the API session for a name column.").await;
    flow.settled().await;
    let id = message_id(&only(&sent).0);
    assert!(stands(
        &flow.board_of(&api).await,
        &id,
        &PeerState::Delivered
    ));
    flow.crash().await;
    flow.until_in(&api, "the message seen", |board| {
        stands(board, &id, &PeerState::Seen) || stands(board, &id, &PeerState::Replied)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(peers.count_in(&api, &format!("· id {id}]")), 1);
    flow.stop().await;
}
