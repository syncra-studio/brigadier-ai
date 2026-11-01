//! Goal 8 (D24, D25): while a work block is open (its workers still work) a follow-up judged
//! `wait` stays in the queue until the block closes, then goes on its own turn and opens the
//! next block, where every message still waiting is judged again. One the user steers in is
//! taken by the open block, framed so the session knows.

use std::sync::Arc;

use brigadier_cli::protocol::{CliRequest, Judgment};
use regex::Regex;
use serde_json::json;

use super::cli_tests::{Answers, brigadier};
use super::{FakeBehavior, Flow, Options, Reply, Script};

const FIRST: &str = "Unrelated: write the release notes.";
const SECOND: &str = "Unrelated too: bump the version.";

/// The ids of the queued messages `text` gives the session to judge.
fn to_judge(text: &str) -> Vec<String> {
    Regex::new(r"\[queued message (\S{8}) · [^\]]*judge it")
        .unwrap()
        .captures_iter(text)
        .map(|found| found[1].to_owned())
        .collect()
}

fn inputs(answers: &Answers) -> Vec<String> {
    answers.lock().unwrap()["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|input| input.as_str().unwrap().to_owned())
        .collect()
}

/// A session whose first turn starts a worker that runs until `release`. Every message given
/// to judge while that block is open is judged `wait`; in a later block, `join`.
async fn block(name: &str) -> (Flow, Answers, Arc<tokio::sync::Notify>) {
    block_judging(name, Some(Judgment::Wait), None).await
}

/// [`block`], judging a message given to judge with `first` and one asked again after its turn
/// ended unjudged (D72) with `again`; `None` leaves it unjudged.
async fn block_judging(
    name: &str,
    first: Option<Judgment>,
    again: Option<Judgment>,
) -> (Flow, Answers, Arc<tokio::sync::Notify>) {
    let answers = Answers::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let script: Script = {
        let (answers, release) = (answers.clone(), release.clone());
        Arc::new(move |turn| {
            let (answers, release) = (answers.clone(), release.clone());
            let (first, again) = (first.clone(), again.clone());
            Box::pin(async move {
                let keep = |name: &str, value: serde_json::Value| {
                    answers.lock().unwrap().insert(name.into(), value);
                };
                if turn.worker().is_some() {
                    keep("worker running", json!(true));
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
                // Its own turn opens the next block: what still waits is given to judge again.
                if turn.input.contains(", now on its own turn]") && turn.input.contains(FIRST) {
                    let waiting = (turn.host.core)
                        .board(&turn.conversation().unwrap())
                        .await
                        .unwrap()
                        .queue
                        .items
                        .len();
                    if waiting == 0 {
                        return Reply::text("Done.");
                    }
                    let again = turn.steered().await.unwrap_or_default();
                    keep("judged again", json!(again));
                    for id in to_judge(&again) {
                        let judged = brigadier(
                            &turn,
                            CliRequest::Queue {
                                id,
                                judgment: Judgment::Join,
                            },
                        )
                        .await;
                        keep("joined", judged);
                    }
                    return Reply::text("Both done.");
                }
                let judgment = if turn.input.contains("you didn't judge it") {
                    again
                } else {
                    first
                };
                for id in to_judge(&turn.input) {
                    let Some(judgment) = judgment.clone() else {
                        continue;
                    };
                    let judged = brigadier(&turn, CliRequest::Queue { id, judgment }).await;
                    keep("judged", judged);
                }
                Reply::text("Noted.")
            })
        })
    };
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
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

/// Waits until `text` waits in the queue, judged (no longer Deciding).
async fn waits(flow: &Flow, text: &str) -> crate::work::QueuedMessage {
    let board = flow
        .until("it to wait, judged", |board| {
            board
                .queue
                .items
                .iter()
                .any(|item| item.text == text && item.delivered && !item.deciding)
        })
        .await;
    board
        .queue
        .items
        .into_iter()
        .find(|item| item.text == text)
        .unwrap()
}

/// `wait` while the block is open: nothing sends it while the worker works (D25); once the
/// block closes the first goes on its own turn, and the second is judged again in that new
/// block, where it joins. Each reaches the thread's turns once.
#[tokio::test]
async fn a_waiting_message_waits_for_the_next_block_and_is_judged_again_there() {
    let (flow, answers, release) = block("block-wait").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    flow.say(FIRST).await;
    let first = waits(&flow, FIRST).await;
    flow.idle().await;
    flow.say(SECOND).await;
    let second = waits(&flow, SECOND).await;
    flow.idle().await;
    // The block is still open: both stay in the queue, neither went on a turn.
    assert!(flow.manager.work_holds(&conv).await);
    let board = flow.board().await;
    assert_eq!(board.queue.items.len(), 2, "{:?}", board.queue);
    let before = inputs(&answers);
    assert!(
        !before
            .iter()
            .any(|input| input.contains("now on its own turn")),
        "{before:#?}"
    );

    release.notify_one();
    let board = flow
        .until("both to leave the queue", |board| {
            board.queue.items.is_empty()
                && board.requests.contains_key(&first.id)
                && board.requests.contains_key(&second.id)
        })
        .await;
    assert!(board.requests[&second.id].steered_into.is_some());
    flow.settled().await;
    let all = inputs(&answers);
    let own: Vec<&String> = all
        .iter()
        .filter(|input| input.contains("now on its own turn"))
        .collect();
    assert_eq!(own.len(), 1, "{all:#?}");
    assert!(
        own[0].contains(FIRST) && !own[0].contains(SECOND),
        "{all:#?}"
    );
    let again = answers.lock().unwrap()["judged again"].clone();
    let again = again.as_str().unwrap();
    assert!(
        again.contains(&second.id[second.id.len() - 8..]) && again.ends_with(SECOND),
        "the second is judged again in the new block: {again}"
    );
    assert_eq!(answers.lock().unwrap()["joined"]["state"], "joined");
    // The new block is no turn of the old one; the second joined it. (The turns that woke the
    // session to judge them went on in the old block.)
    super::eventually("the new block's turn to name both", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.last().is_some_and(|opened| {
            opened.requests.contains(&first.id) && opened.requests.contains(&second.id)
        })
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert!(!view.turns.last().unwrap().joins, "{view:#?}");
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
    let steered = all.last().unwrap();
    assert!(
        steered.starts_with(&format!(
            "[queued message {}, steered in by the user: ",
            &first.id[first.id.len() - 8..]
        )) && steered.ends_with(FIRST),
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
/// at once, framed, on a turn that goes on with that work; nothing is queued.
#[tokio::test]
async fn a_fresh_steer_while_the_block_is_open_is_taken_framed() {
    let (flow, answers, release) = block("block-steer-fresh").await;
    flow.manager
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
    flow.idle().await;
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

/// How many of the session's turns had `words` in them.
fn read(answers: &Answers, words: &str) -> usize {
    inputs(answers)
        .iter()
        .filter(|input| input.contains(words))
        .count()
}

/// Left unjudged while the block is open (D72): it goes back to the queue, never joined, and
/// is asked once more on a turn that joins the block; judged `wait`, it stays queued until the
/// block closes, then goes on its own turn.
#[tokio::test]
async fn an_unjudged_message_is_asked_once_more_and_waits() {
    let (flow, answers, release) =
        block_judging("block-unjudged-wait", None, Some(Judgment::Wait)).await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    flow.say(FIRST).await;
    super::eventually("its second ask to be judged", || {
        answers.lock().unwrap().contains_key("judged")
    })
    .await;
    flow.idle().await;
    assert_eq!(answers.lock().unwrap()["judged"]["state"], "waiting");
    assert_eq!(read(&answers, "you didn't judge it"), 1);
    assert_eq!(read(&answers, FIRST), 2, "{:#?}", inputs(&answers));
    assert!(flow.manager.work_holds(&conv).await);
    let board = flow.board().await;
    let item = board.queue.items[0].clone();
    assert!(
        board.queue.items.len() == 1 && !item.deciding,
        "{:?}",
        board.queue
    );
    assert!(!board.requests.contains_key(&item.id), "never joined");
    release.notify_one();
    flow.until("it to go on its own turn", |board| {
        board.queue.items.is_empty() && board.requests.contains_key(&item.id)
    })
    .await;
    flow.settled().await;
    assert_eq!(read(&answers, "now on its own turn"), 1);
    assert_eq!(read(&answers, FIRST), 3, "{:#?}", inputs(&answers));
    flow.stop().await;
}

/// Left unjudged, then joined on its second ask: it joins the open block once, on a turn that
/// goes on with that work, and nothing sends it again.
#[tokio::test]
async fn an_unjudged_message_asked_once_more_joins_once() {
    let (flow, answers, release) =
        block_judging("block-unjudged-join", None, Some(Judgment::Join)).await;
    flow.say(FIRST).await;
    let board = flow
        .until("it to join", |board| {
            board.queue.items.is_empty() && !board.requests.is_empty()
        })
        .await;
    flow.idle().await;
    assert_eq!(answers.lock().unwrap()["judged"]["state"], "joined");
    assert_eq!(read(&answers, "you didn't judge it"), 1);
    assert_eq!(read(&answers, FIRST), 2, "{:#?}", inputs(&answers));
    assert_eq!(read(&answers, "now on its own turn"), 0);
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let item = (board.requests.keys())
        .find(|id| !view.turns[0].requests.contains(id))
        .unwrap();
    let joining = view
        .turns
        .iter()
        .rfind(|turn| turn.requests.contains(item))
        .unwrap();
    assert!(joining.joins, "no new block: {view:#?}");
    release.notify_one();
    flow.stop().await;
}

/// Left unjudged twice: it stays queued and visible, never joined and never asked a third
/// time while the block is open; once the block closes it goes on its own turn.
#[tokio::test]
async fn a_message_left_unjudged_twice_stays_queued() {
    let (flow, answers, release) = block_judging("block-unjudged-twice", None, None).await;
    flow.say(FIRST).await;
    super::eventually("its second ask", || {
        read(&answers, "you didn't judge it") == 1
    })
    .await;
    flow.idle().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    flow.idle().await;
    assert_eq!(read(&answers, FIRST), 2, "{:#?}", inputs(&answers));
    let board = flow.board().await;
    let item = board.queue.items[0].clone();
    assert!(
        board.queue.items.len() == 1 && !item.deciding,
        "{:?}",
        board.queue
    );
    assert!(!board.requests.contains_key(&item.id), "never joined");
    release.notify_one();
    flow.until("it to go on its own turn", |board| {
        board.queue.items.is_empty() && board.requests.contains_key(&item.id)
    })
    .await;
    super::eventually("its own turn", || {
        read(&answers, "now on its own turn") == 1
    })
    .await;
    flow.stop().await;
}

/// O38: Claude may stream the first item of a judge turn before it echoes the message that
/// woke it. That turn is still Brigadier's and joins the open block as a turn of its own,
/// never one the CLI began on its own going on with the turn before (D25, D72); a turn the CLI
/// then begins on its own still goes on with it.
#[tokio::test]
async fn a_judge_turn_whose_first_item_comes_before_its_message_joins_the_block() {
    use brigadier_providers::{ItemStatus, ProviderEvent, Role, TurnStatus};
    type Events = Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>>;
    let spawn = |status| ProviderEvent::ToolCall {
        item_id: "spawn".into(),
        name: "agents/spawnAgent".into(),
        input: Some(r#"{"message":"sleep 40"}"#.into()),
        status,
        output: None,
        kept: None,
    };
    let say = |item: &str, role, text: &str| ProviderEvent::Message {
        item_id: item.into(),
        role,
        text: text.into(),
    };
    let events: Events = Arc::default();
    let script: Script = {
        let events = events.clone();
        Arc::new(move |turn| {
            let events = events.clone();
            Box::pin(async move {
                let ids = to_judge(&turn.input);
                if ids.is_empty() {
                    let _ = turn
                        .events
                        .send(say("user-1", Role::User, &turn.input))
                        .await;
                    let _ = turn.events.send(spawn(ItemStatus::InProgress)).await;
                    *events.lock().unwrap() = Some(turn.events.clone());
                    return Reply::text("Started a subagent; not waiting for it.");
                }
                // Its first item, then the message it took.
                let _ = turn
                    .events
                    .send(say("first", Role::Assistant, "Looking."))
                    .await;
                let _ = turn
                    .events
                    .send(say("user-2", Role::User, &turn.input))
                    .await;
                for id in ids {
                    brigadier(
                        &turn,
                        CliRequest::Queue {
                            id,
                            judgment: Judgment::Join,
                        },
                    )
                    .await;
                }
                Reply::text("4.")
            })
        })
    };
    let flow = Flow::start("block-judge-item-first", Options::default(), script).await;
    flow.say("Start a subagent in the background.").await;
    flow.settled().await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the first turn to end", || conv.idle()).await;
    assert!(
        flow.manager.work_holds(&conv).await,
        "its subagent holds the work"
    );

    flow.say("What is 2+2?").await;
    super::eventually("the judge turn to end", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        let answered = view.entries.iter().any(
            |entry| matches!(&entry.event, ProviderEvent::Message { text, .. } if text == "4."),
        );
        answered && view.turns.last().is_some_and(|turn| turn.end.is_some())
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(view.turns.len(), 2, "{view:#?}");
    assert_eq!(view.turns[1].user_message, "user-2", "{view:#?}");
    assert!(view.turns[1].joins, "it joins the open block: {view:#?}");
    assert!(!view.entries.iter().any(|entry| entry.steer), "{view:#?}");

    // The subagent ends; the CLI's own turn for it goes on with the judge turn.
    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    for event in [
        spawn(ItemStatus::Completed),
        ProviderEvent::TurnStarted { turn_id: None },
        say("wake", Role::Assistant, "The subagent printed 42."),
    ] {
        sender.send(event).await.unwrap();
    }
    super::eventually("the wake turn's item", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.entries.iter().any(|entry| {
            matches!(&entry.event, ProviderEvent::Message { item_id, .. } if item_id == "wake")
        })
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(view.turns.len(), 2, "{view:#?}");
    assert!(
        view.turns[1].end.is_none(),
        "the judge turn runs again: {view:#?}"
    );
    sender
        .send(ProviderEvent::TurnCompleted {
            turn_id: None,
            status: TurnStatus::Completed,
            duration_ms: None,
            usage: None,
        })
        .await
        .unwrap();
    super::eventually("the wake turn to end", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        view.turns.len() == 2 && view.turns[1].end.is_some() && view.turns[1].joins
    })
    .await;
    flow.stop().await;
}

/// O38, the other order: the CLI begins a turn of its own for its background job's end, and
/// the first thing in it is a message Brigadier steered in. That turn is the open block's, and
/// so is the message: it joins, drawing no second block (D25, D74).
#[tokio::test]
async fn a_message_steered_first_into_the_clis_own_turn_joins_the_block() {
    use brigadier_providers::{ProviderEvent, Role, TurnStatus};
    type Events = Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>>;
    let say = |item: &str, role, text: &str| ProviderEvent::Message {
        item_id: item.into(),
        role,
        text: text.into(),
    };
    let events: Events = Arc::default();
    let script: Script = {
        let events = events.clone();
        Arc::new(move |turn| {
            let events = events.clone();
            Box::pin(async move {
                let _ = turn
                    .events
                    .send(say("user-1", Role::User, &turn.input))
                    .await;
                let _ = (turn.events)
                    .send(ProviderEvent::BackgroundJobs {
                        running: vec!["job".into()],
                    })
                    .await;
                *events.lock().unwrap() = Some(turn.events.clone());
                Reply::text("Started it in the background.")
            })
        })
    };
    let flow = Flow::start("block-steer-own-turn", Options::default(), script).await;
    flow.say("Start the job.").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the first turn to end", || conv.idle()).await;
    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    for event in [
        ProviderEvent::BackgroundJobs {
            running: Vec::new(),
        },
        ProviderEvent::TurnStarted { turn_id: None },
        say("user-2", Role::User, "What is 2+2?"),
        say("reply", Role::Assistant, "4."),
        ProviderEvent::TurnCompleted {
            turn_id: None,
            status: TurnStatus::Completed,
            duration_ms: None,
            usage: None,
        },
    ] {
        sender.send(event).await.unwrap();
    }
    super::eventually("its own turn to end", || {
        let view = flow.manager.thread_view(&flow.conversation).unwrap();
        let answered = view.entries.iter().any(|entry| {
            matches!(&entry.event, ProviderEvent::Message { item_id, .. } if item_id == "reply")
        });
        answered && view.turns.last().is_some_and(|turn| turn.end.is_some())
    })
    .await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(view.turns.len(), 2, "{view:#?}");
    assert!(view.turns[1].joins, "it joins the open block: {view:#?}");
    flow.stop().await;
}

/// O38 on Codex: a follow-up judged `wait` while a subagent runs past its turn stays queued
/// through the judge turn's end, and goes on its own turn once the subagent ends, with no turn
/// open (D24, D25).
#[tokio::test]
async fn a_message_waiting_on_a_subagent_goes_on_once_it_ends() {
    use brigadier_providers::{ItemStatus, ProviderEvent};
    type Events = Arc<std::sync::Mutex<Option<tokio::sync::mpsc::Sender<ProviderEvent>>>>;
    let spawn = |status| ProviderEvent::ToolCall {
        item_id: "spawn".into(),
        name: "agents/spawnAgent".into(),
        input: Some(r#"{"message":"sleep 40"}"#.into()),
        status,
        output: None,
        kept: None,
    };
    let events: Events = Arc::default();
    let answers = Answers::default();
    let script: Script = {
        let (events, answers) = (events.clone(), answers.clone());
        Arc::new(move |turn| {
            let (events, answers) = (events.clone(), answers.clone());
            Box::pin(async move {
                if turn.input.contains("Start a subagent") {
                    let _ = turn.events.send(spawn(ItemStatus::InProgress)).await;
                    *events.lock().unwrap() = Some(turn.events.clone());
                    return Reply::text("Started a subagent; not waiting for it.");
                }
                if turn.input.contains(", now on its own turn]") {
                    answers
                        .lock()
                        .unwrap()
                        .insert("own turn".into(), json!(true));
                    return Reply::text("4.");
                }
                for id in to_judge(&turn.input) {
                    brigadier(
                        &turn,
                        CliRequest::Queue {
                            id,
                            judgment: Judgment::Wait,
                        },
                    )
                    .await;
                }
                Reply::text("Noted.")
            })
        })
    };
    let flow = Flow::start("block-subagent-wait", Options::default(), script).await;
    flow.say("Start a subagent in the background.").await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the first turn to end", || conv.idle()).await;
    flow.say(FIRST).await;
    waits(&flow, FIRST).await;
    super::eventually_async("the judge turn to end", || conv.idle()).await;
    assert!(
        flow.manager.work_holds(&conv).await,
        "its subagent still runs"
    );
    assert!(!answers.lock().unwrap().contains_key("own turn"));

    let sender = events.lock().unwrap().clone().expect("the CLI's events");
    sender.send(spawn(ItemStatus::Completed)).await.unwrap();
    super::eventually("it to go on its own turn", || {
        answers.lock().unwrap().contains_key("own turn")
    })
    .await;
    flow.settled().await;
    assert!(flow.board().await.queue.items.is_empty());
    flow.stop().await;
}
