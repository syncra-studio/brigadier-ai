//! "Continue on <other CLI>", asked for (S38c, D46): the thread asks with `brigadier
//! continue-on` from its own turn, which moves the session once that turn ends: its Claude CLI
//! writes the hand-off note, the divider shows, and a successor on Codex carries on at once
//! from the hand-off, on the model the user's rules allow (D25). The thread reads the same
//! after a restart. Refused, in plain words, while its turn runs without --after-turn, for a
//! worker, for a CLI turned off and when the rules allow no model there.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{Agent, CliRequest, GRANT_ENV, SESSION_ENV};
use brigadier_providers::ProviderKind;
use serde_json::Value;

use super::cli_tests::brigadier;
use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::seam::{CARRY_ON, Phase, read_seam, write_seam};
use crate::model::{ContinueWhen, Setup};
use crate::thread_view::{HANDOFF_MARK, NUDGE_MARK};

fn options() -> Options {
    Options {
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    }
}

/// Each turn: its CLI, session and input.
type Turns = Arc<Mutex<Vec<(ProviderKind, String, String)>>>;
/// What the thread's `brigadier continue-on` calls answered, in order.
type Answers = Arc<Mutex<Vec<Value>>>;

/// The note path a nudge names.
fn note_of(nudge: &str) -> std::path::PathBuf {
    let rest = nudge
        .split("write the hand-off note at ")
        .nth(1)
        .expect("a note path");
    rest.split(": doing now").next().unwrap().into()
}

/// Claude's thread: "Move to Codex." asks to continue on Codex, first without --after-turn
/// (refused: its own turn runs), then with it; the nudge writes the note. Codex answers.
fn thread(turns: Turns, answers: Answers) -> Script {
    Arc::new(move |turn: Turn| {
        let (turns, answers) = (turns.clone(), answers.clone());
        Box::pin(async move {
            turns
                .lock()
                .unwrap()
                .push((turn.provider, turn.native_id.clone(), turn.input.clone()));
            if turn.provider == ProviderKind::Codex {
                return Reply::text("Carrying on, on Codex.");
            }
            if turn.input.starts_with(NUDGE_MARK) {
                std::fs::write(
                    note_of(&turn.input),
                    "Doing now: nothing.\nNext: wait for the user.",
                )
                .unwrap();
                return Reply::text("Handed off.");
            }
            if turn.input.contains("Move to Codex.") {
                for after_turn in [false, true] {
                    let answer = brigadier(
                        &turn,
                        CliRequest::ContinueOn {
                            agent: Agent::Codex,
                            after_turn,
                        },
                    )
                    .await;
                    answers.lock().unwrap().push(answer);
                }
                return Reply::text("Moving to Codex after this turn.");
            }
            Reply::text("Noted.")
        })
    })
}

fn orchestrator(flow: &Flow) -> crate::model::ModelChoice {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session { orchestrator, .. }) => orchestrator,
        other => panic!("a session's setup, not {other:?}"),
    }
}

