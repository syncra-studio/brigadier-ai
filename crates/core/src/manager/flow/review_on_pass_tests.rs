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
    flow.background_commands();
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
    flow.background_commands();
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

/// Checks that overlap: one that ends after a newer one started changes nothing, so an older
/// pass neither lets a review start while the newer check runs nor hides its failure.
#[tokio::test]
async fn an_older_check_ending_late_leaves_the_newer_ones_state() {
    let flow = Flow::start(
        "review-on-pass-overlap",
        Options::default(),
        thread(&[], Log::default()),
    )
    .await;
    let id = flow.conversation.clone();
    let older = flow.manager.thread_check_started(&id, None).await;
    let newer = flow.manager.thread_check_started(&id, None).await;
    older.ended(true);
    assert_eq!(flow.manager.thread_check_state(&id), Some("running"));
    newer.ended(false);
    assert_eq!(flow.manager.thread_check_state(&id), Some("failed"));
    // One stopped before it ended doesn't clear a newer one either.
    let older = flow.manager.thread_check_started(&id, None).await;
    let newer = flow.manager.thread_check_started(&id, None).await;
    drop(older);
    assert_eq!(flow.manager.thread_check_state(&id), Some("running"));
    drop(newer);
    assert_eq!(flow.manager.thread_check_state(&id), None);
    flow.stop().await;
}

/// A reviewer that answers after `delay` with one finding.
fn reviewer(delay: std::time::Duration) -> Script {
    script(move |_| async move {
        tokio::time::sleep(delay).await;
        Reply::text("Full review comments:\n\n- [P2] a.rs has no header — a.rs:1\n  Add one.")
    })
}

/// A review that ends within the call's foreground minute comes in the passing check's reply,
/// and no message of it follows: the thread fixes or answers in the same turn, with no
/// `[quiet]` turn to wait for it (#193). The test's call never goes to the background, so it
/// waits for the review however late a loaded machine starts it: here one that takes the
/// whole test-sized foreground window (#442).
#[tokio::test]
async fn a_review_that_ends_within_the_minute_comes_in_the_checks_reply() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "review-on-pass-held",
        Options {
            reviews: Some(reviewer(crate::manager::background_runs::FOREGROUND)),
            ..Options::default()
        },
        thread(&["true"], seen.clone()),
    )
    .await;
    flow.wait_for_reviews();
    flow.say("Build it.").await;
    let board = done(&flow).await;
    let seen = seen.lock().unwrap().clone();
    assert!(!seen[0].contains("[review running]"), "{}", seen[0]);
    assert!(
        seen[0].contains("[review of your commits · Codex found 1 in"),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("a.rs has no header"), "{}", seen[0]);
    assert!(
        seen[1..]
            .iter()
            .all(|input| !input.contains("[review of your commits")),
        "{seen:#?}"
    );
    assert_eq!(board.reviews.len(), 1, "{:#?}", board.reviews);
    flow.stop().await;
}

/// A review still running when the call's foreground minute is nearly up: the call answers
/// then, before it would go to the background, with the line to wait, and the findings come
/// as a message (#193).
#[tokio::test]
async fn a_review_that_outlasts_the_minute_comes_as_a_message() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "review-on-pass-long",
        Options {
            reviews: Some(reviewer(
                crate::manager::background_runs::FOREGROUND + std::time::Duration::from_secs(1),
            )),
            ..Options::default()
        },
        thread(&["true"], seen.clone()),
    )
    .await;
    flow.wait_for_reviews();
    flow.background_commands();
    flow.say("Build it.").await;
    let board = done(&flow).await;
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains("[review running]"), "{}", seen[0]);
    assert!(
        !seen[0].contains("goes on in the background"),
        "{}",
        seen[0]
    );
    assert!(
        seen.iter()
            .any(|input| input.contains("[review of your commits · Codex found 1 in")),
        "{seen:#?}"
    );
    assert!(
        board.background_runs.is_empty(),
        "{:#?}",
        board.background_runs
    );
    flow.stop().await;
}
