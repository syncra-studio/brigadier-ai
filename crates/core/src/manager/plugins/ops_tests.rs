use std::path::{Path, PathBuf};
use std::process::Command;

use brigadier_registry::adapter::TomlEdit;
use brigadier_registry::{Roots, Secret};

use super::*;

/// A Keychain that remembers, in memory.
#[derive(Default)]
struct Kept(std::sync::Mutex<BTreeMap<String, BTreeMap<String, String>>>);

impl SecretSink for Kept {
    fn store(&self, server: &str, name: &str, value: &Secret) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .entry(server.to_owned())
            .or_default()
            .insert(name.to_owned(), value.expose().to_owned());
        Ok(())
    }
}

impl Secrets for Kept {
    fn load(&self, server: &str) -> Result<BTreeMap<String, String>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(server)
            .cloned()
            .unwrap_or_default())
    }

    fn forget(&self, server: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(server);
        Ok(())
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brigadier-ops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn roots(home: &Path) -> Roots {
    Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap()
}

/// The Library's skill `name`, installed for both agents and on or off in each one's user
/// scope (`None`: not installed).
fn seed(home: &Path, project: &Path, claude: Option<bool>, codex: Option<bool>) -> Library {
    let library = Library::in_home(home, PathBuf::from("/bin/brigadier"));
    let skill = home.join("src/tidy");
    write(
        &skill.join("SKILL.md"),
        "---\nname: tidy\ndescription: d\n---\n",
    );
    let item = Item::Skill(SkillSpec {
        name: Some("tidy".into()),
        source: SkillSource::Folder(skill.clone()),
    });
    let packed = library
        .pack("tidy", &item, Some(&skill), &[], false, &Kept::default())
        .unwrap();
    let dir = packed.entry.dir;
    if let Some(on) = claude {
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::json!({"version": 2, "plugins": {"tidy@brigadier": [
                {"scope": "user", "installPath": dir, "version": "1.0.0"}
            ]}})
            .to_string(),
        );
        write(
            &home.join(".claude/settings.json"),
            &serde_json::json!({"enabledPlugins": {"tidy@brigadier": on}}).to_string(),
        );
    }
    let mut config = format!(
        "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        project.display()
    );
    if let Some(on) = codex {
        let cache = home.join(".codex/plugins/cache/brigadier/tidy/1.0.0");
        write(
            &cache.join(".codex-plugin/plugin.json"),
            &std::fs::read_to_string(dir.join(".codex-plugin/plugin.json")).unwrap(),
        );
        write(
            &cache.join("skills/tidy/SKILL.md"),
            "---\nname: tidy\ndescription: d\n---\n",
        );
        config.push_str(&format!("\n[plugins.\"tidy@brigadier\"]\nenabled = {on}\n"));
    }
    write(&home.join(".codex/config.toml"), &config);
    library
}

struct Fixture {
    home: PathBuf,
    project: PathBuf,
    roots: Roots,
    library: Library,
    secrets: Kept,
}

impl Fixture {
    fn new(name: &str, claude: Option<bool>, codex: Option<bool>) -> Self {
        let home = home(name);
        let project = home.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        let library = seed(&home, &project, claude, codex);
        Self {
            roots: roots(&home),
            home,
            project,
            library,
            secrets: Kept::default(),
        }
    }

