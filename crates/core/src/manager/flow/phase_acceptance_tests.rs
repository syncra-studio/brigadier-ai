//! An overnight phase settles done only with evidence per criterion at its tip (BUILD-PLAN E2,
//! research A5(a)), with reviews on and with reviews off.
//!
//! The run's one phase has three criteria. Its lead's report meets the first with lines its
//! file still shows and the second with a check that passed on the tree that lands, and names
//! nothing for the third, which no suite covers. The thread's first settle cites a check it ran
//! before the work landed: refused, the criterion isn't bound to the tip. Its own check at the
//! tip then settles it. The settlement and the morning report carry each criterion's evidence,
//! and the review's state: settled with reviews on, "No review was run" with them off.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::overnight::{OvernightState, ProposedPhase, ProposedPlan};
use crate::work::{PhaseStage, StepOutcome};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The `out-…` id a check's reply ends with.
fn cited(reply: &str) -> String {
    reply
        .rsplit_once("[evidence: ")
        .map(|(_, cite)| cite.trim_end().trim_end_matches(']').to_owned())
        .unwrap_or_else(|| panic!("no evidence reference: {reply}"))
}

#[derive(Default)]
struct Seen {
    /// The thread's check before the work landed.
    early: Option<String>,
    /// Each settle_step reply, and whether it was refused.
    settles: Vec<(bool, String)>,
}

fn plan() -> ProposedPlan {
    ProposedPlan {
        name: "Greeting".into(),
        phases: vec![ProposedPhase {
            number: Some(1),
            name: "Greet".into(),
            scope: "Create p1.txt.".into(),
            done_when: vec![
                "p1.txt exists.".into(),
                "The greeting check passes.".into(),
                "The thread checked it at the tip.".into(),
            ],
            ..Default::default()
        }],
        ..Default::default()
    }
}