#[tokio::test]
async fn asked_from_its_turn_a_claude_session_continues_on_codex_once_with_its_note() {
    let (turns, answers) = (Turns::default(), Answers::default());
    let mut flow = Flow::start(
        "continue-on-codex",
        options(),
        thread(turns.clone(), answers.clone()),
    )
    .await;
    flow.say("Plant: keep the code K-001 as it is.").await;
    flow.settled().await;
    flow.say("Move to Codex.").await;
    flow.until("a turn on Codex", |_| {
        turns
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, ..)| *provider == ProviderKind::Codex)
    })
    .await;
    flow.idle().await;

    // Its own turn runs: refused, with how to ask for after the turn; then accepted for then.
    let answers = answers.lock().unwrap().clone();
    assert_eq!(answers.len(), 2, "{answers:?}");
    assert_eq!(answers[0]["error"]["code"], "refused", "{answers:?}");
    let words = answers[0]["error"]["message"].as_str().unwrap();
    assert!(
        words.contains("after this turn") && words.contains("--after-turn"),
        "{words}"
    );
    assert_eq!(answers[1]["when"], "after-turn", "{answers:?}");
    assert_eq!(answers[1]["agent"], "codex");

    // One note turn on Claude, then one successor on Codex, at once, from the hand-off.
    let turns = turns.lock().unwrap().clone();
    let shape: Vec<_> = turns
        .iter()
        .map(|(provider, _, input)| (*provider, input.starts_with(NUDGE_MARK)))
        .collect();
    assert_eq!(
        shape,
        [
            (ProviderKind::Claude, false),
            (ProviderKind::Claude, false),
            (ProviderKind::Claude, true),
            (ProviderKind::Codex, false),
        ]
    );
    let (_, successor, input) = &turns[3];
    assert!(input.starts_with(HANDOFF_MARK), "{input}");
    assert!(
        input.contains("## Hand-off note\nDoing now: nothing."),
        "{input}"
    );
    assert!(
        input.contains("Plant: keep the code K-001 as it is."),
        "{input}"
    );
    assert!(input.trim_end().ends_with(CARRY_ON), "{input}");

    // The session runs on Codex now, on the model its answer named (D25: the rules forbid
    // nothing, so Codex's own default).
    let now = orchestrator(&flow);
    assert_eq!(now.provider, ProviderKind::Codex);
    assert_eq!(answers[1]["model"], serde_json::json!(now.model));
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert!(matches!(file.phase, Phase::Live), "{:?}", file.phase);
    assert!(file.moving.is_none());
    assert_eq!(file.seams.len(), 1);
    let seam = &file.seams[0];
    assert_eq!(seam.provider, ProviderKind::Codex);
    assert_eq!(&seam.successor, successor);
    assert!(seam.note.is_some() && seam.end.death.is_none(), "{seam:?}");
    assert_eq!(seam.end.moved_to, Some(ProviderKind::Codex));

    // One divider, with its note and where it went; the same after a restart. (The fake Codex
    // keeps no session file, so where the divider stands is the live check's.)
    let before = flow.manager.thread_view(&flow.conversation).unwrap().seams;
    assert_eq!(before.len(), 1, "{before:?}");
    assert!(
        before[0].note.is_some() && before[0].nudge.is_some() && before[0].death.is_none(),
        "{before:?}"
    );
    assert_eq!(before[0].moved_to, Some(ProviderKind::Codex));
    flow.restart().await;
    let after = flow.manager.thread_view(&flow.conversation).unwrap().seams;
    assert_eq!(after, before);

    // Asked again: it runs there already, and nothing moves.
    let again = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, false)
        .await
        .unwrap();
    assert_eq!(again.0, ContinueWhen::Already);
    flow.stop().await;
}

