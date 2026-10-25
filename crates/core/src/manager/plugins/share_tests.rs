//! Sharing on stub homes: the runner does to the stub's files what each CLI's own plugin
//! commands would, and fails the test on any command that would touch an agent's own entry.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_registry::adapter::{AgentAdapter, Step, TomlEdit};
use brigadier_registry::library::{Library, SecretSink};
use brigadier_registry::{Roots, Secret};

use super::*;
use crate::manager::plugins::model::library_plugin;
use crate::manager::plugins::ops::{PluginStep, Secrets};
use crate::manager::plugins::{
    OnlyInReason, PluginProblem, PluginRemoval, PluginScope, PluginsView, Unknown, build,
};

/// A Keychain that remembers, in memory.
#[derive(Default)]
struct Kept(
    Mutex<BTreeMap<String, BTreeMap<String, String>>>,
    std::sync::atomic::AtomicBool,
    std::sync::atomic::AtomicBool,
);

impl SecretSink for Kept {
    fn store(&self, server: &str, name: &str, value: &Secret) -> Result<(), String> {
        if self.2.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err("injected Keychain write failure".into());
        }
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
        if self.1.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err("injected Keychain delete failure".into());
        }
        self.0.lock().unwrap().remove(server);
        Ok(())
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A stub home with projects (each trusted by Codex), its Library and Keychain, and a log of
/// every command the runner was given.
struct Home {
    home: PathBuf,
    roots: Roots,
    library: Library,
    secrets: Kept,
    projects: Vec<PathBuf>,
    log: Mutex<Vec<String>>,
    /// A command line containing this fails.
    fail_on: Mutex<Option<String>>,
}

impl Home {
    fn new(name: &str, projects: &[&str]) -> Self {
        let dir =
            std::env::temp_dir().join(format!("brigadier-share-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let home = std::fs::canonicalize(dir).unwrap();
        let projects: Vec<PathBuf> = projects.iter().map(|name| home.join(name)).collect();
        let mut config = String::new();
        for project in &projects {
            std::fs::create_dir_all(project).unwrap();
            config.push_str(&format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n\n",
                project.display()
            ));
        }
        write(&home.join(".codex/config.toml"), &config);
        Self {
            roots: Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap(),
            library: Library::in_home(&home, PathBuf::from("/bin/brigadier")),
            home,
            secrets: Kept::default(),
            projects,
            log: Mutex::new(Vec::new()),
            fail_on: Mutex::new(None),
        }
    }

    fn with<T>(&self, work: impl FnOnce(&Ctx<'_>, Runner<'_>) -> T) -> T {
        let ctx = Ctx {
            roots: &self.roots,
            env: &[],
            library: &self.library,
            secrets: &self.secrets,
            projects: &self.projects,
            report: &|_: Agent, _: PluginStep| {},
            binary: &|_| None,
        };
        let run = |adapter: &dyn AgentAdapter, steps: &[Step]| -> Result<(), String> {
            for step in steps {
                let line = line(step);
                if let Some(bad) = &*self.fail_on.lock().unwrap()
                    && line.contains(bad.as_str())
                {
                    return Err(format!("{} said no", adapter.label()));
                }
                self.log.lock().unwrap().push(line);
                self.simulate(step);
            }
            Ok(())
        };
        work(&ctx, &run)
    }

    fn sync(&self) -> Result<(), String> {
        self.with(|ctx, run| sync(ctx, None, true, run))
    }

    /// The commands run since the last call.
    fn ran(&self) -> Vec<String> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    fn view(&self) -> PluginsView {
        self.with(|ctx, _| {
            let sharing = Sharing::load(ctx.library).unwrap();
            let catalogs = Catalogs::read(ctx);
            let model = catalogs.model(ctx, &sharing);
            build(&model, &catalogs.projects, &sharing, &Unknown, &[], None).view
        })
    }

    fn item(&self, key: &str) -> crate::manager::plugins::PluginItemView {
        let view = self.view();
        view.items
            .iter()
            .find(|item| item.key == key)
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "no {key} in {:?}",
                    view.items.iter().map(|i| &i.key).collect::<Vec<_>>()
                )
            })
    }

    fn installed(&self, agent: Agent, name: &str) -> bool {
        adapter(agent).installed(&self.roots, &plugin_id(name))
    }

    /// Where `agent` has Library plugin `name` on: (every project, the projects).
    fn placed(&self, agent: Agent, name: &str) -> Placed {
        self.with(|ctx, _| ops::placed(ctx, adapter(agent), &plugin_id(name)))
    }

    fn sharing(&self) -> Sharing {
        Sharing::load(&self.library).unwrap()
    }

    /// Does what `step` asks to the stub's files, as each CLI would.
    fn simulate(&self, step: &Step) {
        let home = &self.home;
        match step {
            Step::Run(call) if call.args.get(1).map(String::as_str) == Some("marketplace") => {}
            Step::Run(call) if call.args.first().map(String::as_str) == Some("mcp") => {
                panic!(
                    "an agent's own entry was touched: {} {}",
                    call.program,
                    call.args.join(" ")
                )
            }
            Step::Run(call) if call.program == "claude" => {
                let (verb, id) = (call.args[1].as_str(), call.args[2].clone());
                let scope = call
                    .args
                    .iter()
                    .position(|arg| arg == "--scope")
                    .map_or("user", |at| call.args[at + 1].as_str());
                let settings = match scope {
                    "user" => home.join(".claude/settings.json"),
                    _ => call
                        .cwd
                        .as_ref()
                        .unwrap()
                        .join(".claude/settings.local.json"),
                };
                let installs = home.join(".claude/plugins/installed_plugins.json");
                let mut json: serde_json::Value = std::fs::read_to_string(&installs)
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
                    .unwrap_or_else(|| serde_json::json!({"version": 2, "plugins": {}}));
                let set = |on: Option<bool>| {
                    let mut value: serde_json::Value = std::fs::read_to_string(&settings)
                        .ok()
                        .and_then(|text| serde_json::from_str(&text).ok())
                        .unwrap_or_else(|| serde_json::json!({}));
                    match on {
                        Some(on) => value["enabledPlugins"][&id] = on.into(),
                        None => {
                            if let Some(map) = value["enabledPlugins"].as_object_mut() {
                                map.remove(&id);
                            }
                        }
                    }
                    write(&settings, &value.to_string());
                };
                match verb {
                    "install" => {
                        let name = library_plugin(&id).unwrap();
                        let entry = self.library.entry(name).unwrap();
                        let mut install = serde_json::json!({
                            "scope": scope, "installPath": entry.dir, "version": entry.version
                        });
                        if scope != "user" {
                            install["projectPath"] =
                                call.cwd.as_ref().unwrap().display().to_string().into();
                        }
                        let list = json["plugins"][&id].as_array().cloned().unwrap_or_default();
                        let mut list: Vec<serde_json::Value> = list;
                        list.push(install);
                        json["plugins"][&id] = list.into();
                        write(&installs, &json.to_string());
                        set(Some(true));
                    }
                    "uninstall" => {
                        if let Some(list) = json["plugins"][&id].as_array_mut() {
                            list.retain(|install| install["scope"] != scope);
                        }
                        write(&installs, &json.to_string());
                        set(None);
                    }
                    "enable" => set(Some(true)),
                    "disable" => set(Some(false)),
                    "update" => {}
                    other => panic!("not simulated: claude plugin {other}"),
                }
            }
            Step::Run(call) if call.program == "codex" => {
                let id = call.args[2].clone();
                let name = library_plugin(&id).unwrap().to_owned();
                let config = home.join(".codex/config.toml");
                match call.args[1].as_str() {
                    "add" => {
                        let entry = self.library.entry(&name).unwrap();
                        let cache = home.join(format!(".codex/plugins/cache/brigadier/{name}"));
                        let _ = std::fs::remove_dir_all(&cache);
                        copy(&entry.dir, &cache.join(&entry.version));
                        let mut doc = read_toml(&config);
                        let plugins = doc
                            .entry("plugins")
                            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
                            .as_table_mut()
                            .unwrap();
                        plugins.entry(id).or_insert_with(|| {
                            toml::Value::Table(toml::Table::from_iter([(
                                "enabled".to_owned(),
                                toml::Value::Boolean(true),
                            )]))
                        });
                        write(&config, &toml::to_string(&doc).unwrap());
                    }
                    "remove" => {
                        let _ = std::fs::remove_dir_all(
                            home.join(format!(".codex/plugins/cache/brigadier/{name}")),
                        );
                        let mut doc = read_toml(&config);
                        if let Some(plugins) =
                            doc.get_mut("plugins").and_then(toml::Value::as_table_mut)
                        {
                            plugins.remove(&id);
                        }
                        write(&config, &toml::to_string(&doc).unwrap());
                    }
                    other => panic!("not simulated: codex plugin {other}"),
                }
            }
            Step::Run(call) => panic!("not simulated: {} {}", call.program, call.args.join(" ")),
            Step::UserConfig(TomlEdit { key, value }) => {
                // Codex's own config writer, for its plugins' and servers' switches only.
                assert!(
                    key.last().map(String::as_str) == Some("enabled"),
                    "a user config edit that isn't a switch: {key:?}"
                );
                set_toml(&home.join(".codex/config.toml"), key, value.as_ref());
            }
            Step::ProjectFile {
                file,
                edit: TomlEdit { key, value },
                ..
            } => {
                assert!(
                    self.projects
                        .iter()
                        .any(|project| *file == project.join(".codex/config.toml")),
                    "a project file edit outside a project's .codex/config.toml: {}",
                    file.display()
                );
                set_toml(file, key, value.as_ref());
            }
        }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn read_toml(file: &Path) -> toml::Table {
    toml::from_str(&std::fs::read_to_string(file).unwrap_or_default()).unwrap()
}

fn set_toml(file: &Path, key: &[String], value: Option<&toml::Value>) {
    let mut doc = read_toml(file);
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

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn line(step: &Step) -> String {
    match step {
        Step::Run(call) => format!("{} {}", call.program, call.args.join(" ")),
        Step::UserConfig(TomlEdit { key, value }) => format!("user {} = {value:?}", key.join(".")),
        Step::ProjectFile {
            edit: TomlEdit { key, value },
            file,
            ..
        } => format!("project {} {} = {value:?}", file.display(), key.join(".")),
    }
}

/// Every file under `dir`, with its bytes.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    if dir.is_file() {
        out.push((dir.to_owned(), std::fs::read(dir).unwrap()));
        return out;
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            out.extend(snapshot(&entry.path()));
        }
    }
    out.sort();
    out
}

