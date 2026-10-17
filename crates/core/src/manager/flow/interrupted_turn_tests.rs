//! A quit or a crash in the middle of the thread's turn (UI audit H1): the request reads
//! interrupted, never as answered, and Continue picks it up with a word to check what already
//! happened.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Flow, Options, Reply, Script};
use crate::work::{RequestState, RunState};

/// A thread whose turn for "Hold." never ends, and that answers anything else at once; every
/// input it was given is kept in `seen`.
fn holding(seen: Arc<Mutex<Vec<String>>>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.lock().unwrap().push(turn.input.clone());
            if turn.input.contains("Hold.") {
                std::future::pending::<()>().await;
            }
            Reply::text("Checked git log: the test is in. Done.")
        })
    })
}

/// Starts a held turn and waits until the thread is in it.
async fn held_turn(flow: &Flow, seen: &Arc<Mutex<Vec<String>>>) -> String {
    flow.say("Hold.").await;
    super::eventually("the thread's turn", || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("Hold."))
    })
    .await;
    let board = flow
        .until("the turn running", |board| board.run == RunState::Running)
        .await;
    board.latest_request().expect("a request").id.clone()
}

/// After the start: the request is interrupted, the run reads failed in its name, and
/// Continue answers it.
async fn interrupted_then_continued(flow: &Flow, request: &str, seen: &Arc<Mutex<Vec<String>>>) {
    flow.until("the request interrupted", |board| {
        board
            .requests
            .get(request)
            .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
    })
    .await;
    let board = flow
        .until("the run failed", |board| board.run == RunState::Failed)
        .await;
    assert_eq!(board.run_request.as_deref(), Some(request));
    // It stays that way: nothing settles it as answered meanwhile.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let board = flow.board().await;
    assert!(
        super::super::conversation::interrupted_by_quit(&board.requests[request].state),
        "{:?}",
        board.requests[request].state
    );
    flow.manager
        .resume(flow.conversation.clone())
        .await
        .unwrap();
    let board = flow
        .until("the request answered", |board| {
            board
                .requests
                .get(request)
                .is_some_and(|r| r.state == RequestState::Done)
        })
        .await;
    assert_eq!(board.latest_request().expect("a request").id, request);
    let inputs = seen.lock().unwrap().clone();
    assert!(
        inputs.last().is_some_and(
            |input| input.contains("Brigadier quit while you were working")
                && input.contains("git status")
        ),
        "{inputs:#?}"
    );
}

#[tokio::test]
async fn a_quit_mid_turn_leaves_the_request_interrupted_and_continue_picks_it_up() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-quit",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.restart().await;
    interrupted_then_continued(&flow, &request, &seen).await;
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_mid_turn_leaves_the_request_interrupted_and_continue_picks_it_up() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-crash",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.crash().await;
    interrupted_then_continued(&flow, &request, &seen).await;
    flow.stop().await;
}

/// A quit while the thread's turn is still starting: the request reads interrupted, its time
/// leaves out the time Brigadier was closed, and Continue sends the message the thread never
/// got (#81).
#[tokio::test]
async fn a_quit_while_the_turn_starts_leaves_it_interrupted_without_the_gap() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-starting",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release));
    flow.say("Add a test.").await;
    reached.notified().await;
    let request = flow
        .board()
        .await
        .latest_request()
        .expect("a request")
        .id
        .clone();
    let quit_at = crate::now_ms();
    // The old start never goes on: that daemon is gone.
    flow.restart_after(Duration::from_secs(2)).await;
    let board = flow
        .until("the request interrupted", |board| {
            board
                .requests
                .get(&request)
                .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
        })
        .await;
    assert_eq!(
        board.requests[&request].state,
        RequestState::Failed {
            error: super::super::conversation::INTERRUPTED_BEFORE_START.into()
        }
    );
    let worked = &board.requests[&request].worked;
    assert!(
        worked
            .iter()
            .all(|span| span.to_ms.is_some_and(|to| to <= quit_at)),
        "{worked:?} counts the time Brigadier was closed (quit at {quit_at})"
    );
    flow.until("the run failed", |board| board.run == RunState::Failed)
        .await;
    assert!(seen.lock().unwrap().is_empty());
    flow.manager
        .resume(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request answered", |board| {
        board
            .requests
            .get(&request)
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
    let inputs = seen.lock().unwrap().clone();
    assert!(
        inputs
            .last()
            .is_some_and(|input| input.contains("Add a test.")),
        "{inputs:#?}"
    );
    flow.stop().await;
}

/// A crash mid-turn, then another while Continue's turn starts: the thread had the message
/// already, so the next Continue carries on from where it stands and sends nothing again.
#[tokio::test]
async fn a_crash_while_continue_starts_continues_and_sends_nothing_again() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-continue-starting",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.crash().await;
    flow.until("the request interrupted", |board| {
        board
            .requests
            .get(&request)
            .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
    })
    .await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release));
    flow.manager
        .resume(flow.conversation.clone())
        .await
        .unwrap();
    reached.notified().await;
    flow.crash().await;
    interrupted_then_continued(&flow, &request, &seen).await;
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains("Hold."))
            .count(),
        1
    );
    flow.stop().await;
}

/// A crash after the CLI took the message but before its turn was seen starting: Continue
/// sends it again, with a word to check what already happened.
#[tokio::test]
async fn a_crash_after_the_send_sends_it_again_with_a_word_to_check() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-after-send",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    // A first answer: the CLI is up and its session known.
    flow.say("Say hi.").await;
    flow.until("the first request answered", |board| {
        board
            .latest_request()
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_send.lock().unwrap() = Some((reached.clone(), release));
    flow.say("Add a test.").await;
    reached.notified().await;
    let request = flow
        .board()
        .await
        .latest_request()
        .expect("a request")
        .id
        .clone();
    flow.crash().await;
    flow.until("the request interrupted", |board| {
        board
            .requests
            .get(&request)
            .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
    })
    .await;
    flow.manager
        .resume(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request answered", |board| {
        board
            .requests
            .get(&request)
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
    let inputs = seen.lock().unwrap().clone();
    assert!(
        inputs
            .last()
            .is_some_and(|input| input.contains("Add a test.")
                && input.contains("check git status and git log")),
        "{inputs:#?}"
    );
    flow.stop().await;
}
