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
            Step::ClaudeFlags { flags, .. } => format!("claude flags {flags:?}"),
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
        claude_configs: &[],
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
        restored: vec![],
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
        claude_configs: &[],
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
    assert_eq!(
        fx.secrets.load("pg/original-0").unwrap()["PGPASS"],
        "hunter2"
    );

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

/// Sharing with the installed CLIs, in throwaway homes: a plain `claude` gets Codex's own
/// server and a plain Codex gets Claude Code's own skill, each through Brigadier's copy, and
/// both originals stay as they were.
#[test]
#[ignore = "runs the installed claude and codex"]
fn plain_clis_get_what_another_agent_has() {
    let home = home("plain-share");
    let claude_dir = home.join("claude");
    let codex_home = home.join("codex");
    let project = home.join("proj");
    std::fs::create_dir_all(&project).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .arg(&project)
            .status()
            .unwrap()
            .success()
    );
    write(&home.join("server.py"), MCP_SERVER);
    let config = format!(
        "[mcp_servers.mine]\ncommand = \"/usr/bin/python3\"\nargs = [\"{}\"]\n\n\
         [projects.\"{}\"]\ntrust_level = \"trusted\"\n",
        home.join("server.py").display(),
        project.display()
    );
    write(&codex_home.join("config.toml"), &config);
    write(
        &claude_dir.join("skills/tidy/SKILL.md"),
        "---\nname: tidy\ndescription: Tidy up.\n---\nbody\n",
    );
    let skill_before = std::fs::read(claude_dir.join("skills/tidy/SKILL.md")).unwrap();
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
    let projects = [project.clone()];
    let ctx = Ctx {
        roots: &roots,
        env: &env,
        library: &library,
        secrets: &secrets,
        projects: &projects,
        claude_configs: &[],
        report: &|_, _| {},
        binary: &|_| None,
    };
    crate::manager::plugins::share::sync(&ctx, None, true, &|adapter, steps| {
        ctx.run(adapter, steps)
    })
    .unwrap();
    let servers = claude_servers(&vars, &project);
    assert!(
        servers.iter().any(|name| name.contains("mine")),
        "{servers:?}"
    );
    let skills = codex_skills(&vars, &project);
    assert!(has(&skills, "tidy"), "{skills:?}");
    // The originals are as they were: Codex's server is still its own, the skill untouched.
    assert!(
        std::fs::read_to_string(codex_home.join("config.toml"))
            .unwrap()
            .contains("[mcp_servers.mine]")
    );
    assert_eq!(
        std::fs::read(claude_dir.join("skills/tidy/SKILL.md")).unwrap(),
        skill_before
    );
    let _ = std::fs::remove_dir_all(home);
}

