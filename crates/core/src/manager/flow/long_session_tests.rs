//! Long sessions behind `BRIGADIER_LONG_SESSIONS` (run2-D2, run2-D15): with the switch on, a
//! session's thread compacts in place with its CLI's own compaction instead of going on in a
//! fresh session at a size; off, the seam comes as before.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ProviderEvent, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::long::{PutBackState, read_long};
use crate::manager::long_sessions::LongSessions;
use crate::manager::seam::{Phase, SeamConfig, read_seam, without_handoff};
use crate::model::DomainEvent;
use crate::thread_view::NUDGE_MARK;
use crate::work::OrchestratorEntry;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Inputs = Arc<Mutex<Vec<(ProviderKind, String)>>>;

/// The seam's point at 60k, set in the environment, and the switch at `long`.
fn switch(flow: &mut Flow, long: Option<LongSessions>) {
    let config = SeamConfig {
        tokens: 60_000,
        hold: Duration::from_secs(60),
        long,
        tokens_set: true,
    };
    flow.seam = Some(config);
    flow.manager.set_seam(config);
}

fn options(thread: ProviderKind) -> Options {
    Options {
        thread,
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

/// "Grow." passes the point mid-turn and the CLI compacts on its own, as Codex does at its
/// point; a nudge writes its hand-off note; anything else is a short answer.
fn thread(inputs: Inputs) -> Script {
    script(move |turn| {
        let inputs = inputs.clone();
        async move {
            inputs
                .lock()
                .unwrap()
                .push((turn.provider, turn.input.clone()));
            if turn.input.starts_with(NUDGE_MARK) {
                std::fs::write(note_of(&turn.input), "Doing now: nothing.").unwrap();
                return Reply::text("Handed off.");
            }
            if without_handoff(&turn.input).contains("Grow.") {
                turn.report_context(70_000).await;
                turn.emit(ProviderEvent::CompactionStarted { automatic: true })
                    .await;
                turn.emit(ProviderEvent::CompactionEnded {
                    automatic: true,
                    tokens_before: Some(70_000),
                    tokens_after: Some(12_000),
                    error: None,
                    native_id: Some("compaction-1".into()),
                })
                .await;
                turn.report_context(12_000).await;
                return Reply::text("Grown.");
            }
            Reply {
                text: "Done.".into(),
                context_tokens: Some(20_000),
                ..Reply::default()
            }
        }
    })
}

async fn breaches(flow: &Flow) -> usize {
    flow.core
        .list_orchestrator_log(&flow.conversation, None, 500)
        .await
        .unwrap()
        .entries
        .into_iter()
        .filter(|logged| matches!(logged.entry, OrchestratorEntry::ContractBreach { .. }))
        .count()
}

#[tokio::test]
async fn a_codex_session_compacts_in_place_with_the_switch_on() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "long-codex",
        options(ProviderKind::Codex),
        thread(inputs.clone()),
    )
    .await;
    switch(&mut flow, LongSessions::parse("native-bare"));
    flow.say("Start.").await;
    flow.settled().await;
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After.").await;
    flow.settled().await;
    // No hand-off was asked for and no successor started: the same CLI session goes on.
    let seen = inputs.lock().unwrap().clone();
    assert!(
        seen.iter().all(|(_, input)| !input.starts_with(NUDGE_MARK)),
        "{seen:?}"
    );
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert_eq!((file.generation, file.phase), (1, Phase::Live));
    assert!(file.seams.is_empty());
    assert_eq!(breaches(&flow).await, 0);
    // Codex compacts where it would by itself, or at the point a comparison set (run2-D6).
    let specs = flow.specs();
    let (_, spec) = specs
        .iter()
        .rev()
        .find(|(kind, _)| *kind == ProviderKind::Codex)
        .unwrap();
    let native = spec.native_compaction.clone().expect("compacts natively");
    assert_eq!(native.codex_limit, Some(60_000));
    assert!(native.restore.is_none());
    flow.stop().await;
}

#[tokio::test]
async fn with_the_switch_off_a_codex_session_still_goes_on_at_its_seam() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "long-codex-off",
        options(ProviderKind::Codex),
        thread(inputs.clone()),
    )
    .await;
    switch(&mut flow, None);
    flow.say("Start.").await;
    flow.settled().await;
    let specs = flow.specs();
    assert!(
        specs
            .iter()
            .all(|(_, spec)| spec.native_compaction.is_none())
    );
    flow.say("Grow.").await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while read_seam(&flow.manager.session_dir(&flow.conversation).record()).generation < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a successor at the seam");
    flow.settled().await;
    // Its CLI compacting on its own is still a breach of the seam's contract.
    assert!(breaches(&flow).await >= 1);
    flow.stop().await;
}

