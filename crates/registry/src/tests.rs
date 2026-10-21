use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A fresh folder for one test, its symlinks resolved (so paths compare with the catalog's).
fn fixture(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "brigadier-registry-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn skill(dir: &Path, name: &str, description: &str) {
    write(
        &dir.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: {description}\n---\n\nBody.\n"),
    );
}

#[cfg(unix)]
fn link(target: &Path, at: &Path) {
    std::fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, at).unwrap();
}

fn roots(home: &Path, extra: &[(&str, &str)]) -> Roots {
    let mut vars = vec![("HOME".to_owned(), home.display().to_string())];
    if cfg!(windows) {
        vars.push(("USERPROFILE".to_owned(), home.display().to_string()));
        vars.push((
            "APPDATA".to_owned(),
            home.join("Library/Application Support")
                .display()
                .to_string(),
        ));
    }
    vars.extend(
        extra
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
    );
    let mut roots = Roots::from_vars(vars).unwrap();
    // One place on every platform, for the fixtures.
    roots.desktop_config = Some(home.join("desktop/claude_desktop_config.json"));
    roots
}

/// A stdio server's command, args, env, cwd and base.
type Stdio<'a> = (
    &'a str,
    &'a [String],
    Vec<(&'a str, &'a str)>,
    Option<&'a Path>,
    Option<&'a Path>,
);

fn stdio(server: &Server) -> Stdio<'_> {
    match &server.transport {
        Transport::Stdio {
            command,
            args,
            env,
            cwd,
            base,
        } => (
            command,
            args,
            env.iter().map(|(k, v)| (k.as_str(), v.expose())).collect(),
            cwd.as_deref(),
            base.as_deref(),
        ),
        Transport::Http { .. } => panic!("{} isn't stdio", server.id),
    }
}

fn http(server: &Server) -> (&str, Vec<(&str, &str)>) {
    match &server.transport {
        Transport::Http { url, headers } => (
            url,
            headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.expose()))
                .collect(),
        ),
        Transport::Stdio { .. } => panic!("{} isn't http", server.id),
    }
}

fn ids(catalog: &Catalog) -> Vec<&str> {
    let mut ids: Vec<&str> = catalog.servers.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    ids
}

#[test]
fn claude_user_and_local_scopes_expand_variables() {
    let home = fixture("claude");
    let project = home.join("work/app");
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{
              "mcpServers": {{
                "railway": {{"command": "npx", "args": ["-y", "railway-mcp", "${{TEAM}}"],
                             "env": {{"TOKEN": "${{RAILWAY_TOKEN}}", "MODE": "${{MODE:-prod}}"}}}},
                "remote": {{"type": "http", "url": "https://api.example.com/${{TEAM}}/mcp",
                            "headers": {{"Authorization": "Bearer ${{API_KEY}}"}}}},
                "oauth": {{"type": "http", "url": "https://mcp.example.com/mcp"}},
                "legacy": {{"type": "sse", "url": "https://sse.example.com"}},
                "off": {{"command": "x", "disabled": true}}
              }},
              "projects": {{
                "{project}": {{"mcpServers": {{"local": {{"command": "./bin/server", "cwd": "data"}}}}}},
                "/elsewhere": {{"mcpServers": {{}}}}
              }}
            }}"#,
            project = project.display()
        ),
    );
    let roots = roots(
        &home,
        &[
            ("TEAM", "acme"),
            ("RAILWAY_TOKEN", "rw-secret"),
            ("API_KEY", "k-1"),
        ],
    );
    let catalog = Catalog::read(&roots, None);
    assert_eq!(
        ids(&catalog),
        [
            format!("claude:local:{}:local", project.display()).as_str(),
            "claude:user:oauth",
            "claude:user:railway",
            "claude:user:remote",
        ]
    );
    let railway = catalog.server("claude:user:railway").unwrap();
    assert_eq!(railway.source, Source::ClaudeUser);
    let (command, args, env, cwd, base) = stdio(railway);
    assert_eq!(command, "npx");
    assert_eq!(args, ["-y", "railway-mcp", "acme"]);
    assert_eq!(env, [("MODE", "prod"), ("TOKEN", "rw-secret")]);
    assert_eq!((cwd, base), (None, None));
    assert!(!railway.signs_in);

    let remote = catalog.server("claude:user:remote").unwrap();
    assert_eq!(
        http(remote),
        (
            "https://api.example.com/acme/mcp",
            vec![("Authorization", "Bearer k-1")]
        )
    );
    assert!(!remote.signs_in);
    let oauth = catalog.server("claude:user:oauth").unwrap();
    assert!(oauth.signs_in);
    let session = |agent| Session {
        agent,
        cwd: Path::new("/elsewhere"),
        project: None,
        in_checkout: false,
    };
    // Claude has it, and signs in by itself; how Codex could is not known yet.
    assert_eq!(
        catalog.sign_in(oauth, &session(Agent::Claude)),
        Some(SignIn::Native)
    );
    assert_eq!(
        catalog.sign_in(oauth, &session(Agent::Codex)),
        Some(SignIn::Unknown)
    );
    assert_eq!(catalog.sign_in(remote, &session(Agent::Codex)), None);

    let local = catalog.find("local").expect("one server is named local");
    assert_eq!(local.source, Source::ClaudeLocal);
    assert_eq!(local.project.as_deref(), Some(project.as_path()));
    let (command, _, _, cwd, base) = stdio(local);
    assert_eq!(command, project.join("./bin/server").display().to_string());
    assert_eq!(cwd, Some(project.join("data").as_path()));
    assert_eq!(base, Some(project.as_path()));
    assert_eq!(
        local.transport.working_dir(),
        Some(project.join("data").as_path())
    );
}

