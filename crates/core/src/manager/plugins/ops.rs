//! Settings › Plugins' actions: a switch, Add, Use everywhere and its Undo, Remove and Update.
//!
//! Every action is carried out by each agent's own commands (and, for Codex in one project, the
//! project's own file: D8), planned by its adapter, so a plain `claude` or `codex` started in a
//! terminal gets what the page shows. Nothing here names an agent: each runs through
//! [`adapters()`].
//!
//! - A Library item (`lib:<name>`) is a plugin every agent installs. Every project: on in the
//!   user's scope. One project: on in that project's scope; when it is off in the user's scope
//!   that makes it that project's only (Claude Code's `--scope local`, Codex's project file).
//! - Another marketplace's plugin is switched the same way; it isn't installed by Brigadier.
//! - An agent's own server or skill (not a plugin's) goes into the Library first (Use
//!   everywhere: D9), keeping the original until every agent has the copy, for Undo (D5).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::adapter::{AgentAdapter, Place, Step, adapter, adapters, apply, owner};
use brigadier_registry::add_parse;
use brigadier_registry::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};
use brigadier_registry::library::{
    self, Entry, Library, LibraryError, Origin, SecretSink, plugin_id,
};
use brigadier_registry::{Agent, Catalog, Roots, Secret, Skill, SkillRoot, Source, Transport};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{PluginKind, provider};

/// How one agent's part of an action stands.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PluginStep {
    Waiting,
    Working,
    Done,
    /// Nothing for this agent to do, with why.
    Skipped {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginJobAgent {
    pub agent: ProviderKind,
    pub step: PluginStep,
}

/// One action, as it goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginJobView {
    pub id: String,
    /// Each agent's part, in the page's agent order.
    pub agents: Vec<PluginJobAgent>,
    pub finished: bool,
    /// Why it stopped, in plain words: nothing (else) was changed.
    pub error: Option<String>,
    /// Done, with something the user should know: "It's added, but it didn't start: …".
    pub warning: Option<String>,
    /// The items it made or changed.
    pub keys: Vec<String>,
}

/// One item a paste would add, as the Add dialog shows it before adding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAddItem {
    pub kind: PluginKind,
    /// Empty for a skill named by its `SKILL.md`, read once fetched.
    pub name: String,
    /// Its command, address, folder or repository.
    pub detail: String,
    pub remote: bool,
    /// Where it comes from, in plain words: "An MCP server from mcp.sentry.dev."
    pub summary: String,
    /// The names of the values it is given (env, headers): kept in the Keychain.
    pub secrets: Vec<String>,
}

/// What [`check_add`] read in a paste.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PluginAddCheck {
    pub items: Vec<PluginAddItem>,
    /// Why it can't be added as it is, in plain words.
    pub error: Option<String>,
    /// It needs a name (a single server object, an address).
    pub needs_name: bool,
}

/// What an action works with.
pub(crate) struct Ctx<'a> {
    pub roots: &'a Roots,
    /// The environment the CLIs run with: the login shell's.
    pub env: &'a [(String, String)],
    pub library: &'a Library,
    pub secrets: &'a dyn Secrets,
    /// The user's projects (folders): where an item may be on or off by itself.
    pub projects: &'a [PathBuf],
    /// Tells the page how each agent's part goes.
    pub report: &'a (dyn Fn(Agent, PluginStep) + Sync),
    /// Each agent's CLI as sessions run it (the user's choice included); `None`: its name on
    /// PATH.
    pub binary: &'a (dyn Fn(Agent) -> Option<PathBuf> + Sync),
}

/// Where values (env, headers) are kept: the Keychain, through `brigadier secret`.
pub(crate) trait Secrets: SecretSink + Sync {
    /// Every value kept for `server`.
    fn load(&self, server: &str) -> Result<BTreeMap<String, String>, String>;
    /// Forgets every value kept for `server`.
    fn forget(&self, server: &str) -> Result<(), String>;
}

impl Ctx<'_> {
    /// The agents' setup as it is now, for every project (`None`) or `project`.
    fn read(&self, project: Option<&Path>) -> Catalog {
        Catalog::read(self.roots, project)
    }

    /// Carries out `steps` for `adapter`.
    fn run(&self, adapter: &dyn AgentAdapter, steps: &[Step]) -> Result<(), String> {
        if steps.is_empty() {
            return Ok(());
        }
        let binary = (self.binary)(adapter.agent());
        apply(
            adapter,
            self.roots,
            &self.read(None),
            steps,
            self.env,
            binary.as_deref(),
        )
        .map(|_| ())
        .map_err(|err| err.to_string())
    }

    /// Runs `plan` for each agent (or only `only`), telling the page how each goes; whether
    /// every agent's part went.
    fn each(
        &self,
        only: Option<Agent>,
        plan: impl FnMut(&dyn AgentAdapter) -> Result<Option<Vec<Step>>, String>,
    ) -> bool {
        self.each_of(|agent| only.is_none_or(|only| only == agent), plan)
    }

    /// [`Self::each`] for the agents `keep` takes.
    fn each_of(
        &self,
        keep: impl Fn(Agent) -> bool,
        mut plan: impl FnMut(&dyn AgentAdapter) -> Result<Option<Vec<Step>>, String>,
    ) -> bool {
        let mut all = true;
        for adapter in adapters() {
            let agent = adapter.agent();
            if !keep(agent) {
                continue;
            }
            (self.report)(agent, PluginStep::Working);
            let step = match plan(adapter) {
                Ok(None) => PluginStep::Skipped {
                    reason: format!("{} doesn't have it.", adapter.label()),
                },
                Ok(Some(steps)) => match self.run(adapter, &steps) {
                    Ok(()) => PluginStep::Done,
                    Err(reason) => PluginStep::Failed { reason },
                },
                Err(reason) => PluginStep::Failed { reason },
            };
            all &= !matches!(step, PluginStep::Failed { .. });
            (self.report)(agent, step);
        }
        all
    }
}