/// Does what `step` asks to the fixture's files, as the CLIs would: Claude's `enabledPlugins`
/// in its user or local settings, Codex's `enabled` keys. As claude 2.1.310 does, a switch to
/// what holds there already (the user's switch counting in a project) writes nothing: it
/// answers `alreadyInGoalState`, which counts as done.
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
        Step::ClaudeFlags { files, flags } => {
            for file in if files.is_empty() {
                vec![home.join(".claude.json")]
            } else {
                files.clone()
            } {
                brigadier_registry::claude_config::set_off_flags(
                    &file,
                    flags,
                    std::time::Duration::ZERO,
                )
                .unwrap();
            }
        }
        // Adding the Library's marketplace changes nothing these tests read.
        Step::Run(call) if call.args.get(1).map(String::as_str) == Some("marketplace") => {}
        Step::Run(call) if call.program == "codex" && call.args[1] == "add" => {
            let cache = home.join(".codex/plugins/cache/brigadier/tidy/1.0.0");
            let entry = Library::in_home(home, PathBuf::from("/bin/brigadier"))
                .entry("tidy")
                .unwrap();
            write(
                &cache.join(".codex-plugin/plugin.json"),
                &std::fs::read_to_string(entry.dir.join(".codex-plugin/plugin.json")).unwrap(),
            );
            write(
                &cache.join("skills/tidy/SKILL.md"),
                "---\nname: tidy\ndescription: d\n---\n",
            );
        }
        Step::Run(call) if call.program == "codex" && call.args[1] == "remove" => {
            std::fs::remove_dir_all(home.join(".codex/plugins/cache/brigadier/tidy")).unwrap();
        }
        Step::Run(call) if call.args.first().map(String::as_str) == Some("plugin") => {
            let on = match call.args[1].as_str() {
                "enable" => true,
                "disable" => false,
                other => panic!("not simulated: plugin {other}"),
            };
            let user = home.join(".claude/settings.json");
            let file = if call.args.contains(&"user".to_owned()) {
                user.clone()
            } else {
                call.cwd
                    .as_ref()
                    .unwrap()
                    .join(".claude/settings.local.json")
            };
            let switch = |file: &Path| -> Option<bool> {
                let settings: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
                settings["enabledPlugins"][&call.args[2]].as_bool()
            };
            let holds = switch(&file).or_else(|| switch(&user)).unwrap_or(false);
            if holds == on {
                return;
            }
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
        claude_configs: &[],
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

    // Global to one project: on there by itself first, then off for every project. Claude
    // Code takes the project's switch on as done while every project's has it on, so it is
    // switched off there first (bug: "it is off in every project after the change").
    set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &run).unwrap();
    assert_eq!(
        taken(),
        [
            "Claude claude plugin disable tidy@brigadier --scope local --json".to_owned(),
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
fn some_projects_of_an_item_on_everywhere_stays_on_there_in_claude_code() {
    // pencil, on for every project in ~/.claude/settings.json, put on only in one project.
    let (fx, projects) = two_projects("places-goal-state", Some(true), Some(true));
    let ctx = places_ctx(&fx, &projects);
    let run = simulated(&fx);
    set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &run).unwrap();
    assert_eq!(places_now(&ctx), vec![(false, vec!["proj".to_owned()]); 2]);
    // Its own switch in the project, not the user's: it stays on there.
    let local: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.project.join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(local["enabledPlugins"]["tidy@brigadier"], true);
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
        // The projects' own switches it wrote stay: said so, not "Nothing was changed".
        Err(
            "Codex: codex said no. Each agent has it where it had it again, but Claude Code now \
             switches it on by itself in proj; Codex now switches it on by itself in proj."
                .to_owned()
        )
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
    assert!(set_places_with(&ctx, "server:pencil", None, &run).is_err());
    assert_eq!(
        set_places_with(&ctx, "lib:tidy", Some(&[]), &run),
        Err("Pick a project, or All projects.".to_owned())
    );
    assert!(set_places_with(&ctx, "lib:gone", None, &run).is_err());
}

/// The fixture's runner: each step done to its files, as the CLIs would.
fn simulated(fx: &Fixture) -> impl Fn(&dyn AgentAdapter, &[Step]) -> Result<(), String> + '_ {
    |_, steps| {
        for step in steps {
            simulate(&fx.home, step);
        }
        Ok(())
    }
}

/// Claude Code's own settings for `project` switch tidy on or off.
fn claude_here(project: &Path, on: bool) {
    write(
        &project.join(".claude/settings.local.json"),
        &serde_json::json!({"enabledPlugins": {"tidy@brigadier": on}}).to_string(),
    );
}