    fn plan(&self, agent: Agent, project: Option<&Path>, on: bool) -> Option<Vec<Step>> {
        let projects = [self.project.clone()];
        let ctx = Ctx {
            roots: &self.roots,
            env: &[],
            library: &self.library,
            secrets: &self.secrets,
            projects: &projects,
            report: &|_, _| {},
            binary: &|_| None,
        };
        plugin_plan(&ctx, adapter(agent), "tidy@brigadier", project, on, true).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

/// Each step as one line: a command, or a key set in a file.
fn lines(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .map(|step| match step {
            Step::Run(call) => format!("{} {}", call.program, call.args.join(" ")),
            Step::UserConfig(TomlEdit { key, value }) => {
                format!("user {} = {value:?}", key.join("."))
            }
            Step::ProjectFile {
                edit: TomlEdit { key, value },
                ..
            } => format!("project {} = {value:?}", key.join(".")),
        })
        .collect()
}

const ON: Option<toml::Value> = Some(toml::Value::Boolean(true));
const OFF: Option<toml::Value> = Some(toml::Value::Boolean(false));

#[test]
fn a_new_library_item_on_in_one_project_is_that_projects_only() {
    let fx = Fixture::new("only-here", None, None);
    let claude = lines(&fx.plan(Agent::Claude, Some(&fx.project), true).unwrap());
    assert_eq!(
        claude.last().unwrap(),
        "claude plugin install tidy@brigadier --scope local --json"
    );
    assert!(claude[0].starts_with("claude plugin marketplace add "));
    let codex = lines(&fx.plan(Agent::Codex, Some(&fx.project), true).unwrap());
    assert_eq!(
        &codex[codex.len() - 3..],
        [
            "codex plugin add tidy@brigadier".to_owned(),
            format!("user plugins.tidy@brigadier.enabled = {OFF:?}"),
            format!("project plugins.tidy@brigadier.enabled = {ON:?}"),
        ]
    );
}

#[test]
fn an_item_on_everywhere_is_turned_off_in_one_project_by_its_own_scope() {
    let fx = Fixture::new("off-here", Some(true), Some(true));
    assert_eq!(
        lines(&fx.plan(Agent::Claude, Some(&fx.project), false).unwrap()),
        ["claude plugin disable tidy@brigadier --scope local --json"]
    );
    assert_eq!(
        lines(&fx.plan(Agent::Codex, Some(&fx.project), false).unwrap()),
        [format!("project plugins.tidy@brigadier.enabled = {OFF:?}")]
    );
    // Already on there: nothing to do.
    assert!(
        fx.plan(Agent::Claude, Some(&fx.project), true)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn an_installed_item_off_everywhere_is_turned_on_in_one_project() {
    let fx = Fixture::new("on-here", Some(false), Some(false));
    assert_eq!(
        lines(&fx.plan(Agent::Claude, Some(&fx.project), true).unwrap()),
        ["claude plugin enable tidy@brigadier --scope local --json"]
    );
    assert_eq!(
        lines(&fx.plan(Agent::Codex, Some(&fx.project), true).unwrap()),
        [format!("project plugins.tidy@brigadier.enabled = {ON:?}")]
    );
}

#[test]
fn off_everywhere_also_turns_it_off_where_a_project_turned_it_on() {
    let fx = Fixture::new("off-all", Some(false), Some(false));
    write(
        &fx.project.join(".codex/config.toml"),
        "[plugins.\"tidy@brigadier\"]\nenabled = true\n",
    );
    assert_eq!(
        lines(&fx.plan(Agent::Codex, None, false).unwrap()),
        [
            format!("user plugins.tidy@brigadier.enabled = {OFF:?}"),
            format!("project plugins.tidy@brigadier.enabled = {OFF:?}"),
        ]
    );
    // Every project: installed, so only switched on.
    assert_eq!(
        lines(&fx.plan(Agent::Claude, None, true).unwrap()),
        ["claude plugin enable tidy@brigadier --scope user --json"]
    );
}

#[test]
fn off_everywhere_also_turns_it_off_where_a_project_switches_it_on_over_every_project() {
    let fx = Fixture::new("off-all-over", Some(true), Some(true));
    write(
        &fx.project.join(".claude/settings.local.json"),
        r#"{"enabledPlugins": {"tidy@brigadier": true}}"#,
    );
    write(
        &fx.project.join(".codex/config.toml"),
        "[plugins.\"tidy@brigadier\"]\nenabled = true\n",
    );
    assert_eq!(
        lines(&fx.plan(Agent::Claude, None, false).unwrap()),
        [
            "claude plugin disable tidy@brigadier --scope user --json",
            "claude plugin disable tidy@brigadier --scope local --json",
        ]
    );
    assert_eq!(
        lines(&fx.plan(Agent::Codex, None, false).unwrap()),
        [
            format!("user plugins.tidy@brigadier.enabled = {OFF:?}"),
            format!("project plugins.tidy@brigadier.enabled = {OFF:?}"),
        ]
    );
    // Remove takes the project's own switch too, so it isn't on there again once reinstalled.
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    assert_eq!(
        lines(&removal(&ctx, adapter(Agent::Codex), "tidy@brigadier")),
        [
            format!("project plugins.tidy@brigadier.enabled = {OFF:?}"),
            "codex plugin remove tidy@brigadier".to_owned(),
        ]
    );
    // Only on for every project: nothing in the project.
    std::fs::remove_file(fx.project.join(".claude/settings.local.json")).unwrap();
    assert_eq!(
        lines(&fx.plan(Agent::Claude, None, false).unwrap()),
        ["claude plugin disable tidy@brigadier --scope user --json"]
    );
}

#[test]
fn an_agents_own_item_off_everywhere_can_be_switched_on_again() {
    let fx = Fixture::new("on-again", None, None);
    let kit = fx.home.join(".codex/plugins/cache/market/kit/1.0.0");
    write(&kit.join(".codex-plugin/plugin.json"), r#"{"name": "kit"}"#);
    write(
        &fx.home.join(".codex/config.toml"),
        &format!(
            "[mcp_servers.pencil]\ncommand = \"pencil-mcp\"\nenabled = false\n\n\
             [plugins.\"kit@market\"]\nenabled = false\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            fx.project.display()
        ),
    );
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    assert_eq!(
        plugin_id_of(&ctx, Agent::Codex, "kit").as_deref(),
        Some("kit@market")
    );
    assert_eq!(
        lines(
            &plugin_plan(&ctx, adapter(Agent::Codex), "kit@market", None, true, false)
                .unwrap()
                .unwrap()
        ),
        [format!("user plugins.kit@market.enabled = {ON:?}")]
    );
    for project in [None, Some(fx.project.as_path())] {
        assert!(has_server(
            &ctx.read(project),
            adapter(Agent::Codex),
            "pencil"
        ));
        assert!(!has_server(
            &ctx.read(project),
            adapter(Agent::Claude),
            "pencil"
        ));
    }
    // Its Remove takes it too.
    assert_eq!(
        server_places(&ctx, "pencil"),
        Ok(vec![(Agent::Codex, Place::Everywhere)])
    );
}

#[test]
fn remove_also_takes_an_install_a_project_has_of_its_own() {
    let fx = Fixture::new("remove-local", None, None);
    let dir = fx.library.entry("tidy").unwrap().dir;
    write(
        &fx.home.join(".claude/plugins/installed_plugins.json"),
        &serde_json::json!({"version": 2, "plugins": {"tidy@brigadier": [
            {"scope": "local", "projectPath": fx.project, "installPath": dir, "version": "1.0.0"}
        ]}})
        .to_string(),
    );
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    let steps = removal(&ctx, adapter(Agent::Claude), "tidy@brigadier");
    assert_eq!(
        lines(&steps).last().unwrap(),
        "claude plugin uninstall tidy@brigadier --scope local --json"
    );
    let Some(Step::Run(call)) = steps.last() else {
        panic!("{steps:?}");
    };
    assert_eq!(call.cwd.as_deref(), Some(fx.project.as_path()));
}

#[test]
fn an_agents_own_skill_or_another_apps_server_goes_through_use_everywhere() {
    let fx = Fixture::new("own", None, None);
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    for key in ["skill:grill", "other:desktop:figma"] {
        assert_eq!(
            switch(&ctx, key, Some(&fx.project), false, None),
            Err(ADD_TO_EVERY_AGENT_FIRST.to_owned())
        );
    }
    assert!(switch(&ctx, "nonsense", None, true, None).is_err());
}

#[test]
fn a_claude_server_switched_in_a_project_says_what_claude_code_needs() {
    let fx = Fixture::new("claude-server", None, None);
    write(
        &fx.home.join(".claude.json"),
        r#"{"mcpServers": {"codescene": {"command": "codescene-mcp"}}}"#,
    );
    let projects = [fx.project.clone()];
    let said = std::sync::Mutex::new(Vec::new());
    let report = |agent: Agent, step: PluginStep| said.lock().unwrap().push((agent, step));
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &report,
        binary: &|_| None,
    };
    assert_eq!(
        switch(&ctx, "server:codescene", Some(&fx.project), true, None),
        Ok(false)
    );
    let said = said.into_inner().unwrap();
    // Claude Code's own words, not a made-up one.
    assert!(
        said.contains(&(
            Agent::Claude,
            PluginStep::Failed {
                reason: "Claude Code can turn it on or off per project once it's in your Library."
                    .to_owned()
            }
        ))
    );
    assert!(ADD_TO_EVERY_AGENT_FIRST.starts_with("Add to every agent first"));
}

#[test]
fn add_to_every_agent_leaves_a_copy_in_a_folder_that_isnt_a_project_as_it_is() {
    let fx = Fixture::new("foreign", None, None);
    let foreign = fx.home.join("clients/freelogo-fos-2026");
    std::fs::create_dir_all(&foreign).unwrap();
    write(
        &fx.home.join(".claude.json"),
        &serde_json::json!({"projects": {
            foreign.display().to_string(): {"mcpServers": {"codescene": {"command": "codescene-mcp"}}},
            fx.project.display().to_string(): {"mcpServers": {"codescene": {"command": "codescene-mcp"}}}
        }})
        .to_string(),
    );
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    let mut found = find(&ctx, &Target::Server("codescene".to_owned())).unwrap();
    assert_eq!(found.projects.as_ref().map(Vec::len), Some(2));
    within_projects(&mut found, &projects);
    // The project's copy moves and the copy goes on there; the other folder's stays.
    assert_eq!(found.projects, Some(vec![fx.project.clone()]));
    assert_eq!(
        found
            .servers
            .iter()
            .map(|(agent, folder, _)| (*agent, folder.clone()))
            .collect::<Vec<_>>(),
        [(Agent::Claude, Some(fx.project.clone()))]
    );
    // Kept only to that folder: nothing moves and the copy is on nowhere.
    let mut only = find(&ctx, &Target::Server("codescene".to_owned())).unwrap();
    within_projects(&mut only, &[]);
    assert_eq!((only.projects, only.servers.len()), (Some(Vec::new()), 0));
}

#[test]
fn a_paste_is_read_before_anything_is_added() {
    let home = home("check");
    let check = check_add(
        &home,
        "claude mcp add --transport http sentry https://mcp.sentry.dev/mcp",
        None,
    );
    assert_eq!(check.error, None);
    assert_eq!(check.items.len(), 1);
    let sentry = &check.items[0];
    assert_eq!((sentry.name.as_str(), sentry.remote), ("sentry", true));
    assert!(
        sentry.summary.contains("mcp.sentry.dev"),
        "{}",
        sentry.summary
    );
    let check = check_add(&home, "{\"command\": \"x\"}", None);
    assert!(check.needs_name, "{check:?}");
    assert!(check.error.is_some());
    let check = check_add(
        &home,
        "{\"command\": \"x\", \"env\": {\"KEY\": \"v\"}}",
        Some("x"),
    );
    assert_eq!(check.items[0].secrets, ["KEY"]);
    let _ = std::fs::remove_dir_all(home);
}

/// Every file under `dir`, read as text.
fn texts(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(texts(&path));
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            found.push((path, text));
        }
    }
    found
}

#[test]
fn add_goes_only_to_the_picked_agents_and_its_values_only_to_the_keychain() {
    let home = home("add-picked");
    let roots = roots(&home);
    // No CLI can be found: each picked agent's part is tried, and fails, without running one.
    let env = [("PATH".to_owned(), home.join("no-bin").display().to_string())];
    let library = Library::in_home(&home, PathBuf::from("/bin/brigadier"));
    let secrets = Kept::default();
    let reported = std::sync::Mutex::new(Vec::new());
    let report = |agent: Agent, _: PluginStep| reported.lock().unwrap().push(agent);
    let missing = home.join("no-bin/agent");
    let binary = |_: Agent| Some(missing.clone());
    let ctx = Ctx {
        roots: &roots,
        env: &env,
        library: &library,
        secrets: &secrets,
        projects: &[],
        report: &report,
        binary: &binary,
    };
    let picked = adapters().last().unwrap().agent();
    let paste = r#"{"mcpServers": {"docs": {"command": "/nowhere/docs-mcp", "env": {"TOKEN": "pasted-token"}}}}"#;
    let options = AddOptions {
        agents: Some(vec![picked]),
        values: BTreeMap::from([
            ("TOKEN".to_owned(), "typed-token".to_owned()),
            ("REGION".to_owned(), "region-eu-west".to_owned()),
        ]),
    };
    let (keys, _) = add(&ctx, paste, None, None, false, &options).unwrap();
    assert_eq!(keys, ["lib:docs"]);
    let mut agents = reported.lock().unwrap().clone();
    agents.dedup();
    assert_eq!(agents, [picked], "only the picked agent's part is planned");
    assert_eq!(
        secrets.load("docs/docs").unwrap(),
        BTreeMap::from([
            ("REGION".to_owned(), "region-eu-west".to_owned()),
            ("TOKEN".to_owned(), "typed-token".to_owned()),
        ])
    );
    for (path, text) in texts(&home) {
        for value in ["typed-token", "pasted-token", "region-eu-west"] {
            assert!(!text.contains(value), "{value} in {}", path.display());
        }
    }

    // Values go with one server; at least one agent is picked.
    let two = r#"{"mcpServers": {"a": {"command": "x"}, "b": {"command": "y"}}}"#;
    assert!(add(&ctx, two, None, None, false, &options).is_err());
    let none = AddOptions {
        agents: Some(Vec::new()),
        values: BTreeMap::new(),
    };
    assert!(add(&ctx, paste, Some("other"), None, false, &none).is_err());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn use_everywhere_refuses_what_the_team_shares_and_keeps_everything_as_it_was() {
    let fx = Fixture::new("shared", None, None);
    write(
        &fx.project.join(".mcp.json"),
        r#"{"mcpServers": {"pg": {"command": "pg-mcp"}}}"#,
    );
    write(
        &fx.home.join(".claude.json"),
        &serde_json::json!({"projects": {fx.project.display().to_string(): {
            "enabledMcpjsonServers": ["pg"]
        }}})
        .to_string(),
    );
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    let err = use_everywhere(&ctx, "server:pg").unwrap_err();
    assert!(err.contains("shared with your team"), "{err}");
    assert!(fx.library.entry("pg").is_none());
    assert!(use_everywhere(&ctx, "server:gone").is_err());
}

#[test]
fn an_original_server_is_saved_without_its_values_and_put_back_with_them() {
    let fx = Fixture::new("saved", None, None);
    let original = Original::Server {
        agent: ProviderKind::Codex,
        name: "pg".into(),
        project: Some(fx.project.clone()),
        transport: SavedTransport::Stdio {
            command: "pg-mcp".into(),
            args: vec![],
            env: vec!["PGPASS".into()],
            cwd: None,
        },
        secrets: "pg/original-0".into(),
    };
    let text = serde_json::to_string(&Moved {
        originals: vec![original.clone()],
    })
    .unwrap();
    assert!(!text.contains("hunter2"));
    fx.secrets
        .store("pg/original-0", "PGPASS", &Secret::new("hunter2"))
        .unwrap();
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        report: &|_, _| {},
        binary: &|_| None,
    };
    // Codex's project file: written by Brigadier (D8), the value back in it as it was.
    original.restore(&ctx).unwrap();
    let file = std::fs::read_to_string(fx.project.join(".codex/config.toml")).unwrap();
    let table: toml::Table = file.parse().unwrap();
    assert_eq!(
        table["mcp_servers"]["pg"]["command"].as_str(),
        Some("pg-mcp")
    );
    assert_eq!(
        table["mcp_servers"]["pg"]["env"]["PGPASS"].as_str(),
        Some("hunter2")
    );
    assert!(fx.secrets.load("pg/original-0").unwrap().is_empty());

    // A skill moved aside comes back to where it was.
    let from = fx.home.join(".claude/skills/grill");
    let kept = fx.home.join("kept/skill-0");
    write(&kept.join("SKILL.md"), "---\nname: grill\n---\n");
    Original::Skill {
        from: from.clone(),
        kept,
    }
    .restore(&ctx)
    .unwrap();
    assert!(from.join("SKILL.md").is_file());
}

/// The real CLIs, in throwaway homes: Add only in one project, off in another, Use
/// everywhere and its Undo, Remove. Run with `--ignored`.
#[test]
#[ignore = "runs the installed claude and codex"]
fn both_clis_follow_the_page() {
    let home = home("live");
    let claude_dir = home.join("claude");
    let codex_home = home.join("codex");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::create_dir_all(&codex_home).unwrap();
    let project = home.join("proj");
    let sibling = home.join("sib");
    for dir in [&project, &sibling] {
        std::fs::create_dir_all(dir).unwrap();
        assert!(
            Command::new("git")
                .arg("init")
                .arg("-q")
                .arg(dir)
                .status()
                .unwrap()
                .success()
        );
    }
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[projects.\"{}\"]\ntrust_level = \"trusted\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            project.display(),
            sibling.display()
        ),
    );
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| key == "PATH" || key == "USER" || key == "TMPDIR")
        .collect();
    vars.push(("HOME".into(), home.display().to_string()));
    vars.push(("CLAUDE_CONFIG_DIR".into(), claude_dir.display().to_string()));
    vars.push(("CODEX_HOME".into(), codex_home.display().to_string()));
    let roots = Roots::from_vars(vars.clone()).unwrap();
    let env: Vec<(String, String)> = roots.vars.clone().into_iter().collect();
    let library = Library::in_home(&home, PathBuf::from("/bin/brigadier"));
    let secrets = Kept::default();
    let projects = [project.clone(), sibling.clone()];
    let failures = std::sync::Mutex::new(Vec::new());
    let report = |agent: Agent, step: PluginStep| {
        if let PluginStep::Failed { reason } = step {
            failures
                .lock()
                .unwrap()
                .push(format!("{agent:?}: {reason}"));
        }
    };
    let ctx = Ctx {
        roots: &roots,
        env: &env,
        library: &library,
        secrets: &secrets,
        projects: &projects,
        report: &report,
        binary: &|_| None,
    };
    let lists_skill = |project: &Path, plugin: &str| -> Vec<Agent> {
        let mut agents: Vec<Agent> = Catalog::read(&roots, Some(project))
            .skills
            .iter()
            .flat_map(|skill| skill.roots.clone())
            .filter_map(|root| match root {
                SkillRoot::Plugin(p) if p.id == plugin_id(plugin) => Some(p.agent),
                _ => None,
            })
            .collect();
        agents.sort_by_key(|agent| format!("{agent:?}"));
        agents.dedup();
        agents
    };
    let skill = home.join("src/tidy");
    write(
        &skill.join("SKILL.md"),
        "---\nname: tidy\ndescription: d\n---\nbody\n",
    );

