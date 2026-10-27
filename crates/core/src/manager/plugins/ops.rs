//! Plain data-dir library operations and catalog choices for the Settings page and CLI.
//! Legacy placement/migration helpers remain here until the cleanup wave retires callers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;
use brigadier_registry::adapter::{
    AgentAdapter, Place, ProjectScope, Step, adapter, adapters, apply,
};
use brigadier_registry::add_parse;
use brigadier_registry::item::{Item, ServerSpec, SkillSource, SkillSpec, TransportSpec};
use brigadier_registry::library::{self, Library, Origin, SecretSink, plugin_id};
use brigadier_registry::{Agent, Catalog, Plugin, Roots, Secret};
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
    /// The names of its configured values; values never enter the page model.
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
    pub claude_configs: &'a [PathBuf],
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

/// Whether `agent` loads plugin `id` among a catalog's `plugins`: anywhere (`project` `None`:
/// one on for every project), or there.
fn plugin_on(plugins: &[Plugin], agent: Agent, id: &str, only_here: bool) -> bool {
    plugins.iter().any(|plugin| {
        plugin.agent == agent && plugin.id == id && (!only_here || plugin.project.is_some())
    })
}

/// The plugins on for every project and in each of the user's projects ([`Catalog::plugins`]:
/// their settings files only), read once: what a change's moves and its read-back look at
/// until its next command runs.
pub(super) struct Plugins {
    user: Vec<Plugin>,
    /// In the order of [`Ctx::projects`].
    projects: Vec<Vec<Plugin>>,
}

impl Plugins {
    pub(super) fn read(ctx: &Ctx<'_>) -> Self {
        Self {
            user: Catalog::plugins(ctx.roots, None),
            projects: ctx
                .projects
                .iter()
                .map(|path| Catalog::plugins(ctx.roots, Some(path)))
                .collect(),
        }
    }

    /// Whether `agent` loads plugin `id` in `project`, or (`None`) for every project.
    fn on(&self, ctx: &Ctx<'_>, agent: Agent, id: &str, project: Option<&Path>) -> bool {
        match project {
            None => plugin_on(&self.user, agent, id, false),
            Some(path) => match ctx.projects.iter().position(|known| known == path) {
                Some(at) => plugin_on(&self.projects[at], agent, id, false),
                None => plugin_on(&Catalog::plugins(ctx.roots, Some(path)), agent, id, false),
            },
        }
    }
}

