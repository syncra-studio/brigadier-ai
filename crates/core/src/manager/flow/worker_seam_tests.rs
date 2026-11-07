//! A worker's hand-off to a fresh CLI session (D14), the same cycle as a thread's: its CLI
//! compacts by itself, and its third compaction in one CLI session asks for its note in the
//! running turn; a fresh session then goes on by itself as the same worker, in the same worktree
//! with its setup not run again, from the note and the `brigadier context` pointer (D13). A
//! failed compaction or a thrash hands off at once, unheard; a context too long for the model
//! hands off without a note. A restart at each step starts one successor and sends nothing
//! twice.

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
    /// The account each worker CLI session ran on, by its id.
    accounts: Mutex<Vec<(String, Option<String>)>>,
    /// The session starts its worker with a routing row, an effort and a granted server.
    routed: std::sync::atomic::AtomicBool,
    /// Its next note turn hits a usage limit.
    limit_note: std::sync::atomic::AtomicBool,
    /// Its next note turn stops as Claude's thrash guard does.
    thrash_note: std::sync::atomic::AtomicBool,
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

const THRASHING: &str = "Autocompact is thrashing: the context refilled right after compacting.";

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
                    if seen.routed.load(std::sync::atomic::Ordering::SeqCst) {
                        use brigadier_cli::protocol::{CliRequest, StartRequest};
                        let started = super::cli_tests::brigadier(
                            &turn,
                            CliRequest::Start(Box::new(StartRequest {
                                title: Some("Grower".into()),
                                work: Some("Mechanical changes and search".into()),
                                effort: Some("low".into()),
                                mcps: vec!["pencil".into()],
                                ..super::cli_tests::start("Begin.")
                            })),
                        )
                        .await;
                        assert!(started["worker"]["id"].is_string(), "{started:#}");
                    } else {
                        turn.start_worker("Grower", "Begin.").await;
                    }
                    return Reply::text("Started.");
                }
                return Reply::text("Heard.");
            }
            seen.worker
                .lock()
                .unwrap()
                .push((turn.native_id.clone(), turn.input.clone()));
            seen.accounts
                .lock()
                .unwrap()
                .push((turn.native_id.clone(), turn.account.clone()));
            let said = crate::manager::seam::without_handoff(&turn.input);
            if turn
                .input
                .contains("Hand-off check: your hand-off note at ")
            {
                // Goal 13's fix: the note names the open item it left out.
                let path = turn.input["Hand-off check: your hand-off note at ".len()
                    + turn
                        .input
                        .find("Hand-off check: your hand-off note at ")
                        .unwrap()..]
                    .split(" leaves out")
                    .next()
                    .unwrap()
                    .to_owned();
                let text = std::fs::read_to_string(&path).unwrap();
                std::fs::write(&path, format!("{text}\nCarrying L1.")).unwrap();
                return Reply::text("Fixed.");
            }
            if turn.input.starts_with(NUDGE_MARK) {
                if seen
                    .thrash_note
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    turn.emit(error(ErrorKind::Other, THRASHING)).await;
                    return Reply::failed(THRASHING);
                }
                if seen
                    .limit_note
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
                {
                    return Reply::limited();
                }
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
                turn.emit(error(ErrorKind::Other, THRASHING)).await;
                return Reply::failed(THRASHING);
            }
            // Its third compaction's note request is steered in, and Claude's thrash guard stops
            // the turn before it reads it.
            if said.contains("Grow and thrash.") {
                compact(&turn, false).await;
                let _ = turn.steered().await;
                turn.emit(error(ErrorKind::Other, THRASHING)).await;
                return Reply::failed(THRASHING);
            }
            // Compacting leaves the context near its point.
            if said.contains("Swell.") {
                turn.emit(ProviderEvent::CompactionEnded {
                    automatic: true,
                    tokens_before: Some(190_000),
                    tokens_after: Some(150_000),
                    error: None,
                    native_id: None,
                })
                .await;
                // Its note request is steered into this turn, which reads it before it ends, as
                // both CLIs do: Claude holds a late one's turn open, Codex refuses it.
                if let Some(nudge) = turn.steered().await {
                    std::fs::write(note_of(&nudge), "Doing now: swelling.\nNext: go on.").unwrap();
                    seen.worker
                        .lock()
                        .unwrap()
                        .push((turn.native_id.clone(), nudge));
                }
                return Reply::text("Swollen.");
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
    flow_then(name, seen, |_| Box::pin(async {})).await
}

