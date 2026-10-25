//! Settings › Plugins' actions on the Library and plugins: placing a plugin per agent (on in
//! every project, in some, off), Add, Update, and taking a Library plugin away.
//!
//! Every action is carried out by each agent's own commands (and, for Codex in one project, the
//! project's own file: D8), planned by its adapter, so a plain `claude` or `codex` started in a
//! terminal gets what the page shows. Nothing here names an agent: each runs through
//! [`adapters()`]. Sharing an agent's own items with the others is [`super::share`]'s.
//!
//! - A Library item (`lib:<name>`) is a plugin every agent installs. Every project: on in the
//!   user's scope. One project: on in that project's scope; when it is off in the user's scope
//!   that makes it that project's only (Claude Code's `--scope local`, Codex's project file).
//! - Another marketplace's plugin is switched the same way; it isn't installed by Brigadier.
//! - What the old Use everywhere moved aside ([`Moved`]) stays the user's: Remove puts it back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::adapter::{AgentAdapter, Place, Step, adapter, adapters, apply};
use brigadier_registry::add_parse;
use brigadier_registry::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};
use brigadier_registry::library::{self, Entry, Library, Origin, SecretSink, plugin_id};
use brigadier_registry::{Agent, Catalog, Roots, Secret};
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
    pub(super) fn run(&self, adapter: &dyn AgentAdapter, steps: &[Step]) -> Result<(), String> {
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
}