/// Claude's thread: "Grow." passes the point and the turn ends there, as a turn does past
/// 200k; a nudge writes its hand-off note; anything else is a short answer.
fn claude_thread(inputs: Inputs) -> Script {
    script(move |turn| {
        let inputs = inputs.clone();
        async move {
            inputs
                .lock()
                .unwrap()
                .push((turn.provider, turn.input.clone()));
            if turn.input.starts_with(NUDGE_MARK) {
                std::fs::write(note_of(&turn.input), "Doing now: nothing.").unwrap();
                return Reply::text("Handed off.");
            }
            if without_handoff(&turn.input).contains("Grow.") {
                turn.report_context(70_000).await;
                return Reply::text("Grown.");
            }
            Reply {
                text: "Done.".into(),
                context_tokens: Some(30_000),
                ..Reply::default()
            }
        }
    })
}

/// Runs a Claude session in `arm` through a turn past the point and one after it: what each
/// compaction was told to keep.
async fn claude_compactions(arm: &str) -> Vec<Option<String>> {
    let inputs: Inputs = Arc::default();
    let options = options(ProviderKind::Claude);
    let behavior = options.behavior.clone();
    let mut flow = Flow::start("long-claude", options, claude_thread(inputs.clone())).await;
    switch(&mut flow, LongSessions::parse(arm));
    flow.say("Start.").await;
    flow.settled().await;
    assert!(behavior.compactions.lock().unwrap().is_empty());
    flow.say("Grow.").await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while behavior.compactions.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a compaction at the point");
    flow.idle().await;
    flow.say("After.").await;
    flow.idle().await;
    // The same CLI session goes on: no hand-off, no successor, no breach.
    let seen = inputs.lock().unwrap().clone();
    assert!(
        seen.iter().all(|(_, input)| !input.starts_with(NUDGE_MARK)),
        "{seen:?}"
    );
    assert!(
        seen.iter().any(|(_, input)| input.contains("After.")),
        "{seen:?}"
    );
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert_eq!((file.generation, file.phase), (1, Phase::Live));
    assert_eq!(breaches(&flow).await, 0);
    // Claude may compact by itself as a backstop just above the point (run2-D5).
    let specs = flow.specs();
    let (_, spec) = specs
        .iter()
        .rev()
        .find(|(kind, _)| *kind == ProviderKind::Claude)
        .unwrap();
    let native = spec.native_compaction.clone().expect("compacts natively");
    assert_eq!(native.backstop_window, Some(110_000));
    flow.stop().await;
    behavior.compactions.lock().unwrap().clone()
}

#[tokio::test]
async fn a_claude_session_compacts_once_at_its_point_with_what_its_summary_must_keep() {
    let compactions = claude_compactions("native-full").await;
    assert_eq!(
        compactions,
        vec![Some(
            crate::manager::long_sessions::COMPACT_INSTRUCTIONS.to_owned()
        )]
    );
    let instructions = compactions[0].clone().unwrap();
    for kept in [
        "open items",
        "decision",
        "failed",
        "files you changed",
        "last test",
        "next step",
        "skills",
        "brigadier brain read",
        "Never copy a secret",
    ] {
        assert!(instructions.contains(kept), "{kept}");
    }
}

#[tokio::test]
async fn a_bare_claude_session_compacts_at_its_point_without_instructions() {
    assert_eq!(claude_compactions("native-bare").await, vec![None]);
}

/// The session's ledger: two decisions and an open item go back; a note doesn't.
const LEDGER: &str = "- L1 · 10-24 14:42 · gen 1 · Decision: store it in SQLite
- L2 · 10-24 14:43 · gen 1 · Rule: never push to main
- L3 · 10-24 14:44 · gen 1 · Note: the build took 4 minutes
- L4 · 10-24 14:45 · gen 1 · Open: the flaky login test
";

const KEPT: [&str; 3] = [
    "- L1 · 10-24 14:42 · gen 1 · Decision: store it in SQLite",
    "- L2 · 10-24 14:43 · gen 1 · Rule: never push to main",
    "- L4 · 10-24 14:45 · gen 1 · Open: the flaky login test",
];

fn record(flow: &Flow) -> std::path::PathBuf {
    flow.manager.session_dir(&flow.conversation).record()
}

fn write_ledger(flow: &Flow, ledger: &str) {
    let record = record(flow);
    std::fs::create_dir_all(&record).unwrap();
    std::fs::write(record.join("ledger.md"), ledger).unwrap();
}

