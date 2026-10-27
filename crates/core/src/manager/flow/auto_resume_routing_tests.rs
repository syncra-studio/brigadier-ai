//! A turn that goes on by itself after a restart keeps its routing pick exactly (D9 a, D34):
//! the thread's, a worker's resumed and a worker's started again before its session began all
//! run on the same agent, model and effort with the same servers granted. The thread keeps the
//! account it started on; a worker takes the one the user's account rules pick at each start.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{Agent, CliRequest, SESSION_ENV, StartRequest};
use brigadier_providers::{ProviderKind, SessionSpec};

use super::cli_tests::{brigadier, start};
use super::{Flow, Options, Reply, Script};
use crate::work::Task;

/// A turn as the scripted CLI took it: whose (a worker's task id, or "thread"), on which
/// account, with what input.
#[derive(Debug, Clone)]
struct Took {
    who: String,
    account: Option<String>,
    input: String,
}

type Log = Arc<Mutex<Vec<Took>>>;

/// The thread starts a Claude worker granted `pencil` and a Codex worker on a model and effort
/// of its own, then holds its turn; the workers hold theirs. Every turn that goes on after a
/// restart answers at once.
fn script(log: Log) -> Script {
    Arc::new(move |turn| {
        let log = log.clone();
        Box::pin(async move {
            let who = turn
                .worker()
                .map_or_else(|| "thread".to_owned(), |task| task.0);
            log.lock().unwrap().push(Took {
                who: who.clone(),
                account: turn.account.clone(),
                input: turn.input.clone(),
            });
            if went_on(&turn.input) {
                return Reply::text("Checked git log; carried on.");
            }
            if who != "thread" {
                if turn.input.contains("Hang") {
                    turn.stopped().await;
                }
                return Reply::text("Done.");
            }
            if turn.input.contains("Start them.") {
                let started = |request: StartRequest| CliRequest::Start(Box::new(request));
                brigadier(
                    &turn,
                    started(StartRequest {
                        mcps: vec!["pencil".into()],
                        ..start("Hang A.")
                    }),
                )
                .await;
                brigadier(
                    &turn,
                    started(StartRequest {
                        agent: Some(Agent::Codex),
                        model: Some("gpt-test".into()),
                        effort: Some("low".into()),
                        ..start("Hang B.")
                    }),
                )
                .await;
                turn.stopped().await;
            }
            Reply::text("Started.")
        })
    })
}

/// A flow whose user setup has two MCP servers, with a second Claude account that new work
/// starts on.
async fn flow(name: &str, log: &Log) -> Flow {
    let flow = Flow::start(name, Options::default(), script(log.clone())).await;
    let home = flow
        .manager
        .runtime
        .platform()
        .paths()
        .data_dir
        .join("integrations-home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers": {"pencil": {"command": "/bin/pencil"},
                           "railway": {"command": "/bin/railway"}}}"#,
    )
    .unwrap();
    flow.manager.registry().refresh();
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.weekly_resets_in(ProviderKind::Claude, None, 90);
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 20);
    flow
}

/// Routing would now pick the user's own login for new work.
fn flip_accounts(flow: &Flow) {
    flow.weekly_resets_in(ProviderKind::Claude, None, 2);
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 120);
}

/// Routing and explicit grants survive an account switch. Native suppression is
/// resolved again for the new account's actual setup, so its off list can differ.
fn pick(kind: ProviderKind, spec: &SessionSpec) -> String {
    let servers: Vec<&str> = spec.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    format!(
        "{kind:?} model={:?} effort={:?} fast={} access={:?} servers={servers:?} naked={} \
         prompt={:?}",
        spec.model,
        spec.effort,
        spec.fast,
        spec.access,
        spec.naked_worker,
        spec.append_system_prompt,
    )
}

/// The CLI starts of `who` (a task id or the conversation id), in order.
fn starts_of(flow: &Flow, who: &str) -> Vec<(ProviderKind, SessionSpec)> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.env
                .iter()
                .any(|(key, value)| key == SESSION_ENV && value == who)
        })
        .collect()
}

/// A turn that went on by itself after the restart (a worker's note, or the thread's).
fn went_on(input: &str) -> bool {
    input.contains("Brigadier restarted while you were working")
}

/// The account the user's account rules pick for new `provider` work now.
fn routed_now(flow: &Flow, provider: ProviderKind) -> Option<String> {
    flow.manager.runtime.launch_account(provider, None).account
}

fn took(log: &Log, who: &str) -> Vec<Took> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|took| took.who == who)
        .cloned()
        .collect()
}

