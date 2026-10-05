//! Overnight runs on the request loop: each phase gets one lead, a fresh verifier with one
//! review, `land_phase` and `phase_done`; the run keeps its directives, deadline wind-down,
//! restart recovery and morning report.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::OvernightRunId;
use crate::overnight::{
    OvernightRun, OvernightState, PhaseState, ProposedPhase, ProposedPlan, StopReason,
};
use crate::work::{TaskState, WorkerRole};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const QUIET: &str = "[quiet]";

/// The phase an orchestrator input starts ("[overnight · phase N] Lead phase N …").
fn kickoff(input: &str) -> Option<u32> {
    let rest = input.split("[overnight · phase ").nth(1)?;
    let number: u32 = rest
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    rest.contains("Lead phase").then_some(number)
}

/// The phase a lead's brief names ("Build phase N").
fn phase_of_lead(turn: &Turn) -> Option<u32> {
    let rest = turn.prompt.split("Build phase ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
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

fn is_reviewer(turn: &Turn) -> bool {
    turn.prompt.contains("Kind: review")
}

fn is_verifier(turn: &Turn) -> bool {
    turn.prompt.contains("You verify this phase")
}

/// An orchestrator that leads each phase as its briefing says: one lead, then land the
/// verifier's work and settle the phase as done.
async fn lead_the_phase(turn: &Turn) -> Reply {
    if let Some(n) = kickoff(&turn.input) {
        let reply = turn
            .call(
                "delegate_task",
                json!({"title": format!("Phase {n}"), "kind": "implement",
                       "spec": format!("Build phase {n}: create p{n}.txt."),
                       "provider": "claude"}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        let note = if n == 1 {
            json!({"kind": "decided", "what": "Named the files pN.txt.", "why": "The plan says so."})
        } else {
            json!({"kind": "waiting", "what": "Add the release key to .env."})
        };
        let reply = turn.call("note_for_user", note).await;
        assert!(!reply.is_error, "{}", reply.text);
        return Reply::text(QUIET);
    }
    if turn.input.contains("[phase verifier]") {
        return Reply::text(QUIET);
    }
    if let Some(n) = reports_in(&turn.input).last() {
        let landed = turn
            .call("land_phase", json!({"task": format!("task-{n}")}))
            .await;
        if landed.is_error {
            return Reply::text(QUIET);
        }
        let reply = turn
            .call(
                "phase_done",
                json!({"outcome": "done",
                       "summary": "It made the file; its verifier checked it and one review found nothing."}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
    }
    Reply::text(QUIET)
}

/// A verifier that asks for its one review and reports every criterion met.
async fn verify(turn: &Turn) -> Reply {
    let review = turn.call("request_review", json!({})).await;
    assert!(!review.is_error, "{}", review.text);
    let reply = turn
        .call(
            "submit_report",
            json!({"summary": "Every criterion is met; the review found nothing.",
                   "done_when": "[met] the file exists: ls shows it"}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Verified.")
}

async fn review(turn: &Turn) -> Reply {
    let reply = turn
        .call("submit_report", json!({"summary": "No findings."}))
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reviewed.")
}

/// A lead that commits its phase's file and reports.
async fn build(turn: &Turn) -> Reply {
    let n = phase_of_lead(turn).expect("a phase lead");
    let file = format!("p{n}.txt");
    turn.write(&file, &format!("{n}\n"));
    turn.git(&["add", &file]);
    turn.git(&["commit", "-q", "-m", &format!("Add {file}")]);
    let reply = turn
        .call(
            "submit_report",
            json!({"summary": format!("Added {file}."), "changes": [file]}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

fn plan(phases: u32) -> ProposedPlan {
    ProposedPlan {
        name: "Three files".into(),
        phases: (1..=phases)
            .map(|n| ProposedPhase {
                number: Some(n),
                name: format!("File {n}"),
                scope: format!("Create p{n}.txt."),
                done_when: vec![format!("p{n}.txt exists.")],
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

/// Proposes and starts a run of `phases` phases from `words`.
async fn start_run(flow: &Flow, words: &str, phases: u32) -> OvernightRun {
    let run = flow
        .manager
        .propose_overnight(
            flow.conversation.clone(),
            "propose-1".into(),
            words.into(),
            Some(plan(phases)),
        )
        .await
        .unwrap();
    assert!(run.problems.is_empty(), "{:?}", run.problems);
    flow.manager
        .start_overnight(
            flow.conversation.clone(),
            run.id.clone(),
            "start-1".into(),
            run.revision,
        )
        .await
        .unwrap()
}

fn run_of(board: &Board, id: &OvernightRunId) -> OvernightRun {
    board.runs.get(id).cloned().expect("the run")
}

async fn finished(flow: &Flow, id: &OvernightRunId) -> Board {
    flow.until("the run to finish", |board| {
        board
            .runs
            .get(id)
            .is_some_and(|run| run.state == OvernightState::Finished)
    })
    .await
}

/// The text of the run's report message.
async fn report_text(flow: &Flow, run: &OvernightRun) -> String {
    let id = run.report_message_id.clone().expect("a report");
    let head = flow
        .core
        .head(&flow.conversation)
        .await
        .unwrap()
        .expect("a head");
    let messages = flow.core.branch(&flow.conversation, &head).await.unwrap();
    let message = messages
        .iter()
        .find(|message| message.id == id)
        .expect("the report message");
    flow.manager.full_text(message).await
}

/// Done when (1) and (4): "stop after phase 2" on a plan of three runs two phases, each with
/// one lead, one verifier (one review) and land_phase, and stops; phase 3 never starts. The
/// morning report has the delegator's sections.
#[tokio::test]
async fn a_run_told_to_stop_after_phase_2_verifies_two_phases_and_stops() {
    let flow = Flow::start(
        "overnight-stop",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                return lead_the_phase(&turn).await;
            }
            if is_reviewer(&turn) {
                return review(&turn).await;
            }
            if is_verifier(&turn) {
                return verify(&turn).await;
            }
            build(&turn).await
        }),
    )
    .await;
    let run = start_run(&flow, "/overnight Make three files. Stop after phase 2.", 3).await;
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    assert_eq!(run.stop, Some(StopReason::StopDirective), "{run:#?}");
    let states: Vec<_> = run.phases.iter().map(|phase| phase.state).collect();
    assert_eq!(
        states,
        [
            PhaseState::Verified,
            PhaseState::Verified,
            PhaseState::Pending
        ],
        "phase 3 is never reached"
    );
    for phase in &run.phases[..2] {
        let of = |role| {
            board
                .tasks
                .values()
                .filter(|task| {
                    task.role == Some(role)
                        && task
                            .run
                            .as_ref()
                            .and_then(|context| context.phase_id.as_deref())
                            == Some(phase.id.as_str())
                })
                .count()
        };
        assert_eq!(of(WorkerRole::Verifier), 1, "phase {}", phase.number);
        assert_eq!(of(WorkerRole::Lead), 1, "phase {}", phase.number);
        assert!(phase.verified_commit.is_some());
    }
    assert!(
        board
            .tasks
            .values()
            .filter(|task| task.role == Some(WorkerRole::Verifier))
            .all(|task| task.state == TaskState::Landed)
    );
    assert_eq!(run.verified_commit, run.phases[1].verified_commit);
    assert!(board.approvals.is_empty(), "no cards");
    let branch = run.workspace.as_ref().expect("a branch").branch.clone();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(
        files.contains("p1.txt") && files.contains("p2.txt"),
        "{files}"
    );
    assert!(!files.contains("p3.txt"), "{files}");

    let report = report_text(&flow, &run).await;
    for section in [
        "stopped where you asked",
        "### Phases",
        "Verified by",
        "### Commits",
        "Add p1.txt",
        "### Decided for you",
        "Named the files pN.txt.",
        "Code review: no findings.",
        "### Waiting on you",
        "Add the release key to .env.",
    ] {
        assert!(report.contains(section), "{section}\n{report}");
    }
    let shown = report.split("### Details").next().unwrap();
    assert!(
        !shown.contains("task-"),
        "workers by name, not number:\n{report}"
    );
    let phases = report.find("### Phases").unwrap();
    assert!(
        report[..phases].lines().filter(|l| !l.is_empty()).count() == 3,
        "three outcome lines first:\n{report}"
    );
    flow.stop().await;
}

/// Done when (2): a run "until" a time winds down 20 minutes early; at the deadline its live
/// lead is asked for a handoff in the delegator's headings, and the run ends at the deadline.
#[tokio::test]
async fn at_the_deadline_live_workers_are_asked_for_a_handoff() {
    let handed: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
    let heard = handed.clone();
    let flow = Flow::start(
        "overnight-deadline",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    return lead_the_phase(&turn).await;
                }
                if is_reviewer(&turn) {
                    return review(&turn).await;
                }
                if is_verifier(&turn) {
                    return verify(&turn).await;
                }
                // Mid-work when the run ends: it hears so in its running turn and hands off.
                let ending = loop {
                    let words = turn.steered().await.expect("told the run is ending");
                    if words.contains("The overnight run is ending") {
                        break words;
                    }
                };
                heard.lock().unwrap().push(ending);
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": "Goal and where it stands: half done. Done: nothing. Next steps: write p1.txt."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Handed off.")
            }
        }),
    )
    .await;
    let later = jiff::Zoned::now()
        .checked_add(jiff::Span::new().hours(2))
        .unwrap();
    let words = format!(
        "/overnight Make one file. Until {}.",
        later.strftime("%H:%M")
    );
    let run = start_run(&flow, &words, 1).await;
    let crate::overnight::Deadline::At { time } = &run.directives.deadline else {
        panic!("a deadline: {:?}", run.directives.deadline);
    };
    assert_eq!(run.wind_down_at_ms, Some(time.at_ms - 20 * 60_000));
    // The lead is at work.
    flow.until("the lead to work", |board| {
        board
            .tasks
            .values()
            .any(|task| task.role == Some(WorkerRole::Lead) && task.state == TaskState::Running)
    })
    .await;
    flow.manager
        .deadline_reached(&flow.conversation, &run.id)
        .await
        .unwrap();
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    assert_eq!(run.stop, Some(StopReason::Deadline));
    let handed = handed.lock().unwrap().clone();
    assert_eq!(handed.len(), 1, "one handoff request: {handed:?}");
    for heading in [
        "Goal and where it stands",
        "Done (with commit hashes)",
        "In progress",
        "Next steps",
        "Decisions and approvals already given",
        "Gotchas learned",
        "How to verify",
    ] {
        assert!(handed[0].contains(heading), "{heading}\n{}", handed[0]);
    }
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.role != Some(WorkerRole::Verifier)),
        "nothing is verified while the run ends"
    );
    assert_ne!(run.phases[0].state, PhaseState::Verified);
    flow.stop().await;
}

/// Done when (3): Brigadier restarts while phase 1's lead works; afterwards the phase goes
/// on to its verifier and landing, and the run finishes.
#[tokio::test]
async fn a_restart_mid_phase_resumes_the_run() {
    let restarted = Arc::new(AtomicBool::new(false));
    let first_turns = Arc::new(AtomicU32::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let (after, turns, released) = (restarted.clone(), first_turns.clone(), release.clone());
    let mut flow = Flow::start(
        "overnight-restart",
        Options::default(),
        script(move |turn| {
            let (after, turns, released) = (after.clone(), turns.clone(), released.clone());
            async move {
                if turn.is_orchestrator() {
                    if after.load(Ordering::SeqCst)
                        && turn.input.contains("Nothing of this phase runs now")
                    {
                        // Its lead ended with the restart: lead the phase again.
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"title": "Phase 1 again", "kind": "implement",
                                       "spec": "Build phase 1: create p1.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text(QUIET);
                    }
                    return lead_the_phase(&turn).await;
                }
                if is_reviewer(&turn) {
                    return review(&turn).await;
                }
                if is_verifier(&turn) {
                    return verify(&turn).await;
                }
                if !after.load(Ordering::SeqCst) {
                    // Mid-turn when Brigadier quits: this CLI is gone with it.
                    turns.fetch_add(1, Ordering::SeqCst);
                    released.notified().await;
                    return Reply::default();
                }
                build(&turn).await
            }
        }),
    )
    .await;
    let run = start_run(&flow, "/overnight Make one file.", 1).await;
    flow.until("the lead to work", |board| {
        board
            .tasks
            .values()
            .any(|task| task.role == Some(WorkerRole::Lead) && task.state == TaskState::Running)
    })
    .await;
    while first_turns.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    restarted.store(true, Ordering::SeqCst);
    flow.restart().await;
    release.notify_waiters();
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    assert_eq!(run.stop, Some(StopReason::Done), "{run:#?}");
    assert_eq!(run.phases[0].state, PhaseState::Verified);
    let branch = run.workspace.as_ref().expect("a branch").branch.clone();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(files.contains("p1.txt"), "{files}");
    flow.stop().await;
}
