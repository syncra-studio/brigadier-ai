//! D58: no Brigadier skill reaches any session, Chat or worker on any CLI; the orchestrator's
//! must-follow rules are its standing instructions, the same text on every CLI (D1), and the
//! how-to is `brigadier help <topic>`.

use brigadier_providers::{SessionSpec, ToolSet};

use super::auto_merge_tests::script;
use super::chat_thread_tests::new_chat;
use super::{Flow, Options, Reply};
use crate::manager::prompts::{self, WORKERS_LINE};

/// A `SKILL.md` of Brigadier's own under one of the spec's skill roots.
fn has_brigadier_skill(spec: &SessionSpec) -> bool {
    spec.add_dirs
        .iter()
        .map(|root| root.join(".claude/skills"))
        .chain(spec.skill_plugins.iter().map(|root| root.join("skills")))
        .chain(
            spec.skill_plugins
                .iter()
                .map(|root| root.join(".claude/skills")),
        )
        .any(|root| {
            ["brigade", "fusion"]
                .iter()
                .any(|name| root.join(name).join("SKILL.md").exists())
        })
}

#[tokio::test]
async fn every_cli_gets_the_same_rules_and_no_skill() {
    let mut blocks = Vec::new();
    for provider in brigadier_providers::adapters::kinds() {
        let flow = Flow::start(
            "standing-every-cli",
            Options {
                thread: provider,
                ..Options::default()
            },
            script(|turn| async move {
                if turn.is_orchestrator() && turn.input.contains("Add the file.") {
                    turn.start_worker("Add the file", "Create file.txt.").await;
                    return Reply::text("Started a worker.");
                }
                Reply::text("Done.")
            }),
        )
        .await;
        flow.say("Add the file.").await;
        flow.settled().await;
        let chat = new_chat(&flow).await;
        flow.say_to(&chat, "Which nozzle?").await;
        flow.settled_in(&chat).await;
        let settings = flow.manager.core.settings();
        let specs: Vec<SessionSpec> = flow
            .specs()
            .into_iter()
            .filter(|(of, _)| *of == provider)
            .map(|(_, spec)| spec)
            .collect();
        let prompt = |spec: &SessionSpec| spec.append_system_prompt.clone().unwrap_or_default();
        let thread = specs
            .iter()
            .find(|spec| {
                spec.tools == ToolSet::Thread && spec.cwd != flow.manager.session_dir(&chat).work()
            })
            .expect("the session started");
        // The session's text is the one template every CLI gets, with the orchestrator's rules.
        assert_eq!(
            prompt(thread),
            prompts::standing_text(&prompts::session_instructions(&settings, provider)),
            "{provider:?}"
        );
        assert!(prompt(thread).contains(WORKERS_LINE), "{provider:?}");
        // The Chat runs on the default Chat CLI.
        let chat_spec = flow
            .specs()
            .into_iter()
            .map(|(_, spec)| spec)
            .find(|spec| spec.cwd == flow.manager.session_dir(&chat).work())
            .expect("the Chat started");
        assert!(!has_brigadier_skill(&chat_spec), "{provider:?}");
        assert!(prompt(&chat_spec).contains(WORKERS_LINE), "{provider:?}");
        // A worker gets the shared base, without the orchestrator's rules.
        let worker = specs
            .iter()
            .find(|spec| spec.tools == ToolSet::Lean)
            .expect("a worker started");
        let base = prompts::standing_text(&prompts::worker_instructions(&settings, provider));
        assert_eq!(prompt(worker), base, "{provider:?}");
        assert!(!prompt(worker).contains(WORKERS_LINE), "{provider:?}");
        // No skill of Brigadier's is installed for any of them, nor written anywhere.
        for spec in &specs {
            assert!(!has_brigadier_skill(spec), "{provider:?}: {spec:#?}");
        }
        assert!(
            !flow.manager.data_dir.join("brigade").exists(),
            "{provider:?}"
        );
        blocks.push(WORKERS_LINE.to_owned());
        flow.stop().await;
    }
    assert!(blocks.len() >= 3, "every registered CLI ran");
}