#[tokio::test]
async fn what_goes_on_by_itself_keeps_its_routing_pick() {
    let log: Log = Arc::default();
    let mut flow = flow("auto-resume-routing", &log).await;
    flow.say("Start them.").await;
    flow.until("both workers work", |board| {
        board.tasks.len() == 2
            && board
                .tasks
                .values()
                .all(|task| task.state == crate::work::TaskState::Running)
    })
    .await;
    super::eventually("both workers' turns", || {
        log.lock()
            .unwrap()
            .iter()
            .filter(|took| took.input.contains("Hang"))
            .count()
            == 2
    })
    .await;
    let board = flow.board().await;
    let (a, b): (Task, Task) = (Flow::task(&board, 1).clone(), Flow::task(&board, 2).clone());
    let thread = flow.conversation.0.clone();
    let before: Vec<(String, (ProviderKind, SessionSpec))> = [&a.id.0, &b.id.0, &thread]
        .iter()
        .map(|who| ((*who).clone(), starts_of(&flow, who)[0].clone()))
        .collect();

    flow.crash().await;
    flip_accounts(&flow);
    super::eventually("all three went on", || {
        [a.id.0.as_str(), b.id.0.as_str(), "thread"]
            .iter()
            .all(|who| took(&log, who).iter().any(|took| went_on(&took.input)))
    })
    .await;
    flow.settled().await;

    for (who, (kind, spec)) in &before {
        let starts = starts_of(&flow, who);
        assert_eq!(starts.len(), 2, "{who}: {starts:#?}");
        let (again_kind, again) = &starts[1];
        assert!(
            matches!(
                again.origin,
                brigadier_providers::model::Origin::Resume { .. }
            ),
            "{who}: {:?}",
            again.origin
        );
        assert_eq!(pick(*again_kind, again), pick(*kind, spec), "{who}");
    }
    // The thread stays on the account it started on (it keeps its cache there); a worker's
    // every start, this one too, takes the account the user's account rules pick then, as a
    // message to it would. Both workers went on, the Claude one now on the user's own login.
    let thread_turns = took(&log, "thread");
    assert_eq!(thread_turns[0].account.as_deref(), Some("acct-b"));
    assert!(
        thread_turns
            .iter()
            .all(|took| took.account.as_deref() == Some("acct-b")),
        "{thread_turns:#?}"
    );
    let a_turns = took(&log, &a.id.0);
    assert_eq!(a_turns[0].account.as_deref(), Some("acct-b"));
    assert!(went_on(&a_turns[1].input), "{a_turns:#?}");
    assert_eq!(a_turns[1].account, routed_now(&flow, ProviderKind::Claude));
    assert_eq!(a_turns[1].account, None);
    // As started: the Claude worker granted only `pencil`, the Codex worker on its model and
    // effort.
    let (_, a_spec) = &before[0].1;
    let servers: Vec<_> = a_spec.mcp_servers.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(servers, ["pencil"]);
    let (b_kind, b_spec) = &before[1].1;
    assert_eq!(*b_kind, ProviderKind::Codex);
    assert_eq!(b_spec.model.as_deref(), Some("gpt-test"));
    assert_eq!(b_spec.effort.as_deref(), Some("low"));
    flow.stop().await;
}

/// A worker cut off before its CLI session began starts again on the pick it was routed to:
/// its agent, model, effort, account and granted servers.
#[tokio::test]
async fn a_worker_started_again_keeps_its_routing_pick() {
    let log: Log = Arc::default();
    let mut flow = flow("auto-resume-routing-start", &log).await;
    flow.say("Start them.").await;
    // Once the thread's turn runs, the next CLI start is a worker's: it never goes on.
    super::eventually("the thread's turn", || !took(&log, "thread").is_empty()).await;
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    reached.notified().await;
    // The other worker's start went through: the one with none is the one held.
    super::eventually("the other worker's start", || flow.specs().len() == 2).await;
    let board = flow.board().await;
    let held = board
        .tasks
        .values()
        .find(|task| starts_of(&flow, &task.id.0).is_empty())
        .expect("the worker whose start is held")
        .clone();
    assert!(held.native_session.is_none());
    flow.crash().await;
    flip_accounts(&flow);
    super::eventually("the worker started again", || {
        !took(&log, &held.id.0).is_empty()
    })
    .await;
    flow.settled().await;
    let starts = starts_of(&flow, &held.id.0);
    assert_eq!(starts.len(), 1, "{starts:#?}");
    let (kind, spec) = &starts[0];
    let choice = &held.route.choice;
    assert_eq!(*kind, choice.provider);
    assert_eq!(spec.model, choice.model);
    assert_eq!(spec.effort, choice.effort);
    let servers: Vec<_> = spec.mcp_servers.iter().map(|s| s.name.clone()).collect();
    assert_eq!(servers, held.plain.as_ref().unwrap().mcps);
    let turns = took(&log, &held.id.0);
    assert!(turns[0].input.contains(&held.spec), "{turns:#?}");
    match kind {
        ProviderKind::Claude => {
            assert_eq!(servers, ["pencil"]);
            // It never ran on an account: it takes the one the account rules pick now.
            assert_eq!(turns[0].account, routed_now(&flow, ProviderKind::Claude));
        }
        ProviderKind::Codex => {
            assert_eq!(spec.model.as_deref(), Some("gpt-test"));
            assert_eq!(spec.effort.as_deref(), Some("low"));
        }
    }
    flow.stop().await;
}
