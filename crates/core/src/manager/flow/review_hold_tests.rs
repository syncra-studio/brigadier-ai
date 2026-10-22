//! A landed worker's session and branch are kept until a passing run_checks covers its landing
//! (SESSION-FIXES-PLAN.md Q6.1, §11): review findings the thread asked for and keeps open go
//! back to it as one fix, which resumes that session in its old worktree. Nothing reviews the
//! landing or the fix by itself.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn, git};
use crate::manager::review_hold::hold_owner;
use crate::work::{Task, TaskState};

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
    /// Whether task-1's session files and branch were still kept when the findings came.
    held_at_review: Option<bool>,
}

fn branch_exists(flow: &Flow, branch: &str) -> bool {
    !git(&flow.repo, &["branch", "--list", branch])
        .trim()
        .is_empty()
}

/// The worker that writes NOTES.md and reports; a fix (resumed, or fresh after a refused
/// resume) commits its fix and reports, asking for no review.
async fn notes_worker(turn: Turn, log: Arc<Mutex<Seen>>, refuse_resume: bool) -> Reply {
    if turn.input.contains("[workspace]") || turn.input.contains("task-1's, which landed") {
        log.lock().unwrap().fix = Some((
            turn.native_id.clone(),
            turn.cwd.clone(),
            turn.input.clone(),
            turn.git(&["rev-parse", "HEAD"]),
        ));
        turn.write("NOTES.md", "notes v1\n");
        turn.git(&["commit", "-qam", "Fix the notes"]);
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

async fn delegate_notes(turn: &Turn) {
    let reply = turn
        .call(
            "delegate_task",
            json!({"effort": "medium", "title": "Write the notes",
                   "kind": "implement", "spec": "Write NOTES.md.",
                   "provider": "claude"}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
}

/// The session: a worker writes NOTES.md and lands; the thread asks for a review, which finds
/// two things; it records both open (the first high) and sends them back as one fix, lands
/// the fix and runs the full checks.
async fn fixed_once(name: &str, refuse_resume: bool) -> (Flow, Arc<Mutex<Seen>>) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let (log, reviews_log) = (seen.clone(), seen.clone());
    let flow =
        Flow::start(
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
                        Reply::text(TWO)
                    }
                })),
                ..Options::default()
            },
            script(move |turn| {
                let log = log.clone();
                async move {
                    if !turn.is_orchestrator() {
                        return notes_worker(turn, log, refuse_resume).await;
                    }
                    if turn.input.contains("Add notes.") {
                        delegate_notes(&turn).await;
                        return Reply::text("");
                    }
                    if turn.input.contains("[report task-1") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        let review = turn
                            .call("request_review", json!({"wait_seconds": 60}))
                            .await;
                        assert!(review.text.contains("found 2"), "{}", review.text);
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
                                    {"id": "R1.1", "status": "open", "high": true},
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
                        // One fix at a time resumes a session: a second is no error, and the
                        // thread is told where the first stands (#251).
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
                            again
                                .text
                                .contains("task-2 fixes task-1's findings already (")
                                && again.text.contains("so no new task started"),
                            "{}",
                            again.text
                        );
                        return Reply::text("");
                    }
                    if turn.input.contains("[report task-2") {
                        let reply = turn.call("land_phase", json!({"task": "task-2"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        let checks = turn.call("run_checks", json!({})).await;
                        assert!(
                            checks.text.contains("[checks passed at "),
                            "{}",
                            checks.text
                        );
                        return Reply::text("Fixed the notes.");
                    }
                    Reply::text("Noted.")
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

/// Task-1, once its hold went: its session files and branch are gone.
async fn settled(flow: &Flow) -> Task {
    let board = flow
        .until("task-1's hold to go", |board| {
            Flow::task(board, 1).review_hold.is_none()
        })
        .await;
    Flow::task(&board, 1).clone()
}

#[tokio::test]
async fn findings_go_back_to_the_resumed_worker_and_its_fix_gets_no_review_by_itself() {
    let (flow, seen) = fixed_once("review-hold", false).await;
    let first = settled(&flow).await;
    let board = flow.settled().await;
    // Time for anything automatic to start.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(
        seen.held_at_review,
        Some(true),
        "kept until the findings came"
    );
    let (native, worktree) = seen.first.clone().unwrap();
    let (fix_native, fix_worktree, input, head) = seen.fix.clone().unwrap();
    // The fix resumed task-1's own session, in its worktree made again at the old path, on
    // the session's tip, and heard the findings as one message.
    assert_eq!(fix_native, native);
    assert_eq!(fix_worktree, worktree);
    assert_eq!(head, first.landed.clone().unwrap());
    assert!(input.starts_with("[workspace]"), "{input}");
    assert!(!input.contains("Don't call request_review"), "{input}");
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
    // No follow-up fix tasks, and the one review the thread asked for: no review of either
    // landing, and no re-check of the high finding's fix.
    assert_eq!(board.tasks.len(), 2, "{:#?}", board.tasks.keys());
    assert_eq!(seen.reviews.len(), 1, "{:#?}", seen.reviews);
    assert_eq!(flow.board().await.reviews.len(), 1);
    // The run_checks pass covered task-1's landing: its session files and branch are gone.
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

#[tokio::test]
async fn a_fix_whose_session_cannot_be_resumed_starts_fresh_from_the_report() {
    let (flow, seen) = fixed_once("review-hold-fresh", true).await;
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

/// A landed worker's session and branch stay while nothing checked its landing, and go once
/// the thread's run_checks pass covers it; nothing reviews the landing.
#[tokio::test]
async fn a_landing_is_kept_until_run_checks_passes_over_it() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "review-hold-checked",
        Options {
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            reviews: Some(script(|_| async { Reply::text(TWO) })),
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return notes_worker(turn, log, false).await;
                }
                if turn.input.contains("Add notes.") {
                    delegate_notes(&turn).await;
                    return Reply::text("");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Landed.");
                }
                if turn.input.contains("Check it.") {
                    let checks = turn.call("run_checks", json!({})).await;
                    assert!(
                        checks.text.contains("[checks passed at "),
                        "{}",
                        checks.text
                    );
                    return Reply::text("The checks pass.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the landing", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state == TaskState::Landed)
        })
        .await;
    let first = Flow::task(&board, 1).clone();
    flow.settled().await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    let board = flow.board().await;
    assert!(Flow::task(&board, 1).review_hold.is_some());
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);
    let branch = first.workspace.as_ref().unwrap().branch.clone().unwrap();
    assert!(branch_exists(&flow, &branch));
    flow.say("Check it.").await;
    settled(&flow).await;
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

/// The session's archive lets go of a landing still kept: nothing checked it.
#[tokio::test]
async fn archiving_the_session_settles_an_open_batch() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = seen.clone();
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
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return notes_worker(turn, log, false).await;
                }
                if turn.input.contains("Add notes.") {
                    delegate_notes(&turn).await;
                    return Reply::text("");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Landed.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the landing", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state == TaskState::Landed)
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
    flow.until("task-1's hold to go", |board| {
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
