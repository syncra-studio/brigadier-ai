//! The thread's own work in its transcript index (BUILD-PLAN D2.4): the commands it ran and
//! the files it read are found by `search_transcript` after it was reborn, shown as thread
//! steps (never as a message's `#N`), with where a whole output is kept; a conversation indexed
//! before has its thread's whole stream indexed on its next search.

use std::sync::{Arc, Mutex};

use brigadier_brain::{TranscriptEntry, TranscriptStream};
use brigadier_providers::{ItemStatus, KeptOutput, ProviderEvent, ProviderKind};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::tools::SearchTranscript;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// Only the command's output says it.
const MARKER: &str = "quasarxylophone";
/// Only the read file's path says it.
const READ_MARKER: &str = "nebulapangolin";
/// Past the 300k prepare and 375k swap lines of a 1M window.
const FULL: i64 = 460_000;

/// 20 KiB of output with `MARKER` 1 KiB in.
fn whole_output() -> String {
    let line = "build line filler text\n";
    let mut text = line.repeat(1024 / line.len());
    text.push_str(&format!("error: {MARKER} failed to link\n"));
    while text.len() < 20 * 1024 {
        text.push_str(line);
    }
    text
}

/// A command the thread ran, recorded as its CLI's event: its output cut at 8 KiB, the whole
/// kept.
fn command_event(item: &str, command: &str, whole: &str) -> ProviderEvent {
    ProviderEvent::Command {
        item_id: item.into(),
        command: command.into(),
        cwd: None,
        status: ItemStatus::Completed,
        exit_code: Some(1),
        output: Some(format!(
            "{}… [{} more bytes]",
            &whole[..8 * 1024],
            whole.len() - 8 * 1024
        )),
        duration_ms: None,
        kept: Some(Box::new(KeptOutput {
            bytes: whole.len() as u64,
            whole: Some(whole.into()),
            ..KeptOutput::default()
        })),
    }
}

async fn rebirths(flow: &Flow) -> usize {
    flow.core
        .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
        .await
        .unwrap()
        .entries
        .iter()
        .filter(|entry| matches!(entry.entry, crate::work::OrchestratorEntry::Rebirth { .. }))
        .count()
}

