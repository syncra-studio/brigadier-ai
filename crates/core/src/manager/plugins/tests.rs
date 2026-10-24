use std::path::{Path, PathBuf};

use brigadier_registry::Roots;

use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brigadier-plugins-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn item<'a>(view: &'a PluginsView, key: &str) -> &'a PluginItemView {
    view.items
        .iter()
        .find(|item| item.key == key)
        .unwrap_or_else(|| {
            panic!(
                "no {key} in {:?}",
                view.items.iter().map(|i| &i.key).collect::<Vec<_>>()
            )
        })
}

fn states(item: &PluginItemView) -> Vec<AgentItemState> {
    item.agents
        .iter()
        .map(|agent| agent.state.clone())
        .collect()
}

#[test]
fn the_page_shows_where_each_item_is_on_for_each_agent() {
    let home = home("view");
    let a = home.join("a");
    let b = home.join("b");
    std::fs::create_dir_all(&b).unwrap();
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{"mcpServers": {{"pencil": {{"command": "pencil-mcp"}}}},
                "projects": {{"{}": {{"mcpServers": {{"postgres": {{"command": "pg-mcp"}}}}}}}}}}"#,
            a.display()
        ),
    );
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.pencil]\ncommand = \"pencil-mcp\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            a.display()
        ),
    );
    write(
        &a.join(".codex/config.toml"),
        "[mcp_servers.pencil]\nenabled = false\n",
    );
    write(
        &home.join(".claude/skills/grill/SKILL.md"),
        "---\nname: grill\ndescription: Asks hard questions.\n---\n",
    );
    write(
        &home.join("Library/Application Support/Claude/claude_desktop_config.json"),
        r#"{"mcpServers": {"figma": {"command": "figma-mcp"}}}"#,
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let user = Catalog::read(&roots, None);
    let projects: Vec<ProjectCatalog> = [("a", &a), ("b", &b)]
        .into_iter()
        .map(|(name, path)| ProjectCatalog {
            id: name.to_owned(),
            name: name.to_owned(),
            path: path.clone(),
            catalog: Catalog::read(&roots, Some(path)),
        })
        .collect();
    let every = build(&user, &projects, &[], &Unknown, None);
    assert_eq!(
        every.agents.iter().map(|a| a.agent).collect::<Vec<_>>(),
        [ProviderKind::Claude, ProviderKind::Codex]
    );
    let pencil = item(&every, "server:pencil");
    assert!(pencil.everywhere && pencil.attention.is_none());
    assert_eq!(
        states(pencil),
        [AgentItemState::Ready, AgentItemState::Ready]
    );
    // Claude Code has no per-project off of its own servers: the Library first.
    assert!(!pencil.switchable);
    let postgres = item(&every, "server:postgres");
    assert!(!postgres.everywhere);
    assert_eq!(postgres.on_in, [a.display().to_string()]);
    assert_eq!(
        postgres.attention,
        Some(PluginAttention::NotEverywhere {
            message: "Only Claude Code has it.".to_owned()
        })
    );
    let grill = item(&every, "skill:grill");
    assert_eq!(grill.kind, PluginKind::Skill);
    assert_eq!(grill.description, "Asks hard questions.");
    assert_eq!(
        states(grill),
        [AgentItemState::Ready, AgentItemState::Missing]
    );
    if cfg!(target_os = "macos") {
        assert_eq!(every.elsewhere.len(), 1);
        assert_eq!(every.elsewhere[0].name, "figma");
        assert_eq!(
            every.elsewhere[0].found_in.as_deref(),
            Some("Claude Desktop")
        );
    }
    // In project a: Codex switches pencil off there by its own file.
    let in_a = build(&user, &projects, &[], &Unknown, Some(&a));
    assert_eq!(
        states(item(&in_a, "server:pencil")),
        [AgentItemState::Ready, AgentItemState::Off]
    );
    assert_eq!(
        states(item(&in_a, "server:postgres")),
        [AgentItemState::Ready, AgentItemState::Missing]
    );
    // b isn't trusted in Codex: its line says so.
    let b_view = &every.projects[1];
    assert!(every.projects[0].notes.is_empty());
    assert_eq!(b_view.notes.len(), 1);
    assert_eq!(b_view.notes[0].agent, ProviderKind::Codex);
    assert!(
        b_view.notes[0]
            .message
            .contains("once you trust the project")
    );
    // Each agent's own list: Codex needs a sign-in to pencil; Claude can't start postgres.
    let lists = Lists(vec![
        (
            Agent::Codex,
            None,
            vec![brigadier_registry::adapter::Listed {
                name: "pencil".into(),
                needs_sign_in: true,
                failed: None,
            }],
        ),
        (
            Agent::Claude,
            Some(a.clone()),
            vec![brigadier_registry::adapter::Listed {
                name: "postgres".into(),
                needs_sign_in: false,
                failed: Some("Failed to connect — ENOENT".into()),
            }],
        ),
    ]);
    let every = build(&user, &projects, &[], &lists, None);
    let pencil = item(&every, "server:pencil");
    assert_eq!(
        states(pencil),
        [AgentItemState::Ready, AgentItemState::NeedsSignIn]
    );
    assert_eq!(
        pencil.attention,
        Some(PluginAttention::SignIn {
            message: "Codex needs you to sign in.".to_owned()
        })
    );
    let in_a = build(&user, &projects, &[], &lists, Some(&a));
    let postgres = item(&in_a, "server:postgres");
    assert_eq!(
        states(postgres)[0],
        AgentItemState::WontStart {
            reason: "Failed to connect — ENOENT".to_owned()
        }
    );
    assert_eq!(
        postgres.attention,
        Some(PluginAttention::WontStart {
            message: "Won't start: Failed to connect — ENOENT".to_owned()
        })
    );
    let _ = std::fs::remove_dir_all(home);
}
