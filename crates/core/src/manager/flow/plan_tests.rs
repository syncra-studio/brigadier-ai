//! Plans as documents (THREAD-PARITY-PLAN.md §6): the thread proposes its plan on a card
//! (`propose_plan`), the user's yes builds it and turns plan mode off, their changes send it
//! back for a revision that replaces it, and a lead's outline under "Ask for approval" is the
//! same card, whose one answer starts the lead.

use std::sync::{Arc, Mutex};

use brigadier_providers::ApprovalDecision;
use serde_json::json;
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::{PermissionLevel, Setup};
use crate::work::{
    ApprovalSubject, CardState, Plan, PlanApprover, PlanState, RequestState, TaskState,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const BODY: &str = "# Add a --version flag\nPrint the CLI's version and exit.\n\n## Changes\n- `src/cli.rs`: add `--version` to `Args`.\n\n## Checks\n- `cargo test -p cli`.\n\n## Assumptions\n- The version comes from `Cargo.toml`.";

/// The session's plans, oldest first.
fn plans(board: &Board) -> Vec<Plan> {
    let mut plans: Vec<Plan> = board.plans.values().cloned().collect();
    plans.sort_by_key(|plan| plan.created_at_ms);
    plans
}

fn plan_mode(flow: &Flow) -> bool {
    matches!(
        flow.core.conversation(&flow.conversation).unwrap().setup,
        Some(Setup::Session {
            plan_mode: true,
            ..
        })
    )
}

/// In plan mode the thread is told to propose a plan; `propose_plan` records it as proposed,
/// with its body and phases, and the request holds nothing up while the card waits. The
/// user's yes approves it, turns plan mode off, and continues the same request, where the
/// thread hears it without the plan-mode note.
#[tokio::test]
async fn a_proposed_plan_is_a_document_and_yes_builds_it_with_plan_mode_off() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "plan-yes",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[decision]") {
                    return Reply::text("Added the flag.");
                }
                assert!(
                    turn.input.contains("propose it with propose_plan"),
                    "{}",
                    turn.input
                );
                let empty = turn
                    .call(
                        "propose_plan",
                        json!({ "title": "Version flag", "body": " " }),
                    )
                    .await;
                assert!(empty.is_error, "{}", empty.text);
                let proposed = turn
                    .call(
                        "propose_plan",
                        json!({ "title": "Add a --version flag", "body": BODY,
                                "phases": [{ "title": "Flag" }, { "title": "Docs" }] }),
                    )
                    .await;
                assert!(!proposed.is_error, "{}", proposed.text);
                assert!(proposed.text.contains("[quiet]"), "{}", proposed.text);
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Add a --version flag to the CLI.").await;
    let board = flow.settled().await;
    let [plan] = plans(&board).try_into().expect("one plan");
    assert_eq!(plan.state, PlanState::Proposed);
    assert_eq!(plan.title, "Add a --version flag");
    assert_eq!(plan.body.as_deref(), Some(BODY));
    assert_eq!(
        plan.steps
            .iter()
            .map(|s| s.title.as_str())
            .collect::<Vec<_>>(),
        ["Flag", "Docs"]
    );
    // The card is the request's answer: nothing waits on it.
    assert_eq!(board.latest_request().unwrap().state, RequestState::Done);
    assert!(plan_mode(&flow));

    flow.manager
        .decide_plan(flow.conversation.clone(), plan.id.clone(), true, None)
        .await
        .unwrap();
    flow.until("the yes's turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    let board = flow.settled().await;
    assert_eq!(
        board.plans[&plan.id].state,
        PlanState::Approved {
            by: PlanApprover::User
        }
    );
    assert!(!plan_mode(&flow), "the yes turns plan mode off");
    let heard = inputs.lock().unwrap()[1].clone();
    assert!(
        heard.contains("Yes, implement this plan") && heard.contains("phase"),
        "{heard}"
    );
    assert!(!heard.contains("[plan mode]"), "{heard}");
    // The same request, worked again and done.
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    assert_eq!(board.latest_request().unwrap().state, RequestState::Done);
    // A plan decided once can't be decided again.
    assert!(
        flow.manager
            .decide_plan(flow.conversation.clone(), plan.id.clone(), true, None)
            .await
            .is_err()
    );
    flow.stop().await;
}

