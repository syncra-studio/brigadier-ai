//! Judging (D157, D163): a message the user sends while a session works is a row of the rail at
//! once, Deciding, and a one-shot beside the session judges it in the background, even while a
//! long command holds the running turn: FITS steers it into the open block; WAITS, a late answer
//! or none leaves it in the rail for its own turn, and the user taking the row over drops the
//! answer. Deciding ends with any of them, a restart too. The session's turn is never asked to judge. Same on every CLI.

use std::sync::Arc;
use std::time::{Duration, Instant};

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

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
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some(judge);
    send_with(name, kind, behavior).await
}

/// [`send_while_working`] on Claude, its judge answering FITS only once the permit it returns is
/// given: the answer comes after whatever the test does to the row.
async fn send_while_judged(
    name: &str,
) -> (
    Flow,
    Answers,
    Arc<tokio::sync::Notify>,
    QueuedMessage,
    Arc<Semaphore>,
) {
    let judge = Arc::new(Semaphore::new(0));
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("FITS");
    *behavior.judge_gate.lock().unwrap() = Some(judge.clone());
    let (flow, answers, gate, item) = send_with(name, ProviderKind::Claude, behavior).await;
    (flow, answers, gate, item, judge)
}

async fn send_with(
    name: &str,
    kind: ProviderKind,
    behavior: Arc<FakeBehavior>,
) -> (Flow, Answers, Arc<tokio::sync::Notify>, QueuedMessage) {
    let answers = Answers::default();
    let gate = Arc::new(tokio::sync::Notify::new());
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

/// Waits until `item` is still queued, no longer Deciding.
async fn until_decided(flow: &Flow, item: &QueuedMessage) {
    flow.until("the row to stop reading Deciding", |board| {
        board
            .queue
            .items
            .iter()
            .any(|queued| queued.id == item.id && !queued.deciding)
    })
    .await;
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
        let conv = flow.manager.conv(&flow.conversation).unwrap();
        super::eventually("the judge to answer", || !conv.is_judging()).await;
        assert!(
            flow.manager
                .core
                .board(&flow.conversation)
                .await
                .unwrap()
                .queue
                .items
                .iter()
                .any(|queued| queued.id == item.id),
            "it waits while the command runs",
        );
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

/// The row reads Deciding from its first frame until the judge's answer lands, on every CLI.
#[tokio::test]
async fn a_row_reads_deciding_until_its_judge_answers() {
    for kind in [ProviderKind::Claude, ProviderKind::Codex] {
        let (flow, _answers, gate, item) =
            send_while_working(&format!("queue-deciding-{kind:?}"), kind, "WAITS").await;
        until_decided(&flow, &item).await;
        gate.notify_one();
        flow.settled().await;
        flow.stop().await;
    }
}

/// The write that ends Deciding fails (storage refuses it): it's tried again until the row stops
/// reading Deciding, with nothing else touching the row.
#[tokio::test]
async fn a_failed_write_never_leaves_deciding_on_a_row() {
    let judge = Arc::new(Semaphore::new(0));
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("WAITS");
    *behavior.judge_gate.lock().unwrap() = Some(judge.clone());
    let (flow, _answers, gate, item) =
        send_with("queue-failed-settle", ProviderKind::Claude, behavior).await;
    let db = rusqlite::Connection::open(flow.dir.join("data").join("brigadier.db")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER refuse_queue BEFORE INSERT ON events WHEN NEW.kind = 'queue.changed'
         BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )
    .unwrap();
    judge.add_permits(1);
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually("the judge to answer", || !conv.is_judging()).await;
    tokio::time::sleep(ANSWER_ACTS).await;
    assert!(
        row(&flow, &item).await.unwrap().deciding,
        "the write failed"
    );
    db.execute_batch("DROP TRIGGER refuse_queue;").unwrap();
    until_decided(&flow, &item).await;
    gate.notify_one();
    flow.settled().await;
    flow.stop().await;
}

/// A restart ends what the stop caught Deciding: no judge outlives it.
#[tokio::test]
async fn a_restart_ends_deciding() {
    let (mut flow, _answers, _gate, item, _judge) = send_while_judged("queue-restart-judged").await;
    flow.crash().await;
    flow.until("the start to end Deciding", |board| {
        !board
            .queue
            .items
            .iter()
            .any(|queued| queued.id == item.id && queued.deciding)
    })
    .await;
    flow.stop().await;
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

/// Time for a judge's answer to have acted on a row, had it counted.
const ANSWER_ACTS: Duration = Duration::from_millis(300);

/// Whether row `item` is still queued, and under which words.
async fn row(flow: &Flow, item: &QueuedMessage) -> Option<QueuedMessage> {
    flow.manager
        .core
        .board(&flow.conversation)
        .await
        .unwrap()
        .queue
        .items
        .into_iter()
        .find(|queued| queued.id == item.id)
}

/// The user's Steer takes the row: it goes in once, and the judge's late FITS does nothing more.
#[tokio::test]
async fn steering_a_row_being_judged_drops_the_judges_answer() {
    let (flow, answers, gate, item, judge) = send_while_judged("queue-steer-judged").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(conv.judges(&item.id));
    flow.manager
        .steer_queued(flow.conversation.clone(), item.id.clone())
        .await
        .unwrap();
    assert!(!conv.judges(&item.id), "the judge lost its say");
    let board = flow.until("it to join", |board| joined(board, &item)).await;
    assert!(board.requests[&item.id].steered_into.is_some());
    judge.add_permits(1);
    tokio::time::sleep(ANSWER_ACTS).await;
    gate.notify_one();
    flow.settled().await;
    assert_eq!(inputs(&answers).len(), 1, "never a turn of its own");
    flow.stop().await;
}

/// A Steer refused (the session compacts) still took the row from the judge: its late FITS
/// doesn't steer it in once the compaction ends. It waits for its own turn.
#[tokio::test]
async fn a_refused_steer_still_drops_the_judges_answer() {
    let (flow, answers, gate, item, judge) = send_while_judged("queue-steer-refused").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    conv.set_compacting(true).await;
    assert!(
        flow.manager
            .steer_queued(flow.conversation.clone(), item.id.clone())
            .await
            .is_err()
    );
    assert!(!row(&flow, &item).await.unwrap().deciding);
    conv.set_compacting(false).await;
    judge.add_permits(1);
    tokio::time::sleep(ANSWER_ACTS).await;
    assert!(row(&flow, &item).await.is_some(), "the answer was dropped");
    gate.notify_one();
    flow.until("it to go on its own turn", |board| joined(board, &item))
        .await;
    let board = flow.settled().await;
    assert!(board.requests[&item.id].steered_into.is_none());
    assert_eq!(inputs(&answers).len(), 2);
    flow.stop().await;
}

/// The user edits the row while it is judged: the late FITS is dropped, and it goes on its own
/// turn, with the user's words.
#[tokio::test]
async fn editing_a_row_being_judged_drops_the_judges_answer() {
    let (flow, answers, gate, item, judge) = send_while_judged("queue-edit-judged").await;
    let queue = flow
        .manager
        .edit_queued(
            &flow.conversation,
            &item.id,
            "Check the docs instead.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert!(!queue.items[0].deciding);
    judge.add_permits(1);
    tokio::time::sleep(ANSWER_ACTS).await;
    assert_eq!(
        row(&flow, &item).await.unwrap().text,
        "Check the docs instead."
    );
    gate.notify_one();
    flow.until("it to go on its own turn", |board| joined(board, &item))
        .await;
    flow.settled().await;
    let all = inputs(&answers);
    assert_eq!(all.len(), 2, "{all:#?}");
    assert!(all[1].contains("Check the docs instead.") && !all[1].contains(ASKED));
    flow.stop().await;
}

/// The user moves the row while it is judged: the late FITS is dropped, and it stays where they
/// put it.
#[tokio::test]
async fn moving_a_row_being_judged_drops_the_judges_answer() {
    let (flow, _answers, gate, item, judge) = send_while_judged("queue-move-judged").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    let queue = flow
        .manager
        .move_queued(&flow.conversation, &item.id, 0)
        .await
        .unwrap();
    assert_eq!(queue.items[0].id, item.id);
    assert!(!queue.items[0].deciding);
    assert!(!conv.judges(&item.id));
    judge.add_permits(1);
    tokio::time::sleep(ANSWER_ACTS).await;
    assert!(row(&flow, &item).await.is_some(), "the answer was dropped");
    gate.notify_one();
    flow.until("it to go on its own turn", |board| joined(board, &item))
        .await;
    flow.settled().await;
    flow.stop().await;
}

/// The user deletes the row while it is judged: the late FITS brings nothing back.
#[tokio::test]
async fn deleting_a_row_being_judged_drops_the_judges_answer() {
    let (flow, answers, gate, item, judge) = send_while_judged("queue-delete-judged").await;
    flow.manager
        .delete_queued(&flow.conversation, &item.id)
        .await
        .unwrap();
    judge.add_permits(1);
    tokio::time::sleep(ANSWER_ACTS).await;
    gate.notify_one();
    let board = flow.settled().await;
    assert!(!board.requests.contains_key(&item.id));
    assert_eq!(inputs(&answers).len(), 1);
    flow.stop().await;
}

/// The turn ends before the judge answers: the row doesn't wait for it, and goes on its own turn.
#[tokio::test]
async fn a_row_never_waits_for_its_judge() {
    let (flow, answers, gate, item, judge) = send_while_judged("queue-no-wait").await;
    gate.notify_one();
    flow.until("it to go on its own turn", |board| joined(board, &item))
        .await;
    judge.add_permits(1);
    let board = flow.settled().await;
    assert!(board.requests[&item.id].steered_into.is_none());
    assert_eq!(inputs(&answers).len(), 2);
    flow.stop().await;
}

/// A judge that never answers gives up at its patience; the row stays queued for its own turn.
#[tokio::test]
async fn a_hung_judge_gives_up_after_about_five_seconds() {
    let (flow, _answers, gate, item, judge) = send_while_judged("queue-hung-judge").await;
    let started = Instant::now();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually("the judge to give up", || !conv.is_judging()).await;
    let took = started.elapsed();
    assert!(
        took > Duration::from_secs(4) && took < Duration::from_secs(10),
        "{took:?}"
    );
    until_decided(&flow, &item).await;
    judge.add_permits(1);
    gate.notify_one();
    flow.until("it to go on its own turn", |board| joined(board, &item))
        .await;
    flow.settled().await;
    flow.stop().await;
}

/// The judge's CLI says it will retry: it gives up at once, not at its patience.
#[tokio::test]
async fn a_judge_gives_up_at_its_clis_first_retry() {
    let judge = Arc::new(Semaphore::new(0));
    let behavior = Arc::new(FakeBehavior::default());
    *behavior.judge.lock().unwrap() = Some("FITS");
    *behavior.judge_gate.lock().unwrap() = Some(judge.clone());
    behavior
        .judge_retries
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (flow, _answers, gate, item) =
        send_with("queue-retry-judge", ProviderKind::Claude, behavior).await;
    let started = Instant::now();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually("the judge to give up", || !conv.is_judging()).await;
    let took = started.elapsed();
    assert!(took < Duration::from_secs(3), "{took:?}");
    until_decided(&flow, &item).await;
    judge.add_permits(1);
    gate.notify_one();
    flow.settled().await;
    flow.stop().await;
}
