//! Long sessions (run2-D2, D15): by default a session's thread compacts in place with its CLI's
//! own compaction instead of going on in a fresh session at a size, and nothing goes back into
//! the conversation after it (D13); the test-only `seams` opt-out keeps the seam as before. After
//! five compactions in one CLI session it goes on in a fresh one (D16).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ProviderEvent, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::conversation::long::read_long;
use crate::manager::long_sessions::LongSessions;
use crate::manager::seam::{Phase, SeamConfig, read_seam, without_handoff};
use crate::model::DomainEvent;
use crate::thread_view::NUDGE_MARK;

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

#[tokio::test]
async fn a_codex_session_compacts_in_place_by_default_with_nothing_put_back() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "long-codex",
        options(ProviderKind::Codex),
        thread(inputs.clone()),
    )
    .await;
    switch(&mut flow, LongSessions::parse(""));
    flow.say("Start.").await;
    flow.settled().await;
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After.").await;
    flow.settled().await;
    // No hand-off was asked for, no successor started and nothing of Brigadier's went in: the
    // same CLI session goes on with only the user's messages (D13).
    let seen: Vec<String> = inputs
        .lock()
        .unwrap()
        .iter()
        .map(|(_, input)| input.clone())
        .collect();
    assert_eq!(seen, ["Start.", "Grow.", "After."]);
    assert_eq!(read_long(&record(&flow)).compactions, 1);
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert_eq!((file.generation, file.phase), (1, Phase::Live));
    assert!(file.seams.is_empty());
    // Codex compacts where it would by itself, or at the point a comparison set (run2-D6).
    let specs = flow.specs();
    let (_, spec) = specs
        .iter()
        .rev()
        .find(|(kind, _)| *kind == ProviderKind::Codex)
        .unwrap();
    let native = spec.native_compaction.clone().expect("compacts natively");
    assert_eq!(native.codex_limit, Some(60_000));
    flow.stop().await;
}

#[tokio::test]
async fn with_the_seams_opt_out_a_codex_session_still_goes_on_at_its_seam() {
    let inputs: Inputs = Arc::default();
    let mut flow = Flow::start(
        "long-codex-off",
        options(ProviderKind::Codex),
        thread(inputs.clone()),
    )
    .await;
    switch(&mut flow, LongSessions::parse("seams"));
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
    // The same CLI session goes on: no hand-off, no successor.
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
    // Claude may compact by itself as a backstop just above the point (run2-D5).
    let specs = flow.specs();
    let (_, spec) = specs
        .iter()
        .rev()
        .find(|(kind, _)| *kind == ProviderKind::Claude)
        .unwrap();
    let native = spec.native_compaction.clone().expect("compacts natively");
    assert_eq!(native.backstop_window, Some(110_000));
    // Its `/compact`, as Claude keeps it, is Brigadier's: never the user's bubble (L12).
    for instructions in behavior.compactions.lock().unwrap().iter() {
        let words = match instructions {
            Some(instructions) => format!("/compact {instructions}"),
            None => "/compact".to_owned(),
        };
        assert_eq!(
            flow.noted_said(&words),
            Some(brigadier_providers::session_file::Said::Brigadier),
            "{words}"
        );
    }
    flow.stop().await;
    behavior.compactions.lock().unwrap().clone()
}

#[tokio::test]
async fn a_claude_session_compacts_once_at_its_point_with_what_its_summary_must_keep() {
    let compactions = claude_compactions("").await;
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
    // The summary points to where everything else is (D13).
    assert!(instructions.ends_with("next: run `brigadier context`."));
}

#[tokio::test]
async fn a_bare_claude_session_compacts_at_its_point_without_instructions() {
    assert_eq!(claude_compactions("native-bare").await, vec![None]);
}

/// The session's ledger: it stays where it is, for `brigadier context` (D13).
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
    switch(&mut flow, LongSessions::parse(""));
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
    // …and the successor is pointed at `brigadier context`, where it pulls the rest (D13).
    assert!(
        seen.iter()
            .any(|(_, input)| input.contains(crate::manager::seam::CONTEXT_POINTER))
    );
    let told = notices(&flow).await;
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("its compaction failed: the summary failed"));
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
    flow.idle().await;
    assert_eq!(read_long(&record(&flow)).compactions, 1);
    assert_eq!(read_seam(&record(&flow)).generation, 1);
    flow.stop().await;
}

/// D16: the fifth compaction in one CLI session hands off to a fresh one, under the hood (no
/// notice, run2-D44); the fourth doesn't, and the successor counts from none.
#[tokio::test]
async fn the_fifth_compaction_in_a_row_hands_off_to_a_fresh_session() {
    let (flow, inputs) = codex_compacting("long-codex-five", None, 25_000).await;
    flow.idle().await;
    for _ in 2..crate::manager::long_sessions::SESSION_HANDOFF_AFTER {
        flow.say("Grow.").await;
        flow.idle().await;
    }
    assert_eq!(read_long(&record(&flow)).compactions, 4);
    assert_eq!(read_seam(&record(&flow)).generation, 1);
    flow.say("Grow.").await;
    until_successor(&flow).await;
    assert!(notices(&flow).await.is_empty());
    let seen = inputs.lock().unwrap().clone();
    assert_eq!(
        seen.iter()
            .filter(|(_, input)| input.starts_with(NUDGE_MARK))
            .count(),
        1
    );
    // The successor's own compactions count again from none.
    flow.say("Grow.").await;
    flow.idle().await;
    let file = read_long(&record(&flow));
    assert_eq!((file.generation, file.compactions), (2, 1));
    flow.stop().await;
}

