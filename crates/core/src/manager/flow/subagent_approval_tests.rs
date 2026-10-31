//! D69: a CLI's own subagent asks for a permission; a fork of the session that started it
//! answers, the user never sees it, and the fork is deleted.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::model::Origin;
use brigadier_providers::{
    Access, ApprovalDecision, Artifact, Decider, ProviderEvent, ProviderKind,
};

use super::{FakeBehavior, Flow, Options, Reply, Script};
use crate::model::PermissionLevel;
use crate::work::CardState;

const AGENT: &str = "toolu_agent";
const PUSH: &str = "git push --force origin main";

#[derive(Default)]
struct Seen {
    /// Each fork's turn: its CLI session and what it was asked.
    forks: Vec<(String, String)>,
    /// What the subagent's request got.
    decision: Option<Option<ApprovalDecision>>,
    /// The thread's CLI session.
    thread: Option<String>,
    /// Read while a request nobody could answer still waits: its card, the user's approvals
    /// and the subagent's rows.
    waiting: Option<(CardState, usize, Vec<ProviderEvent>)>,
}

/// The thread's turn asks as its subagent would; each fork answers with `fork_reply`. When
/// `waits` the subagent doesn't wait for an answer: the turn reads the state once no fork could
/// answer.
fn script(fork_reply: &'static str, waits: bool, seen: Arc<Mutex<Seen>>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.input.contains("One of your own subagents") {
                assert!(turn.input.contains(PUSH), "{}", turn.input);
                assert!(turn.input.contains("Its input: {"), "{}", turn.input);
                let fork = (turn.native_id.clone(), turn.input.clone());
                seen.lock().unwrap().forks.push(fork);
                return Reply::text(fork_reply);
            }
            if !turn.is_orchestrator() || !turn.input.contains("Go.") {
                return Reply::text("Noted.");
            }
            seen.lock().unwrap().thread = Some(turn.native_id.clone());
            if !waits {
                let decision = turn.ask_subagent_approval(AGENT, PUSH).await;
                seen.lock().unwrap().decision = Some(decision);
                return Reply::text("Done.");
            }
            let asking = turn.clone();
            tokio::spawn(async move { asking.ask_subagent_approval(AGENT, PUSH).await });
            let host = turn.host.clone();
            let id = turn.conversation().expect("the thread's");
            tokio::time::timeout(*super::PATIENCE, async {
                while host.fork_failures.lock().unwrap().is_empty() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("no fork could answer");
            let board = host.core.board(&id).await.unwrap();
            let card = board
                .approvals
                .values()
                .next()
                .expect("its card")
                .state
                .clone();
            let users = board
                .activity(&id, Default::default())
                .map_or(0, |activity| activity.approvals.len());
            let view = host.thread_view_with_workers(&id).await.unwrap();
            let rows = rows_of(&view.entries);
            seen.lock().unwrap().waiting = Some((card, users, rows));
            Reply::text("Done.")
        })
    })
}

/// The subagent's own rows of a thread.
fn rows_of(entries: &[brigadier_providers::session_file::FileEntry]) -> Vec<ProviderEvent> {
    entries
        .iter()
        .filter(|entry| entry.parent.as_deref() == Some(AGENT))
        .map(|entry| entry.event.clone())
        .collect()
}

