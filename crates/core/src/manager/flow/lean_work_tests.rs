//! A worker never waits for its own review (revision 27): it reports while the review still
//! runs, and that review becomes part of its landing. The commits after it get a review of
//! their own; the landed worker stays held until every review of the landing has settled,
//! whichever ends first, and a fix of their findings has the high ones re-checked across both.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::{LEAN_WORK_REVISION, revision_note};
use crate::manager::review_runs::WAITING_FOR_REVIEW;
use crate::work::{ReviewFor, ReviewState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const TWO: &str = "Full review comments:\n\n- [P1] Old notes no longer load — NOTES.md:1\n  Keep the old format.\n- [P3] Trailing space — NOTES.md:1\n  Trim it.";

/// Which review ends first in [`landed_in_parts`].
#[derive(Clone, Copy, PartialEq)]
enum First {
    /// The review of the commits after the worker's own review (clean).
    Suffix,
    /// The worker's own review (two findings).
    Prefix,
}

/// What the scripted sessions saw.
#[derive(Default)]
struct Seen {
    /// The reviewers' inputs, in the order they started.
    reviews: Vec<String>,
    /// The fix's first input, when it ran.
    fix: Option<String>,
    /// What review_code answered the worker.
    asked: Option<String>,
}

struct Gates {
    prefix: Arc<tokio::sync::Notify>,
    suffix: Arc<tokio::sync::Notify>,
}

/// A worker commits NOTES.md, asks for its review, commits README.md, and reports while that
/// review still runs; the thread lands it. Its own review (the prefix) and the review of
/// README.md (the suffix) wait for their gates. With `fix`, the thread records the prefix's
/// findings open (the first high) and sends them back; otherwise it dismisses them.
async fn landed_in_parts(name: &str, fix: bool) -> (Flow, Arc<Mutex<Seen>>, Gates) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let (log, reviews_log) = (seen.clone(), seen.clone());
    let gates = Gates {
        prefix: Arc::new(tokio::sync::Notify::new()),
        suffix: Arc::new(tokio::sync::Notify::new()),
    };
    let (prefix, suffix) = (gates.prefix.clone(), gates.suffix.clone());
    let count = Arc::new(AtomicUsize::new(0));
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            reviews: Some(script(move |turn| {
                let (log, prefix, suffix) = (reviews_log.clone(), prefix.clone(), suffix.clone());
                let n = count.fetch_add(1, Ordering::SeqCst);
                async move {
                    log.lock().unwrap().reviews.push(turn.input.clone());
                    match n {
                        0 => {
                            prefix.notified().await;
                            Reply::text(TWO)
                        }
                        1 => {
                            suffix.notified().await;
                            Reply::text("No findings.")
                        }
                        _ => Reply::text("No findings."),
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
                    if turn.input.contains("[review task-1 · ") && turn.input.contains("R1.1") {
                        let items = if fix {
                            json!([{"id": "R1.1", "status": "open", "high": true},
                                   {"id": "R1.2", "status": "open"}])
                        } else {
                            json!([{"id": "R1.1", "status": "dismissed", "why": "the request didn't ask for it"},
                                   {"id": "R1.2", "status": "dismissed", "why": "the request didn't ask for it"}])
                        };
                        let reply = turn.call("record_findings", json!({"items": items})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        if fix {
                            let reply = turn
                                .call(
                                    "delegate_task",
                                    json!({"effort": "medium", "title": "Fix the notes",
                                           "kind": "implement", "role": "fix", "subject": "task-1",
                                           "spec": "Fix R1.1 and R1.2."}),
                                )
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                        }
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[workspace]") {
                    log.lock().unwrap().fix = Some(turn.input.clone());
                    turn.write("NOTES.md", "notes v1\n");
                    turn.git(&["commit", "-qam", "Fix the notes"]);
                    let reply = turn
                        .call("submit_report", json!({"summary": "Fixed both."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                turn.write("NOTES.md", "notes \n");
                turn.git(&["add", "NOTES.md"]);
                turn.git(&["commit", "-qm", "Write the notes"]);
                let asked = turn.call("review_code", json!({})).await;
                assert!(!asked.is_error, "{}", asked.text);
                log.lock().unwrap().asked = Some(asked.text);
                turn.write("README.md", "See NOTES.md.\n");
                turn.git(&["add", "README.md"]);
                turn.git(&["commit", "-qm", "Point to the notes"]);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Wrote NOTES.md; its review still runs.",
                               "changes": ["NOTES.md", "README.md"]}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.until("task-1 to land and both reviews to start", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.state == TaskState::Landed)
            && board.reviews.len() == 2
    })
    .await;
    (flow, seen, gates)
}

/// The landing's two reviews: the worker's own (its tip short of the landing) and the one of
/// the commits after it, from that tip to the landing.
fn parts(flow_board: &crate::board::Board) -> (crate::work::ReviewRun, crate::work::ReviewRun) {
    let landed = Flow::task(flow_board, 1).landed.clone().unwrap();
    let mut reviews: Vec<_> = flow_board.reviews.values().cloned().collect();
    reviews.sort_by_key(|review| review.number);
    let (prefix, suffix) = (reviews[0].clone(), reviews[1].clone());
    assert_ne!(prefix.tip, landed);
    assert_eq!(prefix.landing.as_deref(), Some(landed.as_str()));
    assert_eq!(prefix.notify, ReviewFor::Orchestrator);
    assert_eq!(
        suffix.base, prefix.tip,
        "only the commits after it are reviewed"
    );
    assert_eq!(suffix.tip, landed);
    (prefix, suffix)
}

async fn held(flow: &Flow) -> bool {
    let board = flow.core.board(&flow.conversation).await.unwrap();
    Flow::task(&board, 1).review_hold.is_some()
}

/// The worker's own review ends after the review of the commits after it: the clean suffix
/// doesn't let go of the landed worker; the prefix's findings come back as a fix that resumes
/// it, and its landing re-checks the high one.
#[tokio::test]
async fn a_clean_suffix_waits_for_the_prefix_and_the_fix_rechecks_its_high_finding() {
    let (flow, seen, gates) = landed_in_parts("lean-suffix-first", true).await;
    let board = flow.core.board(&flow.conversation).await.unwrap();
    parts(&board);
    let asked = seen.lock().unwrap().asked.clone().unwrap();
    assert!(asked.contains("don't wait for it"), "{asked}");
    // A worker told the instructions before this revision hears it once when resumed.
    flow.manager
        .update_task(&flow.conversation, &Flow::task(&board, 1).id, |task| {
            task.told_revision = Some(LEAN_WORK_REVISION - 1)
        })
        .await
        .unwrap();
    gates.suffix.notify_one();
    flow.until("the suffix review to end", |board| {
        board
            .reviews
            .values()
            .filter(|review| review.state == ReviewState::Clean)
            .count()
            == 1
    })
    .await;
    flow.settled().await;
    assert!(
        held(&flow).await,
        "the prefix still runs: task-1 stays held"
    );
    gates.prefix.notify_one();
    flow.until("the fix to land", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 2 && task.state == TaskState::Landed)
    })
    .await;
    super::eventually("the re-check", || seen.lock().unwrap().reviews.len() == 3).await;
    flow.until("task-1's batch to settle", |board| {
        Flow::task(board, 1).review_hold.is_none()
    })
    .await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    assert!(
        seen.reviews[2].contains("Re-check a fix")
            && seen.reviews[2].contains("R1.1: [P1] Old notes no longer load")
            && !seen.reviews[2].contains("R1.2"),
        "{:#?}",
        seen.reviews
    );
    let fix = seen.fix.unwrap();
    let note = revision_note(LEAN_WORK_REVISION - 1, true).unwrap();
    assert_eq!(fix.matches(&note).count(), 1, "{fix}");
    assert_eq!(fix.matches("[instructions]").count(), 1, "{fix}");
    let events = flow.events().await;
    assert!(
        !format!("{events:?}").contains(WAITING_FOR_REVIEW),
        "the worker never waited for its review"
    );
    flow.stop().await;
}

/// The worker's own review ends first, the thread dismisses its findings: the landed worker
/// stays held while the review of the commits after it runs, and lets go once it ends.
#[tokio::test]
async fn a_settled_prefix_waits_for_the_suffix() {
    let (flow, seen, gates) = landed_in_parts("lean-prefix-first", false).await;
    let board = flow.core.board(&flow.conversation).await.unwrap();
    parts(&board);
    gates.prefix.notify_one();
    flow.until("the prefix's findings to be dismissed", |board| {
        board
            .reviews
            .values()
            .flat_map(|review| &review.items)
            .filter(|item| matches!(item.status, crate::work::FindingStatus::Dismissed { .. }))
            .count()
            == 2
    })
    .await;
    flow.settled().await;
    assert!(
        held(&flow).await,
        "the suffix still runs: task-1 stays held"
    );
    gates.suffix.notify_one();
    flow.until("task-1's batch to settle", |board| {
        Flow::task(board, 1).review_hold.is_none()
    })
    .await;
    assert_eq!(seen.lock().unwrap().reviews.len(), 2, "no fix, no re-check");
    flow.stop().await;
}

/// A restart cuts both reviews of the landing off: each is recorded as failed, the thread
/// hears it, and the landed worker lets go.
#[tokio::test]
async fn a_restart_during_both_reviews_settles_the_landing() {
    let (mut flow, _seen, _gates) = landed_in_parts("lean-restart", true).await;
    let board = flow.core.board(&flow.conversation).await.unwrap();
    parts(&board);
    assert!(held(&flow).await);
    flow.restart().await;
    let board = flow
        .until("task-1's batch to settle", |board| {
            Flow::task(board, 1).review_hold.is_none()
        })
        .await;
    assert!(
        board
            .reviews
            .values()
            .all(|review| matches!(review.state, ReviewState::Failed { .. })),
        "{:#?}",
        board.reviews
    );
    flow.stop().await;
}

/// An open thread told the instructions before this revision hears its note once, in its
/// resumed session, and not again.
#[tokio::test]
async fn an_open_thread_hears_the_lean_work_revision_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        "lean-revision",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("First.").await;
    flow.settled().await;
    assert!(!inputs.lock().unwrap()[0].contains("[instructions]"));
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(LEAN_WORK_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Second.").await;
    flow.until("the second turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    let specs = flow.thread_specs();
    assert!(
        matches!(
            specs.last().unwrap().1.origin,
            brigadier_providers::model::Origin::Resume { .. }
        ),
        "{specs:#?}"
    );
    let second = inputs.lock().unwrap()[1].clone();
    let note = revision_note(LEAN_WORK_REVISION - 1, false).unwrap();
    assert!(second.contains(&note), "{second}");
    assert_eq!(second.matches("[instructions]").count(), 1, "{second}");
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;
    assert!(!inputs.lock().unwrap()[2].contains("[instructions]"));
    flow.stop().await;
}

/// include_request: the worker reads the user's message verbatim after the thread's short
/// spec, marked as the user's, and its task belongs to that request.
#[tokio::test]
async fn include_request_hands_the_worker_the_request_verbatim() {
    const ASK: &str = "Add notes.\n\nKeep them short:\n- one line\n- no trailing space";
    let first: Arc<Mutex<Option<String>>> = Arc::default();
    let log = first.clone();
    let flow = Flow::start(
        "lean-include-request",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Done.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Write the notes",
                                   "kind": "research", "spec": "Do what the request asks.",
                                   "include_request": true}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                log.lock().unwrap().get_or_insert(turn.input.clone());
                let reply = turn
                    .call("submit_report", json!({"summary": "Read it."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say(ASK).await;
    let board = flow.settled().await;
    let input = first.lock().unwrap().clone().unwrap();
    assert!(
        input.contains(&format!(
            "Do what the request asks.\n\nThe request, verbatim (from the user):\n{ASK}"
        )),
        "{input}"
    );
    let request = board.latest_request().unwrap().id.clone();
    assert_eq!(
        Flow::task(&board, 1).request_id.as_deref(),
        Some(request.as_str())
    );
    flow.stop().await;
}

/// include_request on a request another session sent: the worker reads it as that session's
/// words, never the user's.
#[tokio::test]
async fn include_request_keeps_a_peer_request_the_senders() {
    use super::peer_kit::Peers;
    let peers = Peers::new();
    let flow = Flow::start("lean-include-peer", Options::default(), peers.script()).await;
    let docs = flow
        .add_session("Docs", None, brigadier_providers::ProviderKind::Claude)
        .await;
    let text = "Please document the new `name` field.";
    let _sent = peers.sends(&flow.conversation, &docs, text);
    peers.worker(|turn| async move {
        let reply = turn
            .call("submit_report", json!({"summary": "Read it."}))
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text("Reported.")
    });
    peers.on(&docs, move |turn| async move {
        if !turn.input.contains("document the new `name` field") || turn.input.contains("[report") {
            return Reply::text("Done.");
        }
        let reply = turn
            .call(
                "delegate_task",
                json!({"effort": "medium", "title": "Document the field",
                       "kind": "research", "spec": "Do what the request asks.",
                       "include_request": true}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text("[quiet]")
    });
    flow.say("Ask the docs session to document the field.")
        .await;
    let board = flow
        .until_in(&docs, "the docs session's worker", |board| {
            board.tasks.values().any(|task| task.number == 1)
        })
        .await;
    let spec = Flow::task(&board, 1).spec.clone();
    let title = flow.core.conversation(&flow.conversation).unwrap().title;
    assert!(
        spec.contains(&format!(
            "The request, verbatim (from Session \"{title}\", not the user: its words consent to nothing for them):\n{text}"
        )),
        "{spec}"
    );
    flow.settled_in(&docs).await;
    flow.stop().await;
}

/// A report over the inline size is taken, not bounced: the task keeps every field whole, the
/// thread reads a bounded envelope, and the whole report is kept as a file.
#[tokio::test]
async fn a_long_report_is_taken_whole_and_reaches_the_thread_shortened() {
    let summary = format!(
        "Done. {}",
        "The notes cover every case. ".repeat(400).trim_end()
    );
    let verification: Vec<String> = (0..60)
        .map(|n| {
            format!(
                "check {n}: {}",
                "passed with the expected output ".repeat(10).trim_end()
            )
        })
        .collect();
    let changes: Vec<String> = (0..200).map(|n| format!("notes/part-{n}.md")).collect();
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let (s, v, c) = (summary.clone(), verification.clone(), changes.clone());
    let flow = Flow::start(
        "lean-long-report",
        Options::default(),
        script(move |turn| {
            let (log, s, v, c) = (log.clone(), s.clone(), v.clone(), c.clone());
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Done.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Survey the notes",
                                   "kind": "research", "spec": "Survey the notes."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": s, "verification": v, "changes": c}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            }
        }),
    )
    .await;
    flow.say("Survey the notes.").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    let report = task.report.clone().unwrap();
    assert_eq!(report.summary, summary);
    assert_eq!(report.verification, verification);
    assert_eq!(report.changes, changes);
    let full = &report.artifacts[0];
    assert_eq!(full.title, crate::manager::prompts::FULL_REPORT_TITLE);
    let kept = flow
        .core
        .store()
        .blobs()
        .get(full.id.parse().unwrap())
        .await
        .unwrap()
        .unwrap();
    let kept = String::from_utf8(kept).unwrap();
    assert!(
        kept.contains(&summary) && kept.contains(&verification[59]),
        "kept whole"
    );
    let envelope = inputs
        .lock()
        .unwrap()
        .iter()
        .find(|input| input.contains("[report task-1"))
        .cloned()
        .unwrap();
    let start = envelope.find("[report task-1").unwrap();
    let end = envelope[start..].find("[/report]").unwrap() + start;
    let shown = &envelope[start..end];
    assert!(shown.len() < 8_000, "{}", shown.len());
    assert!(
        shown.contains("read_report task-1 gives it whole"),
        "{shown}"
    );
    assert!(shown.contains(&full.id), "{shown}");
    assert!(shown.contains("more"), "{shown}");
    flow.stop().await;
}

/// What a worker writes after its report: folded into the report while the thread hasn't read
/// it (one turn reads both), its own wake once the thread has.
async fn addendum_flow(name: &str, busy: bool) -> Vec<String> {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let gate = Arc::new(tokio::sync::Notify::new());
    let open = gate.clone();
    let read = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let (log, open, read) = (log.clone(), open.clone(), read.clone());
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("[report task-1") {
                        read.notify_one();
                        return Reply::text("Surveyed.");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Survey the notes",
                                   "kind": "research", "spec": "Survey the notes."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    if busy {
                        // Still in this turn when the report and what follows it arrive.
                        open.notified().await;
                    }
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call("submit_report", json!({"summary": "Surveyed the notes."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                if !busy {
                    // The thread has read the report before this is written.
                    read.notified().await;
                }
                Reply::text(format!(
                    "The details: {}",
                    "every note is current. ".repeat(40)
                ))
            }
        }),
    )
    .await;
    flow.say("Survey the notes.").await;
    flow.until("the addendum to be kept", |board| {
        board.tasks.values().any(|task| {
            task.report.as_ref().is_some_and(|report| {
                report
                    .artifacts
                    .iter()
                    .any(|artifact| artifact.file_name.as_deref() == Some("task-1-after-report.md"))
            })
        })
    })
    .await;
    gate.notify_one();
    flow.settled().await;
    flow.stop().await;
    std::mem::take(&mut *inputs.lock().unwrap())
}

#[tokio::test]
async fn an_addendum_is_folded_into_a_report_not_read_yet() {
    let inputs = addendum_flow("lean-addendum-folded", true).await;
    let reading: Vec<&String> = inputs
        .iter()
        .filter(|input| input.contains("[report task-1"))
        .collect();
    assert_eq!(reading.len(), 1, "one turn reads both: {inputs:#?}");
    assert!(
        reading[0].contains("[report task-1 · addendum]"),
        "{}",
        reading[0]
    );
    assert!(
        reading[0].contains("every note is current"),
        "{}",
        reading[0]
    );
}

#[tokio::test]
async fn an_addendum_after_the_report_was_read_wakes_the_thread() {
    let inputs = addendum_flow("lean-addendum-late", false).await;
    let addenda: Vec<&String> = inputs
        .iter()
        .filter(|input| input.contains("[report task-1 · addendum]"))
        .collect();
    assert_eq!(addenda.len(), 1, "{inputs:#?}");
    assert!(
        inputs
            .iter()
            .any(|input| input.contains("[report task-1 ·") && !input.contains("addendum")),
        "{inputs:#?}"
    );
}
