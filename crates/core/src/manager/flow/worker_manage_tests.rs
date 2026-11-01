//! Goal 3 (D7, D118): the orchestrator manages its workers with `brigadier pause`, `resume`,
//! `switch` and `close`. A pause ends the turn at once and holds what is sent; a resume gives
//! that, then "Continue where you stopped.", once; a switch goes through goal 1's move with one
//! row; each shows once. The fake CLI is every CLI's stand-in: nothing here is per CLI.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, StartRequest};
use brigadier_providers::{LimitHit, LimitKind, ProviderKind, TurnStatus};
use serde_json::{Value, json};

use super::cli_tests::{brigadier, start};
use super::{FakeBehavior, Flow, Options, Reply};
use crate::thread_view::TurnEnd;
use crate::work::{OrchestratorStepKind, ProviderTransitionCause, Task, TaskState, WorkerStepKind};

const WORDS: &str = "Continue where you stopped.";

#[derive(Clone, Copy)]
enum Mode {
    /// Its "Build it." turn works until stopped or released.
    Hold,
    /// Its first turn hits a usage limit.
    LimitFirst,
    /// A turn after its first on this CLI hits a usage limit.
    LimitOn(ProviderKind),
}

/// Each worker turn: its CLI, account, CLI session and input.
type Seen = Arc<Mutex<Vec<(ProviderKind, Option<String>, String, String)>>>;

struct Bench {
    flow: Flow,
    seen: Seen,
    replies: Arc<Mutex<Vec<Value>>>,
    release: Arc<tokio::sync::Notify>,
}

/// A session whose "Do {request}" runs that `brigadier` request, after starting one worker.
async fn bench(name: &str, request: StartRequest, mode: Mode, accounts: bool) -> Bench {
    bench_with(name, request, mode, accounts, false).await
}

/// [`bench`], with "Switch to another agent automatically" on (`fallback`).
async fn bench_with(
    name: &str,
    request: StartRequest,
    mode: Mode,
    accounts: bool,
    fallback: bool,
) -> Bench {
    let seen: Seen = Arc::default();
    let replies: Arc<Mutex<Vec<Value>>> = Arc::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let (logged, answered, released) = (seen.clone(), replies.clone(), release.clone());
    // A message sent while a turn runs comes again on its own turn (D72): each runs once.
    let ran: Arc<Mutex<std::collections::HashSet<String>>> = Arc::default();
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(move |turn| {
            let (logged, answered, released) = (logged.clone(), answered.clone(), released.clone());
            let ran = ran.clone();
            let request = request.clone();
            Box::pin(async move {
                if turn.worker().is_none() {
                    let reply = if turn.input.contains("Start one.") {
                        brigadier(&turn, CliRequest::Start(Box::new(request))).await
                    } else if let Some((n, json)) = turn
                        .input
                        .split("Do ")
                        .nth(1)
                        .and_then(|rest| rest.lines().next()?.split_once(' '))
                    {
                        if !ran.lock().unwrap().insert(n.to_owned()) {
                            return Reply::text("Done before.");
                        }
                        brigadier(&turn, serde_json::from_str(json).unwrap()).await
                    } else {
                        return Reply::text("Heard.");
                    };
                    answered.lock().unwrap().push(reply);
                    return Reply::text("Done.");
                }
                let first = {
                    let mut seen = logged.lock().unwrap();
                    seen.push((
                        turn.provider,
                        turn.account.clone(),
                        turn.native_id.clone(),
                        turn.input.clone(),
                    ));
                    seen.len() == 1
                };
                let limited = match mode {
                    Mode::LimitFirst => first,
                    Mode::LimitOn(provider) => !first && turn.provider == provider,
                    Mode::Hold => false,
                };
                if limited {
                    return Reply {
                        limit: Some(LimitHit {
                            window: Some("five_hour".into()),
                            resets_at_ms: Some(crate::now_ms() + 60_000),
                            kind: LimitKind::UsageWindow,
                        }),
                        ..Reply::default()
                    };
                }
                if turn.input.contains("Build it.") {
                    tokio::select! {
                        () = released.notified() => return Reply::text("Built."),
                        _ = turn.stopped() => return Reply::text("Stopped."),
                    }
                }
                Reply::text("Went on.")
            })
        }),
    )
    .await;
    if accounts {
        flow.add_accounts(&[(ProviderKind::Claude, "acct-free")], true)
            .await;
    }
    let mut settings = flow.core.settings();
    settings.switch_providers = fallback;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Start one.").await;
    let bench = Bench {
        flow,
        seen,
        replies,
        release,
    };
    super::eventually("the worker's first turn", || !bench.inputs().is_empty()).await;
    bench
}

