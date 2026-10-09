//! Computer use through a worker's own computer grant (COMPUTER-USE-PLAN.md §4.6): what asks
//! on a card at the lower permission levels and what doesn't, what reaches the helper (a fake
//! here), a denial, a reused pid, and the user's Stop while a card waits.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_computer::action::ActRequest;
use brigadier_computer::wire::{Instance, LaunchRequest, Op, Permissions};
use brigadier_providers::ApprovalDecision;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::manager::computer::fake::FakeHelper;
use crate::model::PermissionLevel;
use crate::tools::{ComputerCall, ToolReply};
use crate::work::{Approval, ApprovalSubject, CardState, WaitingItem, WaitingSource};

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

/// The open "Waiting on you" items for missing permissions.
fn permission_items(board: &Board) -> Vec<&WaitingItem> {
    board
        .waiting
        .values()
        .filter(|item| matches!(item.source, WaitingSource::Computer))
        .collect()
}

fn missing(accessibility: bool, screen_recording: bool) -> Option<Permissions> {
    Some(Permissions {
        accessibility,
        screen_recording,
        restarting: false,
    })
}

/// A missing permission fails the call with `permission_missing` (an act's describe too) and
/// lists one item for the user, however many calls hit it; it holds up no request. A partial
/// grant keeps it; reading both granted closes it.
#[tokio::test]
async fn a_missing_permission_lists_one_item_until_both_are_granted() {
    let (flow, worker, helper) = start("computer-permission", PermissionLevel::FullAccess).await;
    let turn = &worker.turn;
    lock(&helper.desktop).permissions = missing(false, false);
    for call in [act(5), ComputerCall::Apps, act(6)] {
        let reply = turn.computer(call).await;
        assert!(reply.is_error);
        assert!(reply.text.contains("permission_missing"), "{}", reply.text);
        // The worker is told the user is already asked, so neither it nor the thread asks again.
        assert!(
            reply.text.contains("don't ask them again"),
            "{}",
            reply.text
        );
    }
    let board = flow.board().await;
    let items = permission_items(&board);
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0].request_id, None);
    // The thread relaying the worker's report doesn't list it a second time; other asks it does.
    let note = |what: &str| crate::tools::NoteForUser {
        kind: crate::tools::NoteKind::Waiting,
        what: what.into(),
        why: None,
    };
    let again = flow
        .manager
        .note_for_user(
            &flow.conversation,
            note("Grant Brigadier the macOS Accessibility and Screen Recording permissions."),
        )
        .await
        .unwrap();
    assert!(again.contains("already listed"), "{again}");
    flow.manager
        .note_for_user(&flow.conversation, note("Add STRIPE_KEY to .env."))
        .await
        .unwrap();
    let board = flow.board().await;
    assert_eq!(board.waiting.len(), 2, "{:?}", board.waiting);
    // Accessibility granted, Screen Recording not yet: still asked.
    lock(&helper.desktop).permissions = missing(true, false);
    assert!(turn.computer(ComputerCall::Apps).await.is_error);
    let p = flow.manager.computer_permissions().await.unwrap();
    assert!(!p.screen_recording);
    assert_eq!(permission_items(&flow.board().await).len(), 1);
    // Both read granted (the card or Settings reading them): the item is over.
    lock(&helper.desktop).permissions = None;
    flow.manager.computer_permissions().await.unwrap();
    assert!(permission_items(&flow.board().await).is_empty());
    finish(flow, worker).await;
}

/// A permissions read that finds the helper just gone (it exits to pick up a screen-recording
/// grant) starts the next one and answers, instead of showing Settings an error.
#[tokio::test]
async fn a_permissions_read_starts_a_helper_that_just_restarted() {
    let (flow, worker, helper) =
        start("computer-permission-restart", PermissionLevel::FullAccess).await;
    flow.manager.computer_permissions().await.unwrap();
    let before = helper.starts.load(Ordering::SeqCst);
    assert_eq!(before, 1, "the first read started the helper");
    lock(&helper.desktop).gone_on_read = true;
    let p = flow.manager.computer_permissions().await.unwrap();
    assert!(p.accessibility && p.screen_recording);
    assert_eq!(helper.starts.load(Ordering::SeqCst), before + 1);
    finish(flow, worker).await;
}