/// What a page key names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Library(String),
    /// Another marketplace's plugin, by its name, and the item in it.
    Plugin(String),
    Server(String),
    Skill(String),
    /// Another app's server, by its id.
    Other(String),
}

fn target(key: &str) -> Result<Target, String> {
    let (kind, name) = key
        .split_once(':')
        .ok_or_else(|| format!("Brigadier doesn't know \"{key}\"."))?;
    let name = name.to_owned();
    Ok(match kind {
        "lib" => Target::Library(name),
        "plugin" => Target::Plugin(name.split('/').next().unwrap_or_default().to_owned()),
        "server" => Target::Server(name),
        "skill" => Target::Skill(name),
        "other" => Target::Other(name),
        _ => return Err(format!("Brigadier doesn't know \"{key}\".")),
    })
}

fn just_me(project: &Path) -> Place {
    Place::Project {
        path: project.to_owned(),
        shared: false,
    }
}

/// Whether `agent` loads plugin `id` in `catalog`: anywhere (`project` `None`: one on for every
/// project), or there.
fn plugin_on(catalog: &Catalog, agent: Agent, id: &str, only_here: bool) -> bool {
    catalog.plugins.iter().any(|plugin| {
        plugin.agent == agent && plugin.id == id && (!only_here || plugin.project.is_some())
    })
}

/// Whether `project` keeps plugin `id` on for `adapter`'s agent by itself, so it stays on there
/// once it is off for every project: it counts only there, or the project's own settings switch
/// it on over every project's.
fn kept_on_in(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str, project: &Path) -> bool {
    let catalog = ctx.read(Some(project));
    let agent = adapter.agent();
    plugin_on(&catalog, agent, id, true)
        || (plugin_on(&catalog, agent, id, false) && adapter.switched_on_in(project, id))
}

/// `agent`'s id for the plugin named `name` (another marketplace's), wherever it has it, on or
/// off.
fn plugin_id_of(ctx: &Ctx<'_>, agent: Agent, name: &str) -> Option<String> {
    std::iter::once(None)
        .chain(ctx.projects.iter().map(|path| Some(path.as_path())))
        .find_map(|project| {
            let catalog = ctx.read(project);
            catalog
                .plugins
                .into_iter()
                .chain(catalog.off.plugins)
                .find(|plugin| plugin.agent == agent && plugin.name == name)
                .map(|plugin| plugin.id)
        })
}

/// The steps that turn plugin `id` on or off for `adapter`'s agent: in `project`, else in every
/// project. `library`: it is the Library's, installed when missing.
fn plugin_plan(
    ctx: &Ctx<'_>,
    adapter: &dyn AgentAdapter,
    id: &str,
    project: Option<&Path>,
    on: bool,
    library: bool,
) -> Result<Option<Vec<Step>>, String> {
    let agent = adapter.agent();
    let installed = adapter.installed(ctx.roots, id);
    let unsupported = |err: brigadier_registry::adapter::Unsupported| err.0;
    let mut steps = Vec::new();
    let install = |place: &Place, steps: &mut Vec<Step>| -> Result<(), String> {
        steps.extend(ctx.library.market_steps(adapter));
        steps.extend(adapter.install_plugin(id, place).map_err(unsupported)?);
        Ok(())
    };
    match project {
        Some(path) => {
            let place = just_me(path);
            let here = plugin_on(&ctx.read(Some(path)), agent, id, false);
            if here == on {
                return Ok(Some(steps));
            }
            if on && !installed && !here {
                if !library {
                    return Ok(None);
                }
                install(&place, &mut steps)?;
            } else {
                steps.extend(adapter.set_plugin(id, &place, on).map_err(unsupported)?);
            }
        }
        None if on => {
            if !installed {
                if !library {
                    return Ok(None);
                }
                install(&Place::Everywhere, &mut steps)?;
            }
            steps.extend(
                adapter
                    .set_plugin(id, &Place::Everywhere, true)
                    .map_err(unsupported)?,
            );
        }
        None => {
            if installed {
                steps.extend(
                    adapter
                        .set_plugin(id, &Place::Everywhere, false)
                        .map_err(unsupported)?,
                );
            }
            // Where a project turns it on by itself, it stays on: each is turned off too.
            for path in ctx.projects {
                if kept_on_in(ctx, adapter, id, path) {
                    steps.extend(
                        adapter
                            .set_plugin(id, &just_me(path), false)
                            .map_err(unsupported)?,
                    );
                }
            }
            if !installed && steps.is_empty() {
                return Ok(None);
            }
        }
    }
    Ok(Some(steps))
}

/// Whether `adapter`'s agent has its own server `name` (not a plugin's), on or off.
fn has_server(catalog: &Catalog, adapter: &dyn AgentAdapter, name: &str) -> bool {
    catalog
        .servers
        .iter()
        .chain(&catalog.off.servers)
        .any(|server| server.name == name && server.plugin.is_none() && adapter.owns(server.source))
}

const USE_EVERYWHERE_FIRST: &str =
    "Use everywhere first: then every agent can turn it on or off per project.";

