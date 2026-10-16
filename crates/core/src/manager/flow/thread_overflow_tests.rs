//! The thread's in-turn overflow guard (BUILD-PLAN D2.3b): on a 200k-window model, a thread
//! whose context grows mid-turn is asked to commit past the steer line, asked again after each
//! further stretch of growth, and past the hard line its turn is stopped and it is reborn at
//! once: a fresh session continues the request from a briefing that says where the stopped one
//! stood. A compaction its CLI begins, or "prompt is too long", does the same; a daemon that
//! dies between the stop and the rebirth finishes it after the restart. No compaction is
//! recorded, on either provider, and the request never fails for it.

use std::sync::{Arc, Mutex};

use brigadier_providers::{NoticeLevel, ProviderEvent, ProviderKind};
use tokio::sync::Notify;

use super::{Flow, Options, Reply, Script, Turn};
use crate::knowledge::RebirthTrigger;
use crate::manager::fault::{FaultKey, InjectedError};
use crate::manager::prompts::{OVERFLOW_REVISION, revision_note};
use crate::model::{DomainEvent, streams};
use crate::work::{OrchestratorEntry, RequestState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const WINDOW: i64 = 200_000;
const REQUEST: &str = "Build the parser.";
const STOPPED: &str = "[Where your earlier session stopped]";

async fn context(turn: &Turn, tokens: i64) {
    turn.emit(ProviderEvent::ContextSize {
        used_tokens: tokens,
        window_tokens: Some(WINDOW),
    })
    .await;
}

/// What the stopped turn does after it starts small.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Growth {
    /// Its context grows past the steer line, a stretch further, and on past the hard line.
    PastTheLine,
    /// Its CLI begins compacting and the session's hook refuses it.
    Compacting,
    /// Its model refuses the context ("prompt is too long", injected by the test).
    TooLong,
}

/// What the first session saw, and the fresh session's first input.
#[derive(Default)]
struct Seen {
    steers: Vec<String>,
    stopped: bool,
    first_native: Option<String>,
    fresh: Option<(String, String)>,
}

/// The steer lines and the stretch on a 200k window: Claude steers at 153k, 5.7k apart, and
/// stops at 170k; Codex steers at 137.7k, 5.1k apart, and stops at 153k.
fn steps(provider: ProviderKind) -> [i64; 3] {
    match provider {
        ProviderKind::Claude => [155_000, 158_000, 162_000],
        ProviderKind::Codex => [140_000, 143_000, 146_000],
    }
}

fn thread(
    growth: Growth,
    seen: &Arc<Mutex<Seen>>,
    started: &Arc<Notify>,
    go: &Arc<Notify>,
) -> Script {
    let (seen, started, go) = (seen.clone(), started.clone(), go.clone());
    script(move |turn| {
        let (seen, started, go) = (seen.clone(), started.clone(), go.clone());
        async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            if turn.input.contains(STOPPED) {
                seen.lock().unwrap().fresh = Some((turn.input.clone(), turn.native_id.clone()));
                return Reply::text("The parser is built.");
            }
            if !turn.input.contains(REQUEST) {
                return Reply::text("Noted.");
            }
            seen.lock().unwrap().first_native = Some(turn.native_id.clone());
            context(&turn, 20_000).await;
            match growth {
                Growth::PastTheLine => {
                    let [first, not_yet, again] = steps(turn.provider);
                    context(&turn, first).await;
                    let first = turn.steered().await.expect("asked to commit");
                    // Not yet a stretch further: not asked again.
                    context(&turn, not_yet).await;
                    context(&turn, again).await;
                    let second = turn.steered().await.expect("asked again");
                    seen.lock().unwrap().steers = vec![first, second];
                    started.notify_one();
                    // Past the hard line of either provider.
                    context(&turn, 175_000).await;
                }
                Growth::Compacting => turn.emit(ProviderEvent::CompactionRefused).await,
                Growth::TooLong => {
                    started.notify_one();
                    go.notified().await;
                    // Its next event fires the armed fault.
                    turn.emit(ProviderEvent::Notice {
                        level: NoticeLevel::Info,
                        message: "building".into(),
                    })
                    .await;
                }
            }
            seen.lock().unwrap().stopped = turn.stopped().await;
            Reply::text("Still building.")
        }
    })
}

struct Run {
    flow: Flow,
    seen: Arc<Mutex<Seen>>,
    started: Arc<Notify>,
    go: Arc<Notify>,
}

async fn start(name: &str, provider: ProviderKind, growth: Growth) -> Run {
    let (seen, started, go): (Arc<Mutex<Seen>>, Arc<Notify>, Arc<Notify>) = Default::default();
    let flow = Flow::start(
        name,
        Options {
            thread: provider,
            ..Options::default()
        },
        thread(growth, &seen, &started, &go),
    )
    .await;
    Run {
        flow,
        seen,
        started,
        go,
    }
}

/// The request ended, and how.
async fn ended(flow: &Flow) -> RequestState {
    let board = flow
        .until("the request ended", |board| {
            board.requests.values().any(|request| {
                !matches!(request.state, RequestState::Working | RequestState::Waiting)
            })
        })
        .await;
    board.requests.values().next().unwrap().state.clone()
}

/// The thread's orchestrator log.
async fn entries(flow: &Flow) -> Vec<OrchestratorEntry> {
    flow.manager
        .core
        .store()
        .read_stream_since(streams::orchestrator(&flow.conversation), 0, 10_000)
        .await
        .unwrap()
        .iter()
        .filter_map(
            |stored| match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                Ok(DomainEvent::OrchestratorLogged { entry, .. }) => Some(entry),
                _ => None,
            },
        )
        .collect()
}

