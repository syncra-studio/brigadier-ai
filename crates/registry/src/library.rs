//! The Library, `~/.brigadier/library/`: a local plugin marketplace that every agent installs
//! from with its own `plugin` commands, so an item added once is native to each CLI, in the
//! app and in a terminal (STRIP-DOWN-PLAN §9.4, X2).
//!
//! Each item is one plugin, named after it, in `plugins/<name>/`:
//!
//! - `.claude-plugin/plugin.json` with `.mcp.json`: Claude Code's.
//! - `.codex-plugin/plugin.json` with `.codex.mcp.json`: Codex's. Codex leaves `${…}`
//!   templates literal and runs a command in the project's folder, so its copy names the
//!   plugin's folder by its absolute path and carries no template (research P1, P2).
//! - `skills/<skill>/`: the skill's own files.
//!
//! No env or header value is written to a file: each goes to the Keychain ([`SecretSink`]),
//! and the server runs through `brigadier mcp run <plugin>/<server> -- …` (env) or names
//! `brigadier secret headers <plugin>/<server>` as its headers helper (each agent's own key).
//!
//! A plugin's version counts up whenever its files change ([`Packed::changed`]); the hash of
//! its files is kept in `.brigadier.json` next to them.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::adapter::{AgentAdapter, CLI_TIMEOUT, Place, Step, Unsupported, output_within};
use crate::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};
use crate::{Agent, Secret};

/// The marketplace's name: plugins are `<item>@brigadier`.
pub const MARKETPLACE: &str = "brigadier";

/// Where a server's env and header values are kept, by `<plugin>/<server>` and name.
pub trait SecretSink {
    fn store(&self, server: &str, name: &str, value: &Secret) -> Result<(), String>;
}

/// Why an item couldn't go into the Library, in plain words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibraryError {
    /// Another item in the Library has the name.
    NameTaken(String),
    /// Another MCP server the agents know has the name: two servers of one name don't both
    /// load in every agent (research P3).
    ServerNameTaken(String),
    /// The name can't be a plugin's or a server's.
    BadName(String),
    /// No `SKILL.md` where the skill should be.
    NotASkill(PathBuf),
    Io(String),
    Secret(String),
}

impl std::fmt::Display for LibraryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameTaken(name) => write!(f, "Your Library already has something called {name}."),
            Self::ServerNameTaken(name) => write!(
                f,
                "Another MCP server is already called {name}. Pick another name."
            ),
            Self::BadName(name) => write!(
                f,
                "\"{name}\" can't be used as a name: use letters, digits, - and _."
            ),
            Self::NotASkill(dir) => write!(f, "{} has no SKILL.md.", dir.display()),
            Self::Io(err) => write!(f, "Couldn't save it in your Library: {err}"),
            Self::Secret(err) => write!(f, "Couldn't keep its secret in the Keychain: {err}"),
        }
    }
}

impl std::error::Error for LibraryError {}

fn io(err: std::io::Error) -> LibraryError {
    LibraryError::Io(err.to_string())
}

/// One plugin in the Library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub version: String,
    pub description: String,
    /// Its folder.
    pub dir: PathBuf,
}

impl Entry {
    /// What the agents' plugin commands call it.
    pub fn id(&self) -> String {
        plugin_id(&self.name)
    }
}

/// `<name>@brigadier`.
pub fn plugin_id(name: &str) -> String {
    format!("{name}@{MARKETPLACE}")
}

/// What [`Library::pack`] made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packed {
    pub entry: Entry,
    /// Its files differ from the last pack (a new item, or new contents).
    pub changed: bool,
    /// Its server, as each agent's copy starts it (for [`check_tools`]).
    pub server: Option<String>,
}

/// Where a Library item fetched from Git or skills.sh came from, and the commit it was packed
/// from: what "Update available" compares with.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Origin {
    pub source: SkillSource,
    pub commit: String,
}

/// The Library at a folder.
#[derive(Clone)]
pub struct Library {
    root: PathBuf,
    /// The `brigadier` CLI, by absolute path: neither agent passes its own PATH on reliably.
    brigadier: PathBuf,
}

impl Library {
    /// The Library in `home`'s `.brigadier/library`.
    pub fn in_home(home: &Path, brigadier: PathBuf) -> Self {
        Self::at(home.join(".brigadier").join("library"), brigadier)
    }