#[test]
fn a_project_with_an_install_of_its_own_is_switched_on_there_before_global_goes_off() {
    let (fx, projects) = two_projects("places-local-install", Some(true), Some(true));
    // Claude Code has an install of the project's own too, which follows the user's switch.
    let installs = fx.home.join(".claude/plugins/installed_plugins.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&installs).unwrap()).unwrap();
    let user = json["plugins"]["tidy@brigadier"][0].clone();
    let mut local = user.clone();
    local["scope"] = "local".into();
    local["projectPath"] = fx.project.display().to_string().into();
    json["plugins"]["tidy@brigadier"] = serde_json::json!([user, local]);
    write(&installs, &json.to_string());
    let ctx = places_ctx(&fx, &projects);
    let run = simulated(&fx);
    set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &run).unwrap();
    assert_eq!(places_now(&ctx), vec![(false, vec!["proj".to_owned()]); 2]);
}

#[test]
fn a_put_back_that_fails_stops_before_switching_anything_off() {
    let (fx, projects) = two_projects("places-stop", Some(false), Some(false));
    // On only in proj, by each agent's own project switch.
    claude_here(&fx.project, true);
    write(
        &fx.project.join(".codex/config.toml"),
        "[plugins.\"tidy@brigadier\"]\nenabled = true\n",
    );
    let ctx = places_ctx(&fx, &projects);
    // Codex can't switch it off in proj; Claude Code can't be put back on there either.
    let run = |_: &dyn AgentAdapter, steps: &[Step]| -> Result<(), String> {
        for step in steps {
            if matches!(step, Step::ProjectFile { edit: TomlEdit { value, .. }, .. } if *value == OFF)
            {
                return Err("codex said no".to_owned());
            }
            if matches!(step, Step::Run(call) if call.args[1] == "enable"
                && call.cwd.as_deref() == Some(fx.project.as_path()))
            {
                return Err("claude said no".to_owned());
            }
            simulate(&fx.home, step);
        }
        Ok(())
    };
    let err = set_places_with(&ctx, "lib:tidy", Some(&projects[1..]), &run).unwrap_err();
    assert!(err.starts_with("Codex: codex said no."), "{err}");
    // Claude Code couldn't go back on in proj, so it stays on in proj2: not off everywhere.
    assert_eq!(places_now(&ctx)[0], (false, vec!["proj2".to_owned()]));
}

#[test]
fn undo_puts_each_agent_back_exactly_missing_installs_and_project_offs_too() {
    let (fx, projects) = two_projects("places-undo", Some(true), None);
    claude_here(&projects[1], false);
    let ctx = places_ctx(&fx, &projects);
    let run = simulated(&fx);
    let before = every_placed(&ctx, "tidy@brigadier");
    set_places_with(&ctx, "lib:tidy", Some(&projects[..1]), &run).unwrap();
    assert_eq!(places_now(&ctx), vec![(false, vec!["proj".to_owned()]); 2]);
    change(&ctx, "tidy@brigadier", &before, &run).unwrap();
    // Claude Code on for every project but proj2; Codex without it, as before.
    assert_eq!(
        every_placed(&ctx, "tidy@brigadier"),
        [
            (
                Agent::Claude,
                Placed {
                    has: true,
                    global: true,
                    on_in: vec![fx.project.clone()],
                }
            ),
            (
                Agent::Codex,
                Placed {
                    has: false,
                    global: false,
                    on_in: Vec::new(),
                }
            ),
        ]
    );
}

#[test]
fn add_to_every_agent_keeps_the_projects_the_others_leave_it_off_in() {
    let (fx, projects) = two_projects("missing-offs", Some(true), None);
    claude_here(&projects[1], false);
    let ctx = places_ctx(&fx, &projects);
    let run = simulated(&fx);
    add_missing_with(&ctx, "lib:tidy", &run).unwrap();
    // Codex gets it for every project but proj2, like Claude Code.
    assert_eq!(places_now(&ctx), vec![(true, vec!["proj".to_owned()]); 2]);
}

