use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A fresh folder for one test, its symlinks resolved (so paths compare with the catalog's),
/// removed when the test ends.
fn fixture(name: &str) -> Fixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "brigadier-registry-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Fixture(std::fs::canonicalize(dir).unwrap())
}

struct Fixture(PathBuf);

impl std::ops::Deref for Fixture {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
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
fn unreadable_configs_and_entries_are_problems() {
    let home = fixture("problems");
    let project = home.join("app");
    write(
        &home.join(".cursor/mcp.json"),
        r#"{"mcpServers": {
            "figma": {"url": "ws://127.0.0.1:3845/mcp"},
            "odd": {"type": "websocket", "command": "odd-mcp"},
            "empty": {"args": ["x"]},
            "ok": {"command": "ok-mcp"}}}"#,
    );
    write(
        &project.join(".mcp.json"),
        "{\n  \"mcpServers\": {\n    oops\n}",
    );
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.fine]\ncommand = \"fine\"\n\n[mcp_servers.bad\n",
    );
    let roots = roots(&home, &[]);
    let catalog = Catalog::read(&roots, Some(&project));
    assert_eq!(ids(&catalog), ["cursor:user:ok"]);
    let found: Vec<_> = catalog
        .problems
        .iter()
        .map(|p| {
            (
                p.id.as_str(),
                p.name.as_str(),
                p.read.as_deref(),
                p.line,
                p.error.as_str(),
            )
        })
        .collect();
    let codex_file = format!(
        "codex:user:file:{}",
        home.join(".codex/config.toml").display()
    );
    let mcp_file = format!(
        "claude:project:file:{}",
        project.join(".mcp.json").display()
    );
    assert_eq!(
        found,
        [
            (
                codex_file.as_str(),
                "config.toml",
                None,
                Some(4),
                "The file couldn't be parsed (line 4)"
            ),
            (
                "cursor:user:empty",
                "empty",
                None,
                None,
                "It has no command or url"
            ),
            (
                "cursor:user:figma",
                "figma",
                Some("ws://127.0.0.1:3845/mcp"),
                None,
                "Unsupported address “ws://”: only http and https"
            ),
            (
                "cursor:user:odd",
                "odd",
                Some("odd-mcp"),
                None,
                "Unknown type “websocket”"
            ),
            (
                mcp_file.as_str(),
                ".mcp.json",
                None,
                Some(3),
                "The file couldn't be parsed (line 3)"
            ),
        ]
    );
    assert_eq!(catalog.problems[1].file, home.join(".cursor/mcp.json"));
    assert_eq!(
        catalog.problems[4].project.as_deref(),
        Some(project.as_path())
    );
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
    let servers = servers::codex_layers(&roots(&home, &[]), &project, &nested, &mut Vec::new());
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
                "projects": {{"{}": {{"mcpServers": {{"cl": {{"command": "b"}}}}, "hasTrustDialogAccepted":true, "enabledMcpjsonServers":["cp"]}}}}}}"#,
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
    // The local scope is keyed on the repository's main folder: a folder that is neither it
    // nor a checkout of it doesn't load it.
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

/// A linked worktree of `main` at `worktree`, laid out as git lays it out.
fn linked_worktree(main: &Path, worktree: &Path) {
    std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    write(&main.join(".git/worktrees/wt/commondir"), "../..\n");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    );
}

#[test]
fn claudes_per_project_off_flags_switch_its_servers_off_there_only() {
    let home = fixture("claude-off");
    let project = home.join("app");
    let other = home.join("other");
    let worktree = home.join("worktrees/task");
    linked_worktree(&project, &worktree);
    std::fs::create_dir_all(&other).unwrap();
    let plugin_root = home.join(".claude/plugins/cache/lib/copy/1.0.0");
    write(
        &plugin_root.join(".claude-plugin/plugin.json"),
        r#"{"name": "copy", "version": "1.0.0"}"#,
    );
    write(
        &plugin_root.join(".mcp.json"),
        r#"{"mcpServers": {"cu": {"command": "copy"}}}"#,
    );
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"version": 2, "plugins": {{"copy@lib": [{{"scope": "user", "installPath": "{}", "version": "1.0.0"}}]}}}}"#,
            plugin_root.display()
        ),
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"copy@lib": true}}"#,
    );
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{"mcpServers": {{"cu": {{"command": "a"}}, "on": {{"command": "z"}}}},
                "projects": {{"{}": {{
                    "mcpServers": {{"cl": {{"command": "b"}}}},
                    "disabledMcpServers": ["cu", "cl", "cp", "plugin:copy:cu"]
                }}}}}}"#,
            project.display()
        ),
    );
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {"cp": {"command": "c"}}}"#,
    );
    let roots = roots(&home, &[]);
    let catalog = Catalog::read(&roots, Some(&project));
    assert_eq!(catalog.claude_off_error, None);
    let session = |cwd| Session {
        agent: Agent::Claude,
        cwd,
        project: Some(&project),
        in_checkout: true,
    };
    let server = |name: &str, source: Source| {
        catalog
            .servers
            .iter()
            .find(|server| server.name == name && server.source == source)
            .unwrap()
    };
    for cwd in [&project, &worktree] {
        let session = session(cwd);
        for (name, source) in [
            ("cu", Source::ClaudeUser),
            ("cl", Source::ClaudeLocal),
            ("cp", Source::ClaudeProject),
            ("cu", Source::ClaudePlugin),
        ] {
            assert!(
                !catalog.server_native(server(name, source), &session),
                "{name} {source:?} in {}",
                cwd.display()
            );
        }
        assert!(catalog.server_native(server("on", Source::ClaudeUser), &session));
    }
    // The local server loads in a linked worktree of its repository once it isn't switched
    // off; the flags count in that project only.
    let elsewhere = Catalog::read(&roots, Some(&other));
    let in_other = Session {
        agent: Agent::Claude,
        cwd: &other,
        project: Some(&other),
        in_checkout: true,
    };
    let user = |name: &str| {
        elsewhere
            .servers
            .iter()
            .find(|server| server.name == name && server.source == Source::ClaudeUser)
            .unwrap()
    };
    assert!(elsewhere.server_native(user("cu"), &in_other));
    let mut on = catalog.clone();
    on.claude_off_here.clear();
    assert!(on.server_native(server("cl", Source::ClaudeLocal), &session(&worktree)));
    assert!(on.server_native(server("cu", Source::ClaudePlugin), &session(&project)));
    // Not trusted, its team server waits for approval.
    assert_eq!(
        catalog.claude_approval("cp"),
        Some(claude_config::Approval::Pending)
    );
    assert_eq!(catalog.claude_approval("cu"), None);
}