    pub fn at(root: PathBuf, brigadier: PathBuf) -> Self {
        Self { root, brigadier }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn marketplace_file(&self) -> PathBuf {
        self.root.join(".claude-plugin").join("marketplace.json")
    }

    fn plugin_dir(&self, name: &str) -> PathBuf {
        self.root.join("plugins").join(name)
    }

    /// Its plugins, as its marketplace lists them.
    pub fn entries(&self) -> Vec<Entry> {
        let Ok(text) = std::fs::read_to_string(self.marketplace_file()) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Vec::new();
        };
        value
            .get("plugins")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|plugin| {
                let name = plugin.get("name")?.as_str()?.to_owned();
                Some(Entry {
                    dir: self.plugin_dir(&name),
                    version: plugin
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    description: plugin
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    name,
                })
            })
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries().into_iter().find(|entry| entry.name == name)
    }

    /// The names of plugin `name`'s server's values (env or headers), kept in the Keychain:
    /// what its settings show, never the values.
    pub fn value_names(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.plugin_dir(name).join(".brigadier.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|meta| {
                meta.get("values")?.as_array().map(|names| {
                    names
                        .iter()
                        .filter_map(|name| name.as_str().map(str::to_owned))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// A name for a new plugin made from an item called `name`: `name` itself when it can be
    /// one and is free, else it with other characters as `-`, then `-2`, `-3`… until free.
    pub fn fresh_name(&self, name: &str) -> String {
        let mut base: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .take(56)
            .collect();
        if base.is_empty() {
            base = "item".to_owned();
        }
        let taken: Vec<String> = self.entries().into_iter().map(|entry| entry.name).collect();
        let free = |candidate: &str| {
            !taken.iter().any(|known| known == candidate) && !self.plugin_dir(candidate).exists()
        };
        if free(&base) {
            return base;
        }
        (2..)
            .map(|n| format!("{base}-{n}"))
            .find(|candidate| free(candidate))
            .unwrap_or(base)
    }

    /// Packs `item` as plugin `name` (an update when `replace` and it is there already). A
    /// skill's files are read from `skill_dir` (fetched by the caller for a Git or skills.sh
    /// source); `taken` are the names of the MCP servers the agents have besides this item's.
    pub fn pack(
        &self,
        name: &str,
        item: &Item,
        skill_dir: Option<&Path>,
        taken: &[String],
        replace: bool,
        secrets: &dyn SecretSink,
    ) -> Result<Packed, LibraryError> {
        check_name(name)?;
        if !replace && self.entry(name).is_some() {
            return Err(LibraryError::NameTaken(name.to_owned()));
        }
        let dir = self.plugin_dir(name);
        let staging = self.root.join("plugins").join(format!(".{name}.new"));
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(io)?;
        }
        std::fs::create_dir_all(&staging).map_err(io)?;
        let mut values: Vec<String> = Vec::new();
        let (description, server) = match item {
            Item::Server(spec) => {
                if taken.contains(&spec.name) {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(LibraryError::ServerNameTaken(spec.name.clone()));
                }
                check_name(&spec.name)?;
                self.write_server(name, &dir, &staging, spec, secrets)?;
                values = match &spec.transport {
                    TransportSpec::Stdio { env, .. } => env,
                    TransportSpec::Http { headers, .. } => headers,
                }
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
                (
                    format!("The {} MCP server.", spec.name),
                    Some(spec.name.clone()),
                )
            }
            Item::Skill(spec) => {
                let source = skill_dir.ok_or_else(|| {
                    LibraryError::Io("the skill's files weren't fetched".to_owned())
                })?;
                (write_skill(&staging, spec, source)?, None)
            }
        };
        let hash = hash_tree(&staging).map_err(io)?;
        let old = read_meta(&dir);
        let changed = old.as_ref().map(|(_, old)| old) != Some(&hash);
        let version = match (&old, changed) {
            (Some((version, _)), false) => version.clone(),
            (Some((version, _)), true) => next_version(version),
            (None, _) => "1.0.0".to_owned(),
        };
        let manifest = |mcp: &str| {
            let mut manifest = json!({
                "name": name,
                "version": version,
                "description": description,
            });
            if staging.join("skills").is_dir() {
                manifest["skills"] = json!("./skills/");
            }
            if server.is_some() {
                manifest["mcpServers"] = json!(mcp);
            }
            manifest
        };
        write_json(
            &staging.join(".claude-plugin").join("plugin.json"),
            &manifest("./.mcp.json"),
        )?;
        write_json(
            &staging.join(".codex-plugin").join("plugin.json"),
            &manifest("./.codex.mcp.json"),
        )?;
        write_json(
            &staging.join(".brigadier.json"),
            &json!({ "version": version, "hash": hash, "values": values }),
        )?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(io)?;
        }
        std::fs::rename(&staging, &dir).map_err(io)?;
        let entry = Entry {
            name: name.to_owned(),
            version,
            description,
            dir,
        };
        self.list(&entry)?;
        Ok(Packed {
            entry,
            changed,
            server,
        })
    }

    /// Takes plugin `name` out of the Library (the agents' installs are theirs to remove).
    pub fn remove(&self, name: &str) -> Result<(), LibraryError> {
        let mut market = self.marketplace();
        if let Some(Value::Array(plugins)) = market.get_mut("plugins") {
            plugins.retain(|plugin| plugin.get("name").and_then(Value::as_str) != Some(name));
        }
        write_json(&self.marketplace_file(), &market)?;
        let dir = self.plugin_dir(name);
        if dir.exists() {
            std::fs::remove_dir_all(dir).map_err(io)?;
        }
        Ok(())
    }

    fn origins_file(&self) -> PathBuf {
        self.root.join(".origins.json")
    }

    fn origins(&self) -> BTreeMap<String, Origin> {
        std::fs::read_to_string(self.origins_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Where plugin `name` was fetched from, when from Git or skills.sh.
    pub fn origin(&self, name: &str) -> Option<Origin> {
        self.origins().remove(name)
    }

    /// Keeps (or with `None` forgets) where plugin `name` was fetched from.
    pub fn set_origin(&self, name: &str, origin: Option<&Origin>) -> Result<(), LibraryError> {
        let mut origins = self.origins();
        match origin {
            Some(origin) => origins.insert(name.to_owned(), origin.clone()),
            None => origins.remove(name),
        };
        let value =
            serde_json::to_value(&origins).map_err(|err| LibraryError::Io(err.to_string()))?;
        write_json(&self.origins_file(), &value)
    }

    fn marketplace(&self) -> Value {
        std::fs::read_to_string(self.marketplace_file())
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| {
                json!({
                    "name": MARKETPLACE,
                    "owner": { "name": "Brigadier" },
                    "plugins": [],
                })
            })
    }

    /// Puts `entry` in the marketplace's list, or brings its line up to date.
    fn list(&self, entry: &Entry) -> Result<(), LibraryError> {
        let mut market = self.marketplace();
        let line = json!({
            "name": entry.name,
            "source": format!("./plugins/{}", entry.name),
            "description": entry.description,
            "version": entry.version,
        });
        let plugins = market
            .get_mut("plugins")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LibraryError::Io("its marketplace file is damaged".to_owned()))?;
        match plugins
            .iter_mut()
            .find(|plugin| plugin.get("name").and_then(Value::as_str) == Some(&entry.name))
        {
            Some(known) => *known = line,
            None => plugins.push(line),
        }
        write_json(&self.marketplace_file(), &market)
    }

    /// Writes a server's two configs into `staging`, its values into the Keychain.
    fn write_server(
        &self,
        plugin: &str,
        dir: &Path,
        staging: &Path,
        spec: &ServerSpec,
        secrets: &dyn SecretSink,
    ) -> Result<(), LibraryError> {
        let id = format!("{plugin}/{}", spec.name);
        let brigadier = self.brigadier.display().to_string();
        let (claude, codex) = match &spec.transport {
            TransportSpec::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                for (name, value) in env {
                    secrets
                        .store(&id, name, value)
                        .map_err(LibraryError::Secret)?;
                }
                // Codex runs a relative command in the project and keeps templates literal:
                // its copy names the plugin's folder outright.
                let root = dir.display().to_string();
                let codex_args: Vec<String> =
                    args.iter().map(|arg| plugin_root(arg, &root)).collect();
                let codex_command = plugin_root(command, &root);
                let wrap = |command: String, args: Vec<String>| {
                    if env.is_empty() {
                        (command, args)
                    } else {
                        let mut wrapped = vec!["mcp".to_owned(), "run".to_owned(), id.clone()];
                        wrapped.push("--".to_owned());
                        wrapped.push(command);
                        wrapped.extend(args);
                        (brigadier.clone(), wrapped)
                    }
                };
                let stdio = |(command, args): (String, Vec<String>)| {
                    let mut server = json!({ "type": "stdio", "command": command, "args": args });
                    if let Some(cwd) = cwd {
                        server["cwd"] = json!(cwd.display().to_string());
                    }
                    server
                };
                (
                    stdio(wrap(command.clone(), args.clone())),
                    stdio(wrap(codex_command, codex_args)),
                )
            }
            TransportSpec::Http { url, headers } => {
                for (name, value) in headers {
                    secrets
                        .store(&id, name, value)
                        .map_err(LibraryError::Secret)?;
                }
                // Each agent runs the helper for the headers when it connects (Codex 0.164's
                // `http_headers_helper`, Claude Code's `headersHelper`).
                let mut claude = json!({ "type": "http", "url": url });
                let mut codex = claude.clone();
                if !headers.is_empty() {
                    let helper = json!(format!("{brigadier} secret headers {id}"));
                    claude["headersHelper"] = helper.clone();
                    codex["http_headers_helper"] = helper;
                }
                (claude, codex)
            }
        };
        let (mut claude, mut codex) = (claude, codex);
        if let Some(oauth) = &spec.oauth {
            claude["oauth"] = json!({ "clientId": oauth.client_id });
            codex["oauth"] = json!({ "client_id": oauth.client_id });
        }
        write_json(
            &staging.join(".mcp.json"),
            &json!({ "mcpServers": { &spec.name: claude } }),
        )?;
        write_json(
            &staging.join(".codex.mcp.json"),
            &json!({ "mcpServers": { &spec.name: codex } }),
        )
    }

    /// The steps that bring `entry` to `adapter`'s agent at `place`: the marketplace added
    /// (adding it again is harmless) and read again, then the plugin installed, or updated
    /// when `installed`.
    pub fn sync(
        &self,
        adapter: &dyn AgentAdapter,
        entry: &Entry,
        place: &Place,
        installed: bool,
    ) -> Result<Vec<Step>, Unsupported> {
        let mut steps = self.market_steps(adapter);
        steps.extend(if installed {
            adapter.update_plugin(&entry.id(), place)?
        } else {
            adapter.install_plugin(&entry.id(), place)?
        });
        Ok(steps)
    }

    /// The marketplace added to `adapter`'s agent (again is harmless), and read again.
    pub fn market_steps(&self, adapter: &dyn AgentAdapter) -> Vec<Step> {
        let mut steps = vec![Step::Run(adapter.add_marketplace(&self.root))];
        if let Some(refresh) = adapter.refresh_marketplace(MARKETPLACE) {
            steps.push(Step::Run(refresh));
        }
        steps
    }

    /// How `agent`'s copy of `entry`'s server starts: its command and args (stdio only).
    pub fn server_command(&self, entry: &Entry, agent: Agent) -> Option<(String, Vec<String>)> {
        let file = match agent {
            Agent::Claude => ".mcp.json",
            Agent::Codex => ".codex.mcp.json",
        };
        let text = std::fs::read_to_string(entry.dir.join(file)).ok()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        let (_, server) = value.get("mcpServers")?.as_object()?.iter().next()?;
        let command = server.get("command")?.as_str()?.to_owned();
        let args = server
            .get("args")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|arg| arg.as_str().map(str::to_owned))
            .collect();
        Some((command, args))
    }
}

