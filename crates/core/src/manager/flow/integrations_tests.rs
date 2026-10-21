//! Shared MCP servers and skills (Phase 7): a worker keeps its agent's own servers and gets
//! the ones its task names from the other agents' configs, every switched-on skill its agent
//! doesn't find by itself, and nothing the user switched off; a thread goes without what was
//! switched off too.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::ProviderKind;
use brigadier_providers::model::{McpServer, McpTransport, SessionSpec};
use brigadier_registry::Roots;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str) {
    write(
        &dir.join(name).join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: The {name} skill.\n---\nDo it.\n"),
    );
}

/// A home beside the flow's repository: Claude has `claude-only` and a `dup`, Codex has
/// `codex-user` and another `dup`, the project has `project-codex` in its Codex layer (trusted
/// in Codex), and each agent has a skill of its own, the project one in `.agents/skills`.
/// The manager reads its catalog from it.
fn home(flow: &Flow) -> PathBuf {
    let home = flow.repo.parent().unwrap().join("home");
    write(
        &home.join(".claude.json"),
        &json!({"mcpServers": {
            "claude-only": {"command": "/bin/cat", "args": ["-u"],
                            "env": {"TOKEN": "claude-secret-1"}},
            "dup": {"command": "/bin/echo"},
        }})
        .to_string(),
    );
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.codex-user]\ncommand = \"/bin/cat\"\n\n[mcp_servers.dup]\ncommand = \"/usr/bin/true\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            flow.repo.display()
        ),
    );
    write(
        &flow.repo.join(".codex/config.toml"),
        "[mcp_servers.project-codex]\ncommand = \"/bin/cat\"\nenv = { WORD = \"project-secret-2\" }\n",
    );
    skill(&home.join(".agents/skills"), "agents-only");
    skill(&home.join(".claude/skills"), "claude-only-skill");
    skill(&flow.repo.join(".agents/skills"), "project-skill");
    flow.manager
        .set_registry(Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap());
    home
}

/// The thread delegates each task in `tasks` (provider, kind, integrations) on "Go."; each
/// worker reports at once.
fn delegating(tasks: Vec<(&'static str, &'static str, Vec<&'static str>)>) -> Script {
    let tasks = Arc::new(tasks);
    script(move |turn| {
        let tasks = tasks.clone();
        async move {
            if turn.is_orchestrator() {
                if !turn.input.contains("Go.") {
                    return Reply::text("Noted.");
                }
                for (n, (provider, kind, integrations)) in tasks.iter().enumerate() {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "low", "title": format!("Look around {n}"),
                                   "kind": kind, "spec": "Look around.", "provider": provider,
                                   "integrations": integrations}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                return Reply::text("[quiet]");
            }
            let reply = turn
                .call("submit_report", json!({"summary": "Looked around."}))
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }
    })
}

/// The workers' sessions, in the order they started.
fn workers(flow: &Flow) -> Vec<(ProviderKind, SessionSpec)> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| {
            spec.append_system_prompt.as_deref().is_some_and(|prompt| {
                !prompt.contains(crate::manager::prompts::THREAD_OPENING)
                    && prompt != brigadier_review::REVIEW_ROLE
            })
        })
        .collect()
}

/// The one worker of `provider`'s session (two workers may start in either order).
fn worker(flow: &Flow, provider: ProviderKind) -> SessionSpec {
    let mut of: Vec<SessionSpec> = workers(flow)
        .into_iter()
        .filter(|(kind, _)| *kind == provider)
        .map(|(_, spec)| spec)
        .collect();
    assert_eq!(of.len(), 1, "one {provider:?} worker");
    of.remove(0)
}

fn server<'a>(spec: &'a SessionSpec, name: &str) -> Option<&'a McpServer> {
    spec.mcp_servers.iter().find(|server| server.name == name)
}