/// Free text on the card rejects the plan with the user's words; the thread hears them,
/// revises, and its next `propose_plan` replaces the rejected plan, which leaves one plan
/// waiting for the user.
#[tokio::test]
async fn changes_send_a_plan_back_and_the_next_proposal_replaces_it() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let checked = Arc::new(Notify::new());
    let go = checked.clone();
    let flow = Flow::start(
        "plan-changes",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            let go = go.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                let title = if turn.input.contains("What they want changed") {
                    // The test looks at the rejected plan first.
                    go.notified().await;
                    "Add --version and -V"
                } else {
                    "Add a --version flag"
                };
                let proposed = turn
                    .call("propose_plan", json!({ "title": title, "body": BODY }))
                    .await;
                assert!(!proposed.is_error, "{}", proposed.text);
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Add a --version flag to the CLI.").await;
    let board = flow.settled().await;
    let [first] = plans(&board).try_into().expect("one plan");
    // A plan without phases has one, named after it, for its progress.
    assert_eq!(first.steps.len(), 1);
    assert_eq!(first.steps[0].title, "Add a --version flag");
    flow.manager
        .decide_plan(
            flow.conversation.clone(),
            first.id.clone(),
            false,
            Some("Also accept -V.".into()),
        )
        .await
        .unwrap();
    let board = flow.core.board(&flow.conversation).await.unwrap();
    assert_eq!(
        board.plans[&first.id].state,
        PlanState::Rejected {
            message: Some("Also accept -V.".into())
        }
    );
    assert!(plan_mode(&flow), "changes keep plan mode on");
    checked.notify_one();
    flow.until("the revised plan", |board| board.plans.len() == 2)
        .await;
    let board = flow.settled().await;
    let [old, new] = plans(&board).try_into().expect("two plans");
    assert_eq!(old.id, first.id);
    assert_eq!(old.state, PlanState::Superseded);
    assert_eq!(new.state, PlanState::Proposed);
    assert_eq!(new.title, "Add --version and -V");
    let heard = inputs.lock().unwrap()[1].clone();
    assert!(heard.contains("\u{201c}Also accept -V.\u{201d}"), "{heard}");
    assert!(
        heard.contains("propose it again with propose_plan"),
        "{heard}"
    );
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    flow.stop().await;
}