const CLAUDE_DOCS: &str = r#"{"mcpServers": {"docs": {"command": "/opt/docs-mcp", "args": ["--port", "1"], "env": {"TOKEN": "abc123"}}}}"#;

fn tidy(home: &Path, body: &str) -> PathBuf {
    let dir = home.join(".claude/skills/tidy");
    write(
        &dir.join("SKILL.md"),
        &format!("---\nname: tidy\ndescription: Tidies up.\n---\n{body}\n"),
    );
    dir
}

#[test]
fn shares_to_lacking_agents_only_and_leaves_originals_byte_identical() {
    let home = Home::new("lacking", &["proj"]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    let skill = tidy(&home.home, "body");
    let before = (snapshot(&home.home.join(".claude.json")), snapshot(&skill));
    home.sync().unwrap();
    // Codex lacked both: it has Brigadier's copies. Claude Code has its own: no copy.
    for name in ["docs", "tidy"] {
        assert!(home.installed(Agent::Codex, name), "{name}");
        assert!(!home.installed(Agent::Claude, name), "{name}");
    }
    assert!(home.placed(Agent::Codex, "docs").global);
    assert_eq!(
        (snapshot(&home.home.join(".claude.json")), snapshot(&skill)),
        before
    );
    // The value went to the Keychain only.
    assert_eq!(home.secrets.load("docs/docs").unwrap()["TOKEN"], "abc123");
    for (path, bytes) in snapshot(&home.home.join(".brigadier")) {
        assert!(
            !String::from_utf8_lossy(&bytes).contains("abc123"),
            "{}",
            path.display()
        );
    }
    let docs = home.item("server:docs");
    assert_eq!(docs.removal, PluginRemoval::Copies);
    assert_eq!(docs.problem, None);
    assert!(
        docs.locked
            .as_deref()
            .unwrap()
            .starts_with("Claude Code keeps it on")
    );
    assert_eq!(docs.detail, "/opt/docs-mcp --port 1");
    // The second pass finds nothing to do.
    home.ran();
    home.sync().unwrap();
    assert_eq!(home.ran(), Vec::<String>::new());
}

#[test]
fn no_copy_for_an_agent_that_has_it_by_name_or_by_address_and_a_conflict_shares_neither() {
    let home = Home::new("dedupe", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    // Codex: the same server by another name, and another setup called "notes".
    let config = home.home.join(".codex/config.toml");
    write(
        &config,
        "[mcp_servers.docs-alt]\ncommand = \"/opt/docs-mcp\"\nargs = [\"--port\", \"1\"]\n\n\
         [mcp_servers.notes]\ncommand = \"/opt/notes-a\"\n",
    );
    let claude = r#"{"mcpServers": {
        "docs": {"command": "/opt/docs-mcp", "args": ["--port", "1"]},
        "notes": {"command": "/opt/notes-b"}}}"#;
    write(&home.home.join(".claude.json"), claude);
    home.sync().unwrap();
    assert!(
        home.library.entries().is_empty(),
        "{:?}",
        home.library.entries()
    );
    assert!(home.item("server:docs").problem.is_none());
    assert!(matches!(
        home.item("server:notes").problem,
        Some(PluginProblem::Conflict { .. })
    ));
}