/// A Claude session whose turns on "Overflow." fail as too long, `overflows` times, or with
/// Claude's thrash stop.
async fn claude_overflowing(name: &str, overflows: u32, message: &'static str) -> (Flow, Inputs) {
    let inputs: Inputs = Arc::default();
    let left = Arc::new(std::sync::atomic::AtomicU32::new(overflows));
    let base = claude_thread(inputs.clone());
    let seen = inputs.clone();
    let options = options(ProviderKind::Claude);
    let mut flow = Flow::start(
        name,
        options,
        script(move |turn| {
            let (base, left, seen) = (base.clone(), left.clone(), seen.clone());
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
                    seen.lock()
                        .unwrap()
                        .push((turn.provider, turn.input.clone()));
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
async fn a_context_too_long_compacts_and_goes_again_by_itself_when_that_recovers() {
    let (flow, inputs) =
        claude_overflowing("long-claude-too-long-once", 1, "Prompt is too long").await;
    flow.say("Overflow.").await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while given(&inputs, "Overflow.") < 2 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the request again, with no word from the user");
    let board = flow.settled().await;
    flow.idle().await;
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 1);
    assert_eq!(given(&inputs, "Overflow."), 2);
    // The request went through: nothing failed, nothing to tell.
    assert!(
        board
            .requests
            .values()
            .all(|request| matches!(request.state, crate::work::RequestState::Done)),
        "{:?}",
        board.requests
    );
    assert!(notices(&flow).await.is_empty());
    assert_eq!(read_seam(&record(&flow)).generation, 1);
    flow.stop().await;
}

#[tokio::test]
async fn a_context_still_too_long_after_compacting_goes_on_in_a_fresh_session() {
    let (flow, inputs) =
        claude_overflowing("long-claude-too-long-twice", 2, "Prompt is too long").await;
    flow.say("Overflow.").await;
    until_successor(&flow).await;
    assert_eq!(flow.behavior.compactions.lock().unwrap().len(), 1);
    // Tried twice in the session, then the successor takes the request on from its hand-off.
    assert_eq!(given(&inputs, "Overflow."), 2);
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|request| !matches!(request.state, crate::work::RequestState::Failed { .. })),
        "{:?}",
        board.requests
    );
    flow.stop().await;
}

/// How many inputs held `needle`, outside hand-offs.
fn given(inputs: &Inputs, needle: &str) -> usize {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, input)| without_handoff(input).contains(needle))
        .count()
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

/// `brigadier checkpoint` (run2-D9): the thread replaces and shows its checkpoint, with the
/// switch off too; a worker and a Chat keep none.
#[tokio::test]
async fn the_thread_sets_and_shows_its_checkpoint_and_workers_and_chats_keep_none() {
    use brigadier_cli::protocol::Tool;
    use serde_json::{Value, json};

    use super::chat_thread_tests::new_chat;
    use super::cli_tests::{brigadier, tool};

    let refused = Arc::new(Mutex::new(Vec::<Value>::new()));
    let flow = Flow::start(
        "long-checkpoint",
        Options::default(),
        script({
            let refused = refused.clone();
            move |turn| {
                let refused = refused.clone();
                async move {
                    let set = |text: &str| tool(Tool::Checkpoint, "set", json!({"text": text}));
                    if turn.worker().is_some() || !turn.input.contains("Start a worker") {
                        let answer = brigadier(&turn, set("Goal: mine.")).await;
                        refused.lock().unwrap().push(answer);
                        return Reply::text("Refused.");
                    }
                    let show = || tool(Tool::Checkpoint, "show", json!({}));
                    let none = brigadier(&turn, show()).await;
                    assert_eq!(
                        none,
                        json!({"action": "show", "text": "No checkpoint yet.\n"})
                    );
                    for text in ["Goal: one.", "Goal: two.\nNext: test."] {
                        let saved = brigadier(&turn, set(text)).await;
                        assert_eq!(
                            saved,
                            json!({"action": "set", "text": "Checkpoint saved.\n"})
                        );
                    }
                    let shown = brigadier(&turn, show()).await;
                    assert_eq!(shown["text"], "Goal: two.\nNext: test.\n");
                    turn.start_worker("Checkpoint", "Keep a checkpoint.").await;
                    Reply::text("Saved.")
                }
            }
        }),
    )
    .await;
    flow.say("Start a worker and keep a checkpoint.").await;
    flow.settled().await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Keep a checkpoint.").await;
    flow.settled_in(&chat).await;
    let record = flow.manager.session_dir(&flow.conversation).record();
    assert_eq!(
        crate::manager::long_sessions::checkpoint::read(&record).as_deref(),
        Some("Goal: two.\nNext: test.")
    );
    {
        let refused = refused.lock().unwrap();
        assert_eq!(refused.len(), 2, "the worker's and the Chat's: {refused:?}");
        for answer in refused.iter() {
            assert_eq!(
                answer["error"]["message"], "Only a session's thread keeps a checkpoint.",
                "{answer}"
            );
        }
    }
    let chat_record = flow.manager.session_dir(&chat).record();
    assert_eq!(
        crate::manager::long_sessions::checkpoint::read(&chat_record),
        None
    );
    flow.stop().await;
}