/// The refusals, each in plain words: a worker (moved by its session), a CLI turned off, rules
/// that allow no model on it, and a Chat.
#[tokio::test]
async fn refused_for_a_worker_a_cli_turned_off_and_rules_that_allow_no_model_there() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn: Turn| {
            let answers = answers.clone();
            Box::pin(async move {
                if turn.input.contains("As a worker.") {
                    let session = turn.env(SESSION_ENV).unwrap();
                    let grant = turn.env(GRANT_ENV).unwrap();
                    let mut caller = turn.host.cli_caller(session, grant).unwrap();
                    caller.task_id = Some(crate::work::TaskId("t-1".into()));
                    let reply = turn
                        .host
                        .cli_call(
                            &caller,
                            brigadier_cli::protocol::CliCall {
                                protocol: brigadier_cli::protocol::PROTOCOL,
                                request: CliRequest::ContinueOn {
                                    agent: Agent::Codex,
                                    after_turn: true,
                                },
                            },
                        )
                        .await;
                    answers
                        .lock()
                        .unwrap()
                        .push(serde_json::to_value(reply).unwrap());
                } else if turn.input.contains("Move to Codex.") {
                    let answer = brigadier(
                        &turn,
                        CliRequest::ContinueOn {
                            agent: Agent::Codex,
                            after_turn: true,
                        },
                    )
                    .await;
                    answers.lock().unwrap().push(answer);
                }
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("continue-on-refused", options(), script).await;
    let refusal = |answer: &Value| -> String {
        assert_eq!(answer["error"]["code"], "refused", "{answer}");
        answer["error"]["message"].as_str().unwrap().to_owned()
    };

    flow.say("As a worker.").await;
    flow.settled().await;
    let worker = answers.lock().unwrap().pop().unwrap();
    assert!(
        refusal(&worker).contains("a worker is moved by its session"),
        "{worker}"
    );

    // Codex turned off.
    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Move to Codex.").await;
    flow.settled().await;
    let off = answers.lock().unwrap().pop().unwrap();
    assert!(refusal(&off).contains("Codex is turned off"), "{off}");
    let mut settings = flow.core.settings();
    settings.disabled_providers.clear();
    flow.core.update_settings(settings).await.unwrap();

    // The rules allow no Codex model it would pick: their words, and nothing moves.
    std::fs::write(
        flow.manager.models_rules_path(),
        "# My rules\n\n- allow codex: gpt-7\n",
    )
    .unwrap();
    flow.say("Move to Codex.").await;
    flow.settled().await;
    let ruled = answers.lock().unwrap().pop().unwrap();
    let words = refusal(&ruled);
    assert!(words.len() > "refused: ".len(), "{ruled}");
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Claude);
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert!(file.moving.is_none() && file.seams.is_empty(), "{file:?}");

    // A Chat has no seam: it changes its model in its picker.
    let chat = super::chat_thread_tests::new_chat(&flow).await;
    let refused = flow
        .manager
        .continue_on(&chat, ProviderKind::Codex, None, true)
        .await
        .unwrap_err();
    assert!(refused.contains("only a session"), "{refused}");
    flow.stop().await;
}

/// From the app, with no turn running: it moves now, with its note. A session waiting for
/// Claude's reset can't write one: it moves at once without, as S38 2b's does, and its waiting
/// message goes to Codex.
#[tokio::test]
async fn from_the_app_an_idle_session_moves_now_and_one_at_its_limit_without_a_note() {
    let turns = Turns::default();
    let script: Script = {
        let turns = turns.clone();
        Arc::new(move |turn: Turn| {
            let turns = turns.clone();
            Box::pin(async move {
                turns.lock().unwrap().push((
                    turn.provider,
                    turn.native_id.clone(),
                    turn.input.clone(),
                ));
                match turn.provider {
                    ProviderKind::Codex => Reply::text("On Codex."),
                    _ if turn.input.starts_with(NUDGE_MARK) => {
                        std::fs::write(note_of(&turn.input), "Doing now: nothing.").unwrap();
                        Reply::text("Handed off.")
                    }
                    _ if turn.input.contains("Hit the limit.") => Reply::limited(),
                    _ => Reply::text("Noted."),
                }
            })
        })
    };
    let flow = Flow::start("continue-on-app", options(), script).await;
    flow.say("Hello.").await;
    flow.idle().await;
    let (when, choice) = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, true)
        .await
        .unwrap();
    assert_eq!(when, ContinueWhen::Now);
    assert_eq!(choice.provider, ProviderKind::Codex);
    flow.until("a turn on Codex", |_| {
        turns
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, ..)| *provider == ProviderKind::Codex)
    })
    .await;
    flow.idle().await;
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert_eq!(file.seams.len(), 1);
    assert!(file.seams[0].note.is_some(), "{file:?}");
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);

    // Back on Claude, which then hits its limit: the message waits for its reset.
    let (when, _) = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Claude, None, true)
        .await
        .unwrap();
    assert_eq!(when, ContinueWhen::Now);
    // Codex writes no note in this fake: the generation ends without one, and Claude carries on.
    flow.until("a carry-on on Claude", |_| {
        turns.lock().unwrap().iter().any(|(provider, _, input)| {
            *provider == ProviderKind::Claude && input.starts_with(HANDOFF_MARK)
        })
    })
    .await;
    flow.idle().await;
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Claude);
    flow.say("Hit the limit.").await;
    flow.until("the wait for Claude's reset", |_| {
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_some()
    })
    .await;
    // Its request still works while it waits: only the turn is over.
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the limited turn to end", || async {
        conv.working_cli().await.is_none()
    })
    .await;
    let (when, _) = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, false)
        .await
        .unwrap();
    assert_eq!(when, ContinueWhen::Now);
    flow.until("the waiting message on Codex", |_| {
        turns.lock().unwrap().iter().any(|(provider, _, input)| {
            *provider == ProviderKind::Codex && input.contains("Hit the limit.")
        })
    })
    .await;
    flow.idle().await;
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    let last = file.seams.last().unwrap();
    assert_eq!(last.provider, ProviderKind::Codex);
    assert_eq!(
        last.end.death.as_deref(),
        Some("usage limit on Claude Code · continued on Codex")
    );
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);
    flow.stop().await;
}