async fn a_phase_settles_done_only_with_evidence_at_its_tip(reviews: bool) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        if reviews {
            "acceptance-on"
        } else {
            "acceptance-off"
        },
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    turn.write("p1.txt", "phase 1\n");
                    turn.git(&["add", "p1.txt"]);
                    turn.git(&["commit", "-qm", "Add p1.txt"]);
                    let check = turn
                        .call("run_check", json!({"command": "echo greeting ok"}))
                        .await;
                    assert!(!check.is_error, "{}", check.text);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added p1.txt.", "changes": ["p1.txt"],
                            "done_when": [
                                "[met] p1.txt exists: p1.txt:1 `phase 1`",
                                format!("[met] The greeting check passes: {}", cited(&check.text)),
                            ]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains("[overnight] The run") && turn.input.contains("has started")
                {
                    // A check of its own before anything landed: on another tree than the tip's.
                    let check = turn
                        .call("run_check", json!({"command": "echo thread checked"}))
                        .await;
                    assert!(!check.is_error, "{}", check.text);
                    log.lock().unwrap().early = Some(cited(&check.text));
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Phase 1", "kind": "implement",
                                   "spec": "Build phase 1: create p1.txt.", "provider": "claude",
                                   "phase": 1}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let landed = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!landed.is_error, "{}", landed.text);
                    let early = log.lock().unwrap().early.clone().unwrap();
                    let settle = |line: String| {
                        let turn = &turn;
                        async move {
                            turn.call(
                                "settle_step",
                                json!({"phase": 1, "outcome": "done",
                                       "summary": "Made p1.txt and checked each criterion.",
                                       "done_when": line}),
                            )
                            .await
                        }
                    };
                    let refused = settle(format!("[met] p1-c3: {early}")).await;
                    log.lock()
                        .unwrap()
                        .settles
                        .push((refused.is_error, refused.text));
                    let check = turn
                        .call("run_check", json!({"command": "echo thread checked"}))
                        .await;
                    assert!(!check.is_error, "{}", check.text);
                    let settled = settle(format!("[met] p1-c3: {}", cited(&check.text))).await;
                    log.lock()
                        .unwrap()
                        .settles
                        .push((settled.is_error, settled.text.clone()));
                    assert!(!settled.is_error, "{}", settled.text);
                    if !settled.text.contains("the run is ending now") {
                        let ended = turn
                            .call(
                                "end_run",
                                json!({"outcome": "done", "why": "The phase is settled."}),
                            )
                            .await;
                        assert!(!ended.is_error, "{}", ended.text);
                    }
                    return Reply::text("Good morning: the run is over; the report follows.");
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.review_work = reviews;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Hello.").await;
    flow.settled().await;

    let run = flow
        .manager
        .propose_overnight(
            flow.conversation.clone(),
            "propose-1".into(),
            "/overnight Greet.".into(),
            Some(plan()),
        )
        .await
        .unwrap();
    assert!(run.problems.is_empty(), "{:?}", run.problems);
    let run = flow
        .manager
        .start_overnight(
            flow.conversation.clone(),
            run.id.clone(),
            "start-1".into(),
            run.revision,
        )
        .await
        .unwrap();
    let board = flow
        .until("the run to finish", |board| {
            board
                .runs
                .get(&run.id)
                .is_some_and(|run| run.state == OvernightState::Finished)
        })
        .await;

    let settles = seen.lock().unwrap().settles.clone();
    assert_eq!(settles.len(), 2, "{settles:#?}");
    let (refused, why) = &settles[0];
    assert!(refused, "{why}");
    assert!(
        why.contains("p1-c3: [met] with no evidence that holds at the tip")
            && why.contains("ran on another tree than the tip's"),
        "{why}"
    );
    // The two criteria the report met are not what kept it.
    assert!(
        !why.lines()
            .any(|line| line.starts_with("- p1-c1") || line.starts_with("- p1-c2")),
        "{why}"
    );
    assert!(!settles[1].0, "{}", settles[1].1);

    let run = board.runs.get(&run.id).cloned().unwrap();
    let plan = board.plans.get(run.plan_id.as_ref().unwrap()).unwrap();
    assert_eq!(plan.steps[0].stage, PhaseStage::Done);
    let settled = plan.steps[0].settled.clone().unwrap();
    assert_eq!(settled.outcome, StepOutcome::Done);
    assert_eq!(run.verified_commit, settled.tip);
    let evidence = settled.evidence.join("\n");
    for line in [
        "p1-c1: p1.txt:1 shows `phase 1` at the tip",
        "p1-c2: out-",
        "(`echo greeting ok`) passed on the tip's tree",
        "p1-c3: out-",
        "(`echo thread checked`) passed on the tip's tree",
    ] {
        assert!(evidence.contains(line), "{line}: {evidence}");
    }
    let review = settled.review.clone().unwrap();
    if reviews {
        assert!(review.starts_with("Review settled:"), "{review}");
    } else {
        assert_eq!(review, "No review was run: reviews are off.");
    }

    // The morning report says what each criterion rests on, and the review's state.
    let id = run.report_message_id.clone().expect("a report");
    let head = flow.core.head(&flow.conversation).await.unwrap().unwrap();
    let messages = flow.core.branch(&flow.conversation, &head).await.unwrap();
    let message = messages.iter().find(|message| message.id == id).unwrap();
    let report = flow.manager.full_text(message).await;
    assert!(
        report.contains("Evidence: p1-c1: p1.txt:1 shows `phase 1` at the tip"),
        "{report}"
    );
    assert!(report.contains("Evidence: p1-c3: out-"), "{report}");
    if !reviews {
        assert!(
            report.contains("No review was run: reviews are off."),
            "{report}"
        );
    }
    flow.stop().await;
}

#[tokio::test]
async fn a_phase_settles_done_only_with_evidence_at_its_tip_with_reviews_on() {
    a_phase_settles_done_only_with_evidence_at_its_tip(true).await;
}

#[tokio::test]
async fn a_phase_settles_done_only_with_evidence_at_its_tip_with_reviews_off() {
    a_phase_settles_done_only_with_evidence_at_its_tip(false).await;
}
