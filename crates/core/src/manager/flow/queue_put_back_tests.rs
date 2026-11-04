//! A queued message the user pulls out to edit and sends again goes back to its slot (#673):
//! it is the user's row, as an edited or moved one is, so no judge steers it in meanwhile.
//! Every platform: the judge and the CLI are the flow's in-process fakes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::json;

use super::{FakeBehavior, Flow, Options, Reply, Script, eventually};
use crate::manager::SendOutcome;
use crate::work::QueuedMessage;

/// Time for a judge's answer to have acted on a row, had it been asked.
const ANSWER_ACTS: Duration = Duration::from_millis(300);

type Inputs = Arc<Mutex<Vec<String>>>;

fn bash(status: ItemStatus) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: "bash-1".into(),
        name: "Bash".into(),
        input: Some(json!({ "command": "cargo test --workspace" }).to_string()),
        status,
        output: (status != ItemStatus::InProgress).then(|| "ok".into()),
        kept: None,
    }
}

/// "Run the tests." runs a command until `gate`; every turn's input is kept.
fn working(inputs: Inputs, running: Arc<Mutex<bool>>, gate: Arc<tokio::sync::Notify>) -> Script {
    Arc::new(move |turn| {
        let (inputs, running, gate) = (inputs.clone(), running.clone(), gate.clone());
        Box::pin(async move {
            inputs.lock().unwrap().push(turn.input.clone());
            if !turn.input.contains("Run the tests.") {
                return Reply::text("Noted.");
            }
            turn.emit(bash(ItemStatus::InProgress)).await;
            *running.lock().unwrap() = true;
            gate.notified().await;
            turn.emit(bash(ItemStatus::Completed)).await;
            Reply::text("The tests pass.")
        })
    })
}

async fn queue_of(flow: &Flow) -> Vec<QueuedMessage> {
    flow.manager
        .core
        .board(&flow.conversation)
        .await
        .unwrap()
        .queue
        .items
}

async fn send(flow: &Flow, text: &str, queue_index: Option<u32>) -> QueuedMessage {
    let sent = flow
        .manager
        .send_message(
            flow.conversation.clone(),
            text.into(),
            Vec::new(),
            Vec::new(),
            false,
            queue_index,
        )
        .await
        .unwrap();
    let SendOutcome::Queued(item) = sent else {
        panic!("it queues while the turn works: {sent:?}");
    };
    item
}

/// Two rows wait (their judges said WAITS); the first is pulled out to edit and sent back to its
/// slot while a judge would now say FITS: it waits there, before the second, and goes on its own
/// turn after the block, with the user's new words.
#[tokio::test]
async fn a_row_put_back_from_an_edit_keeps_its_slot_unjudged() {
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("WAITS");
    let inputs = Inputs::default();
    let running = Arc::new(Mutex::new(false));
    let gate = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "queue-put-back",
        Options {
            thread: ProviderKind::Claude,
            behavior: behavior.clone(),
            ..Options::default()
        },
        working(inputs.clone(), running.clone(), gate.clone()),
    )
    .await;
    flow.say("Run the tests.").await;
    eventually("the command to run", || *running.lock().unwrap()).await;
    let first = send(&flow, "Check the formatting.", None).await;
    let second = send(&flow, "Then the docs.", None).await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    eventually("the judges to answer", || !conv.is_judging()).await;
    let queued: Vec<_> = queue_of(&flow)
        .await
        .into_iter()
        .map(|item| item.id)
        .collect();
    assert_eq!(queued, [first.id.clone(), second.id.clone()]);

    // "Edit message": the row leaves the queue for the composer, and comes back to slot 0.
    *behavior.judge.lock().unwrap() = Some("FITS");
    flow.manager
        .delete_queued(&flow.conversation, &first.id)
        .await
        .unwrap();
    let edited = send(&flow, "Check the formatting and the lints.", Some(0)).await;
    assert!(
        !edited.deciding,
        "no judge reads it, so it never reads Deciding: {edited:?}"
    );
    tokio::time::sleep(ANSWER_ACTS).await;
    let queued: Vec<_> = queue_of(&flow)
        .await
        .into_iter()
        .map(|item| (item.id, item.text))
        .collect();
    assert_eq!(
        queued,
        [
            (
                edited.id.clone(),
                "Check the formatting and the lints.".to_owned()
            ),
            (second.id.clone(), "Then the docs.".to_owned()),
        ],
        "back in its slot, not steered into the running block"
    );

    gate.notify_one();
    let board = flow
        .until("both to go on their own turns", |board| {
            board.queue.items.is_empty()
                && board.requests.contains_key(&edited.id)
                && board.requests.contains_key(&second.id)
        })
        .await;
    assert!(board.requests[&edited.id].steered_into.is_none());
    flow.settled().await;
    let all = inputs.lock().unwrap().clone();
    let at = |text: &str| all.iter().position(|input| input.contains(text));
    let (edited_at, second_at) = (at("the lints."), at("Then the docs."));
    assert!(edited_at.is_some_and(|edited| edited > 0), "{all:#?}");
    assert!(
        edited_at <= second_at,
        "the edit goes first, from its slot: {all:#?}"
    );
    flow.stop().await;
}

/// An ordinary follow-up (no slot) is still judged, as before: FITS steers it in.
#[tokio::test]
async fn a_follow_up_without_a_slot_is_still_judged() {
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("FITS");
    let inputs = Inputs::default();
    let running = Arc::new(Mutex::new(false));
    let gate = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "queue-put-back-judged",
        Options {
            thread: ProviderKind::Claude,
            behavior,
            ..Options::default()
        },
        working(inputs.clone(), running.clone(), gate.clone()),
    )
    .await;
    flow.say("Run the tests.").await;
    eventually("the command to run", || *running.lock().unwrap()).await;
    let item = send(&flow, "Check the formatting.", None).await;
    let board = flow
        .until("it to join the running block", |board| {
            board.queue.items.is_empty() && board.requests.contains_key(&item.id)
        })
        .await;
    assert!(board.requests[&item.id].steered_into.is_some());
    gate.notify_one();
    flow.settled().await;
    flow.stop().await;
}
