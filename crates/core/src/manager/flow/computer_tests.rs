//! Computer use through a worker's own computer grant (COMPUTER-USE-PLAN.md §4.6): what asks
//! on a card at the lower permission levels and what doesn't, what reaches the helper (a fake
//! here), a denial, a reused pid, and the user's Stop while a card waits.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_computer::action::ActRequest;
use brigadier_computer::wire::{Instance, LaunchRequest, Op};
use brigadier_providers::ApprovalDecision;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::manager::computer::fake::FakeHelper;
use crate::model::PermissionLevel;
use crate::tools::{ComputerCall, ToolReply};
use crate::work::{Approval, ApprovalSubject, CardState};

/// Pids no process has, so nothing a test records can ever end a real one.
const TEXTEDIT: Instance = Instance {
    pid: 4_000_001,
    started_us: 1,
};
const LAUNCHED: Instance = Instance {
    pid: 4_000_002,
    started_us: 1,
};

/// A worker's turn, held open for the test, and what lets it end.
struct Worker {
    turn: Turn,
    release: oneshot::Sender<()>,
}

/// A session at `permission` whose thread delegates one task; its worker's first turn is
/// handed to the test, which drives the computer tools with it. TextEdit's windows 5 and 6
/// are on the fake desktop.
async fn start(name: &str, permission: PermissionLevel) -> (Flow, Worker, FakeHelper) {
    let (tx, mut rx) = mpsc::unbounded_channel::<Worker>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let script: Script = Arc::new(move |turn: Turn| {
        let tx = tx.clone();
        Box::pin(async move {
            if turn.is_orchestrator() {
                if turn.earlier == 0 {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "high", "title": "Edit the note", "kind": "implement",
                                   "spec": "Type into TextEdit."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Done.");
            }
            let tx = tx.lock().unwrap().take();
            if let (Some(tx), Some(_)) = (tx, turn.task_number()) {
                let (release, wait) = oneshot::channel();
                let _ = tx.send(Worker { turn, release });
                let _ = wait.await;
            }
            Reply::text("Done.")
        })
    });
    let flow = Flow::start(
        name,
        Options {
            permission,
            ..Options::default()
        },
        script,
    )
    .await;
    let helper = FakeHelper::default();
    helper.show(5, TEXTEDIT);
    helper.show(6, TEXTEDIT);
    flow.manager.set_computer_starter(helper.starter());
    flow.say("Type a note in TextEdit.").await;
    let worker = tokio::time::timeout(Duration::from_secs(60), rx.recv())
        .await
        .expect("the worker started")
        .unwrap();
    assert!(!worker.turn.computer_grant.is_empty(), "a computer grant");
    (flow, worker, helper)
}

fn act(window: u32) -> ComputerCall {
    ComputerCall::Act(ActRequest {
        window,
        actions: vec![serde_json::from_value(json!({"do": "key", "key": "tab"})).unwrap()],
        screenshot: Default::default(),
    })
}

fn launch() -> ComputerCall {
    ComputerCall::Launch(LaunchRequest {
        app: Some("TextEdit".into()),
        open: None,
    })
}

/// The computer cards of the conversation.
fn cards(board: &Board) -> Vec<&Approval> {
    board
        .approvals
        .values()
        .filter(|card| matches!(card.subject, ApprovalSubject::Action { .. }))
        .collect()
}

fn pending(board: &Board) -> Option<&Approval> {
    cards(board)
        .into_iter()
        .find(|card| card.state == CardState::Pending)
}

/// What reached the helper, by name, past the start's ping and anything that only tidies up.
fn ops(helper: &FakeHelper) -> Vec<&'static str> {
    helper
        .ops()
        .iter()
        .filter_map(|op| match op {
            Op::Describe { window } => Some(match window {
                5 => "describe 5",
                6 => "describe 6",
                9 => "describe 9",
                _ => "describe ?",
            }),
            Op::Act(a) => Some(match a.window {
                5 => "act 5",
                6 => "act 6",
                9 => "act 9",
                _ => "act ?",
            }),
            Op::Launch(_) => Some("launch"),
            Op::Ping | Op::Cancel { .. } | Op::EndSession => None,
            _ => Some("other"),
        })
        .collect()
}