pub(super) fn just_me(project: &Path) -> Place {
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
pub(super) fn plugin_id_of(ctx: &Ctx<'_>, agent: Agent, name: &str) -> Option<String> {
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

/// Where one agent has a plugin: what the scope sets, and what a failed move puts back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placed {
    /// It has it at all: installed, for every project or one, or on somewhere.
    pub has: bool,
    /// On in the user's scope: every project, but where a project turns it off.
    pub global: bool,
    /// The user's projects it is on in.
    pub on_in: Vec<PathBuf>,
}

impl Placed {
    /// Not had at all.
    pub fn none() -> Self {
        Self {
            has: false,
            global: false,
            on_in: Vec::new(),
        }
    }

    /// On in every project (`None`) or in exactly `projects`, of the user's `all`.
    pub fn at(scope: Option<&[PathBuf]>, all: &[PathBuf]) -> Self {
        Self {
            has: true,
            global: scope.is_none(),
            on_in: scope.map_or_else(|| all.to_vec(), <[PathBuf]>::to_vec),
        }
    }

    /// Installed, off in every project.
    pub fn off() -> Self {
        Self {
            has: true,
            global: false,
            on_in: Vec::new(),
        }
    }

    /// The same places, whatever order the projects come in.
    pub fn same(&self, other: &Self) -> bool {
        let sorted = |paths: &[PathBuf]| {
            let mut paths = paths.to_vec();
            paths.sort();
            paths
        };
        self.has == other.has
            && (!self.has
                || (self.global == other.global && sorted(&self.on_in) == sorted(&other.on_in)))
    }
}

pub(super) fn placed(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Placed {
    let agent = adapter.agent();
    let global = plugin_on(&ctx.read(None), agent, id, false);
    let on_in: Vec<PathBuf> = ctx
        .projects
        .iter()
        .filter(|path| plugin_on(&ctx.read(Some(path)), agent, id, false))
        .cloned()
        .collect();
    let has = global
        || !on_in.is_empty()
        || adapter.installed(ctx.roots, id)
        || !adapter.installed_in_projects(ctx.roots, id).is_empty();
    Placed { has, global, on_in }
}

/// What an agent's own settings hold of a Library plugin beyond where it is on: its installs
/// and the projects that switch it on by themselves. A failed change that puts every agent
/// back where it was can still leave these changed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    placed: Placed,
    installed: bool,
    /// The projects with an install of their own.
    installed_in: Vec<PathBuf>,
    /// The projects whose own settings switch it on.
    switched_on: Vec<PathBuf>,
}

fn held(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Held {
    Held {
        placed: placed(ctx, adapter, id),
        installed: adapter.installed(ctx.roots, id),
        installed_in: adapter
            .installed_in_projects(ctx.roots, id)
            .into_iter()
            .filter_map(|place| match place {
                Place::Project { path, .. } => Some(path),
                Place::Everywhere => None,
            })
            .collect(),
        switched_on: ctx
            .projects
            .iter()
            .filter(|path| adapter.switched_on_in(path, id))
            .cloned()
            .collect(),
    }
}

/// One move of a Library plugin for one agent, planned against the agent's setup as it is when
/// it runs: installing stays apart from switching it on for every project or for one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Move {
    /// `project`'s own settings switch it on, so it stays on there whatever every project
    /// gets (an install of the project's own can follow the user's switch otherwise).
    Keep(PathBuf),
    /// Installed for the agent, switched off for every project.
    Install,
    /// On or off in `project`, over what every project gets.
    Set(PathBuf, bool),
    /// On or off in the user's scope (installed first when on).
    Global(bool),
    /// Taken away from the agent: it had none before.
    Remove,
}

impl Move {
    /// It puts the item on somewhere (or installs it): a put-back runs these first.
    fn enables(&self) -> bool {
        matches!(
            self,
            Self::Keep(_) | Self::Install | Self::Global(true) | Self::Set(_, true)
        )
    }
}

fn move_steps(
    ctx: &Ctx<'_>,
    adapter: &dyn AgentAdapter,
    id: &str,
    step: &Move,
) -> Result<Vec<Step>, String> {
    let agent = adapter.agent();
    let installed = adapter.installed(ctx.roots, id);
    let installed_in = |path: &Path| {
        adapter
            .installed_in_projects(ctx.roots, id)
            .iter()
            .any(|place| matches!(place, Place::Project { path: p, .. } if p == path))
    };
    let unsupported = |err: brigadier_registry::adapter::Unsupported| err.0;
    let mut steps = Vec::new();
    match step {
        Move::Keep(path) if adapter.switched_on_in(path, id) => {}
        Move::Keep(path) if installed || installed_in(path) => steps.extend(
            adapter
                .set_plugin(id, &just_me(path), true)
                .map_err(unsupported)?,
        ),
        Move::Keep(path) => {
            steps.extend(ctx.library.market_steps(adapter));
            steps.extend(
                adapter
                    .install_plugin(id, &just_me(path))
                    .map_err(unsupported)?,
            );
        }
        Move::Install if installed || !adapter.installed_in_projects(ctx.roots, id).is_empty() => {}
        Move::Install => {
            steps.extend(ctx.library.market_steps(adapter));
            steps.extend(
                adapter
                    .install_plugin(id, &Place::Everywhere)
                    .map_err(unsupported)?,
            );
            steps.extend(
                adapter
                    .set_plugin(id, &Place::Everywhere, false)
                    .map_err(unsupported)?,
            );
        }
        Move::Set(path, on) => {
            if plugin_on(&ctx.read(Some(path)), agent, id, false) != *on {
                steps.extend(
                    adapter
                        .set_plugin(id, &just_me(path), *on)
                        .map_err(unsupported)?,
                );
            }
        }
        Move::Global(on) => {
            let now = plugin_on(&ctx.read(None), agent, id, false);
            if *on && !installed {
                steps.extend(ctx.library.market_steps(adapter));
                steps.extend(
                    adapter
                        .install_plugin(id, &Place::Everywhere)
                        .map_err(unsupported)?,
                );
            }
            if (*on && (!installed || !now)) || (!*on && installed && now) {
                steps.extend(
                    adapter
                        .set_plugin(id, &Place::Everywhere, *on)
                        .map_err(unsupported)?,
                );
            }
        }
        Move::Remove => steps.extend(removal(ctx, adapter, id)),
    }
    Ok(steps)
}

/// The moves that bring an agent to `want`, in three rounds every agent takes together: on
/// where it goes first, then every project's, then off where it leaves, so it is never off
/// where it stays on.
fn moves_to(want: &Placed, projects: &[PathBuf]) -> [Vec<Move>; 3] {
    if !want.has {
        return [vec![Move::Remove], Vec::new(), Vec::new()];
    }
    if want.global {
        return [
            vec![Move::Global(true)],
            projects
                .iter()
                .map(|path| Move::Set(path.clone(), want.on_in.contains(path)))
                .collect(),
            Vec::new(),
        ];
    }
    let mut first: Vec<Move> = want.on_in.iter().cloned().map(Move::Keep).collect();
    if first.is_empty() {
        first.push(Move::Install);
    }
    [
        first,
        vec![Move::Global(false)],
        projects
            .iter()
            .filter(|path| !want.on_in.contains(path))
            .map(|path| Move::Set(path.clone(), false))
            .collect(),
    ]
}

/// A folder's name, as the page shows a project.
pub(super) fn folder_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// "a", "a and b", "a, b and c", of folders.
fn folder_names<'a>(paths: impl IntoIterator<Item = &'a PathBuf>) -> String {
    let names: Vec<String> = paths.into_iter().map(|path| folder_name(path)).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Where `placed` has it, in words: "has it on in every project", "has it on only in a and b".
fn where_on(placed: &Placed, projects: &[PathBuf]) -> String {
    if !placed.has {
        return "doesn't have it".to_owned();
    }
    if placed.global {
        let off: Vec<&PathBuf> = projects
            .iter()
            .filter(|path| !placed.on_in.contains(path))
            .collect();
        return if off.is_empty() {
            "has it on in every project".to_owned()
        } else {
            format!("has it on in every project but {}", folder_names(off))
        };
    }
    if placed.on_in.is_empty() {
        "has it off in every project".to_owned()
    } else {
        format!("has it on only in {}", folder_names(&placed.on_in))
    }
}

/// What a change that failed left changed, agent by agent: "Codex has it on only in a, where it
/// had it on in every project." `None` when every agent has it where it had it.
fn left_changed(
    before: &[(Agent, Held)],
    after: &[(Agent, Held)],
    projects: &[PathBuf],
) -> Option<String> {
    let changed: Vec<String> = before
        .iter()
        .zip(after)
        .filter(|((_, was), (_, is))| !was.placed.same(&is.placed))
        .map(|((agent, was), (_, is))| {
            format!(
                "{} {}, where it {}",
                adapter(*agent).label(),
                where_on(&is.placed, projects),
                where_on(&was.placed, projects).replacen("has it", "had it", 1)
            )
        })
        .collect();
    (!changed.is_empty()).then(|| format!("{}.", changed.join("; ")))
}

/// What a change that failed left in the agents' own settings although each has it where it
/// had it: "Claude Code now switches it on by itself in a". Empty when nothing is.
fn settings_left(before: &[(Agent, Held)], after: &[(Agent, Held)]) -> Vec<String> {
    let mut left = Vec::new();
    for ((agent, was), (_, is)) in before.iter().zip(after) {
        let label = adapter(*agent).label();
        let gained = |now: &[PathBuf], then: &[PathBuf]| -> Vec<PathBuf> {
            now.iter()
                .filter(|path| !then.contains(path))
                .cloned()
                .collect()
        };
        if is.installed != was.installed {
            left.push(if is.installed {
                format!("{label} has it installed now")
            } else {
                format!("{label} no longer has it installed")
            });
        }
        for (now, then, more, less) in [
            (
                &is.installed_in,
                &was.installed_in,
                "has an install of its own in",
                "no longer has an install of its own in",
            ),
            (
                &is.switched_on,
                &was.switched_on,
                "now switches it on by itself in",
                "no longer switches it on by itself in",
            ),
        ] {
            let added = gained(now, then);
            if !added.is_empty() {
                left.push(format!("{label} {more} {}", folder_names(&added)));
            }
            let dropped = gained(then, now);
            if !dropped.is_empty() {
                left.push(format!("{label} {less} {}", folder_names(&dropped)));
            }
        }
    }
    left
}

/// Carries out one agent's steps.
pub(crate) type Runner<'a> = &'a dyn Fn(&dyn AgentAdapter, &[Step]) -> Result<(), String>;

/// Brings each agent of `wants` to its places, every agent taking each round before the next
/// starts, then reads each again: an agent that isn't where it should be failed.
/// `seen` keeps how each agent's settings read after its last move: one that reads otherwise
/// later was changed by someone else meanwhile.
fn drive(
    ctx: &Ctx<'_>,
    id: &str,
    wants: &[(Agent, Placed)],
    run: Runner<'_>,
    seen: &mut Vec<(Agent, Held)>,
) -> Result<(), (Agent, String)> {
    for round in 0..3 {
        for (agent, want) in wants {
            let adapter = adapter(*agent);
            let moves = &moves_to(want, ctx.projects)[round];
            for step in moves {
                move_steps(ctx, adapter, id, step)
                    .and_then(|steps| run(adapter, &steps))
                    .map_err(|reason| (*agent, reason))?;
            }
            if !moves.is_empty() {
                seen.retain(|(known, _)| known != agent);
                seen.push((*agent, held(ctx, adapter, id)));
            }
        }
    }
    for (agent, want) in wants {
        let is = placed(ctx, adapter(*agent), id);
        if !is.same(want) {
            return Err((
                *agent,
                format!(
                    "it {} after the change",
                    where_on(&is, ctx.projects).replacen("has it ", "is ", 1)
                ),
            ));
        }
    }
    Ok(())
}

/// Puts each agent of `before` back where it had it: every move that puts it on somewhere
/// first, then, only if all of those went, the moves that take it off. Whether every move went.
fn put_back_with(ctx: &Ctx<'_>, id: &str, before: &[(Agent, Placed)], run: Runner<'_>) -> bool {
    let mut all = true;
    for enabling in [true, false] {
        if !all {
            break;
        }
        for round in 0..3 {
            for (agent, was) in before {
                let adapter = adapter(*agent);
                for step in moves_to(was, ctx.projects)[round]
                    .iter()
                    .filter(|step| step.enables() == enabling)
                {
                    all &= move_steps(ctx, adapter, id, step)
                        .and_then(|steps| run(adapter, &steps))
                        .is_ok();
                }
            }
        }
    }
    all
}

/// Brings each agent of `wants` to its places for Library plugin `id`. If a move fails, those
/// agents go back where they had it, as far as they can, and the error says what was left
/// changed: "Nothing was changed" only when the agents' settings read the same as before.
///
/// An agent whose settings someone else changed since this change last moved it (a terminal,
/// the user's own editor) isn't put back: what they did stays.
pub(super) fn change(
    ctx: &Ctx<'_>,
    id: &str,
    wants: &[(Agent, Placed)],
    run: Runner<'_>,
) -> Result<(), String> {
    let read = || -> Vec<(Agent, Held)> {
        adapters()
            .into_iter()
            .map(|adapter| (adapter.agent(), held(ctx, adapter, id)))
            .collect()
    };
    let before = read();
    for (agent, _) in wants {
        (ctx.report)(*agent, PluginStep::Working);
    }
    let mut seen = Vec::new();
    let Err((agent, reason)) = drive(ctx, id, wants, run, &mut seen) else {
        for (agent, _) in wants {
            (ctx.report)(*agent, PluginStep::Done);
        }
        return Ok(());
    };
    (ctx.report)(
        agent,
        PluginStep::Failed {
            reason: reason.clone(),
        },
    );
    let back: Vec<(Agent, Placed)> = before
        .iter()
        .filter(|(other, _)| wants.iter().any(|(wanted, _)| wanted == other))
        .filter(|(other, _)| {
            *other == agent
                || seen
                    .iter()
                    .all(|(known, then)| known != other || *then == held(ctx, adapter(*other), id))
        })
        .map(|(agent, held)| (*agent, held.placed.clone()))
        .collect();
    put_back_with(ctx, id, &back, run);
    let after = read();
    let label = adapter(agent).label();
    let stop = if reason.ends_with(['.', '!', '?']) {
        ""
    } else {
        "."
    };
    Err(match left_changed(&before, &after, ctx.projects) {
        Some(left) => format!("{label}: {reason}{stop} It couldn't all be put back: {left}"),
        None if before == after => format!("Nothing was changed: {label}: {reason}{stop}"),
        None => format!(
            "{label}: {reason}{stop} Each agent has it where it had it again, but {}.",
            settings_left(&before, &after).join("; ")
        ),
    })
}

/// The Library plugin id of `lib:<name>`.
fn library_id(ctx: &Ctx<'_>, key: &str) -> Result<String, String> {
    let name = key
        .strip_prefix("lib:")
        .ok_or_else(|| format!("Brigadier doesn't know \"{key}\"."))?;
    if ctx.library.entry(name).is_none() {
        return Err(format!("{name} isn't in your Library any more."));
    }
    Ok(plugin_id(name))
}

fn every_placed(ctx: &Ctx<'_>, id: &str) -> Vec<(Agent, Placed)> {
    adapters()
        .into_iter()
        .map(|adapter| (adapter.agent(), placed(ctx, adapter, id)))
        .collect()
}

/// Puts Library item `key` on in every project (`places` `None`) or in exactly the user's
/// projects `places`, for every agent, carrying out each agent's steps with `run`.
#[cfg(test)]
pub(super) fn set_places_with(
    ctx: &Ctx<'_>,
    key: &str,
    places: Option<&[PathBuf]>,
    run: Runner<'_>,
) -> Result<(), String> {
    let id = library_id(ctx, key)?;
    if places.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".to_owned());
    }
    let want = Placed::at(places, ctx.projects);
    let wants: Vec<(Agent, Placed)> = adapters()
        .into_iter()
        .map(|adapter| (adapter.agent(), want.clone()))
        .collect();
    change(ctx, &id, &wants, run)
}