    // Only in this project.
    add(
        &ctx,
        &skill.display().to_string(),
        None,
        Some(&project),
        false,
        &AddOptions::default(),
    )
    .unwrap();
    assert_eq!(failures.lock().unwrap().as_slice(), [] as [String; 0]);
    assert_eq!(lists_skill(&project, "tidy"), [Agent::Claude, Agent::Codex]);
    assert!(lists_skill(&sibling, "tidy").is_empty());
    let exclude = std::fs::read_to_string(project.join(".git/info/exclude")).unwrap();
    assert!(exclude.contains(".codex/config.toml"), "{exclude}");

    // Everywhere, then off in the sibling.
    switch(&ctx, "lib:tidy", None, true, None).unwrap();
    assert_eq!(lists_skill(&sibling, "tidy").len(), 2);
    switch(&ctx, "lib:tidy", Some(&sibling), false, None).unwrap();
    assert!(lists_skill(&sibling, "tidy").is_empty());
    assert_eq!(lists_skill(&project, "tidy").len(), 2);
    assert_eq!(failures.lock().unwrap().as_slice(), [] as [String; 0]);

    // Remove: gone from every agent and project.
    remove(&ctx, "lib:tidy").unwrap();
    assert!(lists_skill(&project, "tidy").is_empty());
    assert!(lists_skill(&sibling, "tidy").is_empty());
    assert!(library.entry("tidy").is_none());