/// The skills in a session's shared-skills plugin, by folder name.
fn shared_skills(spec: &SessionSpec) -> Vec<String> {
    let mut names: Vec<String> = spec
        .skill_plugins
        .iter()
        .filter(|plugin| plugin.to_string_lossy().contains("shared-skills"))
        .flat_map(|plugin| std::fs::read_dir(plugin.join("skills")).unwrap())
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn workers_get_the_servers_their_task_names_and_the_other_agents_skills() {
    let flow = Flow::start(
        "integrations-shared",
        Options::default(),
        delegating(vec![
            ("codex", "research", vec!["claude-only", "codex-user"]),
            ("claude", "research", vec!["project-codex"]),
        ]),
    )
    .await;
    home(&flow);
    flow.say("Go.").await;
    flow.settled().await;
    let (claude, codex) = (
        worker(&flow, ProviderKind::Claude),
        worker(&flow, ProviderKind::Codex),
    );
    let (claude, codex) = (&claude, &codex);

    // The Codex worker gets Claude's server, its value in its env; its own `codex-user` stays
    // its agent's, not added again.
    let given = server(codex, "claude-only").expect("the Claude-only server");
    assert_eq!(
        given.transport,
        McpTransport::Stdio {
            command: "/bin/cat".into(),
            args: vec!["-u".into()],
            cwd: None
        }
    );
    assert_eq!(
        given.env,
        [("TOKEN".to_owned(), "claude-secret-1".to_owned())]
    );
    assert!(!given.trusted);
    assert!(server(codex, "codex-user").is_none());
    // The Claude worker gets the project's Codex server.
    let given = server(claude, "project-codex").expect("the project's Codex server");
    assert_eq!(
        given.env,
        [("WORD".to_owned(), "project-secret-2".to_owned())]
    );
    assert!(server(claude, "claude-only").is_none());

    // Skills: each gets those only the other agent finds. The read-only Codex worker starts
    // outside the checkout, so the project's `.agents` skill is shared with it too.
    assert_eq!(shared_skills(claude), ["agents-only", "project-skill"]);
    assert_eq!(shared_skills(codex), ["claude-only-skill", "project-skill"]);
    let plugin = codex
        .skill_plugins
        .iter()
        .find(|plugin| plugin.to_string_lossy().contains("shared-skills"))
        .unwrap();
    let text = std::fs::read_to_string(plugin.join("skills/claude-only-skill/SKILL.md")).unwrap();
    assert!(
        text.contains("description: The claude-only-skill skill."),
        "{text}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_switched_off_server_is_given_to_no_thread_or_worker() {
    let flow = Flow::start(
        "integrations-off",
        Options::default(),
        delegating(vec![
            ("claude", "research", vec![]),
            ("codex", "research", vec![]),
        ]),
    )
    .await;
    home(&flow);
    let mut settings = flow.core.settings();
    settings.mcp_servers_off = vec!["claude:user:claude-only".into()];
    settings.skills_off = vec!["agents:user:agents-only".into()];
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Go.").await;
    flow.settled().await;

    // The Claude thread goes without its own server (and the skill isn't one it has).
    let threads = flow.thread_specs();
    let (ProviderKind::Claude, thread) = threads.last().unwrap() else {
        panic!("a Claude thread: {threads:?}");
    };
    assert_eq!(thread.switched_off.servers, ["claude-only"]);
    assert!(thread.switched_off.skill_names.is_empty());
    // So does the Claude worker; the Codex worker never had it, and its own skill is off.
    let (claude, codex) = (
        worker(&flow, ProviderKind::Claude),
        worker(&flow, ProviderKind::Codex),
    );
    let (claude, codex) = (&claude, &codex);
    assert_eq!(claude.switched_off.servers, ["claude-only"]);
    assert!(codex.switched_off.servers.is_empty());
    assert_eq!(
        codex.switched_off.skill_files,
        [flow
            .repo
            .parent()
            .unwrap()
            .join("home/.agents/skills/agents-only/SKILL.md")]
    );
    // Nor is the switched-off skill shared with the Claude worker.
    assert_eq!(shared_skills(claude), ["project-skill"]);
    flow.stop().await;
}

#[tokio::test]
async fn unknown_ambiguous_switched_off_and_reserved_names_are_refused() {
    let flow = Flow::start(
        "integrations-refused",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() || !turn.input.contains("Go.") {
                return Reply::text("Noted.");
            }
            for (name, error) in [
                ("nowhere", "unknown integration `nowhere`"),
                (
                    "dup",
                    "integration `dup` names several different MCP servers",
                ),
                ("codex-user", "is switched off for Brigadier"),
                ("brigadier", "the name of Brigadier's own server"),
            ] {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "low", "title": format!("Use {name}"),
                               "kind": "research", "spec": "Look around.",
                               "integrations": [name]}),
                    )
                    .await;
                assert!(reply.is_error, "{name}: {}", reply.text);
                assert!(reply.text.contains(error), "{name}: {}", reply.text);
            }
            // An id picks one of the two.
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "low", "title": "Use dup", "kind": "research",
                           "spec": "Look around.", "provider": "claude",
                           "integrations": ["codex:user:dup"]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("[quiet]")
        }),
    )
    .await;
    let home = home(&flow);
    // Brigadier's own server's name, in a config of the user's.
    write(
        &home.join(".cursor/mcp.json"),
        &json!({"mcpServers": {"brigadier": {"command": "/bin/cat"}}}).to_string(),
    );
    flow.manager
        .set_registry(Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap());
    let mut settings = flow.core.settings();
    settings.mcp_servers_off = vec!["codex:user:codex-user".into()];
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Go.").await;
    flow.until("the one task", |board| board.tasks.len() == 1)
        .await;
    super::eventually_async("the worker to start", || async {
        !workers(&flow).is_empty()
    })
    .await;
    let workers = workers(&flow);
    let (ProviderKind::Claude, claude) = &workers[0] else {
        panic!("a Claude worker: {workers:?}");
    };
    // Named by the Claude worker's own `dup`, so it gets a number.
    let given = server(claude, "dup-2").expect("Codex's dup");
    assert!(
        matches!(&given.transport, McpTransport::Stdio { command, .. } if command == Path::new("/usr/bin/true"))
    );
    flow.stop().await;
}
