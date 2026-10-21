//! The merge said while workers write (SESSION-FIXES-PLAN.md Q3): it is held, stored, until
//! they land, then the thread is told it is due and merges on the held consent without a new
//! question; a failed or stopped writer stops it; review findings and a review still running
//! never stop or hold it, and its result names them (#242); the user's Cancel spends the
//! consent for good, under the landing lock. The
//! thread's own commit that names the findings it fixes is covered by the held consent too.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::landing::pending_merge;
use crate::work::{MergeCondition, OrchestratorStepKind, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Waits for `gate` to be set, as a held worker or a slow reviewer does. It has no deadline of
/// its own: on a loaded machine one would let the turn go on before the test set it, out of
/// the order the test is about. If it is never set, the test's wait for what follows fails.
async fn until_set(gate: &AtomicBool) {
    while !gate.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const ONE_MEDIUM: &str =
    "Full review comments:\n\n- [P2] The second note has no title — B.md:1\n  Add one.";

/// What the scripted session does.
#[derive(Clone, Copy, Default)]
struct Plan {
    /// The reviews find [`ONE_MEDIUM`] in task-2's landing.
    finds: bool,
    /// What the thread records the finding as ("open", "fixed"), or nothing.
    records: Option<&'static str>,
    /// Task-2's review runs until [`Held::review`] is set.
    slow_review: bool,
}

struct Held {
    flow: Flow,
    /// The workers write once it is set.
    release: Arc<AtomicBool>,
    /// A slow review ends once it is set.
    review: Arc<AtomicBool>,
    /// The thread's inputs and what its finish_session calls answered, in order.
    log: Arc<Mutex<Vec<String>>>,
}

impl Held {
    fn logged(&self, what: &str) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|line| line.contains(what))
            .count()
    }

    /// The id of the finding in task-2's review, from the message that brought it.
    fn finding(&self) -> String {
        let log = self.log.lock().unwrap();
        log.iter()
            .find_map(|line| finding_id(line))
            .expect("task-2's review")
    }

    /// Whether the session's notes are on `main` (a merge may fast-forward it).
    fn merged(&self) -> bool {
        let files = git(&self.flow.repo, &["ls-tree", "--name-only", "main"]);
        files.contains("A.md") || files.contains("B.md")
    }
}