/// Turns the item `key` on or off: in `project`, else in every project; for one agent only
/// when `only`.
pub(crate) fn switch(
    ctx: &Ctx<'_>,
    key: &str,
    project: Option<&Path>,
    on: bool,
    only: Option<Agent>,
) -> Result<bool, String> {
    Ok(match target(key)? {
        Target::Library(name) => {
            if ctx.library.entry(&name).is_none() {
                return Err(format!("{name} isn't in your Library any more."));
            }
            let id = plugin_id(&name);
            ctx.each(only, |adapter| {
                plugin_plan(ctx, adapter, &id, project, on, true)
            })
        }
        Target::Plugin(name) => ctx.each(only, |adapter| {
            match plugin_id_of(ctx, adapter.agent(), &name) {
                Some(id) => plugin_plan(ctx, adapter, &id, project, on, false),
                None => Ok(None),
            }
        }),
        Target::Server(name) => {
            let place = project.map_or(Place::Everywhere, just_me);
            let catalog = ctx.read(project);
            ctx.each(only, |adapter| {
                if !has_server(&catalog, adapter, &name) {
                    return Ok(None);
                }
                adapter
                    .set_server(&name, &place, on)
                    .map(Some)
                    .map_err(|_| USE_EVERYWHERE_FIRST.to_owned())
            })
        }
        Target::Skill(_) | Target::Other(_) => return Err(USE_EVERYWHERE_FIRST.to_owned()),
    })
}

/// How an item reads in the Add dialog.
fn describe(item: &Item) -> PluginAddItem {
    match item {
        Item::Server(spec) => {
            let (detail, remote, secrets, summary) = match &spec.transport {
                TransportSpec::Stdio {
                    command, args, env, ..
                } => (
                    std::iter::once(command.as_str())
                        .chain(args.iter().map(String::as_str))
                        .collect::<Vec<_>>()
                        .join(" "),
                    false,
                    env,
                    "An MCP server your agents start on this Mac.".to_owned(),
                ),
                TransportSpec::Http { url, headers } => {
                    let host = url
                        .split_once("://")
                        .map_or(url.as_str(), |(_, rest)| rest)
                        .split(['/', '?', '#'])
                        .next()
                        .unwrap_or_default()
                        .rsplit('@')
                        .next()
                        .unwrap_or_default();
                    let summary = if headers.is_empty() {
                        format!(
                            "An MCP server from {host}. If it asks, it signs in with your browser."
                        )
                    } else {
                        format!("An MCP server from {host}.")
                    };
                    (url.clone(), true, headers, summary)
                }
            };
            PluginAddItem {
                kind: PluginKind::Server,
                name: spec.name.clone(),
                detail,
                remote,
                summary,
                secrets: secrets.iter().map(|(name, _)| name.clone()).collect(),
            }
        }
        Item::Skill(spec) => {
            let (detail, summary) = match &spec.source {
                SkillSource::Folder(dir) => (
                    dir.display().to_string(),
                    "A skill from a folder on this Mac.".to_owned(),
                ),
                SkillSource::Git { url, .. } => (url.clone(), format!("A skill from {url}.")),
                SkillSource::SkillsSh(id) => (id.clone(), format!("A skill from skills.sh: {id}.")),
            };
            PluginAddItem {
                kind: PluginKind::Skill,
                name: spec.name.clone().unwrap_or_default(),
                detail,
                remote: false,
                summary,
                secrets: Vec::new(),
            }
        }
    }
}

/// What a paste would add, read before anything is added.
pub(crate) fn check_add(home: &Path, text: &str, name: Option<&str>) -> PluginAddCheck {
    match add_parse::parse(text, name, home) {
        Ok(items) => PluginAddCheck {
            items: items.iter().map(describe).collect(),
            error: None,
            needs_name: false,
        },
        Err(err) => PluginAddCheck {
            items: Vec::new(),
            needs_name: matches!(err, add_parse::AddError::NoName),
            error: Some(err.to_string()),
        },
    }
}

/// Every MCP server name the agents know, in every project, but those of `except`'s plugin.
fn server_names(ctx: &Ctx<'_>, except: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    for project in std::iter::once(None).chain(ctx.projects.iter().map(|p| Some(p.as_path()))) {
        for server in ctx.read(project).servers {
            let ours = except.is_some_and(|name| {
                server
                    .plugin
                    .as_ref()
                    .is_some_and(|plugin| plugin.id == plugin_id(name))
            });
            if !ours && !names.contains(&server.name) {
                names.push(server.name);
            }
        }
    }
    names
}

