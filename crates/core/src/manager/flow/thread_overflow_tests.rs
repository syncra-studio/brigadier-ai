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
use crate::manager::prompts::{
    ANSWER_DETAILS_REVISION, ANSWER_FILES_REVISION, ANSWER_FORMAT_REVISION, COMMIT_FIRST_REVISION,
    LEAN_TOOLS_REVISION, OVERFLOW_REVISION, SAVE_ONLY_REVISION, SHORT_START_REVISION,
    STORED_FIX_REVISION, THREAD_STEPS_REVISION, revision_note,
};
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
/// The stopped session's handoff note, written in a fork once its CLI closed.
const NOTE: &str = "Failed checks: cargo test lexer::spans failed at src/lexer.rs:42 (not fixed).";

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

/// The steer lines and the stretch on a 200k window: Claude steers at 128.5k, 5k apart, and
/// stops at 142.8k (it refuses past about 175k); Codex steers at 137.7k, 5.1k apart, and stops at 153k.
fn steps(provider: ProviderKind) -> [i64; 3] {
    match provider {
        ProviderKind::Claude => [130_000, 133_000, 136_000],
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
            if turn.input.contains("Write the handoff note") {
                return Reply::text(NOTE);
            }
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

/// What the stopped session checked and found so far came in its handoff note (#113).
fn noted(fresh: &str) {
    assert!(
        fresh.contains("[Handoff note from your earlier context]") && fresh.contains(NOTE),
        "{fresh}"
    );
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
    noted(&fresh);
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
    noted(&continued(&run).await);
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
    noted(&continued(&run).await);
    run.flow.stop().await;
}

/// A Claude session learns its model's window only when its first turn ends. Until then the
/// lines aren't known, and a guess (a 200k window) would stop a 1M-window thread far too early:
/// a long first turn goes on, unasked and unstopped. Once a session of the model reported its
/// window, the model's later sessions use it from their first report.
#[tokio::test]
async fn a_first_turn_with_no_window_known_yet_is_neither_steered_nor_stopped() {
    let script = script(move |turn| async move {
        if !turn.is_orchestrator() || !turn.input.contains(REQUEST) {
            return Reply::text("Noted.");
        }
        // The steer and the stop wait on the same lines: with none known, neither comes.
        for tokens in [20_000, 160_000, 300_000] {
            turn.emit(ProviderEvent::ContextSize {
                used_tokens: tokens,
                window_tokens: None,
            })
            .await;
        }
        Reply::text("The parser is built.")
    });
    let flow = Flow::start("thread-overflow-no-window", Options::default(), script).await;
    flow.say(REQUEST).await;
    assert_eq!(ended(&flow).await, RequestState::Done);
    assert!(
        !entries(&flow)
            .await
            .iter()
            .any(|entry| matches!(entry, OrchestratorEntry::StoppedForRebirth { .. })),
        "not stopped"
    );
    // A window reported once is kept for the model's later sessions, and only for its model.
    let manager = &flow.manager;
    assert_eq!(
        manager.model_window(ProviderKind::Claude, Some("opus"), Some(1_000_000)),
        Some(1_000_000)
    );
    assert_eq!(
        manager.model_window(ProviderKind::Claude, Some("opus"), None),
        Some(1_000_000)
    );
    assert_eq!(
        manager.model_window(ProviderKind::Claude, Some("sonnet"), None),
        None
    );
    assert_eq!(
        manager.model_window(ProviderKind::Codex, Some("opus"), None),
        None
    );
    // The CLI's default model may differ between sessions: nothing is shared for it.
    assert_eq!(
        manager.model_window(ProviderKind::Claude, None, Some(200_000)),
        Some(200_000)
    );
    assert_eq!(manager.model_window(ProviderKind::Claude, None, None), None);
    flow.stop().await;
}

/// A fresh session that the model refuses as too long before it makes any call can't make
/// progress: the request fails once, saying so, instead of being reborn again and again.
#[tokio::test]
async fn a_fresh_session_refused_again_before_any_call_fails_the_request_once() {
    let (first, fresh, go) = (
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
        Arc::new(Notify::new()),
    );
    let fresh_turns = Arc::new(Mutex::new(0));
    let script = {
        let (first, fresh, go, fresh_turns) = (
            first.clone(),
            fresh.clone(),
            go.clone(),
            fresh_turns.clone(),
        );
        script(move |turn| {
            let (first, fresh, go, fresh_turns) = (
                first.clone(),
                fresh.clone(),
                go.clone(),
                fresh_turns.clone(),
            );
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                let starts = if turn.input.contains(STOPPED) {
                    *fresh_turns.lock().unwrap() += 1;
                    &fresh
                } else if turn.input.contains(REQUEST) {
                    &first
                } else {
                    return Reply::text("Noted.");
                };
                context(&turn, 20_000).await;
                starts.notify_one();
                go.notified().await;
                // Its next event fires the armed fault.
                turn.emit(ProviderEvent::Notice {
                    level: NoticeLevel::Info,
                    message: "reading".into(),
                })
                .await;
                turn.stopped().await;
                Reply::text("Still reading.")
            }
        })
    };
    let flow = Flow::start("thread-overflow-unproven", Options::default(), script).await;
    let refuse = || async {
        flow.manager
            .debug_inject_error(
                FaultKey::Conversation(flow.conversation.clone()),
                ProviderKind::Claude,
                InjectedError::ContextWindow,
                0,
            )
            .await
            .unwrap();
    };
    flow.say(REQUEST).await;
    first.notified().await;
    refuse().await;
    go.notify_one();
    fresh.notified().await;
    refuse().await;
    go.notify_one();
    match ended(&flow).await {
        RequestState::Failed { error } => {
            assert!(
                error.contains("A fresh session couldn't take this request"),
                "{error}"
            )
        }
        other => panic!("the request should fail, not {other:?}"),
    }
    assert_eq!(*fresh_turns.lock().unwrap(), 1, "reborn once, not again");
    let stops = entries(&flow)
        .await
        .into_iter()
        .filter(|entry| matches!(entry, OrchestratorEntry::StoppedForRebirth { .. }))
        .count();
    assert_eq!(stops, 1);
    flow.stop().await;
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
    told_once(OVERFLOW_REVISION, "near your model's limit").await;
}

/// Rule R for search_transcript's thread steps (BUILD-PLAN D2.4).
#[tokio::test]
async fn a_session_from_before_thread_steps_in_search_is_told_once() {
    told_once(THREAD_STEPS_REVISION, "thread step N").await;
}

/// Rule R for the Details of every final answer (BUILD-PLAN E3), here for its helper.
#[tokio::test]
async fn a_session_from_before_details_on_every_answer_is_told_once() {
    told_once(ANSWER_DETAILS_REVISION, "an explanation of an error too").await;
}

/// Rule R for the restored voice and the research answer's format (Q79).
#[tokio::test]
async fn a_session_from_before_the_answer_format_is_told_once() {
    told_once(ANSWER_FORMAT_REVISION, "End with one `### Sources` heading").await;
}

/// Rule R for the commit on the first check expected to pass (the gate's bench, T4).
#[tokio::test]
async fn a_session_from_before_commit_on_the_first_check_is_told_once() {
    told_once(
        COMMIT_FIRST_REVISION,
        "on the first check you expect to pass",
    )
    .await;
}

/// Rule R for the shorter instructions, whose findings and merge rules moved into the messages
/// that need them (BUILD-PLAN O2).
#[tokio::test]
async fn a_session_from_before_the_short_start_is_told_once() {
    told_once(
        SHORT_START_REVISION,
        "where they say more than your instructions, follow them",
    )
    .await;
}

/// Rule R for a Claude thread's `run` behind its tool search (BUILD-PLAN O5).
#[tokio::test]
async fn a_session_from_before_the_lean_tools_is_told_once() {
    told_once(
        LEAN_TOOLS_REVISION,
        "load it through ToolSearch by its name",
    )
    .await;
    // A try by hand stays out of run_check, where a T4 bench run put it in the check's command
    // and a T6 one ran a scratch script as a check (BUILD-PLAN O3).
    told_once(
        LEAN_TOOLS_REVISION,
        "A try by hand or a one-off script is a shell call of its own, never run_check or part of its command.",
    )
    .await;
    // The shell stops a command at two minutes, so a longer one still goes through run.
    told_once(
        LEAN_TOOLS_REVISION,
        "your shell stops a command at two minutes, so one that may run longer still goes through run",
    )
    .await;
}

/// Rule R for the Save… under an answer, which offers only the worker files it links (#218).
#[tokio::test]
async fn a_session_from_before_the_answer_files_is_told_once() {
    told_once(
        ANSWER_FILES_REVISION,
        "link one you hand over as [notes.md](outputs/notes.md)",
    )
    .await;
}

/// Rule R for handing a worker's file over with its Save only (#43).
#[tokio::test]
async fn a_session_from_before_save_only_is_told_once() {
    told_once(
        SAVE_ONLY_REVISION,
        "never offer a card to copy it somewhere (Downloads, the Desktop)",
    )
    .await;
}

/// Rule R for keeping a corrected deliverable with store_output (#27).
#[tokio::test]
async fn a_session_from_before_stored_fixes_is_told_once() {
    told_once(
        STORED_FIX_REVISION,
        "keep your corrected copy with store_output as that task's output",
    )
    .await;
}

/// A session told the instructions at the revision before `revision` hears its note (which
/// says `words`) exactly once after a restart.
async fn told_once(revision: u32, words: &str) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = inputs.clone();
    let mut flow = Flow::start(
        &format!("thread-rule-r-{revision}"),
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
                            revision: Some(revision - 1),
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
    let note = revision_note(revision - 1, false).unwrap();
    assert!(note.contains(words), "{note}");
    let told: usize = inputs
        .lock()
        .unwrap()
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{:?}", inputs.lock().unwrap());
    flow.stop().await;
}