/// The flow, set up by `before` before its session starts the worker.
async fn flow_then(
    name: &str,
    seen: Arc<Seen>,
    before: impl for<'a> FnOnce(
        &'a Flow,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>>,
) -> (Flow, Task) {
    let options = Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            take_in_steers: true,
            ..FakeBehavior::default()
        }),
        files: vec![(".brigadier/setup", SETUP), (".gitignore", ".setup-runs\n")],
        ..Options::default()
    };
    let flow = Flow::start(name, options, script(seen.clone())).await;
    before(&flow).await;
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

    // The third: its note is asked for in the running turn, and a fresh CLI session goes on by
    // itself, from the note and the pointer, with nothing else put in (D13). Its session hears
    // only the end of the work, the successor's.
    send(&flow, &task, "Grow.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 4
    })
    .await;
    flow.idle().await;
    let nudges = seen.given(NUDGE_MARK);
    assert_eq!(nudges.len(), 1, "{nudges:?}");
    assert_eq!(nudges[0].0, first);
    let next = seen.given(CARRY_ON);
    assert_eq!(next.len(), 1, "{next:?}");
    let (successor, input) = &next[0];
    assert_ne!(successor, &first);
    assert!(input.starts_with(HANDOFF_MARK), "{input}");
    assert!(input.contains(CONTEXT_POINTER), "{input}");
    assert!(input.contains("Next: grow more."), "{input}");
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
    until(&flow, "its grow heard", || seen.heard("finished") == 5).await;
    let long = read_long(&record(&flow, &task));
    assert_eq!((long.generation, long.compactions), (2, 1));
    // No compaction woke its session.
    let parent = seen.parent.lock().unwrap().clone();
    assert!(
        !parent
            .iter()
            .any(|input| input.contains("compacted its context")),
        "{parent:#?}"
    );
    flow.stop().await;
}

/// A thrash stop before its CLI read the steered note request: the note is asked for once more,
/// in a turn of its own, and the worker goes on from it, unheard (the live run's lost notes).
#[tokio::test]
async fn a_thrash_before_the_steered_note_request_is_read_asks_again_in_a_turn_of_its_own() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-thrash-reask", seen.clone()).await;
    for n in 1..=2 {
        send(&flow, &task, "Grow.").await;
        until(&flow, "the grow heard", || seen.heard("finished") == n + 1).await;
    }
    send(&flow, &task, "Grow and thrash.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 4
    })
    .await;
    flow.idle().await;
    // The steered request went unread; only the second reached a turn of its own.
    assert_eq!(seen.given(NUDGE_MARK).len(), 1);
    let file = seam_file(&flow, &task);
    assert!(file.seams[0].end.death.is_none(), "{:?}", file.seams[0]);
    assert!(file.seams[0].note.is_some());
    let carried = seen.given(CARRY_ON);
    assert_eq!(carried.len(), 1, "{carried:?}");
    assert!(carried[0].1.contains("Next: grow more."));
    assert_eq!(seen.heard("failed"), 0);
    flow.stop().await;
}

/// A note turn that thrashes too is asked no more: the generation ends without a note, and the
/// successor goes on, however often its CLI would stop again (the live run's endless loop).
#[tokio::test]
async fn a_second_thrash_ends_the_generation_without_asking_again() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-thrash-once", seen.clone()).await;
    for n in 1..=2 {
        send(&flow, &task, "Grow.").await;
        until(&flow, "the grow heard", || seen.heard("finished") == n + 1).await;
    }
    seen.thrash_note
        .store(true, std::sync::atomic::Ordering::SeqCst);
    send(&flow, &task, "Grow and thrash.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 4
    })
    .await;
    flow.idle().await;
    assert_eq!(seen.given(NUDGE_MARK).len(), 1);
    let file = seam_file(&flow, &task);
    assert!(file.seams[0].end.death.is_some(), "{:?}", file.seams[0]);
    assert_eq!(seen.given(CARRY_ON).len(), 1);
    flow.stop().await;
}