#[test]
fn twins_of_one_endpoint_get_one_copy() {
    let home = Home::new("twins", &[]);
    write(
        &home.home.join(".claude.json"),
        r#"{"mcpServers": {
            "a": {"type": "http", "url": "https://mcp.example.com/mcp"},
            "b": {"type": "http", "url": "https://mcp.example.com/mcp"}}}"#,
    );
    home.sync().unwrap();
    assert_eq!(home.library.entries().len(), 1);
}

#[test]
fn a_copy_goes_when_its_agent_gets_its_own_original() {
    let home = Home::new("own-later", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    home.sync().unwrap();
    assert!(home.installed(Agent::Codex, "docs"));
    let config = home.home.join(".codex/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    write(
        &config,
        &format!(
            "{text}\n[mcp_servers.docs]\ncommand = \"/opt/docs-mcp\"\nargs = [\"--port\", \"1\"]\nenv = {{ TOKEN = \"abc123\" }}\n"
        ),
    );
    home.sync().unwrap();
    assert!(!home.installed(Agent::Codex, "docs"));
}

#[test]
fn an_edited_original_repacks_and_updates_the_copies() {
    let home = Home::new("edit", &[]);
    tidy(&home.home, "first");
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    home.sync().unwrap();
    let version = home.library.entry("tidy").unwrap().version;
    home.ran();
    tidy(&home.home, "second");
    home.sync().unwrap();
    let entry = home.library.entry("tidy").unwrap();
    assert_ne!(entry.version, version);
    assert!(
        std::fs::read_to_string(entry.dir.join("skills/tidy/SKILL.md"))
            .unwrap()
            .contains("second")
    );
    assert!(
        home.ran()
            .contains(&"codex plugin add tidy@brigadier".to_owned())
    );
    // A value alone: kept again, no CLI update.
    write(
        &home.home.join(".claude.json"),
        &CLAUDE_DOCS.replace("abc123", "rotated"),
    );
    home.sync().unwrap();
    assert_eq!(home.secrets.load("docs/docs").unwrap()["TOKEN"], "rotated");
    assert_eq!(home.ran(), Vec::<String>::new());
}

#[test]
fn a_removed_original_takes_its_copies_only_after_two_clean_reads() {
    let home = Home::new("gone", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    home.sync().unwrap();
    // Unreadable (an editor mid-save, a truncated file): never a removal.
    write(&home.home.join(".claude.json"), "");
    for _ in 0..3 {
        home.sync().unwrap();
    }
    std::fs::remove_file(home.home.join(".claude.json")).unwrap();
    home.sync().unwrap();
    assert!(home.installed(Agent::Codex, "docs"));
    // Read cleanly without it, twice: the copies go, with their values.
    write(&home.home.join(".claude.json"), r#"{"mcpServers": {}}"#);
    home.sync().unwrap();
    assert!(home.installed(Agent::Codex, "docs"));
    home.sync().unwrap();
    assert!(!home.installed(Agent::Codex, "docs"));
    assert!(home.library.entry("docs").is_none());
    assert!(home.secrets.load("docs/docs").unwrap().is_empty());
    assert!(home.sharing().declined.is_empty());
}

#[test]
fn remove_keeps_the_original_is_remembered_and_share_again_undoes_it() {
    let home = Home::new("remove", &[]);
    let skill = tidy(&home.home, "body");
    let before = snapshot(&skill);
    home.sync().unwrap();
    home.with(|ctx, run| remove(ctx, "skill:tidy", run))
        .unwrap();
    assert!(!home.installed(Agent::Codex, "tidy"));
    assert!(home.library.entry("tidy").is_none());
    assert_eq!(snapshot(&skill), before);
    home.sync().unwrap();
    assert!(
        !home.installed(Agent::Codex, "tidy"),
        "shared again after Remove"
    );
    let item = home.item("skill:tidy");
    assert!(matches!(
        item.problem,
        Some(PluginProblem::OnlyIn {
            reason: OnlyInReason::Removed,
            ..
        })
    ));
    assert!(item.can_share);
    assert_eq!(item.removal, PluginRemoval::None);
    home.with(|ctx, run| share_again(ctx, "skill:tidy", run))
        .unwrap();
    assert!(home.installed(Agent::Codex, "tidy"));
    assert!(home.sharing().declined.is_empty());
}

#[test]
fn an_interrupted_remove_is_finished_and_never_shared_again() {
    let home = Home::new("remove-cut", &[]);
    tidy(&home.home, "body");
    home.sync().unwrap();
    *home.fail_on.lock().unwrap() = Some("codex plugin remove".to_owned());
    assert!(
        home.with(|ctx, run| remove(ctx, "skill:tidy", run))
            .is_err()
    );
    let sharing = home.sharing();
    assert_eq!(sharing.declined.len(), 1);
    assert_eq!(sharing.pending_removal, ["tidy"]);
    *home.fail_on.lock().unwrap() = None;
    home.sync().unwrap();
    assert!(!home.installed(Agent::Codex, "tidy"));
    assert!(home.library.entry("tidy").is_none());
    assert!(home.sharing().pending_removal.is_empty());
}

#[test]
fn remove_takes_a_brigadier_added_item_from_every_agent() {
    let home = Home::new("remove-added", &[]);
    home.with(|ctx, run| {
        let skill = home.home.join("src/neat");
        write(&skill.join("SKILL.md"), "---\nname: neat\n---\n");
        ops::add(
            ctx,
            &skill.display().to_string(),
            None,
            None,
            false,
            &ops::AddOptions::default(),
            run,
        )
    })
    .unwrap();
    assert!(home.installed(Agent::Claude, "neat") && home.installed(Agent::Codex, "neat"));
    assert_eq!(home.item("lib:neat").removal, PluginRemoval::Everything);
    home.with(|ctx, run| remove(ctx, "lib:neat", run)).unwrap();
    assert!(!home.installed(Agent::Claude, "neat") && !home.installed(Agent::Codex, "neat"));
    assert!(home.library.entry("neat").is_none());
    assert!(home.sharing().declined.is_empty());
}

#[test]
fn scope_and_switch_move_a_codex_original_and_its_copy_together() {
    let home = Home::new("scope", &["one", "two"]);
    let config = home.home.join(".codex/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    write(
        &config,
        &format!("[mcp_servers.pg]\ncommand = \"/opt/pg-mcp\"\n\n{text}"),
    );
    home.sync().unwrap();
    assert!(home.placed(Agent::Claude, "pg").global);
    let one = home.projects[0].clone();
    let key = "server:pg";
    home.with(|ctx, run| set_scope(ctx, key, Some(std::slice::from_ref(&one)), run))
        .unwrap();
    // Codex: off in two by its own file; Claude Code's copy on only in one.
    assert_ne!(codex_switch_in(&home.projects[1], "pg"), Some(true));
    assert_eq!(
        read_toml(&config)["mcp_servers"]["pg"]["enabled"].as_bool(),
        Some(false)
    );
    let claude = home.placed(Agent::Claude, "pg");
    assert_eq!(
        (claude.global, claude.on_in.clone()),
        (false, vec![one.clone()])
    );
    assert_eq!(
        home.item(key).scope,
        PluginScope::SomeProjects {
            project_ids: vec![one.display().to_string()],
            folders: Vec::new()
        }
    );
    // Off for every agent, keeping "only one"; on again, back to only one.
    home.with(|ctx, run| switch(ctx, key, false, run)).unwrap();
    let off = home.item(key);
    assert!(!off.on);
    assert!(
        !home.placed(Agent::Claude, "pg").global
            && home.placed(Agent::Claude, "pg").on_in.is_empty()
    );
    assert!(matches!(off.scope, PluginScope::SomeProjects { .. }));
    home.with(|ctx, run| switch(ctx, key, true, run)).unwrap();
    let on = home.item(key);
    assert!(on.on);
    assert_eq!(on.scope, off.scope);
    // All projects again.
    home.with(|ctx, run| set_scope(ctx, key, None, run))
        .unwrap();
    assert_eq!(home.item(key).scope, PluginScope::AllProjects);
}

#[test]
fn locked_rows_refuse_switch_and_scope_and_touch_nothing() {
    let home = Home::new("locked", &["proj"]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    let skill = tidy(&home.home, "body");
    home.sync().unwrap();
    let before = (snapshot(&home.home.join(".claude.json")), snapshot(&skill));
    home.ran();
    for key in ["server:docs", "skill:tidy"] {
        let why = home.item(key).locked.unwrap();
        assert_eq!(
            home.with(|ctx, run| switch(ctx, key, false, run)),
            Err(why.clone())
        );
        let one = home.projects.clone();
        assert_eq!(
            home.with(|ctx, run| set_scope(ctx, key, Some(&one), run)),
            Err(why)
        );
    }
    assert_eq!(home.ran(), Vec::<String>::new());
    assert_eq!(
        (snapshot(&home.home.join(".claude.json")), snapshot(&skill)),
        before
    );
}

#[test]
fn synced_and_system_skills_are_neither_shown_nor_shared() {
    let home = Home::new("hidden", &[]);
    write(
        &home
            .home
            .join(".claude/skills/synced/org_acct/cloud/SKILL.md"),
        "---\nname: cloud\n---\n",
    );
    write(
        &home.home.join(".codex/skills/.system/builtin/SKILL.md"),
        "---\nname: builtin\n---\n",
    );
    home.sync().unwrap();
    assert!(home.library.entries().is_empty());
    assert!(home.view().items.is_empty());
}

#[test]
fn a_team_server_is_shared_in_its_project_only_and_locked() {
    let home = Home::new("team", &["proj", "other"]);
    let project = home.projects[0].clone();
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {"team": {"command": "/opt/team-mcp"}}}"#,
    );
    home.sync().unwrap();
    let codex = home.placed(Agent::Codex, "team");
    assert!(codex.has && !codex.global);
    assert_eq!(codex.on_in, std::slice::from_ref(&project));
    let item = home.item("server:team");
    assert!(
        item.locked
            .as_deref()
            .unwrap()
            .starts_with("Shared with your team")
    );
}

#[test]
fn a_server_reading_variables_or_a_disabled_one_is_kept_as_it_is() {
    let home = Home::new("templated", &[]);
    write(
        &home.home.join(".claude.json"),
        r#"{"mcpServers": {
            "tmpl": {"command": "/opt/t", "env": {"TOKEN": "${MY_TOKEN}"}},
            "quiet": {"command": "/opt/q", "disabled": true}}}"#,
    );
    home.sync().unwrap();
    // A ${VAR} server isn't copied; the row says why, and shows the text as written.
    let tmpl = home.item("server:tmpl");
    assert!(matches!(
        tmpl.problem,
        Some(PluginProblem::OnlyIn {
            reason: OnlyInReason::CantShare,
            ..
        })
    ));
    assert!(
        !home
            .library
            .entries()
            .iter()
            .any(|entry| entry.name == "tmpl")
    );
    // Claude Code's switched-off server: still its own (no copy for it), off for Codex too.
    assert!(!home.installed(Agent::Claude, "quiet"));
    let codex = home.placed(Agent::Codex, "quiet");
    assert!(codex.has && !codex.global && codex.on_in.is_empty());
    assert!(!home.item("server:quiet").on);
}

