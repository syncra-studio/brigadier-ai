//! The merge said while workers write (SESSION-FIXES-PLAN.md Q3): it is held, stored, until
//! they land, then the thread is told it is due and merges on the held consent without a new
//! question; a failed or stopped writer, or an open finding, stops it; an unrecorded finding
//! holds it; the user's Cancel spends the consent for good, under the landing lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

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
                        for _ in 0..3000 {
                            if review_gate.load(Ordering::SeqCst) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(20)).await;
                        }
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
                    // A writer: holds until released, then writes its own file.
                    for _ in 0..1000 {
                        if gate.load(Ordering::SeqCst) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    let file = if turn.task_number() == Some(1) {
                        "A.md"
                    } else {
                        "B.md"
                    };
                    turn.write(file, "note\n");
                    // A later turn (the self-check after a rebase) finds its file in already.
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
                    // The merge isn't offered on a card while they write.
                    let card = turn.call("propose_merge", json!({})).await;
                    assert!(card.is_error, "{}", card.text);
                    log(&format!("propose_merge: {}", card.text));
                    let reply = turn
                        .call(
                            "finish_session",
                            json!({"user_words": "merge it when done"}),
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
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[merge due]") {
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

#[tokio::test]
async fn merge_when_done_with_two_writers_merges_once_they_land_without_asking_again() {
    let held = held("held-merge", Plan::default()).await;
    writing(&held).await;
    assert_eq!(held.logged("Workers are still writing (task-1, task-2)"), 1);
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
    held.flow
        .until("the merge to stop", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. }))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(held.logged("[merge stopped] task-2 was stopped"), 1);
    assert_eq!(held.logged("[merge due] The workers"), 0);
    assert!(!held.merged());
    assert!(pending_merge(&held.flow.board().await).is_none());
    held.flow.stop().await;
}

/// "Wait" written while the merge is held takes the consent back: it stops rather than come
/// due, and finish_session refuses it.
#[tokio::test]
async fn a_wait_written_while_held_stops_the_merge() {
    let held = held("held-merge-wait", Plan::default()).await;
    writing(&held).await;
    held.flow.say("Wait, I want to look at it first.").await;
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge to stop", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. }))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(
        held.logged(
            "[merge stopped] The user wrote since they consented, and their message says \"wait\"."
        ),
        1
    );
    assert_eq!(held.logged("[merge due] The workers"), 0);
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

#[tokio::test]
async fn an_open_finding_stops_the_held_merge_at_medium_severity() {
    let held = held(
        "held-merge-open",
        Plan {
            finds: true,
            records: Some("open"),
            ..Plan::default()
        },
    )
    .await;
    writing(&held).await;
    held.release.store(true, Ordering::SeqCst);
    held.flow
        .until("the merge to stop", |board| {
            board
                .orchestrator_steps
                .iter()
                .any(|step| matches!(step.kind, OrchestratorStepKind::MergeStopped { .. }))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(
        held.logged(&format!(
            "[merge stopped] {} is open: The second note has no title",
            held.finding()
        )),
        1
    );
    assert!(!held.merged());
    held.flow.stop().await;
}

#[tokio::test]
async fn an_unrecorded_finding_holds_the_merge_until_recorded() {
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
    let log = held.log.clone();
    held.flow
        .until("the merge to wait", move |_| {
            log.lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("[merge waiting]"))
        })
        .await;
    held.flow.settled().await;
    assert_eq!(held.logged("[merge waiting]"), 1);
    assert_eq!(held.logged("[merge due] The workers"), 0);
    // finish_session itself refuses while a finding is unrecorded.
    let refused = held
        .flow
        .manager
        .finish_session(&held.flow.conversation, "", None)
        .await
        .unwrap_err()
        .to_string();
    let id = held.finding();
    assert!(
        refused.contains(&format!("Record the findings {id}")),
        "{refused}"
    );
    assert!(!held.merged());
    // Recorded fixed: the merge is due.
    let reply = held
        .flow
        .manager
        .record_findings(
            &held.flow.conversation,
            serde_json::from_value(
                json!({"items": [{"id": id, "status": "fixed", "task": "task-2"}]}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(reply.contains("Recorded"), "{reply}");
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
    held.flow.settled().await;
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

/// The last writer landed, but its review still runs: the held merge waits for it even when
/// the thread calls finish_session early, since the review may bring an open finding.
#[tokio::test]
async fn a_held_merge_waits_for_the_last_landings_review_even_when_called_early() {
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
    held.flow
        .until("both to land with task-2's review running", |board| {
            board
                .tasks
                .values()
                .filter(|task| task.state == TaskState::Landed)
                .count()
                == 2
                && crate::manager::landing::findings_now(board).running
        })
        .await;
    let refused = held
        .flow
        .manager
        .finish_session(&held.flow.conversation, "", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("A review of the session's work still runs"),
        "{refused}"
    );
    assert!(!held.merged());
    held.review.store(true, Ordering::SeqCst);
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

/// After the merge, the user's next message makes the session's worktree again, on a fresh
/// branch from the base; its turn hears so, so it never takes it for a leftover (bug 5).
#[tokio::test]
async fn the_message_after_a_merge_hears_of_its_fresh_worktree() {
    let held = held("held-merge-fresh", Plan::default()).await;
    writing(&held).await;
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
    // finish_session said the next message makes a fresh one.
    assert_eq!(
        held.logged("the user's next message starts a fresh branch from `main` in the same folder"),
        1
    );
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
    let tip = git(&held.flow.repo, &["rev-parse", "main"]);
    assert!(
        input.contains(&format!(
            "[workspace] A fresh branch `{}` from `main` at {}",
            branch_of(&held.flow).await,
            &tip[..10]
        )),
        "{input}"
    );
    assert!(input.contains("never remove it"), "{input}");
    held.flow.stop().await;
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