async fn run(name: &str, fork_reply: &'static str, waits: bool) -> (Flow, Arc<Mutex<Seen>>) {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let options = Options {
        permission: PermissionLevel::AcceptEdits,
        behavior: Arc::new(FakeBehavior {
            cleanup: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start(name, options, script(fork_reply, waits, seen.clone())).await;
    flow.say("Go.").await;
    flow.until("the subagent's request is through", |_| {
        let seen = seen.lock().unwrap();
        seen.decision.is_some() || seen.waiting.is_some()
    })
    .await;
    flow.settled().await;
    (flow, seen)
}

/// The forks started, each of the thread's session, read-only and never saved where it can be.
fn forks_of(flow: &Flow, thread: &str) -> usize {
    let forks: Vec<_> = flow
        .specs()
        .into_iter()
        .filter(|(_, spec)| matches!(spec.origin, Origin::Fork { .. }))
        .collect();
    for (kind, spec) in &forks {
        assert_eq!(*kind, ProviderKind::Claude);
        assert!(matches!(&spec.origin, Origin::Fork { native_id } if native_id == thread));
        assert_eq!(spec.access, Access::ReadOnly);
        assert!(spec.ephemeral && spec.mcp_servers.is_empty() && spec.sweep_dir.is_none());
        assert!(!spec.owned_cwd && spec.project.is_none() && spec.stop_hook.is_none());
    }
    forks.len()
}

/// Each fork's CLI session was removed once it answered.
fn forks_deleted(flow: &Flow, seen: &Seen) {
    let removed: Vec<String> = flow
        .behavior
        .removals
        .lock()
        .unwrap()
        .iter()
        .flat_map(|(_, _, artifacts)| artifacts.clone())
        .filter_map(|artifact| match artifact {
            Artifact::Session { id, .. } => Some(id),
            _ => None,
        })
        .collect();
    for (fork, _) in &seen.forks {
        assert!(removed.contains(fork), "fork {fork} left: {removed:?}");
    }
}

#[tokio::test]
async fn a_subagents_request_is_allowed_by_a_fork_of_its_orchestrator_and_the_user_sees_none() {
    let (flow, seen) = run("subagent-allow", "Fine by the goal.\nALLOW", false).await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    assert_eq!(seen.decision, Some(Some(ApprovalDecision::Allow)));
    assert_eq!(seen.forks.len(), 1);
    assert_eq!(forks_of(&flow, seen.thread.as_deref().unwrap()), 1);
    forks_deleted(&flow, &seen);
    let board = flow.board().await;
    let card = board.approvals.values().next().expect("its card");
    assert!(card.for_orchestrator() && card.task_id.is_none());
    assert_eq!(
        card.state,
        CardState::Allowed {
            by: Decider::Orchestrator,
            similar: false
        }
    );
    // Never the user's: nothing waits on them, then or now.
    assert!(
        board
            .activity(&flow.conversation, Default::default())
            .is_none_or(|activity| activity.approvals.is_empty())
    );
    // The subagent's thread shows it, read-only: asked, then allowed by its orchestrator.
    let view = flow
        .manager
        .thread_view_with_workers(&flow.conversation)
        .await
        .unwrap();
    let rows = rows_of(&view.entries);
    assert!(
        matches!(&rows[..], [
            ProviderEvent::ApprovalRequested { request },
            ProviderEvent::ApprovalResolved { decision: ApprovalDecision::Allow, decided_by: Decider::Orchestrator, .. },
        ] if request.command.as_deref() == Some(PUSH)),
        "{rows:#?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_subagents_request_denied_by_the_fork_carries_its_reason() {
    let (flow, seen) = run(
        "subagent-deny",
        "DENY: a force-push to main is off-goal",
        false,
    )
    .await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    let Some(Some(ApprovalDecision::Deny { message })) = &seen.decision else {
        panic!("{:?}", seen.decision);
    };
    assert_eq!(
        message,
        "Your orchestrator declined this: a force-push to main is off-goal"
    );
    forks_deleted(&flow, &seen);
    let board = flow.board().await;
    let card = board.approvals.values().next().expect("its card");
    assert_eq!(
        card.state,
        CardState::Denied {
            by: Decider::Orchestrator,
            message: Some("a force-push to main is off-goal".into()),
        }
    );
    let view = flow
        .manager
        .thread_view_with_workers(&flow.conversation)
        .await
        .unwrap();
    assert!(
        rows_of(&view.entries).iter().any(|row| matches!(row,
            ProviderEvent::ApprovalResolved { decision: ApprovalDecision::Deny { message }, decided_by: Decider::Orchestrator, .. }
                if message == "a force-push to main is off-goal")),
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_request_no_fork_answers_is_asked_once_more_then_waits_and_says_so() {
    let (flow, seen) = run("subagent-unanswered", "Let me think about it.", true).await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    // Asked twice, each fork deleted; Brigadier answered nothing.
    assert_eq!(seen.forks.len(), 2);
    assert_eq!(forks_of(&flow, seen.thread.as_deref().unwrap()), 2);
    forks_deleted(&flow, &seen);
    let (card, users, rows) = seen.waiting.clone().expect("read while it waits");
    assert_eq!(card, CardState::Pending);
    assert_eq!(users, 0, "never the user's card");
    assert!(
        matches!(&rows[..], [
            ProviderEvent::ApprovalRequested { .. },
            ProviderEvent::Notice { message, .. },
        ] if message.starts_with("The orchestrator couldn't answer this request")),
        "{rows:#?}"
    );
    flow.stop().await;
}

/// A fork a crash cut off before it was deleted goes at the next start; nothing else does.
#[tokio::test]
async fn a_fork_a_restart_cut_off_is_deleted() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            cleanup: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start("subagent-fork-left", options, script("ALLOW", false, seen)).await;
    let ledger = flow.manager.runtime.ledger().clone();
    let session = |id: &str| Artifact::Session {
        provider: ProviderKind::Claude,
        id: id.into(),
        home: None,
        cwd: None,
    };
    ledger
        .record("fork:left", session("fork-left"))
        .await
        .unwrap();
    ledger
        .record("gen:kept", session("gen-kept"))
        .await
        .unwrap();
    flow.manager.sweep_forks().await;
    assert!(ledger.artifacts("fork:left").is_empty());
    assert_eq!(ledger.artifacts("gen:kept").len(), 1);
    ledger.dispose("gen:kept").await;
    flow.stop().await;
}