/// A compaction that leaves its context above half its point hands off at once, the first one
/// too, as a thread's does.
#[tokio::test]
async fn a_compaction_that_leaves_the_context_near_its_point_hands_off() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-half", seen.clone()).await;
    send(&flow, &task, "Swell.").await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 2
    })
    .await;
    flow.idle().await;
    assert_eq!(seen.given(NUDGE_MARK).len(), 1);
    assert_eq!(seen.given(CARRY_ON).len(), 1);
    assert!(seam_file(&flow, &task).seams[0].end.death.is_none());
    flow.stop().await;
}

/// What its session and the user send it is in its brief, in the order it came: where it reads
/// it after a compaction or a hand-off.
#[tokio::test]
async fn what_a_worker_is_sent_goes_in_its_brief() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-sent", seen.clone()).await;
    let caller = crate::manager::cli::CliCaller {
        conversation_id: flow.conversation.clone(),
        task_id: None,
        grant: String::new(),
    };
    flow.manager
        .cli_send(
            &caller,
            &task.id.0[task.id.0.len() - 8..],
            Some("From the session.".into()),
            None,
        )
        .await
        .unwrap();
    until(&flow, "its turn heard", || seen.heard("finished") == 2).await;
    flow.idle().await;
    flow.manager
        .send_to_worker(
            flow.conversation.clone(),
            task.id.clone(),
            "From the user.".into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    until(&flow, "its turn heard", || seen.heard("finished") == 3).await;
    flow.idle().await;
    assert_eq!(
        worker(&flow).await.messages,
        ["From the session.", "From the user."]
    );
    flow.stop().await;
}

/// A message its session sends while the third compaction's note is written waits for the
/// successor: the note turn never cuts it off on the old CLI (phase review).
#[tokio::test]
async fn a_message_sent_during_the_note_goes_to_the_successor() {
    let seen = Arc::new(Seen::default());
    let (flow, task, _, release) = gated_flow("worker-seam-race", seen.clone()).await;
    let first = seen.worker.lock().unwrap()[0].0.clone();
    send(&flow, &task, "Next.").await;
    release.notify_one();
    until(&flow, "the successor's turn", || {
        seen.given("Next.").iter().any(|(id, _)| id != &first)
    })
    .await;
    flow.idle().await;
    let next = seen.given("Next.");
    assert_eq!(next.len(), 1, "{next:?}");
    assert_ne!(next[0].0, first);
    assert!(next[0].1.starts_with(HANDOFF_MARK), "{}", next[0].1);
    assert!(!next[0].1.contains(CARRY_ON), "{}", next[0].1);
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
            take_in_steers: true,
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
async fn a_restart_in_the_note_turn_ends_the_generation_and_the_successor_goes_on() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-restart-note", seen.clone()).await;
    flow.crash().await;
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    flow.idle().await;
    let next = seen.given(CARRY_ON);
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
    send(&flow, &task, "After.").await;
    // Its successor's CLI never comes up before the crash.
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    release.notify_one();
    reached.notified().await;
    let Phase::Starting {
        successor,
        messages,
        ..
    } = seam_file(&flow, &task).phase
    else {
        panic!("not starting")
    };
    assert_eq!(messages.len(), 1, "{messages:?}");
    assert!(messages[0].starts_with("After."), "{messages:?}");
    flow.crash().await;
    // Another successor starts at once, carrying it, once.
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    flow.idle().await;
    let after = seen.given("After.");
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].1.starts_with(HANDOFF_MARK));
    assert_ne!(after[0].0, successor);
    assert_eq!(seam_file(&flow, &task).seams[0].successor, after[0].0);
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_after_the_successor_took_its_handoff_resumes_it() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, release) = gated_flow("worker-seam-crash-after", seen.clone()).await;
    send(&flow, &task, "After.").await;
    flow.idle().await;
    // Its successor took its input, and Brigadier dies before it heard so.
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_send.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    release.notify_one();
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

/// The specs the worker's CLI sessions were started with, in order.
fn worker_specs(
    flow: &Flow,
    task: &Task,
) -> Vec<(
    brigadier_providers::ProviderKind,
    brigadier_providers::SessionSpec,
)> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.env.iter().any(|(key, value)| {
                key == brigadier_cli::protocol::SESSION_ENV && *value == task.id.0
            })
        })
        .collect()
}

