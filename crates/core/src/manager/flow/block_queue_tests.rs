//! Goal 8 (D24, D25, D157): while a work block is open (its workers still work) a follow-up
//! is judged at once by a one-shot beside the session: one that waits stays in the queue until
//! the block closes, then goes on its own turn; one that fits joins the open block. One the user
//! steers in is taken by the open block, framed so the session knows. A block nothing really runs
//! in (its workers paused or stopped, its turn stopped) judges nothing: a follow-up goes right in,
//! with no row in the rail.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::CliRequest;
use brigadier_providers::{ItemStatus, ProviderEvent};
use serde_json::json;
use tokio::sync::mpsc;

use super::chat_thread_tests::new_chat;
use super::cli_tests::{Answers, brigadier, start};
use super::{FakeBehavior, Flow, Options, Reply, Script};
use crate::manager::SendOutcome;
use crate::model::ConversationId;

const FIRST: &str = "Unrelated: write the release notes.";

fn inputs(answers: &Answers) -> Vec<String> {
    answers.lock().unwrap()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|input| input.as_str().unwrap().to_owned())
        .collect()
}

/// A script whose "Fix it with a worker." starts a worker that runs until the Notify it returns.
fn worker_script() -> (Script, Answers, Arc<tokio::sync::Notify>) {
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
                let chat = turn.input.contains("Fix it from the Chat.");
                if chat || turn.input.contains("Fix it with a worker.") {
                    if chat {
                        let request = CliRequest::Start(Box::new(start("Fix the bug.")));
                        brigadier(&turn, request).await;
                    } else {
                        turn.start_worker("Fix", "Fix the bug.").await;
                    }
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
    (script, answers, release)
}

/// A session whose first turn starts a worker that runs until `release`; the one-shot judging
/// a follow-up answers `judge`.
async fn block_judged(
    name: &str,
    judge: &'static str,
) -> (Flow, Answers, Arc<tokio::sync::Notify>) {
    let (script, answers, release) = worker_script();
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

/// Waits until `text` waits in the queue, judged.
async fn waits(flow: &Flow, text: &str) -> crate::work::QueuedMessage {
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let board = flow
        .until("it to wait, judged", |board| {
            !conv.is_judging() && board.queue.items.iter().any(|item| item.text == text)
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

/// WAITS, then the block's only worker is paused: a paused worker keeps the block open (D159)
/// but never holds the queue, so the message goes on its own turn, not after a block end that
/// may never come.
#[tokio::test]
async fn a_waiting_message_goes_once_only_paused_workers_hold_the_block() {
    let (flow, answers, release) = block("block-paused").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    flow.say(FIRST).await;
    let first = waits(&flow, FIRST).await;
    let task = flow
        .manager
        .core
        .tasks(&flow.conversation)
        .await
        .unwrap()
        .remove(0);
    assert!(flow.manager.pause_task(task.id).await.unwrap());
    flow.until("it to leave the queue", |board| {
        board.queue.items.is_empty() && board.requests.contains_key(&first.id)
    })
    .await;
    flow.settled().await;
    assert!(
        flow.manager.work_holds(&conv).await,
        "the paused worker keeps the block open"
    );
    let all = inputs(&answers);
    assert_eq!(
        all.iter().filter(|input| input.contains(FIRST)).count(),
        1,
        "{all:#?}"
    );
    release.notify_one();
    flow.stop().await;
}

/// FITS while only the block's workers work: it joins the open block at once, framed, on a
/// turn that goes on with that work, and never draws a block of its own (D157).
// Uses the debug-only fault injections, as tab_chat_tests does.
#[cfg(debug_assertions)]
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
// Uses the debug-only fault injections, as tab_chat_tests does.
#[cfg(debug_assertions)]
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

/// The user's follow-up to conversation `id`, as the composer sends it.
async fn follow_up(flow: &Flow, id: &ConversationId, text: &str) -> SendOutcome {
    flow.manager
        .send_message(id.clone(), text.into(), Vec::new(), Vec::new(), false, None)
        .await
        .unwrap()
}

/// `outcome` went right in: nothing queued, so nothing reads Deciding, and no judge is out.
async fn went_right_in(
    flow: &Flow,
    id: &ConversationId,
    outcome: SendOutcome,
) -> crate::model::Message {
    let SendOutcome::Sent(message) = outcome else {
        panic!("it goes right in, with no row in the rail: {outcome:?}");
    };
    assert!(
        !flow
            .board_of(id)
            .await
            .queue
            .items
            .iter()
            .any(|item| item.deciding),
        "no row reads Deciding"
    );
    assert!(
        !flow.manager.conv(id).unwrap().is_judging(),
        "no judge was asked"
    );
    message
}

/// Pauses the only worker of `id`, once its thread rests with it holding the block alone.
async fn only_paused(flow: &Flow, id: &ConversationId) {
    let conv = flow.manager.conv(id).unwrap();
    let task = flow.manager.core.tasks(id).await.unwrap().remove(0);
    assert!(flow.manager.pause_task(task.id).await.unwrap());
    super::eventually_async("the thread to rest", || conv.idle()).await;
    assert!(
        flow.manager.work_holds(&conv).await,
        "it keeps the block open"
    );
    assert!(
        !flow.manager.queue_holds(&conv).await,
        "it holds no message"
    );
}

/// Only a paused worker holds the block: a follow-up starts a turn at once with no row and no
/// judge, unframed, and goes on in the open block from its first write (D159).
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_follow_up_goes_right_in_when_only_paused_workers_hold() {
    let (flow, answers, release) = block("block-paused-right-in").await;
    let id = flow.conversation.clone();
    let open = flow.board().await.latest_request().unwrap().id.clone();
    only_paused(&flow, &id).await;
    let sent = follow_up(&flow, &id, FIRST).await;
    let message = went_right_in(&flow, &id, sent).await;
    flow.idle().await;
    let marks = super::tab_chat_tests::steer_marks(&flow, &message.id).await;
    assert!(
        !marks.is_empty()
            && marks
                .iter()
                .all(|into| into.as_deref() == Some(open.as_str())),
        "every write of its request names the open block's: {marks:?}"
    );
    let all = inputs(&answers);
    assert_eq!(all.last().unwrap(), FIRST, "{all:#?}");
    let view = flow.manager.thread_view(&id).unwrap();
    assert!(view.turns.last().unwrap().joins, "{view:#?}");
    release.notify_one();
    flow.stop().await;
}

/// The same in a Chat: it judges its queue as a session does, so it skips the judge as one does.
#[tokio::test]
async fn a_chats_follow_up_goes_right_in_when_only_paused_workers_hold() {
    let (script, answers, release) = worker_script();
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("WAITS");
    let flow = Flow::start(
        "block-chat-paused-right-in",
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Fix it from the Chat.").await;
    let conv = flow.manager.conv(&chat).unwrap();
    super::eventually("the worker to run", || {
        answers.lock().unwrap().contains_key("worker running")
    })
    .await;
    super::eventually_async("the thread to rest", || conv.idle()).await;
    only_paused(&flow, &chat).await;
    let sent = follow_up(&flow, &chat, FIRST).await;
    went_right_in(&flow, &chat, sent).await;
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input == FIRST)
    })
    .await;
    release.notify_one();
    flow.stop().await;
}

/// The composer's Stop stops the turn and its workers: nothing holds, so a follow-up starts a
/// turn at once, a block of its own.
#[tokio::test]
async fn a_follow_up_goes_right_in_after_the_block_is_stopped() {
    let (flow, answers, release) = block("block-stopped-right-in").await;
    let id = flow.conversation.clone();
    let conv = flow.manager.conv(&id).unwrap();
    let task = flow.manager.core.tasks(&id).await.unwrap().remove(0);
    flow.manager.interrupt(id.clone()).await.unwrap();
    flow.manager.stop_task_by_user(task.id).await.unwrap();
    super::eventually_async("the thread to rest", || conv.idle()).await;
    assert!(!flow.manager.work_holds(&conv).await, "nothing holds");
    let sent = follow_up(&flow, &id, FIRST).await;
    let message = went_right_in(&flow, &id, sent).await;
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input.contains(FIRST))
    })
    .await;
    flow.idle().await;
    assert!(
        flow.board().await.requests[&message.id]
            .steered_into
            .is_none(),
        "the stopped block is over"
    );
    release.notify_one();
    flow.stop().await;
}

