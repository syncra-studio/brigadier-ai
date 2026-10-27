//! The seam (S22): past a debug threshold of 60k the thread is asked for its hand-off note, its
//! CLI closes behind a divider, and a fresh session starts at once from the hand-off, with the
//! messages that waited or told to carry on. Each message reaches the thread once, across a
//! note turn that runs long, a restart or a crash at every step, and a generation that dies
//! without its note still goes on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::session_file::Said;
use brigadier_providers::{ErrorKind, ProviderError, ProviderEvent};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::seam::{CARRY_ON, Phase, SeamConfig, SeamFile, read_seam, without_handoff};
use crate::thread_view::{HANDOFF_MARK, NUDGE_MARK};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What each turn of the thread was given, with its CLI session.
type Inputs = Arc<Mutex<Vec<(String, String)>>>;

/// Turns the seam on at 60k, for this manager and every one the flow restarts.
fn seam_on(flow: &mut Flow, hold: Duration) {
    let config = SeamConfig {
        tokens: 60_000,
        hold,
        long: None,
        tokens_set: false,
    };
    flow.seam = Some(config);
    flow.manager.set_seam(config);
}

fn options() -> Options {
    Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            take_in_steers: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
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

/// How the thread answers: "Grow." passes the threshold mid-turn and goes on once the nudge
/// came; the nudge writes the note (unless `noteless`); anything else is answered at length,
/// so the hand-off leaves spans out.
fn thread(inputs: Inputs, noteless: Arc<std::sync::atomic::AtomicBool>) -> Script {
    script(move |turn| {
        let inputs = inputs.clone();
        let noteless = noteless.clone();
        async move {
            inputs
                .lock()
                .unwrap()
                .push((turn.native_id.clone(), turn.input.clone()));
            if turn.input.starts_with(NUDGE_MARK) {
                if !noteless.load(std::sync::atomic::Ordering::SeqCst) {
                    let note = note_of(&turn.input);
                    std::fs::write(&note, "Doing now: nothing.\nNext: wait for the user.").unwrap();
                }
                return Reply::text("Handed off.");
            }
            // A successor's hand-off quotes earlier turns: only the user's own words count.
            if without_handoff(&turn.input).contains("Grow.") {
                turn.report_context(70_000).await;
                turn.until_steered().await;
                return Reply::text("Grown.");
            }
            Reply {
                text: format!("Answer: {}", "the output of a long read. ".repeat(200)),
                context_tokens: Some(20_000),
                ..Reply::default()
            }
        }
    })
}

fn seam_file(flow: &Flow) -> SeamFile {
    read_seam(&flow.manager.session_dir(&flow.conversation).record())
}

async fn until_seam(flow: &Flow, what: &str, done: impl Fn(&SeamFile) -> bool) -> SeamFile {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let file = seam_file(flow);
            if done(&file) {
                break file;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}: {:?}", seam_file(flow)))
}

fn closed(file: &SeamFile) -> bool {
    matches!(file.phase, Phase::Closed { .. })
}

/// The inputs whose text holds `needle`.
fn given(inputs: &Inputs, needle: &str) -> Vec<(String, String)> {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, input)| input.contains(needle))
        .cloned()
        .collect()
}

/// Grows the thread past seam `n`'s threshold and waits until its successor carried on and
/// the thread is idle.
async fn grow(flow: &Flow, n: u32) -> SeamFile {
    flow.say("Grow.").await;
    let file = until_seam(flow, "the successor", |file| file.generation == n + 1).await;
    flow.idle().await;
    file
}

/// Waits until the note turn is under way: the seam is nudged.
async fn until_nudged(flow: &Flow) {
    until_seam(flow, "the nudge", |file| {
        matches!(file.phase, Phase::Nudged { .. })
    })
    .await;
}

/// A note that never comes: the thread's note turn waits until `release`.
fn gate_note(
    base: Script,
    reached: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
) -> Script {
    script(move |turn| {
        let (base, reached, release) = (base.clone(), reached.clone(), release.clone());
        async move {
            if turn.input.starts_with(NUDGE_MARK) {
                reached.notify_one();
                release.notified().await;
            }
            base(turn).await
        }
    })
}