#[test]
fn a_failed_update_is_tried_again() {
    let home = Home::new("retry", &[]);
    tidy(&home.home, "first");
    home.sync().unwrap();
    tidy(&home.home, "second");
    *home.fail_on.lock().unwrap() = Some("codex plugin add".to_owned());
    home.sync().unwrap();
    assert!(home.sharing().failed.contains_key("skill:tidy"));
    let version = home.library.entry("tidy").unwrap().version;
    let applied = |home: &Home| home.sharing().items["tidy"].applied.get("codex").cloned();
    assert_ne!(applied(&home), Some(version.clone()));
    *home.fail_on.lock().unwrap() = None;
    home.ran();
    home.sync().unwrap();
    assert!(
        home.ran()
            .contains(&"codex plugin add tidy@brigadier".to_owned())
    );
    assert_eq!(applied(&home), Some(version));
    assert!(home.sharing().failed.is_empty());
}

#[test]
fn a_skill_save_never_overwrites_an_edit_made_meanwhile_and_copies_follow() {
    let home = Home::new("save", &[]);
    let skill = tidy(&home.home, "first");
    home.sync().unwrap();
    let details = home.with(|ctx, _| details(ctx, "skill:tidy")).unwrap();
    let [file] = details.skills.as_slice() else {
        panic!("{:?}", details.skills);
    };
    assert_eq!(file.read_only, None);
    let opened = file.contents.clone();
    // Someone edits it meanwhile.
    tidy(&home.home, "theirs");
    let theirs = std::fs::read_to_string(skill.join("SKILL.md")).unwrap();
    let saved = home
        .with(|ctx, _| write_skill(ctx, "skill:tidy", &file.path, "mine", &opened))
        .unwrap();
    assert_eq!(
        saved,
        Saved::ChangedSinceOpened {
            current: theirs.clone()
        }
    );
    assert_eq!(
        std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        theirs
    );
    // Saved over what it holds now: the copy follows.
    let mine = theirs.replace("theirs", "mine");
    assert_eq!(
        home.with(|ctx, _| write_skill(ctx, "skill:tidy", &file.path, &mine, &theirs)),
        Ok(Saved::Saved)
    );
    home.with(|ctx, run| follow_skill(ctx, "skill:tidy", &file.path, run))
        .unwrap();
    let entry = home.library.entry("tidy").unwrap();
    assert!(
        std::fs::read_to_string(entry.dir.join("skills/tidy/SKILL.md"))
            .unwrap()
            .contains("mine")
    );
}