#[test]
fn an_unreadable_claude_config_leaves_the_off_flags_unknown() {
    let home = fixture("claude-off-bad");
    let project = home.join("app");
    std::fs::create_dir_all(&project).unwrap();
    write(&home.join(".claude.json"), "{ not json");
    let catalog = Catalog::read(&roots(&home, &[]), Some(&project));
    assert!(
        catalog
            .claude_off_error
            .as_deref()
            .is_some_and(|err| err.starts_with("Brigadier couldn't read"))
    );
    assert!(catalog.claude_off_here.is_empty());
}

#[test]
fn an_overridden_entry_is_not_native() {
    let home = fixture("override");
    let project = home.join("app");
    let codex_home = home.join("codex-home");
    write(
        &home.join(".claude.json"),
        &format!(
            r#"{{"mcpServers": {{"db": {{"command": "user-db"}}}},
                "projects": {{"{}": {{"mcpServers": {{"db": {{"command": "local-db"}}}}}}}}}}"#,
            project.display()
        ),
    );
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[mcp_servers.db]\ncommand = \"user-db\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display()
        ),
    );
    write(
        &project.join(".codex/config.toml"),
        "[mcp_servers.db]\ncommand = \"project-db\"\n",
    );
    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    let catalog = Catalog::read(&roots, Some(&project));
    let p = project.display();
    let native = |id: &str, session: Session<'_>| {
        catalog.server_native(catalog.server(id).unwrap(), &session)
    };
    let codex = Session {
        agent: Agent::Codex,
        cwd: &project,
        project: Some(&project),
        in_checkout: true,
    };
    // Codex loads the project's `db`, not the user's.
    assert!(native(&format!("codex:project:{p}:db"), codex));
    assert!(!native("codex:user:db", codex));
    // Elsewhere the user's is Codex's own again.
    let scratch = Session {
        cwd: &home,
        project: None,
        in_checkout: false,
        ..codex
    };
    assert!(native("codex:user:db", scratch));
    // Claude's local scope wins over its user scope where it applies.
    let claude = Session {
        agent: Agent::Claude,
        ..codex
    };
    assert!(native(&format!("claude:local:{p}:db"), claude));
    assert!(!native("claude:user:db", claude));
    assert!(native(
        "claude:user:db",
        Session {
            agent: Agent::Claude,
            ..scratch
        }
    ));
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
    // A name counts only the servers kept; an id names any.
    let on = |server: &Server| server.id != "claude:user:dup";
    assert_eq!(catalog.find_among("dup", on).unwrap().id, "desktop:dup");
    let off = |server: &Server| server.name != "pencil";
    assert_eq!(catalog.find_among("pencil", off), Err(FindError::Unknown));
    assert_eq!(
        catalog.find_among("claude:user:pencil", off).unwrap().id,
        "claude:user:pencil"
    );
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
    assert_eq!(
        shared.entries,
        [
            (SkillRoot::AgentsUser, home.join(".agents/skills/shared")),
            (SkillRoot::ClaudeUser, home.join(".claude/skills/shared")),
        ]
    );

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
fn copied_skills_merge_and_synced_skills_are_claudes() {
    let home = fixture("copies");
    let codex_home = home.join(".codex");
    // The same skill copied (not linked) into three roots, with a file beside SKILL.md.
    for root in [
        home.join(".agents/skills"),
        home.join(".claude/skills"),
        codex_home.join("skills"),
    ] {
        skill(
            &root.join("use-railway"),
            "use-railway",
            "Deploy to Railway.",
        );
        write(&root.join("use-railway/scripts/run.sh"), "echo hi\n");
    }
    // Same name, a file that differs: two skills.
    skill(
        &home.join(".agents/skills/impeccable"),
        "impeccable",
        "One.",
    );
    skill(
        &home.join(".claude/skills/impeccable"),
        "impeccable",
        "One.",
    );
    write(&home.join(".claude/skills/impeccable/extra.md"), "more\n");
    // Claude's synced skills, the same in two accounts.
    for account in ["org_a", "org_b"] {
        skill(
            &home.join(format!(".claude/skills/synced/{account}/docx")),
            "docx",
            "Word files.",
        );
    }
    write(
        &home.join(".claude/skills/synced/org_a/manifest.json"),
        "{}",
    );

    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    let catalog = Catalog::read(&roots, None);
    let mut ids: Vec<&str> = catalog.skills.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(
        ids,
        [
            "agents:user:impeccable",
            "agents:user:use-railway",
            "claude:synced:org_a:docx",
            "claude:user:impeccable",
        ]
    );
    let railway = catalog.skill("agents:user:use-railway").unwrap();
    assert_eq!(
        railway.roots,
        [
            SkillRoot::AgentsUser,
            SkillRoot::ClaudeUser,
            SkillRoot::CodexUser
        ]
    );
    assert_eq!(railway.entries.len(), 3);
    let docx = catalog.skill("claude:synced:org_a:docx").unwrap();
    assert_eq!(docx.entries.len(), 2);
    let claude = Session {
        agent: Agent::Claude,
        cwd: &home,
        project: None,
        in_checkout: false,
    };
    let codex = Session {
        agent: Agent::Codex,
        ..claude
    };
    assert!(catalog.skill_native(docx, &claude));
    assert!(!catalog.skill_native(docx, &codex));
}