/// The folder of the skill `source` names, fetched into `scratch` (an empty folder the caller
/// removes) for a Git or skills.sh source: the one [`Library::pack`] copies from.
pub fn fetch_skill(source: &SkillSource, scratch: &Path) -> Result<PathBuf, String> {
    let (url, rev, subdir, wanted) = match source {
        SkillSource::Folder(dir) => return Ok(dir.clone()),
        SkillSource::Git { url, rev, subdir } => (url.clone(), rev.clone(), subdir.clone(), None),
        SkillSource::SkillsSh(id) => {
            let mut parts = id.split('/');
            let (Some(owner), Some(repo)) = (parts.next(), parts.next()) else {
                return Err(format!("\"{id}\" isn't a skills.sh name."));
            };
            (
                format!("https://github.com/{owner}/{repo}"),
                None,
                None,
                parts.next().map(str::to_owned),
            )
        }
    };
    let checkout = scratch.join("checkout");
    let mut git = Command::new("git");
    git.args(["clone", "--quiet", "--depth", "1"]);
    if let Some(rev) = &rev {
        git.args(["--branch", rev]);
    }
    let out = output_within(
        git.arg(&url).arg(&checkout).env("GIT_TERMINAL_PROMPT", "0"),
        CLI_TIMEOUT,
    )
    .map_err(|err| format!("Couldn't run git: {err}."))?;
    if !out.status.success() {
        return Err(format!("Couldn't download {url}."));
    }
    let base = match &subdir {
        Some(subdir) => checkout.join(subdir),
        None => checkout.clone(),
    };
    if base.join("SKILL.md").is_file() && wanted.is_none() {
        return Ok(base);
    }
    // The skill by name: a folder holding `SKILL.md` whose name (or folder) matches.
    let mut found = Vec::new();
    skill_folders(&base, 0, &mut found);
    let pick = |dir: &&PathBuf| match &wanted {
        None => true,
        Some(wanted) => {
            let text = std::fs::read_to_string(dir.join("SKILL.md")).unwrap_or_default();
            crate::skills::frontmatter(&text).0.as_deref() == Some(wanted)
                || dir.file_name().is_some_and(|name| name == wanted.as_str())
        }
    };
    let matching: Vec<&PathBuf> = found.iter().filter(pick).collect();
    match matching.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!("No skill found in {url}.")),
        _ => Err(format!("{url} has several skills: name the one you want.")),
    }
}