/// The request went on in a fresh session reborn mid-turn, with no compaction recorded.
async fn continued(run: &Run) -> String {
    assert_eq!(ended(&run.flow).await, RequestState::Done);
    let (fresh, native, first) = {
        let seen = run.seen.lock().unwrap();
        let (fresh, native) = seen.fresh.clone().expect("a fresh session continued");
        (fresh, native, seen.first_native.clone())
    };
    assert_ne!(Some(native), first, "a new CLI session");
    assert!(
        fresh.contains("Brigadier stopped your earlier session's turn"),
        "{fresh}"
    );
    assert!(fresh.contains("transcript.md"), "{fresh}");
    assert!(fresh.contains("The last commit:"), "{fresh}");
    let entries = entries(&run.flow).await;
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, OrchestratorEntry::StoppedForRebirth { .. })),
        "the stop was recorded"
    );
    assert!(entries.iter().any(|entry| matches!(entry,
        OrchestratorEntry::Rebirth { record } if record.trigger == RebirthTrigger::MidTurn)));
    assert!(
        !entries.iter().any(|entry| matches!(
            entry,
            OrchestratorEntry::Provider {
                event: ProviderEvent::CompactionStarted { .. },
                ..
            }
        )),
        "no compaction"
    );
    // Its sessions launch with their CLI's compaction off.
    assert!(
        run.flow
            .thread_specs()
            .iter()
            .all(|(_, spec)| !spec.auto_compact)
    );
    fresh
}

async fn past_the_hard_line(provider: ProviderKind) {
    let run = start(
        &format!("thread-overflow-{provider}"),
        provider,
        Growth::PastTheLine,
    )
    .await;
    run.flow.say(REQUEST).await;
    let fresh = continued(&run).await;
    {
        let seen = run.seen.lock().unwrap();
        assert_eq!(seen.steers.len(), 2, "asked twice");
        assert!(
            seen.steers
                .iter()
                .all(|steer| steer.contains("Commit the finished steps"))
        );
        assert!(seen.stopped, "its turn was stopped at the hard line");
    }
    assert!(
        fresh.contains(REQUEST),
        "the request is carried on: {fresh}"
    );
    run.flow.stop().await;
}

#[tokio::test]
async fn a_claude_thread_past_the_hard_line_is_reborn_mid_turn_and_its_request_goes_on() {
    past_the_hard_line(ProviderKind::Claude).await;
}

#[tokio::test]
async fn a_codex_thread_past_the_hard_line_is_reborn_mid_turn_and_its_request_goes_on() {
    past_the_hard_line(ProviderKind::Codex).await;
}

#[tokio::test]
async fn a_refused_compaction_rebirths_the_thread_mid_turn() {
    let run = start(
        "thread-overflow-compacting",
        ProviderKind::Claude,
        Growth::Compacting,
    )
    .await;
    run.flow.say(REQUEST).await;
    continued(&run).await;
    assert!(run.seen.lock().unwrap().stopped);
    run.flow.stop().await;
}

/// "Prompt is too long" on the thread is no failed request: the thread is reborn and the
/// request goes on.
#[tokio::test]
async fn prompt_too_long_on_the_thread_rebirths_it_and_the_request_goes_on() {
    let run = start(
        "thread-overflow-too-long",
        ProviderKind::Claude,
        Growth::TooLong,
    )
    .await;
    run.flow.say(REQUEST).await;
    run.started.notified().await;
    run.flow
        .manager
        .debug_inject_error(
            FaultKey::Conversation(run.flow.conversation.clone()),
            ProviderKind::Claude,
            InjectedError::ContextWindow,
            0,
        )
        .await
        .unwrap();
    run.go.notify_one();
    continued(&run).await;
    run.flow.stop().await;
}

/// The daemon dies after the stop was recorded and before the rebirth: the restart rebirths
/// the thread and finishes the request instead of failing it as cut off.
#[tokio::test]
async fn a_daemon_killed_between_the_stop_and_the_rebirth_finishes_the_request() {
    let mut run = start(
        "thread-overflow-crash",
        ProviderKind::Claude,
        Growth::PastTheLine,
    )
    .await;
    let (reached, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
    let conv = run.flow.manager.conv(&run.flow.conversation).unwrap();
    *conv.rebirth_stop_pause.lock().unwrap() = Some((reached.clone(), release.clone()));
    run.flow.say(REQUEST).await;
    reached.notified().await;
    run.flow.crash().await;
    let fresh = continued(&run).await;
    assert!(
        fresh.contains(&format!("The request, in the user's words: {REQUEST}")),
        "{fresh}"
    );
    // The first session's turn was never stopped: the daemon died first.
    assert!(!run.seen.lock().unwrap().stopped);
    run.flow.stop().await;
}

/// Rule R: a session told the instructions before this change hears the note once.
#[tokio::test]
async fn a_session_from_before_the_overflow_guard_is_told_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let mut flow = Flow::start(
        "thread-overflow-rule-r",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.is_orchestrator() {
                    seen.lock().unwrap().push(turn.input.clone());
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    flow.core
        .record(vec![(
            streams::orchestrator(&flow.conversation),
            DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(OVERFLOW_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Fix the page.").await;
    flow.settled().await;
    flow.say("Thanks.").await;
    flow.settled().await;
    let note = revision_note(OVERFLOW_REVISION - 1, false).unwrap();
    assert!(note.contains("near your model's limit"), "{note}");
    let told: usize = inputs
        .lock()
        .unwrap()
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{:?}", inputs.lock().unwrap());
    flow.stop().await;
}
