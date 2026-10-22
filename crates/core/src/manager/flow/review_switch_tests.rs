//! Settings' "Review the work" switch: off, no review starts and nothing waits for one, for
//! sessions already running too.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::super::review_runs::WAITING_FOR_REVIEW;
use super::{Flow, Options, Reply, Script, Turn};
use crate::work::TaskState;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A reviewer that must never be asked.
fn no_reviews() -> Option<Script> {
    Some(script(|turn| async move {
        panic!("no review runs while reviews are off: {}", turn.input)
    }))
}

async fn set_reviews(flow: &Flow, on: bool) {
    let mut settings = flow.core.settings();
    settings.review_work = on;
    flow.core.update_settings(settings).await.unwrap();
}

/// The task numbers of the reports in an orchestrator's input, in order.
fn reports_in(input: &str) -> Vec<u32> {
    input
        .split("[report task-")
        .skip(1)
        .filter_map(|rest| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .collect()
}

/// The orchestrator's part in these sessions: delegates one implement task, then lands it on
/// its report.
async fn lead_and_land(turn: &Turn) -> Reply {
    if let Some(n) = reports_in(&turn.input).first() {
        let reply = turn
            .call("land_phase", json!({"task": format!("task-{n}")}))
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        return Reply::text("Added the greeting.");
    }
    if turn.input.contains("[review ") {
        return Reply::text("");
    }
    let reply = turn
        .call(
            "delegate_task",
            json!({"effort": "medium", "title": "Add a greeting", "kind": "implement",
                   "spec": "Create hello.txt.", "provider": "claude"}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("")
}

/// Reviews off: the thread and the lead are told so, the lead's request_review and the thread's
/// review_plan start none, the landing gets none, and the lead never waits for one.
#[tokio::test]
async fn with_reviews_off_nothing_is_reviewed_and_nothing_waits() {
    let flow = Flow::start(
        "reviews-off",
        Options {
            reviews: no_reviews(),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                assert!(
                    turn.prompt.contains(crate::manager::prompts::REVIEWS_OFF),
                    "the thread starts knowing reviews are off"
                );
                if turn.input.contains("Add a greeting") {
                    let plan = turn
                        .call(
                            "review_plan",
                            json!({"plan": "1. Create hello.txt.", "brief": "Add a greeting."}),
                        )
                        .await;
                    assert!(!plan.is_error, "{}", plan.text);
                    assert!(plan.text.starts_with("Reviews are off"), "{}", plan.text);
                }
                return lead_and_land(&turn).await;
            }
            assert!(
                turn.input.contains("don't call request_review"),
                "the lead's brief has no review: {}",
                turn.input
            );
            turn.write("hello.txt", "hello\n");
            turn.git(&["add", "-A"]);
            turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
            let review = turn.call("request_review", json!({})).await;
            assert!(!review.is_error, "{}", review.text);
            assert!(
                review.text.starts_with("Reviews are off"),
                "{}",
                review.text
            );
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    set_reviews(&flow, false).await;
    flow.say("Add a greeting file and merge it.").await;
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Landed);
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    flow.stop().await;
}

/// A lead that asked for its review ended its turn to wait for the findings. The user turns
/// reviews off: it goes on at once, hears that nothing waits on its review, and reports; the
/// thread hears the switch in a note before its next turn. The review still running may
/// finish later, and its findings still arrive.
#[tokio::test]
async fn switching_reviews_off_releases_a_worker_waiting_for_its_review() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = heard.clone();
    let flow = Flow::start(
        "reviews-switched",
        Options {
            reviews: Some(script(move |_| {
                let held = held.clone();
                async move {
                    held.notified().await;
                    Reply::text("- [P3] hello.txt could say more — hello.txt:1\n  Say more.")
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.is_orchestrator() {
                    // Only a verifier waits for its own review: the lead's work gets one.
                    if reports_in(&turn.input).first() == Some(&1) {
                        let reply = turn.call("start_verifier", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
                    return lead_and_land(&turn).await;
                }
                if !turn.prompt.contains("You verify this phase") {
                    turn.write("hello.txt", "hello\n");
                    turn.git(&["add", "-A"]);
                    turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains("Nothing waits on your review now") {
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Verified hello.txt.", "changes": ["hello.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                let started = turn.call("request_review", json!({})).await;
                assert!(
                    started.text.starts_with("Started a review"),
                    "{}",
                    started.text
                );
                Reply::text("Waiting for the review.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    flow.until("the verifier to wait for its review", |board| {
        board
            .tasks
            .values()
            .any(|task| task.blocked_reason.as_deref() == Some(WAITING_FOR_REVIEW))
    })
    .await;
    set_reviews(&flow, false).await;
    flow.manager.reviews_switched(false).await;
    let board = flow
        .until("the lead to land", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Landed)
        })
        .await;
    assert_eq!(board.reviews.len(), 1, "the landing starts no review");
    let inputs = heard.lock().unwrap().clone();
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("Nothing waits on your review now")),
        "the lead heard it may go on: {inputs:#?}"
    );
    // The review that was running finishes all the same, and nothing waited on it.
    release.notify_one();
    flow.until("the review to end", |board| {
        board
            .reviews
            .values()
            .all(|review| review.state != crate::work::ReviewState::Running)
    })
    .await;
    flow.settled().await;
    flow.stop().await;
}

/// The user turns reviews off just as a lead ends its turn to wait for its review: the end of
/// the turn waits for the switch (the lock both take), then sees reviews off. The lead is never
/// left waiting: it is asked to report, hears why, and does.
#[tokio::test]
async fn switching_reviews_off_as_a_worker_starts_to_wait_never_leaves_it_waiting() {
    let asked = Arc::new(tokio::sync::Notify::new());
    let ended = asked.clone();
    let go = Arc::new(tokio::sync::Notify::new());
    let held = go.clone();
    let turns = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let flow = Flow::start(
        "reviews-race-wait",
        Options {
            // The review never ends in this test.
            reviews: Some(script(|_| std::future::pending())),
            ..Options::default()
        },
        script(move |turn| {
            let (ended, held, turns) = (ended.clone(), held.clone(), turns.clone());
            async move {
                if turn.is_orchestrator() {
                    return lead_and_land(&turn).await;
                }
                // Its next turn (asked to report, or the note) reports.
                if turns.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                turn.write("hello.txt", "hello\n");
                turn.git(&["add", "-A"]);
                turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
                let started = turn.call("request_review", json!({})).await;
                assert!(
                    started.text.starts_with("Started a review"),
                    "{}",
                    started.text
                );
                ended.notify_one();
                held.notified().await;
                Reply::text("Waiting for the review.")
            }
        }),
    )
    .await;
    flow.say("Add a greeting file and merge it.").await;
    asked.notified().await;
    let switch = flow.manager.reviews.lock().await;
    go.notify_one();
    // The turn's end now waits for the lock.
    tokio::time::sleep(Duration::from_millis(300)).await;
    set_reviews(&flow, false).await;
    drop(switch);
    flow.manager.reviews_switched(false).await;
    let board = flow
        .until("the lead to land", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Landed)
        })
        .await;
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.blocked_reason.as_deref() != Some(WAITING_FOR_REVIEW)),
        "nobody waits for the review"
    );
    let events = flow.events().await;
    assert!(
        !format!("{events:?}").contains(WAITING_FOR_REVIEW),
        "the lead never waited for its review"
    );
    flow.stop().await;
}

/// The user turns reviews off while a review is about to start (the lead's request_review waits
/// for the lock the switch holds): it doesn't start, and the lead hears reviews are off.
#[tokio::test]
async fn switching_reviews_off_while_a_review_starts_stops_it_starting() {
    let asking = Arc::new(tokio::sync::Notify::new());
    let about = asking.clone();
    let flow = Flow::start(
        "reviews-race-start",
        Options {
            reviews: no_reviews(),
            ..Options::default()
        },
        script(move |turn| {
            let about = about.clone();
            async move {
                if turn.is_orchestrator() {
                    return lead_and_land(&turn).await;
                }
                turn.write("hello.txt", "hello\n");
                turn.git(&["add", "-A"]);
                turn.git(&["commit", "-q", "-m", "Add hello.txt"]);
                about.notify_one();
                let review = turn.call("request_review", json!({})).await;
                assert!(!review.is_error, "{}", review.text);
                assert!(
                    review.text.starts_with("Reviews are off"),
                    "{}",
                    review.text
                );
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    let switch = flow.manager.reviews.lock().await;
    flow.say("Add a greeting file and merge it.").await;
    asking.notified().await;
    // request_review now waits for the lock to start its review.
    tokio::time::sleep(Duration::from_millis(300)).await;
    set_reviews(&flow, false).await;
    drop(switch);
    flow.manager.reviews_switched(false).await;
    let board = flow.settled().await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Landed);
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    flow.stop().await;
}