#[test]
fn server_details_carry_names_never_values() {
    let home = Home::new("details", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    let details = home.with(|ctx, _| details(ctx, "server:docs")).unwrap();
    assert_eq!(
        details.server,
        Some(crate::manager::plugins::PluginServerSettings::Stdio {
            command: "/opt/docs-mcp".to_owned(),
            args: vec!["--port".to_owned(), "1".to_owned()],
            cwd: None,
            env_names: vec!["TOKEN".to_owned()],
        })
    );
    assert!(!serde_json::to_string(&details).unwrap().contains("abc123"));
}

#[test]
fn a_copy_keeps_the_oauth_client_and_the_folder_it_runs_in() {
    let home = Home::new("oauth-cwd", &["proj"]);
    let project = home.projects[0].clone();
    write(
        &project.join(".mcp.json"),
        r#"{"mcpServers": {
            "local": {"command": "./bin/mcp", "args": ["data/config.json"]},
            "remote": {"type": "http", "url": "https://mcp.example.org/mcp", "oauth": {"clientId": "client-1"}}}}"#,
    );
    home.sync().unwrap();
    let read = |name: &str, file: &str| -> serde_json::Value {
        let entry = home.library.entry(name).unwrap();
        serde_json::from_str(&std::fs::read_to_string(entry.dir.join(file)).unwrap()).unwrap()
    };
    let local = read("local", ".codex.mcp.json");
    assert_eq!(
        local["mcpServers"]["local"]["cwd"],
        serde_json::json!(project.display().to_string())
    );
    assert_eq!(
        read("remote", ".mcp.json")["mcpServers"]["remote"]["oauth"]["clientId"],
        "client-1"
    );
    assert_eq!(
        read("remote", ".codex.mcp.json")["mcpServers"]["remote"]["oauth"]["client_id"],
        "client-1"
    );
}

#[test]
fn remove_puts_back_an_original_the_old_use_everywhere_moved() {
    let home = Home::new("legacy", &[]);
    let from = home.home.join(".claude/skills/old");
    let kept = home.library.root().join(".moved/old/skill-0");
    write(&kept.join("SKILL.md"), "---\nname: old\n---\nkept\n");
    home.with(|ctx, run| {
        ops::add(
            ctx,
            &kept.display().to_string(),
            Some("old"),
            None,
            false,
            &ops::AddOptions::default(),
            run,
        )
    })
    .unwrap();
    write(
        &home.library.root().join(".moved/old.json"),
        &serde_json::json!({"originals": [{"kind": "skill", "from": from, "kept": kept}]})
            .to_string(),
    );
    home.with(|ctx, run| remove(ctx, "lib:old", run)).unwrap();
    assert!(
        std::fs::read_to_string(from.join("SKILL.md"))
            .unwrap()
            .contains("kept")
    );
    assert!(home.library.entry("old").is_none());
    assert!(!home.library.root().join(".moved/old.json").exists());
    // The original is back, and Remove is remembered: it isn't shared again.
    home.sync().unwrap();
    assert!(!home.installed(Agent::Codex, "old"));
}

/// No module here writes an agent's own config: every change goes through a CLI's own
/// commands, Codex's config writer or a project's `.codex/config.toml`.
#[test]
fn no_plugins_code_writes_the_agents_own_configs() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/manager/plugins");
    let mut files = vec![dir.with_extension("rs")];
    files.extend(
        std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| !path.to_string_lossy().contains("tests")),
    );
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap();
        for (at, line) in text.lines().enumerate() {
            let writes = line.contains("fs::write")
                || line.contains("fs::rename")
                || line.contains("File::create");
            let names = [
                "claude_json",
                ".claude.json",
                "codex_home",
                "config.toml",
                "settings.json",
            ];
            assert!(
                !(writes && names.iter().any(|name| line.contains(name))),
                "{}:{} writes an agent's own config: {line}",
                file.display(),
                at + 1
            );
        }
    }
}

#[test]
fn review_error_details_do_not_expand_secrets() {
    let mut home = Home::new("review-secret-display", &[]);
    home.roots
        .vars
        .insert("REVIEW_SECRET".into(), "SENTINEL_PRIVATE_TOKEN".into());
    write(
        &home.home.join(".claude.json"),
        r#"{"mcpServers": {
        "command": {"type":"unknown", "command":"${REVIEW_SECRET}"},
        "address": {"url":"${REVIEW_SECRET}://invalid"}}}"#,
    );
    let display = serde_json::to_string(&home.view()).unwrap();
    assert!(!display.contains("SENTINEL_PRIVATE_TOKEN"), "{display}");
    assert!(display.contains("${REVIEW_SECRET}"));
}

#[test]
fn review_failed_pack_keeps_previous_secrets() {
    let home = Home::new("review-failed-pack", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    home.sync().unwrap();
    let installed = snapshot(&home.library.entry("docs").unwrap().dir);
    write(
        &home.library.root().join("plugins/.docs.new"),
        "block staging",
    );
    write(
        &home.home.join(".claude.json"),
        &CLAUDE_DOCS.replace("abc123", "rotated"),
    );
    home.sync().unwrap();
    assert!(home.sharing().failed.contains_key("server:docs"));
    assert_eq!(
        home.secrets
            .load("docs/docs")
            .unwrap()
            .get("TOKEN")
            .map(String::as_str),
        Some("abc123")
    );
    assert_eq!(
        snapshot(&home.library.entry("docs").unwrap().dir),
        installed
    );
}

#[test]
fn review_partial_legacy_restore_can_retry() {
    let home = Home::new("review-legacy-retry", &[]);
    let from = home.home.join(".claude/skills/old");
    let second = home.home.join(".claude/skills/second");
    let kept = home.library.root().join(".moved/old/skill-0");
    let kept_second = home.library.root().join(".moved/old/skill-1");
    write(&kept.join("SKILL.md"), "---\nname: old\n---\nkept\n");
    write(&kept_second.join("SKILL.md"), "second kept");
    write(&second.join("SKILL.md"), "conflict");
    home.with(|ctx, run| {
        ops::add(
            ctx,
            &kept.display().to_string(),
            Some("old"),
            None,
            false,
            &ops::AddOptions::default(),
            run,
        )
    })
    .unwrap();
    write(
        &ops::moved_file(&home.library, "old"),
        &serde_json::json!({"originals": [
            {"kind":"skill", "from":from, "kept":kept},
            {"kind":"skill", "from":second, "kept":kept_second}
        ]})
        .to_string(),
    );
    assert!(home.with(|ctx, run| remove(ctx, "lib:old", run)).is_err());
    let checkpoint: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ops::moved_file(&home.library, "old")).unwrap(),
    )
    .unwrap();
    assert_eq!(checkpoint["restored"], serde_json::json!([0]));
    assert!(from.join("SKILL.md").exists());
    assert!(home.library.entry("old").is_some());
    write(
        &ops::moved_file(&home.library, "old"),
        "interrupted journal",
    );
    assert!(home.with(|ctx, run| remove(ctx, "lib:old", run)).is_err());
    assert!(home.library.entry("old").is_some());
    write(
        &ops::moved_file(&home.library, "old"),
        &checkpoint.to_string(),
    );
    std::fs::remove_dir_all(&second).unwrap();
    home.with(|ctx, run| remove(ctx, "lib:old", run)).unwrap();
    assert_eq!(
        std::fs::read_to_string(second.join("SKILL.md")).unwrap(),
        "second kept"
    );
    assert!(home.library.entry("old").is_none());
}

