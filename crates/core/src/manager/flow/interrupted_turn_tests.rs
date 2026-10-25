//! A quit or a crash in the middle of the thread's turn (UI audit H1): the request is never
//! read as answered. The turn goes on by itself once at the next start, with a word to check
//! what already happened (D9 a); cut off again, or should that turn not begin, the request reads
//! interrupted and Continue picks it up.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_store::StreamPage;

use super::{Flow, Options, Reply, Script};
use crate::model::streams;
use crate::work::{RequestState, RunState};

/// A thread whose turn for "Hold." never ends, nor (while `hold_resumes` is set) one that goes
/// on after a quit, and that answers anything else at once; every input it was given is kept in
/// `seen`.
fn holding_resumes(seen: Arc<Mutex<Vec<String>>>, hold_resumes: Arc<AtomicBool>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        let hold_resumes = hold_resumes.clone();
        Box::pin(async move {
            seen.lock().unwrap().push(turn.input.clone());
            if turn.input.contains("Hold.")
                || (turn.input.contains("Brigadier quit") && hold_resumes.load(Ordering::SeqCst))
            {
                std::future::pending::<()>().await;
            }
            Reply::text("Checked git log: the test is in. Done.")
        })
    })
}

fn holding(seen: Arc<Mutex<Vec<String>>>) -> Script {
    holding_resumes(seen, Arc::default())
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

fn interrupted(board: &crate::board::Board, request: &str) -> bool {
    board
        .requests
        .get(request)
        .is_some_and(|r| super::super::conversation::interrupted_by_quit(&r.state))
}

async fn answered(flow: &Flow, request: &str) {
    flow.until("the request answered", |board| {
        board
            .requests
            .get(request)
            .is_some_and(|r| r.state == RequestState::Done)
    })
    .await;
}

/// How often the conversation's turns went on by themselves, as stored.
async fn auto_resumes(flow: &Flow) -> usize {
    flow.core
        .store()
        .read_stream(
            streams::conversation(&flow.conversation),
            StreamPage {
                kinds: vec!["turn.auto_resumed".into()],
                limit: 100,
                ..StreamPage::default()
            },
        )
        .await
        .unwrap()
        .len()
}

/// After the start the request goes on by itself, in its own block, and is answered: the turn
/// was told to check git status and git log first.
async fn went_on_by_itself(flow: &Flow, request: &str, seen: &Arc<Mutex<Vec<String>>>) {
    answered(flow, request).await;
    let board = flow.board().await;
    assert_eq!(board.latest_request().expect("a request").id, request);
    let inputs = seen.lock().unwrap().clone();
    assert!(
        inputs.last().is_some_and(
            |input| input.contains("Brigadier quit while you were working")
                && input.contains("git status")
        ),
        "{inputs:#?}"
    );
    assert_eq!(auto_resumes(flow).await, 1);
}

/// After the start: the request is interrupted, the run reads failed in its name, nothing goes
/// on by itself, and Continue answers it.
async fn interrupted_then_continued(flow: &Flow, request: &str, seen: &Arc<Mutex<Vec<String>>>) {
    flow.until("the request interrupted", |board| {
        interrupted(board, request)
    })
    .await;
    let board = flow
        .until("the run failed", |board| board.run == RunState::Failed)
        .await;
    assert_eq!(board.run_request.as_deref(), Some(request));
    // It stays that way: nothing settles it as answered meanwhile.
    let given = seen.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let board = flow.board().await;
    assert!(
        interrupted(&board, request),
        "{:?}",
        board.requests[request].state
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        given,
        "a turn went on by itself"
    );
    flow.manager
        .resume(flow.conversation.clone())
        .await
        .unwrap();
    answered(flow, request).await;
    let board = flow.board().await;
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
async fn a_quit_mid_turn_goes_on_by_itself_once() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-quit",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.restart().await;
    went_on_by_itself(&flow, &request, &seen).await;
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_mid_turn_goes_on_by_itself_once() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-crash",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.crash().await;
    went_on_by_itself(&flow, &request, &seen).await;
    flow.stop().await;
}

/// Cut off again while it went on by itself (a crash, then a quit): the request waits for the
/// user's Continue, never going on by itself twice.
#[tokio::test]
async fn cut_off_again_it_waits_for_continue() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let hold_resumes = Arc::new(AtomicBool::new(true));
    let mut flow = Flow::start(
        "interrupted-twice",
        Options::default(),
        holding_resumes(seen.clone(), hold_resumes.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.crash().await;
    super::eventually("the turn going on by itself", || {
        seen.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("Brigadier quit while you were working"))
    })
    .await;
    flow.until("its turn running", |board| board.run == RunState::Running)
        .await;
    flow.restart().await;
    hold_resumes.store(false, Ordering::SeqCst);
    interrupted_then_continued(&flow, &request, &seen).await;
    assert_eq!(auto_resumes(&flow).await, 1);
    flow.stop().await;
}

/// A request the user stopped is theirs to resume: a restart leaves it stopped.
#[tokio::test]
async fn a_stopped_request_does_not_go_on_by_itself() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let given = seen.clone();
    let mut flow = Flow::start(
        "interrupted-stopped",
        Options::default(),
        Arc::new(move |turn| {
            let seen = given.clone();
            Box::pin(async move {
                seen.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Hold.") {
                    turn.stopped().await;
                }
                Reply::text("Done.")
            })
        }),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("the request stopped", |board| {
        board.requests[&request].state == RequestState::Stopped
    })
    .await;
    flow.restart().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let board = flow.board().await;
    assert_eq!(board.requests[&request].state, RequestState::Stopped);
    assert_eq!(seen.lock().unwrap().len(), 1, "{:#?}", seen.lock().unwrap());
    assert_eq!(auto_resumes(&flow).await, 0);
    flow.stop().await;
}

