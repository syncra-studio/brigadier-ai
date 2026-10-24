//! The `/brigade` skill (S15): every session and Chat, on both CLIs, starts with it installed;
//! the user's copy overrides the shipped one until Settings resets it.

use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};

use super::chat_thread_tests::new_chat;
use super::{Flow, Options, Reply};
use crate::manager::brigade::{FUSION, SKILL};

async fn flow(name: &str, thread: ProviderKind) -> Flow {
    Flow::start(
        name,
        Options {
            thread,
            ..Options::default()
        },
        std::sync::Arc::new(|_| Box::pin(async { Reply::text("ok") })),
    )
    .await
}

/// The skill text a spec's installed plugin holds.
fn installed(spec: &SessionSpec) -> String {
    assert_eq!(
        spec.installed_skills.len(),
        1,
        "{:?}",
        spec.installed_skills
    );
    let plugin = &spec.installed_skills[0];
    assert!(plugin.join(".claude-plugin/plugin.json").is_file());
    // `/fusion` ships beside it, in the same plugin (S36).
    assert_eq!(
        std::fs::read_to_string(plugin.join("skills/fusion/SKILL.md")).unwrap(),
        FUSION
    );
    std::fs::read_to_string(plugin.join("skills/brigade/SKILL.md")).unwrap()
}

#[tokio::test]
async fn every_session_and_chat_on_both_clis_starts_with_the_skill() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = flow("brigade-installed", provider).await;
        flow.say("Is the build green?").await;
        flow.settled().await;
        let chat = new_chat(&flow).await;
        flow.say_to(&chat, "Which nozzle?").await;
        flow.settled_in(&chat).await;
        let specs = flow.specs();
        let thread = specs
            .iter()
            .find(|(of, spec)| *of == provider && spec.tools == ToolSet::Thread)
            .expect("the session started");
        assert_eq!(installed(&thread.1), SKILL, "{provider:?}");
        // Brigadier's other skills stay for workers only.
        assert!(thread.1.skill_plugins.is_empty());
        let chat_dir = flow.manager.session_dir(&chat).work();
        let chat = specs
            .iter()
            .find(|(_, spec)| spec.cwd == chat_dir)
            .expect("the Chat started");
        assert_eq!(installed(&chat.1), SKILL);
        // The instructions point at it; how workers run is its to say.
        let prompt = thread.1.append_system_prompt.as_deref().unwrap();
        assert!(prompt.contains(
            "You may run workers of your own through `brigadier start` when the job calls for them; the brigade skill says how."
        ));
        flow.stop().await;
    }
}

#[tokio::test]
async fn the_users_copy_overrides_the_shipped_skill_until_reset() {
    let flow = flow("brigade-override", ProviderKind::Claude).await;
    assert!(!flow.manager.brigade_skill().custom);
    let path = flow.manager.brigade_override_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        "---\nname: brigade\ndescription: mine\n---\nMy way.\n",
    )
    .unwrap();
    assert!(flow.manager.brigade_skill().custom);
    flow.say("Is the build green?").await;
    flow.settled().await;
    let spec = flow.thread_specs().pop().unwrap().1;
    assert!(installed(&spec).ends_with("My way.\n"));
    let reset = flow.manager.reset_brigade_skill().unwrap();
    assert!(!reset.custom);
    assert!(!path.exists());
    // The installed copy is the shipped one again at once.
    assert_eq!(installed(&spec), SKILL);
    flow.stop().await;
}

#[tokio::test]
async fn create_writes_the_shipped_skill_as_the_users_copy_and_keeps_one_there() {
    let flow = flow("brigade-create", ProviderKind::Claude).await;
    let path = flow.manager.brigade_override_path();
    let created = flow.manager.create_brigade_skill().unwrap();
    assert!(created.custom);
    assert_eq!(created.path, path);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), SKILL);
    // A copy already there is the user's: Create keeps it.
    std::fs::write(
        &path,
        "---\nname: brigade\ndescription: mine\n---\nMy way.\n",
    )
    .unwrap();
    assert!(flow.manager.create_brigade_skill().unwrap().custom);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .ends_with("My way.\n")
    );
    flow.stop().await;
}