#[test]
fn skills_for_display_hide_synced_only_entries_but_keep_local_copies() {
    let home = fixture("skill-provenance");
    let synced = home.join(".claude/skills/synced");
    skill(&synced.join("org_a/builtin"), "renamed", "Provider skill.");
    skill(&synced.join("org_a/personal"), "personal", "User skill.");
    skill(&synced.join("org_a/unknown"), "unknown", "No provenance.");
    for account in ["org_a", "org_b"] {
        skill(
            &synced.join(account).join("shared"),
            "shared",
            "Same bytes.",
        );
    }
    for root in [home.join(".claude/skills"), synced.join("org_a")] {
        skill(&root.join("local-copy"), "local-copy", "Same local bytes.");
    }
    let catalog = Catalog::read(&roots(&home, &[]), None);
    let builtin = catalog.skill("claude:synced:org_a:builtin").unwrap();
    assert!(builtin.user_installed().is_none());
    // Classification is display-only: Claude still finds synced skills natively.
    assert!(catalog.skill_native(
        builtin,
        &Session {
            agent: Agent::Claude,
            cwd: &home,
            project: None,
            in_checkout: false,
        }
    ));
    for name in ["personal", "unknown"] {
        assert!(
            catalog
                .skill(&format!("claude:synced:org_a:{name}"))
                .unwrap()
                .user_installed()
                .is_none()
        );
    }
    let shared = catalog.skill("claude:synced:org_a:shared").unwrap();
    assert_eq!(shared.entries.len(), 2);
    assert!(shared.user_installed().is_none());
    let local = catalog.skill("claude:user:local-copy").unwrap();
    assert_eq!(local.entries.len(), 2);
    assert_eq!(local.user_installed().as_ref(), Some(local));
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

    // A skill root and a config whose folders didn't exist when watching started.
    skill(&home.join(".agents/skills/first"), "first", "First.");
    assert!(
        eventually(|| registry.catalog(None).skill("agents:user:first").is_some()),
        "a skill root made later wasn't seen"
    );
    write(
        &home.join(".cursor/mcp.json"),
        r#"{"mcpServers": {"cur": {"command": "c"}}}"#,
    );
    assert!(
        eventually(|| registry.catalog(None).server("cursor:user:cur").is_some()),
        "a config in a folder made later wasn't seen"
    );
}

/// A Claude Code plugin installed at `root` with `manifest`, and one skill in its `skills/`.
fn claude_plugin(root: &Path, manifest: &str, skill_name: &str) {
    write(&root.join(".claude-plugin/plugin.json"), manifest);
    skill(
        &root.join("skills").join(skill_name),
        skill_name,
        "From a plugin.",
    );
}

/// The names of the skills `session` finds by itself, with the plugin they come with.
fn native_skills(catalog: &Catalog, session: &Session<'_>) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .skills
        .iter()
        .filter(|skill| catalog.skill_native(skill, session))
        .map(|skill| match skill.plugin() {
            Some(plugin) => plugin.skill_name(&skill.name),
            None => skill.name.clone(),
        })
        .collect();
    names.sort();
    names
}