#[tokio::test]
async fn two_seams_each_with_a_divider_its_note_and_a_successor() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "seam-two",
        options(),
        thread(inputs.clone(), Arc::default()),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Plant: keep the code K-001 as it is.").await;
    flow.settled().await;
    for n in 0..30 {
        flow.say(&format!("Filler {n}.")).await;
        flow.settled().await;
    }
    let first = grow(&flow, 1).await;
    assert_eq!(first.seams.len(), 1);
    assert!(first.seams[0].note.is_some() && first.seams[0].end.death.is_none());
    assert!(first.seams[0].messages.is_empty());

    // The successor started at once from its hand-off, told to carry on.
    let carried = given(&inputs, CARRY_ON);
    assert_eq!(carried.len(), 1, "{carried:?}");
    let (successor, input) = &carried[0];
    assert_eq!(*successor, first.seams[0].successor);
    assert_ne!(Some(successor), first.seams[0].end.predecessor.as_ref());
    assert!(input.starts_with(HANDOFF_MARK), "{input}");
    assert!(input.contains("## Hand-off note\nDoing now: nothing."));
    // The rest is pulled, never put in (D13): the user's earlier words are a `brigadier
    // context` away.
    assert!(input.contains(crate::manager::seam::CONTEXT_POINTER));
    assert!(!input.contains("Plant: keep the code K-001 as it is."));
    assert!(input.trim_end().ends_with(CARRY_ON));
    let start = std::fs::read_to_string(&first.seams[0].start).unwrap();
    assert!(input.starts_with(start.as_str()));
    // The nudge, the hand-off and the word to carry on are Brigadier's, never a bubble (L12).
    let nudges = given(&inputs, NUDGE_MARK);
    assert!(!nudges.is_empty());
    for (_, nudge) in nudges.iter().chain([&carried[0]]) {
        assert_eq!(flow.noted_said(nudge), Some(Said::Brigadier), "{nudge}");
    }

    // The user's next message goes to that successor, resumed.
    flow.say("After one.").await;
    flow.idle().await;
    let after = given(&inputs, "After one.");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_eq!(after[0].0, first.seams[0].successor);
    assert!(!after[0].1.contains(HANDOFF_MARK));

    // A restart keeps the seam; a second one comes the same way.
    flow.restart().await;
    let second = grow(&flow, 2).await;
    assert_eq!(second.seams.len(), 2);
    let carried = given(&inputs, CARRY_ON);
    assert_eq!(carried.len(), 2);
    assert_eq!(carried[1].0, second.seams[1].successor);
    assert!(carried[1].1.starts_with(&format!("{HANDOFF_MARK}2 ·")));

    // Two dividers, after a restart too, each with its note; the successor's first message
    // reads as its own words, not the hand-off.
    flow.restart().await;
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(view.seams.len(), 2, "{:?}", view.seams);
    for mark in &view.seams {
        assert!(mark.note.is_some() && mark.start.is_some() && mark.death.is_none());
        assert!(mark.nudge.is_some() && mark.at.is_some() && mark.carried);
    }
    let texts: Vec<&str> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.event {
            ProviderEvent::Message { item_id, text, .. }
                if view
                    .seams
                    .iter()
                    .any(|mark| mark.at.as_ref() == Some(item_id)) =>
            {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(texts, [CARRY_ON, CARRY_ON]);
    let said: Vec<_> = view
        .entries
        .iter()
        .filter(|entry| matches!(&entry.event, ProviderEvent::Message { item_id, .. } if view.seams.iter().any(|mark| mark.at.as_ref() == Some(item_id))))
        .map(|entry| entry.said.clone())
        .collect();
    assert_eq!(said, [Some(Said::Brigadier), Some(Said::Brigadier)]);
    flow.stop().await;
}

#[tokio::test]
async fn a_message_during_the_note_turn_reaches_the_successor_once() {
    let inputs: Inputs = Arc::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let reached = Arc::new(tokio::sync::Notify::new());
    let base = thread(inputs.clone(), Arc::default());
    let mut flow = Flow::start(
        "seam-once",
        options(),
        gate_note(base, reached.clone(), release.clone()),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Grow.").await;
    reached.notified().await;
    flow.say("Meanwhile.").await;
    // It waits in the queue, not in the note turn.
    assert!(given(&inputs, "Meanwhile.").is_empty());
    assert_eq!(flow.board().await.queue.items.len(), 1);
    release.notify_one();
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    // It starts the successor, with its hand-off; nothing else is said to carry on.
    let meanwhile = given(&inputs, "Meanwhile.");
    assert_eq!(meanwhile.len(), 1, "{meanwhile:?}");
    assert_eq!(meanwhile[0].0, file.seams[0].successor);
    assert!(meanwhile[0].1.starts_with(HANDOFF_MARK));
    // Only the user's words show of it: the hand-off before them is Brigadier's (L12).
    assert_eq!(
        flow.noted_said(&meanwhile[0].1),
        Some(Said::Both {
            user: "Meanwhile.".into()
        })
    );
    assert_eq!(
        flow.said_in_thread("Meanwhile."),
        [Some(Said::Both {
            user: "Meanwhile.".into()
        })]
    );
    assert!(given(&inputs, CARRY_ON).is_empty());
    assert!(file.seams[0].note.is_some());
    assert_eq!(file.seams[0].messages.len(), 1);
    flow.stop().await;
}

#[tokio::test]
async fn a_note_turn_still_working_a_minute_after_a_message_is_stopped() {
    let inputs: Inputs = Arc::default();
    let base = thread(inputs.clone(), Arc::default());
    let mut flow = Flow::start(
        "seam-hold",
        options(),
        script(move |turn| {
            let base = base.clone();
            async move {
                if turn.input.starts_with(NUDGE_MARK) {
                    // It never finishes its note: only a stop ends the turn.
                    turn.stopped().await;
                    return Reply::text("");
                }
                base(turn).await
            }
        }),
    )
    .await;
    seam_on(&mut flow, Duration::from_millis(300));
    flow.say("Grow.").await;
    until_nudged(&flow).await;
    flow.say("Meanwhile.").await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    assert_eq!(
        file.seams[0].end.death.as_deref(),
        Some("it was still working a minute after a message came")
    );
    assert_ne!(
        file.seams[0].end.predecessor.as_deref(),
        Some(file.seams[0].successor.as_str()),
        "the hand-off reused its predecessor: {file:?}"
    );
    let meanwhile = given(&inputs, "Meanwhile.");
    assert_eq!(meanwhile.len(), 1);
    assert!(
        meanwhile[0]
            .1
            .contains("check `git status` and `git log` first")
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_message_queued_just_before_the_nudge_waits_a_minute_at_most() {
    let inputs: Inputs = Arc::default();
    let base = thread(inputs.clone(), Arc::default());
    let reached = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (at_grow, go) = (reached.clone(), release.clone());
    let mut flow = Flow::start(
        "seam-queued",
        options(),
        script(move |turn| {
            let (base, at_grow, go) = (base.clone(), at_grow.clone(), go.clone());
            async move {
                if turn.input.starts_with(NUDGE_MARK) {
                    turn.stopped().await;
                    return Reply::text("");
                }
                if turn.input == "Grow." {
                    at_grow.notify_one();
                    go.notified().await;
                }
                base(turn).await
            }
        }),
    )
    .await;
    seam_on(&mut flow, Duration::from_millis(300));
    flow.say("Grow.").await;
    reached.notified().await;
    // Queued while the turn runs, before the seam.
    let outcome = flow
        .manager
        .send_message(
            flow.conversation.clone(),
            "Meanwhile.".into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    let super::super::conversation::SendOutcome::Queued(item) = outcome else {
        panic!("not queued")
    };
    assert!(item.delivered, "not steered: {item:?}");
    release.notify_one();
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    assert_eq!(
        file.seams[0].end.death.as_deref(),
        Some("it was still working a minute after a message came")
    );
    // It reached the working thread at once, Deciding (run2-D40), and joined that work; the
    // successor reads it with `brigadier context` (D13), and nothing sends it again.
    let meanwhile = given(&inputs, "Meanwhile.");
    assert_eq!(meanwhile.len(), 1, "{meanwhile:?}");
    assert_ne!(meanwhile[0].0, file.seams[0].successor, "{meanwhile:#?}",);
    assert!(
        meanwhile[0].1.starts_with("[queued message "),
        "{meanwhile:?}"
    );
    assert!(!given(&inputs, CARRY_ON).is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_stop_in_the_note_turn_leaves_the_successor_to_the_users_next_message() {
    let inputs: Inputs = Arc::default();
    let base = thread(inputs.clone(), Arc::default());
    let reached = Arc::new(tokio::sync::Notify::new());
    let at_note = reached.clone();
    let mut flow = Flow::start(
        "seam-stop",
        options(),
        script(move |turn| {
            let (base, at_note) = (base.clone(), at_note.clone());
            async move {
                if turn.input.starts_with(NUDGE_MARK) {
                    at_note.notify_one();
                    turn.stopped().await;
                    return Reply::text("");
                }
                base(turn).await
            }
        }),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Grow.").await;
    reached.notified().await;
    flow.manager
        .interrupt(flow.conversation.clone())
        .await
        .unwrap();
    let file = until_seam(&flow, "the close", closed).await;
    flow.idle().await;
    let Phase::Closed { end } = &file.phase else {
        unreachable!()
    };
    assert!(end.stopped);
    assert_eq!(end.death.as_deref(), Some("the user stopped it"));
    // Nothing carries on by itself, after a restart either.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(given(&inputs, HANDOFF_MARK).is_empty());
    flow.restart().await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(given(&inputs, HANDOFF_MARK).is_empty());
    assert!(closed(&seam_file(&flow)));
    // The user's next message starts the successor, once.
    flow.say("Go on.").await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    let go = given(&inputs, "Go on.");
    assert_eq!(go.len(), 1, "{go:?}");
    assert_eq!(go[0].0, file.seams[0].successor);
    assert!(go[0].1.starts_with(HANDOFF_MARK));
    assert!(given(&inputs, CARRY_ON).is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_context_too_long_ends_the_generation_without_a_note() {
    let inputs: Inputs = Arc::default();
    let base = thread(inputs.clone(), Arc::default());
    let mut flow = Flow::start(
        "seam-too-long",
        options(),
        script(move |turn| {
            let base = base.clone();
            async move {
                if without_handoff(&turn.input).contains("Overflow.") {
                    turn.emit(ProviderEvent::Error {
                        error: ProviderError {
                            kind: ErrorKind::ContextWindow,
                            message: "Prompt is too long".into(),
                            will_retry: false,
                            limit: None,
                            code: None,
                        },
                    })
                    .await;
                    return Reply::failed("Prompt is too long");
                }
                base(turn).await
            }
        }),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("First.").await;
    flow.settled().await;
    flow.say("Overflow.").await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    assert_eq!(
        file.seams[0].end.death.as_deref(),
        Some("its context was too long for the model")
    );
    let carried = given(&inputs, CARRY_ON);
    assert_eq!(carried.len(), 1);
    assert!(carried[0].1.contains(
        "The previous session ended without a hand-off note (its context was too long for \
         the model): check `git status` and `git log` first."
    ));
    assert!(!carried[0].1.contains("First."));
    assert!(carried[0].1.contains(crate::manager::seam::CONTEXT_POINTER));
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(
        view.seams[0].death.as_deref(),
        Some("its context was too long for the model")
    );
    // The request the generation died in isn't failed: the successor finished it.
    let board = flow.board().await;
    let states: Vec<_> = board.requests.values().map(|r| r.state.clone()).collect();
    assert!(
        states
            .iter()
            .all(|state| *state == crate::work::RequestState::Done),
        "{states:?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_nudge_that_cannot_be_steered_is_a_turn_of_its_own() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "seam-own-turn",
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                refuse_steers: true,
                ..FakeBehavior::default()
            }),
            ..Options::default()
        },
        script({
            let inputs = inputs.clone();
            move |turn| {
                let inputs = inputs.clone();
                async move {
                    inputs
                        .lock()
                        .unwrap()
                        .push((turn.native_id.clone(), turn.input.clone()));
                    if turn.input.starts_with(NUDGE_MARK) {
                        std::fs::write(note_of(&turn.input), "Next: wait.").unwrap();
                        return Reply::text("Handed off.");
                    }
                    if turn.input.starts_with(HANDOFF_MARK) {
                        return Reply::text("Carrying on.");
                    }
                    Reply {
                        text: "Big.".into(),
                        context_tokens: Some(70_000),
                        ..Reply::default()
                    }
                }
            }
        }),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Big.").await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    // One nudge, its own turn, in the predecessor.
    let nudges = given(&inputs, NUDGE_MARK);
    assert_eq!(nudges.len(), 1, "{nudges:?}");
    assert_eq!(file.seams[0].end.predecessor.as_ref(), Some(&nudges[0].0));
    assert!(file.seams[0].end.death.is_none());
    flow.stop().await;
}

#[tokio::test]
async fn a_restart_between_note_and_successor_starts_one_successor() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "seam-restart",
        options(),
        thread(inputs.clone(), Arc::default()),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("First.").await;
    flow.settled().await;
    // The successor's CLI never comes up before Brigadier stops.
    let reached = Arc::new(tokio::sync::Notify::new());
    flow.say("Grow.").await;
    until_nudged(&flow).await;
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    reached.notified().await;
    let Phase::Starting { successor, .. } = seam_file(&flow).phase else {
        panic!("not starting")
    };
    flow.crash().await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    // One successor, under another id: the one reserved before was never started.
    let carried = given(&inputs, CARRY_ON);
    assert_eq!(carried.len(), 1, "{carried:?}");
    assert_ne!(file.seams[0].successor, successor);
    assert_eq!(carried[0].0, file.seams[0].successor);
    assert!(carried[0].1.contains("## Hand-off note"));
    // The next message goes to the same successor, resumed.
    flow.say("Next.").await;
    flow.idle().await;
    let next = given(&inputs, "Next.");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].0, file.seams[0].successor);
    assert_eq!(seam_file(&flow).generation, 2);
    flow.stop().await;
}

#[tokio::test]
async fn a_restart_in_the_note_turn_ends_the_generation_with_the_note_it_wrote() {
    for written in [true, false] {
        let inputs: Inputs = Arc::default();
        let reached = Arc::new(tokio::sync::Notify::new());
        let base = thread(inputs.clone(), Arc::default());
        let at = reached.clone();
        let mut flow = Flow::start(
            "seam-restart-note",
            options(),
            script(move |turn| {
                let (base, at) = (base.clone(), at.clone());
                async move {
                    if turn.input.starts_with(NUDGE_MARK) {
                        if written {
                            std::fs::write(note_of(&turn.input), "Next: wait.").unwrap();
                        }
                        at.notify_one();
                        // Brigadier quits before the turn ends.
                        std::future::pending::<()>().await;
                    }
                    base(turn).await
                }
            }),
        )
        .await;
        seam_on(&mut flow, Duration::from_secs(60));
        flow.say("Grow.").await;
        reached.notified().await;
        flow.crash().await;
        // The generation ends, and its successor carries on at once.
        let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
        flow.idle().await;
        assert_eq!(
            file.seams[0].end.death.as_deref(),
            (!written).then_some("Brigadier restarted before it wrote one")
        );
        let carried = given(&inputs, CARRY_ON);
        assert_eq!(carried.len(), 1);
        assert_eq!(
            carried[0].1.contains("## Hand-off note\nNext: wait."),
            written
        );
        flow.stop().await;
    }
}

#[tokio::test]
async fn a_crash_before_the_successor_took_its_handoff_sends_the_message_once() {
    let inputs: Inputs = Arc::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let note = Arc::new(tokio::sync::Notify::new());
    let base = thread(inputs.clone(), Arc::default());
    let mut flow = Flow::start(
        "seam-crash-before",
        options(),
        gate_note(base, note.clone(), release.clone()),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Grow.").await;
    note.notified().await;
    // A message waits for the successor, whose CLI never comes up before the crash.
    flow.say("After.").await;
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_start.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    release.notify_one();
    reached.notified().await;
    let starting = seam_file(&flow);
    let Phase::Starting {
        successor,
        messages,
        ..
    } = &starting.phase
    else {
        panic!("{starting:?}")
    };
    assert_eq!(messages.len(), 1);
    let reserved = successor.clone();
    flow.crash().await;
    let file = until_seam(&flow, "the successor", |file| file.generation == 2).await;
    flow.idle().await;
    let after = given(&inputs, "After.");
    assert_eq!(after.len(), 1, "{after:?}");
    assert!(after[0].1.starts_with(HANDOFF_MARK));
    // Another id: the one reserved before the crash was never started.
    assert_ne!(file.seams[0].successor, reserved);
    assert_eq!(after[0].0, file.seams[0].successor);
    assert_eq!(file.seams[0].messages.len(), 1);
    // It carried the message: nothing else told it to carry on.
    assert!(given(&inputs, CARRY_ON).is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn a_crash_after_the_successor_took_its_handoff_sends_nothing_again() {
    let inputs: Inputs = Arc::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let note = Arc::new(tokio::sync::Notify::new());
    let base = thread(inputs.clone(), Arc::default());
    let mut flow = Flow::start(
        "seam-crash-after",
        options(),
        gate_note(base, note.clone(), release.clone()),
    )
    .await;
    seam_on(&mut flow, Duration::from_secs(60));
    flow.say("Grow.").await;
    note.notified().await;
    flow.say("After.").await;
    // The successor took its input, and Brigadier dies before it heard so.
    let reached = Arc::new(tokio::sync::Notify::new());
    *flow.behavior.hold_send.lock().unwrap() =
        Some((reached.clone(), Arc::new(tokio::sync::Notify::new())));
    release.notify_one();
    reached.notified().await;
    let Phase::Starting { successor, .. } = seam_file(&flow).phase else {
        panic!("not starting")
    };
    flow.crash().await;
    let file = until_seam(&flow, "the seam to complete", |file| file.generation == 2).await;
    assert_eq!(file.seams[0].successor, successor);
    // Its turn the crash cut off goes on by itself in that successor, once, with a word to
    // check (D9 a): the message it took never goes again, and nothing carried on.
    flow.until("the cut-off turn gone on", |board| {
        board
            .latest_request()
            .is_some_and(|request| request.state == crate::work::RequestState::Done)
    })
    .await;
    flow.idle().await;
    let on = given(&inputs, "your turn goes on by itself");
    assert_eq!(on.len(), 1, "{on:?}");
    assert_eq!(on[0].0, successor);
    let after = given(&inputs, "After.");
    assert!(after.is_empty(), "{after:?}");
    assert!(given(&inputs, CARRY_ON).is_empty());
    // The next message resumes that successor, never another.
    flow.say("Next.").await;
    flow.idle().await;
    let next = given(&inputs, "Next.");
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].0, successor);
    assert!(!next[0].1.contains(HANDOFF_MARK));
    assert_eq!(seam_file(&flow).seams.len(), 1);
    flow.stop().await;
}

/// A second close joins the predecessor's shutdown: it cannot release the next turn before
/// the predecessor's exit is pumped, whether the first close was explicit or an idle retirement.
#[tokio::test]
async fn overlapping_closes_keep_the_next_turn_behind_the_predecessors_exit() {
    async fn poll_once(future: std::pin::Pin<&mut impl std::future::Future>) -> bool {
        let mut future = future;
        std::future::poll_fn(|cx| std::task::Poll::Ready(future.as_mut().poll(cx).is_pending()))
            .await
    }

    for retire in [false, true] {
        let inputs: Inputs = Arc::default();
        let flow = Flow::start(
            "seam-close-overlap",
            options(),
            thread(inputs.clone(), Arc::default()),
        )
        .await;
        flow.say("First.").await;
        flow.settled().await;
        let conv = flow.manager.conv(&flow.conversation).unwrap();
        let cli = conv.cli().await.unwrap();
        let native_id = cli.session.native_id();
        let reached = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        *flow.behavior.hold_close.lock().unwrap() = Some((reached.clone(), release.clone()));
        let mut first = Box::pin(async {
            if retire {
                conv.retire_cli_for(&cli, None).await;
            } else {
                conv.close_cli().await;
            }
        });
        assert!(poll_once(first.as_mut()).await);
        reached.notified().await;
        let mut second = Box::pin(conv.close_cli());
        assert!(
            poll_once(second.as_mut()).await,
            "overlapping close returned before the predecessor exited (retire={retire})"
        );
        // A next-turn driver can already be running while the predecessor is closing.
        flow.say("Next.").await;
        flow.manager.next_turn_now(&conv).await;
        assert!(given(&inputs, "Next.").is_empty());
        release.notify_one();
        first.await;
        // The waiter resumes only after a replacement CLI starts. It joined the old
        // shutdown, so it must leave this new generation and its running turn alone.
        flow.manager.next_turn_now(&conv).await;
        let resumed = conv.cli().await.unwrap();
        assert!(!Arc::ptr_eq(&cli, &resumed));
        second.await;
        assert!(
            conv.cli()
                .await
                .is_some_and(|current| Arc::ptr_eq(&current, &resumed))
        );
        assert!(resumed.session.is_running());
        flow.settled().await;
        let next = given(&inputs, "Next.");
        assert_eq!(next.len(), 1, "{next:?}");
        assert_eq!(next[0].0, native_id);
        flow.stop().await;
    }
}