fn codex_pg(home: &Home) {
    let config = home.home.join(".codex/config.toml");
    let text = std::fs::read_to_string(&config).unwrap();
    write(
        &config,
        &format!("{text}\n[mcp_servers.pg]\ncommand = \"/opt/pg-mcp\"\n"),
    );
}

#[test]
fn review_scope_survives_sync() {
    let home = Home::new("review-scope-sync", &["one", "two"]);
    codex_pg(&home);
    home.sync().unwrap();
    home.with(|ctx, run| set_scope(ctx, "server:pg", Some(&home.projects[..1]), run))
        .unwrap();
    home.sync().unwrap();
    let new = home.home.join("new-unselected");
    std::fs::create_dir_all(&new).unwrap();
    let catalog = Catalog::read(&home.roots, Some(&new));
    for agent in [Agent::Claude, Agent::Codex] {
        let session = brigadier_registry::Session {
            agent,
            project: Some(&new),
            cwd: &new,
            in_checkout: true,
        };
        assert!(
            !catalog
                .servers
                .iter()
                .any(|server| server.name == "pg" && catalog.server_native(server, &session)),
            "Only some projects copy loads in a new unselected folder for {agent:?}"
        );
    }
    assert!(!home.placed(Agent::Claude, "pg").global);
    assert_eq!(home.placed(Agent::Claude, "pg").on_in, home.projects[..1]);
}

#[test]
fn review_scope_back_to_all_stays_switchable() {
    let home = Home::new("review-scope-back", &["one", "two"]);
    codex_pg(&home);
    home.sync().unwrap();
    home.with(|ctx, run| set_scope(ctx, "server:pg", Some(&home.projects[..1]), run))
        .unwrap();
    home.with(|ctx, run| set_scope(ctx, "server:pg", None, run))
        .unwrap();
    assert!(home.item("server:pg").locked.is_none());
    home.with(|ctx, run| switch(ctx, "server:pg", false, run))
        .unwrap();
    assert!(!home.item("server:pg").on);
    // An independent project definition still locks it, even at the same endpoint.
    write(
        &home.projects[1].join(".codex/config.toml"),
        "[mcp_servers.pg]\ncommand = \"/opt/pg-mcp\"\n",
    );
    assert!(home.item("server:pg").locked.is_some());
}

#[test]
fn review_project_alias_does_not_suppress_other_projects() {
    let home = Home::new("review-alias-scope", &["one", "two"]);
    write(
        &home.home.join(".claude.json"),
        r#"{"mcpServers":{"docs":{"url":"https://example.org/mcp"}}}"#,
    );
    write(
        &home.projects[0].join(".codex/config.toml"),
        "[mcp_servers.docs-alt]\nurl = \"https://example.org/mcp\"\n",
    );
    let before = snapshot(&home.projects[0].join(".codex/config.toml"));
    home.sync().unwrap();
    let placed = home.placed(Agent::Codex, "docs");
    assert!(
        placed.has && placed.on_in.contains(&home.projects[1]),
        "{placed:?}"
    );
    assert!(
        !placed.on_in.contains(&home.projects[0]),
        "alias and copy both load: {placed:?}"
    );
    let after = snapshot(&home.projects[0].join(".codex/config.toml"));
    assert!(
        after[0].1.starts_with(&before[0].1),
        "original definition changed"
    );
}

#[test]
fn review_credentials_make_definitions_different() {
    let home = Home::new("review-credentials", &[]);
    for (claude, codex) in [
        (
            r#"{"url":"https://example.org/mcp", "headers":{"Authorization":"Bearer tenant-a"}}"#,
            "url = \"https://example.org/mcp\"\nhttp_headers = { Authorization = \"Bearer tenant-b\" }",
        ),
        (
            r#"{"command":"/opt/mcp", "env":{"TOKEN":"tenant-a"}}"#,
            "command = \"/opt/mcp\"\nenv = { TOKEN = \"tenant-b\" }",
        ),
        (
            r#"{"url":"https://example.org/mcp", "oauth":{"clientId":"client-a"}}"#,
            "url = \"https://example.org/mcp\"\noauth = { client_id = \"client-b\" }",
        ),
    ] {
        write(
            &home.home.join(".claude.json"),
            &format!(r#"{{"mcpServers":{{"docs":{claude}}}}}"#),
        );
        write(
            &home.home.join(".codex/config.toml"),
            &format!("[mcp_servers.docs]\n{codex}\n"),
        );
        home.sync().unwrap();
        assert!(
            matches!(
                home.item("server:docs").problem,
                Some(PluginProblem::Conflict { .. })
            ),
            "different credentials were silently merged"
        );
        assert!(home.library.entries().is_empty());
        let display = serde_json::to_string(&home.view()).unwrap();
        assert!(!display.contains("tenant-a") && !display.contains("tenant-b"));
    }
}

#[test]
fn review_rename_skill_keeps_decline() {
    let home = Home::new("review-rename-decline", &[]);
    let dir = home.home.join(".claude/skills/original");
    write(&dir.join("SKILL.md"), "---\nname: first\n---\nbody\n");
    home.sync().unwrap();
    home.with(|ctx, run| remove(ctx, "skill:first", run))
        .unwrap();
    write(&dir.join("SKILL.md"), "---\nname: second\n---\nchanged\n");
    home.sync().unwrap();
    assert!(
        home.library.entries().is_empty(),
        "renaming an original re-shares a removed skill"
    );
    assert!(matches!(
        home.item("skill:second").problem,
        Some(PluginProblem::OnlyIn {
            reason: OnlyInReason::Removed,
            ..
        })
    ));
}

#[test]
fn review_disabled_project_original_is_in_inventory() {
    let home = Home::new("review-disabled-project", &["one"]);
    let original = home.projects[0].join(".codex/config.toml");
    write(
        &original,
        "[mcp_servers.quiet]\ncommand = \"/opt/quiet\"\nenabled = false\n",
    );
    let before = snapshot(&original);
    home.sync().unwrap();
    let item = home.item("server:quiet");
    assert!(!item.on);
    assert!(item.locked.is_some());
    assert_eq!(snapshot(&original), before);
    assert!(!home.placed(Agent::Claude, "quiet").global);
}

#[test]
fn review_added_server_shows_secret_names() {
    let home = Home::new("review-added-names", &[]);
    home.with(|ctx, run| {
        ops::add(
            ctx,
            r#"{"mcpServers":{
        "stdio":{"command":"/opt/mcp", "env":{"TOKEN":"private-env"}},
        "remote":{"url":"https://example.org/mcp", "headers":{"Authorization":"private-header"}}
    }}"#,
            None,
            None,
            false,
            &ops::AddOptions::default(),
            run,
        )
    })
    .unwrap();
    for (key, name) in [("lib:stdio", "TOKEN"), ("lib:remote", "Authorization")] {
        let details = home.with(|ctx, _| details(ctx, key)).unwrap();
        match details.server.as_ref().unwrap() {
            PluginServerSettings::Stdio { env_names, .. } => assert_eq!(env_names, &[name]),
            PluginServerSettings::Http { header_names, .. } => assert_eq!(header_names, &[name]),
        }
        let display = serde_json::to_string(&details).unwrap();
        assert!(!display.contains("private-env") && !display.contains("private-header"));
    }
}