/// The repository `source` is fetched from, and the branch or tag (`None`: its default).
fn remote(source: &SkillSource) -> Option<(String, Option<String>)> {
    match source {
        SkillSource::Folder(_) => None,
        SkillSource::Git { url, rev, .. } => Some((url.clone(), rev.clone())),
        SkillSource::SkillsSh(id) => {
            let mut parts = id.split('/');
            Some((
                format!("https://github.com/{}/{}", parts.next()?, parts.next()?),
                None,
            ))
        }
    }
}

/// The name the skill at `dir` gives itself in its `SKILL.md`.
pub fn skill_name(dir: &Path) -> Option<String> {
    crate::skills::frontmatter(&std::fs::read_to_string(dir.join("SKILL.md")).ok()?).0
}

/// The commit [`fetch_skill`] fetched into `scratch`, for a Git or skills.sh source.
pub fn fetched_commit(scratch: &Path) -> Option<String> {
    let out = output_within(
        Command::new("git")
            .arg("-C")
            .arg(scratch.join("checkout"))
            .args(["rev-parse", "HEAD"]),
        CLI_TIMEOUT,
    )
    .ok()?;
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    (out.status.success() && !commit.is_empty()).then_some(commit)
}

/// The commit `source`'s repository is at now (its branch or tag, else its default branch).
pub fn latest_commit(source: &SkillSource) -> Result<String, String> {
    let (url, rev) = remote(source).ok_or("A folder has no newer version.")?;
    let out = output_within(
        Command::new("git")
            .args(["ls-remote", &url, rev.as_deref().unwrap_or("HEAD")])
            .env("GIT_TERMINAL_PROMPT", "0"),
        CLI_TIMEOUT,
    )
    .map_err(|err| format!("Couldn't run git: {err}."))?;
    if !out.status.success() {
        return Err(format!("Couldn't reach {url}."));
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| {
            format!(
                "{url} has no {}.",
                rev.as_deref().unwrap_or("default branch")
            )
        })
}