#[test]
fn secrets_never_show_in_debug() {
    let home = fixture("debug");
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {"s": {"command": "x", "env": {"TOKEN": "hunter2"}}}}"#,
    );
    let catalog = Catalog::read(&roots(&home, &[]), None);
    let shown = format!("{catalog:?}");
    assert!(shown.contains("TOKEN"));
    assert!(!shown.contains("hunter2"), "{shown}");
}

#[test]
fn claude_config_dir_moves_claude_json_and_skills() {
    let home = fixture("config-dir");
    let config = home.join("alt");
    write(
        &config.join(".claude.json"),
        r#"{"mcpServers": {"moved": {"command": "m"}}}"#,
    );
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {"ignored": {"command": "i"}}}"#,
    );
    skill(
        &config.join("skills/alt-skill"),
        "alt-skill",
        "From the moved folder.",
    );
    skill(
        &home.join(".claude/skills/home-skill"),
        "home-skill",
        "Not read.",
    );
    let roots = roots(
        &home,
        &[("CLAUDE_CONFIG_DIR", &config.display().to_string())],
    );
    let catalog = Catalog::read(&roots, None);
    assert_eq!(ids(&catalog), ["claude:user:moved"]);
    let skills: Vec<&str> = catalog.skills.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(skills, ["claude:user:alt-skill"]);
}