#[test]
fn claude_plugins_enabled_disabled_and_project_scoped() {
    let home = fixture("claude-plugins");
    let project = home.join("work/app");
    let other = home.join("work/other");
    let cache = home.join(".claude/plugins/cache/market");
    let install = |name: &str, scope: &str, project: Option<&Path>| {
        let at = project.map_or_else(String::new, |project| {
            format!(r#", "projectPath": "{}""#, project.display())
        });
        format!(
            r#""{name}@market": [{{"scope": "{scope}", "installPath": "{}", "version": "1.0.0"{at}}}]"#,
            cache.join(name).join("1.0.0").display()
        )
    };
    for (name, manifest) in [
        ("on", r#"{"name": "on"}"#),
        ("off", r#"{"name": "off"}"#),
        ("dormant", r#"{"name": "dormant", "defaultEnabled": false}"#),
        ("unlisted", r#"{"name": "unlisted"}"#),
        ("here", r#"{"name": "here"}"#),
        ("mine", r#"{"name": "mine"}"#),
        ("theirs", r#"{"name": "theirs"}"#),
        ("hushed", r#"{"name": "hushed"}"#),
    ] {
        claude_plugin(
            &cache.join(name).join("1.0.0"),
            manifest,
            &format!("{name}-skill"),
        );
    }
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"version": 2, "plugins": {{{}}}}}"#,
            [
                install("on", "user", None),
                install("off", "user", None),
                install("dormant", "user", None),
                install("unlisted", "user", None),
                install("here", "user", None),
                install("hushed", "user", None),
                install("mine", "project", Some(&project)),
                install("theirs", "local", Some(&other)),
            ]
            .join(", ")
        ),
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"on@market": true, "off@market": false, "here@market": false,
                               "hushed@market": true}}"#,
    );
    // The project's settings switch one on, the user's own local settings one off.
    write(
        &project.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"here@market": true, "mine@market": true}}"#,
    );
    write(
        &project.join(".claude/settings.local.json"),
        r#"{"enabledPlugins": {"hushed@market": false}}"#,
    );
    write(
        &other.join(".claude/settings.local.json"),
        r#"{"enabledPlugins": {"theirs@market": true}}"#,
    );
    // A plugin folder placed in the user's skills folder, switched as `<name>@skills-dir`.
    claude_plugin(
        &home.join(".claude/skills/dropped"),
        r#"{"name": "dropped-in"}"#,
        "dropped-skill",
    );
    let roots = roots(&home, &[]);
    let anywhere = Session {
        agent: Agent::Claude,
        cwd: &home,
        project: None,
        in_checkout: false,
    };
    let user = Catalog::read(&roots, None);
    assert_eq!(
        native_skills(&user, &anywhere),
        [
            "dropped-in:dropped-skill",
            "hushed:hushed-skill",
            "on:on-skill",
            "unlisted:unlisted-skill"
        ]
    );
    let in_project = Session {
        cwd: &project,
        project: Some(&project),
        ..anywhere
    };
    let catalog = Catalog::read(&roots, Some(&project));
    assert_eq!(
        native_skills(&catalog, &in_project),
        [
            "dropped-in:dropped-skill",
            "here:here-skill",
            "mine:mine-skill",
            "on:on-skill",
            "unlisted:unlisted-skill"
        ]
    );
    // Codex finds none of them by itself: Brigadier gives them to its workers.
    let codex = Session {
        agent: Agent::Codex,
        ..in_project
    };
    assert!(native_skills(&catalog, &codex).is_empty());
    // What only the project switches on, and its installs, count only there.
    let mine = catalog
        .skills
        .iter()
        .find(|skill| skill.name == "mine-skill")
        .unwrap();
    assert_eq!(
        mine.id,
        format!(
            "claude:plugin:{}:mine@market:skills/mine-skill",
            project.display()
        )
    );
    assert!(!catalog.skill_native(mine, &anywhere));
    assert!(
        catalog
            .skills
            .iter()
            .all(|skill| !skill.name.starts_with("theirs") && !skill.name.starts_with("off"))
    );
    let on = user
        .skill("claude:plugin:on@market:skills/on-skill")
        .unwrap();
    assert_eq!(on.dir, cache.join("on/1.0.0/skills/on-skill"));
}

#[test]
fn claude_plugin_layout_and_servers() {
    let home = fixture("claude-plugin-layout");
    let cache = home.join(".claude/plugins/cache/market");
    let full = cache.join("full/abc123");
    // `skills/` and the manifest's entries, one level deep; the root SKILL.md isn't read then.
    write(
        &full.join(".claude-plugin/plugin.json"),
        r#"{"name": "full", "skills": ["./extra", "./one", "../outside"]}"#,
    );
    skill(&full.join("skills/conv"), "conv", "Conventional.");
    skill(
        &full.join("skills/group/deep"),
        "deep",
        "Too deep for Claude.",
    );
    skill(&full.join("extra/ex"), "ex", "From an entry.");
    skill(&full.join("one"), "one", "An entry that is one skill.");
    skill(&full, "full-root", "Not read.");
    write(
        &full.join(".mcp.json"),
        r#"{"mcpServers": {"tool": {"command": "${CLAUDE_PLUGIN_ROOT}/bin/tool",
                                    "args": ["--team", "${TEAM}"]},
                           "remote": {"type": "http", "url": "https://mcp.example.com/mcp"}}}"#,
    );
    // Only a root SKILL.md: the plugin is that one skill. Its install's folder is gone, so the
    // cache folder of its commit is read.
    let root_only = cache.join("solo/def456");
    write(&root_only.join(".claude-plugin/plugin.json"), "{}");
    skill(&root_only, "solo-root", "The plugin's own.");
    write(
        &home.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"plugins": {{"full@market": [{{"scope": "user", "installPath": "{}"}}],
                             "solo@market": [{{"scope": "user", "installPath": "{}",
                                               "gitCommitSha": "def456aaaa"}}]}}}}"#,
            full.display(),
            cache.join("solo/gone").display()
        ),
    );
    let roots = roots(&home, &[("TEAM", "blue")]);
    let catalog = Catalog::read(&roots, None);
    let mut names: Vec<String> = catalog
        .skills
        .iter()
        .map(|skill| skill.plugin().unwrap().skill_name(&skill.name))
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["full:conv", "full:ex", "full:one", "solo:solo-root"]
    );
    let tool = catalog.server("claude:plugin:full@market:tool").unwrap();
    assert_eq!(tool.source, Source::ClaudePlugin);
    assert_eq!(tool.native_name(), "plugin_full_tool");
    assert_eq!(tool.file, full.join(".mcp.json"));
    let (command, args, ..) = stdio(tool);
    assert_eq!(command, full.join("bin/tool").display().to_string());
    assert_eq!(args, ["--team", "blue"]);
    let remote = catalog.server("claude:plugin:full@market:remote").unwrap();
    assert!(remote.signs_in);
    let claude = Session {
        agent: Agent::Claude,
        cwd: &home,
        project: None,
        in_checkout: false,
    };
    assert!(catalog.server_native(tool, &claude));
    assert!(!catalog.server_native(
        tool,
        &Session {
            agent: Agent::Codex,
            ..claude
        }
    ));
}