#[tokio::test]
async fn a_successor_keeps_its_workers_routing_account_and_grants() {
    for (name, said) in [("count", "Grow."), ("fallback", "Fail.")] {
        let seen = Arc::new(Seen::default());
        seen.routed.store(true, std::sync::atomic::Ordering::SeqCst);
        let (flow, task) = flow_then(
            &format!("worker-seam-routing-{name}"),
            seen.clone(),
            |flow| {
                Box::pin(async move {
                    // Two servers of the user's Claude setup, one granted; two Claude accounts.
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
                    flow.add_accounts(
                        &[
                            (brigadier_providers::ProviderKind::Claude, "first"),
                            (brigadier_providers::ProviderKind::Claude, "second"),
                        ],
                        true,
                    )
                    .await;
                    // The user's account rule: Claude runs on "second".
                    let before = flow.core.settings();
                    let mut settings = before.clone();
                    for entry in &mut settings.accounts {
                        entry.default = entry.id == "second";
                    }
                    let settings = crate::accounts::edited(&before, settings);
                    flow.core.update_settings(settings).await.unwrap();
                    flow.manager.runtime.sync_accounts().await;
                })
            },
        )
        .await;
        let rounds = if said == "Grow." { 3 } else { 1 };
        for _ in 0..rounds {
            send(&flow, &task, said).await;
        }
        until(&flow, "the generation closed or carried on", || {
            let file = seam_file(&flow, &task);
            matches!(file.phase, Phase::Closed { .. }) || file.generation == 2
        })
        .await;
        flow.idle().await;
        if seam_file(&flow, &task).generation == 1 {
            send(&flow, &task, "Next.").await;
        }
        until(&flow, "the successor", || {
            seam_file(&flow, &task).generation == 2
        })
        .await;
        flow.idle().await;

        // The routing row's agent and model, the flag's effort: the same in both sessions.
        let specs = worker_specs(&flow, &task);
        assert_eq!(specs.len(), 2, "{name}");
        let (before, after) = (&specs[0], &specs[1]);
        assert_eq!(
            before.0,
            brigadier_providers::ProviderKind::Claude,
            "{name}"
        );
        assert_eq!(after.0, before.0, "{name}");
        assert_eq!(before.1.model.as_deref(), Some("sonnet"), "{name}");
        assert_eq!(after.1.model, before.1.model, "{name}");
        assert_eq!(before.1.effort.as_deref(), Some("low"), "{name}");
        assert_eq!(after.1.effort, before.1.effort, "{name}");
        assert_eq!(after.1.fast, before.1.fast, "{name}");
        // Naked both times, with only the granted server (D13, D18).
        let servers = |spec: &brigadier_providers::SessionSpec| -> Vec<String> {
            spec.mcp_servers
                .iter()
                .map(|server| server.name.clone())
                .collect()
        };
        assert!(before.1.naked_worker && after.1.naked_worker, "{name}");
        assert_eq!(servers(&before.1), vec!["pencil".to_owned()], "{name}");
        assert_eq!(servers(&after.1), servers(&before.1), "{name}");
        assert_eq!(after.1.switched_off, before.1.switched_off, "{name}");
        // On the account the user's account rules give it, as before.
        let successor = seam_file(&flow, &task).seams[0].successor.clone();
        let accounts = seen.accounts.lock().unwrap().clone();
        let first = accounts.first().unwrap();
        let next = accounts.iter().find(|(id, _)| *id == successor).unwrap();
        assert_eq!(first.1.as_deref(), Some("second"), "{name}: {accounts:?}");
        assert_eq!(next.1, first.1, "{name}: {accounts:?}");
        let task = worker(&flow).await;
        assert_eq!(task.route.choice.model.as_deref(), Some("sonnet"), "{name}");
        assert_eq!(task.route.choice.effort.as_deref(), Some("low"), "{name}");
        flow.stop().await;
    }
}

