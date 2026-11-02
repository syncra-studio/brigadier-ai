//! Goal 8 (D24, D25, D157): while a work block is open (its workers still work) a follow-up
//! is judged at once by a one-shot beside the session: one that waits stays in the queue until
//! the block closes, then goes on its own turn; one that fits joins the open block. One the user
//! steers in is taken by the open block, framed so the session knows.

use std::sync::Arc;

use serde_json::json;

use super::cli_tests::Answers;
use super::{FakeBehavior, Flow, Options, Reply, Script};

const FIRST: &str = "Unrelated: write the release notes.";

fn inputs(answers: &Answers) -> Vec<String> {
    answers.lock().unwrap()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|input| input.as_str().unwrap().to_owned())
        .collect()
}

/// A session whose first turn starts a worker that runs until `release`; the one-shot judging
/// a follow-up answers `judge`.
async fn block_judged(
    name: &str,
    judge: &'static str,
) -> (Flow, Answers, Arc<tokio::sync::Notify>) {
    let answers = Answers::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let script: Script = {
        let (answers, release) = (answers.clone(), release.clone());
        Arc::new(move |turn| {
            let (answers, release) = (answers.clone(), release.clone());
            Box::pin(async move {
                if turn.worker().is_some() {
                    answers
                        .lock()
                        .unwrap()
                        .insert("worker running".into(), json!(true));
                    release.notified().await;
                    return Reply::text("Fixed.");
                }
                answers
                    .lock()
                    .unwrap()
                    .entry("inputs".into())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .push(json!(turn.input));
                if turn.input.contains("Fix it with a worker.") {
                    turn.start_worker("Fix", "Fix the bug.").await;
                    super::eventually("the worker to run", || {
                        answers.lock().unwrap().contains_key("worker running")
                    })
                    .await;
                    return Reply::text("A worker is on it.");
                }
                Reply::text("Noted.")
            })
        })
    };
    let behavior = Arc::new(FakeBehavior {
        session_file: true,
        ..FakeBehavior::default()
    });
    *behavior.judge.lock().unwrap() = Some(judge);
    let flow = Flow::start(
        name,
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    flow.say("Fix it with a worker.").await;
    flow.settled().await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the thread's turn to end", || conv.idle()).await;
    assert!(
        flow.manager.work_holds(&conv).await,
        "its worker holds the work"
    );
    (flow, answers, release)
}

async fn block(name: &str) -> (Flow, Answers, Arc<tokio::sync::Notify>) {
    block_judged(name, "WAITS").await
}

/// Waits until `text` waits in the queue, judged (no longer Deciding).
async fn waits(flow: &Flow, text: &str) -> crate::work::QueuedMessage {
    let board = flow
        .until("it to wait, judged", |board| {
            board
                .queue
                .items
                .iter()
                .any(|item| item.text == text && !item.deciding)
        })
        .await;
    board
        .queue
        .items
        .into_iter()
        .find(|item| item.text == text)
        .unwrap()
}

/// WAITS while the block is open: nothing sends it while the worker works (D157); once the
/// block closes it goes on its own turn, once.
#[tokio::test]
async fn a_waiting_message_waits_until_the_block_ends() {
    let (flow, answers, release) = block("block-wait").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    flow.say(FIRST).await;
    let first = waits(&flow, FIRST).await;
    flow.idle().await;
    assert!(flow.manager.work_holds(&conv).await);
    assert_eq!(flow.board().await.queue.items.len(), 1);
    assert!(!inputs(&answers).iter().any(|input| input.contains(FIRST)));
    release.notify_one();
    let board = flow
        .until("it to leave the queue", |board| {
            board.queue.items.is_empty() && board.requests.contains_key(&first.id)
        })
        .await;
    assert!(board.requests[&first.id].steered_into.is_none());
    flow.settled().await;
    let all = inputs(&answers);
    assert_eq!(
        all.iter().filter(|input| input.contains(FIRST)).count(),
        1,
        "{all:#?}"
    );
    flow.stop().await;
}

/// FITS while only the block's workers work: it joins the open block at once, framed, on a
/// turn that goes on with that work, and never draws a block of its own (D157).
#[tokio::test]
async fn a_message_that_fits_joins_the_open_block() {
    let (flow, answers, release) = block_judged("block-fits", "FITS").await;
    let open = flow.board().await.latest_request().unwrap().id.clone();
    flow.say(FIRST).await;
    let board = flow
        .until("it to join", |board| {
            board.queue.items.is_empty()
                && board
                    .requests
                    .values()
                    .any(|request| request.steered_into.as_deref() == Some(open.as_str()))
        })
        .await;
    let joined = board
        .requests
        .values()
        .find(|request| request.steered_into.as_deref() == Some(open.as_str()))
        .unwrap()
        .id
        .clone();
    flow.idle().await;
    let marks = super::tab_chat_tests::steer_marks(&flow, &joined).await;
    assert!(
        marks
            .iter()
            .all(|into| into.as_deref() == Some(open.as_str())),
        "every write of its request names the open block's: {marks:?}"
    );
    let all = inputs(&answers);
    assert_eq!(
        all.last().unwrap(),
        &format!(
            "{}{FIRST}",
            super::super::conversation::queue_judging::STEERED
        ),
        "{all:#?}"
    );
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(view.turns.last().unwrap().joins, "{view:#?}");
    release.notify_one();
    flow.stop().await;
}

/// The user's Steer on a message that waits while the block is open: the block takes it on a
/// turn that goes on with that work, under the line saying the user steered it in.
#[tokio::test]
async fn a_waiting_message_steered_in_is_taken_by_the_open_block() {
    let (flow, answers, release) = block("block-steer-waiting").await;
    flow.say(FIRST).await;
    let first = waits(&flow, FIRST).await;
    flow.idle().await;
    flow.manager
        .steer_queued(flow.conversation.clone(), first.id.clone())
        .await
        .unwrap();
    flow.until("it to join", |board| {
        board.queue.items.is_empty() && board.requests.contains_key(&first.id)
    })
    .await;
    flow.idle().await;
    let all = inputs(&answers);
    assert_eq!(
        all.last().unwrap(),
        &format!(
            "{}{FIRST}",
            super::super::conversation::queue_judging::STEERED
        ),
        "{all:#?}"
    );
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let joining = view
        .turns
        .iter()
        .find(|turn| turn.requests.contains(&first.id))
        .unwrap();
    assert!(joining.joins, "it goes on in the open block: {view:#?}");
    release.notify_one();
    flow.stop().await;
}

/// A message the user steers in fresh while only the block's workers work: the block takes it
/// at once, framed, on a turn that goes on with that work; nothing is queued. Its request is
/// steered into the open block's in the very write that stores it, so no frame ever draws it
/// as a block of its own (D65, D113).
#[tokio::test]
async fn a_fresh_steer_while_the_block_is_open_is_taken_framed() {
    let (flow, answers, release) = block("block-steer-fresh").await;
    let open = flow.board().await.latest_request().unwrap().id.clone();
    let sent = flow
        .manager
        .send_message(
            flow.conversation.clone(),
            FIRST.into(),
            Vec::new(),
            Vec::new(),
            true,
            None,
        )
        .await
        .unwrap();
    let crate::manager::SendOutcome::Sent(message) = sent else {
        panic!("a steer is sent, never queued: {sent:?}");
    };
    flow.idle().await;
    let marks = super::tab_chat_tests::steer_marks(&flow, &message.id).await;
    assert!(!marks.is_empty());
    assert!(
        marks
            .iter()
            .all(|into| into.as_deref() == Some(open.as_str())),
        "every write of its request names the open block's: {marks:?}"
    );
    assert!(flow.board().await.queue.items.is_empty());
    let all = inputs(&answers);
    let steered = all.last().unwrap();
    assert_eq!(
        steered,
        &format!(
            "{}{FIRST}",
            super::super::conversation::queue_judging::STEERED
        ),
        "{all:#?}"
    );
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(view.turns.last().unwrap().joins, "{view:#?}");
    release.notify_one();
    flow.stop().await;
}
