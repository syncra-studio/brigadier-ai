//! Goal 8 (D1, goal.md "for the orchestrator AND its workers"): the user's message to a busy
//! worker (the composer's To:) queues or steers as one to the session does. Queued, it waits in
//! the session's rail for that worker, is edited, deleted or steered like any row, never goes
//! to the session, and goes as the worker's next turn once its turn completes; after a stop it
//! waits. Kept across a restart, it goes on then.

use std::sync::Arc;

use brigadier_cli::protocol::CliRequest;
use serde_json::{Value, json};

use super::cli_tests::{Answers, brigadier, id_of, start, wait};
use super::{FakeBehavior, Flow, Options, Reply, Script};
use crate::work::{QueuedMessage, TaskState};

/// A session that starts one worker on "Start one."; the worker's "Build it." turn reads what
/// is steered in until `release` (or a stop), and each turn's input is kept.
fn script(answers: Answers, release: Arc<tokio::sync::Notify>) -> Script {
    Arc::new(move |turn| {
        let (answers, release) = (answers.clone(), release.clone());
        Box::pin(async move {
            let push = |name: &str, value: String| {
                let mut answers = answers.lock().unwrap();
                let list = answers.entry(name.into()).or_insert_with(|| json!([]));
                list.as_array_mut().unwrap().push(json!(value));
            };
            if turn.worker().is_some() {
                push("worker", turn.input.clone());
                if !turn.input.contains("Build it.") {
                    return Reply::text("Done.");
                }
                loop {
                    tokio::select! {
                        () = release.notified() => return Reply::text("Built."),
                        true = turn.stopped() => return Reply::text("Stopped."),
                        Some(steered) = turn.steered() => push("steered", steered),
                    }
                }
            }
            push("thread", turn.input.clone());
            if turn.input.contains("Start one.") {
                brigadier(&turn, CliRequest::Start(Box::new(start("Build it.")))).await;
                return Reply::text("Started.");
            }
            Reply::text("Noted.")
        })
    })
}

fn list(answers: &Answers, name: &str) -> Vec<String> {
    answers
        .lock()
        .unwrap()
        .get(name)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The session with its worker in its "Build it." turn.
async fn working(name: &str) -> (Flow, Answers, Arc<tokio::sync::Notify>, crate::work::Task) {
    let answers = Answers::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script(answers.clone(), release.clone()),
    )
    .await;
    flow.say("Start one.").await;
    super::eventually("the worker to work", || {
        list(&answers, "worker")
            .iter()
            .any(|input| input.contains("Build it."))
    })
    .await;
    let task = Flow::task(&flow.board().await, 1).clone();
    (flow, answers, release, task)
}

async fn to_worker(flow: &Flow, task: &crate::work::Task, text: &str, steer: bool) -> bool {
    flow.manager
        .send_to_worker(
            flow.conversation.clone(),
            task.id.clone(),
            text.into(),
            Vec::new(),
            Vec::new(),
            steer,
            None,
        )
        .await
        .unwrap()
}

fn row<'a>(items: &'a [QueuedMessage], text: &str) -> &'a QueuedMessage {
    items
        .iter()
        .find(|item| item.text == text)
        .unwrap_or_else(|| panic!("{text} queued: {items:?}"))
}