#[test]
fn codex_plugins_enabled_disabled_and_versions() {
    let home = fixture("codex-plugins");
    let codex_home = home.join("codex-home");
    let cache = codex_home.join("plugins/cache/market");
    let plugin = |name: &str, version: &str, manifest: &str| {
        let root = cache.join(name).join(version);
        write(&root.join(".codex-plugin/plugin.json"), manifest);
        root
    };
    // On: the newest version by semver, its skills to any depth, its servers in its folder.
    let old = plugin("docs", "1.9.0", r#"{"name": "docs"}"#);
    skill(&old.join("skills/old"), "old", "An older version's.");
    let docs = plugin("docs", "1.10.0", r#"{"name": "documents"}"#);
    skill(&docs.join("skills/write"), "write", "Writes documents.");
    skill(&docs.join("skills/group/inner"), "inner", "A nested skill.");
    write(
        &docs.join(".mcp.json"),
        r#"{"mcpServers": {"docs-server": {"command": "./bin/server", "cwd": "."},
                           "hushed": {"command": "h"},
                           "dormant": {"command": "d", "enabled": false}}}"#,
    );
    // Its manifest's entries instead of `skills/`.
    let listed = plugin(
        "listed",
        "local",
        r#"{"name": "listed", "skills": ["./a"]}"#,
    );
    skill(&listed.join("a/first"), "first", "Listed.");
    skill(&listed.join("skills/unread"), "unread", "Not listed.");
    plugin("listed", "9.9.9", r#"{"name": "listed"}"#);
    // Off, and not switched at all.
    let off = plugin("off", "1.0.0", r#"{"name": "off"}"#);
    skill(&off.join("skills/no"), "no", "Switched off.");
    let unlisted = plugin("unlisted", "1.0.0", r#"{"name": "unlisted"}"#);
    skill(&unlisted.join("skills/no"), "no", "Never switched on.");
    write(
        &codex_home.join("config.toml"),
        "[plugins.\"docs@market\"]\nenabled = true\n\n\
         [plugins.\"docs@market\".mcp_servers.hushed]\nenabled = false\n\n\
         [plugins.\"listed@market\"]\n\n\
         [plugins.\"off@market\"]\nenabled = false\n",
    );
    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    let catalog = Catalog::read(&roots, None);
    let codex = Session {
        agent: Agent::Codex,
        cwd: &home,
        project: None,
        in_checkout: false,
    };
    assert_eq!(
        native_skills(&catalog, &codex),
        ["documents:inner", "documents:write", "listed:first"]
    );
    let claude = Session {
        agent: Agent::Claude,
        ..codex
    };
    assert!(native_skills(&catalog, &claude).is_empty());
    let inner = catalog
        .skill("codex:plugin:docs@market:skills/group/inner")
        .unwrap();
    assert_eq!(
        inner.entries[0].1,
        docs.join("skills/group/inner"),
        "Codex knows it by this path"
    );
    let plugin_servers: Vec<&str> = catalog
        .servers
        .iter()
        .filter(|server| server.source == Source::CodexPlugin)
        .map(|server| server.id.as_str())
        .collect();
    assert_eq!(plugin_servers, ["codex:plugin:docs@market:docs-server"]);
    let server = catalog.find("docs-server").unwrap();
    assert_eq!(server.native_name(), "docs-server");
    let (command, _, _, cwd, _) = stdio(server);
    assert_eq!(command, docs.join("./bin/server").display().to_string());
    assert_eq!(cwd, Some(docs.join(".").as_path()));
    assert!(catalog.server_native(server, &codex));
    assert!(!catalog.server_native(server, &claude));
}

#[test]
fn a_trusted_projects_codex_file_switches_plugins_and_servers_there_only() {
    let home = fixture("codex-layer");
    let codex_home = home.join("codex-home");
    let cache = codex_home.join("plugins/cache/brigadier");
    for name in ["here", "every"] {
        let root = cache.join(name).join("1.0.0");
        write(
            &root.join(".codex-plugin/plugin.json"),
            &format!(r#"{{"name": "{name}"}}"#),
        );
        skill(&root.join(format!("skills/{name}")), name, "A skill.");
    }
    let project = home.join("project");
    let other = home.join("other");
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[plugins.\"here@brigadier\"]\nenabled = false\n\n\
             [plugins.\"every@brigadier\"]\nenabled = true\n\n\
             [mcp_servers.pencil]\ncommand = \"pencil\"\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display(),
            other.display()
        ),
    );
    write(
        &project.join(".codex/config.toml"),
        "# mine\n[plugins.\"here@brigadier\"]\nenabled = true\n\n\
         [plugins.\"every@brigadier\"]\nenabled = false\n\n\
         [mcp_servers.pencil]\nenabled = false\n",
    );
    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    fn session(project: &Path) -> Session<'_> {
        Session {
            agent: Agent::Codex,
            cwd: project,
            project: Some(project),
            in_checkout: true,
        }
    }
    let here = Catalog::read(&roots, Some(&project));
    assert_eq!(native_skills(&here, &session(&project)), ["here:here"]);
    let pencil = here.servers.iter().find(|s| s.name == "pencil").unwrap();
    assert!(!here.server_native(pencil, &session(&project)));
    let there = Catalog::read(&roots, Some(&other));
    assert_eq!(native_skills(&there, &session(&other)), ["every:every"]);
    let pencil = there.servers.iter().find(|s| s.name == "pencil").unwrap();
    assert!(there.server_native(pencil, &session(&other)));
    // Untrusted: Codex doesn't read the file, and neither does the catalog.
    write(
        &codex_home.join("config.toml"),
        "[plugins.\"here@brigadier\"]\nenabled = false\n\n\
         [plugins.\"every@brigadier\"]\nenabled = true\n",
    );
    let untrusted = Catalog::read(&roots, Some(&project));
    assert_eq!(
        native_skills(&untrusted, &session(&project)),
        ["every:every"]
    );
}

#[test]
fn a_codex_project_entry_that_only_switches_a_server_keeps_the_users_command() {
    let home = fixture("codex-switch-only");
    let codex_home = home.join("codex-home");
    let project = home.join("project");
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[mcp_servers.pencil]\ncommand = \"pencil\"\nenabled = false\n\n\
             [mcp_servers.figma]\nurl = \"https://figma.example/mcp\"\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display()
        ),
    );
    write(
        &project.join(".codex/config.toml"),
        "[mcp_servers.pencil]\nenabled = true\n\n[mcp_servers.figma]\nenabled = false\n",
    );
    let roots = roots(&home, &[("CODEX_HOME", &codex_home.display().to_string())]);
    let here = Catalog::read(&roots, Some(&project));
    assert!(here.problems.is_empty(), "{:?}", here.problems);
    let pencil = here.server(&format!("codex:project:{}:pencil", project.display()));
    let pencil = pencil.unwrap_or_else(|| panic!("{:?}", ids(&here)));
    assert_eq!(stdio(pencil).0, "pencil");
    let session = Session {
        agent: Agent::Codex,
        cwd: &project,
        project: Some(&project),
        in_checkout: true,
    };
    assert!(here.server_native(pencil, &session));
    let figma = here.servers.iter().find(|s| s.name == "figma").unwrap();
    assert!(!here.server_native(figma, &session));
}