    // Use everywhere of Claude's own skill, then Undo.
    write(
        &claude_dir.join("skills/grill/SKILL.md"),
        "---\nname: grill\ndescription: g\n---\n",
    );
    use_everywhere(&ctx, "skill:grill").unwrap();
    assert!(!claude_dir.join("skills/grill").exists());
    assert_eq!(lists_skill(&sibling, "grill").len(), 2);
    assert!(can_undo(&library, "grill"));
    undo_use_everywhere(&ctx, "lib:grill").unwrap();
    assert!(claude_dir.join("skills/grill/SKILL.md").is_file());
    assert!(lists_skill(&sibling, "grill").is_empty());
    assert!(library.entry("grill").is_none());
    assert_eq!(failures.lock().unwrap().as_slice(), [] as [String; 0]);

    // Claude's own server with a value: moved, its value kept aside, and put back by Undo.
    let out = Command::new("claude")
        .args([
            "mcp",
            "add-json",
            "pg",
            r#"{"command":"/usr/bin/true","env":{"PGPASS":"hunter2"}}"#,
        ])
        .args(["--scope", "user"])
        .envs(vars.iter().cloned())
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    use_everywhere(&ctx, "server:pg").unwrap();
    let claude_json = || std::fs::read_to_string(claude_dir.join(".claude.json")).unwrap();
    assert!(!claude_json().contains("hunter2"));
    let servers = |project: &Path| -> Vec<Agent> {
        let mut agents: Vec<Agent> = Catalog::read(&roots, Some(project))
            .servers
            .iter()
            .filter_map(|server| server.plugin.as_ref())
            .filter(|plugin| plugin.id == plugin_id("pg"))
            .map(|plugin| plugin.agent)
            .collect();
        agents.sort_by_key(|agent| format!("{agent:?}"));
        agents.dedup();
        agents
    };
    assert_eq!(servers(&sibling), [Agent::Claude, Agent::Codex]);
    for file in ["plugins/pg/.mcp.json", "plugins/pg/.codex.mcp.json"] {
        let text = std::fs::read_to_string(library.root().join(file)).unwrap();
        assert!(!text.contains("hunter2"), "{text}");
    }
    undo_use_everywhere(&ctx, "lib:pg").unwrap();
    assert!(claude_json().contains("hunter2"));
    assert!(servers(&sibling).is_empty());
    assert_eq!(failures.lock().unwrap().as_slice(), [] as [String; 0]);
    let _ = std::fs::remove_dir_all(home);
}

