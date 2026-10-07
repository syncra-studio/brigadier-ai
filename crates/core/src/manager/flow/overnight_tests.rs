//! Overnight runs on the session's own thread (THREAD-PLAN.md Q10): the run's phases become
//! the plan of its request and the thread hears of the run; it delegates each phase by its
//! source number, lands the work, settles each phase with `settle_step` and ends the run with
//! `end_run`. Code keeps the user's restrictions, the deadline's wind-down, restart recovery,
//! the switch of the thread's workspace to the run's worktree and back, and the morning report.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::model::Origin;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::{Environment, OvernightRunId, Setup};
use crate::overnight::{OvernightRun, OvernightState, ProposedPhase, ProposedPlan, StopReason};
use crate::work::{PhaseStage, Plan, ReviewKind, ReviewState, StepOutcome, TaskState, WorkerRole};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const QUIET: &str = "[quiet]";
const MORNING: &str = "Good morning: the run is over; the report follows.";

/// The task numbers of the reports in a thread's input, in order.
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

/// The phase a lead's brief names ("Build phase N").
fn phase_of_lead(turn: &Turn) -> Option<u32> {
    let rest = turn.prompt.split("Build phase ").nth(1)?;
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// How the scripted thread works through its run.
#[derive(Default)]
struct Thread {
    /// The phases it delegates, in order.
    order: Vec<u32>,
    /// Phases it tries to delegate at the start, which Brigadier must refuse.
    refused: Vec<u32>,
    /// It commits a tiny edit of its own in its workspace at the start.
    tiny_edit: bool,
    /// It tries to settle each phase before landing its lead's work (Brigadier refuses).
    early: bool,
    /// How it settles each phase (done when absent).
    outcomes: HashMap<u32, StepOutcome>,
    /// Each lead's phase, by task number.
    led: Mutex<HashMap<u32, u32>>,
    /// Each turn's input and workspace.
    turns: Mutex<Vec<(String, Vec<PathBuf>)>>,
}

impl Thread {
    async fn delegate(&self, turn: &Turn, phase: u32) {
        let reply = turn
            .call(
                "delegate_task",
                json!({"title": format!("Phase {phase}"), "kind": "implement",
                       "spec": format!("Build phase {phase}: create p{phase}.txt."),
                       "provider": "claude", "phase": phase}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        let task = reply
            .text
            .split("task-")
            .nth(1)
            .and_then(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .and_then(|number| number.parse().ok())
            .expect("the task's number");
        self.led.lock().unwrap().insert(task, phase);
    }

    /// One turn of the thread.
    async fn turn(&self, turn: &Turn) -> Reply {
        self.turns
            .lock()
            .unwrap()
            .push((turn.input.clone(), turn.add_dirs.clone()));
        if turn.input.contains("[overnight] The run") && turn.input.contains("has started") {
            for phase in &self.refused {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"title": "No", "kind": "implement",
                               "spec": format!("Build phase {phase}."), "phase": phase}),
                    )
                    .await;
                assert!(reply.is_error, "phase {phase} is refused: {}", reply.text);
            }
            if self.tiny_edit {
                let workspace = &turn.add_dirs[0];
                std::fs::write(workspace.join("notes.txt"), "notes\n").unwrap();
                super::git(workspace, &["add", "notes.txt"]);
                super::git(
                    workspace,
                    &[
                        "commit",
                        "-q",
                        "-m",
                        "Add notes\n\nBrigadier-Author: thread",
                    ],
                );
            }
            for note in [
                json!({"kind": "decided", "what": "Named the files pN.txt.", "why": "The plan says so."}),
                json!({"kind": "waiting", "what": "Add the release key to .env."}),
            ] {
                let reply = turn.call("note_for_user", note).await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            if let Some(first) = self.order.first() {
                self.delegate(turn, *first).await;
            }
            return Reply::text(QUIET);
        }
        let mut ending = turn.input.contains("The overnight run is ending now");
        let led: Vec<(u32, u32)> = reports_in(&turn.input)
            .into_iter()
            .filter_map(|task| Some((task, *self.led.lock().unwrap().get(&task)?)))
            .collect();
        for (task, phase) in led {
            if self.early {
                // Not settled while its lead's work isn't landed.
                let early = turn
                    .call(
                        "settle_step",
                        json!({"phase": phase, "outcome": "done", "summary": "Too early."}),
                    )
                    .await;
                assert!(
                    early.is_error && early.text.contains("still has work going"),
                    "{}",
                    early.text
                );
            }
            let landed = turn
                .call("land_phase", json!({"task": format!("task-{task}")}))
                .await;
            let outcome = match self.outcomes.get(&phase) {
                Some(outcome) => *outcome,
                None if landed.is_error => StepOutcome::Partial,
                None => StepOutcome::Done,
            };
            if landed.is_error {
                // Nothing of it landed (a handoff): what is left is said.
                let _ = turn
                    .call("stop_worker", json!({"task": format!("task-{task}")}))
                    .await;
            }
            let settled = turn
                .call(
                    "settle_step",
                    json!({"phase": phase,
                           "outcome": match outcome {
                               StepOutcome::Done => "done",
                               StepOutcome::Partial => "partial",
                               StepOutcome::Blocked => "blocked",
                           },
                           "summary": format!("Made p{phase}.txt; `ls` shows it; its review found nothing."),
                           "left": if outcome == StepOutcome::Done { String::new() } else { format!("write p{phase}.txt") }}),
                )
                .await;
            assert!(!settled.is_error, "{}", settled.text);
            if settled.text.contains("the run is ending now") {
                ending = true;
                continue;
            }
            if ending {
                continue;
            }
            let next = self
                .order
                .iter()
                .skip_while(|n| **n != phase)
                .nth(1)
                .copied();
            match next {
                Some(next) => self.delegate(turn, next).await,
                None => {
                    let ended = turn
                        .call(
                            "end_run",
                            json!({"outcome": "done", "why": "Every phase is settled."}),
                        )
                        .await;
                    assert!(!ended.is_error, "{}", ended.text);
                    ending = true;
                }
            }
        }
        Reply::text(if ending { MORNING } else { QUIET })
    }
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
            json!({"summary": format!("Added {file}."), "changes": [file],
                   "done_when": format!("[met] p{n}.txt exists: ls shows it")}),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    Reply::text("Reported.")
}

/// A flow whose thread works as `thread` says and whose leads build their phase.
async fn flow_with(name: &str, thread: Arc<Thread>) -> Flow {
    Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let thread = thread.clone();
            async move {
                if turn.is_orchestrator() {
                    return thread.turn(&turn).await;
                }
                build(&turn).await
            }
        }),
    )
    .await
}