/// Goals 2 + 13 for a worker: three compactions → rebirth → three → rebirth. Each first note
/// leaves its open promise out; the check asks once, the worker names it, and each successor's
/// first message carries the note and the check that passed (core-goals D9, D121).
#[tokio::test]
async fn a_worker_goes_through_two_rebirths_its_check_fixing_its_note_each_time() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-cycle", seen.clone()).await;
    let ledger = record(&flow, &task);
    std::fs::create_dir_all(&ledger).unwrap();
    std::fs::write(
        ledger.join("ledger.md"),
        "- L1 · 11-01 21:05 · gen 1 · Open: promise to report the grow count\n\
         - L2 · 11-01 21:06 · gen 1 · Decision: grow in steps of one\n",
    )
    .unwrap();
    let mut heard = 1;
    for generation in [2, 3] {
        // Two heard as any turn; the third's end is its successor's.
        for _ in 1..crate::manager::long_sessions::HANDOFF_AFTER {
            send(&flow, &task, "Grow.").await;
            heard += 1;
            until(&flow, "the grow heard", || seen.heard("finished") == heard).await;
        }
        send(&flow, &task, "Grow.").await;
        heard += 1;
        until(&flow, "the successor", || {
            seam_file(&flow, &task).generation == generation
        })
        .await;
        until(&flow, "its turn heard", || seen.heard("finished") == heard).await;
        flow.idle().await;
    }
    let fixes = seen.given("Hand-off check: your hand-off note at");
    assert_eq!(fixes.len(), 2, "{fixes:#?}");
    for (_, fix) in &fixes {
        assert!(fix.starts_with(NUDGE_MARK));
        assert!(
            fix.contains("L1 · 11-01 21:05") && !fix.contains("L2 ·"),
            "{fix}"
        );
    }
    let starts = seen.given(HANDOFF_MARK);
    assert_eq!(starts.len(), 2, "{starts:#?}");
    for (_, start) in &starts {
        assert!(start.contains("Carrying L1."), "{start}");
        assert!(
            start.contains("Hand-off check: every open item of the ledger (1) is in the note"),
            "{start}"
        );
        assert!(start.contains(CONTEXT_POINTER));
    }
    // The fix turns are the worker's own: its session hears none of them.
    assert_eq!(seen.heard("finished"), heard);
    let view = flow
        .manager
        .worker_thread(&flow.conversation, &task.id)
        .await
        .unwrap();
    assert_eq!(view.seams.len(), 2, "{:?}", view.seams);
    assert!(
        view.seams
            .iter()
            .all(|seam| seam.nudge.is_some() && seam.at.is_some()),
        "{:?}",
        view.seams
    );
    flow.stop().await;
}

/// Development builds' on-demand compaction of an idle worker goes through its CLI's own
/// compaction: its session hears none of them, and the third asks for its note (goal 2).
// Uses the debug-only fault injections, as tab_chat_tests does.
#[cfg(debug_assertions)]
#[tokio::test]
async fn three_compactions_asked_for_on_an_idle_worker_hand_it_off_unheard() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-seam-debug-compact", seen.clone()).await;
    let key = || crate::manager::fault::FaultKey::Task(task.id.clone());
    for n in 1..=crate::manager::long_sessions::HANDOFF_AFTER {
        flow.manager.debug_compact(key()).await.unwrap();
        until(&flow, "the compaction counted", || {
            read_long(&record(&flow, &task)).compactions == n
                || matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
        })
        .await;
        flow.idle().await;
    }
    // Its successor goes on by itself, as after any hand-off, and only its end is heard.
    until(&flow, "the successor", || {
        seam_file(&flow, &task).generation == 2
    })
    .await;
    until(&flow, "the successor's turn heard", || {
        seen.heard("finished") == 2
    })
    .await;
    assert_eq!(seen.given(NUDGE_MARK).len(), 1);
    assert_eq!(
        seen.heard("finished"),
        2,
        "{:?}",
        seen.parent.lock().unwrap()
    );
    flow.stop().await;
}

/// The hold a worker's hand-off keeps messages for, here and after a restart.
fn hold_for(flow: &mut Flow, hold: std::time::Duration) {
    let config = crate::manager::seam::SeamConfig {
        hold,
        ..flow.manager.seam_config()
    };
    flow.seam = Some(config);
    flow.manager.set_seam(config);
}

