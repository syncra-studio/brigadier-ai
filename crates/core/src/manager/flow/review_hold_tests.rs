//! One review per change, and its open findings back to the worker that did it
//! (SESSION-FIXES-PLAN.md Q6.1): a landed worker's session and branch are kept until its
//! review's findings settle, its fix resumes that session in its old worktree, and a fix's
//! landing gets no review, only one re-check when a finding is marked high.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn, git};
use crate::manager::review_hold::hold_owner;
use crate::work::{FindingStatus, ReviewState, Task, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const TWO: &str = "Full review comments:\n\n- [P1] Old notes no longer load — NOTES.md:1\n  Keep the old format.\n- [P3] Trailing space — NOTES.md:1\n  Trim it.";

/// What the scripted worker of task-1 and its fix saw.
#[derive(Default)]
struct Seen {
    /// Task-1's CLI session and worktree.
    first: Option<(String, std::path::PathBuf)>,
    /// The fix's CLI session, worktree, first input, and its worktree's HEAD then.
    fix: Option<(String, std::path::PathBuf, String, String)>,
    /// The reviewers' inputs, oldest first.
    reviews: Vec<String>,
    /// Whether task-1's session files and branch were still kept when the review came.
    held_at_review: Option<bool>,
}

fn branch_exists(flow: &Flow, branch: &str) -> bool {
    !git(&flow.repo, &["branch", "--list", branch])
        .trim()
        .is_empty()
}

/// The session: a worker writes NOTES.md and lands; the review finds two things; the thread
/// records both open (the first `high` when `high`) and sends them back as one fix.
async fn fixed_once(name: &str, high: bool, refuse_resume: bool) -> (Flow, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let (log, reviews_log) = (seen.clone(), seen.clone());
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            reviews: Some(script(move |turn| {
                let log = reviews_log.clone();
                async move {
                    log.lock().unwrap().reviews.push(turn.input.clone());
                    if turn.input.contains("Re-check a fix") {
                        Reply::text("No findings.")
                    } else {
                        Reply::text(TWO)
                    }
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add notes.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Write the notes",
                                       "kind": "implement", "spec": "Write NOTES.md.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if let Some(n) = ["1", "2"]
                        .into_iter()
                        .find(|n| turn.input.contains(&format!("[report task-{n}")))
                    {
                        let reply = turn
                            .call("land_phase", json!({"task": format!("task-{n}")}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Landed.");
                    }
                    if turn.input.contains("[review task-1 · ") {
                        let kept = turn.host.runtime.ledger().owners().iter().any(
                            |(owner, artifacts, _)| {
                                owner.starts_with("review-hold:") && !artifacts.is_empty()
                            },
                        );
                        log.lock().unwrap().held_at_review = Some(kept);
                        let reply = turn
                            .call(
                                "record_findings",
                                json!({"items": [
                                    {"id": "R1.1", "status": "open", "high": high},
                                    {"id": "R1.2", "status": "open"},
                                ]}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        // Another provider asked for: the fix stays on its worker's model.
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Fix the notes",
                                       "kind": "implement", "role": "fix", "subject": "task-1",
                                       "provider": "codex",
                                       "spec": "Fix R1.1 (old notes load) and R1.2 (trailing space)."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        // One fix at a time resumes a session: a second is no error, and
                        // the thread is told where the first stands (#251).
                        let again = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Fix it again",
                                       "kind": "implement", "role": "fix", "subject": "task-1",
                                       "spec": "Fix R1.1."}),
                            )
                            .await;
                        assert!(!again.is_error, "{}", again.text);
                        assert!(
                            again.text.contains("task-2 fixes task-1's findings already (")
                                && again.text.contains("so no new task started"),
                            "{}",
                            again.text
                        );
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Noted.");
                }
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.input.contains("[workspace]") {
                    let head = turn.git(&["rev-parse", "HEAD"]);
                    log.lock().unwrap().fix = Some((
                        turn.native_id.clone(),
                        turn.cwd.clone(),
                        turn.input.clone(),
                        head,
                    ));
                    turn.write("NOTES.md", "notes v1\n");
                    turn.git(&["commit", "-qam", "Fix the notes"]);
                    // A resumed lead's old instructions ask for a review: a fix gets none.
                    let review = turn.call("request_review", json!({})).await;
                    assert!(review.is_error, "{}", review.text);
                    assert!(
                        review.text.contains("gets no review of its own"),
                        "{}",
                        review.text
                    );
                    let reply = turn
                        .call("submit_report", json!({"summary": "Fixed both."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains("task-1's, which landed") {
                    // The fallback: a fresh session, told what the landed worker reported.
                    log.lock().unwrap().fix = Some((
                        turn.native_id.clone(),
                        turn.cwd.clone(),
                        turn.input.clone(),
                        turn.git(&["rev-parse", "HEAD"]),
                    ));
                    turn.write("NOTES.md", "notes v1\n");
                    turn.git(&["commit", "-qam", "Fix the notes"]);
                    // A resumed lead's old instructions ask for a review: a fix gets none.
                    let review = turn.call("request_review", json!({})).await;
                    assert!(review.is_error, "{}", review.text);
                    assert!(
                        review.text.contains("gets no review of its own"),
                        "{}",
                        review.text
                    );
                    let reply = turn
                        .call("submit_report", json!({"summary": "Fixed both."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                log.lock().unwrap().first = Some((turn.native_id.clone(), turn.cwd.clone()));
                if refuse_resume {
                    super::REFUSED_RESUMES
                        .lock()
                        .unwrap()
                        .push(turn.native_id.clone());
                }
                turn.write("NOTES.md", "notes \n");
                turn.git(&["add", "NOTES.md"]);
                turn.git(&["commit", "-qm", "Write the notes"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Wrote NOTES.md.", "changes": ["NOTES.md"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.until("the fix to land", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 2 && task.state == TaskState::Landed)
    })
    .await;
    (flow, seen)
}

/// Task-1, once its batch settled: its hold and branch are gone.
async fn settled(flow: &Flow) -> Task {
    let board = flow
        .until("task-1's batch to settle", |board| {
            Flow::task(board, 1).review_hold.is_none()
        })
        .await;
    Flow::task(&board, 1).clone()
}

#[tokio::test]
async fn findings_go_back_to_the_resumed_worker_and_its_fix_is_rechecked_only_when_high() {
    for high in [true, false] {
        let (flow, seen) = fixed_once("review-hold", high, false).await;
        let first = settled(&flow).await;
        // The re-check starts once the fix's landing is recorded and cleaned up.
        if high {
            super::eventually("the re-check", || seen.lock().unwrap().reviews.len() == 2).await;
        }
        let board = flow.settled().await;
        let seen = std::mem::take(&mut *seen.lock().unwrap());
        assert_eq!(
            seen.held_at_review,
            Some(true),
            "kept until the findings came"
        );
        let (native, worktree) = seen.first.clone().unwrap();
        let (fix_native, fix_worktree, input, head) = seen.fix.clone().unwrap();
        // The fix resumed task-1's own session, in its worktree made again at the old path,
        // on the session's tip, and heard the findings as one message.
        assert_eq!(fix_native, native);
        assert_eq!(fix_worktree, worktree);
        assert_eq!(head, first.landed.clone().unwrap());
        assert!(input.starts_with("[workspace]"), "{input}");
        assert!(input.contains("Don't call request_review"), "{input}");
        // Its new task's folders, not the landed task's.
        let fix = Flow::task(&board, 2);
        let scratch = fix.workspace.as_ref().unwrap().scratch.clone();
        assert!(input.contains("this is a new one, task-2."), "{input}");
        assert!(
            input.contains(&format!("your outputs folder {scratch}/outputs")),
            "{input}"
        );
        let test_dir = crate::manager::workers::test_data_dir(&fix.id);
        assert!(
            input.contains(&format!("your test data folder {}.", test_dir.display())),
            "{input}"
        );
        assert!(
            input.contains("Fix R1.1 (old notes load) and R1.2 (trailing space)."),
            "{input}"
        );
        assert_eq!(fix.route.choice.provider, first.route.choice.provider);
        assert_eq!(fix.route.choice.model, first.route.choice.model);
        // No follow-up fix tasks, and the fix's landing got no review of its own: a re-check
        // of the high finding only, when there is one.
        assert_eq!(board.tasks.len(), 2, "{:#?}", board.tasks.keys());
        let rechecks: Vec<&String> = seen
            .reviews
            .iter()
            .filter(|input| input.contains("Re-check a fix"))
            .collect();
        if high {
            assert_eq!(seen.reviews.len(), 2, "{:#?}", seen.reviews);
            assert_eq!(rechecks.len(), 1);
            assert!(rechecks[0].contains("R1.1: [P1] Old notes no longer load"));
            assert!(!rechecks[0].contains("R1.2"));
        } else {
            assert_eq!(seen.reviews.len(), 1, "{:#?}", seen.reviews);
        }
        // The batch settled when the fix ended: the session files and branch are gone.
        let branch = first.workspace.as_ref().unwrap().branch.clone().unwrap();
        assert!(!branch_exists(&flow, &branch), "{branch}");
        assert!(
            flow.manager
                .runtime
                .ledger()
                .artifacts(&hold_owner(&first.id))
                .is_empty()
        );
        assert!(!fix_worktree.exists());
        flow.stop().await;
    }
}

#[tokio::test]
async fn a_fix_whose_session_cannot_be_resumed_starts_fresh_from_the_report() {
    let (flow, seen) = fixed_once("review-hold-fresh", false, true).await;
    settled(&flow).await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    let (native, _) = seen.first.clone().unwrap();
    let (fix_native, _, input, _) = seen.fix.clone().unwrap();
    assert_ne!(fix_native, native);
    assert!(
        input.contains("Its worker's report:\nWrote NOTES.md."),
        "{input}"
    );
    flow.stop().await;
}

/// A clean review, or one whose findings the thread records none open, sends nothing back:
/// the landed worker's session and branch go at once.
#[tokio::test]
async fn a_batch_with_nothing_open_settles_at_once() {
    for findings in [false, true] {
        let flow = Flow::start(
            "review-hold-clean",
            Options {
                behavior: Arc::new(FakeBehavior {
                    cleanup: true,
                    ..Default::default()
                }),
                reviews: Some(script(move |_| async move {
                    Reply::text(if findings { TWO } else { "No findings." })
                })),
                ..Options::default()
            },
            script(|turn| async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add notes.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Write the notes",
                                       "kind": "implement", "spec": "Write NOTES.md.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[report task-1") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Landed.");
                    }
                    if turn.input.contains("[review task-1 · ") {
                        let reply = turn
                            .call(
                                "record_findings",
                                json!({"items": [
                                    {"id": "R1.1", "status": "fixed"},
                                    {"id": "R1.2", "status": "dismissed", "why": "style"},
                                ]}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Recorded.");
                    }
                    return Reply::text("Noted.");
                }
                turn.write("NOTES.md", "notes\n");
                turn.git(&["add", "NOTES.md"]);
                turn.git(&["commit", "-qm", "Write the notes"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Wrote NOTES.md.", "changes": ["NOTES.md"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }),
        )
        .await;
        flow.say("Add notes.").await;
        let board = flow
            .until("the review to end", |board| {
                board
                    .reviews
                    .values()
                    .any(|review| review.state != ReviewState::Running)
            })
            .await;
        let first = settled(&flow).await;
        if findings {
            let board = flow.settled().await;
            let review = board.reviews.values().next().unwrap();
            assert!(
                review
                    .items
                    .iter()
                    .all(|item| item.status != FindingStatus::New)
            );
        } else {
            assert_eq!(
                board.reviews.values().next().unwrap().state,
                ReviewState::Clean
            );
        }
        let branch = first.workspace.as_ref().unwrap().branch.clone().unwrap();
        assert!(!branch_exists(&flow, &branch), "{branch}");
        assert!(
            flow.manager
                .runtime
                .ledger()
                .artifacts(&hold_owner(&first.id))
                .is_empty()
        );
        flow.stop().await;
    }
}

/// The session's archive settles a batch still open: the thread kept a finding open and sent
/// no fix.
#[tokio::test]
async fn archiving_the_session_settles_an_open_batch() {
    let flow = Flow::start(
        "review-hold-archive",
        Options {
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            reviews: Some(script(|_| async { Reply::text(TWO) })),
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Add notes.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write the notes",
                                   "kind": "implement", "spec": "Write NOTES.md.",
                                       "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Landed.");
                }
                if turn.input.contains("[review task-1 · ") {
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [
                                {"id": "R1.1", "status": "open"},
                                {"id": "R1.2", "status": "dismissed", "why": "style"},
                            ]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Left R1.1 for you.");
                }
                return Reply::text("Noted.");
            }
            turn.write("NOTES.md", "notes\n");
            turn.git(&["add", "NOTES.md"]);
            turn.git(&["commit", "-qm", "Write the notes"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Wrote NOTES.md.", "changes": ["NOTES.md"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the open finding", |board| {
            board.reviews.values().any(|review| {
                review
                    .items
                    .iter()
                    .any(|item| matches!(item.status, FindingStatus::Open { .. }))
            })
        })
        .await;
    let first = Flow::task(&board, 1).clone();
    assert!(first.review_hold.is_some());
    let branch = first.workspace.as_ref().unwrap().branch.clone().unwrap();
    assert!(branch_exists(&flow, &branch));
    flow.settled().await;
    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    flow.until("task-1's batch to settle", |board| {
        Flow::task(board, 1).review_hold.is_none()
    })
    .await;
    assert!(!branch_exists(&flow, &branch), "{branch}");
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&hold_owner(&first.id))
            .is_empty()
    );
    flow.stop().await;
}