/// A scratch folder for one fetch, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-plugin-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).map_err(|err| format!("Couldn't make a folder: {err}."))?;
        Ok(Self(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `item`'s files, fetched: its skill's folder, and where it came from when fetched.
fn fetch(item: &Item, scratch: &Scratch) -> Result<(Option<PathBuf>, Option<Origin>), String> {
    let Item::Skill(spec) = item else {
        return Ok((None, None));
    };
    let dir = library::fetch_skill(&spec.source, &scratch.0)?;
    let origin = library::fetched_commit(&scratch.0).map(|commit| Origin {
        source: spec.source.clone(),
        commit,
    });
    Ok((Some(dir), origin))
}

/// The name a fetched skill goes by: the one asked for, its `SKILL.md`'s, else its folder's.
fn skill_name(spec: &SkillSpec, dir: &Path) -> String {
    spec.name
        .clone()
        .or_else(|| library::skill_name(dir))
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

/// Starts a packed stdio server as the first agent's copy does and asks it for its tools;
/// why it didn't answer.
fn check_started(ctx: &Ctx<'_>, entry: &Entry) -> Option<String> {
    let first = adapters()[0].agent();
    let (command, args) = ctx.library.server_command(entry, first)?;
    let env: BTreeMap<String, String> = ctx.env.iter().cloned().collect();
    library::check_tools(
        &command,
        &args,
        &env,
        None,
        std::time::Duration::from_secs(60),
    )
    .err()
}

/// What Add's Advanced asks for beyond the name and where.
#[derive(Debug, Clone, Default)]
pub(crate) struct AddOptions {
    /// Only these agents get it (`None`: every agent).
    pub agents: Option<Vec<Agent>>,
    /// Set or added to the one pasted server's env (a command) or headers (an address). They
    /// go to the Keychain, as pasted values do, never into a file.
    pub values: BTreeMap<String, String>,
}

/// Gives the one server in `items` `values`: each replaces the pasted value of its name, or
/// is added.
fn give_values(items: &mut [Item], values: &BTreeMap<String, String>) -> Result<(), String> {
    if values.is_empty() {
        return Ok(());
    }
    let [Item::Server(spec)] = items else {
        return Err(
            "Headers and environment values go with one MCP server: paste just that server."
                .to_owned(),
        );
    };
    let kept = match &mut spec.transport {
        TransportSpec::Stdio { env, .. } => env,
        TransportSpec::Http { headers, .. } => headers,
    };
    for (name, value) in values {
        let name = name.trim();
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || c == '=') {
            return Err(format!(
                "\"{name}\" can't be a name: use letters, digits, - or _."
            ));
        }
        match kept.iter_mut().find(|(have, _)| have == name) {
            Some((_, secret)) => *secret = Secret::new(value.clone()),
            None => kept.push((name.to_owned(), Secret::new(value.clone()))),
        }
    }
    Ok(())
}

/// Adds what `text` holds to the Library and gives it to every agent (or the ones `options`
/// picks): in `project` only ("just me" unless `shared`), else everywhere. Its keys, and what
/// the user should know.
pub(crate) fn add(
    ctx: &Ctx<'_>,
    text: &str,
    name: Option<&str>,
    project: Option<&Path>,
    shared: bool,
    options: &AddOptions,
) -> Result<(Vec<String>, Option<String>), String> {
    if options.agents.as_ref().is_some_and(Vec::is_empty) {
        return Err("Pick at least one agent to add it to.".to_owned());
    }
    let picked = |agent: Agent| {
        options
            .agents
            .as_ref()
            .is_none_or(|agents| agents.contains(&agent))
    };
    let mut items = add_parse::parse(text, name, &ctx.roots.home).map_err(|err| err.to_string())?;
    give_values(&mut items, &options.values)?;
    let place = match project {
        Some(path) => Place::Project {
            path: path.to_owned(),
            shared,
        },
        None => Place::Everywhere,
    };
    let mut keys = Vec::new();
    let mut warning = None;
    let mut failed = None;
    for item in &items {
        let scratch = Scratch::new()?;
        let (skill_dir, origin) = fetch(item, &scratch)?;
        let name = match (item, &skill_dir) {
            (Item::Server(spec), _) => name.map_or_else(|| spec.name.clone(), str::to_owned),
            (Item::Skill(spec), Some(dir)) => skill_name(spec, dir),
            (Item::Skill(_), None) => return Err("The skill's files weren't fetched.".to_owned()),
        };
        let packed = ctx
            .library
            .pack(
                &name,
                item,
                skill_dir.as_deref(),
                &server_names(ctx, None),
                false,
                ctx.secrets,
            )
            .map_err(|err| err.to_string())?;
        if origin.is_some() {
            ctx.library
                .set_origin(&name, origin.as_ref())
                .map_err(|err| err.to_string())?;
        }
        let entry = packed.entry;
        let all = ctx.each_of(picked, |adapter| {
            ctx.library
                .sync(adapter, &entry, &place, false)
                .map(Some)
                .map_err(|err| err.0)
        });
        if !all {
            failed = Some(format!("Not every agent could add {name}."));
        }
        if packed.server.is_some()
            && let Some(why) = check_started(ctx, &entry)
        {
            warning = Some(format!("{name} is added, but it didn't start: {why}"));
        }
        keys.push(format!("lib:{name}"));
    }
    Ok((keys, failed.or(warning)))
}

/// What Use everywhere moved, kept for its Undo.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Moved {
    originals: Vec<Original>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Original {
    /// An agent's own server: put back by its own commands. Its values are in the Keychain,
    /// under `secrets`.
    Server {
        agent: ProviderKind,
        name: String,
        /// Its project, for one kept to a project.
        project: Option<PathBuf>,
        transport: SavedTransport,
        secrets: String,
    },
    /// A skill's folder (or link), moved aside to `kept`.
    Skill { from: PathBuf, kept: PathBuf },
}

/// A server's transport without its values: their names only.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum SavedTransport {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<String>,
        cwd: Option<PathBuf>,
    },
    Http {
        url: String,
        headers: Vec<String>,
    },
}

