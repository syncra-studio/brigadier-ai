//! The plugins of Claude Code and Codex the user switched on: their skills and MCP servers
//! are the agent's own (checked against claude 2.1.306 and codex-cli 0.163.0).
//!
//! - Claude Code records installs in `<claude dir>/plugins/installed_plugins.json` (`plugins`:
//!   id → `[{scope, installPath, projectPath}]`) and switches them with `enabledPlugins` in its
//!   user, project and local settings, each over the one before; a plugin none of them names
//!   is on when its manifest's `defaultEnabled` isn't false. A `project` or `local` install
//!   counts only in its `projectPath`. A plugin folder (`.claude-plugin/plugin.json`) placed in
//!   a skills folder is a plugin too, switched as `<name>@skills-dir`. Claude reads `skills/`
//!   and the manifest's `skills` entries one level deep; the root `SKILL.md` only when it has
//!   neither. It shows each skill as `<plugin>:<name>`, each server as `plugin:<plugin>:<name>`.
//! - Codex loads a plugin only where `$CODEX_HOME/config.toml` has `[plugins."<id>"]` with
//!   `enabled` not false, from `$CODEX_HOME/plugins/cache/<marketplace>/<plugin>/<version>`: a
//!   `local` version, else the highest (codex-rs `core-plugin-common/src/installed.rs`). Its
//!   manifest (`.codex-plugin/plugin.json`, else `.claude-plugin/plugin.json`) names skill
//!   folders (`./…`), else `skills/` counts, each read to any depth. It shows each skill as
//!   `<plugin>:<name>`; its servers keep their own names. Plugins installed from ChatGPT's
//!   catalog without an entry there are switched in the user's account, which isn't on disk:
//!   they don't count.
//!
//! A plugin's MCP servers are in its manifest's `mcpServers` (a file, or the servers), else
//! its `.mcp.json`.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::{Agent, Roots};

/// A plugin its agent loads.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Plugin {
    pub agent: Agent,
    /// What its agent's switch names it: `<plugin>@<marketplace>`, `<name>@skills-dir`.
    pub id: String,
    /// What its agent puts before its skills' names: its manifest's name, else its folder's.
    pub name: String,
    /// The folder holding its manifest.
    pub root: PathBuf,
    /// The only project it counts in: a project's install, one in a project's skills folder,
    /// or one only a project's settings switch on. `None`: every session.
    pub project: Option<PathBuf>,
}

impl Plugin {
    /// Whether a session loading `project`'s setup loads it.
    pub fn counts_in(&self, project: Option<&Path>) -> bool {
        self.project.is_none() || self.project.as_deref() == project
    }

    /// How its agent shows its skill `name`.
    pub fn skill_name(&self, name: &str) -> String {
        format!("{}:{name}", self.name)
    }

    /// The name its agent gives its server `name`: Claude's `plugin:<plugin>:<name>` as its
    /// tools are prefixed (`mcp__plugin_<plugin>_<name>__…`), Codex's the server's own.
    pub fn server_name(&self, name: &str) -> String {
        match self.agent {
            Agent::Claude => format!("plugin_{}_{name}", self.name)
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect(),
            Agent::Codex => name.to_owned(),
        }
    }

    /// The start of its skills' and servers' ids: `claude:plugin:<id>`, with its project's path
    /// before the id when it counts only there.
    pub(crate) fn prefix(&self) -> String {
        let agent = match self.agent {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        };
        match &self.project {
            Some(project) => format!("{agent}:plugin:{}:{}", project.display(), self.id),
            None => format!("{agent}:plugin:{}", self.id),
        }
    }

    fn manifest_path(&self) -> Option<PathBuf> {
        manifest_path(&self.root, self.agent)
    }

    fn manifest(&self) -> Value {
        self.manifest_path()
            .and_then(|path| read_json(&path))
            .unwrap_or(Value::Null)
    }