/// Queued for the worker: never into its running turn, never the session's; edited and steered
/// rows keep their worker; a deleted one goes nowhere; once the turn completes the next goes,
/// once, as the worker's next turn.
#[tokio::test]
async fn a_message_to_a_busy_worker_waits_for_it_and_goes_once_its_turn_completes() {
    let (flow, answers, release, task) = working("worker-queue").await;
    assert!(
        to_worker(&flow, &task, "Use blue.", false).await,
        "it waits"
    );
    assert!(to_worker(&flow, &task, "Round the corners.", false).await);
    assert!(to_worker(&flow, &task, "Ship it.", false).await);
    let board = flow.board().await;
    for text in ["Use blue.", "Round the corners.", "Ship it."] {
        assert_eq!(
            row(&board.queue.items, text).worker.as_ref(),
            Some(&task.id)
        );
    }

    // Edited: the same row, still the worker's.
    let blue = row(&board.queue.items, "Use blue.").id.clone();
    let edited = flow
        .manager
        .edit_queued(
            &flow.conversation,
            &blue,
            "Use green.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(edited.items[0].text, "Use green.");
    assert_eq!(edited.items[0].worker.as_ref(), Some(&task.id));
    // Deleted: it goes nowhere.
    let corners = row(&edited.items, "Round the corners.").id.clone();
    flow.manager
        .delete_queued(&flow.conversation, &corners)
        .await
        .unwrap();
    // Steered: into the worker's running turn now.
    let ship = row(&edited.items, "Ship it.").id.clone();
    flow.manager
        .steer_queued(flow.conversation.clone(), ship)
        .await
        .unwrap();
    super::eventually("the running turn to take it", || {
        list(&answers, "steered")
            .iter()
            .any(|steered| steered.contains("Ship it."))
    })
    .await;
    let board = flow.board().await;
    let texts: Vec<&str> = board
        .queue
        .items
        .iter()
        .map(|item| item.text.as_str())
        .collect();
    assert_eq!(texts, ["Use green."], "{:?}", board.queue);

    // The turn completes: the edited one goes, once.
    release.notify_one();
    flow.until("the queue to empty", |board| board.queue.items.is_empty())
        .await;
    super::eventually("the worker's next turn", || {
        list(&answers, "worker")
            .iter()
            .any(|input| input.contains("Use green."))
    })
    .await;
    flow.idle().await;
    let all = [list(&answers, "worker"), list(&answers, "steered")].concat();
    assert_eq!(
        all.iter()
            .filter(|input| input.contains("Use green."))
            .count(),
        1,
        "{all:#?}"
    );
    assert_eq!(
        all.iter()
            .filter(|input| input.contains("Ship it."))
            .count(),
        1,
        "{all:#?}"
    );
    assert!(
        !all.iter().any(|input| input.contains("Round the corners.")),
        "{all:#?}"
    );
    assert!(
        !all.iter().any(|input| input.contains("Use blue.")),
        "{all:#?}"
    );
    // The session never took them as its own messages, nor judged them.
    let thread = list(&answers, "thread");
    assert!(
        !thread.iter().any(|input| input.contains("[queued message")),
        "{thread:#?}"
    );
    flow.stop().await;
}

/// Steer (the setting, ⌥⏎) goes into the worker's running turn at once, as before.
#[tokio::test]
async fn a_steer_to_a_busy_worker_goes_into_its_running_turn() {
    let (flow, answers, release, task) = working("worker-steer").await;
    assert!(
        !to_worker(&flow, &task, "Use blue.", true).await,
        "nothing waits"
    );
    super::eventually("the running turn to take it", || {
        list(&answers, "steered")
            .iter()
            .any(|steered| steered.contains("Use blue."))
    })
    .await;
    assert!(flow.board().await.queue.items.is_empty());
    release.notify_one();
    flow.stop().await;
}

/// After the user stops the worker's turn its message waits (§5.3): nothing sends it by
/// itself; their Steer on it sends it as the worker's next turn.
#[tokio::test]
async fn after_a_stop_the_workers_message_waits_until_steered() {
    let (flow, answers, _release, task) = working("worker-stopped").await;
    assert!(to_worker(&flow, &task, "Use blue.", false).await);
    flow.manager.pause_task(task.id.clone()).await.unwrap();
    flow.until("the turn to end", |board| {
        Flow::task(board, 1).state != TaskState::Running
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let board = flow.board().await;
    assert_eq!(board.queue.items.len(), 1, "it waits: {:?}", board.queue);
    assert!(
        !list(&answers, "worker")
            .iter()
            .any(|input| input.contains("Use blue.")),
        "nothing sent it"
    );
    let id = board.queue.items[0].id.clone();
    flow.manager
        .steer_queued(flow.conversation.clone(), id)
        .await
        .unwrap();
    super::eventually("the worker to take it", || {
        list(&answers, "worker")
            .iter()
            .any(|input| input.contains("Use blue."))
    })
    .await;
    assert!(flow.board().await.queue.items.is_empty());
    flow.stop().await;
}

/// Two deliveries of one worker row at once (a drain and the user's Steer, or two Steers):
/// it reaches the worker once.
#[tokio::test]
async fn a_worker_row_steered_twice_at_once_reaches_the_worker_once() {
    let (flow, answers, _release, task) = working("worker-row-once").await;
    assert!(to_worker(&flow, &task, "Use blue.", false).await);
    flow.manager.pause_task(task.id.clone()).await.unwrap();
    flow.until("the turn to end", |board| {
        Flow::task(board, 1).state != TaskState::Running
    })
    .await;
    let id = flow.board().await.queue.items[0].id.clone();
    let (first, second) = tokio::join!(
        flow.manager
            .steer_queued(flow.conversation.clone(), id.clone()),
        flow.manager.steer_queued(flow.conversation.clone(), id),
    );
    first.unwrap();
    second.unwrap();
    flow.until("the queue to empty", |board| board.queue.items.is_empty())
        .await;
    flow.idle().await;
    let all = [list(&answers, "worker"), list(&answers, "steered")].concat();
    assert_eq!(
        all.iter()
            .filter(|input| input.contains("Use blue."))
            .count(),
        1,
        "{all:#?}"
    );
    flow.stop().await;
}

/// Kept across a restart: the worker's message goes on once Brigadier is back.
#[tokio::test]
async fn a_workers_queued_message_survives_a_restart_and_goes_on() {
    let (mut flow, answers, _release, task) = working("worker-queue-restart").await;
    assert!(to_worker(&flow, &task, "Use blue.", false).await);
    flow.restart().await;
    super::eventually("the worker to take it after the restart", || {
        list(&answers, "worker")
            .iter()
            .any(|input| input.contains("Use blue."))
    })
    .await;
    flow.until("the queue to empty", |board| board.queue.items.is_empty())
        .await;
    assert_eq!(
        list(&answers, "worker")
            .iter()
            .filter(|input| input.contains("Use blue."))
            .count(),
        1
    );
    flow.stop().await;
}

/// A wait on a worker that already reported ends at once: idle, it sends nothing until it is
/// messaged, so the wait (seen live on Codex) held its command and the work block open for good.
#[tokio::test]
async fn a_wait_on_a_worker_that_already_reported_ends() {
    let answers = Answers::default();
    let kept = answers.clone();
    let flow = Flow::start(
        "wait-reported",
        Options::default(),
        Arc::new(move |turn| {
            let answers = kept.clone();
            Box::pin(async move {
                if turn.worker().is_some() {
                    return Reply::text("Done.");
                }
                let id =
                    id_of(&brigadier(&turn, CliRequest::Start(Box::new(start("Say hi.")))).await);
                let took = brigadier(&turn, wait(&[&id], Some(60))).await;
                assert_eq!(took["timedOut"], false, "{took:#}");
                let again = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    brigadier(&turn, wait(&[&id], None)),
                )
                .await;
                answers
                    .lock()
                    .unwrap()
                    .insert("again".into(), json!(again.is_ok()));
                Reply::text("Waited.")
            })
        }),
    )
    .await;
    flow.say("Go.").await;
    super::eventually("the second wait to end", || {
        answers.lock().unwrap().contains_key("again")
    })
    .await;
    assert_eq!(
        answers.lock().unwrap()["again"],
        json!(true),
        "the wait on a reported worker never ended"
    );
    flow.stop().await;
}
/// A bare `brigadier wait` ends once its session's last worker that could send is closed (seen
/// live on Codex: a `wait <id>` took the worker's event, the worker was closed, and the bare
/// wait beside it held the turn's command, and the block, open for good).
#[tokio::test]
async fn a_bare_wait_ends_when_the_last_worker_is_closed() {
    let answers = Answers::default();
    let kept = answers.clone();
    let flow = Flow::start(
        "bare-wait-close",
        Options::default(),
        Arc::new(move |turn| {
            let answers = kept.clone();
            Box::pin(async move {
                if turn.worker().is_some() {
                    return Reply::text("Done.");
                }
                let id =
                    id_of(&brigadier(&turn, CliRequest::Start(Box::new(start("Say hi.")))).await);
                let took = brigadier(&turn, wait(&[&id], Some(60))).await;
                assert_eq!(took["timedOut"], false, "{took:#}");
                let close = async {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    brigadier(&turn, CliRequest::Close { id: id.clone() }).await
                };
                let bare = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    brigadier(&turn, wait(&[], None)),
                );
                let (bare, _) = tokio::join!(bare, close);
                answers
                    .lock()
                    .unwrap()
                    .insert("bare".into(), json!(bare.is_ok()));
                Reply::text("Closed.")
            })
        }),
    )
    .await;
    flow.say("Go.").await;
    super::eventually("the bare wait to end", || {
        answers.lock().unwrap().contains_key("bare")
    })
    .await;
    assert_eq!(
        answers.lock().unwrap()["bare"],
        json!(true),
        "the bare wait never ended"
    );
    flow.stop().await;
}