/// "Run on." runs until the Notify it returns, whatever a Stop says, as a CLI slow to take one.
fn slow_stop_script() -> (Script, Answers, Arc<tokio::sync::Notify>) {
    let answers = Answers::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let script: Script = {
        let (answers, release) = (answers.clone(), release.clone());
        Arc::new(move |turn| {
            let (answers, release) = (answers.clone(), release.clone());
            Box::pin(async move {
                answers
                    .lock()
                    .unwrap()
                    .entry("inputs".into())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .push(json!(turn.input));
                if turn.input.contains("Run on.") {
                    answers
                        .lock()
                        .unwrap()
                        .insert("running".into(), json!(true));
                    release.notified().await;
                }
                Reply::text("Noted.")
            })
        })
    };
    (script, answers, release)
}

/// A session or Chat in the turn of [`slow_stop_script`]; the judge answers FITS once it takes a
/// permit of the semaphore it returns, none given yet.
async fn running_slow(
    name: &str,
    chat: bool,
) -> (
    Flow,
    ConversationId,
    Answers,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Semaphore>,
) {
    let (script, answers, release) = slow_stop_script();
    let judge = Arc::new(tokio::sync::Semaphore::new(0));
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("FITS");
    *behavior.judge_gate.lock().unwrap() = Some(judge.clone());
    let flow = Flow::start(
        name,
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    let id = if chat {
        new_chat(&flow).await
    } else {
        flow.conversation.clone()
    };
    flow.say_to(&id, "Run on.").await;
    super::eventually("the turn to run", || {
        answers.lock().unwrap().contains_key("running")
    })
    .await;
    (flow, id, answers, release, judge)
}

/// The user's Stop, its turn not ended yet: a follow-up is no row and no judge; the turn after it
/// carries it.
async fn right_in_behind_a_stop(name: &str, chat: bool) {
    let (flow, id, answers, release, judge) = running_slow(name, chat).await;
    flow.manager.interrupt(id.clone()).await.unwrap();
    assert!(
        flow.manager.conv(&id).unwrap().is_busy().await,
        "the stopped turn is still ending"
    );
    let sent = follow_up(&flow, &id, FIRST).await;
    went_right_in(&flow, &id, sent).await;
    release.notify_one();
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input == FIRST)
    })
    .await;
    judge.add_permits(1);
    flow.stop().await;
}