const MCP_SERVER: &str = r#"import json, sys
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    if req['method'] == 'initialize':
        result = {'protocolVersion': req['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1'}}
    elif req['method'] == 'tools/list':
        result = {'tools': [{'name': 'probe', 'inputSchema': {'type': 'object'}}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
"#;

/// What a plain CLI prints, run in `folder` with the throwaway homes in `vars`.
fn plain(vars: &[(String, String)], folder: &Path, program: &str, args: &[&str]) -> String {
    let out = Command::new(program)
        .args(args)
        .env_clear()
        .envs(vars.iter().cloned())
        .current_dir(folder)
        .output()
        .unwrap();
    assert!(out.status.success(), "{program} {args:?}: {out:?}");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The servers a plain `claude mcp list` in `folder` lists.
fn claude_servers(vars: &[(String, String)], folder: &Path) -> Vec<String> {
    let text = plain(vars, folder, "claude", &["mcp", "list"]);
    println!("claude mcp list in {}:\n{text}", folder.display());
    let mut names: Vec<String> = text
        .lines()
        .filter(|line| line.contains(" - "))
        .filter_map(|line| line.split_once(": ").map(|(name, _)| name.to_owned()))
        .collect();
    names.sort();
    names
}

/// The servers a plain `codex mcp list` in `folder` has on.
fn codex_servers(vars: &[(String, String)], folder: &Path) -> Vec<String> {
    let text = plain(vars, folder, "codex", &["mcp", "list", "--json"]);
    println!("codex mcp list --json in {}:\n{text}", folder.display());
    let listed: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    let mut names: Vec<String> = listed
        .iter()
        .filter(|server| server["enabled"] == true)
        .filter_map(|server| server["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    names
}

/// The lines a CLI started in `folder` prints, until `stop` says enough; then it is stopped.
fn read_until(
    mut command: Command,
    vars: &[(String, String)],
    folder: &Path,
    input: &[serde_json::Value],
    stop: impl Fn(&serde_json::Value) -> Option<Vec<String>>,
) -> Vec<String> {
    use std::io::{BufRead, Write};
    let mut child = command
        .env_clear()
        .envs(vars.iter().cloned())
        .current_dir(folder)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for message in input {
        writeln!(stdin, "{message}").unwrap();
    }
    stdin.flush().unwrap();
    let mut found = None;
    for line in std::io::BufReader::new(child.stdout.take().unwrap()).lines() {
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line.unwrap()) else {
            continue;
        };
        if let Some(names) = stop(&message) {
            found = Some(names);
            break;
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let mut names = found.expect("the CLI ended before it said");
    names.sort();
    names
}

/// The skills a plain `claude` in `folder` starts with, from its `system/init`.
fn claude_skills(vars: &[(String, String)], folder: &Path) -> Vec<String> {
    let mut command = Command::new("claude");
    command.args(["-p", "--output-format", "stream-json", "--verbose", "hi"]);
    let names = read_until(command, vars, folder, &[], |message| {
        (message["type"] == "system" && message["subtype"] == "init").then(|| {
            message["skills"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
    });
    println!("claude skills in {}: {names:?}", folder.display());
    names
}

/// The skills a plain Codex in `folder` has on, from its app-server's `skills/list`.
fn codex_skills(vars: &[(String, String)], folder: &Path) -> Vec<String> {
    let mut command = Command::new("codex");
    command.arg("app-server");
    let input = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "probe", "version": "1"}}}),
        serde_json::json!({"jsonrpc": "2.0", "method": "initialized"}),
        serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "skills/list",
            "params": {"cwds": [folder]}}),
    ];
    let names = read_until(command, vars, folder, &input, |message| {
        (message["id"] == 2 && message.get("method").is_none()).then(|| {
            message["result"]["data"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|entry| entry["skills"].as_array().unwrap().iter())
                .filter(|skill| skill["enabled"] == true)
                .filter_map(|skill| skill["name"].as_str().map(str::to_owned))
                .collect()
        })
    });
    println!("codex skills in {}: {names:?}", folder.display());
    names
}

fn has(names: &[String], name: &str) -> bool {
    names
        .iter()
        .any(|listed| listed == name || listed.ends_with(&format!(":{name}")))
}

/// D8 and G17, with the real CLIs in throwaway homes: what the page turns on only in a
/// project, or off in a project while on everywhere, is exactly what a plain `claude` and a
/// plain `codex` started there have; the Codex file stays out of git; and a project Codex
/// doesn't trust is said so in plain words.
#[test]
#[ignore = "runs the installed claude and codex"]
fn plain_clis_have_what_the_page_shows() {
    let home = home("plain");
    let claude_dir = home.join("claude");
    let codex_home = home.join("codex");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let (project, sibling, wild) = (home.join("proj"), home.join("sib"), home.join("wild"));
    for dir in [&project, &sibling, &wild] {
        std::fs::create_dir_all(dir).unwrap();
        assert!(
            Command::new("git")
                .args(["init", "-q"])
                .arg(dir)
                .status()
                .unwrap()
                .success()
        );
    }
    // Codex trusts two of the three; the user's own Codex server is on everywhere.
    write(
        &codex_home.join("config.toml"),
        &format!(
            "[mcp_servers.mine]\ncommand = \"/usr/bin/python3\"\nargs = [\"{}\"]\n\n\
             [projects.\"{}\"]\ntrust_level = \"trusted\"\n\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            home.join("server.py").display(),
            project.display(),
            sibling.display()
        ),
    );
    write(&home.join("server.py"), MCP_SERVER);
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(key, _)| key == "PATH" || key == "USER" || key == "TMPDIR")
        .collect();
    vars.push(("HOME".into(), home.display().to_string()));
    vars.push(("CLAUDE_CONFIG_DIR".into(), claude_dir.display().to_string()));
    vars.push(("CODEX_HOME".into(), codex_home.display().to_string()));
    let roots = Roots::from_vars(vars.clone()).unwrap();
    let env: Vec<(String, String)> = roots.vars.clone().into_iter().collect();
    let library = Library::in_home(&home, PathBuf::from("/bin/brigadier"));
    let secrets = Kept::default();
    let projects = [project.clone(), sibling.clone(), wild.clone()];
    let failures = std::sync::Mutex::new(Vec::new());
    let report = |agent: Agent, step: PluginStep| {
        if let PluginStep::Failed { reason } = step {
            failures
                .lock()
                .unwrap()
                .push(format!("{agent:?}: {reason}"));
        }
    };
    let ctx = Ctx {
        roots: &roots,
        env: &env,
        library: &library,
        secrets: &secrets,
        projects: &projects,
        report: &report,
        binary: &|_| None,
    };
    let no_failures = || assert_eq!(failures.lock().unwrap().as_slice(), [] as [String; 0]);
    let server = |name: &str| {
        format!(
            r#"{{"mcpServers": {{"{name}": {{"command": "/usr/bin/python3", "args": ["{}"]}}}}}}"#,
            home.join("server.py").display()
        )
    };
    let tidy = home.join("src/tidy");
    write(
        &tidy.join("SKILL.md"),
        "---\nname: tidy\ndescription: Tidy up.\n---\nbody\n",
    );
    let neat = home.join("src/neat");
    write(
        &neat.join("SKILL.md"),
        "---\nname: neat\ndescription: Keep it neat.\n---\nbody\n",
    );

    // Only in this project: a server and a skill, "just me".
    add(
        &ctx,
        &server("here"),
        None,
        Some(&project),
        false,
        &AddOptions::default(),
    )
    .unwrap();
    add(
        &ctx,
        &tidy.display().to_string(),
        None,
        Some(&project),
        false,
        &AddOptions::default(),
    )
    .unwrap();
    no_failures();
    // On everywhere, then off in this project: a server and a skill from the Library, and the
    // user's own Codex server.
    add(
        &ctx,
        &server("every"),
        None,
        None,
        false,
        &AddOptions::default(),
    )
    .unwrap();
    add(
        &ctx,
        &neat.display().to_string(),
        None,
        None,
        false,
        &AddOptions::default(),
    )
    .unwrap();
    switch(&ctx, "lib:every", Some(&project), false, None).unwrap();
    switch(&ctx, "lib:neat", Some(&project), false, None).unwrap();
    switch(&ctx, "server:mine", Some(&project), false, None).unwrap();
    no_failures();

    // The page.
    use crate::manager::plugins::{ProjectCatalog, Unknown, build};
    let catalog = |path: &Path, id: &str| ProjectCatalog {
        id: id.to_owned(),
        name: id.to_owned(),
        path: path.to_owned(),
        catalog: Catalog::read(&roots, Some(path)),
    };
    let user = Catalog::read(&roots, None);
    let page = build(
        &user,
        &[
            catalog(&project, "proj"),
            catalog(&sibling, "sib"),
            catalog(&wild, "wild"),
        ],
        &library.entries(),
        &Unknown,
        None,
    );
    let item = |key: &str| {
        page.items
            .iter()
            .find(|item| item.key == key)
            .unwrap_or_else(|| panic!("{key} not on the page: {page:#?}"))
    };
    let here = project.display().to_string();
    for key in ["lib:here", "lib:tidy"] {
        assert!(!item(key).everywhere, "{key}");
        assert_eq!(item(key).on_in, std::slice::from_ref(&here), "{key}");
    }
    for key in ["lib:every", "lib:neat"] {
        assert!(item(key).everywhere, "{key}");
        assert_eq!(item(key).off_in, std::slice::from_ref(&here), "{key}");
    }
    let wild_line = page.projects.iter().find(|line| line.id == "wild").unwrap();
    assert_eq!(wild_line.notes.len(), 1, "{wild_line:?}");
    assert_eq!(
        wild_line.notes[0].message,
        "Codex only reads this project's settings once you trust the project."
    );

    // Plain `claude` and `codex`, in the project and next door.
    let claude_in = claude_servers(&vars, &project);
    assert!(
        has(&claude_in, "here") && !has(&claude_in, "every"),
        "{claude_in:?}"
    );
    let claude_next = claude_servers(&vars, &sibling);
    assert!(
        !has(&claude_next, "here") && has(&claude_next, "every"),
        "{claude_next:?}"
    );
    let codex_in = codex_servers(&vars, &project);
    assert!(has(&codex_in, "here"), "{codex_in:?}");
    assert!(
        !has(&codex_in, "every") && !has(&codex_in, "mine"),
        "{codex_in:?}"
    );
    let codex_next = codex_servers(&vars, &sibling);
    assert!(!has(&codex_next, "here"), "{codex_next:?}");
    assert!(
        has(&codex_next, "every") && has(&codex_next, "mine"),
        "{codex_next:?}"
    );
    let skills_in = claude_skills(&vars, &project);
    assert!(
        has(&skills_in, "tidy") && !has(&skills_in, "neat"),
        "{skills_in:?}"
    );
    let skills_next = claude_skills(&vars, &sibling);
    assert!(
        !has(&skills_next, "tidy") && has(&skills_next, "neat"),
        "{skills_next:?}"
    );
    let skills_in = codex_skills(&vars, &project);
    assert!(
        has(&skills_in, "tidy") && !has(&skills_in, "neat"),
        "{skills_in:?}"
    );
    let skills_next = codex_skills(&vars, &sibling);
    assert!(
        !has(&skills_next, "tidy") && has(&skills_next, "neat"),
        "{skills_next:?}"
    );

    // The Codex file is the user's alone.
    let ignored = plain(
        &vars,
        &project,
        "git",
        &["check-ignore", "-v", ".codex/config.toml"],
    );
    println!("git check-ignore -v .codex/config.toml: {ignored}");
    assert!(ignored.contains("info/exclude"), "{ignored}");

    // A project Codex doesn't trust: refused in the same plain words, Claude still done.
    assert!(!switch(&ctx, "lib:every", Some(&wild), false, None).unwrap());
    assert_eq!(
        failures.lock().unwrap().as_slice(),
        ["Codex: Codex only reads this project's settings once you trust the project."]
    );
    let _ = std::fs::remove_dir_all(home);
}

/// Does what `step` asks to the fixture's files, as the CLIs would: Claude's `enabledPlugins`
/// in its user or local settings, Codex's `enabled` keys.
fn simulate(home: &Path, step: &Step) {
    fn set_toml(file: &Path, key: &[String], value: Option<&toml::Value>) {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let mut doc: toml::Table = toml::from_str(&text).unwrap();
        let (last, path) = key.split_last().unwrap();
        let mut table = &mut doc;
        for part in path {
            table = table
                .entry(part.clone())
                .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                .as_table_mut()
                .unwrap();
        }
        match value {
            Some(value) => {
                table.insert(last.clone(), value.clone());
            }
            None => {
                table.remove(last);
            }
        }
        write(file, &toml::to_string(&doc).unwrap());
    }
    match step {
        Step::Run(call) if call.args.first().map(String::as_str) == Some("plugin") => {
            let on = match call.args[1].as_str() {
                "enable" => true,
                "disable" => false,
                other => panic!("not simulated: plugin {other}"),
            };
            let file = if call.args.contains(&"user".to_owned()) {
                home.join(".claude/settings.json")
            } else {
                call.cwd
                    .as_ref()
                    .unwrap()
                    .join(".claude/settings.local.json")
            };
            let mut settings: serde_json::Value = std::fs::read_to_string(&file)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_else(|| serde_json::json!({}));
            settings["enabledPlugins"][&call.args[2]] = on.into();
            write(&file, &settings.to_string());
        }
        Step::Run(call) => panic!("not simulated: {}", call.args.join(" ")),
        Step::UserConfig(TomlEdit { key, value }) => {
            set_toml(&home.join(".codex/config.toml"), key, value.as_ref());
        }
        Step::ProjectFile {
            file,
            edit: TomlEdit { key, value },
            ..
        } => set_toml(file, key, value.as_ref()),
    }
}

/// The fixture with a second project, both trusted by Codex.
fn two_projects(name: &str, claude: Option<bool>, codex: Option<bool>) -> (Fixture, [PathBuf; 2]) {
    let fx = Fixture::new(name, claude, codex);
    let other = fx.home.join("proj2");
    std::fs::create_dir_all(&other).unwrap();
    let config = fx.home.join(".codex/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    write(
        &config,
        &format!(
            "{text}\n[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
            other.display()
        ),
    );
    let projects = [fx.project.clone(), other];
    (fx, projects)
}

fn places_ctx<'a>(fx: &'a Fixture, projects: &'a [PathBuf]) -> Ctx<'a> {
    Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects,
        report: &|_, _| {},
        binary: &|_| None,
    }
}

