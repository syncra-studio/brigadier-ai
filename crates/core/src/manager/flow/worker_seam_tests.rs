//! A worker's hand-off to a fresh CLI session (D14): its CLI compacts by itself, and its third
//! compaction in one CLI session asks for its note once its turn ends (its session heard that
//! end); the next message its session sends starts a fresh session as the same worker, in the
//! same worktree with its setup not run again, from the note and the `brigadier context`
//! pointer (D13). A failed compaction or a thrash hands off at once, unheard; a context too
//! long for the model hands off without a note. A restart at each step starts one successor
//! and sends nothing twice.

use std::sync::{Arc, Mutex};

use brigadier_providers::{ErrorKind, ProviderError, ProviderEvent};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::long::read_long;
use crate::manager::seam::{CARRY_ON, CONTEXT_POINTER, Phase, SeamFile, read_seam};
use crate::thread_view::{HANDOFF_MARK, NUDGE_MARK};
use crate::work::Task;

/// What each turn was given, with its CLI session: the worker's and its session's apart.
#[derive(Default)]
struct Seen {
    worker: Mutex<Vec<(String, String)>>,
    parent: Mutex<Vec<String>>,
}

impl Seen {
    /// The worker's inputs whose text holds `needle`.
    fn given(&self, needle: &str) -> Vec<(String, String)> {
        self.worker
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, input)| input.contains(needle))
            .cloned()
            .collect()
    }

    /// How many of the worker's turn ends its session heard as `word` ("finished", "failed").
    fn heard(&self, word: &str) -> usize {
        self.parent
            .lock()
            .unwrap()
            .iter()
            .filter(|input| input.contains("[worker events wn-"))
            .map(|input| input.matches(&format!("): {word}")).count())
            .sum()
    }
}

/// The note path a nudge names.
fn note_of(nudge: &str) -> std::path::PathBuf {
    let rest = nudge
        .split("write the hand-off note at ")
        .nth(1)
        .expect("a note path");
    rest.split(": doing now").next().unwrap().into()
}

fn error(kind: ErrorKind, message: &str) -> ProviderEvent {
    ProviderEvent::Error {
        error: ProviderError {
            kind,
            message: message.into(),
            will_retry: false,
            limit: None,
            code: None,
        },
    }
}

/// Its CLI compacting by itself mid-turn, `failed` or not.
async fn compact(turn: &Turn, failed: bool) {
    turn.emit(ProviderEvent::CompactionStarted { automatic: true })
        .await;
    turn.emit(ProviderEvent::CompactionEnded {
        automatic: true,
        tokens_before: Some(150_000),
        tokens_after: Some(20_000),
        error: failed.then(|| "Compaction failed.".to_owned()),
        native_id: None,
    })
    .await;
}

/// The worker: "Grow." compacts once; "Fail." compacts and fails to; "Thrash." stops as
/// Claude's thrash guard does; "Overflow." is too long for the model; a nudge writes its note.
/// Its session starts it on "Start one." and hears the rest.
fn script(seen: Arc<Seen>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.worker().is_none() {
                if turn.conversation().is_none() {
                    return Reply::text("ok");
                }
                seen.parent.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Start one.") {
                    turn.start_worker("Grower", "Begin.").await;
                    return Reply::text("Started.");
                }
                return Reply::text("Heard.");
            }
            seen.worker
                .lock()
                .unwrap()
                .push((turn.native_id.clone(), turn.input.clone()));
            let said = crate::manager::seam::without_handoff(&turn.input);
            if turn.input.starts_with(NUDGE_MARK) {
                std::fs::write(
                    note_of(&turn.input),
                    "Doing now: growing.\nNext: grow more.",
                )
                .unwrap();
                return Reply::text("Handed off.");
            }
            if said.contains("Grow.") {
                compact(&turn, false).await;
                return Reply::text("Grown.");
            }
            if said.contains("Fail.") {
                compact(&turn, true).await;
                return Reply::failed("Compaction failed.");
            }
            if said.contains("Thrash.") {
                turn.emit(error(
                    ErrorKind::Other,
                    "Autocompact is thrashing: the context refilled right after compacting.",
                ))
                .await;
                return Reply::failed("Autocompact is thrashing.");
            }
            if said.contains("Overflow.") {
                turn.emit(error(ErrorKind::ContextWindow, "Prompt is too long"))
                    .await;
                return Reply::failed("Prompt is too long");
            }
            Reply::text("Done.")
        })
    })
}

