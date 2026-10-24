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
    };
    for key in ["skill:grill", "other:desktop:figma"] {
        assert_eq!(
            switch(&ctx, key, Some(&fx.project), false, None),
            Err(USE_EVERYWHERE_FIRST.to_owned())
        );
    }
    assert!(switch(&ctx, "nonsense", None, true, None).is_err());
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