/// A detail in an assistant's old turn is outside the size tail, but fits the requested move.
#[tokio::test]
async fn a_requested_move_carries_predecessor_turns_beyond_the_size_tail() {
    let turns = Turns::default();
    let script: Script = {
        let turns = turns.clone();
        Arc::new(move |turn: Turn| {
            let turns = turns.clone();
            Box::pin(async move {
                turns.lock().unwrap().push((
                    turn.provider,
                    turn.native_id.clone(),
                    turn.input.clone(),
                ));
                if turn.provider == ProviderKind::Codex {
                    return Reply::text("On Codex.");
                }
                if turn.input.starts_with(NUDGE_MARK) {
                    std::fs::write(note_of(&turn.input), "Next: wait.").unwrap();
                    return Reply::text("Handed off.");
                }
                let detail = if turn.input == "first batch" {
                    "SECRET-ASSISTANT-DETAIL"
                } else {
                    "batch"
                };
                Reply {
                    text: format!("{detail} {}", "x".repeat(4_000)),
                    context_tokens: Some(90_000),
                    ..Reply::default()
                }
            })
        })
    };
    let flow = Flow::start("d49-request-tail", options(), script).await;
    for n in 0..24 {
        flow.say(if n == 0 { "first batch" } else { "next batch" })
            .await;
        flow.settled().await;
    }
    flow.manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, false)
        .await
        .unwrap();
    flow.until("expanded hand-off on Codex", |_| {
        turns
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, ..)| *provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let turns = turns.lock().unwrap().clone();
    let input = &turns
        .iter()
        .find(|(provider, ..)| *provider == ProviderKind::Codex)
        .unwrap()
        .2;
    assert!(input.contains("SECRET-ASSISTANT-DETAIL"));
    let record = flow.manager.session_dir(&flow.conversation).record();
    let file = read_seam(&record);
    assert_eq!(file.seams[0].end.hot_tokens, Some(90_000));
    let lineage = crate::manager::session_dir::read_lineage(&record);
    let files: Vec<_> = lineage
        .iter()
        .filter(|s| s.provider == ProviderKind::Claude)
        .filter_map(|s| {
            Some((
                s.provider,
                brigadier_providers::session_file::locate(
                    s.provider,
                    std::path::Path::new(s.file.as_ref()?),
                ),
            ))
        })
        .collect();
    let entries = crate::manager::records::transcript(&files);
    let size = crate::manager::seam::bootstrap(&crate::manager::seam::Boot {
        n: 1,
        tail: crate::manager::seam::TailBudget {
            reason: crate::manager::seam::Reason::Size,
            hot_tokens: Some(90_000),
            target_window: Some(1_000_000),
        },
        key: "test",
        folder: &record,
        ledger_path: &record.join("ledger.md"),
        ledger: None,
        archive_path: &record.join("ledger-archive.md"),
        archive: None,
        lineage: &[],
        note_path: &record.join("handoff/1.md"),
        note: Some("Next: wait."),
        start_path: &record.join("handoff/1-start.md"),
        death: None,
        entries: &entries,
        blobs: flow.core.store().blobs().root(),
        long: false,
    });
    assert!(!size.contains("SECRET-ASSISTANT-DETAIL"));
    assert!(size.contains("brigadier brain read test"));
    flow.stop().await;
}

/// A move back to Claude uses its startup window too. With no reported window it keeps
/// the old tail; with a known one the earlier lineage detail fits, without a history read.
#[tokio::test]
async fn a_move_to_claude_carries_what_fits_its_reported_window() {
    for window in [Some(100_000), None] {
        let turns = Turns::default();
        let script: Script = {
            let turns = turns.clone();
            Arc::new(move |turn: Turn| {
                let turns = turns.clone();
                Box::pin(async move {
                    turns.lock().unwrap().push((
                        turn.provider,
                        turn.native_id.clone(),
                        turn.input.clone(),
                    ));
                    if turn.input.starts_with(NUDGE_MARK) {
                        std::fs::write(note_of(&turn.input), "Next: wait.").unwrap();
                        return Reply {
                            text: "Handed off.".into(),
                            context_tokens: Some(90_000),
                            ..Reply::default()
                        };
                    }
                    let text = if turn.input.starts_with(HANDOFF_MARK) {
                        "Carrying on.".into()
                    } else {
                        let detail = if turn.input == "first batch" {
                            "CLAUDE-TARGET-OLD-DETAIL"
                        } else {
                            "batch"
                        };
                        format!("{detail} {}", "x".repeat(4_000))
                    };
                    Reply {
                        text,
                        context_tokens: Some(90_000),
                        ..Reply::default()
                    }
                })
            })
        };
        let flow = Flow::start(
            "d49-claude-target",
            Options {
                behavior: Arc::new(FakeBehavior {
                    session_file: true,
                    claude_window: Some(window),
                    ..FakeBehavior::default()
                }),
                ..Options::default()
            },
            script,
        )
        .await;
        // Keep the detail only in the first CLI's native transcript, outside the 60KB tail.
        for n in 0..24 {
            flow.say(if n == 0 { "first batch" } else { "next batch" })
                .await;
            flow.idle().await;
        }
        for target in [ProviderKind::Codex, ProviderKind::Claude] {
            flow.manager
                .continue_on(&flow.conversation, target, None, false)
                .await
                .unwrap();
            flow.until("a successor on the requested CLI", |_| {
                turns
                    .lock()
                    .unwrap()
                    .last()
                    .is_some_and(|(provider, _, input)| {
                        *provider == target && input.starts_with(HANDOFF_MARK)
                    })
            })
            .await;
            flow.idle().await;
        }
        let input = turns.lock().unwrap().last().unwrap().2.clone();
        assert_eq!(input.contains("CLAUDE-TARGET-OLD-DETAIL"), window.is_some());
        assert!(input.contains("## Hand-off note\nNext: wait."));
        assert!(input.contains("brigadier brain read"));
        if window.is_some() {
            // Half the reported 100K window, at four UTF-8 bytes per token.
            assert!(input.len() > 60_000 && input.len() < 200_000);
        }
        let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
        assert_eq!(file.seams.len(), 2);
        let last = file.seams.last().unwrap();
        assert_eq!(last.provider, ProviderKind::Claude);
        assert_eq!(last.end.moved_to, Some(ProviderKind::Claude));
        assert_eq!(last.end.hot_tokens, Some(90_000));
        flow.stop().await;
    }
}