/// Each agent's places now: (global, the projects' names it is on in).
fn places_now(ctx: &Ctx<'_>) -> Vec<(bool, Vec<String>)> {
    adapters()
        .into_iter()
        .map(|adapter| {
            let placed = placed(ctx, adapter, "tidy@brigadier");
            (
                placed.global,
                placed.on_in.iter().map(|path| folder_name(path)).collect(),
            )
        })
        .collect()
}

#[test]
fn the_picker_moves_an_item_between_global_and_projects_on_before_off() {
    let (fx, projects) = two_projects("places", Some(true), Some(true));
    let ctx = places_ctx(&fx, &projects);
    let done = std::sync::Mutex::new(Vec::new());
    let run = |adapter: &dyn AgentAdapter, steps: &[Step]| -> Result<(), String> {
        for step in steps {
            simulate(&fx.home, step);
        }
        done.lock().unwrap().extend(
            lines(steps)
                .into_iter()
                .map(|line| format!("{:?} {line}", adapter.agent())),
        );
        Ok(())
    };
    let taken = || std::mem::take(&mut *done.lock().unwrap());
    let both = |global: bool, on_in: &[&str]| {
        vec![
            (
                global,
                on_in
                    .iter()
                    .map(|name| (*name).to_owned())
                    .collect::<Vec<_>>()
            );
            2
        ]
    };
    assert_eq!(places_now(&ctx), both(true, &["proj", "proj2"]));

    // Global to one project: on there by itself first, then off for every project.
    set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &run).unwrap();
    assert_eq!(
        taken(),
        [
            "Claude claude plugin enable tidy@brigadier --scope local --json".to_owned(),
            format!("Codex project plugins.tidy@brigadier.enabled = {ON:?}"),
            "Claude claude plugin disable tidy@brigadier --scope user --json".to_owned(),
            format!("Codex user plugins.tidy@brigadier.enabled = {OFF:?}"),
        ]
    );
    assert_eq!(places_now(&ctx), both(false, &["proj"]));

    // To the other project: on in the new one before off in the old one.
    set_places_with(&ctx, "lib:tidy", Some(&projects[1..]), &run).unwrap();
    let moved = taken();
    let on_new = moved
        .iter()
        .position(|line| line.contains("enable"))
        .unwrap();
    let off_old = moved
        .iter()
        .position(|line| line.contains("disable"))
        .unwrap();
    assert!(on_new < off_old, "{moved:?}");
    assert_eq!(places_now(&ctx), both(false, &["proj2"]));

    // Several at once, then Global again: every project, with nothing left off.
    set_places_with(&ctx, "lib:tidy", Some(&projects), &run).unwrap();
    assert_eq!(places_now(&ctx), both(false, &["proj", "proj2"]));
    taken();
    set_places_with(&ctx, "lib:tidy", None, &run).unwrap();
    let global = taken();
    assert!(
        global
            .iter()
            .all(|line| !line.contains("disable") && !line.contains("Some(Boolean(false))")),
        "{global:?}"
    );
    assert_eq!(places_now(&ctx), both(true, &["proj", "proj2"]));
}