#[test]
fn add_to_every_agent_installs_an_item_on_nowhere_switched_off() {
    let (fx, projects) = two_projects("missing-off", Some(false), None);
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
    add_missing_with(&ctx, "lib:tidy", &run).unwrap();
    let done = done.into_inner().unwrap();
    assert!(
        done.contains(&"Codex codex plugin add tidy@brigadier".to_owned()),
        "{done:?}"
    );
    assert_eq!(
        done.last().unwrap(),
        &format!("Codex user plugins.tidy@brigadier.enabled = {OFF:?}")
    );
    let codex = &every_placed(&ctx, "tidy@brigadier")[1].1;
    assert_eq!(
        (codex.has, codex.global, codex.on_in.len()),
        (true, false, 0)
    );
}

#[test]
fn review_legacy_server_restore_is_idempotent_and_retains_secrets() {
    let fx = Fixture::new("review-restored-server", None, None);
    fx.secrets
        .store("legacy/original-0", "TOKEN", &Secret::new("saved-value"))
        .unwrap();
    let original = Original::Server {
        agent: ProviderKind::Codex,
        name: "legacy".into(),
        project: Some(fx.project.clone()),
        transport: SavedTransport::Stdio {
            command: "/opt/mcp".into(),
            args: vec!["two words".into()],
            env: vec!["TOKEN".into()],
            cwd: None,
        },
        secrets: "legacy/original-0".into(),
    };
    let projects = [fx.project.clone()];
    let ctx = Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects: &projects,
        claude_configs: &[],
        report: &|_, _| {},
        binary: &|_| None,
    };
    original.restore(&ctx).unwrap();
    let file = fx.project.join(".codex/config.toml");
    let before = std::fs::read(&file).unwrap();
    // Restart in the gap between successful restore and its durable checkpoint.
    original.restore(&ctx).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), before);
    assert_eq!(
        fx.secrets.load("legacy/original-0").unwrap()["TOKEN"],
        "saved-value"
    );
    write(
        &file,
        "[mcp_servers.legacy]\ncommand = \"somebody-elses-command\"\n",
    );
    assert!(original.restore(&ctx).is_err());
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("somebody-elses-command")
    );
}

#[test]
fn the_plugins_alone_read_as_the_whole_catalog_lists_them() {
    let (fx, projects) = two_projects("plugins-alone", Some(true), Some(true));
    // Claude Code: off by the first project's settings; an install of the second's own.
    claude_here(&projects[0], false);
    let installs = fx.home.join(".claude/plugins/installed_plugins.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&installs).unwrap()).unwrap();
    let mut local = json["plugins"]["tidy@brigadier"][0].clone();
    local["scope"] = "local".into();
    local["projectPath"] = projects[1].display().to_string().into();
    json["plugins"]["tidy@brigadier"]
        .as_array_mut()
        .unwrap()
        .push(local);
    write(&installs, &json.to_string());
    // Codex: off by the second project's own file.
    write(
        &projects[1].join(".codex/config.toml"),
        "[plugins.\"tidy@brigadier\"]\nenabled = false\n",
    );
    let sorted = |plugins: Vec<brigadier_registry::Plugin>| {
        let mut plugins: Vec<String> = plugins
            .into_iter()
            .map(|plugin| format!("{plugin:?}"))
            .collect();
        plugins.sort();
        plugins
    };
    for project in [
        None,
        Some(projects[0].as_path()),
        Some(projects[1].as_path()),
    ] {
        assert_eq!(
            sorted(Catalog::plugins(&fx.roots, project)),
            sorted(Catalog::read(&fx.roots, project).plugins),
            "{project:?}"
        );
    }
}
fn bytes_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(bytes_under(&path));
        } else {
            found.insert(path, std::fs::read(entry.path()).unwrap());
        }
    }
    found
}