#[tokio::test]
async fn a_follow_up_goes_right_in_behind_a_turn_being_stopped() {
    right_in_behind_a_stop("block-stopping-right-in", false).await;
}

#[tokio::test]
async fn a_chats_follow_up_goes_right_in_behind_a_turn_being_stopped() {
    right_in_behind_a_stop("block-chat-stopping-right-in", true).await;
}

/// A row Deciding when the user stops: it stops reading so at once, with its judge still out, and
/// that judge's FITS no longer moves it; a new follow-up goes right in past the paused rail.
#[tokio::test]
async fn a_stop_ends_deciding_on_a_row_already_in_the_rail() {
    let (flow, id, answers, release, judge) = running_slow("block-stop-row", false).await;
    let row = match follow_up(&flow, &id, FIRST).await {
        SendOutcome::Queued(item) => item,
        sent => panic!("it is judged while the turn runs: {sent:?}"),
    };
    assert!(row.deciding, "{row:?}");
    flow.manager.interrupt(id.clone()).await.unwrap();
    let queue = flow.board().await.queue;
    assert!(queue.paused);
    assert!(
        queue.items.iter().all(|item| !item.deciding),
        "its judge is still out, but can't move it: {queue:?}"
    );
    release.notify_one();
    let conv = flow.manager.conv(&id).unwrap();
    super::eventually_async("the turn to end", || conv.idle()).await;
    let sent = follow_up(&flow, &id, "Then the docs.").await;
    went_right_in(&flow, &id, sent).await;
    super::eventually("its turn", || {
        inputs(&answers)
            .iter()
            .any(|input| input == "Then the docs.")
    })
    .await;
    judge.add_permits(1);
    flow.idle().await;
    let queue = flow.board().await.queue;
    assert_eq!(
        queue.items.iter().map(|item| &item.id).collect::<Vec<_>>(),
        [&row.id],
        "the row waits for the user's Resume: {queue:?}"
    );
    flow.stop().await;
}