/// What `record/seam-held.json` keeps.
fn side_file(flow: &Flow, task: &Task) -> Vec<String> {
    std::fs::read(record(flow, task).join("seam-held.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// A message acknowledged while the note turn ran survives a restart: its successor starts at
/// once and takes it, and it never goes again.
#[tokio::test]
async fn a_message_held_through_a_restart_starts_the_successor_with_it_once() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-restart-held", seen.clone()).await;
    send(&flow, &task, "Meanwhile.").await;
    assert!(seen.given("Meanwhile.").is_empty());
    flow.crash().await;
    until(&flow, "the successor took it", || {
        !seen.given("Meanwhile.").is_empty()
    })
    .await;
    flow.idle().await;
    assert_eq!(seam_file(&flow, &task).generation, 2);
    assert!(side_file(&flow, &task).is_empty());
    send(&flow, &task, "Next.").await;
    until(&flow, "the next", || !seen.given("Next.").is_empty()).await;
    flow.idle().await;
    let held = seen.given("Meanwhile.");
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held[0].1.starts_with(HANDOFF_MARK), "{}", held[0].1);
    flow.stop().await;
}

/// A crash while the successor starts leaves the message in both the side file and
/// `Starting.messages`: it goes to the next successor once.
#[tokio::test]
async fn a_message_in_both_the_side_file_and_starting_goes_once() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, release) = gated_flow("worker-seam-both", seen.clone()).await;
    send(&flow, &task, "Meanwhile.").await;
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    release.notify_one();
    reached.notified().await;
    let Phase::Starting { messages, .. } = seam_file(&flow, &task).phase else {
        panic!("not starting")
    };
    assert_eq!(side_file(&flow, &task), messages);
    assert!(messages[0].contains("Meanwhile."), "{messages:?}");
    flow.crash().await;
    until(&flow, "the next successor took it", || {
        !seen.given("Meanwhile.").is_empty()
    })
    .await;
    flow.idle().await;
    let held = seen.given("Meanwhile.");
    assert_eq!(held.len(), 1, "{held:?}");
    assert_eq!(held[0].1.matches("Meanwhile.").count(), 1, "{}", held[0].1);
    assert!(side_file(&flow, &task).is_empty());
    flow.stop().await;
}

/// A crash after a message was saved and before its successor began: the side file alone brings
/// it back, and the successor starts with it.
#[tokio::test]
async fn a_crash_between_saving_a_message_and_starting_delivers_it_once() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-saved", seen.clone()).await;
    std::fs::write(
        record(&flow, &task).join("seam-held.json"),
        serde_json::to_vec(&["Saved.\n\n[Worker message saved-1]"]).unwrap(),
    )
    .unwrap();
    flow.crash().await;
    until(&flow, "the successor took it", || {
        !seen.given("Saved.").is_empty()
    })
    .await;
    flow.idle().await;
    assert_eq!(seen.given("Saved.").len(), 1);
    assert!(side_file(&flow, &task).is_empty());
    flow.stop().await;
}

/// A crash after a queued message was saved and before it left the queue: it leaves the queue
/// on the way back up, and doesn't go twice.
#[tokio::test]
async fn a_row_saved_for_the_successor_leaves_the_queue_on_recovery() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-row", seen.clone()).await;
    flow.manager
        .send_to_worker(
            flow.conversation.clone(),
            task.id.clone(),
            "Use blue.".into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    let id = flow.board().await.queue.items[0].id.clone();
    std::fs::write(
        record(&flow, &task).join("seam-held.json"),
        serde_json::to_vec(&[crate::manager::workers::marked_once("Use blue.", &id)]).unwrap(),
    )
    .unwrap();
    flow.crash().await;
    until(&flow, "the successor took it", || {
        !seen.given("Use blue.").is_empty()
    })
    .await;
    flow.until("the queue to empty", |board| board.queue.items.is_empty())
        .await;
    flow.idle().await;
    assert_eq!(seen.given("Use blue.").len(), 1);
    flow.stop().await;
}

