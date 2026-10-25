//! Every item an agent has, shared with every agent: an agent that lacks it gets Brigadier's
//! Library copy, installed through its own `plugin` commands, while the agents that have it
//! keep their own entry exactly as it is (L5 Q1, L6 Q13).
//!
//! - **The original is the source of truth** (L6 Q14). Each pass compares it with the copy's
//!   fingerprint and re-packs on a change; once it is gone (read cleanly, two passes running)
//!   the copies go too. A file that can't be read is never taken for a removal.
//! - **No agent ever loads two.** An agent with an entry of its own (on or off, or the same
//!   server under another name) gets no copy, and loses one it was given when it gets its own.
//! - **Remove** takes only what Brigadier made and is remembered (L8): the item isn't shared
//!   again until Share with every agent. It is written down before any copy goes.
//! - **Locked** (L21): an original no agent command can switch off or keep to some projects
//!   (Claude's own servers, skill folders) makes the switch and the scope refuse.
//!
//! What sharing remembers is `.sharing.json` in the Library's folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_registry::adapter::{Place, adapter, adapters};
use brigadier_registry::item::{
    Item, OAuthSpec, ServerSpec, SkillSource, SkillSpec, TransportSpec,
};
use brigadier_registry::library::{self, Library, plugin_id};
use brigadier_registry::{Agent, Catalog, Server, Transport};
use serde::{Deserialize, Serialize};

use super::model::{self, KeyKind, Model, Origin, ProjectCatalog, Row};
use super::ops::{self, Ctx, Placed, Runner};
use super::{PluginItemDetails, PluginKind, PluginServerSettings, PluginSkillFile, provider};

const FILE: &str = ".sharing.json";

/// What sharing remembers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct Sharing {
    pub version: u32,
    /// Each Library plugin sharing made, by its name.
    pub items: BTreeMap<String, Record>,
    /// Items the user removed: never shared again until Share with every agent.
    pub declined: Vec<Declined>,
    /// Library plugins whose removal started: finished by the next pass.
    pub pending_removal: Vec<String>,
    /// The projects a switched-off item was on in (`None`: every project), by page key: what
    /// switching it on again restores.
    pub off_scopes: BTreeMap<String, Option<Vec<PathBuf>>>,
    /// The last share that failed, by page key.
    pub failed: BTreeMap<String, Failure>,
    /// Other apps' servers brought in (`other:<id>`): no longer offered for import.
    pub imported: Vec<String>,
}

/// A Library plugin made from an agent's own item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    pub kind: PluginKind,
    /// The item's name.
    pub name: String,
    /// The original's fingerprint when it was packed (values hashed, never kept).
    pub fingerprint: String,
    /// The original's config file, or skill folder.
    pub file: PathBuf,
    /// The packed version.
    pub version: String,
    /// The version each agent took, by agent.
    #[serde(default)]
    pub applied: BTreeMap<String, String>,
    /// Passes running that read the original's file cleanly and found it gone.
    #[serde(default)]
    pub absent_passes: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Declined {
    pub kind: PluginKind,
    pub name: String,
    /// A server's endpoint: the same server renamed stays declined.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// A skill's folder: the same skill renamed in it stays declined.
    #[serde(default)]
    pub source: Option<String>,
}

impl Declined {
    /// What remembers `row` as removed.
    fn of(row: &Row) -> Self {
        Self {
            kind: row.kind,
            name: row.key.name.clone(),
            endpoint: row.endpoint(),
            source: row.skill_dirs().into_iter().next(),
        }
    }