/// Its worktree's setup script counts its runs.
const SETUP: &str = "#!/bin/sh\necho ran >> .setup-runs\n";

async fn flow(name: &str, seen: Arc<Seen>) -> (Flow, Task) {
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            ..FakeBehavior::default()
        }),
        files: vec![(".brigadier/setup", SETUP), (".gitignore", ".setup-runs\n")],
        ..Options::default()
    };
    let flow = Flow::start(name, options, script(seen.clone())).await;
    flow.say("Start one.").await;
    flow.until("its worker started", |board| !board.tasks.is_empty())
        .await;
    flow.idle().await;
    let task = worker(&flow).await;
    until(&flow, "its first turn's end heard", || {
        seen.heard("finished") == 1
    })
    .await;
    flow.idle().await;
    (flow, task)
}

async fn worker(flow: &Flow) -> Task {
    let board = flow.board().await;
    Flow::task(&board, 1).clone()
}

fn record(flow: &Flow, task: &Task) -> std::path::PathBuf {
    flow.manager
        .session_dir(&flow.conversation)
        .worker(task)
        .join("record")
}

fn seam_file(flow: &Flow, task: &Task) -> SeamFile {
    read_seam(&record(flow, task))
}

async fn until(flow: &Flow, what: &str, done: impl Fn() -> bool) {
    flow.until(what, |_| done()).await;
}

/// Sends the worker `text`, as its session's `brigadier send` does, and waits until it is idle.
async fn send(flow: &Flow, task: &Task, text: &str) {
    let task = flow
        .manager
        .task_by_id(&flow.conversation, &task.id)
        .await
        .unwrap();
    flow.manager
        .message_worker(&flow.conversation, &task, text.into())
        .await
        .unwrap();
}

#[tokio::test]
async fn the_third_compaction_hands_a_worker_to_a_fresh_session_in_its_worktree() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-count", seen.clone()).await;
    let first = seen.worker.lock().unwrap()[0].0.clone();
    for n in 1..=2 {
        send(&flow, &task, "Grow.").await;
        until(&flow, "the grow heard", || seen.heard("finished") == n + 1).await;
    }
    let long = read_long(&record(&flow, &task));
    assert_eq!((long.generation, long.compactions), (1, 2));
    assert!(seen.given(NUDGE_MARK).is_empty());

    // The third: its session hears the turn end, then the note is asked for in a turn of
    // its own, and its CLI closes.
    send(&flow, &task, "Grow.").await;
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    until(&flow, "the third grow heard", || {
        seen.heard("finished") == 4
    })
    .await;
    flow.idle().await;
    let nudges = seen.given(NUDGE_MARK);
    assert_eq!(nudges.len(), 1, "{nudges:?}");
    assert_eq!(nudges[0].0, first);

    // The next message starts its successor: a fresh CLI session, from the note and the
    // pointer, with that message, and nothing else put in (D13).
    send(&flow, &task, "Next.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 5
    })
    .await;
    flow.idle().await;
    let next = seen.given("Next.");
    assert_eq!(next.len(), 1, "{next:?}");
    let (successor, input) = &next[0];
    assert_ne!(successor, &first);
    assert!(input.starts_with(HANDOFF_MARK), "{input}");
    assert!(input.contains(CONTEXT_POINTER), "{input}");
    assert!(input.contains("Next: grow more."), "{input}");
    assert!(!input.contains(CARRY_ON), "{input}");
    let file = seam_file(&flow, &task);
    assert_eq!(&file.seams[0].successor, successor);
    assert_eq!(
        file.seams[0].end.predecessor.as_deref(),
        Some(first.as_str())
    );
    assert!(file.seams[0].end.death.is_none());
    let task = worker(&flow).await;
    assert_eq!(task.native_session.as_deref(), Some(successor.as_str()));

    // The same worktree, whose setup ran once.
    let specs: Vec<_> = flow
        .specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.env.iter().any(|(key, value)| {
                key == brigadier_cli::protocol::SESSION_ENV && *value == task.id.0
            })
        })
        .collect();
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].1.cwd, specs[1].1.cwd);
    assert_eq!(specs[0].1.add_dirs, specs[1].1.add_dirs);
    let worktree = std::path::PathBuf::from(
        task.workspace
            .clone()
            .and_then(|w| w.worktree)
            .expect("its worktree"),
    );
    let runs = std::fs::read_to_string(worktree.join(".setup-runs")).unwrap();
    assert_eq!(runs.lines().count(), 1, "{runs}");

    // Its thread shows the hand-off as a divider that folds the nudge in and takes the
    // hand-off off its successor's first message, as a session's does.
    let view = flow
        .manager
        .worker_thread(&flow.conversation, &task.id)
        .await
        .unwrap();
    assert_eq!(view.seams.len(), 1);
    assert!(view.seams[0].nudge.is_some(), "{:?}", view.seams);
    assert!(view.seams[0].at.is_some(), "{:?}", view.seams);
    let shown = format!("{:?}", view.entries);
    assert!(!shown.contains(HANDOFF_MARK), "{shown}");

    // Its successor counts its own compactions from one.
    send(&flow, &task, "Grow.").await;
    until(&flow, "its grow heard", || seen.heard("finished") == 6).await;
    let long = read_long(&record(&flow, &task));
    assert_eq!((long.generation, long.compactions), (2, 1));
    flow.stop().await;
}