#[test]
fn review_all_problem_reasons_sort_before_healthy_rows() {
    let home = Home::new("review-problem-sort", &[]);
    write(
        &home.home.join(".claude.json"),
        r#"{"mcpServers":{
        "aaa-healthy":{"url":"https://example.org/mcp"},
        "zzz-unshareable":{"command":"${MISSING}"},
        "zzz-removed":{"command":"/opt/removed"}
    }}"#,
    );
    home.sync().unwrap();
    home.with(|ctx, run| remove(ctx, "server:zzz-removed", run))
        .unwrap();
    let view = home.view();
    let healthy = view
        .items
        .iter()
        .position(|row| row.key == "server:aaa-healthy")
        .unwrap();
    assert!(healthy > 0);
    for (at, row) in view.items.iter().enumerate() {
        if row.problem.is_some() {
            assert!(at < healthy, "{} sorted after a healthy row", row.key);
        }
    }
}

#[test]
fn review_import_preserves_arguments_env_headers_and_other_app_file() {
    let mut home = Home::new("review-import", &["one", "two"]);
    let file = home.home.join("other-app/config.json");
    home.roots.desktop_config = Some(file.clone());
    write(
        &file,
        r#"{"mcpServers":{
        "stdio":{"command":"/opt/with spaces/mcp", "args":["one two", "a\"b", "--path", "/some folder"], "env":{"TOKEN":"private-env"}},
        "remote":{"url":"https://example.org/mcp", "headers":{"Authorization":"Bearer private-header"}}
    }}"#,
    );
    let before = snapshot(&file);
    for name in ["stdio", "remote"] {
        let key = home
            .view()
            .elsewhere
            .iter()
            .find(|row| row.name == name)
            .unwrap()
            .key
            .clone();
        let imported = home
            .with(|ctx, run| import(ctx, &key, None, Some(&home.projects[..1]), run))
            .unwrap();
        assert_eq!(imported, format!("lib:{name}"));
        assert!(home.view().elsewhere.iter().all(|row| row.key != key));
        for agent in [Agent::Claude, Agent::Codex] {
            let placed = home.placed(agent, name);
            assert!(
                !placed.global && placed.on_in == home.projects[..1],
                "{placed:?}"
            );
        }
    }
    assert_eq!(
        home.secrets.load("stdio/stdio").unwrap()["TOKEN"],
        "private-env"
    );
    assert_eq!(
        home.secrets.load("remote/remote").unwrap()["Authorization"],
        "Bearer private-header"
    );
    let settings = home
        .with(|ctx, _| details(ctx, "lib:stdio"))
        .unwrap()
        .server
        .unwrap();
    assert!(
        matches!(settings, PluginServerSettings::Stdio {command,args,..}
        if command == "/opt/with spaces/mcp" && args == ["one two", "a\"b", "--path", "/some folder"])
    );
    assert_eq!(snapshot(&file), before);
    for (_, bytes) in snapshot(&home.home.join(".brigadier")) {
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("private-env") && !text.contains("private-header"));
    }
}

#[test]
fn review_import_clash_requires_name_and_rejects_agent_entries() {
    let mut home = Home::new("review-import-clash", &[]);
    let file = home.home.join("other-app/config.json");
    home.roots.desktop_config = Some(file.clone());
    write(&file, r#"{"mcpServers":{"docs":{"command":"other-mcp"}}}"#);
    let key = home.view().elsewhere[0].key.clone();
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    let error = home
        .with(|ctx, run| import(ctx, &key, None, None, run))
        .unwrap_err();
    assert!(error.contains("docs"), "{error}");
    assert!(home.library.entries().is_empty());
    assert!(home.sharing().imported.is_empty());
    home.with(|ctx, run| import(ctx, &key, Some("renamed"), None, run))
        .unwrap();
    assert!(home.library.entry("renamed").is_some());
    assert!(
        home.with(|ctx, run| import(ctx, "other:claude:user:docs", Some("fake"), None, run))
            .is_err()
    );
    assert!(home.library.entry("fake").is_none());
}

// Jobs share their serialization queue; these tests wait for their own background work to end.
#[cfg(unix)]
static BACKGROUND: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn stub_job_setup(home: &Home, script: &str) -> super::super::jobs::Setup {
    use std::os::unix::fs::PermissionsExt;
    let binary = home.home.join("stub-cli");
    write(&binary, script);
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let path = binary.clone();
    super::super::jobs::Setup {
        registry: brigadier_registry::Registry::new(home.roots.clone()),
        library: home.library.clone(),
        cli: binary,
        projects: home.projects.clone(),
        binary: std::sync::Arc::new(move |_| Some(path.clone())),
    }
}

#[cfg(unix)]
fn until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if done() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    done()
}

#[test]
#[cfg(unix)]
fn review_removed_original_schedules_its_confirming_pass() {
    let _serial = BACKGROUND.lock().unwrap();
    let home = Home::new("review-confirm-pass", &[]);
    tidy(&home.home, "body");
    home.sync().unwrap();
    std::fs::remove_dir_all(home.home.join(".claude/skills/tidy")).unwrap();
    // One event only. No watcher, request, or hand-run second pass follows it.
    super::super::jobs::request_sync(stub_job_setup(&home, "#!/bin/sh\nexit 0\n"), true);
    assert!(
        until(|| home.library.entry("tidy").is_none()),
        "the confirming absent pass was never scheduled"
    );
}

