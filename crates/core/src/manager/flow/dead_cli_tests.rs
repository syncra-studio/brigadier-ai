//! A worker's CLI that ended and the cards it left: the worker is kept and its next message
//! resumes it (#811), a dead CLI's cards can't catch an answer (#767), and a level switch that
//! settles a card says so (#650).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::model::{ApprovalKind, Origin, ToolSet};
use brigadier_providers::{ApprovalDecision, ApprovalRequest};

use super::{Flow, Options, Reply, Script};
use crate::model::{PermissionLevel, Setup};
use crate::work::{ApprovalSubject, CardId, CardState, Task, TaskState};

type Inputs = Arc<Mutex<Vec<String>>>;

/// A session that starts a worker on `Start: <prompt>`. The worker crashes on "Crash.", and
/// asks permission on "Ask." or when it goes on. Everything said is kept.
fn script(said: Inputs) -> Script {
    Arc::new(move |turn| {
        let said = said.clone();
        Box::pin(async move {
            said.lock().unwrap().push(turn.input.clone());
            if turn.worker().is_some() {
                if turn.input.contains("Crash.") {
                    return Reply::crashed(15);
                }
                if turn.input.contains("Ask.") || turn.input.contains("Continue where") {
                    let answer = turn.ask_approval("curl https://example.com", "curl").await;
                    return Reply::text(format!("Answered: {answer:?}."));
                }
                return Reply::text("Done.");
            }
            if let Some(prompt) = turn.input.strip_prefix("Start: ") {
                turn.start_worker("Worker", prompt).await;
            }
            Reply::text("Noted.")
        })
    })
}

async fn flow(name: &str, permission: PermissionLevel) -> (Flow, Inputs) {
    let said = Inputs::default();
    let flow = Flow::start(
        name,
        Options {
            permission,
            ..Options::default()
        },
        script(said.clone()),
    )
    .await;
    (flow, said)
}

fn worker_starts(flow: &Flow) -> Vec<Origin> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| spec.tools == ToolSet::Lean)
        .map(|(_, spec)| spec.origin)
        .collect()
}

async fn pending_card(flow: &Flow) -> CardId {
    flow.until("a waiting card", |board| {
        board
            .approvals
            .values()
            .any(|card| card.state == CardState::Pending)
    })
    .await
    .approvals
    .values()
    .find(|card| card.state == CardState::Pending)
    .unwrap()
    .id
    .clone()
}

async fn send(flow: &Flow, task: &Task, text: &str) {
    flow.manager
        .send_to_worker(
            flow.conversation.clone(),
            task.id.clone(),
            text.into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
}

/// #811: a CLI killed from outside leaves the worker and its worktree. Nothing starts by itself;
/// its session and its row say so, and its next message resumes the same CLI session. An
/// explicit stop still ends it.
#[tokio::test]
async fn a_worker_whose_cli_died_is_kept_and_its_next_message_resumes_it() {
    let (flow, said) = flow("dead-cli-kept", PermissionLevel::Full).await;
    flow.say("Start: Crash.").await;
    let board = flow
        .until("its CLI's end to be noted", |board| {
            board.tasks.values().any(|task| task.error.is_some())
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    assert_eq!(task.state, TaskState::Reported);
    let error = task.error.as_deref().unwrap();
    assert!(error.contains("the next message continues it"), "{error}");
    assert!(error.contains("signal 15"), "{error}");
    let worktree = task
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.worktree.clone())
        .expect("a worktree");
    assert!(std::path::Path::new(&worktree).is_dir());
    super::eventually("its session to hear it", || {
        said.lock().unwrap().iter().any(|input| {
            input.contains("stopped") && input.contains("the next message continues it")
        })
    })
    .await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(worker_starts(&flow).len(), 1, "nothing starts by itself");

    send(&flow, &task, "Go on.").await;
    let board = flow
        .until("its next turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
                && Flow::task(board, 1).error.is_none()
        })
        .await;
    let starts = worker_starts(&flow);
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert!(
        matches!(&starts[1], Origin::Resume { native_id }
            if Some(native_id) == Flow::task(&board, 1).native_session.as_ref()),
        "{starts:?}"
    );
    assert!(
        said.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("Go on."))
    );

    flow.manager.stop_task(task.id.clone()).await.unwrap();
    let board = flow.board().await;
    assert!(Flow::task(&board, 1).state.is_final());
    flow.stop().await;
}