/// A worker that really runs, with no Stop: its follow-up is still judged (D157).
#[tokio::test]
async fn a_follow_up_is_still_judged_while_a_worker_really_runs() {
    let (flow, _answers, release) = block("block-running-judged").await;
    let id = flow.conversation.clone();
    judged(&flow, &id, FIRST).await;
    waits(&flow, FIRST).await;
    release.notify_one();
    flow.stop().await;
}

/// `text` is judged: a row of the rail, Deciding from the first frame.
async fn judged(flow: &Flow, id: &ConversationId, text: &str) {
    let row = match follow_up(flow, id, text).await {
        SendOutcome::Queued(item) => item,
        sent => panic!("it is judged: {sent:?}"),
    };
    assert!(row.deciding, "Deciding from the first frame: {row:?}");
}

/// The gap before Stop's worker stops arrive: the worker still runs, yet the block is stopped, so
/// a follow-up goes right in and joins it. The turn it starts ends the Stop: the next one, the
/// worker still running, is judged again.
#[cfg(debug_assertions)]
#[tokio::test]
async fn a_follow_up_goes_right_in_after_a_stop_before_its_worker_stops() {
    let (flow, answers, release) = block("block-stop-gap").await;
    let id = flow.conversation.clone();
    let open = flow.board().await.latest_request().unwrap().id.clone();
    flow.manager.interrupt(id.clone()).await.unwrap();
    let conv = flow.manager.conv(&id).unwrap();
    assert!(
        flow.manager.queue_holds(&conv).await,
        "its worker still runs"
    );
    let sent = follow_up(&flow, &id, FIRST).await;
    let message = went_right_in(&flow, &id, sent).await;
    flow.idle().await;
    let marks = super::tab_chat_tests::steer_marks(&flow, &message.id).await;
    assert!(
        !marks.is_empty()
            && marks
                .iter()
                .all(|into| into.as_deref() == Some(open.as_str())),
        "it joins the block that holds: {marks:?}"
    );
    assert_eq!(inputs(&answers).last().unwrap(), FIRST);
    judged(&flow, &id, "And the docs.").await;
    waits(&flow, "And the docs.").await;
    release.notify_one();
    flow.stop().await;
}

/// The same gap in a Chat.
#[tokio::test]
async fn a_chats_follow_up_goes_right_in_after_a_stop_before_its_worker_stops() {
    let (script, answers, release) = worker_script();
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("WAITS");
    let flow = Flow::start(
        "block-chat-stop-gap",
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Fix it from the Chat.").await;
    let conv = flow.manager.conv(&chat).unwrap();
    super::eventually("the worker to run", || {
        answers.lock().unwrap().contains_key("worker running")
    })
    .await;
    super::eventually_async("the thread to rest", || conv.idle()).await;
    flow.manager.interrupt(chat.clone()).await.unwrap();
    assert!(
        flow.manager.queue_holds(&conv).await,
        "its worker still runs"
    );
    let sent = follow_up(&flow, &chat, FIRST).await;
    went_right_in(&flow, &chat, sent).await;
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input == FIRST)
    })
    .await;
    release.notify_one();
    flow.stop().await;
}