async fn notices(flow: &Flow) -> Vec<String> {
    flow.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::ConversationNotice { notice, .. } => Some(notice.text),
            _ => None,
        })
        .collect()
}

/// Waits until the put-back is in `state`.
async fn until_put_back(flow: &Flow, state: PutBackState) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while read_long(&record(flow)).put_back.map(|put| put.state) != Some(state) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("a put-back {state:?}: {:?}", read_long(&record(flow))));
}

/// Starts a session on `thread` in `arm`, with the ledger.
async fn long_flow(
    name: &str,
    provider: ProviderKind,
    behavior: FakeBehavior,
    arm: &str,
    inputs: Inputs,
) -> Flow {
    let options = Options {
        thread: provider,
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            take_in_steers: true,
            ..behavior
        }),
        ..Options::default()
    };
    let script = if provider == ProviderKind::Claude {
        claude_thread(inputs)
    } else {
        thread(inputs)
    };
    let mut flow = Flow::start(name, options, script).await;
    switch(&mut flow, LongSessions::parse(arm));
    write_ledger(&flow, LEDGER);
    flow.say("Start.").await;
    flow.settled().await;
    flow
}

#[tokio::test]
async fn claudes_decisions_go_back_whole_after_its_compaction_with_no_notice() {
    let inputs: Inputs = Arc::default();
    let flow = long_flow(
        "long-claude-restore",
        ProviderKind::Claude,
        FakeBehavior::default(),
        "native",
        inputs.clone(),
    )
    .await;
    flow.say("Grow.").await;
    until_put_back(&flow, PutBackState::Received).await;
    flow.idle().await;
    let file = read_seam(&record(&flow));
    assert_eq!((file.generation, file.phase), (1, Phase::Live));
    assert!(notices(&flow).await.is_empty());
    // `native` puts back but says nothing of what the summary must keep (run2-D7 off).
    assert_eq!(*flow.behavior.compactions.lock().unwrap(), vec![None]);
    flow.stop().await;
}

#[tokio::test]
async fn with_claudes_hooks_off_the_user_is_told_once_and_the_session_goes_on() {
    let inputs: Inputs = Arc::default();
    let flow = long_flow(
        "long-claude-hooks-off",
        ProviderKind::Claude,
        FakeBehavior {
            hooks_off: true,
            ..FakeBehavior::default()
        },
        "native-full",
        inputs.clone(),
    )
    .await;
    for _ in 0..2 {
        flow.say("Grow.").await;
        flow.idle().await;
        flow.idle().await;
    }
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 2);
    let told = notices(&flow).await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("hooks are off"));
    assert!(read_long(&record(&flow)).hooks_off_told);
    let file = read_seam(&record(&flow));
    assert_eq!((file.generation, file.phase), (1, Phase::Live));
    flow.stop().await;
}

#[tokio::test]
async fn decisions_too_long_to_go_back_whole_send_the_session_to_a_fresh_one() {
    let inputs: Inputs = Arc::default();
    let flow = long_flow(
        "long-claude-too-long",
        ProviderKind::Claude,
        FakeBehavior::default(),
        "native",
        inputs.clone(),
    )
    .await;
    let long = format!(
        "- L1 · 10-24 14:42 · gen 1 · Decision: {}\n",
        "keep every word ".repeat(700)
    );
    write_ledger(&flow, &long);
    flow.say("Grow.").await;
    until_put_back(&flow, PutBackState::Failed).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while read_seam(&record(&flow)).generation < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a fresh session at the fallback");
    flow.idle().await;
    let told = notices(&flow).await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("could not be put back whole"));
    flow.stop().await;
}

/// The Codex notes put back after compaction `compaction-1` so far.
fn codex_notes(flow: &Flow) -> Vec<String> {
    flow.behavior
        .put_backs
        .lock()
        .unwrap()
        .iter()
        .filter(|note| note.contains("brigadier-restore compaction-1"))
        .cloned()
        .collect()
}