/// Its CLI won't start when the cut-off turn goes on by itself, after the resume itself was
/// taken: the request reads interrupted again, and Continue (once the CLI starts) picks it up,
/// sending the word once.
#[tokio::test]
async fn a_turn_that_cannot_go_on_by_itself_waits_for_continue() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-no-start",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    flow.behavior.fail_starts.store(true, Ordering::SeqCst);
    flow.crash().await;
    // The resume was taken, and its turn failed to start: still interrupted, not failed so.
    super::eventually("a start that failed", || {
        flow.behavior.failed_starts.load(Ordering::SeqCst) > 0
    })
    .await;
    let board = flow
        .until("the request interrupted", |board| {
            interrupted(board, &request) && board.run == RunState::Failed
        })
        .await;
    assert_eq!(
        board.requests[&request].state,
        RequestState::Failed {
            error: super::super::conversation::INTERRUPTED_BY_QUIT.into()
        }
    );
    assert_eq!(auto_resumes(&flow).await, 1);
    assert_eq!(seen.lock().unwrap().len(), 1);
    flow.behavior.fail_starts.store(false, Ordering::SeqCst);
    interrupted_then_continued(&flow, &request, &seen).await;
    assert_eq!(
        seen.lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains("Brigadier quit while you were working"))
            .count(),
        1
    );
    flow.stop().await;
}

/// A quit while the thread's turn is still starting, before its CLI session existed: the
/// request's time leaves out the time Brigadier was closed, and the message the thread never
/// got goes by itself (#81).
#[tokio::test]
async fn a_quit_while_the_turn_starts_sends_it_by_itself_without_the_gap() {
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
    answered(&flow, &request).await;
    let board = flow.board().await;
    let worked = &board.requests[&request].worked;
    assert!(
        worked
            .iter()
            .filter(|span| span.from_ms < quit_at)
            .all(|span| span.to_ms.is_some_and(|to| to <= quit_at)),
        "{worked:?} counts the time Brigadier was closed (quit at {quit_at})"
    );
    let inputs = seen.lock().unwrap().clone();
    assert_eq!(inputs.len(), 1, "{inputs:#?}");
    assert!(inputs[0].contains("Add a test."), "{inputs:#?}");
    assert_eq!(auto_resumes(&flow).await, 1);
    flow.stop().await;
}

/// A crash mid-turn, then another while the turn that goes on by itself starts: the thread had
/// the message already, so Continue carries on from where it stands and sends nothing again.
#[tokio::test]
async fn a_crash_while_it_goes_on_continues_and_sends_nothing_again() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let mut flow = Flow::start(
        "interrupted-continue-starting",
        Options::default(),
        holding(seen.clone()),
    )
    .await;
    let request = held_turn(&flow, &seen).await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release));
    flow.crash().await;
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
    assert_eq!(auto_resumes(&flow).await, 1);
    flow.stop().await;
}

/// A crash after the CLI took the message but before its turn was seen starting, its session
/// known: it goes again by itself, with a word to check what already happened.
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
    answered(&flow, &request).await;
    let inputs = seen.lock().unwrap().clone();
    assert!(
        inputs
            .last()
            .is_some_and(|input| input.contains("Add a test.")
                && input.contains("check git status and git log")),
        "{inputs:#?}"
    );
    assert_eq!(auto_resumes(&flow).await, 1);
    flow.stop().await;
}