/// A worker the user resumes after their Stop really runs: no Stop stands, so a follow-up is
/// judged as usual, with no turn having started since.
#[tokio::test]
async fn a_follow_up_is_judged_again_once_a_worker_is_resumed_after_a_stop() {
    let (flow, _answers, release) = block("block-stop-resumed").await;
    let id = flow.conversation.clone();
    let task = flow.manager.core.tasks(&id).await.unwrap().remove(0);
    assert!(flow.manager.pause_task(task.id.clone()).await.unwrap());
    flow.manager.interrupt(id.clone()).await.unwrap();
    flow.manager.resume_task(task.id).await.unwrap();
    let conv = flow.manager.conv(&id).unwrap();
    assert!(
        flow.manager.queue_holds(&conv).await,
        "its worker runs again"
    );
    assert!(conv.idle().await, "no turn started since the Stop");
    judged(&flow, &id, FIRST).await;
    waits(&flow, FIRST).await;
    release.notify_one();
    flow.stop().await;
}

/// Don't resume also marks the thread interrupted, but it is no Stop: a worker that really runs
/// still has its follow-up judged.
#[tokio::test]
async fn dont_resume_is_no_stop_a_running_workers_follow_up_is_still_judged() {
    let (flow, _answers, release) = block("block-dont-resume").await;
    let id = flow.conversation.clone();
    flow.manager.dont_resume(&id).await.unwrap();
    judged(&flow, &id, FIRST).await;
    waits(&flow, FIRST).await;
    release.notify_one();
    flow.stop().await;
}

/// A session whose "Start it." turn emits `events` and rests, keeping the CLI's events; the
/// judge answers WAITS.
async fn cli_work(
    name: &str,
    events: Vec<ProviderEvent>,
) -> (Flow, Answers, mpsc::Sender<ProviderEvent>) {
    let answers = Answers::default();
    let sender: Arc<Mutex<Option<mpsc::Sender<ProviderEvent>>>> = Arc::default();
    let script: Script = {
        let (answers, sender) = (answers.clone(), sender.clone());
        Arc::new(move |turn| {
            let (answers, sender, events) = (answers.clone(), sender.clone(), events.clone());
            Box::pin(async move {
                answers
                    .lock()
                    .unwrap()
                    .entry("inputs".into())
                    .or_insert_with(|| json!([]))
                    .as_array_mut()
                    .unwrap()
                    .push(json!(turn.input));
                if turn.input.contains("Start it.") {
                    for event in events {
                        let _ = turn.events.send(event).await;
                    }
                    *sender.lock().unwrap() = Some(turn.events.clone());
                }
                Reply::text("Noted.")
            })
        })
    };
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("WAITS");
    let flow = Flow::start(
        name,
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    flow.say("Start it.").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the turn to end", || conv.idle()).await;
    assert!(flow.manager.queue_holds(&conv).await, "its work holds");
    let sender = sender.lock().unwrap().clone().expect("the CLI's events");
    (flow, answers, sender)
}

fn job(running: &[&str]) -> ProviderEvent {
    ProviderEvent::BackgroundJobs {
        running: running.iter().map(|job| (*job).to_owned()).collect(),
    }
}

fn subagent() -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: "spawn".into(),
        name: "agents/spawnAgent".into(),
        input: Some(r#"{"message":"sleep 40"}"#.into()),
        status: ItemStatus::InProgress,
        output: None,
        kept: None,
    }
}

/// After the user's Stop `flow`'s follow-up goes right in, on a turn of its own message.
async fn right_in_after_stop(flow: &Flow, answers: &Answers) {
    let id = flow.conversation.clone();
    let sent = follow_up(flow, &id, FIRST).await;
    went_right_in(flow, &id, sent).await;
    super::eventually("its turn", || {
        inputs(answers).iter().any(|input| input == FIRST)
    })
    .await;
}

/// With no Stop, a background command or a subagent still running has it judged (D157).
#[tokio::test]
async fn a_follow_up_is_still_judged_while_cli_work_really_runs() {
    for (name, event) in [
        ("block-job-judged", job(&["a"])),
        ("block-subagent-judged", subagent()),
    ] {
        let (flow, _answers, _sender) = cli_work(name, vec![event]).await;
        let id = flow.conversation.clone();
        judged(&flow, &id, FIRST).await;
        waits(&flow, FIRST).await;
        flow.stop().await;
    }
}

