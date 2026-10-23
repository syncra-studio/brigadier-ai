//! The standing instructions (STRIP-DOWN-PLAN S17, §6): every session, worker and Chat, on both
//! CLIs, starts with the same file and nothing else of Brigadier's, and its CLI's environment
//! names its own folder and ledger.

use std::sync::Arc;

use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};

use super::chat_thread_tests::new_chat;
use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::{
    LEDGER_ENV, NO_AI_COAUTHORS, SESSION_DIR_ENV, STANDING_INSTRUCTIONS,
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
        let chat = new_chat(&flow).await;
        flow.say_to(&chat, "Which nozzle?").await;
        flow.settled_in(&chat).await;
        let specs = flow.specs();
        // Exactly the file, and for Codex the user's default "no AI co-authors" (Claude's
        // settings hold it to that instead).
        let expected = match provider {
            ProviderKind::Claude => STANDING_INSTRUCTIONS.trim_end().to_owned(),
            ProviderKind::Codex => {
                format!("{}\n\n{NO_AI_COAUTHORS}", STANDING_INSTRUCTIONS.trim_end())
            }
        };
        let thread = of(&specs, ToolSet::Thread);
        assert_eq!(thread.append_system_prompt.as_deref(), Some(&*expected));
        let folder = flow.manager.session_dir(&flow.conversation);
        assert_eq!(
            env(&thread, SESSION_DIR_ENV),
            Some(&*folder.root().display().to_string())
        );
        assert_eq!(
            env(&thread, LEDGER_ENV),
            Some(&*folder.record().join("ledger.md").display().to_string())
        );
        // A worker gets the same file, with its own folder and ledger.
        let worker = of(&specs, ToolSet::Lean);
        assert_eq!(worker.append_system_prompt.as_deref(), Some(&*expected));
        let own = env(&worker, SESSION_DIR_ENV).expect("its folder");
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
        let chat_spec = of(&specs, ToolSet::Chat);
        let chat_prompt = chat_spec.append_system_prompt.as_deref().unwrap();
        assert!(
            chat_prompt.starts_with(&format!("{}\n\n", STANDING_INSTRUCTIONS.trim_end())),
            "{chat_prompt}"
        );
        assert!(!chat_prompt.contains("runs no workers"));
        let chat_folder = flow.manager.session_dir(&chat);
        assert_eq!(
            env(&chat_spec, LEDGER_ENV),
            Some(&*chat_folder.record().join("ledger.md").display().to_string())
        );
        flow.stop().await;
    }
}

/// The ledger shows in Sources once the session wrote one, and opens from there by its full
/// path; nothing else outside the checkout does.
#[tokio::test]
async fn the_ledger_shows_once_written_and_opens_by_its_path() {
    let flow = Flow::start(
        "standing-ledger",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await;
    flow.say("Hello").await;
    flow.settled().await;
    let id = flow.conversation.clone();
    assert_eq!(flow.manager.thread_view(&id).unwrap().ledger, None);
    let ledger = flow.manager.session_dir(&id).record().join("ledger.md");
    std::fs::write(
        &ledger,
        "- Rule from the user: \"tabs never\" (message 1, project)\n",
    )
    .unwrap();
    let shown = flow
        .manager
        .thread_view(&id)
        .unwrap()
        .ledger
        .expect("listed");
    assert_eq!(shown, ledger.display().to_string());
    let file = flow.manager.read_file(&id, shown.clone()).await.unwrap();
    assert_eq!(file.path, shown);
    assert!(file.text.unwrap().contains("tabs never"));
    let record = flow.manager.session_dir(&id).record();
    let out = record
        .join("..")
        .join("tmp")
        .join("x")
        .display()
        .to_string();
    assert!(flow.manager.read_file(&id, out).await.is_err());
    flow.stop().await;
}