fn plan(phases: u32) -> ProposedPlan {
    ProposedPlan {
        name: "Files".into(),
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

fn plan_of<'a>(board: &'a Board, run: &OvernightRun) -> &'a Plan {
    board
        .plans
        .get(run.plan_id.as_ref().expect("the run's plan"))
        .expect("the plan")
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

fn session_worktree(flow: &Flow) -> (PathBuf, String) {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (PathBuf::from(path), branch),
        other => panic!("no session worktree: {other:?}"),
    }
}

fn real(path: &std::path::Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// "stop after phase 2" on a plan of three: the thread works on its own session, its
/// workspace switched to the run's worktree (its tiny edit lands on the run's branch, the
/// session's branch stays as it was) and back after; phase 3 never starts; Merge takes the
/// tip the thread accepted; the morning report lists commits, reviews, decisions, waiting
/// items and usage.
#[tokio::test]
async fn a_run_told_to_stop_after_phase_2_settles_two_phases_and_stops() {
    let thread = Arc::new(Thread {
        order: vec![1, 2, 3],
        refused: vec![3],
        tiny_edit: true,
        early: true,
        ..Default::default()
    });
    let flow = flow_with("overnight-stop", thread.clone()).await;
    flow.say("Hello.").await;
    flow.until("the first turn", |_| {
        !thread.turns.lock().unwrap().is_empty()
    })
    .await;
    flow.settled().await;
    let (session, session_branch) = session_worktree(&flow);
    let session_tip = super::git(&flow.repo, &["rev-parse", &session_branch]);

    let run = start_run(&flow, "/overnight Make three files. Stop after phase 2.", 3).await;
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    assert_eq!(run.stop, Some(StopReason::StopDirective), "{run:#?}");
    let workspace = run.workspace.clone().expect("a branch");

    // The plan: phases 1 and 2 settled done, phase 3 never started.
    let plan = plan_of(&board, &run);
    let stages: Vec<_> = plan.steps.iter().map(|step| step.stage).collect();
    assert_eq!(
        stages,
        [PhaseStage::Done, PhaseStage::Done, PhaseStage::Pending],
        "{plan:#?}"
    );
    assert!(plan.steps[2].settled.is_none());
    assert_eq!(
        run.verified_commit,
        plan.steps[1].settled.as_ref().and_then(|s| s.tip.clone()),
        "Merge takes the tip the thread accepted"
    );
    assert!(
        board
            .tasks
            .values()
            .filter(|task| task.role == Some(WorkerRole::Lead))
            .all(|task| task.phase != Some(3)),
        "nothing started for phase 3"
    );
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.role != Some(WorkerRole::Verifier)),
        "no verifier starts by itself"
    );

    // The thread's tiny edit and the leads' work are on the run's branch; the session's branch
    // is unchanged.
    let files = super::git(
        &flow.repo,
        &["ls-tree", "-r", "--name-only", &workspace.branch],
    );
    for file in ["notes.txt", "p1.txt", "p2.txt"] {
        assert!(files.contains(file), "{file}: {files}");
    }
    assert!(!files.contains("p3.txt"), "{files}");
    assert_eq!(
        super::git(&flow.repo, &["rev-parse", &session_branch]),
        session_tip,
        "the session's branch is unchanged"
    );
    let notes = super::git(
        &flow.repo,
        &[
            "log",
            "-1",
            "--format=%H",
            &workspace.branch,
            "--",
            "notes.txt",
        ],
    );
    assert!(
        board
            .reviews
            .values()
            .any(|review| review.kind == ReviewKind::Code && review.tip == notes.trim()),
        "the thread's commit gets its review: {:#?}",
        board.reviews
    );

    // The thread worked from the run's worktree during the run.
    let turns = thread.turns.lock().unwrap().clone();
    let during: Vec<_> = turns
        .iter()
        .filter(|(input, _)| input.contains("[overnight]") || input.contains("[report task-"))
        .collect();
    assert!(!during.is_empty());
    for (_, dirs) in &during {
        assert_eq!(
            dirs.iter().map(|dir| real(dir)).collect::<Vec<_>>(),
            vec![real(std::path::Path::new(&workspace.path))]
        );
    }

    // The report.
    let report = report_text(&flow, &run).await;
    for section in [
        "stopped where you asked",
        "2 of 3 phases done.",
        "Merge takes phases 1 and 2",
        "### Phases",
        "✓ Phase 1 · File 1: done. 1 task landed.",
        "Phase 3 · File 3: not reached.",
        "### Commits",
        "Add p1.txt",
        "Add notes",
        "review",
        "### Decided for you",
        "Named the files pN.txt.",
        "### Waiting on you",
        "Add the release key to .env.",
        "How each phase was checked:",
        "Settled: Made p1.txt",
        "Usage: ",
    ] {
        assert!(report.contains(section), "{section}\n{report}");
    }
    let shown = report.split("### Details").next().unwrap();
    assert!(
        !shown.contains("task-"),
        "workers by name, not number:\n{report}"
    );

    // After the run the same native session goes on, in the session's checkout.
    flow.say("Thanks.").await;
    flow.until("the turn after the run", |_| {
        thread
            .turns
            .lock()
            .unwrap()
            .last()
            .is_some_and(|(input, _)| input.contains("Thanks."))
    })
    .await;
    let after = thread.turns.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        after.1.iter().map(|dir| real(dir)).collect::<Vec<_>>(),
        vec![real(&session)]
    );
    let specs = flow.thread_specs();
    assert_eq!(specs[0].1.origin, Origin::New, "{specs:#?}");
    let resumed: Vec<_> = specs[1..].iter().map(|(_, spec)| &spec.origin).collect();
    assert!(resumed.len() >= 2, "{specs:#?}");
    assert!(
        resumed
            .iter()
            .all(|origin| matches!(origin, Origin::Resume { .. }) && *origin == resumed[0]),
        "one native session throughout: {resumed:#?}"
    );
    assert_eq!(
        specs
            .last()
            .unwrap()
            .1
            .add_dirs
            .iter()
            .map(|d| real(d))
            .collect::<Vec<_>>(),
        vec![real(&session)]
    );
    flow.stop().await;
}