    /// Each skill folder (one holding `SKILL.md`) its agent reads, with its path in the plugin.
    pub(crate) fn skill_dirs(&self) -> Vec<PathBuf> {
        let manifest = self.manifest();
        let entries = manifest_paths(&self.root, manifest.get("skills"), self.agent);
        let mut dirs = Vec::new();
        match self.agent {
            Agent::Claude => {
                let conventional = self.root.join("skills");
                let has_folder = conventional.is_dir();
                if has_folder {
                    children(&conventional, &mut dirs);
                }
                for entry in &entries {
                    if entry.file_name().is_some_and(|name| name == "SKILL.md") && entry.is_file() {
                        dirs.extend(entry.parent().map(Path::to_owned));
                    } else if entry.join("SKILL.md").is_file() {
                        dirs.push(entry.clone());
                    } else if entry.is_dir() {
                        children(entry, &mut dirs);
                    }
                }
                if !has_folder && entries.is_empty() && self.root.join("SKILL.md").is_file() {
                    dirs.push(self.root.clone());
                }
            }
            Agent::Codex => {
                let roots = if entries.is_empty() {
                    vec![self.root.join("skills")]
                } else {
                    entries
                };
                for root in roots {
                    descendants(&root, 0, &mut dirs);
                }
            }
        }
        let mut seen = Vec::new();
        dirs.retain(|dir| {
            let real = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.clone());
            let new = !seen.contains(&real);
            seen.push(real);
            new
        });
        dirs
    }

    /// The file its MCP servers are read from and those servers (`mcpServers` of a file, or a
    /// file that is only the servers); `None` without any.
    pub(crate) fn server_file(&self) -> Option<(PathBuf, Option<Value>)> {
        let manifest = self.manifest();
        match manifest.get("mcpServers") {
            Some(Value::Object(servers)) => {
                Some((self.manifest_path()?, Some(Value::Object(servers.clone()))))
            }
            Some(Value::String(path)) => {
                let file =
                    manifest_paths(&self.root, Some(&Value::String(path.clone())), self.agent)
                        .into_iter()
                        .next()?;
                Some((file.clone(), servers_of(&file)))
            }
            _ => {
                let file = self.root.join(".mcp.json");
                file.is_file().then(|| (file.clone(), servers_of(&file)))
            }
        }
    }
}

/// A file's servers: its `mcpServers`, or the file itself when it has none; `None` when it
/// can't be parsed.
fn servers_of(file: &Path) -> Option<Value> {
    let value = read_json(file)?;
    Some(match value.get("mcpServers") {
        Some(servers) => servers.clone(),
        None => value,
    })
}

/// The manifest of the plugin at `root`, as `agent` looks for it.
fn manifest_path(root: &Path, agent: Agent) -> Option<PathBuf> {
    let own = match agent {
        Agent::Claude => [".claude-plugin"].as_slice(),
        Agent::Codex => [".codex-plugin", ".claude-plugin"].as_slice(),
    };
    own.iter()
        .map(|folder| root.join(folder).join("plugin.json"))
        .find(|path| path.is_file())
}

/// A manifest's paths (one, or a list) inside the plugin; any other is ignored, as its agent
/// does (Codex also wants each to start with `./`).
fn manifest_paths(root: &Path, value: Option<&Value>, agent: Agent) -> Vec<PathBuf> {
    let list: Vec<&str> = match value {
        Some(Value::String(path)) => vec![path],
        Some(Value::Array(paths)) => paths.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    list.into_iter()
        .filter_map(|path| {
            let relative = match agent {
                Agent::Codex => path.strip_prefix("./")?,
                Agent::Claude => path,
            };
            let relative = Path::new(relative);
            let inside = relative
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir));
            (inside
                && relative
                    .components()
                    .any(|part| matches!(part, Component::Normal(_))))
            .then(|| root.join(relative))
        })
        .collect()
}

/// The skill folders directly in `folder`.
fn children(folder: &Path, out: &mut Vec<PathBuf>) {
    for dir in sorted_dirs(folder) {
        if dir.join("SKILL.md").is_file() {
            out.push(dir);
        }
    }
}

/// `folder` and every folder under it that holds a `SKILL.md`, hidden ones left out.
fn descendants(folder: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    if folder.join("SKILL.md").is_file() {
        out.push(folder.to_owned());
    }
    for dir in sorted_dirs(folder) {
        descendants(&dir, depth + 1, out);
    }
}