/// Waits for the next computer card, checks the helper meanwhile, and answers it.
async fn answer(flow: &Flow, decision: ApprovalDecision, check: impl FnOnce()) {
    let board = flow
        .until("a computer card", |board| pending(board).is_some())
        .await;
    check();
    let card = pending(&board).unwrap().id.clone();
    flow.manager
        .answer_card(flow.conversation.clone(), card, decision)
        .await
        .unwrap();
}

/// `call`, with its card answered `decision`.
async fn asked(
    flow: &Flow,
    turn: &Turn,
    call: ComputerCall,
    decision: ApprovalDecision,
) -> ToolReply {
    tokio::join!(turn.computer(call), answer(flow, decision, || {})).0
}

async fn card_count(flow: &Flow) -> usize {
    cards(&flow.board().await).len()
}

async fn finish(flow: Flow, worker: Worker) {
    drop(worker.release);
    drop(worker.turn);
    flow.stop().await;
}

/// The first act on a window asks once; the same window again doesn't; another window of the
/// same process asks again.
#[tokio::test]
async fn an_act_asks_once_per_window_of_an_instance() {
    let (flow, worker, helper) = start("computer-act", PermissionLevel::AskForApproval).await;
    let turn = &worker.turn;
    let reply = asked(&flow, turn, act(5), ApprovalDecision::Allow).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(card_count(&flow).await, 1);
    assert_eq!(ops(&helper), ["describe 5", "describe 5", "act 5"]);

    let reply = turn.computer(act(5)).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(card_count(&flow).await, 1, "approved already");
    assert_eq!(
        ops(&helper),
        ["describe 5", "describe 5", "act 5", "describe 5", "act 5"]
    );

    let reply = asked(&flow, turn, act(6), ApprovalDecision::Allow).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(card_count(&flow).await, 2);
    assert_eq!(
        ops(&helper)[5..],
        ["describe 6", "describe 6", "act 6"],
        "another window asks again"
    );
    let board = flow.board().await;
    assert!(cards(&board).iter().all(|c| c.state != CardState::Pending));
    finish(flow, worker).await;
}

/// A denied card fails the call with the user's no, and nothing acts.
#[tokio::test]
async fn a_denied_act_never_reaches_the_helper() {
    let (flow, worker, helper) = start("computer-deny", PermissionLevel::AskForApproval).await;
    let reply = asked(
        &flow,
        &worker.turn,
        act(5),
        ApprovalDecision::Deny {
            message: String::new(),
        },
    )
    .await;
    assert!(reply.is_error);
    assert!(reply.text.contains("the user declined"), "{}", reply.text);
    assert_eq!(card_count(&flow).await, 1);
    assert_eq!(ops(&helper), ["describe 5"]);
    finish(flow, worker).await;
}

/// An approval is bound to the process instance: the same pid started again asks again.
#[tokio::test]
async fn a_reused_pid_asks_again() {
    let (flow, worker, helper) = start("computer-pid", PermissionLevel::ApproveForMe).await;
    let turn = &worker.turn;
    let reply = asked(&flow, turn, act(5), ApprovalDecision::Allow).await;
    assert!(!reply.is_error, "{}", reply.text);
    // TextEdit quit and another process got its pid; window 5 is its now.
    helper.show(
        5,
        Instance {
            pid: TEXTEDIT.pid,
            started_us: 2,
        },
    );
    let reply = asked(&flow, turn, act(5), ApprovalDecision::Allow).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(card_count(&flow).await, 2);
    assert_eq!(
        ops(&helper),
        [
            "describe 5",
            "describe 5",
            "act 5",
            "describe 5",
            "describe 5",
            "act 5"
        ]
    );
    finish(flow, worker).await;
}

