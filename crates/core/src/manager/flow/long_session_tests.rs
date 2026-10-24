//! Long sessions behind `BRIGADIER_LONG_SESSIONS` (run2-D2, run2-D15): with the switch on, a
//! session's thread compacts in place with its CLI's own compaction instead of going on in a
//! fresh session at a size; off, the seam comes as before.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ProviderEvent, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::long_sessions::LongSessions;
use crate::manager::seam::{Phase, SeamConfig, read_seam, without_handoff};
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