#[test]
fn project_mcp_json_and_cursor_configs() {
    let home = fixture("project");
    let project = home.join("app");
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {"db": {"command": "scripts/db-mcp", "args": ["${DB_URL}"]}}}"#,
    );
    write(
        &project.join(".cursor/mcp.json"),
        r#"{"mcpServers": {"cur": {"command": "node", "args": ["${workspaceFolder}/mcp.js", "${userHome}"],
                                    "env": {"KEY": "${env:CURSOR_KEY}"}}}}"#,
    );
    write(
        &home.join(".cursor/mcp.json"),
        r#"{"mcpServers": {"web": {"url": "https://web.example.com/mcp", "headers": {"X-Key": "${env:CURSOR_KEY}"}}}}"#,
    );
    let roots = roots(&home, &[("DB_URL", "postgres://x"), ("CURSOR_KEY", "ck")]);
    let without = Catalog::read(&roots, None);
    assert_eq!(ids(&without), ["cursor:user:web"]);
    let catalog = Catalog::read(&roots, Some(&project));
    let p = project.display();
    assert_eq!(
        ids(&catalog),
        [
            format!("claude:project:{p}:db").as_str(),
            format!("cursor:project:{p}:cur").as_str(),
            "cursor:user:web",
        ]
    );
    let (command, args, ..) = stdio(catalog.find("db").unwrap());
    assert_eq!(
        command,
        project.join("scripts/db-mcp").display().to_string()
    );
    assert_eq!(args, ["postgres://x"]);
    let (command, args, env, ..) = stdio(catalog.find("cur").unwrap());
    assert_eq!(command, "node");
    assert_eq!(
        args,
        [
            project.join("mcp.js").display().to_string(),
            home.display().to_string()
        ]
    );
    assert_eq!(env, [("KEY", "ck")]);
    let web = catalog.find("web").unwrap();
    assert_eq!(http(web).1, [("X-Key", "ck")]);
    // A header that isn't Authorization may be an API key: still, no Authorization means the
    // server may want a sign-in.
    assert!(web.signs_in);
}

#[test]
fn codex_user_config_servers_skills_and_trust() {
    let home = fixture("codex");
    let codex_home = home.join("codex-home");
    write(
        &codex_home.join("config.toml"),
        r#"
model = "gpt-5"

[mcp_servers.node_repl]
command = "node"
args = ["repl.js"]
env = { NODE_ENV = "dev" }
env_vars = ["SHARED", { name = "OTHER", source = "local" }, "MISSING"]
cwd = "/srv/repl"

[mcp_servers.api]
url = "https://api.example.com/mcp"
bearer_token_env_var = "API_TOKEN"
http_headers = { "X-Static" = "s" }
env_http_headers = { "X-Env" = "HDR" }

[mcp_servers.off]
command = "x"
enabled = false

[[skills.config]]
path = "/some/SKILL.md"
enabled = false

[projects."/trusted"]
trust_level = "trusted"

[projects."/untrusted"]
trust_level = "untrusted"
"#,
    );
    let roots = roots(
        &home,
        &[
            ("CODEX_HOME", &codex_home.display().to_string()),
            ("SHARED", "sh"),
            ("OTHER", "ot"),
            ("API_TOKEN", "tk"),
            ("HDR", "hv"),
        ],
    );
    let catalog = Catalog::read(&roots, None);
    assert_eq!(ids(&catalog), ["codex:user:api", "codex:user:node_repl"]);
    let (command, args, env, cwd, base) = stdio(catalog.find("node_repl").unwrap());
    assert_eq!((command, args), ("node", &["repl.js".to_owned()][..]));
    assert_eq!(
        env,
        [("NODE_ENV", "dev"), ("SHARED", "sh"), ("OTHER", "ot")]
    );
    assert_eq!((cwd, base), (Some(Path::new("/srv/repl")), None));
    let api = catalog.find("api").unwrap();
    assert_eq!(
        http(api),
        (
            "https://api.example.com/mcp",
            vec![
                ("X-Static", "s"),
                ("X-Env", "hv"),
                ("Authorization", "Bearer tk")
            ]
        )
    );
    assert!(!api.signs_in);
    assert_eq!(
        catalog.codex_skill_config,
        [serde_json::json!({"path": "/some/SKILL.md", "enabled": false})]
    );
    assert_eq!(catalog.codex_trusted, [PathBuf::from("/trusted")]);
}