#[test]
fn a_disabled_server_is_kept_off_and_raw_text_is_kept_as_written() {
    let home = fixture("disabled-raw");
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {
            "quiet": {"command": "q-mcp", "disabled": true},
            "tmpl": {"command": "t-mcp", "args": ["--key=${KEY}"], "env": {"A": "plain"}},
            "plain": {"type": "http", "url": "https://mcp.example.com/${PATH_PART}"}}}"#,
    );
    write(
        &home.join(".codex/config.toml"),
        "[mcp_servers.fromenv]\ncommand = \"e-mcp\"\nenv_vars = [\"TOKEN\"]\n",
    );
    let catalog = Catalog::read(
        &roots(&home, &[("KEY", "secret-key"), ("PATH_PART", "mcp")]),
        None,
    );
    // Switched off, but still the user's: kept with the switched-off servers, never loaded.
    assert!(!catalog.servers.iter().any(|server| server.name == "quiet"));
    let quiet = catalog
        .off
        .servers
        .iter()
        .find(|server| server.name == "quiet")
        .unwrap();
    assert!(quiet.disabled);
    // What a variable expands to never replaces what the config wrote.
    let tmpl = catalog.server("claude:user:tmpl").unwrap();
    assert_eq!(tmpl.written.args, ["--key=${KEY}"]);
    assert!(tmpl.templated);
    let plain = catalog.server("claude:user:plain").unwrap();
    assert_eq!(
        plain.written.url.as_deref(),
        Some("https://mcp.example.com/${PATH_PART}")
    );
    assert!(plain.templated);
    assert!(catalog.server("codex:user:fromenv").unwrap().templated);
}