    /// Whether it is `row`: by name, by endpoint, or by the folder it was found in.
    fn is(&self, row: &Row) -> bool {
        self.kind == row.kind
            && (self.name == row.key.name
                || (self.endpoint.is_some() && self.endpoint == row.endpoint())
                || self
                    .source
                    .as_ref()
                    .is_some_and(|source| row.skill_dirs().contains(source)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Failure {
    pub reason: String,
    /// What it was tried with: a pass while running tries again only once that changes.
    pub signature: String,
}

impl Sharing {
    fn file(library: &Library) -> PathBuf {
        library.root().join(FILE)
    }

    /// What `library`'s folder remembers; nothing when it remembers nothing yet. A file that
    /// can't be read is an error: forgetting it would share what the user removed.
    pub fn load(library: &Library) -> Result<Self, String> {
        match std::fs::read_to_string(Self::file(library)) {
            Ok(text) => serde_json::from_str(&text).map_err(|err| {
                format!(
                    "Brigadier couldn't read {}: {err}",
                    Self::file(library).display()
                )
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(format!(
                "Brigadier couldn't read {}: {err}",
                Self::file(library).display()
            )),
        }
    }

    /// Keeps it, whole: written beside, then moved over the old file.
    pub fn save(&self, library: &Library) -> Result<(), String> {
        let file = Self::file(library);
        let text = serde_json::to_string_pretty(&Self {
            version: 1,
            ..self.clone()
        })
        .map_err(|err| err.to_string())?;
        let temp = file.with_file_name(format!(".sharing.json.{}", std::process::id()));
        std::fs::create_dir_all(library.root())
            .and_then(|()| std::fs::write(&temp, text))
            .and_then(|()| std::fs::rename(&temp, &file))
            .map_err(|err| format!("Brigadier couldn't save {}: {err}", file.display()))
    }

    pub fn is_declined(&self, row: &Row) -> bool {
        self.declined.iter().any(|declined| declined.is(row))
    }

    /// Whether a removed original still waits for its confirming pass (review #11).
    pub fn confirming(&self) -> bool {
        self.items.values().any(|record| record.absent_passes == 1)
    }
}

fn agent_key(agent: Agent) -> String {
    match serde_json::to_value(provider(agent)) {
        Ok(serde_json::Value::String(name)) => name,
        _ => format!("{agent:?}"),
    }
}

/// The user's catalog and each of the user's projects', read now.
pub(crate) struct Catalogs {
    pub user: Catalog,
    pub projects: Vec<ProjectCatalog>,
}

impl Catalogs {
    pub fn read(ctx: &Ctx<'_>) -> Self {
        Self {
            user: Catalog::read(ctx.roots, None),
            projects: ctx
                .projects
                .iter()
                .map(|path| ProjectCatalog {
                    id: path.display().to_string(),
                    name: ops::folder_name(path),
                    path: path.clone(),
                    catalog: Catalog::read(ctx.roots, Some(path)),
                })
                .collect(),
        }
    }

    pub fn model(&self, ctx: &Ctx<'_>, sharing: &Sharing) -> Model {
        model::model(
            &self.user,
            &self.projects,
            &ctx.library.entries(),
            sharing,
            &ctx.roots.home,
        )
    }

    /// Whether `file` couldn't be read as a whole now: missing, or not parsed.
    fn unreadable(&self, file: &Path) -> bool {
        !file.exists()
            || std::iter::once(&self.user)
                .chain(self.projects.iter().map(|project| &project.catalog))
                .flat_map(|catalog| &catalog.problems)
                .any(|problem| problem.file == file && problem.id.contains(":file:"))
    }
}

/// Why `row` can't be shared, in words that follow "Only in Claude Code: ".
pub(crate) fn cant_share(row: &Row) -> Option<String> {
    let original = row.original()?;
    if !library::valid_name(&row.key.name) {
        return Some(
            "its name can't be shared: Brigadier copies names of letters, digits, - and _ only"
                .to_owned(),
        );
    }
    match original {
        Origin::Server { server, .. } if server.templated => {
            Some("it reads ${VAR} values from your shell, so Brigadier doesn't copy it".to_owned())
        }
        Origin::Server { server, .. }
            if server
                .oauth
                .as_ref()
                .is_some_and(|oauth| oauth.client_secret.is_some()) =>
        {
            Some(
                "its sign-in uses a client secret, which Brigadier won't copy into a file"
                    .to_owned(),
            )
        }
        _ => None,
    }
}

/// What tells the original changed: its definition, its values hashed.
fn fingerprint(origin: &Origin) -> String {
    match origin {
        Origin::Skill { skill, .. } => library::hash_dir(&skill.dir).unwrap_or_default(),
        Origin::Server { server, .. } => {
            let mut text = format!("{}\n{}\n", server.name, model::endpoint(server));
            let values = match &server.transport {
                Transport::Stdio { env, .. } => env,
                Transport::Http { headers, .. } => headers,
            };
            for (name, value) in values {
                text.push_str(&format!("{name}={}\n", library::hash_text(value.expose())));
            }
            if let Some(oauth) = &server.oauth {
                text.push_str(&format!(
                    "oauth {} {}\n",
                    oauth.client_id.as_deref().unwrap_or_default(),
                    oauth
                        .client_secret
                        .as_ref()
                        .map(|secret| library::hash_text(secret.expose()))
                        .unwrap_or_default()
                ));
            }
            library::hash_text(&text)
        }
    }
}

/// The item `origin` is, as the Library packs it, and the folder of a skill's files.
fn item_of(origin: &Origin, name: &str) -> (Item, Option<PathBuf>) {
    match origin {
        Origin::Skill { skill, .. } => (
            Item::Skill(SkillSpec {
                name: Some(name.to_owned()),
                source: SkillSource::Folder(skill.dir.clone()),
            }),
            Some(skill.dir.clone()),
        ),
        Origin::Server { server, .. } => (Item::Server(spec_of(server)), None),
    }
}

/// `server` as a server to add: as written (a server that reads variables isn't shared), its
/// working folder the one it runs in (its config's, else its source's), its OAuth client's id.
fn spec_of(server: &Server) -> ServerSpec {
    let transport = match &server.transport {
        Transport::Stdio {
            command, args, env, ..
        } => TransportSpec::Stdio {
            command: command.clone(),
            args: args.clone(),
            env: env.clone(),
            cwd: server.transport.working_dir().map(Path::to_owned),
        },
        Transport::Http { url, headers } => TransportSpec::Http {
            url: url.clone(),
            headers: headers.clone(),
        },
    };
    ServerSpec {
        name: server.name.clone(),
        transport,
        oauth: server
            .oauth
            .as_ref()
            .and_then(|oauth| oauth.client_id.clone())
            .map(|client_id| OAuthSpec { client_id }),
    }
}

/// Packs `origin` as Library plugin `lib`.
fn pack(
    ctx: &Ctx<'_>,
    lib: &str,
    origin: &Origin,
    name: &str,
    replace: bool,
) -> Result<library::Packed, String> {
    let (item, dir) = item_of(origin, name);
    let mut taken = ops::server_names(ctx, Some(lib));
    taken.retain(|known| known != name);
    // Its values wait aside until the pack holds: one that fails keeps the installed copy's
    // values as they were (review #3).
    let staged = Staged::default();
    let packed = ctx
        .library
        .pack(lib, &item, dir.as_deref(), &taken, replace, &staged)
        .map_err(|err| err.to_string())?;
    staged.commit(
        ctx.secrets,
        if replace {
            packed.server.as_deref().map(|name| format!("{lib}/{name}"))
        } else {
            None
        },
    )?;
    Ok(packed)
}

/// Values a pack gives, kept aside until it holds.
#[derive(Default)]
struct Staged(std::sync::Mutex<Vec<(String, String, brigadier_registry::Secret)>>);

impl Staged {
    /// Replace only after packing succeeds. A failed Keychain write restores the previous
    /// values, including those removed by the new definition.
    fn commit(self, secrets: &dyn ops::Secrets, replacing: Option<String>) -> Result<(), String> {
        let previous = replacing
            .as_deref()
            .map(|server| secrets.load(server))
            .transpose()?;
        if let Some(server) = &replacing {
            secrets.forget(server)?;
        }
        for (server, name, value) in self.0.into_inner().unwrap_or_else(|p| p.into_inner()) {
            if let Err(error) = secrets.store(&server, &name, &value) {
                if let (Some(server), Some(previous)) = (&replacing, &previous) {
                    secrets.forget(server)?;
                    for (name, value) in previous {
                        secrets.store(
                            server,
                            name,
                            &brigadier_registry::Secret::new(value.clone()),
                        )?;
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

impl library::SecretSink for Staged {
    fn store(
        &self,
        server: &str,
        name: &str,
        value: &brigadier_registry::Secret,
    ) -> Result<(), String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push((
            server.to_owned(),
            name.to_owned(),
            value.clone(),
        ));
        Ok(())
    }
}

/// A free Library name for `row`, whose Claude form (`plugin_<lib>_<server>`) no server of
/// Claude's own takes either.
fn library_name(ctx: &Ctx<'_>, row: &Row, catalogs: &Catalogs) -> String {
    let claude_names: Vec<String> = std::iter::once(&catalogs.user)
        .chain(catalogs.projects.iter().map(|project| &project.catalog))
        .flat_map(|catalog| catalog.servers.iter().chain(&catalog.off.servers))
        .map(Server::native_name)
        .collect();
    let server = row.kind == PluginKind::Server;
    let clash =
        |lib: &str| server && claude_names.contains(&format!("plugin_{lib}_{}", row.key.name));
    let mut lib = ctx.library.fresh_name(&row.key.name);
    let mut n = 2;
    while clash(&lib) {
        lib = ctx.library.fresh_name(&format!("{}-{n}", row.key.name));
        n += 1;
    }
    lib
}

/// Where `row`'s own entries cover each agent: every project, or these projects.
fn own_places(row: &Row, agent: Agent) -> (bool, Vec<PathBuf>) {
    let mut global = false;
    let mut projects = Vec::new();
    for source in row
        .own
        .sources
        .iter()
        .filter(|source| source.agent() == agent)
    {
        match source {
            Origin::Server { server, .. } => match &server.project {
                None => global = true,
                Some(path) => projects.push(path.clone()),
            },
            Origin::Skill { skill, .. } => {
                for root in &skill.roots {
                    match root {
                        brigadier_registry::SkillRoot::ClaudeProject(path)
                        | brigadier_registry::SkillRoot::AgentsProject(path) => {
                            projects.push(path.clone());
                        }
                        brigadier_registry::SkillRoot::Plugin(plugin) => match &plugin.project {
                            Some(path) => projects.push(path.clone()),
                            None => global = true,
                        },
                        _ => global = true,
                    }
                }
            }
        }
    }
    (global, projects)
}

/// Where each agent should have `row`'s copy: where the item is on (its own entries' places,
/// every agent's together), less where that agent has its own; none for an agent its own
/// entries cover everywhere, or that has the same server under another name (review #6).
fn wanted(row: &Row, projects: &[PathBuf]) -> Vec<(Agent, Placed)> {
    let global = !row.own.everywhere.is_empty()
        || row
            .equivalent
            .iter()
            .any(|(_, scope, on)| scope.is_none() && *on);
    let on_in: Vec<PathBuf> = projects
        .iter()
        .filter(|path| {
            global
                || row.own.here.iter().any(|(at, _)| at == *path)
                || row
                    .equivalent
                    .iter()
                    .any(|(_, scope, on)| scope.as_ref() == Some(*path) && *on)
        })
        .cloned()
        .collect();
    adapters()
        .into_iter()
        .map(|adapter| {
            let agent = adapter.agent();
            // Its own entries, and the same server under another name, each where it is.
            let (mut own_global, mut own_projects) = own_places(row, agent);
            for (twin, project, _) in &row.equivalent {
                if *twin == agent {
                    match project {
                        None => own_global = true,
                        Some(path) => own_projects.push(path.clone()),
                    }
                }
            }
            if own_global {
                return (agent, Placed::none());
            }
            let rest: Vec<PathBuf> = on_in
                .iter()
                .filter(|path| !own_projects.contains(path))
                .cloned()
                .collect();
            let placed = if own_projects.is_empty() || global || !rest.is_empty() {
                Placed {
                    has: true,
                    global,
                    on_in: rest,
                }
            } else {
                Placed::none()
            };
            (agent, placed)
        })
        .collect()
}

/// Where `agent` has Library plugin `id` now, as `row` read it (cheap: no catalog read).
fn copy_now(ctx: &Ctx<'_>, row: &Row, agent: Agent, id: &str) -> Placed {
    let adapter = adapter(agent);
    let global = row.copy.everywhere.contains(&agent);
    let on_in: Vec<PathBuf> = ctx
        .projects
        .iter()
        .filter(|path| {
            row.copy
                .here
                .iter()
                .any(|(at, a)| at == *path && *a == agent)
        })
        .cloned()
        .collect();
    let has = row.copy.anywhere.contains(&agent)
        || adapter.installed(ctx.roots, id)
        || !adapter.installed_in_projects(ctx.roots, id).is_empty();
    Placed { has, global, on_in }
}

/// One sharing pass: every item (or only page key `only`) brought to every agent that lacks
/// it, copies following their originals. `retry`: try again what failed with the same inputs
/// (at the app's start, or asked for), else leave it until something changes. Each item's
/// failure is kept; with `only`, its failure is the error.
pub(crate) fn sync(
    ctx: &Ctx<'_>,
    only: Option<&str>,
    retry: bool,
    run: Runner<'_>,
) -> Result<(), String> {
    let mut sharing = Sharing::load(ctx.library)?;
    // A removal that started finishes first.
    for lib in sharing.pending_removal.clone() {
        if ctx.library.entry(&lib).is_some() {
            ops::remove_library_with(ctx, &lib, run)?;
        }
        sharing.items.remove(&lib);
        sharing.pending_removal.retain(|known| *known != lib);
        sharing.save(ctx.library)?;
    }
    let catalogs = Catalogs::read(ctx);
    let model = catalogs.model(ctx, &sharing);
    // Rows with a copy first: a renamed twin (the same endpoint) never gets a second one.
    let mut rows: Vec<&Row> = model.rows.iter().collect();
    rows.sort_by_key(|row| row.lib.is_none());
    let mut given: Vec<String> = model
        .rows
        .iter()
        .filter(|row| row.lib.is_some())
        .filter_map(Row::endpoint)
        .collect();
    let mut error = None;
    for row in rows {
        let key = row.key.text();
        if only.is_some_and(|only| {
            only != key
                && row
                    .lib
                    .as_ref()
                    .is_none_or(|lib| only != format!("lib:{lib}"))
        }) {
            continue;
        }
        let signature = format!(
            "{}|{:?}|{:?}|{:?}",
            row.original().map(fingerprint).unwrap_or_default(),
            row.own.everywhere,
            row.own.here,
            row.copy.anywhere
        );
        if !retry
            && sharing
                .failed
                .get(&key)
                .is_some_and(|failure| failure.signature == signature)
        {
            continue;
        }
        match sync_row(ctx, &catalogs, &mut sharing, row, &mut given, run) {
            Ok(()) => {
                sharing.failed.remove(&key);
            }
            Err(reason) => {
                if only.is_some() {
                    error = Some(reason.clone());
                }
                sharing.failed.insert(key, Failure { reason, signature });
            }
        }
        sharing.save(ctx.library)?;
    }
    let entries = ctx.library.entries();
    sharing
        .items
        .retain(|lib, _| entries.iter().any(|entry| entry.name == *lib));
    sharing.save(ctx.library)?;
    error.map_or(Ok(()), Err)
}

fn sync_row(
    ctx: &Ctx<'_>,
    catalogs: &Catalogs,
    sharing: &mut Sharing,
    row: &Row,
    given: &mut Vec<String>,
    run: Runner<'_>,
) -> Result<(), String> {
    match row.key.kind {
        KeyKind::Plugin => return Ok(()),
        KeyKind::Library => return sync_library(ctx, catalogs, sharing, row, run),
        KeyKind::Server | KeyKind::Skill => {}
    }
    let Some(original) = row.original() else {
        // Only a problem (a config that couldn't be read): nothing to share or follow.
        if let Some(record) = row.lib.as_ref().and_then(|lib| sharing.items.get_mut(lib)) {
            record.absent_passes = 0;
        }
        return Ok(());
    };
    let endpoint = row.endpoint();
    if row.conflict || sharing.is_declined(row) || cant_share(row).is_some() {
        return Ok(());
    }
    let wants = wanted(row, ctx.projects);
    let lib = match &row.lib {
        Some(lib) => lib.clone(),
        None => {
            if wants.iter().all(|(_, placed)| !placed.has)
                || endpoint.as_ref().is_some_and(|mine| given.contains(mine))
            {
                return Ok(());
            }
            let lib = library_name(ctx, row, catalogs);
            let packed = pack(ctx, &lib, original, &row.key.name, false)?;
            sharing.items.insert(
                lib.clone(),
                Record {
                    kind: row.kind,
                    name: row.key.name.clone(),
                    fingerprint: fingerprint(original),
                    file: original.file(),
                    version: packed.entry.version,
                    applied: BTreeMap::new(),
                    absent_passes: 0,
                },
            );
            sharing.save(ctx.library)?;
            lib
        }
    };
    given.extend(endpoint);
    let id = plugin_id(&lib);
    let print = fingerprint(original);
    let record = sharing
        .items
        .get_mut(&lib)
        .ok_or_else(|| format!("Brigadier lost track of {lib}."))?;
    record.absent_passes = 0;
    record.file = original.file();
    if record.fingerprint != print {
        let packed = pack(ctx, &lib, original, &row.key.name, true)?;
        record.fingerprint = print;
        record.version = packed.entry.version;
    }
    // Every agent with an install takes the packed version (it may have missed one before).
    let mut failed = Vec::new();
    for adapter in adapters() {
        let agent = adapter.agent();
        let behind = record
            .applied
            .get(&agent_key(agent))
            .is_none_or(|applied| *applied != record.version);
        if behind
            && (adapter.installed(ctx.roots, &id)
                || !adapter.installed_in_projects(ctx.roots, &id).is_empty())
        {
            match ops::refresh_copy(ctx, adapter, &id, run) {
                Ok(()) => {
                    record
                        .applied
                        .insert(agent_key(agent), record.version.clone());
                }
                Err(err) => failed.push(format!("{}: {err}", adapter.label())),
            }
        }
    }
    let moves: Vec<(Agent, Placed)> = wants
        .into_iter()
        .filter(|(agent, want)| !copy_now(ctx, row, *agent, &id).same(want))
        .collect();
    if !moves.is_empty() {
        ops::change(ctx, &id, &moves, run)?;
        for (agent, want) in &moves {
            if want.has {
                record
                    .applied
                    .insert(agent_key(*agent), record.version.clone());
            } else {
                record.applied.remove(&agent_key(*agent));
            }
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Not every agent took its new version: {}",
            failed.join("; ")
        ))
    }
}

/// A Library plugin no agent has an own entry for: a copy whose original is gone, or an item
/// added through Brigadier, which every agent has.
fn sync_library(
    ctx: &Ctx<'_>,
    catalogs: &Catalogs,
    sharing: &mut Sharing,
    row: &Row,
    run: Runner<'_>,
) -> Result<(), String> {
    let Some(lib) = &row.lib else {
        return Ok(());
    };
    if let Some(record) = sharing.items.get_mut(lib) {
        // Gone only when its file reads cleanly without it, two passes running (review #4).
        let still = match record.kind {
            PluginKind::Skill => record.file.join("SKILL.md").exists(),
            PluginKind::Server => catalogs.unreadable(&record.file),
        };
        if still {
            record.absent_passes = 0;
            return Ok(());
        }
        record.absent_passes += 1;
        if record.absent_passes < 2 {
            return Ok(());
        }
        ops::remove_library_with(ctx, lib, run)?;
        sharing.items.remove(lib);
        return Ok(());
    }
    let id = plugin_id(lib);
    let lacking = adapters().into_iter().any(|adapter| {
        !row.copy.anywhere.contains(&adapter.agent())
            && !adapter.installed(ctx.roots, &id)
            && adapter.installed_in_projects(ctx.roots, &id).is_empty()
    });
    if lacking {
        ops::add_missing_with(ctx, &format!("lib:{lib}"), run)?;
    }
    Ok(())
}

/// The row page key `key` names, read now.
fn find(ctx: &Ctx<'_>, sharing: &Sharing, key: &str) -> Result<(Catalogs, Row), String> {
    let catalogs = Catalogs::read(ctx);
    let row = catalogs
        .model(ctx, sharing)
        .find(key)
        .cloned()
        .ok_or_else(|| "It's gone: none of your agents has it now.".to_owned())?;
    Ok((catalogs, row))
}

/// Whether `library`'s sharing waits for a confirming pass (a removed original read once).
pub(crate) fn confirming(library: &Library) -> bool {
    Sharing::load(library).is_ok_and(|sharing| sharing.confirming())
}

/// Brings another app's server (`key`: `other:<id>`, as the page's `elsewhere` lists it) to
/// every agent, as Add does a pasted server: packed into the Library (its values to the
/// Keychain), on in every project or in `scope`. The other app's file is only read. `name`
/// replaces its own on a clash. Its key.
pub(crate) fn import(
    ctx: &Ctx<'_>,
    key: &str,
    name: Option<&str>,
    scope: Option<&[PathBuf]>,
    run: Runner<'_>,
) -> Result<String, String> {
    if scope.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".to_owned());
    }
    let id = key
        .strip_prefix("other:")
        .ok_or_else(|| format!("Brigadier doesn't know \"{key}\"."))?;
    let catalog = Catalog::read(ctx.roots, None);
    let server = catalog
        .servers
        .iter()
        .chain(&catalog.off.servers)
        .find(|server| {
            server.id == id
                && matches!(
                    server.source,
                    brigadier_registry::Source::Desktop | brigadier_registry::Source::CursorUser
                )
        })
        .ok_or_else(|| "It's gone from the app it was in.".to_owned())?;
    if server.templated {
        return Err(
            "It reads ${VAR} values from your shell, so Brigadier can't bring it in as it is."
                .to_owned(),
        );
    }
    let name = name.map_or_else(|| server.name.clone(), str::to_owned);
    let mut spec = spec_of(server);
    spec.name.clone_from(&name);
    let mut sharing = Sharing::load(ctx.library)?;
    let staged = Staged::default();
    let packed = ctx
        .library
        .pack(
            &name,
            &Item::Server(spec),
            None,
            &ops::server_names(ctx, None),
            false,
            &staged,
        )
        .map_err(|err| err.to_string())?;
    staged.commit(ctx.secrets, None)?;
    sharing.imported.push(key.to_owned());
    sharing.save(ctx.library)?;
    let want = Placed::at(scope, ctx.projects);
    let wants: Vec<(Agent, Placed)> = adapters()
        .into_iter()
        .map(|adapter| (adapter.agent(), want.clone()))
        .collect();
    ops::change(ctx, &packed.entry.id(), &wants, run)?;
    Ok(format!("lib:{name}"))
}

/// The Library plugin page key `key` names: its copy, or the Library item itself.
pub(crate) fn library_of(ctx: &Ctx<'_>, key: &str) -> Result<String, String> {
    let sharing = Sharing::load(ctx.library)?;
    let (_, row) = find(ctx, &sharing, key)?;
    row.lib
        .ok_or_else(|| "Only an item in your Library can be updated here.".to_owned())
}

/// The projects `row` is on in: `None` for every project.
fn scope_now(row: &Row, projects: &[PathBuf]) -> Option<Vec<PathBuf>> {
    let everywhere = row.everywhere();
    let here = row.here();
    let on_in: Vec<PathBuf> = projects
        .iter()
        .filter(|path| here.iter().any(|(at, _)| at == *path))
        .cloned()
        .collect();
    if !everywhere.is_empty() && on_in.len() == projects.len() {
        None
    } else {
        Some(on_in)
    }
}

/// Codex's own server `name`'s switch in `project`'s file, when it sets one.
fn codex_switch_in(project: &Path, name: &str) -> Option<bool> {
    let text =
        std::fs::read_to_string(brigadier_registry::adapter::codex::project_file(project)).ok()?;
    let table: toml::Table = toml::from_str(&text).ok()?;
    table
        .get("mcp_servers")?
        .get(name)?
        .get("enabled")?
        .as_bool()
}

/// The steps that put Codex's own server `name` on in every project (`scope` `None`), in
/// exactly `scope`, or (`on` false) off everywhere: its own `enabled`, in its config and in
/// each trusted project's file.
fn codex_server_steps(
    catalogs: &Catalogs,
    name: &str,
    on: bool,
    scope: Option<&[PathBuf]>,
) -> Result<Vec<brigadier_registry::adapter::Step>, String> {
    let codex = adapter(Agent::Codex);
    let unsupported = |err: brigadier_registry::adapter::Unsupported| err.0;
    let off_now = catalogs
        .user
        .off
        .servers
        .iter()
        .any(|server| server.name == name && codex.owns(server.source));
    // Only some projects is on only there: off in the user's own config, on by each picked
    // project's file, so a folder that isn't picked (a new one too) doesn't load it (review #5).
    let everywhere = on && scope.is_none();
    let mut steps = Vec::new();
    if everywhere == off_now {
        steps.extend(
            codex
                .set_server(name, &Place::Everywhere, everywhere)
                .map_err(unsupported)?,
        );
    }
    for project in &catalogs.projects {
        if !codex.trusts(&project.catalog, &project.path) {
            continue;
        }
        let here = on && scope.is_some_and(|scope| scope.contains(&project.path));
        let now = codex_switch_in(&project.path, name);
        let want = if everywhere {
            (now == Some(false)).then_some(true)
        } else if here {
            (now != Some(true)).then_some(true)
        } else {
            (now == Some(true)).then_some(false)
        };
        if let Some(want) = want {
            steps.extend(
                codex
                    .set_server(name, &ops::just_me(&project.path), want)
                    .map_err(unsupported)?,
            );
        }
    }
    Ok(steps)
}

/// Puts `row` on (in every project, or exactly `scope`) or off, for every agent: its Library
/// copy, Codex's own servers, another marketplace's plugin.
fn apply(
    ctx: &Ctx<'_>,
    catalogs: &Catalogs,
    row: &Row,
    on: bool,
    scope: Option<&[PathBuf]>,
    run: Runner<'_>,
) -> Result<(), String> {
    let want = if on {
        Placed::at(scope, ctx.projects)
    } else {
        Placed::off()
    };
    match row.key.kind {
        KeyKind::Plugin => {
            let name = row.key.name.split('/').next().unwrap_or_default();
            for agent in &row.own.anywhere {
                if let Some(id) = ops::plugin_id_of(ctx, *agent, name) {
                    ops::change(ctx, &id, &[(*agent, want.clone())], run)?;
                }
            }
            return Ok(());
        }
        KeyKind::Server | KeyKind::Skill | KeyKind::Library => {}
    }
    for source in &row.own.sources {
        if let Origin::Server { agent, server } = source
            && *agent == Agent::Codex
            && server.project.is_none()
        {
            let steps = codex_server_steps(catalogs, &server.name, on, scope)?;
            if !steps.is_empty() {
                run(adapter(Agent::Codex), &steps)?;
            }
        }
    }
    if let Some(lib) = &row.lib {
        let id = plugin_id(lib);
        let holders = row.holders();
        let wants: Vec<(Agent, Placed)> = adapters()
            .into_iter()
            .map(|adapter| adapter.agent())
            .filter(|agent| !holders.contains(agent))
            .map(|agent| (agent, want.clone()))
            .collect();
        if !wants.is_empty() {
            ops::change(ctx, &id, &wants, run)?;
        }
    }
    Ok(())
}

/// Turns item `key` on or off for every agent, keeping its projects for when it is on again.
pub(crate) fn switch(ctx: &Ctx<'_>, key: &str, on: bool, run: Runner<'_>) -> Result<(), String> {
    let mut sharing = Sharing::load(ctx.library)?;
    let (catalogs, row) = find(ctx, &sharing, key)?;
    if let Some(why) = &row.own.lock {
        return Err(why.clone());
    }
    let key = row.key.text();
    if on {
        let scope = sharing
            .off_scopes
            .get(&key)
            .cloned()
            .flatten()
            .map(|paths| {
                paths
                    .into_iter()
                    .filter(|path| ctx.projects.contains(path))
                    .collect::<Vec<_>>()
            });
        let scope = scope.filter(|paths| !paths.is_empty());
        apply(ctx, &catalogs, &row, true, scope.as_deref(), run)?;
        sharing.off_scopes.remove(&key);
    } else {
        if row.on() {
            sharing
                .off_scopes
                .insert(key.clone(), scope_now(&row, ctx.projects));
            sharing.save(ctx.library)?;
        }
        apply(ctx, &catalogs, &row, false, None, run)?;
    }
    sharing.save(ctx.library)
}

/// Puts item `key` on in every project (`scope` `None`) or exactly the projects `scope`, for
/// every agent.
pub(crate) fn set_scope(
    ctx: &Ctx<'_>,
    key: &str,
    scope: Option<&[PathBuf]>,
    run: Runner<'_>,
) -> Result<(), String> {
    if scope.is_some_and(<[PathBuf]>::is_empty) {
        return Err("Pick a project, or All projects.".to_owned());
    }
    let mut sharing = Sharing::load(ctx.library)?;
    let (catalogs, row) = find(ctx, &sharing, key)?;
    if let Some(why) = &row.own.lock {
        return Err(why.clone());
    }
    if row.on() {
        apply(ctx, &catalogs, &row, true, scope, run)
    } else {
        // Off, it keeps the choice for when it is switched on.
        sharing
            .off_scopes
            .insert(row.key.text(), scope.map(<[PathBuf]>::to_vec));
        sharing.save(ctx.library)
    }
}

/// Removes what Brigadier made for item `key` (L6 Q15, L8): its copies, or a Library item
/// added through Brigadier whole. An agent's own entry stays exactly as it is; a shared item
/// is remembered as removed. An item the old Use everywhere moved gets its originals back
/// first.
pub(crate) fn remove(ctx: &Ctx<'_>, key: &str, run: Runner<'_>) -> Result<(), String> {
    let mut sharing = Sharing::load(ctx.library)?;
    let (_, row) = find(ctx, &sharing, key)?;
    let own = !row.own.sources.is_empty();
    let declined = Declined::of(&row);
    let Some(lib) = row.lib.clone() else {
        if own && row.key.kind != KeyKind::Plugin {
            // Its copies went already: what's left is to remember.
            if !sharing.declined.contains(&declined) {
                sharing.declined.push(declined);
            }
            return sharing.save(ctx.library);
        }
        return Err(
            "Brigadier made nothing for it: it's in your agents' own settings, so it stays."
                .to_owned(),
        );
    };
    let legacy = ops::read_moved(ctx.library, &lib)?;
    // Written down before anything goes: an interrupted removal finishes, and is never undone
    // by sharing it again (review #15).
    if own || legacy.is_some() {
        let declined = if own {
            declined
        } else {
            Declined {
                kind: row.kind,
                name: lib.clone(),
                endpoint: None,
                source: None,
            }
        };
        if !sharing.declined.contains(&declined) {
            sharing.declined.push(declined);
        }
    }
    if let Some(moved) = &legacy {
        // Its copies off first: an original and its copy don't both load.
        let id = plugin_id(&lib);
        for adapter in adapters() {
            if adapter.installed(ctx.roots, &id) {
                let steps = adapter
                    .set_plugin(&id, &Place::Everywhere, false)
                    .map_err(|err| err.0)?;
                run(adapter, &steps)?;
            }
        }
        sharing.save(ctx.library)?;
        // Each original back in turn, written down as it goes: a Remove tried again after one
        // couldn't go back skips those already back (review #4).
        let mut moved = moved.clone();
        for at in 0..moved.originals.len() {
            if moved.restored.contains(&at) {
                continue;
            }
            let original = &moved.originals[at];
            if !original.back() {
                original.restore(ctx).map_err(|err| {
                    format!(
                        "Not every original could be put back, so Brigadier kept its copy: {err}"
                    )
                })?;
            }
            moved.restored.push(at);
            ops::write_moved(ctx.library, &lib, &moved)?;
        }
        // Their values were kept until every original was back.
        for original in &moved.originals {
            if let Some(secrets) = original.secrets() {
                ctx.secrets.forget(secrets)?;
            }
        }
    }
    if !sharing.pending_removal.contains(&lib) {
        sharing.pending_removal.push(lib.clone());
    }
    sharing.save(ctx.library)?;
    ops::remove_library_with(ctx, &lib, run)?;
    if legacy.is_some() {
        let _ = std::fs::remove_file(ops::moved_file(ctx.library, &lib));
        let _ = std::fs::remove_dir_all(ops::moved_dir(ctx.library).join(&lib));
    }
    let key = row.key.text();
    sharing.items.remove(&lib);
    sharing.pending_removal.retain(|known| *known != lib);
    sharing.off_scopes.remove(&key);
    sharing.failed.remove(&key);
    sharing.save(ctx.library)
}

/// Share with every agent: forgets that the user removed item `key` (or that sharing it
/// failed) and shares it now.
pub(crate) fn share_again(ctx: &Ctx<'_>, key: &str, run: Runner<'_>) -> Result<(), String> {
    let mut sharing = Sharing::load(ctx.library)?;
    let (_, row) = find(ctx, &sharing, key)?;
    if row.key.kind == KeyKind::Plugin {
        return Err("It came in its agent's own plugin, so it can't be shared.".to_owned());
    }
    if let Some(why) = cant_share(&row) {
        return Err(format!("It can't be shared: {why}."));
    }
    sharing.declined.retain(|declined| !declined.is(&row));
    let key = row.key.text();
    sharing.failed.remove(&key);
    sharing.save(ctx.library)?;
    sync(ctx, Some(&key), true, run)
}

/// What item `key` is set up with: its skill files to open and edit, or its server's settings
/// (values' names only).
pub(crate) fn details(ctx: &Ctx<'_>, key: &str) -> Result<PluginItemDetails, String> {
    let sharing = Sharing::load(ctx.library).unwrap_or_default();
    let (catalogs, row) = find(ctx, &sharing, key)?;
    let mut skills = Vec::new();
    let mut file = |name: &str, path: PathBuf, read_only: Option<String>| {
        if skills
            .iter()
            .any(|known: &PluginSkillFile| known.path == path.display().to_string())
        {
            return;
        }
        skills.push(PluginSkillFile {
            name: name.to_owned(),
            contents: std::fs::read_to_string(&path).unwrap_or_default(),
            path: path.display().to_string(),
            read_only,
        });
    };
    let mut server = None;
    match row.key.kind {
        KeyKind::Plugin => {
            let name = row.key.name.split('/').next().unwrap_or_default();
            let all = std::iter::once(&catalogs.user)
                .chain(catalogs.projects.iter().map(|project| &project.catalog));
            for catalog in all {
                for skill in catalog.skills.iter().chain(&catalog.off.skills) {
                    if let Some(plugin) = skill.plugin()
                        && plugin.name == name
                    {
                        let label = adapter(plugin.agent).label();
                        file(
                            &skill.name,
                            skill.skill_md(),
                            Some(format!(
                                "It came in {label}'s own plugin: its next update would undo an edit."
                            )),
                        );
                    }
                }
                if server.is_none()
                    && let Some((_, wanted)) = row.key.name.split_once('/')
                {
                    server = catalog
                        .servers
                        .iter()
                        .chain(&catalog.off.servers)
                        .find(|known| {
                            known.name == wanted
                                && known
                                    .plugin
                                    .as_ref()
                                    .is_some_and(|plugin| plugin.name == name)
                        })
                        .map(settings);
                }
            }
        }
        _ => {
            for source in &row.own.sources {
                match source {
                    Origin::Skill { skill, .. } => file(&skill.name, skill.skill_md(), None),
                    Origin::Server { server: found, .. } => {
                        if server.is_none() {
                            server = Some(settings(found));
                        }
                    }
                }
            }
            if row.own.sources.is_empty()
                && let Some(lib) = &row.lib
                && let Some(entry) = ctx.library.entry(lib)
            {
                if let Ok(folders) = std::fs::read_dir(entry.dir.join("skills")) {
                    let mut folders: Vec<_> = folders.flatten().map(|dir| dir.path()).collect();
                    folders.sort();
                    for folder in folders {
                        let name = library::skill_name(&folder).unwrap_or_else(|| lib.clone());
                        file(&name, folder.join("SKILL.md"), None);
                    }
                }
                server = std::iter::once(&catalogs.user)
                    .flat_map(|catalog| catalog.servers.iter().chain(&catalog.off.servers))
                    .find(|known| {
                        known
                            .plugin
                            .as_ref()
                            .is_some_and(|plugin| plugin.id == plugin_id(lib))
                    })
                    .map(settings);
                // Its values are in the Keychain: the Library keeps their names (review #14).
                let names = ctx.library.value_names(lib);
                match &mut server {
                    Some(PluginServerSettings::Stdio { env_names, .. }) => *env_names = names,
                    Some(PluginServerSettings::Http { header_names, .. }) => *header_names = names,
                    None => {}
                }
            }
        }
    }
    Ok(PluginItemDetails {
        key: row.key.text(),
        kind: row.kind,
        skills,
        server,
    })
}

/// A server's settings as its config writes them: no value, only the names of its values.
fn settings(server: &Server) -> PluginServerSettings {
    let written = &server.written;
    match &server.transport {
        Transport::Stdio {
            command,
            args,
            env,
            cwd,
            ..
        } => {
            let (command, args) = match &written.command {
                Some(command) => (command.clone(), written.args.clone()),
                None => (command.clone(), args.clone()),
            };
            let (command, args) = unwrap_brigadier(command, args);
            PluginServerSettings::Stdio {
                command,
                args,
                cwd: written
                    .cwd
                    .clone()
                    .or_else(|| cwd.as_ref().map(|cwd| cwd.display().to_string())),
                env_names: env.iter().map(|(name, _)| name.clone()).collect(),
            }
        }
        Transport::Http { url, headers } => PluginServerSettings::Http {
            url: written.url.clone().unwrap_or_else(|| url.clone()),
            header_names: headers.iter().map(|(name, _)| name.clone()).collect(),
        },
    }
}

/// `brigadier mcp run <id> -- <command> <args…>` as the command it runs.
fn unwrap_brigadier(command: String, args: Vec<String>) -> (String, Vec<String>) {
    let brigadier = Path::new(&command)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("brigadier"));
    if brigadier
        && args.first().map(String::as_str) == Some("mcp")
        && args.get(1).map(String::as_str) == Some("run")
        && let Some(at) = args.iter().position(|arg| arg == "--")
        && let Some((inner, rest)) = args[at + 1..].split_first()
    {
        return (inner.clone(), rest.to_vec());
    }
    (command, args)
}

/// How saving a skill file went, short of an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Saved {
    Saved,
    /// It no longer holds what was opened: nothing was written. Its text now.
    ChangedSinceOpened {
        current: String,
    },
}

/// Saves `contents` as item `key`'s skill file `path`, which must still hold `expected` (what
/// was opened): an edit made meanwhile elsewhere is never overwritten (review #5).
pub(crate) fn write_skill(
    ctx: &Ctx<'_>,
    key: &str,
    path: &str,
    contents: &str,
    expected: &str,
) -> Result<Saved, String> {
    let found = details(ctx, key)?;
    let file = found
        .skills
        .iter()
        .find(|file| file.path == path)
        .ok_or_else(|| "That file isn't one of this item's.".to_owned())?;
    if let Some(why) = &file.read_only {
        return Err(why.clone());
    }
    // Through a link, the file it names (a dotfiles folder) is the one written.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    let now =
        std::fs::read_to_string(&target).map_err(|err| format!("Couldn't read {path}: {err}"))?;
    if now != expected {
        return Ok(Saved::ChangedSinceOpened { current: now });
    }
    let temp = target.with_file_name(format!(".SKILL.md.brigadier-{}", std::process::id()));
    std::fs::write(&temp, contents)
        .and_then(|()| std::fs::rename(&temp, &target))
        .map_err(|err| {
            let _ = std::fs::remove_file(&temp);
            format!("Couldn't save {path}: {err}")
        })?;
    Ok(Saved::Saved)
}

/// After item `key`'s skill file `path` was saved: its copies follow it.
pub(crate) fn follow_skill(
    ctx: &Ctx<'_>,
    key: &str,
    path: &str,
    run: Runner<'_>,
) -> Result<(), String> {
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path));
    let sharing = Sharing::load(ctx.library)?;
    let (_, row) = find(ctx, &sharing, key)?;
    if row.own.sources.is_empty()
        && let Some(lib) = &row.lib
    {
        // A Library item's own files: packed again, and every agent takes the new version.
        let folder = target
            .parent()
            .ok_or_else(|| format!("{path} has no folder."))?
            .to_owned();
        let item = Item::Skill(SkillSpec {
            name: Some(row.key.name.clone()),
            source: SkillSource::Folder(folder.clone()),
        });
        let scratch =
            std::env::temp_dir().join(format!("brigadier-skill-{}", uuid::Uuid::new_v4().simple()));
        copy_dir(&folder, &scratch)
            .map_err(|err| format!("Couldn't read {path}'s folder: {err}"))?;
        let packed = ctx
            .library
            .pack(lib, &item, Some(&scratch), &[], true, ctx.secrets)
            .map_err(|err| err.to_string());
        let _ = std::fs::remove_dir_all(&scratch);
        if packed?.changed {
            let id = plugin_id(lib);
            for adapter in adapters() {
                ops::refresh_copy(ctx, adapter, &id, run)?;
            }
        }
        return Ok(());
    }
    sync(ctx, Some(&row.key.text()), true, run)
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if std::fs::metadata(entry.path())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "share_tests.rs"]
mod tests;