/// A usage limit in the note turn calls the hand-off off: what waited goes with the worker's
/// next input, before it.
#[tokio::test]
async fn a_usage_limit_in_the_note_turn_sends_what_waited_with_the_next_input() {
    let seen = Arc::new(Seen::default());
    let (flow, task, _, release) = gated_flow("worker-seam-limit", seen.clone()).await;
    seen.limit_note
        .store(true, std::sync::atomic::Ordering::SeqCst);
    send(&flow, &task, "Meanwhile.").await;
    release.notify_one();
    flow.until("its quota wait", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    assert!(matches!(seam_file(&flow, &task).phase, Phase::Live));
    send(&flow, &task, "Next.").await;
    until(&flow, "the next", || !seen.given("Next.").is_empty()).await;
    flow.idle().await;
    let next = seen.given("Next.");
    assert_eq!(next.len(), 1, "{next:?}");
    let (held, after) = (
        next[0].1.find("Meanwhile.").expect("what waited went too"),
        next[0].1.find("Next.").unwrap(),
    );
    assert!(held < after, "{}", next[0].1);
    assert_eq!(seen.given("Meanwhile.").len(), 1);
    assert!(side_file(&flow, &task).is_empty());
    flow.stop().await;
}

/// …and after a restart in the quota wait too, from the side file.
#[tokio::test]
async fn a_restart_after_a_usage_limit_in_the_note_turn_keeps_what_waited() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, release) = gated_flow("worker-seam-limit-crash", seen.clone()).await;
    seen.limit_note
        .store(true, std::sync::atomic::Ordering::SeqCst);
    send(&flow, &task, "Meanwhile.").await;
    release.notify_one();
    flow.until("its quota wait", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    flow.crash().await;
    flow.settled().await;
    send(&flow, &task, "Next.").await;
    until(&flow, "the next", || !seen.given("Next.").is_empty()).await;
    flow.idle().await;
    assert_eq!(seen.given("Meanwhile.").len(), 1);
    assert!(side_file(&flow, &task).is_empty());
    flow.stop().await;
}

/// A note turn that never ends: a held message waits a minute at most, and its successor starts
/// with it.
#[tokio::test]
async fn a_hung_note_turn_ends_when_the_hold_runs_out() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-hung", seen.clone()).await;
    hold_for(&mut flow, std::time::Duration::from_millis(200));
    send(&flow, &task, "Meanwhile.").await;
    until(&flow, "the successor took it", || {
        !seen.given("Meanwhile.").is_empty()
    })
    .await;
    flow.idle().await;
    assert_eq!(seam_file(&flow, &task).generation, 2);
    let held = seen.given("Meanwhile.");
    assert_eq!(held.len(), 1, "{held:?}");
    assert!(held[0].1.starts_with(HANDOFF_MARK), "{}", held[0].1);
    flow.stop().await;
}