/// #767: a paused worker's CLI takes its open card with it; its next CLI's request is the one
/// the answer reaches, on the first try.
#[tokio::test]
async fn a_closed_clis_card_expires_and_the_next_allow_reaches_the_real_prompt() {
    let (flow, _said) = flow("dead-cli-card", PermissionLevel::AcceptEdits).await;
    flow.say("Start: Ask.").await;
    let stale = pending_card(&flow).await;
    let board = flow
        .until("it to wait on the card", |board| {
            Flow::task(board, 1).state == TaskState::Blocked
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    assert!(flow.manager.pause_task(task.id.clone()).await.unwrap());
    let board = flow.board().await;
    assert!(
        matches!(board.approvals[&stale].state, CardState::Expired { .. }),
        "{:?}",
        board.approvals[&stale].state
    );
    flow.manager.resume_task(task.id.clone()).await.unwrap();
    let real = pending_card(&flow).await;
    assert_ne!(real, stale);
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            real.clone(),
            ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    let board = flow
        .until("its turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert!(matches!(
        board.approvals[&real].state,
        CardState::Allowed { .. }
    ));
    flow.stop().await;
}

/// #767: an answer that reaches no CLI is never stored as given: the card can't stand for an
/// answer nobody got, and can't catch the next one.
#[tokio::test]
async fn an_answer_that_reaches_no_cli_is_not_stored_as_given() {
    let (flow, _said) = flow("dead-cli-answer", PermissionLevel::AcceptEdits).await;
    flow.say("Start: Say hi.").await;
    let board = flow
        .until("its turn's end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.state == TaskState::Reported)
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    let live = flow.manager.task_live(&task);
    live.close_cli().await;
    live.allow_revival().await;
    let (card, _rx) = flow
        .manager
        .open_approval(
            &flow.conversation,
            Some(task.id.clone()),
            ApprovalSubject::Cli {
                request: ApprovalRequest {
                    network: false,
                    agent: None,
                    id: "no-such-request".into(),
                    kind: ApprovalKind::Command,
                    tool: "Bash".into(),
                    command: Some("curl https://example.com".into()),
                    cwd: None,
                    paths: Vec::new(),
                    reason: None,
                    escalation: true,
                    input: None,
                    grant: None,
                },
            },
        )
        .await
        .unwrap();
    let answer = flow
        .manager
        .answer_card(
            flow.conversation.clone(),
            card.id.clone(),
            ApprovalDecision::Allow,
        )
        .await;
    assert!(answer.is_err(), "{answer:?}");
    let state = flow.board().await.approvals[&card.id].state.clone();
    assert!(matches!(state, CardState::Expired { .. }), "{state:?}");
    flow.stop().await;
}

/// #650: a card the switch to Full access settled: its session hears who settled it, and an
/// answer that comes late is told the same.
#[tokio::test]
async fn a_level_switch_that_settles_a_card_says_so_to_the_session() {
    let (flow, said) = flow("dead-cli-level", PermissionLevel::AcceptEdits).await;
    flow.say("Start: Ask.").await;
    let card = pending_card(&flow).await;
    let mut setup = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
        .unwrap();
    match &mut setup {
        Setup::Session { permission, .. } | Setup::Chat { permission, .. } => {
            *permission = PermissionLevel::Full;
        }
    }
    flow.manager
        .set_setup(flow.conversation.clone(), setup)
        .await
        .unwrap();
    super::eventually("its session to hear who settled it", || {
        said.lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("because the session went to Full access"))
    })
    .await;
    let late = flow
        .manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap_err()
        .to_string();
    assert!(late.contains("went to Full access"), "{late}");
    flow.stop().await;
}
