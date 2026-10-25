//! Every kept role writes the same ledger it could write by hand; tab chats leave none.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::Tool;
use brigadier_providers::ProviderKind;
use serde_json::{Value, json};

use super::chat_thread_tests::new_chat;
use super::cli_tests::{brigadier, tool};
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::LEDGER_ENV;
use crate::model::{ConversationId, TabTarget};

const HAND: &str = "- Rule from the user: \"tabs never\" (message 1; scope: session)\n";
const ENTRY: &str = "Decision: keep spaces (I decided; source: message 1)";

/// `text` with each stamp's time written as `<time>`: `add` stamps the time itself.
fn unstamped(text: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| match line.split_once(" · ") {
            Some((head, rest))
                if head
                    .trim_start_matches(['-', '*', '+', ' '])
                    .starts_with('L') =>
            {
                let (time, rest) = rest.split_once(" · ").expect("a stamp's time");
                assert_eq!(time.len(), 11, "MM-DD HH:MM: {time}");
                format!("{head} · <time> · {rest}")
            }
            _ => line.to_owned(),
        })
        .collect()
}

#[tokio::test]
async fn threads_workers_and_chats_add_and_show_their_own_ledger() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let seen = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let script: Script = {
            let seen = seen.clone();
            Arc::new(move |turn| {
                let seen = seen.clone();
                Box::pin(async move {
                    let path = std::path::PathBuf::from(turn.env(LEDGER_ENV).unwrap());
                    if seen
                        .lock()
                        .unwrap()
                        .contains_key(&path.display().to_string())
                    {
                        return Reply::text("Recorded.");
                    }
                    let empty = brigadier(&turn, tool(Tool::Ledger, "show", json!({}))).await;
                    assert_eq!(empty["text"], "No entries yet.\n");
                    assert!(!path.exists(), "show creates no ledger");
                    let created =
                        brigadier(&turn, tool(Tool::Ledger, "add", json!({"text": ENTRY}))).await;
                    assert_eq!(created, json!({"action": "add", "text": "L1\n"}));
                    assert_eq!(
                        unstamped(&std::fs::read_to_string(&path).unwrap()),
                        format!("- L1 · <time> · gen 1 · {ENTRY}\n")
                    );
                    // The hand-written format in the standing instructions is also the CLI's.
                    std::fs::write(&path, HAND).unwrap();
                    let added =
                        brigadier(&turn, tool(Tool::Ledger, "add", json!({"text": ENTRY}))).await;
                    assert_eq!(added, json!({"action": "add", "text": "L1\n"}));
                    let shown = brigadier(&turn, tool(Tool::Ledger, "show", json!({}))).await;
                    let expected = std::fs::read_to_string(&path).unwrap();
                    assert_eq!(shown["text"], expected);
                    assert_eq!(
                        unstamped(&expected),
                        format!("{HAND}- L1 · <time> · gen 1 · {ENTRY}\n")
                    );
                    seen.lock()
                        .unwrap()
                        .insert(path.display().to_string(), expected);
                    if turn.worker().is_none() && turn.input.contains("Start a worker") {
                        turn.start_worker("Keep ledger", "Keep your ledger.").await;
                    }
                    Reply::text("Recorded.")
                })
            })
        };
        let flow = Flow::start(
            "ledger-roles",
            Options {
                thread: provider,
                ..Options::default()
            },
            script,
        )
        .await;
        flow.say("Start a worker and keep your ledger.").await;
        flow.settled().await;
        let chat = new_chat(&flow).await;
        flow.say_to(&chat, "Keep your ledger.").await;
        flow.settled_in(&chat).await;
        let tasks = flow.core.tasks(&flow.conversation).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 3, "every role uses a different file: {seen:?}");
            assert!(
                seen.contains_key(
                    &flow
                        .manager
                        .session_dir(&flow.conversation)
                        .record()
                        .join("ledger.md")
                        .display()
                        .to_string()
                )
            );
            assert!(
                seen.contains_key(
                    &flow
                        .manager
                        .session_dir(&chat)
                        .record()
                        .join("ledger.md")
                        .display()
                        .to_string()
                )
            );
            let worker = flow
                .manager
                .session_dir(&flow.conversation)
                .worker(&tasks[0])
                .join("record/ledger.md");
            assert!(seen.contains_key(&worker.display().to_string()));
        }
        flow.stop().await;
    }
}