#[test]
fn codex_project_layers_nearest_first() {
    let home = fixture("codex-layers");
    let project = home.join("repo");
    write(
        &project.join(".codex/config.toml"),
        "[mcp_servers.tool]\ncommand = \"./root-tool\"\n\n[mcp_servers.only_root]\ncommand = \"r\"\n",
    );
    let catalog = Catalog::read(&roots(&home, &[]), Some(&project));
    let p = project.display();
    assert_eq!(
        ids(&catalog),
        [
            format!("codex:project:{p}:only_root").as_str(),
            format!("codex:project:{p}:tool").as_str(),
        ]
    );
    let (command, _, _, _, base) = stdio(catalog.find("tool").unwrap());
    assert_eq!(command, project.join("./root-tool").display().to_string());
    assert_eq!(base, Some(project.as_path()));

    // A layer nearer the session's folder wins for a name.
    let nested = project.join("pkg");
    write(
        &nested.join(".codex/config.toml"),
        "[mcp_servers.tool]\ncommand = \"./pkg-tool\"\n",
    );
    let servers = servers::codex_layers(&roots(&home, &[]), &project, &nested);
    let tool: Vec<&Server> = servers.iter().filter(|s| s.name == "tool").collect();
    assert_eq!(tool.len(), 1);
    assert_eq!(
        stdio(tool[0]).0,
        nested.join("./pkg-tool").display().to_string()
    );
    assert_eq!(servers.len(), 2);
}

#[test]
fn claude_desktop_config() {
    let home = fixture("desktop");
    write(
        &home.join("desktop/claude_desktop_config.json"),
        r#"{"mcpServers": {"pencil": {"command": "/Applications/Pencil.app/mcp", "args": ["--stdio"], "env": {"P": "${NOT_EXPANDED}"}}}}"#,
    );
    let catalog = Catalog::read(&roots(&home, &[("NOT_EXPANDED", "x")]), None);
    let pencil = catalog.server("desktop:pencil").unwrap();
    assert_eq!(pencil.source, Source::Desktop);
    let (_, _, env, ..) = stdio(pencil);
    assert_eq!(env, [("P", "${NOT_EXPANDED}")]);
}

#[test]
fn desktop_config_location_per_platform() {
    let roots = Roots::from_vars([
        ("HOME".to_owned(), "/h".to_owned()),
        ("USERPROFILE".to_owned(), "/h".to_owned()),
        ("APPDATA".to_owned(), "/h/AppData/Roaming".to_owned()),
    ])
    .unwrap();
    let expected = if cfg!(target_os = "macos") {
        "/h/Library/Application Support/Claude/claude_desktop_config.json"
    } else if cfg!(windows) {
        "/h/AppData/Roaming/Claude/claude_desktop_config.json"
    } else {
        "/h/.config/Claude/claude_desktop_config.json"
    };
    assert_eq!(roots.desktop_config, Some(PathBuf::from(expected)));
    assert!(Roots::from_vars(Vec::new()).is_none());
}

#[test]
fn native_servers_depend_on_the_session() {
    let home = fixture("native");
    let project = home.join("app");
    let worktree = home.join("worktrees/task");
    let codex_home = home.join("codex-home");
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{"mcpServers": {{"cu": {{"command": "a"}}}},
                "projects": {{"{}": {{"mcpServers": {{"cl": {{"command": "b"}}}}}}}}}}"#,
            project.display()
        ),
    );
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {"cp": {"command": "c"}}}"#,
    );
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[mcp_servers.xu]\ncommand = \"d\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display()
        ),
    );
    write(
        &project.join(".codex/config.toml"),
        "[mcp_servers.xp]\ncommand = \"e\"\n",
    );
    write(
        &home.join("desktop/claude_desktop_config.json"),
        r#"{"mcpServers": {"dt": {"command": "f"}}}"#,
    );
    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    let catalog = Catalog::read(&roots, Some(&project));
    let native = |name: &str, session: Session<'_>| {
        catalog.server_native(catalog.find(name).unwrap(), &session)
    };
    let claude_in_project = Session {
        agent: Agent::Claude,
        cwd: &project,
        project: Some(&project),
        in_checkout: true,
    };
    let claude_in_worktree = Session {
        cwd: &worktree,
        ..claude_in_project
    };
    let codex_in_worktree = Session {
        agent: Agent::Codex,
        ..claude_in_worktree
    };
    assert!(native("cu", claude_in_worktree));
    assert!(native("cp", claude_in_worktree));
    // The local scope is keyed on the exact folder.
    assert!(native("cl", claude_in_project));
    assert!(!native("cl", claude_in_worktree));
    assert!(!native("xu", claude_in_worktree));
    assert!(!native("dt", claude_in_worktree));
    assert!(native("xu", codex_in_worktree));
    assert!(native("xp", codex_in_worktree));
    assert!(!native("cu", codex_in_worktree));
    assert!(!native("cp", codex_in_worktree));
    assert!(!native("dt", codex_in_worktree));
    // An untrusted project's Codex layers aren't loaded.
    let mut untrusted = catalog.clone();
    untrusted.codex_trusted.clear();
    assert!(!untrusted.server_native(untrusted.find("xp").unwrap(), &codex_in_worktree));
    // Without the project, its servers aren't native to anyone.
    let scratch = Session {
        project: None,
        in_checkout: false,
        ..codex_in_worktree
    };
    assert!(!native("xp", scratch));
}