#[test]
#[cfg(unix)]
fn review_stalled_background_cli_releases_the_job_queue() {
    let _serial = BACKGROUND.lock().unwrap();
    let home = Home::new("review-stalled-job", &[]);
    write(
        &home.home.join(".claude.json"),
        &CLAUDE_DOCS.replace("abc123", &"x".repeat(1024 * 1024)),
    );
    let setup = stub_job_setup(
        &home,
        "#!/bin/sh\nif [ \"$1\" = secret ]; then\n  : > \"$HOME/entered\"\n  exec /bin/sleep 30\nfi\nexit 0\n",
    );
    super::super::jobs::request_sync(setup.clone(), true);
    assert!(
        until(|| home.home.join("entered").exists()),
        "state: {:?}, files: {:?}",
        home.sharing(),
        snapshot(&home.home)
            .iter()
            .map(|(p, _)| p)
            .collect::<Vec<_>>()
    );
    let job = super::super::jobs::start(
        setup,
        super::super::jobs::Action::Remove {
            key: "missing".into(),
        },
    );
    assert!(
        until(|| super::super::jobs::job(&job).is_some_and(|job| job.finished)),
        "a stalled background sync kept the queue"
    );
    assert!(super::super::jobs::job(&job).unwrap().error.is_some());
}

#[test]
fn review_each_repository_is_read_and_project_ids_stay_unique() {
    use crate::model::{FolderTrust, Project, ProjectId, ProjectPrefs, ProjectRepo};
    let home = Home::new("review-multi-repo", &["one", "two"]);
    write(
        &home.projects[1].join(".mcp.json"),
        r#"{"mcpServers":{"second":{"url":"https://example.org/mcp"}}}"#,
    );
    let project = Project {
        id: ProjectId("p".into()),
        name: "Multiple".into(),
        created_at_ms: 0,
        prefs: ProjectPrefs::default(),
        repos: home
            .projects
            .iter()
            .map(|path| ProjectRepo {
                path: path.display().to_string(),
                name: "repo".into(),
            })
            .collect(),
        trust: home
            .projects
            .iter()
            .map(|path| FolderTrust {
                path: path.display().to_string(),
                trusted: true,
                decided_at_ms: 0,
            })
            .collect(),
    };
    let repos = super::super::trusted_repos(&[project]);
    assert_eq!(repos.len(), 2);
    let catalogs: Vec<ProjectCatalog> = repos
        .into_iter()
        .map(|(id, name, path)| {
            let catalog = Catalog::read(&home.roots, Some(&path));
            ProjectCatalog {
                id,
                name,
                path,
                catalog,
            }
        })
        .collect();
    let user = Catalog::read(&home.roots, None);
    let model = model::model(&user, &catalogs, &[], &Sharing::default(), &home.home);
    let view = build(&model, &catalogs, &Sharing::default(), &Unknown, &[], None).view;
    assert!(view.items.iter().any(|row| row.key == "server:second"));
    assert_eq!(view.projects.len(), 1);
}

#[test]
fn review_aliases_in_different_scopes_cover_both_agents_in_each_project() {
    let home = Home::new("review-alias-union", &["one", "two"]);
    write(
        &home.projects[0].join(".mcp.json"),
        r#"{"mcpServers":{"first":{"url":"https://example.org/mcp"}}}"#,
    );
    write(
        &home.projects[1].join(".codex/config.toml"),
        "[mcp_servers.second]\nurl = \"https://example.org/mcp\"\n",
    );
    home.sync().unwrap();
    for agent in [Agent::Claude, Agent::Codex] {
        for path in &home.projects {
            let catalog = Catalog::read(&home.roots, Some(path));
            let session = brigadier_registry::Session {
                agent,
                cwd: path,
                project: Some(path),
                in_checkout: true,
            };
            let loaded: Vec<_> = catalog
                .servers
                .iter()
                .filter(|server| catalog.server_native(server, &session))
                .collect();
            assert_eq!(
                loaded.len(),
                1,
                "{agent:?} in {}: {loaded:?}",
                path.display()
            );
        }
    }
}

#[test]
#[cfg(unix)]
fn review_project_change_during_sync_reads_the_latest_roots() {
    let _serial = BACKGROUND.lock().unwrap();
    let home = Home::new("review-latest-roots", &["one", "two"]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    write(
        &home.projects[1].join(".mcp.json"),
        r#"{"mcpServers":{"second":{"url":"https://example.org/mcp"}}}"#,
    );
    let setup = stub_job_setup(
        &home,
        "#!/bin/sh\nif [ \"$1\" = secret ]; then : > \"$HOME/entered\"; exec /bin/sleep 30; fi\nexit 0\n",
    );
    let mut first = setup.clone();
    first.projects.truncate(1);
    super::super::jobs::request_sync(first, true);
    assert!(until(|| home.home.join("entered").exists()));
    super::super::jobs::request_sync(setup, false);
    assert!(
        until(|| home.library.entry("second").is_some()),
        "sync kept the old project's repository list"
    );
    let job = super::super::jobs::start(
        stub_job_setup(&home, "#!/bin/sh\nexit 0\n"),
        super::super::jobs::Action::Remove {
            key: "missing".into(),
        },
    );
    assert!(until(
        || super::super::jobs::job(&job).is_some_and(|job| job.finished)
    ));
}

#[test]
fn review_failed_secret_commit_keeps_previous_credentials_and_reports_deletion_failure() {
    let home = Home::new("review-secret-commit", &[]);
    write(&home.home.join(".claude.json"), CLAUDE_DOCS);
    home.sync().unwrap();
    write(
        &home.home.join(".claude.json"),
        &CLAUDE_DOCS.replace("abc123", "rotated"),
    );
    home.secrets
        .1
        .store(true, std::sync::atomic::Ordering::SeqCst);
    home.sync().unwrap();
    assert!(
        home.sharing().failed["server:docs"]
            .reason
            .contains("delete failure")
    );
    assert_eq!(home.secrets.load("docs/docs").unwrap()["TOKEN"], "abc123");
    home.secrets
        .2
        .store(true, std::sync::atomic::Ordering::SeqCst);
    home.sync().unwrap();
    assert!(
        home.sharing().failed["server:docs"]
            .reason
            .contains("write failure")
    );
    assert_eq!(home.secrets.load("docs/docs").unwrap()["TOKEN"], "abc123");
    home.sync().unwrap();
    assert_eq!(home.secrets.load("docs/docs").unwrap()["TOKEN"], "rotated");
}
