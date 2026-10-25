//! The standing instructions (STRIP-DOWN-PLAN S17, §6): every session, worker and Chat, on both
//! CLIs, starts with the same file and nothing else of Brigadier's, and its CLI's environment
//! names its own folder and ledger.

use std::sync::Arc;

use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};

use super::chat_thread_tests::new_chat;
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::{
    LEDGER_ENV, NO_AI_COAUTHORS, SESSION_DIR_ENV, STANDING_INSTRUCTIONS, WORKERS_LINE,
    standing_text,
};

/// A thread that starts one worker, and a worker that answers.
fn delegating() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.worker().is_some() {
                return Reply::text("Done.");
            }
            if turn.input.contains("Please add a greeting") {
                turn.start_worker("Add a greeting", "Create hello.txt.")
                    .await;
                return Reply::text("");
            }
            Reply::text("ok")
        })
    })
}

fn env<'a>(spec: &'a SessionSpec, name: &str) -> Option<&'a str> {
    spec.env
        .iter()
        .rev()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

fn of(specs: &[(ProviderKind, SessionSpec)], tools: ToolSet) -> SessionSpec {
    specs
        .iter()
        .find(|(_, spec)| spec.tools == tools)
        .unwrap_or_else(|| panic!("a {tools:?} session started"))
        .1
        .clone()
}

#[tokio::test]
async fn every_session_worker_and_chat_starts_with_the_standing_instructions_and_its_ledger() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "standing-instructions",
            Options {
                thread: provider,
                ..Options::default()
            },
            delegating(),
        )
        .await;
        assert!(flow.core.settings().omit_ai_coauthors, "the default");
        flow.say("Please add a greeting").await;
        flow.settled().await;
        // The worker's CLI starts after `brigadier start` answered, so maybe after the request
        // ended.
        flow.until("the worker's CLI started", |_| {
            flow.specs()
                .iter()
                .any(|(_, spec)| spec.tools == ToolSet::Lean)
        })
        .await;
        let chat = new_chat(&flow).await;
        flow.say_to(&chat, "Which nozzle?").await;
        flow.settled_in(&chat).await;
        let specs = flow.specs();
        // Exactly the file with its path placeholders, and for Codex the user's default
        // "no AI co-authors" (Claude's settings hold it to that instead).
        let template = match provider {
            ProviderKind::Claude => STANDING_INSTRUCTIONS.trim_end().to_owned(),
            ProviderKind::Codex => {
                format!("{}\n\n{NO_AI_COAUTHORS}", STANDING_INSTRUCTIONS.trim_end())
            }
        };
        let thread = of(&specs, ToolSet::Thread);
        let folder = flow.manager.session_dir(&flow.conversation);
        let expected = standing_text(&template);
        assert!(expected.contains("Keep a ledger at $BRIGADIER_LEDGER."));
        // S22b's lines (§6 as amended on 10-23): ids and the time from `ledger add`, the
        // Decision and Open kinds, and derived documents that cite entry ids.
        for line in [
            "`brigadier ledger add` works in every role except chats beside a tab and stamps each entry with an id and the time;",
            "- Decision: the user's words and what they answered (the option's text, not just \"A\"), or yours marked \"I decided\"; keep the sentence that defines it, and the reason.",
            "- Exact identifier (hash, absolute path:line, number with units, id)",
            "- Open: a promise, or a question waiting on the user; `brigadier ledger close <id>` when it is done.",
            "A plan or brief built from the ledger cites entry ids; mark lines of your own \"proposed\". Tell the user each \"I decided\" that changes what they asked.",
        ] {
            assert!(expected.contains(line), "{line}");
        }
        assert_eq!(thread.append_system_prompt.as_deref(), Some(&*expected));
        assert_eq!(
            env(&thread, SESSION_DIR_ENV),
            Some(&*folder.root().display().to_string())
        );
        assert_eq!(
            env(&thread, LEDGER_ENV),
            Some(&*folder.record().join("ledger.md").display().to_string())
        );
        // A worker gets the same file, with its own folder and ledger, but without the line on
        // running workers of its own, which it can't.
        let worker = of(&specs, ToolSet::Lean);
        let own = env(&worker, SESSION_DIR_ENV).expect("its folder");
        assert_eq!(
            worker.append_system_prompt.as_deref(),
            Some(&*standing_text(&template.replacen(
                &format!("\n{WORKERS_LINE}"),
                "",
                1
            )))
        );
        assert!(expected.contains(WORKERS_LINE));
        assert!(
            own.starts_with(&*folder.root().join("workers").display().to_string()),
            "{own}"
        );
        assert_eq!(
            env(&worker, LEDGER_ENV),
            Some(&*format!("{own}/record/ledger.md"))
        );
        // A Chat starts with the same file, then its own lines, none of which says it runs no
        // workers (G20).
        let chat_folder = flow.manager.session_dir(&chat);
        let chat_spec = specs
            .iter()
            .find(|(_, spec)| spec.cwd == chat_folder.work())
            .expect("the Chat started")
            .1
            .clone();
        let chat_prompt = chat_spec.append_system_prompt.as_deref().unwrap();
        assert!(
            chat_prompt.starts_with(&format!("{}\n\n", STANDING_INSTRUCTIONS.trim_end())),
            "{chat_prompt}"
        );
        assert!(!chat_prompt.contains("runs no workers"));
        assert_eq!(
            env(&chat_spec, LEDGER_ENV),
            Some(&*chat_folder.record().join("ledger.md").display().to_string())
        );
        // Every placeholder used by the standing text resolves in each actual CLI environment.
        for spec in [&thread, &worker, &chat_spec] {
            let text = spec.append_system_prompt.as_ref().unwrap();
            assert!(!text.contains(env(spec, SESSION_DIR_ENV).unwrap()));
            for name in [SESSION_DIR_ENV, LEDGER_ENV, "TMPDIR"] {
                assert!(text.contains(&format!("${name}")));
                assert!(!env(spec, name).unwrap_or_default().is_empty(), "{name}");
            }
        }
        flow.stop().await;
    }
}

/// The ledger is the thread's working memory alone: writing one gives the session's thread view
/// no ledger field, whether or not it exists.
#[tokio::test]
async fn a_session_with_a_ledger_shows_no_ledger_in_its_thread_view() {
    let flow = Flow::start(
        "standing-ledger",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    let view = flow.manager.thread_view(&id).unwrap();
    let json = serde_json::to_value(&view).unwrap();
    assert!(json.get("ledger").is_none(), "{json}");
    let ledger = flow.manager.session_dir(&id).record().join("ledger.md");
    std::fs::write(
        &ledger,
        "- Rule from the user: \"tabs never\" (message 1, project)\n",
    )
    .unwrap();
    let view = flow.manager.thread_view(&id).unwrap();
    let json = serde_json::to_value(&view).unwrap();
    assert!(json.get("ledger").is_none(), "{json}");
    flow.stop().await;
}