impl Original {
    /// Puts it back as it was.
    fn restore(&self, ctx: &Ctx<'_>) -> Result<(), String> {
        match self {
            Self::Skill { from, kept } => {
                if let Some(dir) = from.parent() {
                    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
                }
                std::fs::rename(kept, from)
                    .map_err(|err| format!("Couldn't put {} back: {err}.", from.display()))
            }
            Self::Server {
                agent,
                name,
                project,
                transport,
                secrets,
            } => {
                let mut values = ctx.secrets.load(secrets)?;
                let mut take = |names: &[String]| -> Vec<(String, Secret)> {
                    names
                        .iter()
                        .map(|name| {
                            (
                                name.clone(),
                                Secret::new(values.remove(name).unwrap_or_default()),
                            )
                        })
                        .collect()
                };
                let transport = match transport {
                    SavedTransport::Stdio {
                        command,
                        args,
                        env,
                        cwd,
                    } => TransportSpec::Stdio {
                        command: command.clone(),
                        args: args.clone(),
                        env: take(env),
                        cwd: cwd.clone(),
                    },
                    SavedTransport::Http { url, headers } => TransportSpec::Http {
                        url: url.clone(),
                        headers: take(headers),
                    },
                };
                let spec = ServerSpec {
                    name: name.clone(),
                    transport,
                };
                let place = project.as_deref().map_or(Place::Everywhere, just_me);
                let agent = adapter(
                    agent_of(*agent).ok_or("Brigadier no longer drives the agent it came from.")?,
                );
                let steps = agent.add_server(name, &spec, &place).map_err(|err| err.0)?;
                ctx.run(agent, &steps)?;
                let _ = ctx.secrets.forget(secrets);
                Ok(())
            }
        }
    }
}

fn moved_dir(library: &Library) -> PathBuf {
    library.root().join(".moved")
}

fn moved_file(library: &Library, name: &str) -> PathBuf {
    moved_dir(library).join(format!("{name}.json"))
}

fn read_moved(library: &Library, name: &str) -> Option<Moved> {
    serde_json::from_str(&std::fs::read_to_string(moved_file(library, name)).ok()?).ok()
}

/// Whether Library item `name` came from Use everywhere and can be undone.
pub(crate) fn can_undo(library: &Library, name: &str) -> bool {
    moved_file(library, name).is_file()
}

fn spec_of(name: &str, transport: &Transport) -> ServerSpec {
    ServerSpec {
        name: name.to_owned(),
        transport: match transport {
            Transport::Stdio {
                command,
                args,
                env,
                cwd,
                ..
            } => TransportSpec::Stdio {
                command: command.clone(),
                args: args.clone(),
                env: env.clone(),
                cwd: cwd.clone(),
            },
            Transport::Http { url, headers } => TransportSpec::Http {
                url: url.clone(),
                headers: headers.clone(),
            },
        },
    }
}

/// A user's own skill folder Brigadier may move aside: the agents' user folders, never one
/// synced from a Claude account, a system skill or a plugin's.
fn movable(root: &SkillRoot) -> bool {
    matches!(
        root,
        SkillRoot::AgentsUser | SkillRoot::ClaudeUser | SkillRoot::CodexUser
    )
}

/// What Use everywhere starts from: the item, where its copy goes, and its originals.
struct Found {
    name: String,
    item: Item,
    /// The original skill's folder, to copy.
    skill_dir: Option<PathBuf>,
    /// `None`: every project; else the projects it was kept to.
    projects: Option<Vec<PathBuf>>,
    servers: Vec<(Agent, Option<PathBuf>, ServerSpec)>,
    skills: Vec<PathBuf>,
}

fn find(ctx: &Ctx<'_>, target: &Target) -> Result<Found, String> {
    let mut found: Option<Found> = None;
    let mut everywhere = false;
    let mut kept_to = Vec::new();
    for catalog in
        std::iter::once(ctx.read(None)).chain(ctx.projects.iter().map(|path| ctx.read(Some(path))))
    {
        match target {
            Target::Server(name) | Target::Other(name) => {
                for server in &catalog.servers {
                    let other = matches!(target, Target::Other(_));
                    let matches = if other {
                        server.id == *name
                    } else {
                        server.name == *name && server.plugin.is_none()
                    };
                    if !matches {
                        continue;
                    }
                    let spec = spec_of(&server.name, &server.transport);
                    let entry = found.get_or_insert_with(|| Found {
                        name: server.name.clone(),
                        item: Item::Server(spec.clone()),
                        skill_dir: None,
                        projects: None,
                        servers: Vec::new(),
                        skills: Vec::new(),
                    });
                    if other {
                        everywhere = true;
                        continue;
                    }
                    if server.source == Source::ClaudeProject {
                        return Err(format!(
                            "{} is shared with your team in {}, so Brigadier leaves it as it is.",
                            server.name,
                            server.file.display()
                        ));
                    }
                    let Some(agent) = owner(server.source) else {
                        continue;
                    };
                    match &server.project {
                        Some(path) => push(&mut kept_to, path.clone()),
                        None => everywhere = true,
                    }
                    let known = (agent, server.project.clone());
                    if !entry
                        .servers
                        .iter()
                        .any(|(a, p, _)| (*a, p.clone()) == known)
                    {
                        entry.servers.push((agent, server.project.clone(), spec));
                    }
                }
            }
            Target::Skill(name) => {
                for skill in catalog
                    .skills
                    .iter()
                    .filter(|skill| skill.name == *name && skill.plugin().is_none())
                {
                    if let Some((root, _)) = skill.entries.iter().find(|(root, _)| !movable(root))
                        && !matches!(root, SkillRoot::ClaudeSynced(_))
                    {
                        return Err(format!(
                            "{name} is part of a project's own files, so Brigadier leaves it as it is."
                        ));
                    }
                    let entry = found.get_or_insert_with(|| skill_found(name, skill));
                    everywhere = true;
                    for (root, path) in &skill.entries {
                        if movable(root) && !entry.skills.contains(path) {
                            entry.skills.push(path.clone());
                        }
                    }
                }
            }
            Target::Library(_) | Target::Plugin(_) => {
                return Err("It's already one every agent can have.".to_owned());
            }
        }
    }
    let mut found = found.ok_or("It's gone: none of your agents has it now.")?;
    found.projects = (!everywhere).then_some(kept_to);
    Ok(found)
}