#[test]
fn a_change_reads_again_then_tells_each_listener() {
    let home = fixture("listeners");
    let registry = Registry::new(roots(&home, &[]));
    let told = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = told.clone();
    registry.on_change(move || {
        count.fetch_add(1, Ordering::SeqCst);
    });
    write(
        &home.join(".claude.json"),
        r#"{"mcpServers": {"late": {"command": "late-mcp"}}}"#,
    );
    registry.changed();
    assert_eq!(told.load(Ordering::SeqCst), 1);
    assert!(registry.catalog(None).server("claude:user:late").is_some());
}

#[cfg(unix)]
#[test]
fn the_watcher_looks_at_each_projects_own_configs_and_skills() {
    let home = fixture("watch-projects");
    let project = home.join("proj");
    let elsewhere = home.join("elsewhere/linked");
    skill(&elsewhere, "linked", "Linked from elsewhere.");
    std::fs::create_dir_all(project.join(".claude/skills")).unwrap();
    link(&elsewhere, &project.join(".claude/skills/linked"));
    let registry = Registry::new(roots(&home, &[]));
    registry.watch_projects(std::slice::from_ref(&project));
    let watched = registry.watched();
    for file in [
        project.join(".mcp.json"),
        project.join(".codex/config.toml"),
        project.join(".claude/settings.json"),
        project.join(".claude/settings.local.json"),
    ] {
        assert!(watched.files.contains(&file), "{}", file.display());
    }
    assert!(watched.trees.contains(&project.join(".agents/skills")));
    // A project skill linked from elsewhere: its own folder too.
    let real = std::fs::canonicalize(&elsewhere).unwrap();
    assert!(watched.trees.contains(&real), "{:?}", watched.trees);
}

#[test]
fn rows_include_user_servers_and_marketplace_skills_with_provenance() {
    let home = fixture("row-provenance");
    let roots = roots(&home, &[]);
    write(
        &roots.codex_home.join("config.toml"),
        r#"
[mcp_servers.node_repl]
command = "node_repl"
[plugins."browser@user-market"]
enabled = true
"#,
    );
    let plugin = roots
        .codex_home
        .join("plugins/cache/user-market/browser/1.0.0");
    write(
        &plugin.join(".codex-plugin/plugin.json"),
        r#"{"name":"browser","skills":"./skills/"}"#,
    );
    skill(&plugin.join("skills/browser"), "browser", "Browse");
    let catalog = Catalog::read(&roots, None);
    let rows = Catalog::rows(&[catalog]);
    let node = rows.iter().find(|row| row.name == "node_repl").unwrap();
    assert_eq!(node.definitions[0].provenance.agent, Some(Agent::Codex));
    assert_eq!(node.definitions[0].provenance.native, "node_repl");
    assert_eq!(
        node.definitions[0].provenance.file,
        roots.codex_home.join("config.toml")
    );
    let browser = rows.iter().find(|row| row.name == "browser").unwrap();
    let origin = &browser.definitions[0].provenance;
    assert_eq!(origin.plugin.as_deref(), Some("browser@user-market"));
    assert_eq!(origin.native, "browser:browser");
    assert_eq!(origin.fingerprint.len(), 64);
    assert_eq!(origin.file, plugin.join("skills/browser/SKILL.md"));
}

#[test]
fn rows_group_content_but_retain_each_source_and_separate_endpoints() {
    let home = fixture("rows-grouping");
    let roots = roots(&home, &[]);
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"db":{"url":"https://one.test/mcp"}}}"#,
    );
    write(
        &roots.codex_home.join("config.toml"),
        "[mcp_servers.db]\nurl = 'https://two.test/mcp'\n",
    );
    skill(&roots.claude_dir.join("skills/shared"), "shared", "same");
    skill(&roots.codex_home.join("skills/shared"), "shared", "same");
    let catalogs = [Catalog::read(&roots, None)];
    let rows = Catalog::rows(&catalogs);
    assert_eq!(rows.iter().filter(|row| row.name == "db").count(), 2);
    let shared = rows.iter().find(|row| row.name == "shared").unwrap();
    assert_eq!(shared.definitions.len(), 2);
    assert!(!shared.different_per_agent);
    let id = shared.id.clone();
    write(
        &roots.codex_home.join("skills/shared/support.txt"),
        "support affects identity",
    );
    let rows = Catalog::rows(&[Catalog::read(&roots, None)]);
    let shared = rows.iter().find(|row| row.name == "shared").unwrap();
    assert_ne!(id, shared.id);
    assert!(shared.different_per_agent);
    assert_eq!(shared.definitions.len(), 2);
}

#[test]
fn read_rows_retains_disabled_project_plugin_definitions() {
    let home = fixture("disabled-project-rows");
    let roots = roots(&home, &[]);
    let project = home.join("project");
    let plugin = home.join("plugin");
    write(
        &plugin.join(".claude-plugin/plugin.json"),
        r#"{"name":"team"}"#,
    );
    skill(&plugin.join("skills/review"), "review", "Review");
    write(
        &plugin.join(".mcp.json"),
        r#"{"mcpServers":{"db":{"url":"https://team.test/mcp"}}}"#,
    );
    write(&roots.claude_dir.join("plugins/installed_plugins.json"), &serde_json::json!({"plugins":{"team@test":[{"scope":"project", "projectPath":project,"installPath":plugin}]}}).to_string());
    write(
        &project.join(".claude/settings.json"),
        r#"{"enabledPlugins":{"team@test":false}}"#,
    );
    let rows = Catalog::read_rows(&roots, std::slice::from_ref(&project));
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row.definitions[0].provenance.project, Some(project.clone()));
        assert_eq!(
            row.definitions[0].provenance.plugin.as_deref(),
            Some("team@test")
        );
    }
    assert!(Catalog::read(&roots, Some(&project)).plugins.is_empty());
}