/// Allow… asks the helper to register; Start over asks it to forget its old entry first.
#[tokio::test]
async fn allow_asks_and_start_over_resets_first() {
    use brigadier_computer::wire::Grant;
    let (flow, worker, helper) = start("computer-start-over", PermissionLevel::FullAccess).await;
    flow.manager
        .request_computer_permission(Grant::Accessibility, false)
        .await
        .unwrap();
    flow.manager
        .request_computer_permission(Grant::ScreenRecording, true)
        .await
        .unwrap();
    let asked: Vec<Op> = helper
        .ops()
        .into_iter()
        .filter(|op| {
            matches!(
                op,
                Op::RequestPermission { .. } | Op::ResetPermission { .. }
            )
        })
        .collect();
    assert_eq!(
        asked,
        vec![
            Op::RequestPermission {
                grant: Grant::Accessibility
            },
            Op::ResetPermission {
                grant: Grant::ScreenRecording
            },
        ]
    );
    finish(flow, worker).await;
}

/// A worker's next call that works closes the item too, and an item from before a restart
/// closes once the permissions are read granted.
#[tokio::test]
async fn a_working_call_or_a_read_after_a_restart_closes_the_item() {
    let (flow, worker, helper) = start("computer-permission-2", PermissionLevel::FullAccess).await;
    let turn = &worker.turn;
    lock(&helper.desktop).permissions = missing(true, false);
    assert!(turn.computer(ComputerCall::Apps).await.is_error);
    assert_eq!(permission_items(&flow.board().await).len(), 1);
    lock(&helper.desktop).permissions = None;
    assert!(!turn.computer(act(5)).await.is_error);
    assert!(permission_items(&flow.board().await).is_empty());

    // An item the daemon didn't raise this run (a restart forgot it) is found by the
    // reconcile and closed by the next read.
    lock(&helper.desktop).permissions = missing(false, true);
    assert!(turn.computer(ComputerCall::Apps).await.is_error);
    flow.manager.forget_computer_waits();
    lock(&helper.desktop).permissions = None;
    flow.manager.computer_permissions().await.unwrap();
    assert_eq!(
        permission_items(&flow.board().await).len(),
        1,
        "stale until found"
    );
    flow.manager.reconcile_waiting(&flow.conversation).await;
    flow.manager.computer_permissions().await.unwrap();
    assert!(permission_items(&flow.board().await).is_empty());
    finish(flow, worker).await;
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}

/// An action the operate worker ran, as the action log keeps it.
fn logged(batch: &str, image: Option<String>) -> crate::work::ComputerAction {
    crate::work::ComputerAction {
        batch: batch.into(),
        index: 0,
        at_ms: crate::now_ms(),
        kind: "click".into(),
        app: "TextEdit".into(),
        app_window: "Notes".into(),
        target: Some("button \"Save\"".into()),
        pid: TEXTEDIT.pid,
        window: 5,
        status: "done".into(),
        rung: Some("element".into()),
        effect: Some("confirmed".into()),
        error: None,
        detail: None,
        dispatch_ms: 1.0,
        record: String::new(),
        image,
    }
}