/// The thread runs a failing build and reads a file, is reborn twice, and its third session's
/// `search_transcript` finds both as thread steps, the command naming where its whole output
/// is kept.
#[tokio::test]
async fn a_command_the_thread_ran_is_found_after_two_rebirths_as_a_thread_step() {
    let found: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = found.clone();
    let flow = Flow::start(
        "transcript-thread",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Write the handoff note") {
                    return Reply::text("Goal and open threads: none.\nWhat to do next: wait.");
                }
                if turn.input.contains("Build it.") {
                    turn.emit(command_event(
                        "cmd-1",
                        "cargo build --release",
                        &whole_output(),
                    ))
                    .await;
                    turn.emit(ProviderEvent::ToolCall {
                        item_id: "read-1".into(),
                        name: "Read".into(),
                        input: Some(
                            json!({"file_path": format!("src/{READ_MARKER}.rs")}).to_string(),
                        ),
                        status: ItemStatus::Completed,
                        output: Some("1\tfn main() {}".into()),
                        kept: None,
                    })
                    .await;
                    return Reply::text("It failed to link.");
                }
                if turn.input.contains("Grow.") && !turn.input.contains("[Brigadier briefing") {
                    return Reply {
                        text: "Grown.".into(),
                        context_tokens: Some(FULL),
                        ..Reply::default()
                    };
                }
                if turn.input.contains("Look it up.") {
                    for query in [MARKER, READ_MARKER] {
                        let reply = turn
                            .call("search_transcript", json!({ "query": query }))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().push(reply.text);
                    }
                    return Reply::text("Found.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Build it.").await;
    flow.settled().await;
    for round in 1..=2 {
        flow.say("Grow.").await;
        flow.settled().await;
        flow.say(&format!("After rebirth {round}.")).await;
        flow.settled().await;
    }
    assert_eq!(rebirths(&flow).await, 2, "reborn twice");
    flow.say("Look it up.").await;
    flow.until("both searches", |_| found.lock().unwrap().len() == 2)
        .await;
    flow.settled().await;
    let found = found.lock().unwrap().clone();

    let command = &found[0];
    assert!(command.contains(MARKER), "{command}");
    assert!(
        command.starts_with("[command · ") && command.contains(" · thread step "),
        "{command}"
    );
    assert!(
        !command.contains(" · #"),
        "never as a message number: {command}"
    );
    let kept_at = format!("whole output ({} bytes): ", whole_output().len());
    assert!(
        command.contains(&format!("Ran `cargo build --release` (exit 1); {kept_at}")),
        "{command}"
    );
    assert!(!command.contains("……"), "{command}");
    // The whole output is where the hit says.
    let path = command
        .split(&kept_at)
        .nth(1)
        .and_then(|rest| rest.lines().next())
        .unwrap();
    let kept = std::fs::read_to_string(path).unwrap();
    assert_eq!(kept, whole_output());

    let read = &found[1];
    assert!(
        read.starts_with("[read · ") && read.contains(" · thread step "),
        "{read}"
    );
    assert!(
        read.contains(&format!("Read src/{READ_MARKER}.rs")),
        "{read}"
    );
    flow.stop().await;
}

/// A conversation whose messages were indexed before its thread's stream was (an old index,
/// its one watermark far past the thread's events): its next search indexes the thread's whole
/// stream from the start.
#[tokio::test]
async fn an_indexed_conversation_gets_its_whole_thread_stream_indexed() {
    let flow = Flow::start(
        "transcript-backfill",
        Options::default(),
        script(|_| async { Reply::text("Noted.") }),
    )
    .await;
    flow.say("The walrus plan.").await;
    flow.settled().await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    flow.manager
        .sync_transcript_stream(&flow.conversation, &brain, TranscriptStream::Conversation)
        .await
        .unwrap();
    // As an index made before thread steps were: messages only, up to a high seq.
    let conversation = flow.conversation.0.clone();
    let indexed = brain.clone();
    tokio::task::spawn_blocking(move || {
        indexed
            .index_transcript(vec![TranscriptEntry {
                conversation_id: conversation,
                stream: TranscriptStream::Conversation,
                seq: 1_000_000,
                role: "assistant".into(),
                request_id: None,
                at_ms: crate::now_ms(),
                text: "An old line.".into(),
            }])
            .unwrap();
    })
    .await
    .unwrap();
    let (watermark, thread) = {
        let (brain, conversation) = (brain.clone(), flow.conversation.0.clone());
        tokio::task::spawn_blocking(move || {
            (
                brain
                    .transcript_watermark(&conversation, TranscriptStream::Conversation)
                    .unwrap(),
                brain
                    .transcript_watermark(&conversation, TranscriptStream::Thread)
                    .unwrap(),
            )
        })
        .await
        .unwrap()
    };
    assert_eq!((watermark, thread), (Some(1_000_000), None));
    // The thread's command, recorded since.
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Provider {
                    provider: ProviderKind::Claude,
                    event: ProviderEvent::Command {
                        item_id: "cmd-late".into(),
                        command: "git log --oneline".into(),
                        cwd: None,
                        status: ItemStatus::Completed,
                        exit_code: Some(0),
                        output: Some(format!("abc123 {MARKER} landed\n")),
                        duration_ms: None,
                        kept: None,
                    },
                },
            },
        )])
        .await
        .unwrap();
    let text = flow
        .manager
        .search_transcript_tool(
            &flow.conversation,
            SearchTranscript {
                query: MARKER.into(),
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(text.starts_with("[command · "), "{text}");
    assert!(text.contains("Ran `git log --oneline` (exit 0)"), "{text}");
    assert!(text.contains(MARKER), "{text}");
    // The messages are still found, with their numbers.
    let text = flow
        .manager
        .search_transcript_tool(
            &flow.conversation,
            SearchTranscript {
                query: "walrus".into(),
                limit: None,
            },
        )
        .await
        .unwrap();
    assert!(
        text.starts_with("[user · ") && text.contains(" · #"),
        "{text}"
    );
    flow.stop().await;
}

/// A reborn Codex thread is told to look up what its briefing doesn't carry before it answers
/// "unknown" (issue #112): nothing but the briefing tells a fresh session it can.
#[tokio::test]
async fn a_reborn_codex_thread_is_told_to_look_up_before_it_answers_unknown() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "transcript-codex-lookup",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Write the handoff note") {
                    return Reply::text("Goal and open threads: none.\nWhat to do next: wait.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Grow.") && !turn.input.contains("[Brigadier briefing") {
                    return Reply {
                        text: "Grown.".into(),
                        context_tokens: Some(FULL),
                        ..Reply::default()
                    };
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Grow.").await;
    flow.settled().await;
    flow.say("After the rebirth.").await;
    flow.settled().await;
    assert_eq!(rebirths(&flow).await, 1, "reborn once");
    let inputs = inputs.lock().unwrap().clone();
    let briefing = inputs
        .iter()
        .find(|input| input.contains("[Brigadier briefing"))
        .expect("the reborn session's briefing");
    let older = briefing
        .split("[Older context]")
        .nth(1)
        .expect("what the briefing says of older context");
    assert!(
        older.contains("before you answer that something from it is unknown")
            && older.contains("look it up")
            && older.contains("`brigadier transcript search`")
            && older.contains("`brigadier brain search`"),
        "{briefing}"
    );
    flow.stop().await;
}