/// Codex compacts mid-turn; its decisions go back once, whole, as one note, whatever its
/// answer does.
async fn codex_put_back(name: &str, behavior: FakeBehavior) {
    let inputs: Inputs = Arc::default();
    let flow = long_flow(name, ProviderKind::Codex, behavior, "native", inputs).await;
    flow.say("Grow.").await;
    until_put_back(&flow, PutBackState::Received).await;
    flow.idle().await;
    let notes = codex_notes(&flow);
    assert_eq!(notes.len(), 1, "{notes:?}");
    for kept in KEPT {
        assert_eq!(notes[0].matches(kept).count(), 1, "{kept}");
    }
    assert!(!notes[0].contains("L3 · "));
    assert!(notes[0].contains("you may quote them"));
    assert!(notes[0].contains("brigadier brain search"));
    assert!(notices(&flow).await.is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn codexs_decisions_go_back_once_after_its_compaction() {
    codex_put_back("long-codex-restore", FakeBehavior::default()).await;
}

#[tokio::test]
async fn a_put_back_whose_answer_was_lost_is_not_sent_again() {
    codex_put_back(
        "long-codex-lost",
        FakeBehavior {
            put_back_lost: true.into(),
            ..FakeBehavior::default()
        },
    )
    .await;
}

#[tokio::test]
async fn a_put_back_that_never_arrived_is_sent_again_once() {
    codex_put_back(
        "long-codex-refused",
        FakeBehavior {
            put_back_refused: 1.into(),
            ..FakeBehavior::default()
        },
    )
    .await;
}

/// A restart between sending the put-back and recording its receipt: the rollout decides.
async fn restarted(name: &str, state: PutBackState, arrived: bool) -> usize {
    let inputs: Inputs = Arc::default();
    let mut flow = long_flow(
        name,
        ProviderKind::Codex,
        FakeBehavior::default(),
        "native",
        inputs,
    )
    .await;
    flow.say("Grow.").await;
    until_put_back(&flow, PutBackState::Received).await;
    flow.idle().await;
    if !arrived {
        flow.behavior.put_backs.lock().unwrap().clear();
    }
    let mut file = read_long(&record(&flow));
    file.put_back.as_mut().unwrap().state = state;
    std::fs::write(
        record(&flow).join("long.json"),
        serde_json::to_vec(&file).unwrap(),
    )
    .unwrap();
    flow.restart().await;
    flow.say("After.").await;
    until_put_back(&flow, PutBackState::Received).await;
    flow.idle().await;
    let notes = codex_notes(&flow).len();
    flow.stop().await;
    notes
}

#[tokio::test]
async fn after_a_restart_a_put_back_that_arrived_is_not_sent_again() {
    assert_eq!(
        restarted("long-codex-receipt-lost", PutBackState::Sent, true).await,
        1
    );
    assert_eq!(
        restarted("long-codex-sending-arrived", PutBackState::Sending, true).await,
        1
    );
}

#[tokio::test]
async fn after_a_restart_a_put_back_that_never_arrived_goes_once() {
    assert_eq!(
        restarted("long-codex-sending-lost", PutBackState::Sending, false).await,
        1
    );
}

/// Waits until the session went on in a fresh one.
async fn until_successor(flow: &Flow) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while read_seam(&record(flow)).generation < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("a fresh session at the fallback");
    flow.idle().await;
}

/// A Codex session whose compaction on "Grow." ends with `error` and `after` tokens.
async fn codex_compacting(name: &str, error: Option<&str>, after: i64) -> (Flow, Inputs) {
    let inputs: Inputs = Arc::default();
    let seen = inputs.clone();
    let error = error.map(str::to_owned);
    let mut flow = Flow::start(
        name,
        options(ProviderKind::Codex),
        script(move |turn| {
            let (inputs, error) = (seen.clone(), error.clone());
            async move {
                inputs
                    .lock()
                    .unwrap()
                    .push((turn.provider, turn.input.clone()));
                if turn.input.starts_with(NUDGE_MARK) {
                    std::fs::write(note_of(&turn.input), "Doing now: nothing.").unwrap();
                    return Reply::text("Handed off.");
                }
                if without_handoff(&turn.input).contains("Grow.") {
                    turn.report_context(70_000).await;
                    turn.emit(ProviderEvent::CompactionStarted { automatic: true })
                        .await;
                    turn.emit(ProviderEvent::CompactionEnded {
                        automatic: true,
                        tokens_before: Some(70_000),
                        tokens_after: Some(after),
                        error,
                        native_id: Some("compaction-1".into()),
                    })
                    .await;
                    return Reply::text("Grown.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    switch(&mut flow, LongSessions::parse("native"));
    write_ledger(&flow, LEDGER);
    flow.say("Start.").await;
    flow.settled().await;
    flow.say("Grow.").await;
    (flow, inputs)
}

#[tokio::test]
async fn a_failed_compaction_sends_the_session_to_a_fresh_one_with_its_working_state() {
    let (flow, inputs) = codex_compacting(
        "long-codex-compaction-failed",
        Some("the summary failed"),
        12_000,
    )
    .await;
    until_successor(&flow).await;
    let seen = inputs.lock().unwrap().clone();
    // The hand-off asks for the working state and the evidence's exact pointers (P3 b, e)…
    let nudge = seen
        .iter()
        .find(|(_, input)| input.starts_with(NUDGE_MARK))
        .expect("a nudge");
    assert!(
        nudge
            .1
            .contains("the last test you ran with its exact command")
    );
    assert!(
        nudge
            .1
            .contains("`brigadier brain read <session> --item <id>`")
    );
    // …and the successor is told to look things up before calling them unknown (P3 a).
    assert!(
        seen.iter()
            .any(|(_, input)| input.contains("Before you say something is unknown"))
    );
    let told = notices(&flow).await;
    assert_eq!(told.len(), 1, "{told:?}");
    flow.stop().await;
}

#[tokio::test]
async fn a_context_still_above_half_the_point_after_compacting_sends_the_session_on() {
    // 40k after compacting at a 60k point (the half is proposed, run2-D10).
    let (flow, _) = codex_compacting("long-codex-churn", None, 40_000).await;
    until_successor(&flow).await;
    flow.stop().await;
}

#[tokio::test]
async fn a_compaction_well_under_half_the_point_keeps_the_session() {
    let (flow, _) = codex_compacting("long-codex-under-half", None, 25_000).await;
    until_put_back(&flow, PutBackState::Received).await;
    flow.idle().await;
    assert_eq!(read_seam(&record(&flow)).generation, 1);
    flow.stop().await;
}

/// A Claude session whose turns on "Overflow." fail as too long, `overflows` times, or with
/// Claude's thrash stop.
async fn claude_overflowing(name: &str, overflows: u32, message: &'static str) -> (Flow, Inputs) {
    let inputs: Inputs = Arc::default();
    let left = Arc::new(std::sync::atomic::AtomicU32::new(overflows));
    let base = claude_thread(inputs.clone());
    let options = options(ProviderKind::Claude);
    let mut flow = Flow::start(
        name,
        options,
        script(move |turn| {
            let (base, left) = (base.clone(), left.clone());
            async move {
                let over = without_handoff(&turn.input).contains("Overflow.")
                    && left
                        .fetch_update(
                            std::sync::atomic::Ordering::SeqCst,
                            std::sync::atomic::Ordering::SeqCst,
                            |n| n.checked_sub(1),
                        )
                        .is_ok();
                if over {
                    turn.emit(ProviderEvent::Error {
                        error: brigadier_providers::ProviderError {
                            kind: if message.starts_with("Prompt") {
                                brigadier_providers::ErrorKind::ContextWindow
                            } else {
                                brigadier_providers::ErrorKind::Other
                            },
                            message: message.into(),
                            will_retry: false,
                            limit: None,
                            code: None,
                        },
                    })
                    .await;
                    return Reply::failed(message);
                }
                base(turn).await
            }
        }),
    )
    .await;
    switch(&mut flow, LongSessions::parse("native-full"));
    flow.say("Start.").await;
    flow.settled().await;
    (flow, inputs)
}

#[tokio::test]
async fn a_context_too_long_compacts_first_and_the_session_goes_on_when_that_recovers() {
    let (flow, inputs) =
        claude_overflowing("long-claude-too-long-once", 1, "Prompt is too long").await;
    flow.say("Overflow.").await;
    flow.idle().await;
    flow.idle().await;
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 1);
    flow.say("After.").await;
    flow.idle().await;
    assert!(
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|(_, input)| input.contains("After."))
    );
    assert_eq!(read_seam(&record(&flow)).generation, 1);
    flow.stop().await;
}

#[tokio::test]
async fn a_context_still_too_long_after_compacting_goes_on_in_a_fresh_session() {
    let (flow, _) = claude_overflowing("long-claude-too-long-twice", 2, "Prompt is too long").await;
    flow.say("Overflow.").await;
    flow.idle().await;
    flow.idle().await;
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 1);
    flow.say("Overflow.").await;
    until_successor(&flow).await;
    flow.stop().await;
}

#[tokio::test]
async fn claudes_thrash_stop_sends_the_session_to_a_fresh_one() {
    let (flow, _) = claude_overflowing(
        "long-claude-thrash",
        1,
        "Autocompact is thrashing: the context refilled to the limit 3 times in a row",
    )
    .await;
    flow.say("Overflow.").await;
    until_successor(&flow).await;
    assert!(flow.behavior.compactions.lock().unwrap().is_empty());
    flow.stop().await;
}