/// The id of the finding a `[review task-2 · …]` message lists ("R2.1"): reviews are numbered
/// in landing order, which the two writers don't fix.
fn finding_id(input: &str) -> Option<String> {
    let review = &input[input.find("[review task-2 · ")?..];
    review.match_indices('R').find_map(|(at, _)| {
        let digits: String = review[at + 1..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        (!digits.is_empty() && review[at + 1 + digits.len()..].starts_with(".1"))
            .then(|| format!("R{digits}.1"))
    })
}

/// A session whose thread delegates two writers and, in the same turn, passes the user's
/// "merge it when done" to finish_session; it lands each report, and calls finish_session
/// again on `[merge due]`.
async fn held(name: &str, plan: Plan) -> Held {
    let release = Arc::new(AtomicBool::new(false));
    let review = Arc::new(AtomicBool::new(false));
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gate, said, review_gate) = (release.clone(), log.clone(), review.clone());
    let flow = Flow::start(
        name,
        Options {
            reviews: Some(script(move |turn| {
                let review_gate = review_gate.clone();
                async move {
                    let base = turn
                        .input
                        .split("git diff ")
                        .nth(1)
                        .and_then(|rest| rest.split("...").next());
                    let changed = base.map(|base| turn.git(&["diff", "--name-only", base, "HEAD"]));
                    if plan.slow_review
                        && changed.as_ref().is_some_and(|files| files.contains("B.md"))
                    {
                        until_set(&review_gate).await;
                    }
                    if plan.finds && changed.is_some_and(|files| files.contains("B.md")) {
                        Reply::text(ONE_MEDIUM)
                    } else {
                        Reply::text("No findings.")
                    }
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (gate, said) = (gate.clone(), said.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    // A writer: holds until released, then writes its own file. One whose CLI
                    // a restart closed meanwhile goes no further, as a killed CLI's turn: it
                    // neither reports nor ends.
                    tokio::select! {
                        () = until_set(&gate) => {}
                        true = turn.stopped() => std::future::pending::<()>().await,
                    }
                    let file = if turn.task_number() == Some(1) {
                        "A.md"
                    } else {
                        "B.md"
                    };
                    turn.write(file, "note\n");
                    // A later turn (a fix sent back after landing) finds its file in already.
                    if !turn.git(&["status", "--porcelain"]).is_empty() {
                        turn.git(&["add", file]);
                        turn.git(&["commit", "-qm", &format!("Write {file}")]);
                    }
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": format!("Wrote {file}."), "changes": [file]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                said.lock().unwrap().push(turn.input.clone());
                let log = |text: &str| said.lock().unwrap().push(text.to_owned());
                // A message sent while the workers write joins their work: it comes steered
                // into this turn, with what Brigadier adds to it.
                if let Some(rest) = turn.input.split("[follow-up ").nth(1) {
                    let id = rest.split(']').next().unwrap();
                    let reply = turn
                        .call("route_follow_up", json!({"follow_up": id, "joins": true}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    let joined = turn.steered().await.unwrap_or_default();
                    log(&joined);
                    // The thread judges the user's words while the merge is held (#222): a
                    // "Wait" takes it back, a question about it doesn't.
                    if joined.contains("[merge held] The user's merge") {
                        if joined.contains("Wait, I want to look at it first.") {
                            let reply = turn
                                .call(
                                    "stop_merge",
                                    json!({"why": "they want to look at it first"}),
                                )
                                .await;
                            log(&format!("stop_merge: {}", reply.text));
                            return Reply::text("Stopped the merge, as you asked.");
                        }
                        if joined.contains("Is it merged yet or not?") {
                            return Reply::text(
                                "Not merged yet: it merges once the workers finish.",
                            );
                        }
                    }
                    return Reply::text(super::super::prompts::QUIET);
                }
                if turn.input.starts_with("Add two notes") {
                    for title in ["Write note A", "Write note B"] {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": title, "kind": "implement",
                                       "spec": "Write the note.", "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    turn.call("run_checks", json!({})).await;
                    let reply = turn
                        .call(
                            "finish_session",
                            json!({"user_words": "merge it when done", "when": "workers_finish"}),
                        )
                        .await;
                    log(&format!("finish_session: {}", reply.text));
                    return Reply::text("[quiet]");
                }
                let reports: Vec<&str> = ["1", "2"]
                    .into_iter()
                    .filter(|n| turn.input.contains(&format!("[report task-{n}")))
                    .collect();
                for n in &reports {
                    let reply = turn
                        .call("land_phase", json!({"task": format!("task-{n}")}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                if !reports.is_empty() && !turn.input.contains("[merge") {
                    return Reply::text("[quiet]");
                }
                if let Some(id) = finding_id(&turn.input)
                    && let Some(status) = plan.records
                {
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [{"id": id, "status": status, "task": "task-2"}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    // A merge due in the same turn goes ahead: findings don't hold it (#242).
                    if !turn.input.contains("[merge due]") {
                        return Reply::text("[quiet]");
                    }
                }
                if turn.input.contains("[merge due]") {
                    turn.call("run_checks", json!({})).await;
                    let reply = turn.call("finish_session", json!({})).await;
                    log(&format!("finish_session: {}", reply.text));
                    return Reply::text("Merged.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add two notes, and merge it when done.").await;
    Held {
        flow,
        release,
        review,
        log,
    }
}

async fn writing(held: &Held) {
    held.flow
        .until("both writers to run", |board| {
            board
                .tasks
                .values()
                .filter(|task| task.state == TaskState::Running)
                .count()
                == 2
        })
        .await;
    held.flow
        .until("the merge to be held", |board| {
            pending_merge(board).is_some()
        })
        .await;
}

/// Waits for the stopped merge and for the thread's turn on its `[merge stopped]`: the step is
/// stored before the thread hears it, and no request works in between. The `[merge held]`
/// answer names `[merge stopped]` too, so it doesn't count as hearing it.
async fn stopped(held: &Held) {
    held.flow
        .until("the merge to stop and the thread to hear it", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. }))
                && held
                    .log
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|line| line.contains("[merge stopped]") && !line.contains("[merge held]"))
        })
        .await;
    held.flow.settled().await;
}

#[tokio::test]
async fn merge_when_done_with_two_writers_merges_once_they_land_without_asking_again() {
    let held = held("held-merge", Plan::default()).await;
    writing(&held).await;
    assert_eq!(
        held.logged("finish_session: [merge held] Merging after 2 workers finish"),
        1
    );
    let pending = pending_merge(&held.flow.board().await).unwrap();
    assert_eq!(pending.condition, MergeCondition::WorkersFinish);
    assert_eq!(pending.waiting.len(), 2);
    assert!(!held.merged());
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(held.logged("[merge due] The workers"), 1);
    assert_eq!(held.logged("finish_session: [finished]"), 1);
    assert!(held.merged());
    let files = git(&held.flow.repo, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("A.md") && files.contains("B.md"), "{files}");
    // No second question was asked: no merge card.
    assert!(
        !held
            .flow
            .board()
            .await
            .questions
            .values()
            .any(|question| { matches!(question.kind, crate::work::QuestionKind::Merge { .. }) })
    );
    held.flow.stop().await;
}

#[tokio::test]
async fn a_stopped_writer_stops_the_held_merge() {
    let held = held("held-merge-stopped", Plan::default()).await;
    writing(&held).await;
    let task2 = Flow::task(&held.flow.board().await, 2).id.clone();
    held.flow.manager.stop_task(task2).await.unwrap();
    held.release.store(true, Ordering::SeqCst);
    stopped(&held).await;
    assert_eq!(held.logged("[merge stopped] task-2 was stopped"), 1);
    assert_eq!(held.logged("[merge due] The workers"), 0);
    assert!(!held.merged());
    assert!(pending_merge(&held.flow.board().await).is_none());
    held.flow.stop().await;
}

/// "Wait" written while the merge is held: the thread hears that a merge is held with the
/// message, reads it as taking the consent back and stops it with stop_merge before it answers,
/// once; finish_session then refuses (#222).
#[tokio::test]
async fn a_wait_written_while_held_stops_the_merge() {
    let held = held("held-merge-wait", Plan::default()).await;
    writing(&held).await;
    held.flow.say("Wait, I want to look at it first.").await;
    held.flow
        .until("the thread to stop the merge", |board| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, OrchestratorStepKind::MergeStopped { reason, .. }
                    if reason == "The user took it back: they want to look at it first.")
            })
        })
        .await;
    // The engine records the step inside the call; the thread logs its reply after (#285).
    super::eventually("the thread's stop_merge reply", || {
        held.logged("stop_merge: Stopped") > 0
    })
    .await;
    assert_eq!(held.logged("stop_merge: Stopped"), 1);
    held.release.store(true, Ordering::SeqCst);
    held.flow.settled().await;
    assert_eq!(held.logged("[merge due] The workers"), 0);
    assert!(
        !held
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("[merge stopped]")),
        "the thread stopped it itself"
    );
    held.flow.checks_passed().await;
    let refused = held
        .flow
        .manager
        .finish_session(&held.flow.conversation, "", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("[not merged]"), "{refused}");
    assert!(!held.merged());
    held.flow.stop().await;
}

/// #222: a question about the held merge, with a "not" and a "don't" in it, leaves the consent
/// standing. The thread hears the merge is held with the message, so it answers once, and the
/// merge goes when the workers land.
#[tokio::test]
async fn a_question_about_the_held_merge_leaves_it_standing_and_gets_one_answer() {
    let held = held("held-merge-question", Plan::default()).await;
    writing(&held).await;
    held.flow
        .say("what is going on? Is it merged yet or not? I don't understand a thing what is going on...")
        .await;
    held.flow
        .until(
            "the question to reach the thread with the held merge",
            |_| held.logged("[merge held] The user's merge of") == 1,
        )
        .await;
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    held.flow.settled().await;
    let board = held.flow.board().await;
    assert!(
        !board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. })),
        "{:#?}",
        held.log.lock().unwrap()
    );
    assert!(
        !held
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.starts_with("[merge stopped]"))
    );
    assert_eq!(held.logged("finish_session: [finished]"), 1);
    assert!(held.merged());
    let messages = held
        .flow
        .core
        .all_messages(&held.flow.conversation)
        .await
        .unwrap();
    let answers = messages
        .iter()
        .filter(|message| message.text.contains("Not merged yet"))
        .count();
    assert_eq!(answers, 1, "one answer to the question");
    held.flow.stop().await;
}

/// #240: a message that asks for the merge merges, whatever else it says ("not caused by this
/// change", "Don't rerun them"): the thread judged it, and the engine checks only that the words
/// are the user's and that no earlier merge used them.
#[tokio::test]
async fn a_request_to_merge_with_a_not_elsewhere_merges() {
    let log: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let said = log.clone();
    let flow = Flow::start(
        "held-merge-asked",
        Options::default(),
        script(move |turn| {
            let said = said.clone();
            async move {
                if !turn.input.contains("then merge into main and push") {
                    return Reply::text("Done.");
                }
                commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                for _ in 0..2 {
                    turn.call("run_checks", json!({})).await;
                    let reply = turn
                        .call(
                            "finish_session",
                            json!({"consent": "then merge into main and push"}),
                        )
                        .await;
                    said.lock().unwrap().push((reply.text, reply.is_error));
                }
                Reply::text("Merged into main.")
            }
        }),
    )
    .await;
    flow.say("Add notes. The test failures are not caused by this change. Don't rerun them, then merge into main and push.")
        .await;
    flow.until("both merge calls", |_| log.lock().unwrap().len() == 2)
        .await;
    flow.settled().await;
    let log = log.lock().unwrap().clone();
    assert!(!log[0].1 && log[0].0.contains("[finished]"), "{log:#?}");
    // One consent, one merge.
    assert!(log[1].1 && log[1].0.contains("[not merged]"), "{log:#?}");
    let files = git(&flow.repo, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("NOTES.md"), "{files}");
    flow.stop().await;
}

/// Waits for the held merge to be done.
async fn merged_step(held: &Held) {
    held.flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
}

/// #242: a finding recorded open, or fixed without a check, never stops the held merge.
async fn a_finding_recorded(name: &str, status: &'static str) {
    let held = held(
        name,
        Plan {
            finds: true,
            records: Some(status),
            ..Plan::default()
        },
    )
    .await;
    writing(&held).await;
    held.release.store(true, Ordering::SeqCst);
    merged_step(&held).await;
    held.flow
        .until("the finding recorded", |board| {
            board.reviews.values().any(|review| {
                review
                    .items
                    .iter()
                    .any(|item| item.status != crate::work::FindingStatus::New)
            })
        })
        .await;
    let board = held.flow.settled().await;
    assert!(held.merged());
    assert!(
        !board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. })),
        "{:#?}",
        held.log.lock().unwrap()
    );
    assert_eq!(held.logged("[merge waiting]"), 0);
    held.flow.stop().await;
}