/// The folders under `dir` (a few levels down) that hold `SKILL.md`.
fn skill_folders(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    if dir.join("SKILL.md").is_file() {
        out.push(dir.to_owned());
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !path.ends_with(".git"))
        .collect();
    children.sort();
    for child in children {
        skill_folders(&child, depth + 1, out);
    }
}

/// `${CLAUDE_PLUGIN_ROOT}` and `${PLUGIN_ROOT}` in `text` as `root`.
fn plugin_root(text: &str, root: &str) -> String {
    text.replace("${CLAUDE_PLUGIN_ROOT}", root)
        .replace("${PLUGIN_ROOT}", root)
}

/// Whether `name` can be a Library plugin's or a server's.
pub fn valid_name(name: &str) -> bool {
    check_name(name).is_ok()
}

/// The SHA-256 of `text`, in hex: what remembers a value without keeping it.
pub fn hash_text(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The hash of every file in `dir` (names and bytes): what tells a skill folder changed.
pub fn hash_dir(dir: &Path) -> std::io::Result<String> {
    hash_tree(dir)
}

/// A plugin's or server's name: letters, digits, `-` and `_`, not starting with `.`.
fn check_name(name: &str) -> Result<(), LibraryError> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(LibraryError::BadName(name.to_owned()))
    }
}

