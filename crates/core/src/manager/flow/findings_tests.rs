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
                    // Big enough for an other-vendor review (BUILD-PLAN B2).
                    commit_in_workspace(
                        &turn,
                        "notes.txt",
                        &format!("notes \n{}", super::thread_tests::reviewed_notes()),
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
                        // How to judge them comes with them (BUILD-PLAN O2).
                        "is no bug: dismiss it with that reason",
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
    let recorded = replies.clone();
    let board = flow
        .until("the thread to record the findings", move |board| {
            !recorded.lock().unwrap().is_empty()
                && board.reviews.values().any(|review| {
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
                    why: None,
                    checked: None
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
        "Recorded: R1.1 fixed (unverified: not checked in the merged code), R1.2 open, R1.3 dismissed: the file is generated. Unverified: R1.1. The user sees them as unverified, not as clean: tell them so plainly, or check each in the merged code and record it again with `checked`."
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
    // R1.1 was recorded fixed on no check: the merge says it is unverified, never clean.
    assert!(
        merged.contains(
            "Recorded fixed but never checked in the merged code (unverified): R1.1 Old notes no longer load."
        ),
        "{merged}"
    );
    assert!(!merged.contains("R1.3"), "{merged}");
    assert!(!merged.contains("all fixed or dismissed"), "{merged}");
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

/// A fix of the thread's own goes with run_check's `fixes` (BUILD-PLAN O4): a failing check
/// commits and records nothing; a passing one commits the fix with the Fixes trailer and records
/// those findings fixed, checked by that check, with no git commit or record_findings call of
/// the thread's own. The finding it didn't name stays unrecorded, and so does one a passing check
/// names without a commit; an id that is no finding, or one in `high` but not in `fixes`, is
/// refused before anything runs, and one in `high` is marked high.
#[tokio::test]
async fn a_passing_check_with_fixes_commits_the_fix_and_records_it_checked() {
    // Every run_check reply and every later thread input, so a check's result is there
    // whether it came in the call or, past the foreground limit, from the background.
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = replies.clone();
    let tree: Arc<Mutex<Option<std::path::PathBuf>>> = Arc::default();
    let workspace_seen = tree.clone();
    // Each check is held in its call whatever the machine's load, so the next starts after it.
    let conversation: Arc<Mutex<Option<crate::model::ConversationId>>> = Arc::default();
    let held = conversation.clone();
    let flow = Flow::start(
        "findings-fixes",
        Options {
            reviews: Some(script(|_| async { Reply::text(THREE) })),
            ..Options::default()
        },
        script(move |turn| {
            let (seen, workspace_seen, held) = (seen.clone(), workspace_seen.clone(), held.clone());
            async move {
                let hold = || {
                    let id = held.lock().unwrap().clone().expect("the session");
                    super::super::background_runs::hold_foreground(
                        &id,
                        Arc::new(tokio::sync::Notify::new()),
                    );
                };
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(
                        &turn,
                        "notes.txt",
                        &format!("notes \n{}", super::thread_tests::reviewed_notes()),
                        "Add notes\n\nBrigadier-Author: thread",
                    );
                    return Reply::text("Added NOTES.md.");
                }
                seen.lock().unwrap().push(turn.input.clone());
                // The first review's findings; the re-check of the high one's fix comes after.
                if turn.input.contains("[review of your commits · Codex found 3")
                    && workspace_seen.lock().unwrap().is_none()
                {
                    assert!(turn.input.contains("run_check's `commit` and `fixes`"));
                    let workspace = turn.add_dirs[0].clone();
                    *workspace_seen.lock().unwrap() = Some(workspace.clone());
                    std::fs::write(workspace.join("notes.txt"), "fixed notes\n").unwrap();
                    // An id that is no finding here is refused before anything runs or commits,
                    // so the call can be made again with the right ids.
                    hold();
                    let refused = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "commit": "Fix the notes", "fixes": ["R1.1", "R9.9"]}),
                        )
                        .await;
                    assert!(refused.is_error, "{}", refused.text);
                    assert!(
                        refused.text.contains("R9.9 is no review finding of this session: nothing ran or was committed."),
                        "{}",
                        refused.text
                    );
                    assert_eq!(
                        super::git(&workspace, &["status", "--porcelain"]).trim(),
                        "M notes.txt"
                    );
                    // `high` names only some of `fixes`.
                    hold();
                    let refused = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "commit": "Fix the notes", "fixes": ["R1.2"], "high": ["R1.1"]}),
                        )
                        .await;
                    assert!(refused.is_error, "{}", refused.text);
                    assert!(
                        refused.text.contains("R1.1 is in `high` but not in `fixes`: nothing ran or was committed."),
                        "{}",
                        refused.text
                    );
                    for (command, fixes, high) in [
                        ("false", json!(["r1.1", "R1.2"]), json!([])),
                        ("true", json!("R1.1, R1.2"), json!("r1.1")),
                    ] {
                        hold();
                        let reply = turn
                            .call(
                                "run_check",
                                json!({"command": command, "commit": "Fix the notes", "fixes": fixes, "high": high}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        seen.lock().unwrap().push(reply.text);
                    }
                    // With nothing left to commit, no commit carries the Fixes trailer, so
                    // nothing is recorded fixed either.
                    hold();
                    let reply = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "commit": "Fix the notes again", "fixes": ["R1.3"]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    seen.lock().unwrap().push(reply.text);
                    // On the session's branch a passing check with no commit records nothing
                    // fixed: the merge takes the branch's commits, not the working tree.
                    std::fs::write(workspace.join("notes.txt"), "fixed notes, trimmed\n").unwrap();
                    hold();
                    let reply = turn
                        .call("run_check", json!({"command": "true", "fixes": ["R1.3"]}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    seen.lock().unwrap().push(reply.text);
                    return Reply::text("Fixed two.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    *conversation.lock().unwrap() = Some(flow.conversation.clone());
    flow.say("Add notes.").await;
    let recorded = replies.clone();
    let board = flow
        .until("the fix's check to record its findings", move |board| {
            // Both checks' replies, each in the call or from the background, the passing
            // one's after its record.
            let said = recorded.lock().unwrap();
            [
                "Not committed: the check didn't pass.",
                "Recorded: R1.1 fixed",
                "Nothing to commit",
                "on this branch a fix counts once",
            ]
            .iter()
            .all(|text| said.iter().any(|said| said.contains(text)))
                && board.reviews.values().any(|review| {
                    review
                        .items
                        .iter()
                        .filter(|item| matches!(item.status, FindingStatus::Fixed { .. }))
                        .count()
                        == 2
                })
        })
        .await;
    let said = |text: &str| {
        let seen = replies.lock().unwrap().clone();
        seen.iter()
            .find(|said| said.contains(text))
            .cloned()
            .unwrap_or_else(|| panic!("nothing said {text:?}: {seen:#?}"))
    };
    let failed = said("Not committed: the check didn't pass.");
    assert!(
        failed.contains("Not recorded fixed: R1.1, R1.2"),
        "{failed}"
    );
    let clean = said("Nothing to commit");
    assert!(
        clean.contains("Not recorded fixed: R1.3, as nothing was committed."),
        "{clean}"
    );
    let uncommitted = said("on this branch a fix counts once");
    assert!(
        uncommitted.contains("Not recorded fixed: R1.3, as nothing was committed: on this branch a fix counts once run_check's `commit` commits it."),
        "{uncommitted}"
    );
    let passed = said("Recorded: R1.1 fixed");
    assert!(passed.contains("R1.2 fixed"), "{passed}");
    assert!(passed.contains("Still not recorded: R1.3."), "{passed}");
    // The findings are recorded fixed only after the fix's commit was made.
    let workspace = tree.lock().unwrap().clone().expect("the fix turn ran");
    let body = super::git(&workspace, &["log", "-1", "--format=%B"]);
    assert!(body.starts_with("Fix the notes"), "{body}");
    assert!(body.contains("Brigadier-Author: thread"), "{body}");
    assert!(body.contains("Brigadier-Fixes: R1.1, R1.2"), "{body}");
    // R1.1 was marked high, so its fix commit gets the one targeted re-check.
    let fix = super::git(&workspace, &["rev-parse", "HEAD"]);
    flow.until("the high finding's re-check", move |board| {
        board
            .reviews
            .values()
            .any(|review| review.number == 2 && review.tip == fix)
    })
    .await;
    // The board's reviews are a map: take the first review's, R1.
    let review = board
        .reviews
        .values()
        .find(|review| review.items.iter().any(|item| item.id.starts_with("R1.")))
        .unwrap();
    for item in &review.items {
        match (item.id.as_str(), &item.status) {
            (
                "R1.1" | "R1.2",
                FindingStatus::Fixed {
                    task: None,
                    checked: Some(checked),
                    ..
                },
            ) => {
                assert!(
                    checked.starts_with("`true` passed after the fix [evidence: out-"),
                    "{checked}"
                );
                // The one named in `high` is marked high, so its fix gets its re-check.
                assert_eq!(item.high, item.id == "R1.1", "{}", item.id);
            }
            ("R1.3", FindingStatus::New) => {}
            (id, status) => panic!("{id}: {status:?}"),
        }
    }
    flow.settled().await;
    flow.stop().await;
}