fn skill_found(name: &str, skill: &Skill) -> Found {
    Found {
        name: name.to_owned(),
        item: Item::Skill(SkillSpec {
            name: Some(name.to_owned()),
            source: SkillSource::Folder(skill.dir.clone()),
        }),
        skill_dir: Some(skill.dir.clone()),
        projects: None,
        servers: Vec::new(),
        skills: Vec::new(),
    }
}

fn push<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

/// Whether `agent` has Library item `id`'s skills or servers where it should: every project,
/// or each of `projects`.
fn lists(ctx: &Ctx<'_>, agent: Agent, id: &str, projects: Option<&[PathBuf]>) -> bool {
    let has = |catalog: &Catalog| {
        catalog.servers.iter().any(|server| {
            server
                .plugin
                .as_ref()
                .is_some_and(|p| p.agent == agent && p.id == id)
        }) || catalog.skills.iter().any(|skill| {
            // One skill copied into several plugins is one entry, with each as a root.
            skill
                .roots
                .iter()
                .any(|root| matches!(root, SkillRoot::Plugin(p) if p.agent == agent && p.id == id))
        })
    };
    match projects {
        None => has(&ctx.read(None)),
        Some(projects) => projects.iter().all(|path| has(&ctx.read(Some(path)))),
    }
}

/// Moves an agent's own server or skill into the Library and gives it to every agent, where it
/// was: every project, or the projects it was kept to. The original goes only once every agent
/// has the copy staged, and comes back if any agent doesn't end up with it (D5). Its key.
pub(crate) fn use_everywhere(ctx: &Ctx<'_>, key: &str) -> Result<String, String> {
    let target = target(key)?;
    let found = find(ctx, &target)?;
    let name = found.name.clone();
    if ctx.library.entry(&name).is_some() {
        return Err(LibraryError::NameTaken(name).to_string());
    }
    let mut taken = server_names(ctx, None);
    taken.retain(|known| *known != name);
    let packed = ctx
        .library
        .pack(
            &name,
            &found.item,
            found.skill_dir.as_deref(),
            &taken,
            false,
            ctx.secrets,
        )
        .map_err(|err| err.to_string())?;
    let id = packed.entry.id();
    let undo_all = |ctx: &Ctx<'_>, originals: &[Original]| {
        for original in originals {
            let _ = original.restore(ctx);
        }
        for adapter in adapters() {
            let _ = ctx.run(adapter, &removal(ctx, adapter, &id));
        }
        let _ = ctx.library.remove(&name);
    };

    // 1. Every agent has the copy, off: two servers of one name (or address) don't both load.
    let staged = ctx.each(None, |adapter| {
        let mut steps = ctx.library.market_steps(adapter);
        if !adapter.installed(ctx.roots, &id) {
            steps.extend(
                adapter
                    .install_plugin(&id, &Place::Everywhere)
                    .map_err(|err| err.0)?,
            );
        }
        steps.extend(
            adapter
                .set_plugin(&id, &Place::Everywhere, false)
                .map_err(|err| err.0)?,
        );
        Ok(Some(steps))
    });
    let missing: Vec<&str> = adapters()
        .into_iter()
        .filter(|adapter| !adapter.installed(ctx.roots, &id))
        .map(|adapter| adapter.label())
        .collect();
    if !staged || !missing.is_empty() {
        undo_all(ctx, &[]);
        return Err(format!(
            "Nothing was changed: {} couldn't take the copy.",
            if missing.is_empty() {
                "an agent".to_owned()
            } else {
                missing.join(" and ")
            }
        ));
    }

    // 2. The originals go, kept for Undo.
    let mut originals = Vec::new();
    let kept = moved_dir(ctx.library).join(&name);
    for (n, (agent, project, spec)) in found.servers.iter().enumerate() {
        let secrets = format!("{name}/original-{n}");
        let (transport, values) = match &spec.transport {
            TransportSpec::Stdio {
                command,
                args,
                env,
                cwd,
            } => (
                SavedTransport::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                    env: env.iter().map(|(name, _)| name.clone()).collect(),
                    cwd: cwd.clone(),
                },
                env,
            ),
            TransportSpec::Http { url, headers } => (
                SavedTransport::Http {
                    url: url.clone(),
                    headers: headers.iter().map(|(name, _)| name.clone()).collect(),
                },
                headers,
            ),
        };
        for (value_name, value) in values {
            if let Err(err) = ctx.secrets.store(&secrets, value_name, value) {
                undo_all(ctx, &originals);
                return Err(format!("Nothing was changed: {err}"));
            }
        }
        let place = project.as_deref().map_or(Place::Everywhere, just_me);
        let agent_adapter = adapter(*agent);
        let removed = agent_adapter
            .remove_server(&spec.name, &place)
            .map_err(|err| err.0)
            .and_then(|steps| ctx.run(agent_adapter, &steps));
        if let Err(err) = removed {
            let _ = ctx.secrets.forget(&secrets);
            undo_all(ctx, &originals);
            return Err(format!("Nothing was changed: {err}"));
        }
        originals.push(Original::Server {
            agent: provider(*agent),
            name: spec.name.clone(),
            project: project.clone(),
            transport,
            secrets,
        });
    }
    for (n, from) in found.skills.iter().enumerate() {
        let to = kept.join(format!("skill-{n}"));
        let moved = std::fs::create_dir_all(&kept)
            .and_then(|()| std::fs::rename(from, &to))
            .map_err(|err| format!("Couldn't move {} aside: {err}.", from.display()));
        if let Err(err) = moved {
            undo_all(ctx, &originals);
            return Err(format!("Nothing was changed: {err}"));
        }
        originals.push(Original::Skill {
            from: from.clone(),
            kept: to,
        });
    }
    let record = Moved {
        originals: originals.clone(),
    };
    let saved = serde_json::to_string_pretty(&record)
        .map_err(|err| err.to_string())
        .and_then(|text| {
            std::fs::create_dir_all(moved_dir(ctx.library))
                .and_then(|()| std::fs::write(moved_file(ctx.library, &name), text))
                .map_err(|err| err.to_string())
        });
    if let Err(err) = saved {
        undo_all(ctx, &originals);
        return Err(format!("Nothing was changed: {err}"));
    }

    // 3. The copy goes on where the original was, and every agent must have it.
    let on = ctx.each(None, |adapter| match &found.projects {
        None => plugin_plan(ctx, adapter, &id, None, true, true),
        Some(projects) => {
            let mut steps = Vec::new();
            for path in projects {
                steps.extend(
                    plugin_plan(ctx, adapter, &id, Some(path), true, true)?.unwrap_or_default(),
                );
            }
            Ok(Some(steps))
        }
    });
    let without: Vec<&str> = adapters()
        .into_iter()
        .filter(|adapter| !lists(ctx, adapter.agent(), &id, found.projects.as_deref()))
        .map(|adapter| adapter.label())
        .collect();
    if !on || !without.is_empty() {
        undo_all(ctx, &originals);
        let _ = std::fs::remove_file(moved_file(ctx.library, &name));
        let _ = std::fs::remove_dir_all(&kept);
        return Err(format!(
            "Nothing was changed: {} didn't pick up the copy, so the original stays.",
            if without.is_empty() {
                "an agent".to_owned()
            } else {
                without.join(" and ")
            }
        ));
    }
    Ok(format!("lib:{name}"))
}

