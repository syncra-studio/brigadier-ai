//! A review of the thread's own changes starts only on a passing check (BUILD-PLAN C10): not
//! while its `run_check` runs, nor on the files its newest check failed on, whether the check
//! ends inside the call or in the background, and the end of a turn doesn't start one either.
//! The pass that follows starts it, and says so.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::board::Board;
use crate::work::RequestState;

type Log = Arc<Mutex<Vec<String>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Commits three files of 60 lines each in the thread's workspace: over the review line.
fn commit_over_the_line(turn: &Turn) {
    let workspace = &turn.add_dirs[0];
    for file in ["a.rs", "b.rs", "c.rs"] {
        let text: String = (1..=60).map(|n| format!("// {file} {n}\n")).collect();
        std::fs::write(workspace.join(file), text).unwrap();
    }
    git(workspace, &["add", "-A"]);
    git(workspace, &["commit", "-q", "-m", "Add three modules"]);
}

/// A thread that commits work over the line and runs `checks` in order, each reply going
/// into `seen` with every later turn's input; it answers once no review is said to run.
fn thread(checks: &'static [&'static str], seen: Log) -> Script {
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            if turn.input.contains("Build it.") {
                commit_over_the_line(&turn);
                let mut waits = false;
                for command in checks {
                    let check = turn.call("run_check", json!({ "command": command })).await;
                    waits = check.text.contains("[review running]")
                        || check.text.contains("goes on in the background");
                    seen.lock().unwrap().push(check.text);
                }
                return Reply::text(if waits { "[quiet]" } else { "Built it." });
            }
            seen.lock().unwrap().push(turn.input.clone());
            Reply::text(if turn.input.contains("[review running]") {
                "[quiet]"
            } else {
                "Built it."
            })
        }
    })
}

async fn done(flow: &Flow) -> Board {
    flow.until("the request is done", |board| {
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await
}

/// A failing check over the line starts no review; the passing one after it, on the same
/// files, starts it and says so.
#[tokio::test]
async fn a_failing_check_starts_no_review_and_the_pass_after_it_does() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "review-on-pass-now",
        Options::default(),
        thread(&["false", "true"], seen.clone()),
    )
    .await;
    flow.say("Build it.").await;
    let board = done(&flow).await;
    let seen = seen.lock().unwrap().clone();
    assert!(!seen[0].contains("[review running]"), "{}", seen[0]);
    assert!(seen[1].contains("[review running]"), "{}", seen[1]);
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// A check that goes to the background holds the review while it runs, the turn's end
/// included; its pass starts the review, said in the message it sends.
#[tokio::test]
async fn a_background_pass_starts_the_review_it_held() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "review-on-pass-background",
        Options::default(),
        thread(&["sleep 4; true"], seen.clone()),
    )
    .await;
    flow.say("Build it.").await;
    let board = done(&flow).await;
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains("goes on in the background"), "{}", seen[0]);
    let finished = seen
        .iter()
        .find(|input| input.contains("[run-1 finished"))
        .expect("the check's result");
    assert!(finished.contains("[review running]"), "{finished}");
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// A turn that ends while its check runs in the background starts no review then, and the
/// check's failure starts none: the work over the line stays unreviewed while it fails.
#[tokio::test]
async fn a_turn_ending_on_a_pending_check_that_fails_starts_no_review() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "review-on-pass-failing",
        Options::default(),
        thread(&["sleep 4; false"], seen.clone()),
    )
    .await;
    flow.say("Build it.").await;
    let board = done(&flow).await;
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains("goes on in the background"), "{}", seen[0]);
    let finished = seen
        .iter()
        .find(|input| input.contains("[run-1 finished"))
        .expect("the check's result");
    assert!(!finished.contains("[review running]"), "{finished}");
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    flow.stop().await;
}