/// Whether `project` keeps plugin `id` on for `adapter`'s agent by itself, so it stays on there
/// once it is off for every project: it counts only there, or the project's own settings switch
/// it on over every project's.
fn kept_on_in(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str, project: &Path) -> bool {
    let plugins = Catalog::plugins(ctx.roots, Some(project));
    let agent = adapter.agent();
    plugin_on(&plugins, agent, id, true)
        || (plugin_on(&plugins, agent, id, false) && adapter.switched_on_in(project, id))
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Where `adapter`'s agent has Library plugin `id` now.
pub(super) fn placed(ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Placed {
    placed_in(&Plugins::read(ctx), ctx, adapter, id)
}

/// Where `adapter`'s agent has Library plugin `id`, as `now` read it.
fn placed_in(now: &Plugins, ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Placed {
    let agent = adapter.agent();
    let global = now.on(ctx, agent, id, None);
    let on_in: Vec<PathBuf> = ctx
        .projects
        .iter()
        .filter(|path| now.on(ctx, agent, id, Some(path)))
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
    held_in(&Plugins::read(ctx), ctx, adapter, id)
}

/// [`held`], as `now` read it.
fn held_in(now: &Plugins, ctx: &Ctx<'_>, adapter: &dyn AgentAdapter, id: &str) -> Held {
    Held {
        placed: placed_in(now, ctx, adapter, id),
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

/// The steps of `step`, planned against the plugins as `now` read them.
fn move_steps(
    ctx: &Ctx<'_>,
    now: &Plugins,
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
        Move::Keep(path) if installed || installed_in(path) => {
            // On there already through every project's switch, an agent whose own command
            // sets the project's (Claude Code) takes it as done (`alreadyInGoalState`) and
            // writes nothing, so it would go off with every project's: switched off there
            // first, the project's own switch is written. A project file is written as asked.
            if adapter.capabilities().project_scope == ProjectScope::Native
                && now.on(ctx, agent, id, Some(path))
            {
                steps.extend(
                    adapter
                        .set_plugin(id, &just_me(path), false)
                        .map_err(unsupported)?,
                );
            }
            steps.extend(
                adapter
                    .set_plugin(id, &just_me(path), true)
                    .map_err(unsupported)?,
            );
        }
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
            if now.on(ctx, agent, id, Some(path)) != *on {
                steps.extend(
                    adapter
                        .set_plugin(id, &just_me(path), *on)
                        .map_err(unsupported)?,
                );
            }
        }
        Move::Global(on) => {
            let now = now.on(ctx, agent, id, None);
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
///
/// The plugins are read once a round, after its moves: one agent's moves leave every other
/// agent's settings as they are, and one move in a round leaves the others' projects alone.
fn drive(
    ctx: &Ctx<'_>,
    id: &str,
    wants: &[(Agent, Placed)],
    run: Runner<'_>,
    seen: &mut Vec<(Agent, Held)>,
    mut now: Plugins,
) -> Result<(), (Agent, String)> {
    for round in 0..3 {
        let mut moved = Vec::new();
        for (agent, want) in wants {
            let adapter = adapter(*agent);
            let moves = &moves_to(want, ctx.projects)[round];
            let failed = moves.iter().find_map(|step| {
                move_steps(ctx, &now, adapter, id, step)
                    .and_then(|steps| run(adapter, &steps))
                    .err()
            });
            if !moves.is_empty() {
                moved.push(*agent);
            }
            if let Some(reason) = failed {
                // How the agents moved before it read now: the put-back knows them as its own.
                remember(ctx, id, seen, &moved, &Plugins::read(ctx));
                return Err((*agent, reason));
            }
        }
        if !moved.is_empty() {
            now = Plugins::read(ctx);
            remember(ctx, id, seen, &moved, &now);
        }
    }
    for (agent, want) in wants {
        let is = placed_in(&now, ctx, adapter(*agent), id);
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

/// Keeps how each of `moved` reads in `now`, as its settings after its last move.
fn remember(
    ctx: &Ctx<'_>,
    id: &str,
    seen: &mut Vec<(Agent, Held)>,
    moved: &[Agent],
    now: &Plugins,
) {
    for agent in moved {
        seen.retain(|(known, _)| known != agent);
        seen.push((*agent, held_in(now, ctx, adapter(*agent), id)));
    }
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
                let now = Plugins::read(ctx);
                for step in moves_to(was, ctx.projects)[round]
                    .iter()
                    .filter(|step| step.enables() == enabling)
                {
                    all &= move_steps(ctx, &now, adapter, id, step)
                        .and_then(|steps| run(adapter, &steps))
                        .is_ok();
                }
            }
        }
    }
    all
}

/// Checks every prospective move before a native off flag is written.
pub(super) fn preflight(ctx: &Ctx<'_>, id: &str, wants: &[(Agent, Placed)]) -> Result<(), String> {
    let now = Plugins::read(ctx);
    for (agent, want) in wants {
        let adapter = adapter(*agent);
        for step in moves_to(want, ctx.projects).iter().flatten() {
            let steps = move_steps(ctx, &now, adapter, id, step)?;
            brigadier_registry::adapter::check_steps(
                adapter,
                ctx.roots,
                &ctx.read(None),
                &steps,
                ctx.env,
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct ChangeError {
    pub agent: Agent,
    pub reason: String,
    pub restored: bool,
    pub left: Option<String>,
    pub rollback: Vec<(Agent, Placed)>,
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.restored {
            write!(f, "Nothing was changed: ")?;
        }
        let stop = if self.reason.ends_with(['.', '!', '?']) {
            ""
        } else {
            "."
        };
        write!(f, "{}: {}{stop}", adapter(self.agent).label(), self.reason)?;
        if let Some(left) = &self.left {
            write!(f, " {left}")?;
        }
        Ok(())
    }
}
impl From<ChangeError> for String {
    fn from(error: ChangeError) -> Self {
        error.to_string()
    }
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
) -> Result<(), ChangeError> {
    let read = |now: &Plugins| -> Vec<(Agent, Held)> {
        adapters()
            .into_iter()
            .map(|adapter| (adapter.agent(), held_in(now, ctx, adapter, id)))
            .collect()
    };
    let now = Plugins::read(ctx);
    let before = read(&now);
    for (agent, _) in wants {
        (ctx.report)(*agent, PluginStep::Working);
    }
    let mut seen = Vec::new();
    let Err((agent, reason)) = drive(ctx, id, wants, run, &mut seen, now) else {
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
    let after = read(&Plugins::read(ctx));
    let restored = before == after;
    let left = left_changed(&before, &after, ctx.projects)
        .map(|left| format!("It couldn't all be put back: {left}"))
        .or_else(|| {
            (!restored).then(|| {
                format!(
                    "Each agent has it where it had it again, but {}.",
                    settings_left(&before, &after).join("; ")
                )
            })
        });
    Err(ChangeError {
        agent,
        reason,
        restored,
        left,
        rollback: back,
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
    let now = Plugins::read(ctx);
    adapters()
        .into_iter()
        .map(|adapter| (adapter.agent(), placed_in(&now, ctx, adapter, id)))
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
    change(ctx, &id, &wants, run).map_err(Into::into)
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
    change(ctx, &id, &wants, run).map_err(Into::into)
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

#[derive(Default)]
pub(crate) struct AddOptions {
    /// Set or added to the one pasted server's private library entry.
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

/// Save plain library content. No adapter or CLI command is run.
pub(crate) fn add(
    ctx: &Ctx<'_>,
    text: &str,
    name: Option<&str>,
    scope: Option<&[PathBuf]>,
    shared: bool,
    options: &AddOptions,
    _run: Runner<'_>,
) -> Result<(Vec<String>, Option<String>), String> {
    if shared {
        return Err(
            "Library items are used by Brigadier sessions; --shared is no longer supported.".into(),
        );
    }
    let mut items = add_parse::parse(text, name, &ctx.roots.home).map_err(|err| err.to_string())?;
    give_values(&mut items, &options.values)?;
    add_plain(ctx.library.root(), &items, scope).map(|keys| (keys, None))
}

fn valid_library_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("Use letters, digits, - or _ for a library name.".into());
    }
    Ok(())
}

fn write_private(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::io::Write;
    std::fs::create_dir_all(path.parent().ok_or("Missing library folder")?)
        .map_err(|e| e.to_string())?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| e.to_string())?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

fn server_value(spec: &ServerSpec) -> serde_json::Value {
    use serde_json::json;
    let values = |pairs: &[(String, Secret)]| {
        pairs
            .iter()
            .map(|(name, value)| (name.clone(), json!(value.expose())))
            .collect::<serde_json::Map<_, _>>()
    };
    let mut value = match &spec.transport {
        TransportSpec::Stdio {
            command,
            args,
            env,
            cwd,
        } => json!({"command":command,"args":args,"env":values(env),"cwd":cwd}),
        TransportSpec::Http { url, headers } => {
            json!({"type":"http","url":url,"headers":values(headers)})
        }
    };
    if let Some(oauth) = &spec.oauth {
        value["oauth"] = json!({"clientId":oauth.client_id});
    }
    json!({"mcpServers": { &spec.name: value }})
}

fn copy_skill(from: &Path, to: &Path, ancestors: &mut Vec<PathBuf>) -> Result<(), String> {
    let canonical = std::fs::canonicalize(from).map_err(|e| e.to_string())?;
    if ancestors.contains(&canonical) {
        return Err("The skill contains a cyclic link.".into());
    }
    if from.is_dir() {
        ancestors.push(canonical);
        std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
        for entry in std::fs::read_dir(from).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_name() == ".git" {
                continue;
            }
            copy_skill(&entry.path(), &to.join(entry.file_name()), ancestors)?;
        }
        ancestors.pop();
    } else {
        std::fs::copy(from, to).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn add_plain(
    root: &Path,
    items: &[Item],
    scope: Option<&[PathBuf]>,
) -> Result<Vec<String>, String> {
    if scope.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".into());
    }
    let mut keys = Vec::new();
    for item in items {
        let scratch = Scratch::new()?;
        let (skill_dir, origin) = fetch(item, &scratch)?;
        let name = match (item, skill_dir.as_deref()) {
            (Item::Server(spec), _) => spec.name.clone(),
            (Item::Skill(spec), Some(dir)) => skill_name(spec, dir),
            _ => return Err("The skill's files weren't fetched.".into()),
        };
        valid_library_name(&name)?;
        let target = match item {
            Item::Server(_) => root.join("servers").join(format!("{name}.json")),
            Item::Skill(_) => root.join("skills").join(&name),
        };
        if target.exists() {
            return Err(format!("Your Library already has something called {name}."));
        }
        for path in [root.to_owned(), target.parent().unwrap().to_owned()] {
            if path
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err("The library folder cannot be a symbolic link.".into());
            }
        }
        std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
        match item {
            Item::Server(spec) => write_private(&target, &server_value(spec))?,
            Item::Skill(_) => {
                let dir = skill_dir.as_deref().ok_or("Missing skill files")?;
                if !dir.join("SKILL.md").is_file() {
                    return Err("The folder has no SKILL.md.".into());
                }
                let staging = root.join(format!(".add-{}", uuid::Uuid::new_v4()));
                copy_skill(dir, &staging, &mut Vec::new())?;
                std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::rename(staging, &target).map_err(|e| e.to_string())?;
            }
        }
        if let Some(origin) = origin {
            write_private(&root.join("origins").join(format!("{name}.json")), &origin)?;
        }
        keys.push(format!("lib:{name}"));
    }
    Ok(keys)
}

fn remove_plain(root: &Path, name: &str) -> Result<(), String> {
    valid_library_name(name)?;
    if root
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err("The library folder cannot be a symbolic link.".into());
    }
    let skill = root.join("skills").join(name);
    let server = root.join("servers").join(format!("{name}.json"));
    let legacy = root.join("plugins").join(name);
    if !skill.exists() && !server.exists() && !legacy.exists() {
        return Err(format!(
            "{name} isn't in your Library; detected items can only be turned off."
        ));
    }
    for path in [&skill, &server, &legacy] {
        if !path.exists() {
            continue;
        }
        let actual = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        let library = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        if !actual.starts_with(&library)
            || path
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Err("The library item points outside the library.".into());
        }
        if path.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .map_err(|e| e.to_string())?;
    }
    let origin = root.join("origins").join(format!("{name}.json"));
    if origin.is_file() {
        let actual = std::fs::canonicalize(&origin).map_err(|e| e.to_string())?;
        if !actual.starts_with(std::fs::canonicalize(root).map_err(|e| e.to_string())?) {
            return Err("The library origin points outside the library.".into());
        }
        std::fs::remove_file(origin).map_err(|e| e.to_string())?;
    }
    Ok(())
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

/// Takes only the plain library item out; never changes any CLI setup.
pub(super) fn remove_library_with(
    ctx: &Ctx<'_>,
    name: &str,
    run: Runner<'_>,
) -> Result<(), String> {
    let _ = run;
    remove_plain(ctx.library.root(), name)
}

/// Refresh legacy library content until the old updater is retired.
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

// The page/CLI path serializes durable choices independently of legacy sync jobs.
static CATALOG_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

impl crate::manager::SessionManager {
    /// The page and launches must read this same provenance-preserving source.
    pub fn plugin_catalog_rows(&self) -> Vec<brigadier_registry::catalog::Row> {
        let projects: Vec<_> = self
            .plugin_projects()
            .into_iter()
            .map(|(_, _, path)| path)
            .collect();
        Catalog::read_rows_with_library(self.registry().roots(), &projects, &self.data_dir)
    }

    pub fn plugin_catalog(&self) -> crate::Result<super::CatalogView> {
        let rows = self.plugin_catalog_rows();
        let choices =
            super::choices::Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        let mut view = super::CatalogView::new(&rows, &choices);
        // Commands are display data only; env/header values never enter the page model.
        for (row, source) in view.rows.iter_mut().zip(&rows) {
            for (definition, source) in row.definitions.iter_mut().zip(&source.definitions) {
                if let brigadier_registry::catalog::Content::Server(server) = &source.content
                    && let Some(command) = &server.written.command
                {
                    definition.endpoint = Some(
                        std::iter::once(command.as_str())
                            .chain(server.written.args.iter().map(String::as_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                    );
                }
            }
        }
        Ok(view)
    }

    pub fn set_plugin_choice(&self, row_id: &str, on: bool) -> crate::Result<()> {
        self.edit_plugin_choice(row_id, Some(on), None)
    }

    pub fn set_plugin_choice_scope(
        &self,
        row_id: &str,
        scope: super::choices::Scope,
    ) -> crate::Result<()> {
        if let super::choices::Scope::Projects(paths) = &scope {
            let projects = self.plugin_projects();
            if paths.is_empty()
                || paths
                    .iter()
                    .any(|path| !projects.iter().any(|(_, _, known)| known == path))
            {
                return Err(crate::Error::Invalid(
                    "Pick registered, trusted projects, or All projects.".into(),
                ));
            }
        }
        self.edit_plugin_choice(row_id, None, Some(scope))
    }

    fn edit_plugin_choice(
        &self,
        row_id: &str,
        on: Option<bool>,
        scope: Option<super::choices::Scope>,
    ) -> crate::Result<()> {
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or_else(|| crate::Error::Invalid(format!("No plugin {row_id}")))?;
        let mut choices =
            super::choices::Choices::load(&self.data_dir).map_err(crate::Error::Invalid)?;
        let mut choice = choices.choice(row);
        if let Some(on) = on {
            choice.on = on;
        }
        if let Some(mut scope) = scope {
            if let super::choices::Scope::Projects(paths) = &mut scope {
                paths.sort();
                paths.dedup();
            }
            choice.scope = scope;
        }
        choices.0.insert(row_id.to_owned(), choice);
        choices.save(&self.data_dir).map_err(crate::Error::Invalid)
    }

    /// Completes before replying: Add/Import/Remove need no frontend job runner.
    pub async fn add_library_item(
        &self,
        text: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
        values: Option<BTreeMap<String, String>>,
    ) -> crate::Result<()> {
        let projects = self.library_scope_paths(project_ids)?;
        let root = self.data_dir.join("library");
        let data_dir = self.data_dir.clone();
        let home = self.registry().roots().home.clone();
        let registry = self.registry();
        super::super::blocking(move || {
            let mut items = add_parse::parse(&text, name.as_deref(), &home)
                .map_err(|e| crate::Error::Invalid(e.to_string()))?;
            give_values(&mut items, &values.unwrap_or_default()).map_err(crate::Error::Invalid)?;
            if root
                .symlink_metadata()
                .is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err(crate::Error::Invalid(
                    "The library folder cannot be a symbolic link.".into(),
                ));
            }
            let staging = root.join(format!(".pending-{}", uuid::Uuid::new_v4()));
            let staged_library = staging.join("library");
            let keys = add_plain(&staged_library, &items, projects.as_deref())
                .map_err(crate::Error::Invalid)?;
            let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
            let rows = Catalog::rows(&[brigadier_registry::catalog::library(&staging)]);
            let mut choices =
                super::choices::Choices::load(&data_dir).map_err(crate::Error::Invalid)?;
            for row in &rows {
                // Validate all destinations before publishing any item.
                let file = &row.definitions[0].provenance.file;
                let relative = file
                    .strip_prefix(&staged_library)
                    .map_err(|e| crate::Error::Invalid(e.to_string()))?;
                let destination = root.join(relative);
                if destination.exists() {
                    return Err(crate::Error::Invalid(format!(
                        "Your Library already has something called {}.",
                        row.name
                    )));
                }
                if keys.contains(&format!("lib:{}", row.name)) {
                    choices.0.insert(
                        row.id.clone(),
                        super::choices::Choice {
                            on: true,
                            scope: projects.clone().map_or(
                                super::choices::Scope::Global(super::choices::Global::Global),
                                super::choices::Scope::Projects,
                            ),
                        },
                    );
                }
            }
            choices.save(&data_dir).map_err(crate::Error::Invalid)?;
            for folder in ["skills", "servers", "origins"] {
                let from = staged_library.join(folder);
                if !from.is_dir() {
                    continue;
                }
                let to = root.join(folder);
                if to
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    return Err(crate::Error::Invalid(
                        "The library folder cannot be a symbolic link.".into(),
                    ));
                }
                std::fs::create_dir_all(&to).map_err(|e| crate::Error::Invalid(e.to_string()))?;
                for entry in
                    std::fs::read_dir(from).map_err(|e| crate::Error::Invalid(e.to_string()))?
                {
                    let entry = entry.map_err(|e| crate::Error::Invalid(e.to_string()))?;
                    let destination = to.join(entry.file_name());
                    if destination.exists() {
                        return Err(crate::Error::Invalid(
                            "The library item already exists.".into(),
                        ));
                    }
                    std::fs::rename(entry.path(), destination)
                        .map_err(|e| crate::Error::Invalid(e.to_string()))?;
                }
            }
            std::fs::remove_dir_all(staging).map_err(|e| crate::Error::Invalid(e.to_string()))?;
            registry.refresh();
            Ok(())
        })
        .await
    }

    fn library_scope_paths(&self, ids: Option<Vec<String>>) -> crate::Result<Option<Vec<PathBuf>>> {
        let Some(ids) = ids else {
            return Ok(None);
        };
        let projects = self.plugin_projects();
        let mut paths = Vec::new();
        for id in ids {
            let found: Vec<_> = projects
                .iter()
                .filter(|(known, _, path)| known == &id || path.to_string_lossy() == id)
                .map(|(_, _, path)| path.clone())
                .collect();
            if found.is_empty() {
                return Err(crate::Error::Invalid(format!("No trusted project {id}")));
            }
            paths.extend(found);
        }
        if paths.is_empty() {
            return Err(crate::Error::Invalid(
                "Pick a project, or All projects.".into(),
            ));
        }
        paths.sort();
        paths.dedup();
        Ok(Some(paths))
    }

    pub async fn import_library_item(
        &self,
        row_id: String,
        name: Option<String>,
        project_ids: Option<Vec<String>>,
    ) -> crate::Result<()> {
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or_else(|| crate::Error::Invalid("No import offer".into()))?;
        let definition = row
            .definitions
            .iter()
            .find(|definition| definition.provenance.agent.is_none())
            .ok_or_else(|| {
                crate::Error::Invalid("This item is already detected in a CLI.".into())
            })?;
        let brigadier_registry::catalog::Content::Server(server) = &definition.content else {
            return Err(crate::Error::Invalid(
                "Only server imports are supported.".into(),
            ));
        };
        let transport = match &server.transport {
            brigadier_registry::Transport::Stdio {
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
            brigadier_registry::Transport::Http { url, headers } => TransportSpec::Http {
                url: url.clone(),
                headers: headers.clone(),
            },
        };
        let spec = ServerSpec {
            name: name.unwrap_or_else(|| server.name.clone()),
            transport,
            oauth: server
                .oauth
                .as_ref()
                .and_then(|oauth| oauth.client_id.as_ref())
                .map(|id| brigadier_registry::item::OAuthSpec {
                    client_id: id.clone(),
                }),
        };
        self.add_library_item(server_value(&spec).to_string(), None, project_ids, None)
            .await
    }

    pub async fn remove_library_item(&self, key: String) -> crate::Result<()> {
        let root = self.data_dir.join("library");
        let name = if let Some(name) = key.strip_prefix("lib:") {
            name.to_owned()
        } else {
            let rows = self.plugin_catalog_rows();
            let row = rows
                .iter()
                .find(|row| row.id == key)
                .ok_or_else(|| crate::Error::Invalid("No library item".into()))?;
            if !row
                .definitions
                .iter()
                .any(|definition| definition.provenance.file.starts_with(&root))
            {
                return Err(crate::Error::Invalid(
                    "Detected items can only be turned off, never deleted.".into(),
                ));
            }
            row.definitions
                .iter()
                .find_map(|definition| {
                    definition
                        .provenance
                        .file
                        .strip_prefix(root.join("plugins"))
                        .ok()
                        .and_then(|path| path.components().next())
                        .map(|part| part.as_os_str().to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| row.name.clone())
        };
        let registry = self.registry();
        super::super::blocking(move || {
            remove_plain(&root, &name).map_err(crate::Error::Invalid)?;
            registry.refresh();
            Ok(())
        })
        .await
    }

    pub fn catalog_item_details(&self, key: String) -> crate::Result<super::PluginItemDetails> {
        use brigadier_registry::catalog::Content;
        let rows = self.plugin_catalog_rows();
        let row = rows
            .iter()
            .find(|row| row.id == key)
            .ok_or_else(|| crate::Error::Invalid("No plugin".into()))?;
        let mut skills = Vec::new();
        let mut server = None;
        for definition in &row.definitions {
            match &definition.content {
                Content::Skill { .. } => {
                    let file = &definition.provenance.file;
                    let path = file.to_string_lossy().into_owned();
                    if skills
                        .iter()
                        .any(|skill: &super::PluginSkillFile| skill.path == path)
                    {
                        continue;
                    }
                    skills.push(super::PluginSkillFile {
                        name: row.name.clone(),
                        path,
                        contents: std::fs::read_to_string(file)
                            .map_err(|e| crate::Error::Invalid(e.to_string()))?,
                        read_only: (!file.starts_with(self.data_dir.join("library/skills"))).then(
                            || "Open this file in your editor to change its instructions.".into(),
                        ),
                    });
                }
                Content::Server(item) if server.is_none() => {
                    server = Some(match &item.transport {
                        brigadier_registry::Transport::Stdio { env, .. } => {
                            super::PluginServerSettings::Stdio {
                                command: item.written.command.clone().unwrap_or_default(),
                                args: item.written.args.clone(),
                                cwd: item.written.cwd.clone(),
                                env_names: env.iter().map(|(name, _)| name.clone()).collect(),
                            }
                        }
                        brigadier_registry::Transport::Http { headers, .. } => {
                            super::PluginServerSettings::Http {
                                url: item.written.url.clone().unwrap_or_default(),
                                header_names: headers
                                    .iter()
                                    .map(|(name, _)| name.clone())
                                    .collect(),
                            }
                        }
                    });
                }
                _ => {}
            }
        }
        Ok(super::PluginItemDetails {
            key,
            kind: if row.kind == brigadier_registry::catalog::Kind::Skill {
                PluginKind::Skill
            } else {
                PluginKind::Server
            },
            skills,
            server,
        })
    }
    pub fn save_catalog_skill(
        &self,
        key: String,
        path: String,
        contents: String,
        expected: String,
    ) -> crate::Result<super::PluginSkillSave> {
        let _guard = CATALOG_WRITES.lock().unwrap_or_else(|p| p.into_inner());
        let row = self.catalog_item_details(key)?;
        let skill = row
            .skills
            .iter()
            .find(|skill| skill.path == path && skill.read_only.is_none())
            .ok_or_else(|| {
                crate::Error::Invalid("Only library skill files can be saved here.".into())
            })?;
        let file = Path::new(&skill.path);
        let canonical =
            std::fs::canonicalize(file).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        let root = std::fs::canonicalize(self.data_dir.join("library/skills"))
            .map_err(|e| crate::Error::Invalid(e.to_string()))?;
        if !canonical.starts_with(root) {
            return Err(crate::Error::Invalid(
                "The skill points outside the library.".into(),
            ));
        }
        if skill.contents != expected {
            return Ok(super::PluginSkillSave::ChangedSinceOpened {
                current: skill.contents.clone(),
            });
        }
        let staging = file.with_file_name(format!(".skill-{}.tmp", uuid::Uuid::new_v4()));
        std::fs::write(&staging, contents).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        std::fs::rename(staging, file).map_err(|e| crate::Error::Invalid(e.to_string()))?;
        self.registry().refresh();
        Ok(super::PluginSkillSave::Saved { job: String::new() })
    }
}