/// The steps that take Library plugin `id` away from `adapter`'s agent: off in each project
/// that turns it on by itself (such a project keeps loading it otherwise), then uninstalled.
fn removal(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    for path in ctx.projects {
        if kept_on_in(ctx, adapter, id, path) {
            steps.extend(
                adapter
                    .set_plugin(id, &just_me(path), false)
                    .unwrap_or_default(),
            );
        }
    }
    // An install of a project's own goes too: turned off, it would stay behind as that
    // project's item. Only where the folder still is, for the CLI runs in it.
    for place in adapter.installed_in_projects(ctx.roots, id) {
        if matches!(&place, Place::Project { path, .. } if path.is_dir()) {
            steps.extend(adapter.remove_plugin(id, &place).unwrap_or_default());
        }
    }
    if adapter.installed(ctx.roots, id) {
        steps.extend(
            adapter
                .remove_plugin(id, &Place::Everywhere)
                .unwrap_or_default(),
        );
    }
    steps
}

/// Undoes Use everywhere: the originals come back, the copy goes from every agent and the
/// Library.
pub(crate) fn undo_use_everywhere(ctx: &Ctx<'_>, key: &str) -> Result<(), String> {
    let Target::Library(name) = target(key)? else {
        return Err("Only an item Use everywhere moved can be undone.".to_owned());
    };
    let moved = read_moved(ctx.library, &name).ok_or_else(|| {
        format!("{name} wasn't moved by Use everywhere, so there's nothing to undo.")
    })?;
    let id = plugin_id(&name);
    // Off first: an original and its copy don't both load.
    ctx.each(None, |adapter| {
        if !adapter.installed(ctx.roots, &id) {
            return Ok(None);
        }
        adapter
            .set_plugin(&id, &Place::Everywhere, false)
            .map(Some)
            .map_err(|err| err.0)
    });
    let mut errors = Vec::new();
    for original in &moved.originals {
        if let Err(err) = original.restore(ctx) {
            errors.push(err);
        }
    }
    if !errors.is_empty() {
        return Err(errors.join(" "));
    }
    remove_library(ctx, &name)?;
    let _ = std::fs::remove_file(moved_file(ctx.library, &name));
    let _ = std::fs::remove_dir_all(moved_dir(ctx.library).join(&name));
    Ok(())
}

fn remove_library(ctx: &Ctx<'_>, name: &str) -> Result<(), String> {
    let id = plugin_id(name);
    let all = ctx.each(None, |adapter| {
        let steps = removal(ctx, adapter, &id);
        Ok((!steps.is_empty()).then_some(steps))
    });
    if !all {
        return Err(format!(
            "Not every agent let go of {name}; it stays in your Library."
        ));
    }
    let servers = server_names_of(ctx, name);
    ctx.library.remove(name).map_err(|err| err.to_string())?;
    let _ = ctx.library.set_origin(name, None);
    for server in servers {
        let _ = ctx.secrets.forget(&format!("{name}/{server}"));
    }
    Ok(())
}

