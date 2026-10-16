//! Workers off CLI compaction (BUILD-PLAN D2.3): on a 200k-window model, a worker whose
//! context grows past the hand-off size mid-turn is asked to wrap up, asked again after each
//! further stretch of growth, and past the hard line its turn is stopped and a fresh session
//! of the same model takes over; a compaction its CLI begins and the hook refuses does the
//! same. No compaction is recorded, on either provider.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderEvent;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const WINDOW: i64 = 200_000;

async fn context(turn: &Turn, tokens: i64) {
    turn.emit(ProviderEvent::ContextSize {
        used_tokens: tokens,
        window_tokens: Some(WINDOW),
    })
    .await;
}

/// What the first session of the worker does after it starts small.
#[derive(Clone, Copy)]
enum Growth {
    /// Its context grows through the hand-off size and on past the hard line.
    PastTheLine,
    /// Its CLI begins compacting and the hook refuses it.
    Compacting,
}

/// What the first session saw, and the fresh session's first message.
#[derive(Default)]
struct Seen {
    steers: Vec<String>,
    stopped: bool,
    fresh: Option<String>,
}

fn growing(provider: &'static str, growth: Growth, seen: &Arc<Mutex<Seen>>) -> Script {
    let seen = seen.clone();
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            if turn.is_orchestrator() {
                if turn.input.contains("Read the logs.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Read the logs",
                                   "kind": "scout", "spec": "Read the build logs.",
                                   "provider": provider}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                return Reply::text("Noted.");
            }
            if turn.input.contains("You are continuing task-") {
                seen.lock().unwrap().fresh = Some(turn.input.clone());
                let reply = turn
                    .call("submit_report", json!({"summary": "Read them."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            context(&turn, 20_000).await;
            match growth {
                Growth::PastTheLine => {
                    // Past the hand-off size (140k): asked to wrap up.
                    context(&turn, 145_000).await;
                    let first = turn.steered().await.expect("asked to wrap up");
                    // Not yet a stretch further: not asked again.
                    context(&turn, 150_000).await;
                    // A stretch further: asked again.
                    context(&turn, 158_000).await;
                    let second = turn.steered().await.expect("asked again");
                    seen.lock().unwrap().steers = vec![first, second];
                    // Past the hard line of either provider (153k Codex, 170k Claude).
                    context(&turn, 175_000).await;
                }
                Growth::Compacting => turn.emit(ProviderEvent::CompactionRefused).await,
            }
            seen.lock().unwrap().stopped = turn.stopped().await;
            Reply::text("Still reading.")
        }
    })
}

async fn run(provider: &'static str, growth: Growth) -> (Flow, Seen) {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let flow = Flow::start(
        &format!("near-limit-{provider}"),
        Options::default(),
        growing(provider, growth, &seen),
    )
    .await;
    flow.say("Read the logs.").await;
    flow.until("the scout reported", |board| {
        board.tasks.values().any(|task| task.report.is_some())
    })
    .await;
    let seen = std::mem::take(&mut *seen.lock().unwrap());
    (flow, seen)
}

/// The worker's recorded events.
async fn worker_events(flow: &Flow) -> Vec<ProviderEvent> {
    let board = flow.board().await;
    let task = board.tasks.keys().next().expect("the scout").clone();
    let page = flow
        .manager
        .core
        .store()
        .read_stream_since(crate::model::streams::task(&task), 0, 10_000)
        .await
        .unwrap();
    page.iter()
        .filter_map(
            |stored| match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                Ok(DomainEvent::WorkerEvent { event, .. }) => Some(event),
                _ => None,
            },
        )
        .collect()
}

async fn past_the_hard_line(provider: &'static str) {
    let (flow, seen) = run(provider, Growth::PastTheLine).await;
    assert_eq!(seen.steers.len(), 2, "asked twice");
    assert!(
        seen.steers
            .iter()
            .all(|steer| steer.contains("handoff note"))
    );
    assert!(seen.stopped, "its turn was stopped at the hard line");
    let fresh = seen.fresh.expect("a fresh session took over");
    assert!(fresh.contains("neared the model's limit"), "{fresh}");
    assert!(fresh.contains("transcript.md is complete."), "{fresh}");
    let board = flow.board().await;
    let task = board.tasks.values().next().unwrap();
    assert_eq!(task.attempts.len(), 1, "the same model and attempt");
    let events = worker_events(&flow).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, ProviderEvent::CompactionStarted { .. })),
        "no compaction"
    );
    assert!(events.iter().any(|event| matches!(event,
        ProviderEvent::Notice { message, .. } if message.contains("stopped the turn there"))));
    // Its sessions launch with their CLI's compaction off.
    let workspace = task.workspace.as_ref().expect("its workspace");
    let places: Vec<&str> = [Some(&workspace.scratch), workspace.worktree.as_ref()]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .collect();
    let workers: Vec<_> = flow
        .specs()
        .into_iter()
        .filter(|(_, spec)| {
            places.iter().any(|place| {
                spec.cwd.starts_with(place)
                    || spec.add_dirs.iter().any(|dir| dir.starts_with(place))
            })
        })
        .collect();
    assert_eq!(workers.len(), 2, "its first session and the fresh one");
    assert!(workers.iter().all(|(_, spec)| !spec.auto_compact));
    flow.stop().await;
}

#[tokio::test]
async fn a_claude_worker_past_the_hard_line_is_handed_off_mid_turn_and_never_compacted() {
    past_the_hard_line("claude").await;
}

#[tokio::test]
async fn a_codex_worker_past_the_hard_line_is_handed_off_mid_turn_and_never_compacted() {
    past_the_hard_line("codex").await;
}

#[tokio::test]
async fn a_refused_compaction_hands_the_worker_off() {
    let (flow, seen) = run("claude", Growth::Compacting).await;
    assert!(seen.stopped);
    let fresh = seen.fresh.expect("a fresh session took over");
    assert!(fresh.contains("neared the model's limit"), "{fresh}");
    let events = worker_events(&flow).await;
    assert!(
        events.iter().any(|event| matches!(event,
        ProviderEvent::Notice { message, .. } if message.contains("that was refused"))),
        "{events:#?}"
    );
    flow.stop().await;
}