#[test]
fn add_remove_only_write_the_data_dir_library_and_catalog_resolves_both_agents() {
    let home = home("plain-library");
    let roots = roots(&home);
    write(&home.join(".claude/settings.json"), "{\"user\":true}\n");
    write(
        &home.join(".claude/skills/native/SKILL.md"),
        "native user skill\n",
    );
    write(&home.join(".codex/config.toml"), "# user config\n");
    std::fs::write(home.join(".codex/binary"), [0, 255, 1, 13, 10]).unwrap();
    write(
        &home.join(".agents/skills/own/SKILL.md"),
        "own shared skill",
    );
    let before_claude = bytes_under(&home.join(".claude"));
    let before_codex = bytes_under(&home.join(".codex"));
    let before_agents = bytes_under(&home.join(".agents"));
    let data_dir = home.join("data");
    let root = data_dir.join("library");
    let library = Library::at(root.clone(), PathBuf::from("/not-a-cli"));
    let secrets = Kept::default();
    let ctx = Ctx {
        roots: &roots,
        env: &[],
        library: &library,
        secrets: &secrets,
        projects: &[],
        claude_configs: &[],
        report: &|_, _| panic!("no CLI work"),
        binary: &|_| panic!("no CLI executable"),
    };
    let no_cli =
        |_: &dyn AgentAdapter, _: &[Step]| -> Result<(), String> { panic!("no adapter steps") };
    let paste = r#"{"mcpServers":{"docs":{"command":"/not-started","env":{"TOKEN":"pasted"}}}}"#;
    let options = AddOptions {
        values: BTreeMap::from([("TOKEN".into(), "typed".into())]),
    };
    let (keys, warning) = add(&ctx, paste, None, None, false, &options, &no_cli).unwrap();
    assert_eq!(keys, ["lib:docs"]);
    assert!(warning.is_none());
    let file = root.join("servers/docs.json");
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["mcpServers"]["docs"]["env"]["TOKEN"], "typed");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let source = home.join("src/tidy");
    write(
        &source.join("SKILL.md"),
        "---\nname: tidy\n---\nDo tidy work.\n",
    );
    write(&source.join("reference.bin"), "supporting content");
    add(
        &ctx,
        &source.to_string_lossy(),
        None,
        None,
        false,
        &AddOptions::default(),
        &no_cli,
    )
    .unwrap();
    assert!(root.join("skills/tidy/SKILL.md").is_file());
    assert!(root.join("skills/tidy/reference.bin").is_file());
    assert!(!root.join(".claude-plugin").exists());
    assert!(!root.join("plugins").exists());
    let rows = Catalog::read_rows_with_library(&roots, &[], &data_dir);
    for agent in [Agent::Claude, Agent::Codex] {
        let resolved = super::super::resolve::resolve(super::super::resolve::Context {
            agent,
            account_home: &roots,
            project_root: None,
            cwd: &home,
            trusted: true,
            role: super::super::resolve::Role::Thread,
            rows: &rows,
            choices: &super::super::choices::Choices::default(),
            gateway: None,
        });
        assert!(resolved.servers.iter().any(|server| server.name == "docs"));
        assert!(
            resolved
                .skills
                .iter()
                .any(|skill| skill.display == "tidy" && !skill.native)
        );
    }
    remove_library_with(&ctx, "docs", &no_cli).unwrap();
    remove_library_with(&ctx, "tidy", &no_cli).unwrap();
    assert!(!root.join("servers/docs.json").exists());
    assert!(!root.join("skills/tidy").exists());
    assert!(
        remove_library_with(&ctx, "native", &no_cli)
            .unwrap_err()
            .contains("only be turned off")
    );
    assert_eq!(bytes_under(&home.join(".claude")), before_claude);
    assert_eq!(bytes_under(&home.join(".codex")), before_codex);
    assert_eq!(bytes_under(&home.join(".agents")), before_agents);
    assert!(!home.join(".brigadier").exists());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn choices_for_a_plain_library_row_survive_a_fresh_read() {
    use super::super::choices::{Choice, Choices, Scope};
    let home = home("plain-choices");
    let roots = roots(&home);
    let data = home.join("data");
    write(&data.join("library/skills/tidy/SKILL.md"), "tidy");
    let rows = Catalog::read_rows_with_library(&roots, &[], &data);
    let row = rows.iter().find(|row| row.name == "tidy").unwrap();
    let scope = Scope::Projects(vec![home.join("project-a"), home.join("project-b")]);
    let choice = Choice { on: false, scope };
    Choices(BTreeMap::from([(row.id.clone(), choice.clone())]))
        .save(&data)
        .unwrap();
    let rows = Catalog::read_rows_with_library(&roots, &[], &data);
    let row = rows.iter().find(|row| row.name == "tidy").unwrap();
    assert_eq!(Choices::load(&data).unwrap().choice(row), choice);
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn add_shared_refuses_without_changing_project_or_cli_files() {
    let fx = Fixture::new("add-shared-refused", None, None);
    let before = bytes_under(&fx.home);
    let projects = [fx.project.clone()];
    let ctx = places_ctx(&fx, &projects);
    assert!(
        add(
            &ctx,
            "https://example.test/mcp",
            Some("example"),
            Some(&projects),
            true,
            &AddOptions::default(),
            &|_, _| panic!("no CLI")
        )
        .unwrap_err()
        .contains("no longer supported")
    );
    assert_eq!(bytes_under(&fx.home), before);
}

#[test]
fn git_add_copies_plain_skill_without_cli_configuration_changes() {
    let home = home("plain-git");
    write(&home.join(".claude/settings.json"), "{\"untouched\":true}");
    write(&home.join(".codex/config.toml"), "# untouched\n");
    let before_claude = bytes_under(&home.join(".claude"));
    let before_codex = bytes_under(&home.join(".codex"));
    let repo = home.join("remote");
    write(
        &repo.join("SKILL.md"),
        "---\nname: git-skill\n---\nGit instructions\n",
    );
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let root = home.join("data/library");
    let item = Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::Git {
            url: repo.to_string_lossy().into_owned(),
            rev: None,
            subdir: None,
        },
    });
    assert_eq!(add_plain(&root, &[item], None).unwrap(), ["lib:git-skill"]);
    assert_eq!(
        std::fs::read(root.join("skills/git-skill/SKILL.md")).unwrap(),
        std::fs::read(repo.join("SKILL.md")).unwrap()
    );
    assert!(root.join("origins/git-skill.json").is_file());
    assert!(!root.join("skills/git-skill/.git").exists());
    remove_plain(&root, "git-skill").unwrap();
    assert!(repo.join("SKILL.md").exists());
    assert_eq!(bytes_under(&home.join(".claude")), before_claude);
    assert_eq!(bytes_under(&home.join(".codex")), before_codex);
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn migrated_library_packages_are_catalog_sources_and_only_their_library_copy_is_removed() {
    let home = home("plain-migrated");
    let data = home.join("data");
    let root = data.join("library");
    let library = Library::at(root.clone(), PathBuf::from("/not-a-cli"));
    let source = home.join("original");
    write(
        &source.join("SKILL.md"),
        "---\nname: migrated\n---\nOriginal instructions\n",
    );
    let source_bytes = bytes_under(&source);
    let item = Item::Skill(SkillSpec {
        name: Some("migrated".into()),
        source: SkillSource::Folder(source.clone()),
    });
    library
        .pack(
            "migrated",
            &item,
            Some(&source),
            &[],
            false,
            &Kept::default(),
        )
        .unwrap();
    let rows = Catalog::read_rows_with_library(&roots(&home), &[], &data);
    let row = rows.iter().find(|row| row.name == "migrated").unwrap();
    assert!(
        row.definitions
            .iter()
            .any(|definition| definition.provenance.file.starts_with(&root))
    );
    remove_plain(&root, "migrated").unwrap();
    assert!(
        !Catalog::read_rows_with_library(&roots(&home), &[], &data)
            .iter()
            .any(|row| row.name == "migrated")
    );
    assert_eq!(bytes_under(&source), source_bytes);
    let _ = std::fs::remove_dir_all(home);
}