#[test]
fn find_by_id_name_unknown_and_ambiguous() {
    let home = fixture("find");
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {"pencil": {"command": "/p/mcp", "args": ["--stdio"]},
                           "dup": {"command": "one"}}}"#,
    );
    write(
        &home.join("desktop/claude_desktop_config.json"),
        r#"{"mcpServers": {"pencil": {"command": "/p/mcp", "args": ["--stdio"]},
                           "dup": {"command": "two"}}}"#,
    );
    let catalog = Catalog::read(&roots(&home, &[]), None);
    // The same server in both Claude apps counts once.
    assert_eq!(catalog.find("pencil").unwrap().id, "claude:user:pencil");
    assert_eq!(catalog.find("desktop:pencil").unwrap().id, "desktop:pencil");
    assert_eq!(catalog.find("nope"), Err(FindError::Unknown));
    assert_eq!(
        catalog.find("dup"),
        Err(FindError::Ambiguous(vec![
            "claude:user:dup".into(),
            "desktop:dup".into()
        ]))
    );
    assert_eq!(catalog.find("desktop:dup").unwrap().name, "dup");
}

#[cfg(unix)]
#[test]
fn skills_dedupe_by_real_folder_and_keep_same_names_apart() {
    let home = fixture("skills");
    let codex_home = home.join(".codex");
    let project = home.join("app");
    skill(
        &home.join(".agents/skills/shared"),
        "shared",
        "Linked into Claude's skills.",
    );
    link(
        &home.join(".agents/skills/shared"),
        &home.join(".claude/skills/shared"),
    );
    skill(
        &home.join(".claude/skills/claude-only"),
        "claude-only",
        "Only Claude finds it.",
    );
    skill(
        &home.join(".agents/skills/agents-only"),
        "agents-only",
        "Only Codex finds it.",
    );
    // Same name, different content.
    skill(
        &home.join(".claude/skills/review"),
        "review",
        "Claude's review.",
    );
    skill(
        &codex_home.join("skills/review"),
        "review",
        "Codex's review.",
    );
    skill(
        &codex_home.join("skills/.system/builtin"),
        "builtin",
        "Codex's own.",
    );
    write(
        &home.join(".agents/skills/not-a-skill/README.md"),
        "no SKILL.md",
    );
    write(
        &home.join(".agents/skills/plain/SKILL.md"),
        "No frontmatter here.\n",
    );
    skill(
        &project.join(".claude/skills/proj"),
        "proj",
        "A project skill for Claude.",
    );
    skill(
        &project.join(".agents/skills/proj-agents"),
        "proj-agents",
        "A project skill for Codex.",
    );

    let catalog = Catalog::read(&roots(&home, &[]), Some(&project));
    let mut found: Vec<(&str, &str, &str)> = catalog
        .skills
        .iter()
        .map(|s| (s.id.as_str(), s.name.as_str(), s.description.as_str()))
        .collect();
    found.sort_unstable();
    let p = project.display();
    let claude_proj = format!("claude:project:{p}:proj");
    let agents_proj = format!("agents:project:{p}:proj-agents");
    let mut expected = vec![
        (
            "agents:user:agents-only",
            "agents-only",
            "Only Codex finds it.",
        ),
        ("agents:user:plain", "plain", ""),
        (
            "agents:user:shared",
            "shared",
            "Linked into Claude's skills.",
        ),
        (
            "claude:user:claude-only",
            "claude-only",
            "Only Claude finds it.",
        ),
        ("claude:user:review", "review", "Claude's review."),
        ("codex:user:review", "review", "Codex's review."),
        (claude_proj.as_str(), "proj", "A project skill for Claude."),
        (
            agents_proj.as_str(),
            "proj-agents",
            "A project skill for Codex.",
        ),
    ];
    expected.sort_unstable();
    assert_eq!(found, expected);

    let shared = catalog.skill("agents:user:shared").unwrap();
    assert_eq!(shared.roots, [SkillRoot::AgentsUser, SkillRoot::ClaudeUser]);
    assert_eq!(shared.dir, home.join(".agents/skills/shared"));

    let worktree = home.join("worktrees/t");
    let claude = Session {
        agent: Agent::Claude,
        cwd: &worktree,
        project: Some(&project),
        in_checkout: true,
    };
    let codex = Session {
        agent: Agent::Codex,
        ..claude
    };
    let read_only_codex = Session {
        cwd: &home,
        in_checkout: false,
        ..codex
    };
    let native =
        |id: &str, session: Session<'_>| catalog.skill_native(catalog.skill(id).unwrap(), &session);
    assert!(native("agents:user:shared", claude) && native("agents:user:shared", codex));
    assert!(native("claude:user:claude-only", claude));
    assert!(!native("claude:user:claude-only", codex));
    assert!(!native("agents:user:agents-only", claude));
    assert!(native("agents:user:agents-only", codex));
    assert!(native("codex:user:review", codex) && !native("codex:user:review", claude));
    assert!(native(&claude_proj, claude) && !native(&claude_proj, codex));
    assert!(native(&agents_proj, codex) && !native(&agents_proj, claude));
    // A Codex session outside a checkout doesn't find the project's `.agents/skills`.
    assert!(!native(&agents_proj, read_only_codex));
}

