//! An overnight phase settles done only with evidence per criterion at its tip (BUILD-PLAN E2,
//! research A5(a)), with reviews on and with reviews off.
//!
//! The run's one phase has three criteria. Its lead's report meets the first two with lines its
//! file still shows, and names nothing for the third. The thread's first settle quotes words
//! the tip doesn't show: the criterion isn't bound to the tip. Lines it quotes at the tip then
//! settle it. The settlement and the morning report carry each criterion's evidence, and the
//! review's state: none asked for with reviews on, "No review was run" with them off.

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

#[derive(Default)]
struct Seen {
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
                    turn.write("p1.txt", "phase 1\ngreeting ok\n");
                    turn.git(&["add", "p1.txt"]);
                    turn.git(&["commit", "-qm", "Add p1.txt"]);
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Added p1.txt.", "changes": ["p1.txt"],
                            "done_when": [
                                "[met] p1.txt exists: p1.txt:1 `phase 1`",
                                "[met] The greeting check passes: p1.txt:2 `greeting ok`",
                            ]}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains("[overnight] The run") && turn.input.contains("has started")
                {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Phase 1", "kind": "implement",
                                   "spec": "Build phase 1: create p1.txt.", "provider": "claude",
                                   "phase": 1}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                if turn.input.contains("[report task-1") {
                    let landed = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!landed.is_error, "{}", landed.text);
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
                    // Words the tip doesn't show.
                    let gapped = settle("[met] p1-c3: p1.txt:1 `phase one`".into()).await;
                    log.lock()
                        .unwrap()
                        .settles
                        .push((gapped.is_error, gapped.text));
                    let settled = settle("[met] p1-c3: p1.txt:2 `greeting ok`".into()).await;
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
                Reply::text("")
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
    // Evidence the tip doesn't show refuses nothing (#251): the phase settles and the thread
    // hears what it has no evidence for.
    let (refused, why) = &settles[0];
    assert!(!refused, "{why}");
    assert!(
        why.starts_with("Settled phase 1")
            && why.contains("It has no evidence at the run's tip")
            && why.contains("p1-c3: [met] with no evidence that holds at the tip"),
        "{why}"
    );
    // The two criteria the report met are not what kept it.
    assert!(
        !why.lines()
            .any(|line| line.starts_with("- p1-c1") || line.starts_with("- p1-c2")),
        "{why}"
    );
    assert!(!settles[1].0, "{}", settles[1].1);
    assert!(!settles[1].1.contains("no evidence"), "{}", settles[1].1);

    let run = board.runs.get(&run.id).cloned().unwrap();
    let plan = board.plans.get(run.plan_id.as_ref().unwrap()).unwrap();
    assert_eq!(plan.steps[0].stage, PhaseStage::Done);
    let settled = plan.steps[0].settled.clone().unwrap();
    assert_eq!(settled.outcome, StepOutcome::Done);
    assert_eq!(run.verified_commit, settled.tip);
    assert!(settled.unevidenced.is_empty(), "{:?}", settled.unevidenced);
    let evidence = settled.evidence.join("\n");
    for line in [
        "p1-c1: p1.txt:1 shows `phase 1` at the tip",
        "p1-c2: p1.txt:2 shows `greeting ok` at the tip",
        "p1-c3: p1.txt:2 shows `greeting ok` at the tip",
    ] {
        assert!(evidence.contains(line), "{line}: {evidence}");
    }
    let review = settled.review.clone().unwrap();
    if reviews {
        // Nothing reviews the phase by itself.
        assert_eq!(review, "No review was asked for.");
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
    assert!(report.contains("Evidence: p1-c3: p1.txt:2"), "{report}");
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

const FRESH_RULE: &str = "A standing rule for this session: keep every greeting lowercase.";
const FRESH_RULE_WORDS: &str = "keep every greeting lowercase";
const LEAD_REPORT: &str = "Wrote hello.txt; LEAD-REPORT-MARK.";

/// What a fresh verifier saw: its instructions and first message, and the candidate's sha.
#[derive(Default)]
struct FreshSeen {
    verifier: Option<(String, String)>,
    candidate: Option<String>,
    reply: Option<String>,
}

/// A fresh verifier, of a worker's reported task or of the thread's own commit with no task:
/// it reads the request verbatim, the user's constraint, the criteria and the candidate's sha,
/// and none of the build's report, Brain section or context pack.
async fn a_fresh_verifier_gets_only_the_request_constraints_criteria_and_commit(own: bool) {
    let seen: Arc<Mutex<FreshSeen>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        if own { "fresh-own" } else { "fresh-task" },
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    if turn.prompt.contains("You are a fresh verifier") {
                        log.lock()
                            .unwrap()
                            .verifier
                            .get_or_insert((turn.prompt.clone(), turn.input.clone()));
                        let reply = turn
                            .call(
                                "submit_report",
                                json!({"summary": "Tried to break it; it holds.",
                                       "done_when": "[met] hello.txt says hello: hello.txt:1 `hello`"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Reported.");
                    }
                    turn.write("hello.txt", "hello\n");
                    turn.git(&["add", "hello.txt"]);
                    turn.git(&["commit", "-qm", "Add hello.txt"]);
                    log.lock().unwrap().candidate = Some(turn.git(&["rev-parse", "HEAD"]));
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": LEAD_REPORT, "changes": ["hello.txt"],
                                   "done_when": "[met] hello.txt says hello: hello.txt:1 `hello`"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains(FRESH_RULE) {
                    let reply = turn
                        .call(
                            "remember",
                            json!({"kind": "constraint", "title": "Greeting rule",
                                   "user_message": "latest", "quote": FRESH_RULE_WORDS,
                                   "scope": "session"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Noted.");
                }
                let fresh = |task: Option<&str>| {
                    let turn = &turn;
                    let mut args = json!({"fresh": true, "done_when": ["hello.txt says hello."]});
                    if let Some(task) = task {
                        args["task"] = json!(task);
                    }
                    async move { turn.call("start_verifier", args).await }
                };
                if turn.input.contains("Add a greeting file.") && !own {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                                   "spec": "Create hello.txt.", "provider": "claude"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                if turn.input.contains("Add a greeting file.") {
                    // The thread builds it itself, then verifies its own commit.
                    let workspace = &turn.cwd;
                    std::fs::write(workspace.join("hello.txt"), "hello\n").unwrap();
                    super::git(workspace, &["add", "hello.txt"]);
                    super::git(workspace, &["commit", "-qm", "Add hello.txt"]);
                    log.lock().unwrap().candidate =
                        Some(super::git(workspace, &["rev-parse", "HEAD"]));
                    let reply = fresh(None).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().reply = Some(reply.text);
                    return Reply::text("");
                }
                if turn.input.contains("[report task-1") && !own {
                    let reply = fresh(Some("task-1")).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().reply = Some(reply.text);
                    return Reply::text("");
                }
                Reply::text("")
            }
        }),
    )
    .await;
    flow.say(FRESH_RULE).await;
    flow.settled().await;
    flow.say("Add a greeting file.").await;
    flow.until("the fresh verifier ran", |_| {
        seen.lock().unwrap().verifier.is_some()
    })
    .await;
    flow.settled().await;
    let (prompt, input, candidate, reply) = {
        let seen = seen.lock().unwrap();
        let (prompt, input) = seen.verifier.clone().unwrap();
        (
            prompt,
            input,
            seen.candidate.clone().unwrap(),
            seen.reply.clone().unwrap(),
        )
    };
    let candidate = candidate.trim();
    assert!(reply.contains("a fresh verifier of"), "{reply}");
    // The request verbatim, the constraint in the user's words, the criteria and the sha.
    assert!(prompt.contains("Add a greeting file."), "{prompt}");
    assert!(prompt.contains(FRESH_RULE_WORDS), "{prompt}");
    assert!(prompt.contains("hello.txt says hello."), "{prompt}");
    assert!(prompt.contains(candidate), "{candidate}: {prompt}");
    assert!(prompt.contains("hello.txt (+1"), "{prompt}");
    // Nothing of the build.
    let all = format!("{prompt}\n{input}");
    for build in [
        "LEAD-REPORT-MARK",
        "Create hello.txt.",
        "[outline",
        crate::manager::context_pack::BRAIN_HEADING,
        "Files the thread read",
        "Definitions in the files",
    ] {
        assert!(!all.contains(build), "{build}: {all}");
    }
    // Its worktree started at the candidate.
    let board = flow.board().await;
    let verifier = board
        .tasks
        .values()
        .find(|task| task.fresh_commit.is_some())
        .expect("the fresh verifier's task");
    assert_eq!(verifier.fresh_commit.as_deref(), Some(candidate));
    assert_eq!(
        verifier.subject.is_some(),
        !own,
        "a task's verifier names it; the thread's own has none"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_fresh_verifier_of_a_workers_task_gets_only_the_request_and_the_commit() {
    a_fresh_verifier_gets_only_the_request_constraints_criteria_and_commit(false).await;
}

#[tokio::test]
async fn a_fresh_verifier_of_the_threads_own_commit_gets_only_the_request_and_the_commit() {
    a_fresh_verifier_gets_only_the_request_constraints_criteria_and_commit(true).await;
}