/// The folders in `folder` (links to folders too), hidden ones left out, by name.
fn sorted_dirs(folder: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// `<plugin>@<marketplace>` split at its last `@`.
fn split_id(id: &str) -> Option<(&str, &str)> {
    let (plugin, marketplace) = id.rsplit_once('@')?;
    (!plugin.is_empty() && !marketplace.is_empty()).then_some((plugin, marketplace))
}

/// The plugin's manifest name, else `fallback`.
fn named(root: &Path, agent: Agent, fallback: &str) -> String {
    manifest_path(root, agent)
        .and_then(|path| read_json(&path))
        .and_then(|manifest| {
            manifest
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.to_owned())
}

/// `enabledPlugins` of Claude's settings files, each over the one before.
fn claude_switches(files: &[PathBuf]) -> BTreeMap<String, bool> {
    let mut switches = BTreeMap::new();
    for file in files {
        if let Some(Value::Object(enabled)) =
            read_json(file).and_then(|settings| settings.get("enabledPlugins").cloned())
        {
            for (id, on) in enabled {
                if let Some(on) = on.as_bool() {
                    switches.insert(id, on);
                }
            }
        }
    }
    switches
}

/// Whether Claude has the plugin `id` at `root` on under `switches`.
fn claude_on(switches: &BTreeMap<String, bool>, id: &str, root: &Path) -> bool {
    switches.get(id).copied().unwrap_or_else(|| {
        manifest_path(root, Agent::Claude)
            .and_then(|path| read_json(&path))
            .and_then(|manifest| manifest.get("defaultEnabled").and_then(Value::as_bool))
            .unwrap_or(true)
    })
}

/// A path Claude stores: `~`, `~/…`, absolute, or relative to the home folder.
fn stored_path(home: &Path, path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if path == "~" => home.to_owned(),
        None => home.join(path),
    }
}

/// One install `installed_plugins.json` records.
struct Install {
    id: String,
    root: PathBuf,
    /// A `project` or `local` install's project (or, in an older record, its own folder).
    project: Option<PathBuf>,
}

fn claude_installs(roots: &Roots) -> Vec<Install> {
    let Some(installed) = read_json(
        &roots
            .claude_dir
            .join("plugins")
            .join("installed_plugins.json"),
    ) else {
        return Vec::new();
    };
    let mut installs = Vec::new();
    for (id, entries) in installed
        .get("plugins")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        for entry in entries.as_array().into_iter().flatten() {
            let Some(path) = entry.get("installPath").and_then(Value::as_str) else {
                continue;
            };
            let root = stored_path(&roots.home, path);
            let project = match entry.get("scope").and_then(Value::as_str) {
                // An older record without `projectPath`: the project holding the install.
                Some("project" | "local") => Some(
                    entry
                        .get("projectPath")
                        .and_then(Value::as_str)
                        .map_or_else(|| root.clone(), |path| stored_path(&roots.home, path)),
                ),
                Some("user" | "managed") => None,
                _ => continue,
            };
            let root = if manifest_path(&root, Agent::Claude).is_some() {
                root
            } else {
                match cache_fallback(roots, id, entry) {
                    Some(root) => root,
                    None => continue,
                }
            };
            installs.push(Install {
                id: id.clone(),
                root,
                project,
            });
        }
    }
    installs
}

/// An install whose folder is gone (the cache re-keyed): the plugin's cache folder whose name
/// starts with its commit, else the newest one.
fn cache_fallback(roots: &Roots, id: &str, entry: &Value) -> Option<PathBuf> {
    let (plugin, marketplace) = split_id(id)?;
    let cache = roots
        .claude_dir
        .join("plugins")
        .join("cache")
        .join(marketplace)
        .join(plugin);
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = sorted_dirs(&cache)
        .into_iter()
        .filter(|dir| manifest_path(dir, Agent::Claude).is_some())
        .filter_map(|dir| Some((std::fs::metadata(&dir).ok()?.modified().ok()?, dir)))
        .collect();
    let commit = entry
        .get("gitCommitSha")
        .and_then(Value::as_str)
        .map(|sha| &sha[..sha.len().min(12)])
        .filter(|sha| !sha.is_empty());
    if let Some(commit) = commit
        && let Some((_, dir)) = candidates.iter().find(|(_, dir)| {
            dir.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(commit))
        })
    {
        return Some(dir.clone());
    }
    candidates.sort();
    candidates.pop().map(|(_, dir)| dir)
}

/// The plugin folders placed directly in a skills folder, by name.
fn skills_folder_plugins(folder: &Path) -> Vec<(String, PathBuf)> {
    sorted_dirs(folder)
        .into_iter()
        .filter(|dir| manifest_path(dir, Agent::Claude).is_some())
        .map(|dir| {
            let fallback = dir
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (named(&dir, Agent::Claude, &fallback), dir)
        })
        .collect()
}

/// Claude's plugins that are on for a session loading `project`'s setup, or with none, the
/// user's alone. With a project, each that counts only there says so ([`Plugin::project`]).
pub(crate) fn claude(roots: &Roots, project: Option<&Path>) -> Vec<Plugin> {
    let mut files = vec![roots.claude_dir.join("settings.json")];
    if let Some(project) = project {
        files.push(project.join(".claude").join("settings.json"));
        files.push(project.join(".claude").join("settings.local.json"));
    }
    let switches = claude_switches(&files);
    // What only a project's settings switch on counts only there.
    let user_switches = project.map(|_| claude_switches(&files[..1]));
    let only_here = |id: &str, root: &Path| {
        user_switches
            .as_ref()
            .is_some_and(|user| !claude_on(user, id, root))
    };
    let mut plugins = Vec::new();
    for install in claude_installs(roots) {
        let scoped = install.project.is_some();
        if install
            .project
            .as_deref()
            .is_some_and(|own| project.is_none_or(|project| !own.starts_with(project)))
            || !claude_on(&switches, &install.id, &install.root)
        {
            continue;
        }
        let fallback = split_id(&install.id).map_or(install.id.as_str(), |(name, _)| name);
        plugins.push(Plugin {
            agent: Agent::Claude,
            name: named(&install.root, Agent::Claude, fallback),
            project: (scoped || only_here(&install.id, &install.root))
                .then(|| project.map(Path::to_owned))
                .flatten(),
            id: install.id,
            root: install.root,
        });
    }
    let mut folders = vec![(roots.claude_dir.join("skills"), None)];
    if let Some(project) = project {
        folders.insert(0, (project.join(".claude").join("skills"), Some(project)));
    }
    for (folder, own) in folders {
        for (name, root) in skills_folder_plugins(&folder) {
            let id = format!("{name}@skills-dir");
            if !claude_on(&switches, &id, &root) {
                continue;
            }
            let project = own
                .or_else(|| only_here(&id, &root).then_some(project).flatten())
                .map(Path::to_owned);
            plugins.push(Plugin {
                agent: Agent::Claude,
                id,
                name,
                root,
                project,
            });
        }
    }
    plugins
}