#[tokio::test]
async fn a_failed_compaction_or_a_thrash_hands_off_at_once_unheard() {
    for (name, said) in [("fail", "Fail."), ("thrash", "Thrash.")] {
        let seen = Arc::new(Seen::default());
        let (flow, task) = flow(&format!("worker-seam-{name}"), seen.clone()).await;
        let first = seen.worker.lock().unwrap()[0].0.clone();
        send(&flow, &task, said).await;
        until(&flow, "the successor", || {
            seam_file(&flow, &task).generation == 2
        })
        .await;
        until(&flow, "the successor's turn heard", || {
            seen.heard("finished") == 2
        })
        .await;
        flow.idle().await;
        // Its note was asked for, and its successor carried on by itself.
        assert_eq!(seen.given(NUDGE_MARK).len(), 1, "{name}");
        let carried = seen.given(CARRY_ON);
        assert_eq!(carried.len(), 1, "{name}: {carried:?}");
        assert_ne!(carried[0].0, first);
        assert!(carried[0].1.contains("Next: grow more."), "{name}");
        assert!(carried[0].1.contains(CONTEXT_POINTER), "{name}");
        // Its session heard nothing of the turn that broke: only the successor's end.
        assert_eq!(seen.heard("failed"), 0, "{name}");
        flow.stop().await;
    }
}

#[tokio::test]
async fn a_context_too_long_hands_off_without_a_note() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-overflow", seen.clone()).await;
    send(&flow, &task, "Overflow.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 2
    })
    .await;
    flow.idle().await;
    assert!(seen.given(NUDGE_MARK).is_empty());
    let carried = seen.given(CARRY_ON);
    assert_eq!(carried.len(), 1, "{carried:?}");
    assert!(
        carried[0]
            .1
            .contains("ended without a hand-off note (its context was too long for the model)")
    );
    assert_eq!(
        seam_file(&flow, &task).seams[0].end.death.as_deref(),
        Some("its context was too long for the model")
    );
    assert_eq!(seen.heard("failed"), 0);
    flow.stop().await;
}

/// The worker grows to its third compaction; its note turn waits until `release`.
fn gated(
    seen: Arc<Seen>,
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
) -> Script {
    let base = script(seen);
    Arc::new(move |turn| {
        let (base, reached, release) = (base.clone(), reached.clone(), release.clone());
        Box::pin(async move {
            if turn.input.starts_with(NUDGE_MARK) {
                reached.notify_one();
                release.notified().await;
            }
            base(turn).await
        })
    })
}