#[test]
fn frontmatter_with_crlf_quotes_and_multiline() {
    let home = fixture("frontmatter");
    write(
        &home.join(".agents/skills/a/SKILL.md"),
        "---\r\nname: \"quoted name\"\r\ndescription: >\r\n  Folded\r\n  text.\r\n---\r\nBody\r\n",
    );
    let catalog = Catalog::read(&roots(&home, &[]), None);
    let skill = &catalog.skills[0];
    assert_eq!(skill.name, "quoted name");
    assert_eq!(skill.description, "Folded text.");
    assert_eq!(skill.skill_md(), home.join(".agents/skills/a/SKILL.md"));
}

/// Waits up to ten seconds for `done`.
#[cfg(unix)]
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

#[cfg(unix)]
#[test]
fn watcher_refreshes_on_config_and_skill_edits_through_symlinks() {
    let home = fixture("watch");
    let elsewhere = home.join("elsewhere/linked");
    skill(&elsewhere, "linked", "Before.");
    link(&elsewhere, &home.join(".claude/skills/linked"));
    write(&home.join(".claude.json"), r#"{"mcpServers": {}}"#);
    let registry = Registry::new(roots(&home, &[]));
    registry.watch().unwrap();
    assert_eq!(registry.catalog(None).skills[0].description, "Before.");

    // An edit of SKILL.md where the symlink points.
    skill(&elsewhere, "linked", "After.");
    assert!(
        eventually(|| registry.catalog(None).skills[0].description == "After."),
        "the edit through the symlink wasn't seen"
    );

    // A server added to `.claude.json`.
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {"new": {"command": "n"}}}"#,
    );
    assert!(
        eventually(|| registry.catalog(None).server("claude:user:new").is_some()),
        "the config edit wasn't seen"
    );

    // A skill added to a watched root, then edited.
    skill(&home.join(".claude/skills/fresh"), "fresh", "New.");
    assert!(eventually(|| registry
        .catalog(None)
        .skill("claude:user:fresh")
        .is_some()));
}