/// `operate` takes a target and an end state, and only it does; its worker hears both, gets
/// the computer tools loaded from the start (a scout doesn't), and its report ends with its
/// batches and its last screenshot, which the thread opens as an image.
#[tokio::test]
async fn an_operate_task_brings_its_batches_and_last_screenshot_with_its_report() {
    let reports: Arc<Mutex<Vec<String>>> = Arc::default();
    let grants: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let shot = Arc::new(Mutex::new(String::new()));
    let script: Script = {
        let (reports, grants, shot) = (reports.clone(), grants.clone(), shot.clone());
        Arc::new(move |turn: Turn| {
            let (reports, grants, shot) = (reports.clone(), grants.clone(), shot.clone());
            Box::pin(async move {
                if turn.is_orchestrator() {
                    if turn.earlier == 0 {
                        let operate = |extra: serde_json::Value| {
                            let mut args = json!({"effort": "medium", "title": "Save the note",
                                "kind": "operate", "spec": "Save the note in TextEdit."});
                            args.as_object_mut()
                                .unwrap()
                                .extend(extra.as_object().unwrap().clone());
                            args
                        };
                        let reply = turn
                            .call("delegate_task", operate(json!({"target": "TextEdit"})))
                            .await;
                        assert!(reply.is_error);
                        assert!(reply.text.contains("needs `target`"), "{}", reply.text);
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": "Find the flag", "kind": "scout",
                                       "spec": "Where is the flag?", "end_state": "Found."}),
                            )
                            .await;
                        assert!(reply.is_error);
                        assert!(
                            reply.text.contains("are for operate tasks"),
                            "{}",
                            reply.text
                        );
                        for args in [
                            operate(json!({"target": "TextEdit, the window \"Notes\"",
                                           "end_state": "The note is saved."})),
                            json!({"effort": "low", "title": "Find the flag", "kind": "scout",
                                   "spec": "Where is the flag?"}),
                        ] {
                            let reply = turn.call("delegate_task", args).await;
                            assert!(!reply.is_error, "{}", reply.text);
                        }
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[report task-") {
                        reports.lock().unwrap().push(turn.input.clone());
                    }
                    return Reply::text("Done.");
                }
                let kind = if turn.prompt.contains("Kind: operate") {
                    "operate"
                } else {
                    "scout"
                };
                grants
                    .lock()
                    .unwrap()
                    .push((kind.into(), turn.computer_grant.clone()));
                if kind == "operate" {
                    assert!(
                        turn.prompt.contains(
                            "Target: TextEdit, the window \"Notes\"\nEnd state: The note is saved."
                        ),
                        "{}",
                        turn.prompt
                    );
                    let Some(crate::tools::Role::Worker {
                        conversation_id,
                        task_id,
                        ..
                    }) = turn.host.grants.resolve(&turn.grant)
                    else {
                        panic!("a worker grant");
                    };
                    let hash = turn
                        .host
                        .core
                        .store()
                        .blobs()
                        .put(b"\x89PNG fake".to_vec())
                        .await
                        .unwrap()
                        .to_string();
                    *shot.lock().unwrap() = hash.clone();
                    let events = [
                        logged("b1", Some("0".repeat(64))),
                        logged("b1", None),
                        logged("b2", Some(hash)),
                    ]
                    .into_iter()
                    .map(|action| crate::model::DomainEvent::ComputerActed {
                        conversation_id: conversation_id.clone(),
                        task_id: task_id.clone(),
                        action,
                    })
                    .collect();
                    turn.host
                        .core
                        .record_conversation(&conversation_id, events)
                        .await
                        .unwrap();
                }
                let reply = turn
                    .call("submit_report", json!({"summary": "Done."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            })
        })
    };
    let flow = Flow::start("computer-operate", Options::default(), script).await;
    flow.say("Save the note in TextEdit, and find the flag.")
        .await;
    let board = flow.settled().await;

    // Claude loads the computer tools at once for the operator only.
    let specs = flow.specs.lock().unwrap().clone();
    let grants = grants.lock().unwrap().clone();
    assert_eq!(grants.len(), 2, "{grants:?}");
    for (kind, grant) in &grants {
        let server = specs
            .iter()
            .flat_map(|(_, spec)| &spec.mcp_servers)
            .find(|server| server.env.iter().any(|(_, value)| value == grant))
            .expect("its computer server");
        assert_eq!(server.name, "computer");
        assert_eq!(server.always_load, kind == "operate", "{kind}");
    }

    let shot = shot.lock().unwrap().clone();
    let reports = reports.lock().unwrap().join("\n");
    let operate = Flow::task(&board, 1);
    assert_eq!(operate.kind, crate::work::TaskKind::Operate);
    assert!(
        reports.contains(&format!(
            "Computer actions: batches 1–2 · last screenshot: artifact {shot}"
        )),
        "{reports}"
    );
    let artifacts = &operate.report.as_ref().expect("a report").artifacts;
    assert!(
        artifacts
            .iter()
            .any(|a| a.id == shot && a.mime == "image/png"),
        "{artifacts:?}"
    );
    // The scout's report has no such line.
    assert_eq!(reports.matches("Computer actions:").count(), 1, "{reports}");

    // The thread reads the screenshot as an image.
    let reply = flow
        .manager
        .orchestrator_call(
            flow.conversation.clone(),
            crate::tools::OrchestratorCall::ReadArtifact(crate::tools::ReadArtifact {
                id: shot.clone(),
                offset: None,
                limit: None,
            }),
        )
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(reply.images.len(), 1);
    assert_eq!(reply.images[0].data, b"\x89PNG fake");
    flow.stop().await;
}