/// After the user's Stop, a background command still running judges nothing.
#[tokio::test]
async fn a_follow_up_goes_right_in_after_a_stop_while_a_background_command_runs() {
    let (flow, answers, sender) = cli_work("block-stop-job", vec![job(&["a"])]).await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    right_in_after_stop(&flow, &answers).await;
    drop(sender);
    flow.stop().await;
}

/// After the user's Stop, a subagent still running judges nothing.
#[tokio::test]
async fn a_follow_up_goes_right_in_after_a_stop_while_a_subagent_runs() {
    let (flow, answers, sender) = cli_work("block-stop-subagent", vec![subagent()]).await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    right_in_after_stop(&flow, &answers).await;
    drop(sender);
    flow.stop().await;
}

/// After the user's Stop, a finished command whose turn hasn't started judges nothing, though it
/// still holds the block.
#[tokio::test]
async fn a_follow_up_goes_right_in_after_a_stop_while_a_finished_commands_turn_is_owed() {
    use brigadier_providers::claude::parse::{Output, Parser};

    let (script, answers, _release) = worker_script();
    let behavior = Arc::new(FakeBehavior {
        session_file: true,
        ..FakeBehavior::default()
    });
    *behavior.judge.lock().unwrap() = Some("WAITS");
    let flow = Flow::start(
        "block-stop-owed",
        Options {
            behavior,
            ..Options::default()
        },
        script,
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Look.").await;
    flow.settled().await;
    let conv = flow.manager.conv(&id).unwrap();
    super::eventually_async("the turn to end", || conv.idle()).await;
    flow.manager.interrupt(id.clone()).await.unwrap();
    // The thread's own record: the command ran, and ended with the turn it owes not yet begun.
    let mut parser = Parser::live();
    for line in include_str!("fixtures/claude-background-watch.jsonl").lines() {
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        if value.get("phase").and_then(serde_json::Value::as_str) == Some("owed") {
            break;
        }
        if value.get("phase").is_some() || value.get("fixture").is_some() {
            continue;
        }
        for output in parser.feed(line) {
            if let Output::Event(event) = output {
                flow.manager.follow_cli_thread(&conv, Some(&event)).await;
            }
        }
    }
    assert!(
        flow.manager.thread_view(&id).unwrap().jobs.waking,
        "its end owes a turn"
    );
    assert!(flow.manager.work_holds(&conv).await, "it holds the block");
    let sent = follow_up(&flow, &id, FIRST).await;
    went_right_in(&flow, &id, sent).await;
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input == FIRST)
    })
    .await;
    flow.stop().await;
}

/// The CLI's own turn for a job's end, after a Stop: the Stop stands, so a follow-up is no row
/// and no judge; it goes once that turn ends.
#[tokio::test]
async fn a_follow_up_goes_right_in_behind_the_clis_own_turn_after_a_stop() {
    let (flow, answers, sender) = cli_work("block-stop-own-turn", vec![job(&["a"])]).await;
    let id = flow.conversation.clone();
    flow.manager.interrupt(id.clone()).await.unwrap();
    sender.send(job(&[])).await.unwrap();
    sender
        .send(ProviderEvent::TurnStarted { turn_id: None })
        .await
        .unwrap();
    let conv = flow.manager.conv(&id).unwrap();
    super::eventually_async("the CLI's own turn", || async { !conv.idle().await }).await;
    let sent = follow_up(&flow, &id, FIRST).await;
    went_right_in(&flow, &id, sent).await;
    assert!(
        !inputs(&answers).iter().any(|input| input == FIRST),
        "it waits for that turn's end"
    );
    sender
        .send(ProviderEvent::TurnCompleted {
            turn_id: None,
            status: brigadier_providers::TurnStatus::Completed,
            duration_ms: Some(1),
            usage: None,
        })
        .await
        .unwrap();
    super::eventually("its turn", || {
        inputs(&answers).iter().any(|input| input == FIRST)
    })
    .await;
    flow.stop().await;
}
