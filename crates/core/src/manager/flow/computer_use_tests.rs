//! STRIP-DOWN-PLAN S35 (#529): while Settings › Computer use is on, every session, Chat and
//! worker on both CLIs gets the vendored driver's own `computer` server and its skill, as a
//! server entry of its own; a lean worker only when the thread asks (`--with computer-use`).

use std::sync::Arc;

use brigadier_cli::protocol::{
    Agent, Capability, CliRequest, GRANT_ENV, SESSION_ENV, StartRequest,
};
use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};
use serde_json::json;

use super::cli_tests::{Answers, brigadier, id_of, start, wait_for};
use super::computer_host_tests::bundle_helper;
use super::{Flow, Options, Reply, Script};
use crate::manager::computer_use::SERVER_NAME;
use crate::model::{ConversationKind, ModelChoice, PermissionLevel, SetupRequest};
use crate::tools::Role;

/// Whether `spec` has the computer tools: the driver's server, its skill and its token
/// redacted. Without them it has neither.
fn has_computer(spec: &SessionSpec) -> bool {
    let server = spec
        .mcp_servers
        .iter()
        .find(|server| server.name == SERVER_NAME);
    let skill = spec
        .skill_plugins
        .iter()
        .any(|plugin| plugin.join("skills/computer-use/SKILL.md").is_file());
    let Some(server) = server else {
        assert!(spec.mcp_servers.is_empty(), "{:?}", spec.mcp_servers);
        assert!(!skill, "{:?}", spec.skill_plugins);
        return false;
    };
    assert!(
        skill,
        "the skill comes with the server: {:?}",
        spec.skill_plugins
    );
    assert_eq!(spec.mcp_servers.len(), 1, "{:?}", spec.mcp_servers);
    assert!(server.trusted);
    assert_eq!(server.tool_timeout_secs, Some(120));
    // The server's own instructions stay one line: its agents have the skill.
    let value = |name: &str| {
        server
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    assert_eq!(value("BRIGADIER_CU_AUDIENCE").as_deref(), Some("brigadier"));
    let token = value("BRIGADIER_CU_SOCKET_AUTH_TOKEN").unwrap();
    let redactor = spec.redactor.as_ref().expect("a redactor");
    assert!(!redactor.redact(&token).contains(&token));
    true
}

async fn chat_on(flow: &Flow, provider: ProviderKind) -> crate::ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Chat".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission: PermissionLevel::default(),
            }),
        )
        .await
        .unwrap()
        .id
}

async fn flow(name: &str, thread: ProviderKind, script: Script, on: bool) -> Flow {
    let flow = Flow::start(
        name,
        Options {
            thread,
            ..Options::default()
        },
        script,
    )
    .await;
    bundle_helper(&flow);
    if !on {
        let mut settings = flow.core.settings();
        settings.computer_use = false;
        flow.core.update_settings(settings).await.unwrap();
    }
    flow
}

#[tokio::test]
async fn a_session_and_a_chat_on_both_clis_have_the_computer_tools_while_it_is_on() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        for on in [true, false] {
            let flow = flow(
                &format!("cu-sessions-{provider:?}-{on}"),
                provider,
                Arc::new(|_| Box::pin(async { Reply::text("ok") })),
                on,
            )
            .await;
            flow.say("Is the build green?").await;
            flow.settled().await;
            let chat = chat_on(&flow, provider).await;
            flow.say_to(&chat, "Which nozzle?").await;
            flow.settled_in(&chat).await;
            let chat_dir = flow.manager.session_dir(&chat).work();
            let specs = flow.specs();
            let session = specs
                .iter()
                .find(|(of, spec)| {
                    *of == provider && spec.tools == ToolSet::Thread && spec.cwd != chat_dir
                })
                .expect("the session started");
            let chat = specs
                .iter()
                .find(|(of, spec)| *of == provider && spec.cwd == chat_dir)
                .expect("the Chat started");
            assert_eq!(has_computer(&session.1), on, "{provider:?} session");
            assert_eq!(has_computer(&chat.1), on, "{provider:?} Chat");
            flow.stop().await;
        }
    }
}

/// The session starts an ordinary worker, a lean one and a lean one with `--with
/// computer-use`, all on `agent`.
fn starting(answers: Answers, agent: Agent) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if let Some(Role::Worker { .. }) = worker {
                return Reply::text("Done.");
            }
            let mut ids = Vec::new();
            for (name, lean, with) in [
                ("ordinary", false, Vec::new()),
                ("lean", true, Vec::new()),
                ("lean with", true, vec![Capability::ComputerUse]),
            ] {
                let started = brigadier(
                    &turn,
                    CliRequest::Start(Box::new(StartRequest {
                        agent: Some(agent),
                        lean,
                        with,
                        ..start("Look at the app.")
                    })),
                )
                .await;
                ids.push(id_of(&started));
                answers.lock().unwrap().insert(name.into(), started);
            }
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            let events = wait_for(&turn, &ids, 3).await;
            answers
                .lock()
                .unwrap()
                .insert("events".into(), json!(events));
            Reply::text("Started three.")
        })
    })
}

#[tokio::test]
async fn a_worker_has_them_and_a_lean_one_only_when_asked_on_both_clis() {
    for (provider, agent) in [
        (ProviderKind::Claude, Agent::Claude),
        (ProviderKind::Codex, Agent::Codex),
    ] {
        for on in [true, false] {
            let answers = Answers::default();
            let flow = flow(
                &format!("cu-workers-{provider:?}-{on}"),
                provider,
                starting(answers.clone(), agent),
                on,
            )
            .await;
            flow.say("Start them.").await;
            flow.until("it started them", |_| {
                answers.lock().unwrap().contains_key("events")
            })
            .await;
            flow.settled().await;
            let specs = flow.specs();
            let board = flow.board().await;
            // Started in order: the ordinary worker, the lean one, the lean one that asked.
            let spec_of = |number: u32| {
                let task = Flow::task(&board, number);
                specs
                    .iter()
                    .find(|(_, spec)| {
                        spec.env
                            .iter()
                            .any(|(key, value)| key == SESSION_ENV && *value == task.id.0)
                    })
                    .map(|(of, spec)| {
                        assert_eq!(*of, provider, "task-{number}");
                        spec.clone()
                    })
                    .unwrap_or_else(|| panic!("task-{number} started"))
            };
            let (ordinary, lean, lean_with) = (spec_of(1), spec_of(2), spec_of(3));
            assert!(lean.lean_worker && lean_with.lean_worker);
            assert_eq!(has_computer(&ordinary), on, "{provider:?} ordinary");
            assert!(!has_computer(&lean), "{provider:?} lean");
            assert_eq!(has_computer(&lean_with), on, "{provider:?} lean with");
            // Asked for, its tools are in the model's list from the start.
            if on {
                assert!(!ordinary.mcp_servers[0].always_load);
                assert!(lean_with.mcp_servers[0].always_load);
            }
            flow.stop().await;
        }
    }
}