/// The deadline: the live lead is asked for a handoff in the delegator's headings, the
/// thread is told the run is ending and writes its morning answer before the report, and the
/// run ends at the deadline with the phase unfinished.
#[tokio::test]
async fn at_the_deadline_the_thread_and_live_workers_end_cleanly() {
    let handed: Arc<Mutex<Vec<String>>> = Arc::default();
    let thread = Arc::new(Thread {
        order: vec![1],
        ..Default::default()
    });
    let (heard, inner) = (handed.clone(), thread.clone());
    let flow = Flow::start(
        "overnight-deadline",
        Options::default(),
        script(move |turn| {
            let (heard, inner) = (heard.clone(), inner.clone());
            async move {
                if turn.is_orchestrator() {
                    return inner.turn(&turn).await;
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
    let told = thread
        .turns
        .lock()
        .unwrap()
        .iter()
        .filter(|(input, _)| input.contains("The overnight run is ending now"))
        .count();
    assert_eq!(told, 1, "the thread hears once that the run ends");
    let plan = plan_of(&board, &run);
    assert_ne!(
        plan.steps[0].settled.as_ref().map(|s| s.outcome),
        Some(StepOutcome::Done)
    );
    assert_eq!(run.verified_commit, None);
    // The thread's morning answer comes before the report.
    let head = flow.core.head(&flow.conversation).await.unwrap().unwrap();
    let messages = flow.core.branch(&flow.conversation, &head).await.unwrap();
    let answer = messages
        .iter()
        .position(|message| message.text.contains(MORNING))
        .expect("the morning answer");
    let report = messages
        .iter()
        .position(|message| Some(&message.id) == run.report_message_id.as_ref())
        .expect("the report");
    assert!(answer < report);
    let text = report_text(&flow, &run).await;
    assert!(text.contains("stopped at its"), "{text}");
    assert!(
        text.contains("Nothing settled done to merge yet."),
        "{text}"
    );
    flow.stop().await;
}

/// Brigadier restarts while phase 1's lead works: the thread hears so, leads the phase again,
/// lands and settles it, and ends the run.
#[tokio::test]
async fn a_restart_mid_run_resumes_it_on_the_thread() {
    let restarted = Arc::new(AtomicBool::new(false));
    let first_turns = Arc::new(AtomicU32::new(0));
    let release = Arc::new(tokio::sync::Notify::new());
    let thread = Arc::new(Thread {
        order: vec![1],
        ..Default::default()
    });
    let (after, turns, released, inner) = (
        restarted.clone(),
        first_turns.clone(),
        release.clone(),
        thread.clone(),
    );
    let mut flow = Flow::start(
        "overnight-restart",
        Options::default(),
        script(move |turn| {
            let (after, turns, released, inner) = (
                after.clone(),
                turns.clone(),
                released.clone(),
                inner.clone(),
            );
            async move {
                if turn.is_orchestrator() {
                    if turn
                        .input
                        .contains("Brigadier restarted during the overnight run")
                    {
                        inner.delegate(&turn, 1).await;
                        return Reply::text(QUIET);
                    }
                    return inner.turn(&turn).await;
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
    let plan = plan_of(&board, &run);
    assert_eq!(
        plan.steps[0].settled.as_ref().map(|s| s.outcome),
        Some(StepOutcome::Done)
    );
    let branch = run.workspace.as_ref().expect("a branch").branch.clone();
    let files = super::git(&flow.repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(files.contains("p1.txt"), "{files}");
    flow.stop().await;
}

/// "only phases 3–4" keeps the plan's own numbers: phases 1 and 2 are left out (no work
/// starts for them), `phase: 3` is the source plan's phase 3, and Merge takes phases 3 and 4.
#[tokio::test]
async fn only_phases_3_to_4_work_by_their_source_numbers() {
    let thread = Arc::new(Thread {
        order: vec![3, 4],
        refused: vec![1, 2, 5],
        ..Default::default()
    });
    let flow = flow_with("overnight-only", thread.clone()).await;
    let run = start_run(&flow, "/overnight Make files. Only phases 3-4.", 4).await;
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    assert_eq!(run.stop, Some(StopReason::Done), "{run:#?}");
    let plan = plan_of(&board, &run);
    let numbers: Vec<_> = plan
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| (step.number_at(index), step.stage))
        .collect();
    assert_eq!(
        numbers,
        [
            (1, PhaseStage::Skipped),
            (2, PhaseStage::Skipped),
            (3, PhaseStage::Done),
            (4, PhaseStage::Done)
        ]
    );
    let lead_of = |number: u32| {
        board
            .tasks
            .values()
            .find(|task| task.phase == Some(number))
            .map(|task| task.id.clone())
    };
    assert_eq!(plan.steps[2].task_id, lead_of(3));
    let files = super::git(
        &flow.repo,
        &[
            "ls-tree",
            "-r",
            "--name-only",
            &run.workspace.as_ref().unwrap().branch,
        ],
    );
    assert!(
        files.contains("p3.txt") && files.contains("p4.txt"),
        "{files}"
    );
    assert!(!files.contains("p1.txt"), "{files}");
    let report = report_text(&flow, &run).await;
    for line in [
        "– Phase 1 · File 1: skipped.",
        "✓ Phase 3 · File 3: done.",
        "Merge takes phases 3 and 4",
        "2 of 2 phases done.",
    ] {
        assert!(report.contains(line), "{line}\n{report}");
    }
    flow.stop().await;
}

/// "skip phase 2": phase 2 starts nothing and doesn't hold back Merge; a phase settled
/// partial does, so the accepted tip stays at the last phase done in a row.
#[tokio::test]
async fn a_skipped_phase_starts_nothing_and_a_partial_one_holds_the_merge_back() {
    let thread = Arc::new(Thread {
        order: vec![1, 3, 4],
        refused: vec![2],
        outcomes: HashMap::from([(3, StepOutcome::Partial)]),
        ..Default::default()
    });
    let flow = flow_with("overnight-skip", thread.clone()).await;
    let run = start_run(&flow, "/overnight Make files. Skip phase 2.", 4).await;
    let board = finished(&flow, &run.id).await;
    let run = run_of(&board, &run.id);
    let plan = plan_of(&board, &run);
    assert_eq!(plan.steps[1].stage, PhaseStage::Skipped);
    assert_eq!(
        run.verified_commit,
        plan.steps[0].settled.as_ref().and_then(|s| s.tip.clone()),
        "phase 3 is partial: Merge stops at phase 1"
    );
    let report = report_text(&flow, &run).await;
    for line in [
        "Merge takes phase 1 (",
        "◐ Phase 3 · File 3: partial, write p3.txt.",
        "✓ Phase 4 · File 4: done.",
        "aren't in the merge",
    ] {
        assert!(report.contains(line), "{line}\n{report}");
    }
    flow.stop().await;
}

/// A review that ends after its run's report wakes nobody, yet its findings aren't lost: the
/// user reads them in the thread.
#[tokio::test]
async fn findings_after_the_runs_report_reach_the_thread() {
    let thread = Arc::new(Thread {
        order: vec![1],
        ..Default::default()
    });
    let flow = flow_with("overnight-late-review", thread).await;
    let run = start_run(&flow, "/overnight Make one file.", 1).await;
    let board = finished(&flow, &run.id).await;
    let lead = board
        .tasks
        .values()
        .find(|task| task.role == Some(WorkerRole::Lead))
        .expect("the lead")
        .clone();
    let late = crate::work::ReviewRun {
        id: "late-review".into(),
        conversation_id: flow.conversation.clone(),
        request_id: lead.request_id.clone(),
        task_id: Some(lead.id.clone()),
        kind: ReviewKind::Code,
        base: "a1b2c3d4".into(),
        tip: lead.landed.clone().expect("landed"),
        author: lead.route.choice.provider,
        reviewer: brigadier_providers::ProviderKind::Codex,
        reviewer_model: None,
        notify: crate::work::ReviewFor::Orchestrator,
        state: ReviewState::Findings { count: 1 },
        started_at_ms: 0,
        ended_at_ms: Some(1),
        findings: None,
    };
    flow.manager
        .tell_review(&late, Some("- [P1] p1.txt is empty — p1.txt:1"))
        .await;
    let notices: Vec<String> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            crate::model::DomainEvent::ConversationNotice { notice, .. } => Some(notice.text),
            _ => None,
        })
        .collect();
    assert!(
        notices
            .iter()
            .any(|text| text.contains("ended after the run's report")
                && text.contains("p1.txt is empty")),
        "{notices:?}"
    );
    flow.stop().await;
}