/// The user's Stop while a card waits settles the card and fails the call; nothing acts.
#[tokio::test]
async fn a_stop_while_a_card_waits_fails_the_call() {
    let (flow, worker, helper) = start("computer-stop", PermissionLevel::AskForApproval).await;
    let stop = async {
        flow.until("a computer card", |board| pending(board).is_some())
            .await;
        helper.stop();
    };
    let reply = tokio::join!(worker.turn.computer(act(5)), stop).0;
    assert!(reply.is_error);
    assert!(reply.text.contains("stopped_by_user"), "{}", reply.text);
    let board = flow.board().await;
    let cards = cards(&board);
    assert_eq!(cards.len(), 1);
    assert!(
        matches!(cards[0].state, CardState::Denied { .. }),
        "{:?}",
        cards[0].state
    );
    assert_eq!(ops(&helper), ["describe 5"]);
    finish(flow, worker).await;
}

/// `launch` asks before the helper sees it; what it opened is the worker's without asking.
#[tokio::test]
async fn a_launch_asks_first_and_its_windows_are_approved() {
    let (flow, worker, helper) = start("computer-launch", PermissionLevel::ApproveForMe).await;
    let turn = &worker.turn;
    helper.launches(LAUNCHED, 9);
    let reply = tokio::join!(
        turn.computer(launch()),
        answer(&flow, ApprovalDecision::Allow, || {
            assert!(ops(&helper).is_empty(), "the card comes first");
        })
    )
    .0;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(ops(&helper), ["launch"]);
    let reply = turn.computer(act(9)).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(card_count(&flow).await, 1);
    assert_eq!(ops(&helper), ["launch", "describe 9", "act 9"]);
    finish(flow, worker).await;
}

/// At Full access nothing asks.
#[tokio::test]
async fn full_access_never_asks() {
    let (flow, worker, helper) = start("computer-full", PermissionLevel::FullAccess).await;
    let turn = &worker.turn;
    helper.launches(LAUNCHED, 9);
    for call in [launch(), act(9), act(5), act(6)] {
        let reply = turn.computer(call).await;
        assert!(!reply.is_error, "{}", reply.text);
    }
    assert_eq!(card_count(&flow).await, 0);
    assert_eq!(
        ops(&helper),
        [
            "launch",
            "describe 9",
            "act 9",
            "describe 5",
            "act 5",
            "describe 6",
            "act 6"
        ]
    );
    // Each act carries the worker's name for its cursor.
    assert_eq!(
        helper.act_labels(),
        vec![Some("Edit the note".to_owned()); 3]
    );
    finish(flow, worker).await;
}

/// Another worker's end leaves a card waiting; only the user's Stop ends every card.
#[tokio::test]
async fn another_workers_end_leaves_a_card_waiting() {
    let (flow, worker, helper) = start("computer-other-end", PermissionLevel::AskForApproval).await;
    let other_ends = async {
        flow.until("a computer card", |board| pending(board).is_some())
            .await;
        flow.manager
            .computer
            .end_worker(&crate::work::TaskId("someone-else".into()))
            .await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(pending(&flow.board().await).is_some(), "still waiting");
        let card = pending(&flow.board().await).unwrap().id.clone();
        flow.manager
            .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
            .await
            .unwrap();
    };
    let reply = tokio::join!(worker.turn.computer(act(5)), other_ends).0;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(ops(&helper), ["describe 5", "describe 5", "act 5"]);
    finish(flow, worker).await;
}

/// A call dropped while its card waits (the CLI cancelled it) expires the card.
#[tokio::test]
async fn a_dropped_call_expires_its_card() {
    let (flow, worker, helper) = start("computer-drop", PermissionLevel::AskForApproval).await;
    tokio::select! {
        _ = worker.turn.computer(act(5)) => panic!("the call ended on its own"),
        _ = flow.until("a computer card", |board| pending(board).is_some()) => {}
    }
    let board = flow
        .until("the card expired", |board| {
            cards(board)
                .iter()
                .any(|c| matches!(c.state, CardState::Expired { .. }))
        })
        .await;
    assert!(pending(&board).is_none());
    assert_eq!(ops(&helper), ["describe 5"]);
    finish(flow, worker).await;
}