async fn gated_flow(
    name: &str,
    seen: Arc<Seen>,
) -> (
    Flow,
    Task,
    Arc<tokio::sync::Notify>,
    Arc<tokio::sync::Notify>,
) {
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start(
        name,
        options,
        gated(seen.clone(), reached.clone(), release.clone()),
    )
    .await;
    flow.say("Start one.").await;
    flow.until("its worker started", |board| !board.tasks.is_empty())
        .await;
    let task = worker(&flow).await;
    until(&flow, "its first turn's end heard", || {
        seen.heard("finished") == 1
    })
    .await;
    flow.idle().await;
    for n in 1..=3 {
        send(&flow, &task, "Grow.").await;
        if n < 3 {
            until(&flow, "the grow heard", || seen.heard("finished") == n + 1).await;
        }
    }
    reached.notified().await;
    (flow, task, reached, release)
}

#[tokio::test]
async fn a_restart_in_the_note_turn_closes_the_generation_and_the_next_message_starts_one() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-restart-note", seen.clone()).await;
    // A message waits for the successor while the note turn runs.
    send(&flow, &task, "Meanwhile.").await;
    assert!(seen.given("Meanwhile.").is_empty());
    flow.crash().await;
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    assert_eq!(
        seam_file(&flow, &task).generation,
        1,
        "nothing started before its session sent more"
    );
    flow.settled().await;
    send(&flow, &task, "Next.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    flow.idle().await;
    let next = seen.given("Next.");
    assert_eq!(next.len(), 1, "{next:?}");
    assert!(next[0].1.starts_with(HANDOFF_MARK));
    assert!(
        next[0]
            .1
            .contains("ended without a hand-off note (Brigadier restarted")
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_before_the_successor_launched_sends_its_message_once() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, release) = gated_flow("worker-seam-crash-before", seen.clone()).await;
    release.notify_one();
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    // Its successor's CLI never comes up before the crash.
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    let (manager, conversation, held) = (
        flow.manager.clone(),
        flow.conversation.clone(),
        task.clone(),
    );
    tokio::spawn(async move {
        let task = manager.task_by_id(&conversation, &held.id).await.unwrap();
        let _ = manager
            .message_worker(&conversation, &task, "After.".into())
            .await;
    });
    reached.notified().await;
    let Phase::Starting {
        successor,
        messages,
        ..
    } = seam_file(&flow, &task).phase
    else {
        panic!("not starting")
    };
    assert_eq!(messages, vec!["After.".to_owned()]);
    flow.crash().await;
    flow.settled().await;
    assert!(matches!(
        seam_file(&flow, &task).phase,
        Phase::Closed { .. }
    ));
    // The next message starts another successor, carrying both, once.
    send(&flow, &task, "Next.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    flow.idle().await;
    let after = seen.given("After.");
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].1.starts_with(HANDOFF_MARK));
    assert!(after[0].1.contains("Next."));
    assert_ne!(after[0].0, successor);
    assert_eq!(seam_file(&flow, &task).seams[0].successor, after[0].0);
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_after_the_successor_took_its_handoff_resumes_it() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, release) = gated_flow("worker-seam-crash-after", seen.clone()).await;
    release.notify_one();
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    // Its successor took its input, and Brigadier dies before it heard so.
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_send.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    let (manager, conversation, held) = (
        flow.manager.clone(),
        flow.conversation.clone(),
        task.clone(),
    );
    tokio::spawn(async move {
        let task = manager.task_by_id(&conversation, &held.id).await.unwrap();
        let _ = manager
            .message_worker(&conversation, &task, "After.".into())
            .await;
    });
    reached.notified().await;
    let Phase::Starting { successor, .. } = seam_file(&flow, &task).phase else {
        panic!("not starting")
    };
    flow.crash().await;
    until(&flow, "the hand-off complete", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    flow.settled().await;
    assert_eq!(seam_file(&flow, &task).seams[0].successor, successor);
    let task = worker(&flow).await;
    assert_eq!(task.native_session.as_deref(), Some(successor.as_str()));
    // The next message resumes that successor: no second hand-off.
    send(&flow, &task, "Next.").await;
    until(&flow, "its turn", || !seen.given("Next.").is_empty()).await;
    flow.idle().await;
    let next = seen.given("Next.");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].0, successor);
    assert!(!next[0].1.contains(HANDOFF_MARK));
    assert_eq!(seam_file(&flow, &task).seams.len(), 1);
    flow.stop().await;
}