/// A move asked for in a turn the daemon's restart cut off is kept in the record: after the
/// restart its CLI writes the note, and the successor starts on Codex.
#[tokio::test]
async fn a_pending_move_survives_a_restart() {
    let (turns, answers) = (Turns::default(), Answers::default());
    let mut flow = Flow::start(
        "continue-on-restart",
        options(),
        thread(turns.clone(), answers),
    )
    .await;
    flow.say("Plant: keep the code K-002 as it is.").await;
    flow.settled().await;
    let record = flow.manager.session_dir(&flow.conversation).record();
    let mut file = read_seam(&record);
    file.moving = Some(crate::model::ModelChoice {
        provider: ProviderKind::Codex,
        model: None,
        effort: None,
        fast: None,
        account: None,
    });
    write_seam(&record, &file).unwrap();
    flow.restart().await;
    flow.until("a turn on Codex", |_| {
        turns
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, ..)| *provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let turns = turns.lock().unwrap().clone();
    let (_, _, input) = turns.last().unwrap();
    assert!(input.starts_with(HANDOFF_MARK), "{input}");
    assert!(input.contains("Plant: keep the code K-002"), "{input}");
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);
    let file = read_seam(&record);
    assert!(file.moving.is_none(), "{file:?}");
    assert_eq!(file.seams.len(), 1);
    assert!(file.seams[0].note.is_some(), "{file:?}");
    assert_eq!(file.seams[0].end.moved_to, Some(ProviderKind::Codex));
    flow.stop().await;
}

/// Asked for the move already coming with a model named, the move takes that model.
#[tokio::test]
async fn a_pending_move_takes_a_model_named_later() {
    let turns = Turns::default();
    let release = Arc::new(tokio::sync::Notify::new());
    let script: Script = {
        let (turns, release) = (turns.clone(), release.clone());
        Arc::new(move |turn: Turn| {
            let (turns, release) = (turns.clone(), release.clone());
            Box::pin(async move {
                turns.lock().unwrap().push((
                    turn.provider,
                    turn.native_id.clone(),
                    turn.input.clone(),
                ));
                if turn.input.contains("Hold.") {
                    release.notified().await;
                }
                Reply::text("Noted.")
            })
        })
    };
    let flow = Flow::start("continue-on-model", options(), script).await;
    flow.say("Hold.").await;
    flow.until("the held turn", |_| !turns.lock().unwrap().is_empty())
        .await;
    let (when, first) = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, true)
        .await
        .unwrap();
    assert_eq!(when, ContinueWhen::AfterTurn);
    let named = crate::model::ModelChoice {
        model: Some("named-model".into()),
        ..first.clone()
    };
    let (when, choice) = flow
        .manager
        .continue_on(
            &flow.conversation,
            ProviderKind::Codex,
            Some(named.clone()),
            true,
        )
        .await
        .unwrap();
    assert_eq!((when, &choice), (ContinueWhen::AfterTurn, &named));
    // Asked again with no model: the one named stays.
    let (_, again) = flow
        .manager
        .continue_on(&flow.conversation, ProviderKind::Codex, None, true)
        .await
        .unwrap();
    assert_eq!(again, named);
    release.notify_one();
    flow.until("a turn on Codex", |_| {
        turns
            .lock()
            .unwrap()
            .iter()
            .any(|(provider, ..)| *provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    assert_eq!(orchestrator(&flow).model.as_deref(), Some("named-model"));
    flow.stop().await;
}