/// Gives Library item `key` to the agents that lack it, where the others have it (installed
/// and off when no agent has it on anywhere).
pub(super) fn add_missing_with(ctx: &Ctx<'_>, key: &str, run: Runner<'_>) -> Result<(), String> {
    let id = library_id(ctx, key)?;
    let now = every_placed(ctx, &id);
    let others: Vec<&Placed> = now
        .iter()
        .map(|(_, placed)| placed)
        .filter(|placed| placed.has)
        .collect();
    let global = others.iter().any(|placed| placed.global);
    let on_in: Vec<PathBuf> = ctx
        .projects
        .iter()
        .filter(|path| {
            others
                .iter()
                .any(|placed| (!global || placed.global) && placed.on_in.contains(path))
        })
        .cloned()
        .collect();
    let want = Placed {
        has: true,
        global,
        on_in,
    };
    let wants: Vec<(Agent, Placed)> = now
        .iter()
        .filter(|(_, placed)| !placed.has)
        .map(|(agent, _)| (*agent, want.clone()))
        .collect();
    if wants.is_empty() {
        return Ok(());
    }
    change(ctx, &id, &wants, run)
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
pub(super) fn server_names(ctx: &Ctx<'_>, except: Option<&str>) -> Vec<String> {
    let mut names = Vec::new();
    for project in std::iter::once(None).chain(ctx.projects.iter().map(|p| Some(p.as_path()))) {
        let catalog = ctx.read(project);
        for server in catalog.servers.into_iter().chain(catalog.off.servers) {
            if matches!(
                server.source,
                brigadier_registry::Source::Desktop
                    | brigadier_registry::Source::CursorUser
                    | brigadier_registry::Source::CursorProject
            ) {
                continue;
            }
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

/// Adds what `text` holds to the Library and gives it to every agent: in every project
/// (`scope` `None`) or in exactly the user's projects `scope`. `shared` (the CLI's `--shared`,
/// one project only) installs it in the form the project commits, for the whole team. Its
/// keys, and what the user should know.
pub(crate) fn add(
    ctx: &Ctx<'_>,
    text: &str,
    name: Option<&str>,
    scope: Option<&[PathBuf]>,
    shared: bool,
    options: &AddOptions,
    run: Runner<'_>,
) -> Result<(Vec<String>, Option<String>), String> {
    if scope.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".to_owned());
    }
    let mut items = add_parse::parse(text, name, &ctx.roots.home).map_err(|err| err.to_string())?;
    give_values(&mut items, &options.values)?;
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
        let given = match (shared, scope) {
            (true, Some([project])) => {
                let place = Place::Project {
                    path: project.clone(),
                    shared: true,
                };
                adapters().into_iter().all(|adapter| {
                    ctx.library
                        .sync(adapter, &entry, &place, false)
                        .map_err(|err| err.0)
                        .and_then(|steps| run(adapter, &steps))
                        .is_ok()
                })
            }
            (true, _) => {
                return Err(
                    "Shared with the team goes to one project: pick just that one.".to_owned(),
                );
            }
            (false, _) => {
                let want = Placed::at(scope, ctx.projects);
                let wants: Vec<(Agent, Placed)> = adapters()
                    .into_iter()
                    .map(|adapter| (adapter.agent(), want.clone()))
                    .collect();
                change(ctx, &entry.id(), &wants, run).is_ok()
            }
        };
        if !given {
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

/// What the old Use everywhere moved: the user's originals, which Remove puts back.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Moved {
    pub originals: Vec<Original>,
    /// Which of them a Remove has put back already, by index.
    #[serde(default)]
    pub restored: Vec<usize>,
}

/// Keeps what Remove did of `moved` (Library item `name`'s), for a Remove tried again.
pub(super) fn write_moved(library: &Library, name: &str, moved: &Moved) -> Result<(), String> {
    let file = moved_file(library, name);
    let text = serde_json::to_string_pretty(moved).map_err(|err| err.to_string())?;
    let temp = file.with_extension("json.new");
    (|| {
        use std::io::Write;
        let mut staged = std::fs::File::create(&temp)?;
        staged.write_all(text.as_bytes())?;
        staged.sync_all()
    })()
    .and_then(|()| std::fs::rename(&temp, &file))
    .and_then(|()| std::fs::File::open(file.parent().unwrap())?.sync_all())
    .map_err(|err| format!("Couldn't keep track of {name}'s originals: {err}"))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(super) enum Original {
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
pub(super) enum SavedTransport {
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
    /// Already where it was: a skill's folder back, its kept copy gone.
    pub fn back(&self) -> bool {
        match self {
            Self::Skill { from, kept } => {
                from.symlink_metadata().is_ok()
                    && kept
                        .symlink_metadata()
                        .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            }
            Self::Server { .. } => false,
        }
    }

    /// Where a server's values are kept until it is back.
    pub fn secrets(&self) -> Option<&str> {
        match self {
            Self::Server { secrets, .. } => Some(secrets),
            Self::Skill { .. } => None,
        }
    }

    /// Puts it back as it was (a server's values stay kept: [`Self::secrets`]).
    pub fn restore(&self, ctx: &Ctx<'_>) -> Result<(), String> {
        match self {
            Self::Skill { from, kept } => {
                if self.back() {
                    return Ok(());
                }
                if from.symlink_metadata().is_ok() {
                    return Err(format!(
                        "Couldn't put {} back: something is already there.",
                        from.display()
                    ));
                }
                if let Some(dir) = from.parent() {
                    std::fs::create_dir_all(dir).map_err(|err| err.to_string())?;
                }
                std::fs::rename(kept, from)
                    .and_then(|()| std::fs::File::open(from.parent().unwrap())?.sync_all())
                    .and_then(|()| std::fs::File::open(kept.parent().unwrap())?.sync_all())
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
                let mut take = |names: &[String]| -> Result<Vec<(String, Secret)>, String> {
                    names.iter().map(|name| {
                        let value = values.remove(name).ok_or_else(|| format!("Couldn't put the server back: its saved value {name} is missing."))?;
                        Ok((name.clone(), Secret::new(value)))
                    }).collect()
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
                        env: take(env)?,
                        cwd: cwd.clone(),
                    },
                    SavedTransport::Http { url, headers } => TransportSpec::Http {
                        url: url.clone(),
                        headers: take(headers)?,
                    },
                };
                let spec = ServerSpec {
                    name: name.clone(),
                    transport,
                    oauth: None,
                };
                let place = project.as_deref().map_or(Place::Everywhere, just_me);
                let agent = adapter(
                    agent_of(*agent).ok_or("Brigadier no longer drives the agent it came from.")?,
                );
                // A crash after the CLI restored it but before the checkpoint must be safe
                // to retry. An independently changed entry is never overwritten.
                let catalog = ctx.read(project.as_deref());
                if let Some(existing) =
                    catalog
                        .servers
                        .iter()
                        .chain(&catalog.off.servers)
                        .find(|server| {
                            server.name == *name
                                && server.project == *project
                                && server.plugin.is_none()
                                && agent.owns(server.source)
                        })
                {
                    let same_values = |a: &[(String, Secret)], b: &[(String, Secret)]| {
                        a.len() == b.len() && a.iter().all(|value| b.contains(value))
                    };
                    let same = match (&spec.transport, &existing.transport) {
                        (
                            TransportSpec::Stdio {
                                command,
                                args,
                                env,
                                cwd,
                            },
                            brigadier_registry::Transport::Stdio {
                                command: other,
                                args: other_args,
                                env: other_env,
                                cwd: other_cwd,
                                ..
                            },
                        ) => {
                            command == existing.written.command.as_ref().unwrap_or(other)
                                && args == other_args
                                && cwd == other_cwd
                                && same_values(env, other_env)
                        }
                        (
                            TransportSpec::Http { url, headers },
                            brigadier_registry::Transport::Http {
                                url: other,
                                headers: other_headers,
                            },
                        ) => url == other && same_values(headers, other_headers),
                        _ => false,
                    };
                    return if same && existing.oauth.is_none() {
                        Ok(())
                    } else {
                        Err(format!(
                            "Couldn't put {name} back: a different server is already there."
                        ))
                    };
                }
                let steps = agent.add_server(name, &spec, &place).map_err(|err| err.0)?;
                ctx.run(agent, &steps)?;
                Ok(())
            }
        }
    }
}

pub(super) fn moved_dir(library: &Library) -> PathBuf {
    library.root().join(".moved")
}

pub(super) fn moved_file(library: &Library, name: &str) -> PathBuf {
    moved_dir(library).join(format!("{name}.json"))
}

pub(super) fn read_moved(library: &Library, name: &str) -> Result<Option<Moved>, String> {
    let file = moved_file(library, name);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(format!(
                "Couldn't read the saved originals of {name}: {err}"
            ));
        }
    };
    serde_json::from_str(&text).map(Some).map_err(|_| {
        format!("Couldn't read the saved originals of {name}; Brigadier kept its copy.")
    })
}

/// The steps that take Library plugin `id` away from `adapter`'s agent: off in each project
/// that turns it on by itself (such a project keeps loading it otherwise), then uninstalled.
pub(super) fn removal(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Vec<Step> {
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

/// Takes Library item `name` from every agent and out of the Library, with its values.
pub(super) fn remove_library_with(
    ctx: &Ctx<'_>,
    name: &str,
    run: Runner<'_>,
) -> Result<(), String> {
    let id = plugin_id(name);
    let mut failed = Vec::new();
    for adapter in adapters() {
        let steps = removal(ctx, adapter, &id);
        if steps.is_empty() {
            continue;
        }
        (ctx.report)(adapter.agent(), PluginStep::Working);
        match run(adapter, &steps) {
            Ok(()) => (ctx.report)(adapter.agent(), PluginStep::Done),
            Err(reason) => {
                (ctx.report)(adapter.agent(), PluginStep::Failed { reason });
                failed.push(adapter.label());
            }
        }
    }
    if !failed.is_empty() {
        return Err(format!(
            "{} didn't let go of {name}; it stays in your Library.",
            failed.join(" and ")
        ));
    }
    let servers = server_names_of(ctx, name);
    for server in servers {
        ctx.secrets.forget(&format!("{name}/{server}"))?;
    }
    ctx.library.remove(name).map_err(|err| err.to_string())?;
    ctx.library
        .set_origin(name, None)
        .map_err(|err| err.to_string())?;
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

/// Fetches Library item `name`'s source again and brings every agent's copy up to date.
pub(crate) fn update(ctx: &Ctx<'_>, name: &str) -> Result<(), String> {
    let origin = ctx
        .library
        .origin(name)
        .ok_or_else(|| format!("{name} has nothing to update from."))?;
    let scratch = Scratch::new()?;
    let item = Item::Skill(SkillSpec {
        name: Some(name.to_owned()),
        source: origin.source.clone(),
    });
    let (dir, fetched) = fetch(&item, &scratch)?;
    let packed = ctx
        .library
        .pack(
            name,
            &item,
            dir.as_deref(),
            &server_names(ctx, Some(name)),
            true,
            ctx.secrets,
        )
        .map_err(|err| err.to_string())?;
    if let Some(fetched) = &fetched {
        ctx.library
            .set_origin(name, Some(fetched))
            .map_err(|err| err.to_string())?;
    }
    if !packed.changed {
        return Ok(());
    }
    let id = packed.entry.id();
    let mut failed = Vec::new();
    for adapter in adapters() {
        if let Err(err) = refresh_copy(ctx, adapter, &id, &|adapter, steps| ctx.run(adapter, steps))
        {
            failed.push(format!("{}: {err}", adapter.label()));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Not every agent took the new version of {name}: {}",
            failed.join("; ")
        ))
    }
}

/// Brings `adapter`'s agent's installs of Library plugin `id` to the Library's version: each
/// with the CLI's own update. Nothing when it has none.
pub(super) fn refresh_copy(
    ctx: &Ctx<'_>,
    adapter: &dyn AgentAdapter,
    id: &str,
    run: Runner<'_>,
) -> Result<(), String> {
    let installed = adapter.installed(ctx.roots, id);
    let projects = adapter.installed_in_projects(ctx.roots, id);
    if !installed && projects.is_empty() {
        return Ok(());
    }
    let mut steps = ctx.library.market_steps(adapter);
    if installed {
        steps.extend(
            adapter
                .update_plugin(id, &Place::Everywhere)
                .map_err(|err| err.0)?,
        );
    }
    for place in projects {
        if matches!(&place, Place::Project { path, .. } if path.is_dir()) {
            steps.extend(adapter.update_plugin(id, &place).map_err(|err| err.0)?);
        }
    }
    run(adapter, &steps)
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
