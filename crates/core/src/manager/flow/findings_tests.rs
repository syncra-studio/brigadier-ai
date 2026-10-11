//! Review findings as items (SESSION-FIXES-PLAN.md Q7): Brigadier splits a review into
//! findings, the thread records what became of each with `record_findings`, and the merge's
//! result lists only the open ones it recorded.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn};
use crate::work::{FindingStatus, OrchestratorStepKind, ReviewState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const THREE: &str = "Full review comments:\n\n- [P1] Old notes no longer load — NOTES.md:1\n  Keep the old format.\n- [P2] NOTES.md has no title — NOTES.md:1\n  Add one.\n- [P3] Trailing space — NOTES.md:2\n  Trim it.";

#[tokio::test]
async fn a_reviews_findings_are_items_the_thread_records_and_the_merge_lists_the_open_ones() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = replies.clone();
    let flow = Flow::start(
        "findings",
        Options {
            reviews: Some(script(|_| async { Reply::text(THREE) })),
            ..Options::default()
        },
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(
                        &turn,
                        "NOTES.md",
                        "notes \n",
                        "Add notes\n\nBrigadier-Author: thread",
                    );
                    return Reply::text("Added NOTES.md.");
                }
                if turn.input.contains("[review of your commits · Codex found 3") {
                    // The message lists each finding by id and asks for one call.
                    for line in [
                        "R1.1: [P1] Old notes no longer load — NOTES.md:1",
                        "R1.2: [P2] NOTES.md has no title — NOTES.md:1",
                        "R1.3: [P3] Trailing space — NOTES.md:2",
                        "record_findings",
                        // Bug 4: after the answer, one update line at most.
                        crate::manager::prompts::LATE_EVENT,
                    ] {
                        assert!(turn.input.contains(line), "{line} in {}", turn.input);
                    }
                    // An unknown id, or a dismissal without why, records nothing.
                    let wrong = turn
                        .call(
                            "record_findings",
                            json!({"items": [
                                {"id": "R1.1", "status": "fixed"},
                                {"id": "R9.9", "status": "open"},
                                {"id": "R1.3", "status": "dismissed"},
                            ]}),
                        )
                        .await;
                    assert!(wrong.is_error, "{}", wrong.text);
                    assert!(wrong.text.contains("R9.9 is no finding"), "{}", wrong.text);
                    assert!(wrong.text.contains("R1.3: say why"), "{}", wrong.text);
                    let reply = turn
                        .call(
                            "record_findings",
                            json!({"items": [
                                {"id": "r1.1", "status": "fixed"},
                                {"id": "R1.2", "status": "open", "why": "the title waits for the user's wording", "high": false},
                                {"id": "R1.3", "status": "dismissed", "why": "the file is generated"},
                            ]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    seen.lock().unwrap().push(reply.text);
                    return Reply::text("Recorded.");
                }
                if turn.input.contains("Merge it.") {
                    let reply = turn
                        .call("finish_session", json!({"user_words": "Merge it."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    seen.lock().unwrap().push(reply.text);
                    return Reply::text("Merged.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let board = flow
        .until("the thread to record the findings", |board| {
            board.reviews.values().any(|review| {
                review
                    .items
                    .iter()
                    .all(|item| item.status != FindingStatus::New)
                    && !review.items.is_empty()
            })
        })
        .await;
    let review = board.reviews.values().next().unwrap();
    assert_eq!(review.number, 1);
    assert_eq!(review.state, ReviewState::Findings { count: 3 });
    let statuses: Vec<_> = review
        .items
        .iter()
        .map(|item| {
            (
                item.id.as_str(),
                item.severity.as_deref(),
                item.status.clone(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        vec![
            (
                "R1.1",
                Some("P1"),
                FindingStatus::Fixed {
                    task: None,
                    why: None
                }
            ),
            (
                "R1.2",
                Some("P2"),
                FindingStatus::Open {
                    why: Some("the title waits for the user's wording".into())
                }
            ),
            (
                "R1.3",
                Some("P3"),
                FindingStatus::Dismissed {
                    why: "the file is generated".into()
                }
            ),
        ]
    );
    assert_eq!(
        replies.lock().unwrap()[0],
        "Recorded: R1.1 fixed, R1.2 open, R1.3 dismissed: the file is generated."
    );
    flow.settled().await;
    flow.say("Merge it.").await;
    flow.until("the merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
    })
    .await;
    flow.settled().await;
    let merged = replies.lock().unwrap()[1].clone();
    assert!(
        merged.contains(
            "Open review findings, as you recorded them (not fixed): R1.2 NOTES.md has no title (the title waits for the user's wording)."
        ),
        "{merged}"
    );
    assert!(
        !merged.contains("R1.1") && !merged.contains("R1.3"),
        "{merged}"
    );
    assert!(!merged.contains("found 3"), "{merged}");
    flow.stop().await;
}

/// §6, "Instruction changes reach open sessions": a thread whose CLI was told the revision
/// before record_findings (an open session when the new build starts) resumes its same session
/// on its next turn, with its new tool list, and hears the revision's note once.
#[tokio::test]
async fn an_open_thread_resumes_with_record_findings_and_its_note() {
    use brigadier_providers::model::Origin;
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let mut flow = Flow::start(
        "findings-revision",
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
    let added = crate::manager::prompts::REVISIONS
        .iter()
        .find(|revision| revision.thread.starts_with("You now have record_findings"))
        .unwrap();
    // What the build before logged: its instructions were one revision older.
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
                            revision: Some(added.number - 1),
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
        matches!(specs.last().unwrap().1.origin, Origin::Resume { .. }),
        "{specs:#?}"
    );
    let second = inputs.lock().unwrap()[1].clone();
    assert!(
        second.contains(&format!("[instructions] {}", added.thread)),
        "{second}"
    );
    flow.say("Third.").await;
    flow.until("the third turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    flow.settled().await;
    let third = inputs.lock().unwrap()[2].clone();
    assert!(!third.contains("[instructions]"), "{third}");
    flow.stop().await;
}