#[test]
fn equivalent_remote_definitions_share_a_row_regardless_of_header_order() {
    let home = fixture("header-order-rows");
    let roots = roots(&home, &[]);
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"db":{"url":"https://same.test/mcp","headers":{"Z":"z","A":"a"}}}}"#,
    );
    write(
        &roots.codex_home.join("config.toml"),
        "[mcp_servers.db]\nurl = 'https://same.test/mcp'\nhttp_headers = { A = 'a', Z = 'z' }\n",
    );
    let rows = Catalog::read_rows(&roots, &[]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].definitions.len(), 2);
}

#[test]
fn rows_exclude_all_openai_marketplaces_and_system_skills() {
    let home = fixture("provider-rows");
    let codex = home.join("custom-codex-home");
    let roots = roots(&home, &[("CODEX_HOME", codex.to_str().unwrap())]);
    let project = home.join("project");
    let mut config = String::new();
    for (index, market) in [
        "openai-curated-remote",
        "openai-bundled",
        "openai-primary-runtime",
        "user-market",
    ]
    .iter()
    .enumerate()
    {
        for enabled in [true, false] {
            let name = format!("plugin-{index}-{enabled}");
            let plugin = codex.join(format!("plugins/cache/{market}/{name}/1.0.0"));
            write(
                &plugin.join(".codex-plugin/plugin.json"),
                &format!(r#"{{"name":"{name}"}}"#),
            );
            skill(&plugin.join("skills").join(&name), &name, &name);
            write(
                &plugin.join(".mcp.json"),
                &format!(r#"{{"mcpServers":{{"{name}":{{"command":"{name}"}}}}}}"#),
            );
            config.push_str(&format!(
                "[plugins.\"{name}@{market}\"]\nenabled = {enabled}\n"
            ));
        }
    }
    write(&codex.join("config.toml"), &config);
    // The row builder also reads project-only switches regardless of trust.
    write(&project.join(".codex/config.toml"), &config);
    skill(
        &codex.join("skills/.system/builtin"),
        "builtin",
        "Codex's own",
    );
    let data = home.join("data");
    for rows in [
        Catalog::read_rows(&roots, std::slice::from_ref(&project)),
        Catalog::read_rows_with_library(&roots, &[project], &data),
    ] {
        assert_eq!(rows.len(), 4, "{rows:?}");
        assert!(rows.iter().all(|row| row.name.starts_with("plugin-3-")));
        assert!(
            rows.iter()
                .flat_map(|row| &row.definitions)
                .all(|definition| definition
                    .provenance
                    .plugin
                    .as_deref()
                    .is_some_and(|id| id.ends_with("@user-market")))
        );
    }
    // Native discovery is still available to callers; only Brigadier rows omit it.
    let native = Catalog::read(&roots, None);
    assert_eq!(
        native
            .plugins
            .iter()
            .filter(|plugin| plugin.provider_supplied())
            .count(),
        3
    );
}

#[test]
fn rows_exclude_claude_synced_skills_but_keep_user_copies() {
    let home = fixture("synced-rows");
    let claude = home.join("custom-claude-home");
    let roots = roots(&home, &[("CLAUDE_CONFIG_DIR", claude.to_str().unwrap())]);
    let source = claude.join("skills/synced/team-a/only/SKILL.md");
    for account in ["team-a", "team-b"] {
        for name in ["only", "shared"] {
            skill(
                &claude.join("skills/synced").join(account).join(name),
                name,
                "Same bytes.",
            );
        }
    }
    skill(&claude.join("skills/shared"), "shared", "Same bytes.");
    skill(
        &roots.agents_skills.join("personal"),
        "personal",
        "User's own.",
    );
    let before = std::fs::read(&source).unwrap();
    let native = Catalog::read(&roots, None);
    assert!(native.skills.iter().any(|skill| {
        skill
            .entries
            .iter()
            .any(|(root, _)| matches!(root, SkillRoot::ClaudeSynced(_)))
    }));
    for rows in [
        Catalog::rows(&[native]),
        Catalog::read_rows(&roots, &[]),
        Catalog::read_rows_with_library(&roots, &[], &home.join("data")),
    ] {
        assert_eq!(rows.len(), 2, "{rows:?}");
        assert!(
            rows.iter()
                .all(|row| ["shared", "personal"].contains(&row.name.as_str()))
        );
        for row in &rows {
            assert_eq!(row.definitions.len(), 1);
            assert!(!row.definitions[0].provenance.provider_supplied());
            assert!(
                !row.definitions[0]
                    .provenance
                    .file
                    .starts_with(claude.join("skills/synced"))
            );
        }
    }
    assert_eq!(std::fs::read(source).unwrap(), before);
}