#[test]
fn a_move_that_fails_puts_every_agent_back_and_says_what_it_couldnt() {
    let (fx, projects) = two_projects("places-fail", Some(true), Some(true));
    let ctx = places_ctx(&fx, &projects);
    // Codex can't switch it off for every project; everything else goes.
    let fail_codex_off = |adapter: &dyn AgentAdapter, steps: &[Step]| -> Result<(), String> {
        for step in steps {
            if matches!(step, Step::UserConfig(TomlEdit { value, .. }) if *value == OFF) {
                return Err("codex said no".to_owned());
            }
            simulate(&fx.home, step);
        }
        let _ = adapter;
        Ok(())
    };
    assert_eq!(
        set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &fail_codex_off),
        Err("Nothing was changed: Codex: codex said no.".to_owned())
    );
    assert_eq!(
        places_now(&ctx),
        vec![(true, vec!["proj".to_owned(), "proj2".to_owned()]); 2]
    );

    // Claude Code can't be switched back on for every project either: what's left is said.
    let (fx, projects) = two_projects("places-left", Some(true), Some(true));
    let ctx = places_ctx(&fx, &projects);
    let stuck = |adapter: &dyn AgentAdapter, steps: &[Step]| -> Result<(), String> {
        for step in steps {
            if matches!(step, Step::UserConfig(TomlEdit { value, .. }) if *value == OFF) {
                return Err("codex said no".to_owned());
            }
            if matches!(step, Step::Run(call) if call.args[1] == "enable" && call.args.contains(&"user".to_owned()))
            {
                return Err("claude said no".to_owned());
            }
            simulate(&fx.home, step);
        }
        let _ = adapter;
        Ok(())
    };
    // It goes back on in each project by itself, as far as it can.
    assert_eq!(
        set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &stuck),
        Err(
            "Codex: codex said no. It couldn't all be put back: Claude Code has it on only in \
             proj and proj2, where it had it on in every project."
                .to_owned()
        )
    );
}

#[test]
fn only_a_library_item_has_places_and_it_needs_one() {
    let fx = Fixture::new("places-what", Some(true), Some(true));
    let projects = [fx.project.clone()];
    let ctx = places_ctx(&fx, &projects);
    let run = |_: &dyn AgentAdapter, _: &[Step]| -> Result<(), String> { panic!("nothing runs") };
    assert_eq!(
        set_places_with(&ctx, "server:pencil", None, &run),
        Err(ADD_TO_EVERY_AGENT_FIRST.to_owned())
    );
    assert_eq!(
        set_places_with(&ctx, "lib:tidy", Some(&[]), &run),
        Err("Pick a project, or Global.".to_owned())
    );
    assert!(set_places_with(&ctx, "lib:gone", None, &run).is_err());
}
