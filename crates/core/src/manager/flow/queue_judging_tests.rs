//! Deciding (D157, D163): a message the user sends while a session works waits in the rail,
//! Deciding, and a one-shot beside the session judges it at once, even while a long command
//! holds the running turn: FITS steers it into the open block, WAITS leaves it in the rail for
//! its own turn. The session's turn is never asked to judge. Same on every CLI.

use std::sync::Arc;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::{Value, json};

use super::cli_tests::Answers;
use super::{FakeBehavior, Flow, Options, Reply, Script};
use crate::model::ConversationId;
use crate::work::QueuedMessage;

const ASKED: &str = "Also check the formatting.";

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

/// The thread's "Run the tests." turn runs a command until `gate`; each turn's input is kept.
fn working(answers: Answers, gate: Arc<tokio::sync::Notify>) -> Script {
    Arc::new(move |turn| {
        let (answers, gate) = (answers.clone(), gate.clone());
        Box::pin(async move {
            answers
                .lock()
                .unwrap()
                .entry("inputs".into())
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .unwrap()
                .push(json!(turn.input));
            if !turn.input.contains("Run the tests.") {
                return Reply::text("Noted.");
            }
            turn.emit(bash(ItemStatus::InProgress)).await;
            answers
                .lock()
                .unwrap()
                .insert("running".into(), json!(true));
            gate.notified().await;
            turn.emit(bash(ItemStatus::Completed)).await;
            Reply::text("The tests pass.")
        })
    })
}

fn inputs(answers: &Answers) -> Vec<String> {
    let answers = answers.lock().unwrap();
    answers
        .get("inputs")
        .map_or_else(Vec::new, |inputs: &Value| {
            inputs
                .as_array()
                .unwrap()
                .iter()
                .map(|input| input.as_str().unwrap().to_owned())
                .collect()
        })
}

/// Starts a session on `kind` whose one-shot judge answers `judge`, runs the command, and sends
/// the follow-up while it runs: the item.
async fn send_while_working(
    name: &str,
    kind: ProviderKind,
    judge: &'static str,
) -> (Flow, Answers, Arc<tokio::sync::Notify>, QueuedMessage) {
    let answers = Answers::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some(judge);
    let flow = Flow::start(
        name,
        Options {
            thread: kind,
            behavior,
            ..Options::default()
        },
        working(answers.clone(), gate.clone()),
    )
    .await;
    flow.say("Run the tests.").await;
    super::eventually("the command to run", || {
        answers.lock().unwrap().contains_key("running")
    })
    .await;
    let sent = flow
        .manager
        .send_message(
            flow.conversation.clone(),
            ASKED.into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    let crate::manager::SendOutcome::Queued(item) = sent else {
        panic!("it queues while the turn works: {sent:?}");
    };
    assert!(item.deciding, "Deciding from the first frame: {item:?}");
    (flow, answers, gate, item)
}

fn joined(board: &super::Board, item: &QueuedMessage) -> bool {
    board.queue.items.is_empty() && board.requests.contains_key(&item.id)
}

/// FITS: decided while the command still holds the turn, it joins the running block, and the
/// session reads it once, inside that turn.
#[tokio::test]
async fn a_follow_up_that_fits_joins_the_running_block_at_once() {
    for kind in [ProviderKind::Claude, ProviderKind::Codex] {
        let (flow, answers, gate, item) =
            send_while_working(&format!("queue-fits-{kind:?}"), kind, "FITS").await;
        let board = flow
            .until("it to join while the command runs", |board| {
                joined(board, &item)
            })
            .await;
        assert!(board.requests[&item.id].steered_into.is_some());
        gate.notify_one();
        flow.settled().await;
        let all = inputs(&answers);
        assert_eq!(all.len(), 1, "never a turn of its own: {all:#?}");
        flow.stop().await;
    }
}

/// WAITS: decided while the command still holds the turn, it waits in the rail, then goes on
/// its own turn once the block ends.
#[tokio::test]
async fn a_follow_up_that_waits_goes_on_its_own_turn_after_the_block() {
    for kind in [ProviderKind::Claude, ProviderKind::Codex] {
        let (flow, answers, gate, item) =
            send_while_working(&format!("queue-waits-{kind:?}"), kind, "WAITS").await;
        flow.until("it to wait while the command runs", |board| {
            board
                .queue
                .items
                .iter()
                .any(|queued| queued.id == item.id && !queued.deciding)
        })
        .await;
        gate.notify_one();
        flow.until("it to go on its own turn", |board| joined(board, &item))
            .await;
        let board = flow.settled().await;
        assert!(board.requests[&item.id].steered_into.is_none());
        let all = inputs(&answers);
        assert_eq!(all.len(), 2, "{all:#?}");
        assert!(all[1].contains(ASKED) && !all[1].contains("[queued message"));
        flow.stop().await;
    }
}

/// A side chat keeps the plain queue: nothing judges it.
#[tokio::test]
async fn a_side_chat_keeps_the_plain_queue() {
    let answers = Answers::default();
    let gate = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        "queue-side-chat",
        Options::default(),
        working(answers.clone(), gate.clone()),
    )
    .await;
    let side: ConversationId = flow
        .manager
        .open_side_chat(&flow.conversation, None)
        .await
        .unwrap()
        .id;
    flow.say_to(&side, "Run the tests.").await;
    super::eventually("the command to run", || {
        answers.lock().unwrap().contains_key("running")
    })
    .await;
    flow.say_to(&side, ASKED).await;
    let board = flow
        .until_in(&side, "it to wait", |board| !board.queue.items.is_empty())
        .await;
    assert!(!board.queue.items[0].deciding, "{:?}", board.queue);
    gate.notify_one();
    flow.until_in(&side, "it to go on its own turn", |board| {
        board.queue.items.is_empty()
    })
    .await;
    flow.stop().await;
}
