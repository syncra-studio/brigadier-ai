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

/// After the start: the request is interrupted, nothing runs, and Continue answers it.
async fn interrupted_then_continued(flow: &Flow, request: &str, seen: &Arc<Mutex<Vec<String>>>) {
    let board = flow
        .until("the request interrupted", |board| {
            board
                .requests
                .get(request)
                .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
        })
        .await;
    assert_ne!(board.run, RunState::Running);
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