/// A pause a hung note turn refuses starts the hold, and goes through once it ran out.
#[tokio::test]
async fn a_pause_refused_by_a_hung_note_turn_goes_through_after_the_hold() {
    let seen = Arc::new(Seen::default());
    let (mut flow, task, _, _) = gated_flow("worker-seam-hung-pause", seen.clone()).await;
    hold_for(&mut flow, std::time::Duration::from_millis(200));
    let refused = flow.manager.pause_task(task.id.clone()).await;
    assert!(refused.unwrap_err().to_string().contains("try again"));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match flow.manager.pause_task(task.id.clone()).await {
            Ok(paused) => {
                assert!(paused);
                break;
            }
            Err(err) => assert!(
                tokio::time::Instant::now() < deadline,
                "still refused: {err}"
            ),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    flow.stop().await;
}

/// A message that reaches the state lock between the check that nothing waits and the hand-off's
/// end isn't stranded: its successor starts with it. Tokio's lock is first come, first served,
/// so the order is the worst one every time.
#[tokio::test]
async fn a_message_at_the_moment_the_generation_closes_starts_the_successor() {
    let seen = Arc::new(Seen::default());
    let (flow, task, _, release) = gated_flow("worker-seam-close-race", seen.clone()).await;
    let live = flow.manager.task_live(&task);
    let rerouting = live.reroute.lock().await;
    release.notify_one();
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    super::eventually_async("its CLI closed", || async { live.cli().await.is_none() }).await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let messaged = worker(&flow).await;
    let state = live.state_guard().await;
    let (manager, conversation) = (flow.manager.clone(), flow.conversation.clone());
    let sent = tokio::spawn(async move {
        manager
            .send_worker_text(&conversation, &messaged, "Late.".into())
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(rerouting);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(state);
    sent.await.unwrap().unwrap();
    until(&flow, "the successor took it", || {
        !seen.given("Late.").is_empty()
    })
    .await;
    flow.idle().await;
    assert_eq!(seen.given("Late.").len(), 1);
    flow.stop().await;
}

/// A send on its way leaves the worker's state lock free: its event pump goes on meanwhile, so a
/// full event channel can't stall the reply the send waits for (#768's timeout, a duplicate).
#[tokio::test]
async fn a_send_on_its_way_leaves_the_state_lock_free() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-lock-send", seen.clone()).await;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow.behavior.hold_send_entry.lock().unwrap() = Some((reached.clone(), release.clone()));
    let (manager, conversation, messaged) = (
        flow.manager.clone(),
        flow.conversation.clone(),
        task.clone(),
    );
    let sent = tokio::spawn(async move {
        manager
            .send_worker_text(&conversation, &messaged, "Later.".into())
            .await
    });
    reached.notified().await;
    let live = flow.manager.task_live(&task);
    let busy = tokio::time::timeout(std::time::Duration::from_secs(2), live.busy()).await;
    release.notify_one();
    assert_eq!(busy.ok(), Some(true), "the send holds the state lock");
    sent.await.unwrap().unwrap();
    until(&flow, "the turn", || !seen.given("Later.").is_empty()).await;
    flow.idle().await;
    flow.stop().await;
}

/// A refused send leaves the worker idle, as it always did.
#[tokio::test]
async fn a_refused_send_leaves_the_worker_idle() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("worker-lock-refused", seen.clone()).await;
    flow.behavior
        .refuse_sends
        .lock()
        .unwrap()
        .push(task.route.choice.provider);
    let latest = worker(&flow).await;
    let refused = flow
        .manager
        .send_worker_text(&flow.conversation, &latest, "Later.".into())
        .await;
    assert!(refused.is_err());
    assert!(!flow.manager.task_live(&task).busy().await);
    flow.behavior.refuse_sends.lock().unwrap().clear();
    flow.stop().await;
}

/// A successor that couldn't start tells its message's sender so: that message isn't kept as
/// well, or the sender's next try would send it twice.
#[tokio::test]
async fn a_message_whose_successor_could_not_start_is_not_kept_as_well() {
    let seen = Arc::new(Seen::default());
    let (flow, task, _, release) = gated_flow("worker-seam-refused", seen.clone()).await;
    let provider = task.route.choice.provider;
    flow.behavior.refuse_sends.lock().unwrap().push(provider);
    release.notify_one();
    until(&flow, "the generation closed", || {
        matches!(seam_file(&flow, &task).phase, Phase::Closed { .. })
    })
    .await;
    flow.idle().await;
    let latest = worker(&flow).await;
    let refused = flow
        .manager
        .send_worker_text(&flow.conversation, &latest, "Refused.".into())
        .await;
    assert!(refused.is_err());
    flow.behavior.refuse_sends.lock().unwrap().clear();
    send(&flow, &task, "Next.").await;
    until(&flow, "the next", || !seen.given("Next.").is_empty()).await;
    flow.idle().await;
    assert!(
        seen.given("Refused.").is_empty(),
        "{:?}",
        seen.given("Refused.")
    );
    flow.stop().await;
}

/// #842 lets its turn run during the send: one that ended before the send returned stays
/// reported, or its idle worker reads running and never hibernates.
#[tokio::test]
async fn a_turn_that_ended_during_its_send_stays_reported() {
    let seen = Arc::new(Seen::default());
    let (flow, task) = flow("fast-send", seen.clone()).await;
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_send_return.lock().unwrap() = Some((reached.clone(), release.clone()));
    let manager = flow.manager.clone();
    let (id, sending) = (flow.conversation.clone(), task.clone());
    let sent = tokio::spawn(async move {
        manager
            .send_worker_text(&id, &sending, "Fast.".into())
            .await
    });
    reached.notified().await;
    until(&flow, "the fast turn reported", || {
        seen.heard("finished") == 2
    })
    .await;
    assert_eq!(worker(&flow).await.state, crate::work::TaskState::Reported);
    release.notify_one();
    sent.await.unwrap().unwrap();
    assert_eq!(worker(&flow).await.state, crate::work::TaskState::Reported);
    flow.stop().await;
}