#[tokio::test]
async fn optional_retro_is_off_until_selected_then_reaches_a_new_session_with_its_dependency() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = flow("20261022-0805-take-over-5-sessions-w189-retro", provider).await;
        let catalog = flow.manager.integrations_view(None).await.unwrap();
        let optional: Vec<_> = catalog
            .skills
            .iter()
            .filter(|skill| skill.id.starts_with("brigadier:optional:"))
            .collect();
        assert_eq!(optional.len(), 2);
        assert!(optional.iter().all(|skill| skill.off));
        flow.say("Hello").await;
        flow.settled().await;
        let spec = flow.thread_specs().pop().unwrap().1;
        assert_eq!(
            spec.installed_skills.len(),
            1,
            "only brigade is on by default"
        );
        let mut settings = flow.core.settings();
        settings
            .skills_off
            .retain(|id| id != "brigadier:optional:retro");
        flow.core.update_settings(settings).await.unwrap();
        let catalog = flow.manager.integrations_view(None).await.unwrap();
        assert!(
            !catalog
                .skills
                .iter()
                .find(|skill| skill.name == "retro")
                .unwrap()
                .off
        );
        // A new session gets the selected skills; an already-running session is unchanged.
        let chat = flow
            .manager
            .create_conversation(
                crate::model::ConversationKind::Chat,
                None,
                Some("Retro".into()),
                Some(crate::model::SetupRequest::Chat {
                    model: crate::model::ModelChoice {
                        provider,
                        model: None,
                        effort: None,
                        fast: None,
                        account: None,
                    },
                    permission: crate::model::PermissionLevel::default(),
                }),
            )
            .await
            .unwrap()
            .id;
        flow.say_to(&chat, "/retro").await;
        flow.settled_in(&chat).await;
        let spec = flow
            .specs()
            .into_iter()
            .find(|(_, spec)| spec.cwd == flow.manager.session_dir(&chat).work())
            .unwrap()
            .1;
        assert_eq!(spec.installed_skills.len(), 1);
        assert_eq!(spec.skill_plugins.len(), 2);
        for name in ["retro", "writing-for-agents"] {
            let plugin = spec
                .skill_plugins
                .iter()
                .find(|plugin| plugin.join(format!("skills/{name}/SKILL.md")).is_file())
                .unwrap();
            assert!(plugin.starts_with(&flow.manager.data_dir));
            let text =
                std::fs::read_to_string(plugin.join(format!("skills/{name}/SKILL.md"))).unwrap();
            assert!(text.starts_with(&format!("---\nname: {name}\n")));
            assert!(plugin.join(format!("skills/{name}/LICENSE")).is_file());
            if name == "retro" {
                assert!(text.contains("disable-model-invocation: true"));
                assert!(text.contains("Call the Skill tool with `writing-for-agents`"));
            } else {
                assert!(
                    plugin
                        .join("skills/writing-for-agents/SKILL-MECHANICS.md")
                        .is_file()
                );
            }
        }
        flow.stop().await;
    }
}

#[tokio::test]
async fn a_worker_gets_the_optional_skills_selected_for_its_session() {
    let flow = Flow::start(
        "20261022-0805-take-over-5-sessions-w189-retro-worker",
        Options::default(),
        std::sync::Arc::new(|turn| {
            Box::pin(async move {
                if turn.tools == ToolSet::Thread {
                    turn.start_worker("Read", "Read the session record.").await;
                }
                Reply::text("ok")
            })
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings
        .skills_off
        .retain(|id| id != "brigadier:optional:retro");
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Start a worker.").await;
    flow.settled().await;
    let workers: Vec<_> = flow
        .specs()
        .into_iter()
        .filter(|(_, spec)| spec.tools == ToolSet::Lean)
        .collect();
    assert!(!workers.is_empty());
    for (_, spec) in workers {
        assert!(spec.installed_skills.is_empty());
        assert_eq!(spec.skill_plugins.len(), 2);
        for name in ["retro", "writing-for-agents"] {
            assert!(
                spec.skill_plugins
                    .iter()
                    .any(|plugin| plugin.join(format!("skills/{name}/SKILL.md")).is_file())
            );
        }
    }
    flow.stop().await;
}