/// How Codex orders two versions of a plugin: by semver when both are, else as text.
fn compare_versions(left: &str, right: &str) -> Ordering {
    let parse = |version: &str| -> Option<(Vec<u64>, Option<String>)> {
        let version = version.split_once('+').map_or(version, |(core, _)| core);
        let (core, pre) = match version.split_once('-') {
            Some((core, pre)) => (core, Some(pre.to_owned())),
            None => (version, None),
        };
        let parts: Vec<u64> = core
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        (parts.len() == 3).then_some((parts, pre))
    };
    match (parse(left), parse(right)) {
        (Some((a, a_pre)), Some((b, b_pre))) => a.cmp(&b).then_with(|| match (a_pre, b_pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let ids = |pre: &str| -> Vec<(bool, u64, String)> {
                    pre.split('.')
                        .map(|id| match id.parse::<u64>() {
                            Ok(n) => (false, n, String::new()),
                            Err(_) => (true, 0, id.to_owned()),
                        })
                        .collect()
                };
                ids(&a).cmp(&ids(&b))
            }
        }),
        _ => left.cmp(right),
    }
}

/// The version folder Codex loads in `folder`: `local`, else the highest.
fn active_version(folder: &Path) -> Option<PathBuf> {
    let mut versions: Vec<String> = sorted_dirs(folder)
        .into_iter()
        .filter_map(|dir| Some(dir.file_name()?.to_string_lossy().into_owned()))
        .filter(|version| {
            version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '+'))
        })
        .collect();
    if versions.iter().any(|version| version == "local") {
        return Some(folder.join("local"));
    }
    versions.sort_by(|a, b| compare_versions(a, b));
    versions.pop().map(|version| folder.join(version))
}

/// Codex's plugins that are on, from its `config.toml`.
pub(crate) fn codex(roots: &Roots, config: Option<&toml::Table>) -> Vec<Plugin> {
    let cache = roots.codex_home.join("plugins").join("cache");
    let mut plugins = Vec::new();
    for (id, switch) in config
        .and_then(|config| config.get("plugins"))
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
    {
        if switch.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let Some((plugin, marketplace)) = split_id(id) else {
            continue;
        };
        let safe = |part: &str| !part.contains(['/', '\\']) && part != "." && part != "..";
        if !safe(plugin) || !safe(marketplace) {
            continue;
        }
        let Some(root) = active_version(&cache.join(marketplace).join(plugin)) else {
            continue;
        };
        if manifest_path(&root, Agent::Codex).is_none() {
            continue;
        }
        plugins.push(Plugin {
            agent: Agent::Codex,
            id: id.clone(),
            name: named(&root, Agent::Codex, plugin),
            root,
            project: None,
        });
    }
    plugins
}

/// The servers of Codex's plugin `id` switched off in its `config.toml`
/// (`[plugins."<id>".mcp_servers.<name>] enabled = false`).
pub(crate) fn codex_servers_off(config: Option<&toml::Table>, id: &str) -> Vec<String> {
    config
        .and_then(|config| config.get("plugins"))
        .and_then(|plugins| plugins.get(id))
        .and_then(|plugin| plugin.get("mcp_servers"))
        .and_then(toml::Value::as_table)
        .into_iter()
        .flatten()
        .filter(|(_, server)| server.get("enabled").and_then(toml::Value::as_bool) == Some(false))
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_as_codex_orders_them() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(
            compare_versions("0.1.12", "0.1.12-5f7cd798dc99"),
            Ordering::Greater
        );
        assert_eq!(compare_versions("zzz", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions("aaa", "0.1.0"), Ordering::Greater);
        assert_eq!(
            compare_versions("1.0.0-alpha.2", "1.0.0-alpha.10"),
            Ordering::Less
        );
    }
}