#[tokio::test]
async fn tab_chats_refuse_ledger_add_and_show_without_creating_a_ledger() {
    for target in [
        TabTarget::File {
            path: "README.md".into(),
        },
        TabTarget::Page {
            page_id: "page-1".into(),
        },
    ] {
        let seen = Arc::new(Mutex::new(Vec::<Value>::new()));
        let script: Script = {
            let seen = seen.clone();
            Arc::new(move |turn| {
                let seen = seen.clone();
                Box::pin(async move {
                    for (action, args) in [("add", json!({"text": ENTRY})), ("show", json!({}))] {
                        let answer = brigadier(&turn, tool(Tool::Ledger, action, args)).await;
                        seen.lock().unwrap().push(answer);
                    }
                    assert!(!std::path::Path::new(turn.env(LEDGER_ENV).unwrap()).exists());
                    Reply::text("Refused.")
                })
            })
        };
        let flow = Flow::start("ledger-tab-chat", Options::default(), script).await;
        flow.manager.keep_tab_chats(vec![]).await.unwrap();
        let chat = flow
            .manager
            .open_tab_chat(
                &flow.conversation,
                "ledger-tab".into(),
                target,
                ConversationId(uuid::Uuid::new_v4().to_string()),
            )
            .await
            .unwrap()
            .id;
        flow.say_to(&chat, "Keep a ledger.").await;
        flow.settled_in(&chat).await;
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            for answer in seen.iter() {
                assert_eq!(answer["error"]["code"], "refused");
                let reason = answer["error"]["message"].as_str().unwrap();
                assert!(reason.contains("beside a tab"), "{reason}");
                assert_eq!(reason.lines().count(), 1);
            }
        }
        flow.stop().await;
    }
}

#[tokio::test]
async fn ledger_append_keeps_old_bytes_and_list_format_and_surfaces_io_errors() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            let path = std::path::PathBuf::from(turn.env(LEDGER_ENV).unwrap());
            for (old, text, expected) in [
                (
                    "# Ledger\n\n* Decision: old (message 1)",
                    "Decision: new\n(source: message 2)\n",
                    "# Ledger\n\n* Decision: old (message 1)\n* L1 · <time> · gen 1 · Decision: new (source: message 2)\n",
                ),
                (
                    "+ Rule: old\r\n",
                    "- Learned from reading: README.md:1 says hello",
                    "+ Rule: old\r\n+ L1 · <time> · gen 1 · Learned from reading: README.md:1 says hello\r\n",
                ),
                (
                    HAND,
                    "Decision: next",
                    "- Rule from the user: \"tabs never\" (message 1; scope: session)\n- L1 · <time> · gen 1 · Decision: next\n",
                ),
            ] {
                std::fs::write(&path, old).unwrap();
                let answer =
                    brigadier(&turn, tool(Tool::Ledger, "add", json!({"text": text}))).await;
                assert_eq!(answer["text"], "L1\n", "{answer}");
                assert_eq!(
                    unstamped(&std::fs::read_to_string(&path).unwrap()),
                    expected
                );
            }
            let old = std::fs::read_to_string(&path).unwrap();
            let refused =
                brigadier(&turn, tool(Tool::Ledger, "add", json!({"text": " \n\t "}))).await;
            assert_eq!(refused["error"]["code"], "failed");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
            // A ledger that cannot be read must fail; no empty-file fallback or replacement.
            std::fs::write(&path, [0xff]).unwrap();
            for (action, args) in [("add", json!({"text": ENTRY})), ("show", json!({}))] {
                let answer = brigadier(&turn, tool(Tool::Ledger, action, args)).await;
                assert_eq!(answer["error"]["code"], "failed");
                assert_eq!(
                    answer["error"]["message"].as_str().unwrap().lines().count(),
                    1
                );
                assert_eq!(std::fs::read(&path).unwrap(), [0xff]);
            }
            Reply::text("Checked.")
        })
    });
    let flow = Flow::start("ledger-format", Options::default(), script).await;
    flow.say("Check ledger formats.").await;
    flow.settled().await;
    flow.stop().await;
}