/// The thread's script for a lead under "Ask for approval": it delegates, asks for the
/// outline's go-ahead (the user's card), and lands the report.
fn outline_script(worker_inputs: Arc<Mutex<Vec<String>>>) -> Script {
    script(move |turn| {
        let worker_inputs = worker_inputs.clone();
        async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[outline task-1") {
                    let reply = turn
                        .call("approve_outline", json!({"task": "task-1"}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    assert!(
                        reply.text.contains("Implement this plan?"),
                        "{}",
                        reply.text
                    );
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Added one.txt.");
                }
                if turn.input.contains("[decision]") {
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add the file", "kind": "implement",
                               "spec": "Create one.txt.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            worker_inputs.lock().unwrap().push(turn.input.clone());
            if turn.earlier == 0 {
                let reply = turn
                    .call("submit_outline", json!({"outline": "1. Create one.txt"}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Waiting for the go-ahead.");
            }
            turn.write("one.txt", "one\n");
            turn.git(&["add", "one.txt"]);
            turn.git(&["commit", "-q", "-m", "Add one.txt"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added one.txt.", "changes": ["one.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    })
}

/// Waits for the lead's outline card and answers it with `decision`; the lead then builds and
/// lands, and no other card ever opens.
async fn answer_the_outline(name: &str, decision: ApprovalDecision) -> (Board, Vec<String>) {
    let worker_inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start(
        name,
        Options {
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        outline_script(worker_inputs.clone()),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow
        .until("the outline card", |board| {
            board.approvals.values().any(|card| {
                card.state == CardState::Pending
                    && matches!(card.subject, ApprovalSubject::Outline { .. })
            })
        })
        .await;
    let card = board.approvals.values().next().unwrap().clone();
    let ApprovalSubject::Outline { outline, .. } = &card.subject else {
        unreachable!()
    };
    assert_eq!(outline, "1. Create one.txt");
    flow.manager
        .answer_card(flow.conversation.clone(), card.id.clone(), decision)
        .await
        .unwrap();
    let board = flow
        .until("the lead lands", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Landed)
        })
        .await;
    let board = {
        drop(board);
        flow.settled().await
    };
    assert_eq!(board.approvals.len(), 1, "one card: {:#?}", board.approvals);
    assert!(
        board
            .approvals
            .values()
            .all(|card| card.state != CardState::Pending)
    );
    let inputs = worker_inputs.lock().unwrap().clone();
    flow.stop().await;
    (board, inputs)
}

/// "Yes, implement this plan" on a lead's outline card resolves its approval through the
/// go-ahead: the lead builds, and no second card opens.
#[tokio::test]
async fn an_outline_cards_yes_starts_its_lead_and_opens_no_second_card() {
    let (board, inputs) = answer_the_outline("plan-outline-yes", ApprovalDecision::Allow).await;
    let lead = Flow::task(&board, 1);
    assert_eq!(lead.state, TaskState::Landed);
    assert!(lead.blocked_reason.is_none());
    assert!(
        inputs.iter().any(|input| input.contains("Go ahead.")),
        "{inputs:#?}"
    );
}

/// What the user types on a lead's outline card goes to the lead as corrections with its
/// go-ahead: the same one answer, and no second card.
#[tokio::test]
async fn changes_on_an_outline_card_start_its_lead_with_them() {
    let (board, inputs) = answer_the_outline(
        "plan-outline-changes",
        ApprovalDecision::Deny {
            message: "Put a newline at the end.".into(),
        },
    )
    .await;
    assert_eq!(Flow::task(&board, 1).state, TaskState::Landed);
    assert!(
        inputs.iter().any(|input| input.contains("corrections")
            && input.contains("From the user: Put a newline at the end.")),
        "{inputs:#?}"
    );
}

/// In plan mode a lead's outline becomes the thread's plan: the thread is told to propose it,
/// and the user's yes on that one card gives the lead its go-ahead, so it builds and lands
/// with no outline card at all.
#[tokio::test]
async fn in_plan_mode_the_users_yes_to_the_plan_starts_the_lead_that_outlined_it() {
    let flow = Flow::start(
        "plan-mode-lead",
        Options {
            plan_mode: true,
            permission: PermissionLevel::AskForApproval,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("[outline task-1") {
                    assert!(turn.input.contains("with propose_plan"), "{}", turn.input);
                    let proposed = turn
                        .call(
                            "propose_plan",
                            json!({ "title": "Add one.txt", "body": "# Add one.txt\nOne file.\n\n## Changes\n- `one.txt`: create it." }),
                        )
                        .await;
                    assert!(!proposed.is_error, "{}", proposed.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[report task-1") {
                    let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Added one.txt.");
                }
                if turn.input.contains("[decision]") {
                    assert!(turn.input.contains("task-1 the go-ahead"), "{}", turn.input);
                    return Reply::text("[quiet]");
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add the file", "kind": "implement",
                               "spec": "Create one.txt.", "provider": "claude"}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            if turn.earlier == 0 {
                let reply = turn
                    .call("submit_outline", json!({"outline": "1. Create one.txt"}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Waiting for the go-ahead.");
            }
            turn.write("one.txt", "one\n");
            turn.git(&["add", "one.txt"]);
            turn.git(&["commit", "-q", "-m", "Add one.txt"]);
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Added one.txt.", "changes": ["one.txt"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    flow.say("Add one.txt.").await;
    let board = flow
        .until("the plan card", |board| {
            board
                .plans
                .values()
                .any(|plan| plan.state == PlanState::Proposed)
        })
        .await;
    let plan = board
        .plans
        .values()
        .find(|plan| plan.state == PlanState::Proposed)
        .unwrap()
        .clone();
    flow.manager
        .decide_plan(flow.conversation.clone(), plan.id.clone(), true, None)
        .await
        .unwrap();
    let board = flow
        .until("the lead lands", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Landed)
        })
        .await;
    assert!(board.approvals.is_empty(), "{:#?}", board.approvals);
    assert!(!plan_mode(&flow));
    flow.stop().await;
}
