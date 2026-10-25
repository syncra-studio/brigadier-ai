use std::path::{Path, PathBuf};

use brigadier_registry::{Catalog, Roots};

use super::model::{ProjectCatalog, provider_app_command};
use super::*;

/// The page for `user` and `projects`, nothing shared yet.
fn page(user: &Catalog, projects: &[ProjectCatalog], health: &dyn Health) -> PluginsView {
    let model = model::model(user, projects, &[], &Sharing::default(), Path::new("/home"));
    build(&model, projects, &Sharing::default(), health, &[], None).view
}

/// The CLI's Library rows' agent states for `user` and `projects`, where `focus` looks.
#[allow(dead_code)]
fn only_in(item: &PluginItemView) -> Option<(Vec<ProviderKind>, OnlyInReason)> {
    match &item.problem {
        Some(PluginProblem::OnlyIn { agents, reason, .. }) => Some((agents.clone(), *reason)),
        _ => None,
    }
}

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

#[test]
fn synced_only_skills_stay_hidden_whether_user_made_or_anthropic() {
    let home = home("synced-provenance");
    let account = home.join(".claude/skills/synced/org_account");
    for name in ["builtin", "personal"] {
        write(
            &account.join(format!("{name}/SKILL.md")),
            &format!("---\nname: {name}\n---\n"),
        );
    }
    write(
        &account.join("manifest.json"),
        r#"{"skills":[
        {"name":"builtin","creatorType":"anthropic"},
        {"name":"personal","creatorType":"user"}]}"#,
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let catalog = Catalog::read(&roots, None);
    assert_eq!(catalog.skills.len(), 2);
    let view = page(&catalog, &[], &Unknown);
    assert!(view.items.is_empty());
    assert!(view.elsewhere.is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn codex_provider_plugins_hide_skills_servers_and_problems_even_when_off() {
    let home = home("provider-plugins");
    let project = home.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let mut config = format!(
        "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        project.display()
    );
    for (marketplace, name, enabled) in [
        ("openai-bundled", "builtin", true),
        ("openai-primary-runtime", "runtime", true),
        ("openai-bundled", "builtin-off", false),
        ("openai-primary-runtime", "runtime-off", false),
        ("openai-curated", "curated", true),
        ("openai-curated-remote", "curated-remote", true),
        ("openai-curated", "curated-off", false),
    ] {
        let plugin = home.join(format!(".codex/plugins/cache/{marketplace}/{name}/1.0.0"));
        write(
            &plugin.join(".codex-plugin/plugin.json"),
            &format!(r#"{{"name":"{name}"}}"#),
        );
        write(
            &plugin.join(format!("skills/{name}/SKILL.md")),
            &format!("---\nname: {name}\n---\n"),
        );
        write(
            &plugin.join(".mcp.json"),
            &serde_json::json!({
                format!("{name}-server"): {"command":"user-mcp"},
                format!("{name}-broken"): {"unexpected": true}
            })
            .to_string(),
        );
        config.push_str(&format!(
            "\n[plugins.\"{name}@{marketplace}\"]\nenabled = {enabled}\n"
        ));
    }
    write(&home.join(".codex/config.toml"), &config);
    // Claude's official marketplace is user-installed, unlike Codex's bundled ones.
    let official = home.join(".claude/plugins/cache/claude-plugins-official/official/1.0.0");
    write(
        &official.join(".claude-plugin/plugin.json"),
        r#"{"name":"official"}"#,
    );
    write(
        &official.join("skills/official/SKILL.md"),
        "---\nname: official\n---\n",
    );
    write(&home.join(".claude/plugins/installed_plugins.json"), &serde_json::json!({
        "plugins": {"official@claude-plugins-official":[{"scope":"user","installPath":official}]}
    }).to_string());
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let catalog = Catalog::read(&roots, None);
    assert!(catalog.skills.iter().any(|skill| skill.name == "builtin"));
    assert!(
        catalog
            .off
            .skills
            .iter()
            .any(|skill| skill.name == "builtin-off")
    );
    let projects = [ProjectCatalog {
        id: "p".into(),
        name: "Project".into(),
        path: project.clone(),
        catalog: Catalog::read(&roots, Some(&project)),
    }];
    let view = page(&catalog, &projects, &Unknown);
    let keys: Vec<_> = view.items.iter().map(|item| item.key.as_str()).collect();
    // What won't start first, then A–Z.
    assert_eq!(
        keys,
        [
            "server:curated-broken",
            "server:curated-remote-broken",
            "plugin:curated",
            "plugin:curated-off",
            "plugin:curated-off/curated-off-server",
            "plugin:curated-remote",
            "plugin:curated-remote/curated-remote-server",
            "plugin:curated/curated-server",
            "plugin:official",
        ]
    );
    assert_eq!(item(&view, "plugin:curated").skill_count, 1);
    assert!(view.elsewhere.is_empty());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn provider_app_servers_are_hidden_including_off_elsewhere_and_attention() {
    let home = home("provider-app-servers");
    write(
        &home.join(".codex/config.toml"),
        r#"
        [mcp_servers.node_repl]
        command = "/Applications/ChatGPT.app/Contents/Resources/cua_node/bin/node_repl"
        [mcp_servers.provider_off]
        command = "/Users/example/Applications/Claude.app/Contents/Resources/mcp"
        enabled = false
        [mcp_servers.user_server]
        command = "/usr/local/bin/node_repl"
    "#,
    );
    write(
        &home.join(".cursor/mcp.json"),
        r#"{"mcpServers": {
        "provider_elsewhere": {"command":"/Applications/Claude.app/Contents/Resources/mcp"},
        "provider_broken": {"command":"/Applications/ChatGPT.app/Contents/Resources/mcp", "type":"unknown"},
        "user_elsewhere": {"command":"user-mcp"}
    }}"#,
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let catalog = Catalog::read(&roots, None);
    assert!(
        catalog
            .servers
            .iter()
            .any(|server| server.name == "node_repl")
    );
    assert!(
        catalog
            .off
            .servers
            .iter()
            .any(|server| server.name == "provider_off")
    );
    assert!(
        catalog
            .problems
            .iter()
            .any(|problem| problem.name == "provider_broken")
    );
    let view = page(&catalog, &[], &Unknown);
    assert_eq!(view.items.len(), 1);
    assert_eq!(view.items[0].key, "server:user_server");
    assert_eq!(view.elsewhere.len(), 1);
    assert_eq!(view.elsewhere[0].name, "user_elsewhere");
    for command in [
        "/Applications/ChatGPT.app.backup/Contents/mcp",
        "/Applications/ChatGPT.app/ContentsElsewhere/mcp",
        "/Applications/ChatGPT.app/Contents/../../../user-mcp",
        "/tmp/ChatGPT.app/Contents/mcp",
        "ChatGPT.app/Contents/mcp",
        "node_repl",
    ] {
        assert!(!provider_app_command(command), "{command}");
    }
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn the_page_shows_scope_lock_problems_and_order() {
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
            "[mcp_servers.pencil]\ncommand = \"pencil-mcp\"\n\n\
             [mcp_servers.docs]\nurl = \"https://mcp.docs.example/mcp\"\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            a.display()
        ),
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
    let view = page(&user, &projects, &Unknown);
    // Both agents have pencil: on everywhere, nothing to share, locked by Claude's own entry.
    let pencil = item(&view, "server:pencil");
    assert!(pencil.on && pencil.problem.is_none());
    assert_eq!(pencil.scope, PluginScope::AllProjects);
    assert_eq!(pencil.removal, PluginRemoval::None);
    assert!(
        pencil
            .locked
            .as_deref()
            .unwrap()
            .starts_with("Claude Code keeps it on")
    );
    // Claude's own server in project a only: some projects, locked.
    let postgres = item(&view, "server:postgres");
    assert_eq!(
        postgres.scope,
        PluginScope::SomeProjects {
            project_ids: vec!["a".to_owned()],
            folders: Vec::new()
        }
    );
    assert!(postgres.locked.is_some());
    // A skill in ~/.claude/skills: locked, its description the subtitle.
    let grill = item(&view, "skill:grill");
    assert_eq!(grill.kind, PluginKind::Skill);
    assert_eq!(grill.description, "Asks hard questions.");
    assert_eq!(grill.skill_count, 1);
    assert!(
        grill
            .locked
            .as_deref()
            .unwrap()
            .contains("loads every skill in")
    );
    // Codex's own remote server: switchable, its address for the icon.
    let docs = item(&view, "server:docs");
    assert!(docs.locked.is_none());
    assert_eq!(docs.url.as_deref(), Some("https://mcp.docs.example/mcp"));
    assert_eq!(docs.icon, None);
    if cfg!(target_os = "macos") {
        assert_eq!(view.elsewhere.len(), 1);
        assert_eq!(view.elsewhere[0].name, "figma");
        assert_eq!(view.elsewhere[0].found_in, "Claude Desktop");
    }
    // b isn't trusted in Codex: its line says so.
    assert!(view.projects[0].notes.is_empty());
    assert_eq!(view.projects[1].notes.len(), 1);
    assert_eq!(view.projects[1].notes[0].agent, ProviderKind::Codex);
    // Each agent's own list: Codex needs a sign-in to pencil; Claude can't start postgres.
    let lists = Lists(vec![
        (
            Agent::Codex,
            None,
            vec![Listed {
                name: "pencil".into(),
                needs_sign_in: true,
                failed: None,
            }],
        ),
        (
            Agent::Claude,
            Some(a.clone()),
            vec![Listed {
                name: "postgres".into(),
                needs_sign_in: false,
                failed: Some("Failed to connect — ENOENT".into()),
            }],
        ),
    ]);
    let view = page(&user, &projects, &lists);
    assert_eq!(
        item(&view, "server:postgres").problem,
        Some(PluginProblem::WontStart {
            message: "Won't start: Failed to connect — ENOENT".to_owned()
        })
    );
    // Problems that need the user come first.
    assert_eq!(view.items[0].key, "server:postgres");
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_plugins_skills_are_one_item_with_their_count() {
    let home = home("plugin-row");
    let root = home.join(".claude/plugins/cache/acme/kit/1.0.0");
    write(
        &root.join(".claude-plugin/plugin.json"),
        r#"{"name": "kit", "description": "Images and video."}"#,
    );
    for skill in ["draw", "film"] {
        write(
            &root.join(format!("skills/{skill}/SKILL.md")),
            &format!("---\nname: {skill}\ndescription: {skill} things.\n---\n"),
        );
    }
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &serde_json::json!({"version": 2, "plugins": {"kit@acme": [
            {"scope": "user", "installPath": root, "version": "1.0.0"}
        ]}})
        .to_string(),
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"kit@acme": true}}"#,
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let view = page(&Catalog::read(&roots, None), &[], &Unknown);
    let kit = item(&view, "plugin:kit");
    assert_eq!(
        (kit.kind, kit.name.as_str(), kit.skill_count),
        (PluginKind::Skill, "kit", 2)
    );
    // It can't be shared: it came in Claude Code's own plugin.
    let Some(PluginProblem::OnlyIn {
        message, reason, ..
    }) = &kit.problem
    else {
        panic!("{:?}", kit.problem);
    };
    assert_eq!(*reason, OnlyInReason::OwnPlugin);
    assert_eq!(
        message,
        "Only in Claude Code: came in Claude Code's own plugin"
    );
    assert_eq!(kit.description, "Images and video.");
    assert!(
        !view
            .items
            .iter()
            .any(|item| item.key.starts_with("plugin:kit/"))
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn an_agents_own_item_switched_off_everywhere_stays_as_an_off_row() {
    let home = home("off-rows");
    let project = home.join("p");
    std::fs::create_dir_all(&project).unwrap();
    // Codex: its own server and another marketplace's plugin, both off for every project.
    let kit = home.join(".codex/plugins/cache/market/kit/1.0.0");
    write(&kit.join(".codex-plugin/plugin.json"), r#"{"name": "kit"}"#);
    write(
        &kit.join("skills/tidy/SKILL.md"),
        "---\nname: tidy\ndescription: Tidies.\n---\n",
    );
    write(
        &home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.pencil]\ncommand = \"pencil-mcp\"\nenabled = false\n\n\
             [plugins.\"kit@market\"]\nenabled = false\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display()
        ),
    );
    // Claude Code: another marketplace's plugin, installed and off for every project.
    let deck = home.join(".claude/plugins/cache/market/deck/1.0.0");
    write(
        &deck.join(".claude-plugin/plugin.json"),
        r#"{"name": "deck"}"#,
    );
    write(
        &deck.join("skills/slides/SKILL.md"),
        "---\nname: slides\ndescription: Slides.\n---\n",
    );
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &serde_json::json!({"version": 2, "plugins": {"deck@market": [
            {"scope": "user", "installPath": deck, "version": "1.0.0"}
        ]}})
        .to_string(),
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"deck@market": false}}"#,
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let user = Catalog::read(&roots, None);
    let projects = vec![ProjectCatalog {
        id: "p".to_owned(),
        name: "p".to_owned(),
        path: project.clone(),
        catalog: Catalog::read(&roots, Some(&project)),
    }];
    let view = page(&user, &projects, &Unknown);
    let pencil = item(&view, "server:pencil");
    assert!(!pencil.on && pencil.locked.is_none());
    assert_eq!(pencil.detail, "pencil-mcp");
    assert!(!item(&view, "plugin:kit").on);
    let deck = item(&view, "plugin:deck");
    assert!(!deck.on);
    assert_eq!(
        (deck.description.as_str(), deck.skill_count),
        ("Slides.", 1)
    );
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn a_claude_server_kept_to_a_folder_that_isnt_a_project_says_where_it_is_on() {
    let home = home("foreign-folder");
    let project = home.join("project");
    let foreign = home.join("clients/freelogo-fos-2026");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&foreign).unwrap();
    write(
        &home.join(".claude.json"),
        &serde_json::json!({"projects": {
            foreign.display().to_string(): {"mcpServers": {"codescene": {"command": "codescene-mcp"}}}
        }})
        .to_string(),
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let projects = [ProjectCatalog {
        id: "p".to_owned(),
        name: "project".to_owned(),
        catalog: Catalog::read(&roots, Some(&project)),
        path: project.clone(),
    }];
    let user = Catalog::read(&roots, None);
    let view = page(&user, &projects, &Unknown);
    let codescene = item(&view, "server:codescene");
    assert_eq!(
        codescene.scope,
        PluginScope::SomeProjects {
            project_ids: Vec::new(),
            folders: vec![foreign.display().to_string()]
        }
    );
    // Claude Code's own server: the switch and the scope can't change it.
    assert!(codescene.locked.is_some());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_foreign_folder_claude_server_locks_the_row_beside_a_codex_one() {
    let home = home("foreign-and-codex");
    let foreign = home.join("clients/elsewhere");
    std::fs::create_dir_all(&foreign).unwrap();
    write(
        &home.join(".claude.json"),
        &serde_json::json!({"projects": {
            foreign.display().to_string(): {"mcpServers": {"codescene": {"command": "codescene-mcp"}}}
        }})
        .to_string(),
    );
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.codescene]\ncommand = \"codescene-mcp\"\nenabled = false\n",
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let view = page(&Catalog::read(&roots, None), &[], &Unknown);
    // A Claude Code entry of its own locks the row, whatever Codex's can do.
    assert!(item(&view, "server:codescene").locked.is_some());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn a_sign_in_where_it_is_on_shows_on_a_row_looking_elsewhere() {
    let home = home("sign-in-elsewhere");
    let project = home.join("project");
    std::fs::create_dir_all(&project).unwrap();
    write(
        &home.join(".claude.json"),
        &serde_json::json!({"projects": {
            project.display().to_string(): {"mcpServers": {"linear": {"type": "http", "url": "https://mcp.linear.app/mcp"}}}
        }})
        .to_string(),
    );
    let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
    let projects = [ProjectCatalog {
        id: "p".to_owned(),
        name: "project".to_owned(),
        catalog: Catalog::read(&roots, Some(&project)),
        path: project.clone(),
    }];
    let health = Lists(vec![(
        Agent::Claude,
        Some(project.clone()),
        vec![Listed {
            name: "linear".to_owned(),
            needs_sign_in: true,
            failed: None,
        }],
    )]);
    // Looking at every project, where it's on only in the project.
    let view = page(&Catalog::read(&roots, None), &projects, &health);
    let linear = item(&view, "server:linear");
    let Some(PluginProblem::NeedsSignIn {
        message,
        server_id,
        project_id,
    }) = &linear.problem
    else {
        panic!("{:?}", linear.problem);
    };
    assert_eq!(message, "Claude Code needs you to sign in.");
    assert!(server_id.starts_with("claude:local:"));
    assert_eq!(project_id.as_deref(), Some("p"));
    std::fs::remove_dir_all(home).unwrap();
}