/// The servers Library item `name` packs.
fn server_names_of(ctx: &Ctx<'_>, name: &str) -> Vec<String> {
    let Some(entry) = ctx.library.entry(name) else {
        return Vec::new();
    };
    std::fs::read_to_string(entry.dir.join(".mcp.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|value| {
            value
                .get("mcpServers")?
                .as_object()
                .map(|servers| servers.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// Where each agent has its own server `name` (not a plugin's), on or off: what Remove takes.
fn server_places(ctx: &Ctx<'_>, name: &str) -> Result<Vec<(Agent, Place)>, String> {
    let mut places: Vec<(Agent, Place)> = Vec::new();
    for project in std::iter::once(None).chain(ctx.projects.iter().map(|p| Some(p.as_path()))) {
        let catalog = ctx.read(project);
        for server in catalog.servers.into_iter().chain(catalog.off.servers) {
            if server.name != name || server.plugin.is_some() {
                continue;
            }
            if server.source == Source::ClaudeProject {
                return Err(format!(
                    "{name} is shared with your team in {}: remove it there.",
                    server.file.display()
                ));
            }
            if let Some(agent) = owner(server.source) {
                push(
                    &mut places,
                    (
                        agent,
                        server.project.as_deref().map_or(Place::Everywhere, just_me),
                    ),
                );
            }
        }
    }
    Ok(places)
}

/// Removes item `key` from every agent and project: through each agent's own commands.
pub(crate) fn remove(ctx: &Ctx<'_>, key: &str) -> Result<(), String> {
    match target(key)? {
        Target::Library(name) => {
            remove_library(ctx, &name)?;
            let _ = std::fs::remove_file(moved_file(ctx.library, &name));
            let _ = std::fs::remove_dir_all(moved_dir(ctx.library).join(&name));
            Ok(())
        }
        Target::Plugin(name) => {
            ctx.each(None, |adapter| {
                Ok(plugin_id_of(ctx, adapter.agent(), &name).map(|id| removal(ctx, adapter, &id)))
            });
            Ok(())
        }
        Target::Server(name) => {
            let places = server_places(ctx, &name)?;
            ctx.each(None, |adapter| {
                let mut steps = Vec::new();
                for (_, place) in places.iter().filter(|(agent, _)| *agent == adapter.agent()) {
                    steps.extend(adapter.remove_server(&name, place).map_err(|err| err.0)?);
                }
                Ok((!steps.is_empty()).then_some(steps))
            });
            Ok(())
        }
        Target::Skill(name) => {
            let found = find(ctx, &Target::Skill(name.clone()))?;
            let trash = ctx.roots.home.join(".Trash");
            for from in &found.skills {
                let to = trash.join(format!(
                    "{name} {}",
                    uuid::Uuid::new_v4()
                        .simple()
                        .to_string()
                        .get(..6)
                        .unwrap_or_default()
                ));
                std::fs::create_dir_all(&trash)
                    .and_then(|()| std::fs::rename(from, &to))
                    .map_err(|err| {
                        format!("Couldn't move {} to the Trash: {err}.", from.display())
                    })?;
            }
            Ok(())
        }
        Target::Other(_) => Err("It belongs to another app: remove it there.".to_owned()),
    }
}

/// Fetches Library item `key`'s source again and brings every agent's copy up to date.
pub(crate) fn update(ctx: &Ctx<'_>, key: &str) -> Result<(), String> {
    let Target::Library(name) = target(key)? else {
        return Err("Only an item in your Library can be updated here.".to_owned());
    };
    let origin = ctx
        .library
        .origin(&name)
        .ok_or_else(|| format!("{name} has nothing to update from."))?;
    let scratch = Scratch::new()?;
    let item = Item::Skill(SkillSpec {
        name: Some(name.clone()),
        source: origin.source.clone(),
    });
    let (dir, fetched) = fetch(&item, &scratch)?;
    let packed = ctx
        .library
        .pack(
            &name,
            &item,
            dir.as_deref(),
            &server_names(ctx, Some(&name)),
            true,
            ctx.secrets,
        )
        .map_err(|err| err.to_string())?;
    if let Some(fetched) = &fetched {
        ctx.library
            .set_origin(&name, Some(fetched))
            .map_err(|err| err.to_string())?;
    }
    if !packed.changed {
        return Ok(());
    }
    let id = packed.entry.id();
    let all = ctx.each(None, |adapter| {
        let mut steps = ctx.library.market_steps(adapter);
        if adapter.installed(ctx.roots, &id) {
            steps.extend(
                adapter
                    .update_plugin(&id, &Place::Everywhere)
                    .map_err(|err| err.0)?,
            );
        }
        for path in ctx.projects {
            if plugin_on(&ctx.read(Some(path)), adapter.agent(), &id, true)
                && !adapter.installed(ctx.roots, &id)
            {
                steps.extend(
                    adapter
                        .update_plugin(&id, &just_me(path))
                        .map_err(|err| err.0)?,
                );
            }
        }
        Ok(Some(steps))
    });
    if all {
        Ok(())
    } else {
        Err(format!("Not every agent took the new version of {name}."))
    }
}

/// Whether Library item `name`'s source has moved on since it was fetched (reads the remote).
pub(crate) fn update_waiting(library: &Library, name: &str) -> bool {
    library.origin(name).is_some_and(|origin| {
        library::latest_commit(&origin.source).is_ok_and(|latest| latest != origin.commit)
    })
}

/// The agent of `kind`, among those Brigadier drives.
pub(crate) fn agent_of(kind: ProviderKind) -> Option<Agent> {
    adapters()
        .into_iter()
        .map(|adapter| adapter.agent())
        .find(|agent| provider(*agent) == kind)
}

/// The page's agent order, each waiting.
pub(crate) fn waiting() -> Vec<PluginJobAgent> {
    adapters()
        .iter()
        .map(|adapter| PluginJobAgent {
            agent: provider(adapter.agent()),
            step: PluginStep::Waiting,
        })
        .collect()
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;