#[tokio::test]
async fn an_open_finding_doesnt_stop_the_held_merge() {
    a_finding_recorded("held-merge-open", "open").await;
}

#[tokio::test]
async fn an_unverified_fix_doesnt_stop_the_held_merge() {
    a_finding_recorded("held-merge-unverified", "fixed").await;
}

/// #242: a finding the thread hasn't recorded doesn't hold the merge, and finish_session
/// merges with it: no "record them first".
#[tokio::test]
async fn an_unrecorded_finding_doesnt_hold_the_merge() {
    let held = held(
        "held-merge-unrecorded",
        Plan {
            finds: true,
            records: None,
            ..Plan::default()
        },
    )
    .await;
    writing(&held).await;
    held.release.store(true, Ordering::SeqCst);
    merged_step(&held).await;
    held.flow.settled().await;
    assert!(held.merged());
    assert_eq!(held.logged("[merge waiting]"), 0);
    assert_eq!(held.logged("Record the findings"), 0);
    held.flow.stop().await;
}

/// #243: "merge it" said while a worker still writes isn't held: finish_session (when "now")
/// merges what has landed and names the worker still running, whose work isn't in it.
#[tokio::test]
async fn a_merge_now_while_a_worker_writes_merges_what_landed_and_names_it() {
    let release = Arc::new(AtomicBool::new(false));
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let (gate, said) = (release.clone(), log.clone());
    let flow = Flow::start(
        "merge-now-while-writing",
        Options::default(),
        script(move |turn| {
            let (gate, said) = (gate.clone(), said.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    // Task-1 writes at once; task-2 holds until released.
                    let file = if turn.task_number() == Some(1) {
                        "A.md"
                    } else {
                        until_set(&gate).await;
                        "B.md"
                    };
                    turn.write(file, "note\n");
                    turn.git(&["add", file]);
                    turn.git(&["commit", "-qm", &format!("Write {file}")]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": format!("Wrote {file}."), "changes": [file]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.starts_with("Add two notes") {
                    for title in ["Write note A", "Write note B"] {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": title, "kind": "implement",
                                       "spec": "Write the note.", "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    turn.call("run_checks", json!({})).await;
                    let reply = turn
                        .call("finish_session", json!({"consent": "merge what you have"}))
                        .await;
                    said.lock().unwrap().push(reply.text);
                    return Reply::text("Merged note A; note B is still being written.");
                }
                if turn.input.contains("[report task-2") {
                    let reply = turn.call("land_phase", json!({"task": "task-2"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add two notes, and merge what you have.").await;
    let merged = log.clone();
    flow.until("the merge", move |_| !merged.lock().unwrap().is_empty())
        .await;
    let result = log.lock().unwrap()[0].clone();
    assert!(result.starts_with("[finished]"), "{result}");
    assert!(
        result.contains("Still running, so not in this merge: task-2 \"Write note B\""),
        "{result}"
    );
    let files = git(&flow.repo, &["ls-tree", "--name-only", "main"]);
    assert!(files.contains("A.md") && !files.contains("B.md"), "{files}");
    let board = flow.board().await;
    assert!(pending_merge(&board).is_none(), "nothing is held");
    release.store(true, Ordering::SeqCst);
    flow.settled().await;
    flow.stop().await;
}

#[tokio::test]
async fn a_cancel_spends_the_consent_across_a_restart() {
    let mut held = held("held-merge-cancel", Plan::default()).await;
    writing(&held).await;
    let reply = held
        .flow
        .manager
        .cancel_merge(&held.flow.conversation)
        .await
        .unwrap();
    assert_eq!(reply, "Merge cancelled.");
    // Not settled(): the user's request works until the held writers end.
    let log = held.log.clone();
    held.flow
        .until("the thread to hear the cancel", move |_| {
            log.lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("[merge cancelled by the user]"))
        })
        .await;
    assert_eq!(held.logged("[merge cancelled by the user]"), 1);
    assert!(pending_merge(&held.flow.board().await).is_none());
    held.flow.restart().await;
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("both to land", |board| {
            board
                .tasks
                .values()
                .filter(|task| task.state == TaskState::Landed)
                .count()
                == 2
        })
        .await;
    held.flow.settled().await;
    assert_eq!(held.logged("[merge due] The workers"), 0);
    // The old message no longer counts.
    held.flow.checks_passed().await;
    let refused = held
        .flow
        .manager
        .finish_session(&held.flow.conversation, "merge it when done", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("[not merged]"), "{refused}");
    assert!(!held.merged());
    held.flow.stop().await;
}

#[tokio::test]
async fn the_held_merge_survives_a_restart() {
    let mut held = held("held-merge-restart", Plan::default()).await;
    writing(&held).await;
    held.flow.restart().await;
    let pending = pending_merge(&held.flow.board().await).expect("still held");
    assert_eq!(pending.condition, MergeCondition::WorkersFinish);
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    assert!(held.merged());
    held.flow.stop().await;
}

/// #242: the last writer landed and its review still runs: the held merge doesn't wait for it,
/// and its result says the work isn't reviewed yet.
#[tokio::test]
async fn a_review_still_running_doesnt_hold_the_merge_and_its_result_says_so() {
    let held = held(
        "held-merge-review-runs",
        Plan {
            slow_review: true,
            ..Plan::default()
        },
    )
    .await;
    writing(&held).await;
    held.release.store(true, Ordering::SeqCst);
    merged_step(&held).await;
    assert!(held.merged());
    let board = held.flow.board().await;
    assert!(
        board
            .reviews
            .values()
            .any(|review| review.state == crate::work::ReviewState::Running),
        "task-2's review still runs"
    );
    let log = held.log.clone();
    held.flow
        .until("the merge's result", move |_| {
            log.lock()
                .unwrap()
                .iter()
                .any(|line| line.starts_with("finish_session: [finished]"))
        })
        .await;
    let result = held
        .log
        .lock()
        .unwrap()
        .iter()
        .find(|line| line.starts_with("finish_session: [finished]"))
        .cloned()
        .unwrap();
    assert!(
        result.contains("Its review still runs, so this work isn't reviewed yet"),
        "{result}"
    );
    held.review.store(true, Ordering::SeqCst);
    held.flow.settled().await;
    held.flow.stop().await;
}

/// A Cancel racing the due merge: whichever takes the landing lock first wins.
#[tokio::test]
async fn a_cancel_waits_for_a_merge_under_way_and_finds_it_merged() {
    let held = held("held-merge-race", Plan::default()).await;
    writing(&held).await;
    // The thread's own call on [merge due] is held at its pause, with the landing lock.
    let conv = held.flow.manager.conv(&held.flow.conversation).unwrap();
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *conv.merge_pause.lock().unwrap() = Some((reached.clone(), release.clone()));
    held.release.store(true, Ordering::SeqCst);
    reached.notified().await;
    let manager = held.flow.manager.clone();
    let id = held.flow.conversation.clone();
    let cancel = tokio::spawn(async move { manager.cancel_merge(&id).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !cancel.is_finished(),
        "the Cancel waits for the landing lock"
    );
    release.notify_one();
    assert_eq!(cancel.await.unwrap().unwrap(), "Too late: already merged.");
    *conv.merge_pause.lock().unwrap() = None;
    held.flow.settled().await;
    assert!(held.merged());
    held.flow.stop().await;
}

/// After the merge, the session keeps its worktree and branch (BUILD-PLAN A3): the branch is at
/// the merge, and the next request works in the same folder.
#[tokio::test]
async fn a_merge_keeps_the_workspace_and_the_next_request_works_in_it() {
    let held = held("held-merge-kept", Plan::default()).await;
    writing(&held).await;
    let before = worktree_of(&held.flow);
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(
        held.logged("The session keeps its worktree and branch: `"),
        1,
        "{:?}",
        held.log.lock().unwrap()
    );
    let branch = branch_of(&held.flow).await;
    assert!(before.join(".git").exists(), "the worktree stays");
    assert_eq!(worktree_of(&held.flow), before);
    let main = git(&held.flow.repo, &["rev-parse", "main"]);
    assert_eq!(git(&held.flow.repo, &["rev-parse", &branch]), main);
    assert_eq!(git(&before, &["rev-parse", "HEAD"]), main);
    held.flow.say("Now add a third note.").await;
    held.flow.settled().await;
    let input = held
        .log
        .lock()
        .unwrap()
        .iter()
        .find(|line| line.contains("Now add a third note."))
        .cloned()
        .unwrap();
    assert!(!input.contains("[workspace] A fresh branch"), "{input}");
    assert_eq!(worktree_of(&held.flow), before);
    assert!(before.join("A.md").exists() && before.join("B.md").exists());
    held.flow.stop().await;
}

/// The session's worktree, as the conversation records it.
fn worktree_of(flow: &Flow) -> std::path::PathBuf {
    match flow
        .manager
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
    {
        Some(crate::model::Setup::Session {
            environment:
                crate::model::Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => std::path::PathBuf::from(path),
        other => panic!("no session worktree: {other:?}"),
    }
}

async fn branch_of(flow: &Flow) -> String {
    match flow
        .manager
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
    {
        Some(crate::model::Setup::Session {
            environment: crate::model::Environment::NewWorktree { branch, .. },
            ..
        }) => branch,
        _ => panic!("a new-worktree session"),
    }
}

/// How the thread fixes its landing review's finding itself, in [`fixed_by_the_thread`].
#[derive(Clone, Copy, Debug, PartialEq)]
enum OwnFix {
    /// Its commit names the finding (`Brigadier-Fixes: R1.1`), recorded `high` or not.
    Named { high: bool },
    /// Its commit doesn't say what it fixes.
    Unnamed,
}

/// A session whose thread delegates one writer and passes the user's "merge it when done" to
/// finish_session; it lands the report, fixes the landing review's one finding itself with a
/// commit of its own (as `fix` says), records it fixed, and calls finish_session on `[merge
/// due]`. The thread's inputs and finish_session's answers, and the reviewers' inputs.
async fn fixed_by_the_thread(
    name: &str,
    fix: OwnFix,
) -> (Flow, Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>) {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let reviews: Arc<Mutex<Vec<String>>> = Arc::default();
    let merge_held = Arc::new(AtomicBool::new(false));
    // The thread waits for the landing's review before it merges: the review doesn't hold the
    // merge (#242), the thread judges it does here.
    let due = Arc::new(AtomicBool::new(false));
    let (said, read, gate) = (log.clone(), reviews.clone(), merge_held.clone());
    let flow = Flow::start(
        name,
        Options {
            reviews: Some(script(move |turn| {
                let read = read.clone();
                async move {
                    let first = {
                        let mut read = read.lock().unwrap();
                        read.push(turn.input.clone());
                        read.len() == 1
                    };
                    // The landing's review finds one thing; nothing after it does.
                    Reply::text(if first { ONE_MEDIUM } else { "No findings." })
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (said, gate, due) = (said.clone(), gate.clone(), due.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    // Writes once the merge is held.
                    until_set(&gate).await;
                    turn.write("B.md", "note\n");
                    turn.git(&["add", "B.md"]);
                    turn.git(&["commit", "-qm", "Write B.md"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Wrote B.md.", "changes": ["B.md"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                said.lock().unwrap().push(turn.input.clone());
                let log = |text: &str| said.lock().unwrap().push(text.to_owned());
                if turn.input.starts_with("Add a note") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write note B", "kind": "implement",
                                   "spec": "Write the note.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    turn.call("run_checks", json!({})).await;
                    let reply = turn
                        .call(
                            "finish_session",
                            json!({"user_words": "merge it when done", "when": "workers_finish"}),
                        )
                        .await;
                    log(&format!("finish_session: {}", reply.text));
                    gate.store(true, Ordering::SeqCst);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[review task-1 · ") {
                    assert!(turn.input.contains("R1.1: [P2]"), "{}", turn.input);
                    let message = match fix {
                        OwnFix::Named { .. } => {
                            "Title note B\n\nBrigadier-Author: thread\nBrigadier-Fixes: R1.1"
                        }
                        OwnFix::Unnamed => "Title note B\n\nBrigadier-Author: thread",
                    };
                    // Unnamed, it is other work: big enough for a review of its own
                    // (BUILD-PLAN B2).
                    let text = match fix {
                        OwnFix::Named { .. } => "# B\nnote\n".to_owned(),
                        OwnFix::Unnamed => {
                            format!("# B\n{}", super::thread_tests::reviewed_notes())
                        }
                    };
                    // Other work goes in a file of code: the review line counts no docs (Q48).
                    let file = match fix {
                        OwnFix::Named { .. } => "B.md",
                        OwnFix::Unnamed => "notes-b.txt",
                    };
                    commit_in_workspace(&turn, file, &text, message);
                    let high = fix == OwnFix::Named { high: true };
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [{"id": "R1.1", "status": "fixed", "high": high,
                                           "checked": "read B.md at the tip"}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    if !due.load(Ordering::SeqCst) && !turn.input.contains("[merge due]") {
                        return Reply::text("[quiet]");
                    }
                    turn.call("run_checks", json!({})).await;
                    let reply = turn.call("finish_session", json!({})).await;
                    log(&format!("finish_session: {}", reply.text));
                    return Reply::text("Merged.");
                }
                if turn.input.contains("[merge due]") {
                    // Not before the landing's review is in and its finding fixed.
                    due.store(true, Ordering::SeqCst);
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add a note, and merge it when done.").await;
    (flow, log, reviews)
}

fn count(log: &Arc<Mutex<Vec<String>>>, what: &str) -> usize {
    log.lock()
        .unwrap()
        .iter()
        .filter(|line| line.contains(what))
        .count()
}

/// The user said "merge it when done"; the landing's review found one thing, and the thread
/// fixed it itself, naming it in its commit. That commit gets no review of its own (a re-check
/// only when the thread recorded the finding high), the held consent covers it, and the merge
/// happens with no second question (SESSION-FIXES-PLAN.md Q3.4, Q6.1).
#[tokio::test]
async fn a_held_merge_covers_the_threads_own_fix_of_a_finding_and_asks_nothing_again() {
    for high in [false, true] {
        let (flow, log, reviews) =
            fixed_by_the_thread("held-merge-own-fix", OwnFix::Named { high }).await;
        flow.until("the merge", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        })
        .await;
        let board = flow.settled().await;
        assert_eq!(
            count(
                &log,
                "finish_session: [merge held] Merging after 1 worker finish"
            ),
            1
        );
        assert_eq!(count(&log, "finish_session: [finished]"), 1);
        assert_eq!(count(&log, "Other work landed"), 0);
        assert!(
            !board
                .questions
                .values()
                .any(|question| matches!(question.kind, crate::work::QuestionKind::Merge { .. })),
            "no second merge card"
        );
        // The thread's fix is what merged.
        assert_eq!(
            git(&flow.repo, &["show", "main:B.md"]).trim_end(),
            "# B\nnote"
        );
        // The landing's review, and a re-check of the fix only when its finding is high: never
        // a full review of the thread's commit.
        let reviews = reviews.lock().unwrap().clone();
        let rechecks = reviews
            .iter()
            .filter(|input| input.contains("Re-check a fix"))
            .count();
        if high {
            assert_eq!(reviews.len(), 2, "{reviews:#?}");
            assert_eq!(rechecks, 1);
            assert!(reviews[1].contains("R1.1: [P2] The second note has no title"));
        } else {
            assert_eq!(reviews.len(), 1, "{reviews:#?}");
        }
        assert_eq!(count(&log, "[review of your commits"), 0);
        flow.stop().await;
    }
}

/// The rule's other side: a commit of the thread's that doesn't name the findings it fixes is
/// other work, with a full review of its own (it crosses the review line). The held consent
/// doesn't cover it, and the merge's result says it went in too, for the thread to tell the
/// user (#243).
#[tokio::test]
async fn a_threads_commit_that_names_no_finding_gets_a_full_review_and_is_named_in_the_merge() {
    let (flow, log, reviews) = fixed_by_the_thread("held-merge-own-commit", OwnFix::Unnamed).await;
    let said = log.clone();
    flow.until("the merge", move |_| {
        count(&said, "finish_session: [finished]") > 0
    })
    .await;
    flow.settled().await;
    let reviews = reviews.lock().unwrap().clone();
    assert!(reviews.len() >= 2, "{reviews:#?}");
    assert!(!reviews[1].contains("Re-check a fix"), "{}", reviews[1]);
    assert_eq!(
        count(
            &log,
            "after the user's consent, and it is in this merge too"
        ),
        1
    );
    assert!(git(&flow.repo, &["ls-tree", "--name-only", "main"]).contains("notes-b.txt"));
    flow.stop().await;
}

/// The user merges while the landing's review still runs; its one finding comes after the
/// merge, recorded high, and the thread fixes it with a commit that names it. That review
/// started before the merge, yet the fix is still known as one: it gets the targeted re-check
/// of the finding, never a full review of the thread's commit.
#[tokio::test]
async fn a_named_fix_of_a_review_that_ran_across_the_merge_gets_only_its_recheck() {
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let reviews: Arc<Mutex<Vec<String>>> = Arc::default();
    let (merged, fixed) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let (said, read, gate, merging) =
        (log.clone(), reviews.clone(), merged.clone(), merged.clone());
    let flow = Flow::start(
        "merged-then-own-fix",
        Options {
            reviews: Some(script(move |turn| {
                let (read, gate) = (read.clone(), gate.clone());
                async move {
                    let first = {
                        let mut read = read.lock().unwrap();
                        read.push(turn.input.clone());
                        read.len() == 1
                    };
                    if !first {
                        return Reply::text("No findings.");
                    }
                    // The landing's review ends only after the merge.
                    until_set(&gate).await;
                    Reply::text(ONE_MEDIUM)
                }
            })),
            ..Options::default()
        },
        script(move |turn| {
            let (said, merging, fixed) = (said.clone(), merging.clone(), fixed.clone());
            async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if !turn.is_orchestrator() {
                    turn.write("B.md", "note\n");
                    turn.git(&["add", "B.md"]);
                    turn.git(&["commit", "-qm", "Write B.md"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Wrote B.md.", "changes": ["B.md"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                said.lock().unwrap().push(turn.input.clone());
                if turn.input.starts_with("Add a note") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write note B", "kind": "implement",
                                   "spec": "Write the note.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    turn.call("run_checks", json!({})).await;
                    let reply = turn
                        .call("finish_session", json!({"user_words": "merge it"}))
                        .await;
                    assert!(reply.text.contains("[finished]"), "{}", reply.text);
                    merging.store(true, Ordering::SeqCst);
                    return Reply::text("Merged the note.");
                }
                if turn.input.contains("[review task-1 · ") {
                    assert!(turn.input.contains("R1.1: [P2]"), "{}", turn.input);
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [{"id": "R1.1", "status": "open", "high": true}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("The review found the note has no title. Fix it?");
                }
                if turn.input.contains("Fix the title.") && !fixed.swap(true, Ordering::SeqCst) {
                    commit_in_workspace(
                        &turn,
                        "B.md",
                        "# B\nnote\n",
                        "Title note B\n\nBrigadier-Author: thread\nBrigadier-Fixes: R1.1",
                    );
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [{"id": "R1.1", "status": "fixed", "high": true}]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Fixed the missing title.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add a note, and merge it.").await;
    let said = log.clone();
    flow.until("the review heard after the merge", move |board| {
        board.run == crate::work::RunState::Idle
            && said
                .lock()
                .unwrap()
                .iter()
                .any(|input| input.contains("[review task-1 · "))
    })
    .await;
    flow.say("Fix the title.").await;
    let read = reviews.clone();
    flow.until("the fix's review", move |board| {
        read.lock().unwrap().len() >= 2
            && board
                .reviews
                .values()
                .all(|review| review.state != crate::work::ReviewState::Running)
    })
    .await;
    flow.settled().await;
    let reviews = reviews.lock().unwrap().clone();
    assert_eq!(reviews.len(), 2, "{reviews:#?}");
    assert!(reviews[1].contains("Re-check a fix"), "{}", reviews[1]);
    assert!(reviews[1].contains("R1.1: [P2] The second note has no title"));
    let said = log.lock().unwrap().clone();
    assert!(
        !said
            .iter()
            .any(|input| input.contains("[review of your commits")),
        "{said:#?}"
    );
    flow.stop().await;
}