/// Copies the skill at `source` into `staging/skills/<name>`; its description.
fn write_skill(staging: &Path, spec: &SkillSpec, source: &Path) -> Result<String, LibraryError> {
    let text = std::fs::read_to_string(source.join("SKILL.md"))
        .map_err(|_| LibraryError::NotASkill(source.to_owned()))?;
    let (name, description) = crate::skills::frontmatter(&text);
    let name = spec
        .name
        .clone()
        .or(name)
        .or_else(|| {
            source
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    check_name(&name)?;
    copy_tree(source, &staging.join("skills").join(&name)).map_err(io)?;
    Ok(description.unwrap_or_else(|| format!("The {name} skill.")))
}

/// Copies `from` into `to`, files and folders, leaving out `.git`; links are followed.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let path = entry.path();
        if std::fs::metadata(&path)?.is_dir() {
            copy_tree(&path, &to.join(&name))?;
        } else {
            std::fs::copy(&path, to.join(&name))?;
        }
    }
    Ok(())
}

/// A hash of every file under `dir` (paths and bytes), leaving out Brigadier's own note.
fn hash_tree(dir: &Path) -> std::io::Result<String> {
    fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, files)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                files.push(relative.to_owned());
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    walk(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        if file == Path::new(".brigadier.json") || file.starts_with(".claude-plugin") {
            continue;
        }
        if file.starts_with(".codex-plugin") {
            continue;
        }
        let bytes = std::fs::read(dir.join(&file))?;
        let name = file.to_string_lossy();
        hasher.update((name.len() as u64).to_le_bytes());
        hasher.update(name.as_bytes());
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The version and hash in a plugin's `.brigadier.json`.
fn read_meta(dir: &Path) -> Option<(String, String)> {
    let text = std::fs::read_to_string(dir.join(".brigadier.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    Some((
        value.get("version")?.as_str()?.to_owned(),
        value.get("hash")?.as_str()?.to_owned(),
    ))
}

/// `1.0.<n+1>` after `1.0.<n>`.
fn next_version(version: &str) -> String {
    let mut parts: Vec<u64> = version
        .split('.')
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    parts.resize(3, 0);
    parts[2] += 1;
    format!("{}.{}.{}", parts[0], parts[1], parts[2])
}

fn write_json(path: &Path, value: &Value) -> Result<(), LibraryError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    let mut text =
        serde_json::to_string_pretty(value).map_err(|err| LibraryError::Io(err.to_string()))?;
    text.push('\n');
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text).map_err(io)?;
    std::fs::rename(&temp, path).map_err(io)
}

/// Starts a stdio server as `command args` with `env` and asks it for its tools: what proves
/// a packed server works, beyond an agent listing it. Gives up after `timeout`.
pub fn check_tools(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&Path>,
    timeout: Duration,
) -> Result<Vec<String>, String> {
    let mut child = Command::new(command)
        .args(args)
        .envs(env)
        .current_dir(cwd.unwrap_or_else(|| Path::new("/")))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("It won't start: {err}."))?;
    let result = (|| {
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let (send, receive) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + timeout;
        let mut ask = |id: u64, method: &str, params: Value| -> Result<Value, String> {
            if id > 0 {
                // Every request after `initialize` follows its notice.
                let initialized =
                    json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
                writeln!(stdin, "{initialized}").map_err(|err| err.to_string())?;
            }
            let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
            writeln!(stdin, "{request}").map_err(|err| err.to_string())?;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let line = receive
                    .recv_timeout(left)
                    .map_err(|_| "It didn't answer in time.".to_owned())?;
                let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if reply.get("id").and_then(Value::as_u64) == Some(id) {
                    if let Some(error) = reply.get("error") {
                        return Err(format!("It answered with an error: {error}"));
                    }
                    return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
                }
            }
        };
        ask(
            0,
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "brigadier", "version": "1" },
            }),
        )?;
        let tools = ask(1, "tools/list", json!({}))?;
        Ok(tools
            .get("tools")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tool| tool.get("name")?.as_str().map(str::to_owned))
            .collect())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

#[cfg(test)]
mod tests;