impl Bench {
    fn inputs(&self) -> Vec<(ProviderKind, Option<String>, String, String)> {
        self.seen.lock().unwrap().clone()
    }

    fn with(&self, text: &str) -> Vec<(ProviderKind, Option<String>, String, String)> {
        self.inputs()
            .into_iter()
            .filter(|seen| seen.3.contains(text))
            .collect()
    }

    fn id(&self) -> String {
        self.replies.lock().unwrap()[0]["worker"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// How its first turn ends in its own thread, once it has.
    async fn first_turn_end(&self) -> Option<TurnEnd> {
        let task = self.task().await;
        for _ in 0..50 {
            let view = self
                .flow
                .manager
                .worker_thread(&self.flow.conversation, &task.id)
                .await
                .unwrap();
            if let Some(end) = view.turns.first().and_then(|turn| turn.end) {
                return Some(end);
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        None
    }

    async fn task(&self) -> Task {
        Flow::task(&self.flow.board().await, 1).clone()
    }

    /// Runs `request` as `brigadier` in the session's next turn; its answer.
    async fn run(&self, request: CliRequest) -> Value {
        // Its turn before has ended: nothing waits in the queue to be judged.
        self.flow.idle().await;
        let before = self.replies.lock().unwrap().len();
        self.flow
            .say(&format!(
                "Do {before} {}",
                serde_json::to_string(&request).unwrap()
            ))
            .await;
        super::eventually("the command's answer", || {
            self.replies.lock().unwrap().len() > before
        })
        .await;
        self.replies.lock().unwrap()[before].clone()
    }

    /// Its session's rows of `kind` for the worker.
    async fn steps(&self, kind: fn(&OrchestratorStepKind) -> bool) -> usize {
        self.flow
            .board()
            .await
            .orchestrator_steps
            .iter()
            .filter(|step| kind(&step.kind))
            .count()
    }

    /// Waits for its switch row (its launch confirms it as its first turn starts), then
    /// counts every switch row.
    async fn switch_rows(&self) -> usize {
        self.flow
            .until("its switch row", |board| {
                board
                    .orchestrator_steps
                    .iter()
                    .any(|step| manual(&step.kind))
            })
            .await;
        let board = self.flow.idle().await;
        let rows: Vec<_> = board
            .orchestrator_steps
            .iter()
            .filter(|step| any_switch(&step.kind))
            .collect();
        assert!(rows.len() <= 1, "{rows:#?}");
        rows.len()
    }

    async fn lifecycle(&self) -> Vec<WorkerStepKind> {
        self.flow
            .board()
            .await
            .worker_steps
            .iter()
            .map(|step| step.kind)
            .collect()
    }
}

fn pause(id: &str) -> CliRequest {
    CliRequest::Pause { id: id.into() }
}

fn resume(id: &str) -> CliRequest {
    CliRequest::Resume { id: id.into() }
}

fn switch(id: &str, agent: Option<ProviderKind>) -> CliRequest {
    CliRequest::Switch {
        id: id.into(),
        agent,
        model: None,
        effort: None,
    }
}

fn send(id: &str, text: &str) -> CliRequest {
    CliRequest::Send {
        id: id.into(),
        text: Some(text.into()),
        answer: None,
    }
}

fn paused(kind: &OrchestratorStepKind) -> bool {
    matches!(kind, OrchestratorStepKind::Paused { .. })
}

fn resumed(kind: &OrchestratorStepKind) -> bool {
    matches!(kind, OrchestratorStepKind::Resumed { .. })
}

fn manual(kind: &OrchestratorStepKind) -> bool {
    matches!(
        kind,
        OrchestratorStepKind::ProviderTransition {
            task_id: Some(_),
            cause: ProviderTransitionCause::ManualSwitch,
            ..
        }
    )
}

fn any_switch(kind: &OrchestratorStepKind) -> bool {
    matches!(kind, OrchestratorStepKind::ProviderTransition { .. })
}

/// D7: the turn ends now and stays ended, its session hears no "interrupted", a message waits,
/// `wait` doesn't wait on it; the resume gives the message, then the words, once, in the same
/// CLI session; one "Paused" and one "Resumed" row, no lifecycle sentence besides.
#[tokio::test]
async fn a_pause_ends_the_turn_holds_messages_and_a_resume_goes_on_once() {
    let bench = bench("manage-pause", start("Build it."), Mode::Hold, false).await;
    let id = bench.id();
    assert_eq!(bench.run(pause(&id)).await["done"], true);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    let task = bench.task().await;
    assert_eq!(
        task.state,
        TaskState::Paused,
        "its cut turn's end changes nothing"
    );
    assert!(task.plain.as_ref().unwrap().paused);
    assert_eq!(
        bench.first_turn_end().await,
        Some(TurnEnd::Interrupted),
        "D124: not 'You stopped'"
    );
    // A real CLI may still report the cut turn's end after its close: it changes nothing.
    let steps = bench.flow.board().await.worker_steps.len();
    let live = bench.flow.manager.task_live(&task);
    bench
        .flow
        .manager
        .plain_turn_ended(&live, &task, TurnStatus::Interrupted)
        .await;
    assert_eq!(bench.task().await.state, TaskState::Paused);
    assert_eq!(bench.flow.board().await.worker_steps.len(), steps);
    assert_eq!(bench.run(pause(&id)).await["done"], false, "paused already");
    let sent = bench.run(send(&id, "Use blue.")).await;
    assert_eq!(sent["queued"], true, "{sent:#}");
    assert!(bench.with("Use blue.").is_empty(), "held, not sent");
    assert_eq!(bench.task().await.plain.unwrap().held, vec!["Use blue."]);
    let waited = bench.run(super::cli_tests::wait(&[], Some(5))).await;
    assert_eq!(
        waited,
        json!({"events": [], "timedOut": false}),
        "it waits on no paused worker"
    );
    assert!(bench.run(resume(&id)).await.get("error").is_none());
    super::eventually("the resumed turn", || !bench.with(WORDS).is_empty()).await;
    bench
        .flow
        .until("its resumed turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    let resumed_turns = bench.with(WORDS);
    assert_eq!(resumed_turns.len(), 1);
    let input = &resumed_turns[0].3;
    assert!(
        input.find("Use blue.").unwrap() < input.find(WORDS).unwrap(),
        "{input}"
    );
    assert_eq!(bench.with("Use blue.").len(), 1);
    assert_eq!(
        resumed_turns[0].2,
        bench.inputs()[0].2,
        "the same CLI session"
    );
    assert!(bench.task().await.plain.unwrap().held.is_empty());
    assert_eq!(bench.steps(paused).await, 1);
    assert_eq!(bench.steps(resumed).await, 1);
    let lifecycle = bench.lifecycle().await;
    assert!(
        !lifecycle.iter().any(|kind| matches!(
            kind,
            WorkerStepKind::Paused | WorkerStepKind::Resumed | WorkerStepKind::Stopped
        )),
        "{lifecycle:?}"
    );
    assert!(
        bench.run(resume(&id)).await["error"].is_object(),
        "not paused now"
    );
    bench.flow.stop().await;
}

/// A `brigadier wait` already waiting when the pause lands ends, with no made-up event.
#[tokio::test]
async fn a_wait_started_before_the_pause_ends_with_no_event() {
    let bench = bench("manage-wait", start("Build it."), Mode::Hold, false).await;
    let before = bench.replies.lock().unwrap().len();
    bench
        .flow
        .say(&format!(
            "Do w {}",
            serde_json::to_string(&super::cli_tests::wait(&[], None)).unwrap()
        ))
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(bench.replies.lock().unwrap().len(), before, "it waits");
    let task = bench.task().await;
    assert!(bench.flow.manager.pause_task(task.id).await.unwrap());
    super::eventually("the wait to end", || {
        bench.replies.lock().unwrap().len() > before
    })
    .await;
    assert_eq!(
        bench.replies.lock().unwrap()[before],
        json!({"events": [], "timedOut": false})
    );
    bench.flow.stop().await;
}

/// A quota wait is no pause: `wait` keeps waiting through it, and `resume` says so.
#[tokio::test]
async fn a_quota_wait_keeps_wait_waiting_and_is_not_resumed() {
    let bench = bench(
        "manage-quota-wait",
        start("Build it."),
        Mode::LimitFirst,
        false,
    )
    .await;
    bench
        .flow
        .until("its quota wait", |board| {
            Flow::task(board, 1).quota_wait.is_some()
        })
        .await;
    let id = bench.id();
    // Its usage-limit event heard (by a wait, or its session's next turn), a wait goes on.
    let mut waited = bench.run(super::cli_tests::wait(&[], Some(1))).await;
    if waited["events"][0]["event"] == "usage-limit" {
        waited = bench.run(super::cli_tests::wait(&[], Some(1))).await;
    }
    assert_eq!(waited["timedOut"], true, "{waited:#}");
    let refused = bench.run(resume(&id)).await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap()
            .contains("usage limit"),
        "{refused:#}"
    );
    bench.flow.stop().await;
}

/// A switch that finds nowhere to go leaves its quota wait as it was: it goes on at the reset.
#[tokio::test]
async fn a_refused_switch_keeps_its_quota_wait_going_on_at_the_reset() {
    let bench = bench(
        "manage-refused-switch",
        start("Build it."),
        Mode::LimitFirst,
        false,
    )
    .await;
    bench
        .flow
        .until("its quota wait", |board| {
            Flow::task(board, 1).quota_wait.is_some()
        })
        .await;
    let id = bench.id();
    // Its own CLI and model, and no other account of it: nowhere to go.
    let refused = bench.run(switch(&id, Some(bench.inputs()[0].0))).await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("can take it now"),
        "{refused:#}"
    );
    let task = bench.task().await;
    let account = task.quota_wait.as_ref().unwrap().account.clone().unwrap();
    let now = crate::now_ms();
    let monitor = bench.flow.manager.runtime.monitor();
    let mut quota = monitor.current_for(&account, now).unwrap();
    quota.limit.as_mut().unwrap().resets_at_ms = Some(now - 1);
    for window in &mut quota.windows {
        window.resets_at_ms = Some(now - 1);
    }
    quota.observed_at_ms = now;
    monitor.note(&account, &quota, now);
    bench.flow.manager.retry_waiting_workers().await;
    bench
        .flow
        .until("it went on at the reset", |board| {
            Flow::task(board, 1).quota_wait.is_none()
        })
        .await;
    bench.flow.stop().await;
}

/// A resume right after a pause, before anything settled: one resumed turn, and the old turn's
/// end never overwrites it.
#[tokio::test]
async fn a_resume_right_after_a_pause_goes_on_once() {
    let bench = bench("manage-at-once", start("Build it."), Mode::Hold, false).await;
    let task = bench.task().await;
    let manager = &bench.flow.manager;
    assert!(manager.pause_task(task.id.clone()).await.unwrap());
    manager.resume_task(task.id.clone()).await.unwrap();
    bench
        .flow
        .until("its resumed turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(bench.with(WORDS).len(), 1, "{:#?}", bench.inputs());
    assert_eq!(bench.task().await.state, TaskState::Reported);
    assert_eq!(bench.steps(paused).await, 1);
    assert_eq!(bench.steps(resumed).await, 1);
    bench.flow.stop().await;
}

/// A switch of a working worker: one "Switched to" row, its route and its pins on the new
/// CLI, and it goes on there once; a limit there afterwards never takes it back to the old
/// pin (Codex review 4).
#[tokio::test]
async fn a_switch_moves_a_working_worker_once_and_its_pins_follow() {
    let mut request = start("Build it.");
    request.agent = Some(ProviderKind::Claude);
    let bench = bench_with(
        "manage-switch",
        request,
        Mode::LimitOn(ProviderKind::Codex),
        false,
        true,
    )
    .await;
    let id = bench.id();
    let moved = bench.run(switch(&id, Some(ProviderKind::Codex))).await;
    assert!(
        moved["to"].as_str().unwrap().starts_with("Codex"),
        "{moved:#}"
    );
    assert_eq!(
        bench.first_turn_end().await,
        Some(TurnEnd::Interrupted),
        "D124: not 'You stopped'"
    );
    super::eventually("its turn on Codex", || {
        bench
            .with(WORDS)
            .iter()
            .any(|seen| seen.0 == ProviderKind::Codex)
    })
    .await;
    // Its turn there hits a limit: it waits on Codex, the CLI its pin follows now.
    bench
        .flow
        .until("its quota wait on Codex", |board| {
            Flow::task(board, 1).quota_wait.is_some()
        })
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let task = bench.task().await;
    assert_eq!(task.route.choice.provider, ProviderKind::Codex);
    let policy = task.plain.unwrap().quota_policy.unwrap();
    assert_eq!(policy.pin.provider, Some(ProviderKind::Codex));
    assert_eq!(bench.steps(manual).await, 1);
    assert_eq!(bench.steps(any_switch).await, 1, "nothing took it back");
    assert_eq!(bench.with(WORDS).len(), 1);
    assert!(
        bench
            .inputs()
            .iter()
            .skip(1)
            .all(|seen| seen.0 == ProviderKind::Codex)
    );
    bench.flow.stop().await;
}

/// No target: the next account, by the order a limit uses (D82); the same CLI session.
#[tokio::test]
async fn a_switch_with_no_target_takes_the_next_account() {
    let bench = bench("manage-account", start("Build it."), Mode::Hold, true).await;
    let id = bench.id();
    let moved = bench.run(switch(&id, None)).await;
    assert!(moved["to"].is_string(), "{moved:#} {:#?}", bench.inputs());
    super::eventually("its turn on the other account", || {
        !bench.with(WORDS).is_empty()
    })
    .await;
    let (first, next) = (bench.inputs()[0].clone(), bench.with(WORDS)[0].clone());
    assert_eq!(next.0, first.0);
    assert_ne!(next.1, first.1, "another account");
    assert_eq!(next.2, first.2, "its CLI session goes on");
    assert_eq!(bench.switch_rows().await, 1);
    bench.flow.stop().await;
}

/// D118: a paused worker switched stays paused; its row shows at the switch, once; the resume
/// goes on on the new CLI.
#[tokio::test]
async fn a_paused_worker_switched_stays_paused_until_resumed_there() {
    let bench = bench(
        "manage-paused-switch",
        start("Build it."),
        Mode::Hold,
        false,
    )
    .await;
    let id = bench.id();
    bench.run(pause(&id)).await;
    let moved = bench.run(switch(&id, Some(ProviderKind::Codex))).await;
    assert!(
        moved["to"].as_str().unwrap().starts_with("Codex"),
        "{moved:#}"
    );
    assert_eq!(bench.steps(manual).await, 1, "its row at the switch");
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(bench.task().await.state, TaskState::Paused);
    assert_eq!(bench.inputs().len(), 1, "nothing went to it");
    bench.run(resume(&id)).await;
    super::eventually("its turn on Codex", || !bench.with(WORDS).is_empty()).await;
    assert_eq!(bench.with(WORDS)[0].0, ProviderKind::Codex);
    bench
        .flow
        .until("its turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert_eq!(
        bench.steps(any_switch).await,
        1,
        "no second row at its launch"
    );
    bench.flow.stop().await;
}

/// A worker waiting for a reset, switched: it goes on now on the target, its wait gone.
#[tokio::test]
async fn a_worker_waiting_for_quota_switches_now() {
    let bench = bench(
        "manage-quota-switch",
        start("Build it."),
        Mode::LimitFirst,
        false,
    )
    .await;
    bench
        .flow
        .until("its quota wait", |board| {
            Flow::task(board, 1).quota_wait.is_some()
        })
        .await;
    let id = bench.id();
    bench.run(switch(&id, Some(ProviderKind::Codex))).await;
    super::eventually("its turn on Codex", || !bench.with(WORDS).is_empty()).await;
    assert_eq!(bench.with(WORDS)[0].0, ProviderKind::Codex);
    bench
        .flow
        .until("its wait gone", |board| {
            Flow::task(board, 1).quota_wait.is_none()
        })
        .await;
    assert_eq!(bench.switch_rows().await, 1);
    // D126: its switch row tells it went on; no "started working" beside it.
    let lifecycle = bench.lifecycle().await;
    assert!(
        !lifecycle.contains(&WorkerStepKind::Resumed),
        "{lifecycle:?}"
    );
    bench.flow.stop().await;
}

/// #678: a target on its default model is that model, whose own window counts.
#[tokio::test]
async fn a_switch_to_a_default_model_out_by_its_own_window_is_refused() {
    let bench = bench(
        "manage-default-model",
        start("Build it."),
        Mode::Hold,
        false,
    )
    .await;
    let now = crate::now_ms();
    bench.flow.manager.runtime.monitor().note(
        &crate::accounts::AccountRef::new(ProviderKind::Codex, None),
        &brigadier_providers::QuotaSnapshot {
            provider: ProviderKind::Codex,
            windows: vec![brigadier_providers::QuotaWindow {
                id: "astra_week".into(),
                label: "astra_week".into(),
                used_percent: 100.0,
                resets_at_ms: Some(now + 3_600_000),
                window_minutes: Some(7 * 24 * 60),
                bucket: None,
                model: Some("gpt-6-astra".into()),
            }],
            limit: None,
            observed_at_ms: now,
            source: brigadier_providers::QuotaSource::Read,
        },
        now,
    );
    let id = bench.id();
    let refused = bench.run(switch(&id, Some(ProviderKind::Codex))).await;
    assert!(
        refused["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no account of Codex"),
        "{refused:#}"
    );
    assert_eq!(bench.steps(any_switch).await, 0);
    bench.flow.stop().await;
}

/// A paused worker closed: one "Stopped" row, and its record still reads.
#[tokio::test]
async fn a_paused_worker_closed_stops_once_and_still_reads() {
    let bench = bench("manage-close", start("Build it."), Mode::Hold, false).await;
    let id = bench.id();
    bench.run(pause(&id)).await;
    let closed = bench.run(CliRequest::Close { id: id.clone() }).await;
    assert_eq!(closed["closed"], true, "{closed:#}");
    let read = bench
        .run(CliRequest::Read {
            id: id.clone(),
            last: Some(5),
            screen: false,
        })
        .await;
    assert!(read.get("error").is_none(), "{read:#}");
    assert_eq!(
        bench
            .steps(|kind| matches!(kind, OrchestratorStepKind::Stopped { .. }))
            .await,
        1
    );
    bench.release.notify_waiters();
    bench.flow.stop().await;
}

/// Kept across a restart: still paused, still holding, and the resume gives it all once.
#[tokio::test]
async fn a_pause_and_what_it_holds_survive_a_restart() {
    let mut bench = bench("manage-restart", start("Build it."), Mode::Hold, false).await;
    let id = bench.id();
    bench.run(pause(&id)).await;
    bench.run(send(&id, "Use blue.")).await;
    bench.flow.restart().await;
    let task = bench.task().await;
    assert_eq!(task.state, TaskState::Paused);
    assert_eq!(task.plain.unwrap().held, vec!["Use blue."]);
    bench.flow.manager.resume_task(task.id).await.unwrap();
    super::eventually("the resumed turn", || !bench.with(WORDS).is_empty()).await;
    assert_eq!(bench.with("Use blue.").len(), 1);
    bench.flow.stop().await;
}

/// A resume is durable before sending; a daemon exit on either side of acceptance delivers
/// the held batch once. The existing CLI input record reconciles an unrecorded acceptance.
#[tokio::test]
async fn a_resume_cut_off_before_or_after_acceptance_delivers_held_messages_once() {
    for accepted in [false, true] {
        let mut bench = bench(
            &format!("20261030-1141-core-goals-w79-resume-{accepted}"),
            start("Build it."),
            Mode::Hold,
            false,
        )
        .await;
        let task = bench.task().await;
        let manager = bench.flow.manager.clone();
        manager.pause_task(task.id.clone()).await.unwrap();
        bench.run(send(&bench.id(), "Use blue.")).await;
        let live = manager.task_live(&task);
        let reached = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let settled = if accepted {
            *bench.flow.behavior.hold_send.lock().unwrap() =
                Some((reached.clone(), release.clone()));
            None
        } else {
            Some(live.settle.lock().await)
        };
        let resume_id = task.id.clone();
        let resuming = tokio::spawn(async move { manager.resume_task(resume_id).await });
        if accepted {
            reached.notified().await;
        } else {
            bench
                .flow
                .until("unpaused before its send", |board| {
                    Flow::task(board, 1).state == TaskState::Running
                })
                .await;
        }
        // Killing the request drops its locks, as the old daemon dying would.
        resuming.abort();
        assert!(resuming.await.unwrap_err().is_cancelled());
        drop(settled);
        bench.flow.crash().await;
        bench
            .flow
            .until("its recovered turn's end", |board| {
                Flow::task(board, 1).state == TaskState::Reported
            })
            .await;
        let recovered = bench.task().await;
        let plain = recovered.plain.as_ref().unwrap();
        assert!(plain.held.is_empty(), "{plain:?}");
        assert!(plain.resume_input.is_none(), "{plain:?}");
        let transcript = bench
            .flow
            .dir
            .join("data/transcripts")
            .join(recovered.native_session.as_ref().unwrap());
        let inputs = std::fs::read_to_string(&transcript).unwrap();
        assert_eq!(inputs.matches("Use blue.").count(), 1, "{inputs}");
        assert_eq!(inputs.matches(WORDS).count(), 1, "{inputs}");
        // A second restart must not replay an already reconciled resume.
        bench.flow.restart().await;
        let inputs = std::fs::read_to_string(transcript).unwrap();
        assert_eq!(inputs.matches("Use blue.").count(), 1, "{inputs}");
        bench.flow.stop().await;
    }
}

/// The composer's direct delivery holds settle while reviving an idle CLI. Pause must
/// refuse before cancelling that launch, and may be retried once the message is taken.
#[tokio::test]
async fn a_pause_during_an_idle_workers_revival_says_try_again() {
    let bench = bench(
        "20261030-1141-core-goals-w79-revive-pause",
        start("Build it."),
        Mode::Hold,
        false,
    )
    .await;
    bench.release.notify_waiters();
    bench
        .flow
        .until("its idle worker", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    let task = bench.task().await;
    let live = bench.flow.manager.task_live(&task);
    live.close_cli().await;
    live.allow_revival().await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *bench.flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release.clone()));
    let manager = bench.flow.manager.clone();
    let recipient = task.clone();
    let sending = tokio::spawn(async move {
        manager
            .deliver_direct(
                &recipient.conversation_id,
                &recipient,
                "Use blue.",
                &[],
                &[],
            )
            .await
    });
    reached.notified().await;
    let answer = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        bench.flow.manager.pause_task(task.id.clone()),
    )
    .await
    .expect("pause must answer while its CLI is starting");
    assert!(answer.unwrap_err().to_string().contains("try again"));
    release.notify_one();
    sending.await.unwrap().unwrap();
    bench
        .flow
        .until("its message turn's end", |board| {
            Flow::task(board, 1).state == TaskState::Reported
        })
        .await;
    assert_eq!(bench.with("Use blue.").len(), 1);
    assert_eq!(bench.steps(paused).await, 0);
    assert!(bench.flow.manager.pause_task(task.id).await.unwrap());
    assert_eq!(bench.task().await.state, TaskState::Paused);
    assert_eq!(bench.steps(paused).await, 1);
    bench.flow.stop().await;
}
