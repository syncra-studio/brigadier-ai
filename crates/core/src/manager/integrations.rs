//! MCP servers and skills shared between the agents (PLAN.md §6 Phase 7), from the catalog
//! `brigadier_registry` reads of every agent's own configuration:
//!
//! - Each thread and worker keeps its own agent's servers. A worker also gets the servers its
//!   task names (`delegate_task`'s `integrations`), from the other agent, Claude Desktop or
//!   Cursor: only those, since Codex loads every tool of each server it gets into its context.
//! - Every worker gets the switched-on skills its agent doesn't find by itself, in a plugin
//!   folder of Brigadier's ([`shared_skills`]). A thread keeps its own agent's skills.
//! - What the user switched off for Brigadier (Settings' `mcpServersOff` and `skillsOff`, by
//!   catalog id) goes to no thread or worker, their agent's own included
//!   (docs/research/phase7-probes.md). A change takes effect at each session's next start or
//!   resume.
//!
//! Values of a server's environment and headers go only into the CLI's environment (or a Codex
//! thread's config, which stays off disk) and the redactor: the view shows their names.
//!
//! A remote server that signs in with OAuth reaches a worker whose agent's own sign-in doesn't
//! serve it through Brigadier's sign-in proxy ([`super::mcp_auth`]), with another agent's
//! stored sign-in or Brigadier's own, whether the task named it or its agent has it.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use brigadier_providers::{McpServer, McpTransport, ProviderKind, SwitchedOff};
use brigadier_registry::{
    Agent, Catalog, CodexStore, FindError, OAuthClient, Owner, Probe, Registry, Roots, Secret,
    Server, Session, SignIn, SignInStores, SkillRoot, Source, Transport, identity, owner_agent,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::SessionManager;
use super::mcp_auth::{Homes, McpAuth, McpSignInStatus, Open, Place, Prober, Route};
use crate::model::{ConversationId, ProjectId, Settings};
use crate::{Error, Result};

/// Brigadier's own servers' names, which no shared server may take.
const RESERVED: &[&str] = &["brigadier", super::computer_use::SERVER_NAME];

/// The plugin the shared skills are in: Claude shows each as `brigadier-shared:<name>`.
const PLUGIN_NAME: &str = "brigadier-shared";

/// The servers and skills on this machine, for Settings. Only names of environment variables
/// and headers, never their values. A switch takes effect at each session's next start or
/// resume.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationsView {
    pub servers: Vec<McpServerView>,
    pub skills: Vec<SkillView>,
    /// The servers and config files that couldn't be read, which no worker gets.
    pub problems: Vec<ProblemView>,
}

/// A server, or a whole config file, that couldn't be read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProblemView {
    /// Source-qualified like a server's.
    pub id: String,
    /// The server's name, or the file's.
    pub name: String,
    /// Where it is configured, as [`McpServerView::source`].
    pub source: String,
    pub project: Option<String>,
    /// The config file.
    pub file: String,
    pub line: Option<u32>,
    /// What was read of it (an address or command), if anything.
    pub read: Option<String>,
    pub error: String,
}

/// One MCP server, as one source configures it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpServerView {
    /// Stable and source-qualified (`claude:user:railway`): what `mcpServersOff` holds.
    pub id: String,
    /// The name its tools are called by, which `delegate_task` takes.
    pub name: String,
    /// Where it is configured: "Claude Code (user)", "Codex (project)", "Claude Desktop",
    /// "Claude Code (plugin)".
    pub source: String,
    /// The project whose config holds it.
    pub project: Option<String>,
    /// The plugin it comes with, as its agent's switch names it (`<plugin>@<marketplace>`).
    pub plugin: Option<String>,
    /// The file it is configured in.
    pub file: String,
    pub transport: TransportView,
    /// A remote server that signs in with OAuth: how each agent's workers in the project are
    /// signed in to it. `None`: it doesn't sign in.
    pub sign_in: Option<SignInView>,
    /// The sign-in in Brigadier to it waiting for the user, if one is: its id, as
    /// `SignInMcpServer` gave it.
    pub pending_sign_in: Option<String>,
    /// The agents whose workers in the project have it by themselves; any other gets it only
    /// when a task names it.
    pub native_to: Vec<ProviderKind>,
    /// Switched off for Brigadier: no thread or worker gets it.
    pub off: bool,
    /// The same for two copies of one server (with the same name and project): a hash of its
    /// transport with its environment and header values, keyed afresh at each daemon start, so
    /// it tells no value.
    pub copy_key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TransportView {
    Stdio {
        command: String,
        args: Vec<String>,
        /// Names of the variables it is given.
        env: Vec<String>,
        /// The folder it runs in.
        cwd: Option<String>,
    },
    Http {
        url: String,
        /// Names of the headers it is sent.
        headers: Vec<String>,
    },
}

/// How each agent is signed in to a remote server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignInView {
    pub claude: SignInState,
    pub codex: SignInState,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SignInState {
    /// The agent has the server by itself and its own sign-in serves it.
    Native,
    /// The sign-in another agent stored can serve it.
    ReusableFrom { agent: ProviderKind },
    /// Brigadier's own sign-in serves it, to every agent: made at `since_ms` (ms since the
    /// epoch), when known.
    InBrigadier { since_ms: Option<i64> },
    /// No stored sign-in serves it: the user signs in once, in an agent or in Brigadier.
    Needed,
    /// Not known yet: a store couldn't be read without asking the user, or it failed.
    Unknown,
    /// `agent`'s stored sign-in can't be reused: macOS wasn't allowed to let Brigadier read
    /// it. `AskMcpSignInAgain` reads it again.
    NotAllowed { agent: ProviderKind },
    /// Sign-ins to it can't be checked or kept on this system.
    Unsupported,
}

fn provider(agent: Agent) -> ProviderKind {
    match agent {
        Agent::Claude => ProviderKind::Claude,
        Agent::Codex => ProviderKind::Codex,
    }
}

impl SignInState {
    /// `sign_in`, with when Brigadier's own sign-in was made (ms since the epoch).
    fn of(sign_in: SignIn, since: Option<u64>) -> Self {
        match sign_in {
            SignIn::Native => Self::Native,
            SignIn::ReusableFrom(agent) => Self::ReusableFrom {
                agent: provider(agent),
            },
            SignIn::InBrigadier => Self::InBrigadier {
                since_ms: since.and_then(|ms| i64::try_from(ms).ok()),
            },
            SignIn::Needed => Self::Needed,
            SignIn::Unknown => Self::Unknown,
            SignIn::NotAllowed(agent) => Self::NotAllowed {
                agent: provider(agent),
            },
            SignIn::Unsupported => Self::Unsupported,
        }
    }
}

/// `pairs` with their values, in order.
fn sorted(pairs: &[(String, Secret)]) -> Vec<(&str, &str)> {
    let mut pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(name, value)| (name.as_str(), value.expose()))
        .collect();
    pairs.sort_unstable();
    pairs
}

/// [`McpServerView::copy_key`] of `transport`.
fn copy_key(transport: &Transport) -> String {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    let key = KEY.get_or_init(|| {
        let mut key = [0; 32];
        key[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        key
    });
    let mut hasher = blake3::Hasher::new_keyed(key);
    let mut part = |text: &str| {
        hasher.update(&(text.len() as u64).to_le_bytes());
        hasher.update(text.as_bytes());
    };
    match transport {
        Transport::Stdio {
            command, args, env, ..
        } => {
            part("stdio");
            part(command);
            part(&args.len().to_string());
            for arg in args {
                part(arg);
            }
            let env = sorted(env);
            part(&env.len().to_string());
            for (name, value) in env {
                part(name);
                part(value);
            }
            part(
                &transport
                    .working_dir()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_default(),
            );
        }
        Transport::Http { url, headers } => {
            part("http");
            part(url);
            for (name, value) in sorted(headers) {
                part(name);
                part(value);
            }
        }
    }
    hasher.finalize().to_hex()[..32].to_owned()
}

/// One skill folder, wherever it is found.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillView {
    /// Stable and root-qualified (`agents:user:<folder>`): what `skillsOff` holds.
    pub id: String,
    pub name: String,
    pub description: String,
    /// Its folder.
    pub path: String,
    /// Each folder it is found at, as found.
    pub locations: Vec<SkillLocationView>,
    /// The plugin it comes with when it comes only with plugins: its agent shows it as
    /// `<plugin>:<name>`.
    pub plugin: Option<String>,
    /// The agents whose workers in the project find it by themselves; every other worker gets
    /// it from Brigadier.
    pub native_to: Vec<ProviderKind>,
    /// Switched off for Brigadier: no thread or worker gets it.
    pub off: bool,
    /// Claude Code switches its own skills off by name: switching this one off hides every
    /// skill Claude finds by itself under the same name (a plugin's skill only goes by its
    /// `<plugin>:<name>`).
    pub claude_off_by_name: bool,
}

/// One folder a skill is found at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkillLocationView {
    pub path: String,
    /// The skill's own folder, when this one is a link to it.
    pub linked_to: Option<String>,
    /// The plugin this folder comes with, as its agent's switch names it.
    pub plugin: Option<String>,
    /// The agents whose workers in the project find it here by themselves.
    pub native_to: Vec<ProviderKind>,
}

fn agent(provider: ProviderKind) -> Agent {
    match provider {
        ProviderKind::Claude => Agent::Claude,
        ProviderKind::Codex => Agent::Codex,
    }
}

/// The registry for the CLIs' environment `vars`.
pub(super) fn registry(vars: impl IntoIterator<Item = (String, String)>) -> Arc<Registry> {
    let roots = Roots::from_vars(vars).unwrap_or_else(|| Roots {
        home: PathBuf::new(),
        claude_dir: PathBuf::new(),
        claude_json: PathBuf::new(),
        codex_home: PathBuf::new(),
        desktop_config: None,
        cursor_dir: PathBuf::new(),
        agents_skills: PathBuf::new(),
        vars: BTreeMap::new(),
    });
    Registry::new(roots)
}

/// Where a project's workers start: a Claude worker in Brigadier's worker folder with the
/// project linked in, a Codex writer in its checkout. Read-only Codex workers start in a scratch
/// folder and so miss the project's `.agents/skills`; the view counts the writer.
fn worker_session<'a>(provider: ProviderKind, project: Option<&'a Path>) -> Session<'a> {
    Session {
        agent: agent(provider),
        cwd: Path::new(""),
        project,
        in_checkout: provider == ProviderKind::Codex,
    }
}

/// The project whose setup a worker of a session in `repo` loads: only one with a checkout of
/// it starts in it.
fn worker_project(repo: Option<&Path>, has_checkout: bool) -> Option<&Path> {
    repo.filter(|_| has_checkout)
}

/// The agents whose workers in `project` have `server` by themselves.
fn server_native_to(
    catalog: &Catalog,
    server: &Server,
    project: Option<&Path>,
) -> Vec<ProviderKind> {
    [ProviderKind::Claude, ProviderKind::Codex]
        .into_iter()
        .filter(|provider| catalog.server_native(server, &worker_session(*provider, project)))
        .collect()
}

/// Settings' view of `catalog`, read for `project`, with the sign-ins `stores` read.
pub(crate) fn view(
    catalog: &Catalog,
    project: Option<&Path>,
    settings: &Settings,
    stores: &Prober<'_>,
) -> IntegrationsView {
    let names = |pairs: &[(String, brigadier_registry::Secret)]| {
        pairs.iter().map(|(name, _)| name.clone()).collect()
    };
    let servers = catalog
        .servers
        .iter()
        .map(|server| McpServerView {
            id: server.id.clone(),
            name: server.name.clone(),
            source: server.source.label().to_owned(),
            project: server.project.as_ref().map(|p| p.display().to_string()),
            plugin: server.plugin.as_ref().map(|plugin| plugin.id.clone()),
            file: server.file.display().to_string(),
            transport: match &server.transport {
                Transport::Stdio {
                    command, args, env, ..
                } => TransportView::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                    env: names(env),
                    cwd: server
                        .transport
                        .working_dir()
                        .map(|dir| dir.display().to_string()),
                },
                Transport::Http { url, headers } => TransportView::Http {
                    url: url.clone(),
                    headers: names(headers),
                },
            },
            sign_in: {
                let state = |provider| {
                    catalog
                        .sign_in(server, &worker_session(provider, project), stores)
                        .map(|sign_in| SignInState::of(sign_in, stores.signed_in_at(server)))
                };
                state(ProviderKind::Claude)
                    .zip(state(ProviderKind::Codex))
                    .map(|(claude, codex)| SignInView { claude, codex })
            },
            pending_sign_in: server
                .signs_in
                .then(|| stores.pending_sign_in(server))
                .flatten(),
            native_to: server_native_to(catalog, server, project),
            off: settings.mcp_servers_off.contains(&server.id),
            copy_key: copy_key(&server.transport),
        })
        .collect();
    let skills = catalog
        .skills
        .iter()
        .map(|skill| {
            let native_to: Vec<ProviderKind> = [ProviderKind::Claude, ProviderKind::Codex]
                .into_iter()
                .filter(|provider| catalog.skill_native(skill, &worker_session(*provider, project)))
                .collect();
            SkillView {
                id: skill.id.clone(),
                name: skill.name.clone(),
                description: skill.description.clone(),
                path: skill.dir.display().to_string(),
                locations: skill
                    .entries
                    .iter()
                    .map(|(root, path)| SkillLocationView {
                        path: path.display().to_string(),
                        linked_to: std::fs::symlink_metadata(path)
                            .is_ok_and(|meta| meta.file_type().is_symlink())
                            .then(|| skill.dir.display().to_string()),
                        plugin: match root {
                            SkillRoot::Plugin(plugin) => Some(plugin.id.clone()),
                            _ => None,
                        },
                        native_to: [ProviderKind::Claude, ProviderKind::Codex]
                            .into_iter()
                            .filter(|provider| root.native(&worker_session(*provider, project)))
                            .collect(),
                    })
                    .collect(),
                plugin: skill.plugin().map(|plugin| plugin.name.clone()),
                claude_off_by_name: skill.roots.iter().any(|root| {
                    !matches!(root, SkillRoot::Plugin(_))
                        && root.native(&worker_session(ProviderKind::Claude, project))
                }),
                native_to,
                off: settings.skills_off.contains(&skill.id),
            }
        })
        .collect();
    let problems = catalog
        .problems
        .iter()
        .map(|problem| ProblemView {
            id: problem.id.clone(),
            name: problem.name.clone(),
            source: problem.source.label().to_owned(),
            project: problem.project.as_ref().map(|p| p.display().to_string()),
            file: problem.file.display().to_string(),
            line: problem.line.and_then(|line| u32::try_from(line).ok()),
            read: problem.read.clone(),
            error: problem.error.clone(),
        })
        .collect();
    IntegrationsView {
        servers,
        skills,
        problems,
    }
}

/// What is switched off for sessions in one project: the user's switches with that project's
/// exceptions ([`Settings::server_off`]).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Switches<'a> {
    pub settings: &'a Settings,
    pub project: Option<&'a ProjectId>,
}

impl Switches<'_> {
    fn server_off(&self, id: &str) -> bool {
        self.settings.server_off(id, self.project)
    }

    fn skill_off(&self, id: &str) -> bool {
        self.settings.skill_off(id, self.project)
    }
}

fn switched_off_error(entry: &str) -> String {
    format!(
        "integration `{entry}` is switched off for Brigadier (the user's setting): no worker gets it"
    )
}

/// The ids of the servers `entries` name (a name or an id each), or why one can't be given.
pub(crate) fn resolve(
    catalog: &Catalog,
    entries: &[String],
    switches: &Switches<'_>,
) -> std::result::Result<Vec<String>, String> {
    let mut ids = Vec::new();
    for entry in entries
        .iter()
        .map(|entry| entry.trim())
        .filter(|e| !e.is_empty())
    {
        // A name counts only switched-on servers; an id of a switched-off one is refused below.
        let server = match catalog.find_among(entry, |server| !switches.server_off(&server.id)) {
            Ok(server) => server,
            // Only switched-off servers have the name.
            Err(FindError::Unknown)
                if !matches!(catalog.find_among(entry, |_| true), Err(FindError::Unknown)) =>
            {
                return Err(switched_off_error(entry));
            }
            Err(FindError::Unknown) => {
                let mut known: Vec<&str> = catalog
                    .servers
                    .iter()
                    .filter(|server| !switches.server_off(&server.id))
                    .map(|server| server.name.as_str())
                    .collect();
                known.sort_unstable();
                known.dedup();
                return Err(format!(
                    "unknown integration `{entry}`: no MCP server of Claude Code, Codex, Claude Desktop or Cursor has that name or id here (known: {})",
                    if known.is_empty() {
                        "none".into()
                    } else {
                        known.join(", ")
                    }
                ));
            }
            Err(FindError::Ambiguous(ids)) => {
                return Err(format!(
                    "integration `{entry}` names several different MCP servers: give one of their ids instead ({})",
                    ids.join(", ")
                ));
            }
        };
        if RESERVED.contains(&server.name.as_str()) {
            return Err(format!(
                "integration `{entry}`: `{}` is the name of Brigadier's own server, which every worker has; the user's server of that name can't be given",
                server.name
            ));
        }
        if switches.server_off(&server.id) {
            return Err(switched_off_error(entry));
        }
        if !ids.contains(&server.id) {
            ids.push(server.id.clone());
        }
    }
    Ok(ids)
}

/// What a worker gets of the catalog besides its agent's own setup.
#[derive(Debug, Default)]
pub(crate) struct Shared {
    /// The named servers its agent doesn't have by itself.
    pub servers: Vec<McpServer>,
    /// Their environment and header values, for the redactor.
    pub secrets: Vec<String>,
    /// The plugin folder of skills its agent doesn't find by itself.
    pub skills: Option<PathBuf>,
    pub switched_off: SwitchedOff,
    /// Servers of [`Self::servers`] served through Brigadier's sign-in proxy, once their
    /// routes are made.
    pub proxied: Vec<Proxied>,
}

/// A server a worker reaches through the sign-in proxy.
#[derive(Debug)]
pub(crate) struct Proxied {
    /// Its place in [`Shared::servers`].
    pub index: usize,
    /// Where the sign-in that serves it is kept.
    pub place: Place,
    pub url: String,
    /// Its configured headers but `Authorization`.
    pub headers: Vec<(String, Secret)>,
    pub client: Option<OAuthClient>,
    pub homes: Homes,
}

/// What of its agent's own setup `session` goes without: what the user switched off.
pub(crate) fn switched_off(
    catalog: &Catalog,
    session: &Session<'_>,
    switches: &Switches<'_>,
) -> SwitchedOff {
    let mut off = SwitchedOff::default();
    for server in &catalog.servers {
        // A user's server under Brigadier's own name isn't one Brigadier gives: switching it
        // off must not take Brigadier's own away.
        if !switches.server_off(&server.id)
            || RESERVED.contains(&server.name.as_str())
            || !catalog.server_native(server, session)
        {
            continue;
        }
        match (&server.plugin, server.source) {
            (Some(plugin), Source::CodexPlugin) => {
                let entry = (plugin.id.clone(), server.name.clone());
                if !off.plugin_servers.contains(&entry) {
                    off.plugin_servers.push(entry);
                }
            }
            _ => {
                let name = server.native_name();
                if !off.servers.contains(&name) {
                    off.servers.push(name);
                }
            }
        }
    }
    for skill in &catalog.skills {
        if !switches.skill_off(&skill.id) || !catalog.skill_native(skill, session) {
            continue;
        }
        match session.agent {
            // Claude's own skills go by name, its plugins' by `<plugin>:<name>`.
            Agent::Claude => {
                for root in skill.roots.iter().filter(|root| root.native(session)) {
                    let (list, name) = match root {
                        SkillRoot::Plugin(plugin) => {
                            (&mut off.plugin_skills, plugin.skill_name(&skill.name))
                        }
                        _ => (&mut off.skill_names, skill.name.clone()),
                    };
                    if !list.contains(&name) {
                        list.push(name);
                    }
                }
            }
            // Codex knows a skill by the path it found it at; both forms are given. In a
            // checkout it finds the project's skills there, not in the project's own folder.
            Agent::Codex => {
                let in_checkout = session.project.filter(|_| session.in_checkout);
                for dir in skill.entries.iter().map(|(_, dir)| dir).chain([&skill.dir]) {
                    let moved = in_checkout
                        .and_then(|project| dir.strip_prefix(project).ok())
                        .map(|relative| session.cwd.join(relative));
                    for dir in [Some(dir.clone()), moved].into_iter().flatten() {
                        let file = dir.join("SKILL.md");
                        if !off.skill_files.contains(&file) {
                            off.skill_files.push(file);
                        }
                    }
                }
            }
        }
    }
    if session.agent == Agent::Codex && !off.skill_files.is_empty() {
        off.codex_skill_config = catalog.codex_skill_config.clone();
    }
    off
}

/// Where the sign-in that serves `server` to `session`'s agent is kept when it isn't the
/// agent's own (another agent's stored sign-in, or Brigadier's), with the configured client
/// of the entry it was made for, which its renewals need.
fn reused_sign_in(
    catalog: &Catalog,
    server: &Server,
    session: &Session<'_>,
    stores: &Prober<'_>,
) -> Option<(Place, Option<OAuthClient>)> {
    match catalog.sign_in_from(server, session, stores)? {
        (SignIn::ReusableFrom(agent), Some(entry)) => {
            Some((stores.place(agent.into(), entry)?, entry.oauth.clone()))
        }
        (SignIn::InBrigadier, _) => Some((
            stores.place(Owner::Brigadier, server)?,
            server.oauth.clone(),
        )),
        _ => None,
    }
}

/// Whether another sign-in than its agent's own could serve `server`, as far as is known
/// without asking the user: Brigadier holds one, or another agent's entry is the same server.
fn may_reuse(catalog: &Catalog, server: &Server, quiet: &Prober<'_>) -> bool {
    let id = identity(&server.transport);
    quiet.probe(Owner::Brigadier, server) == Probe::Usable
        || catalog.servers.iter().any(|other| {
            other.id != server.id
                && other.signs_in
                && owner_agent(other.source).is_some()
                && identity(&other.transport) == id
        })
}

/// `server` as a worker gets it under `name`; through the sign-in proxy with the sign-in at
/// `reused`'s place and its client, once its route is made.
fn give(
    shared: &mut Shared,
    name: String,
    server: &Server,
    reused: Option<(Place, Option<OAuthClient>, &Homes)>,
) {
    let (transport, env) = match &server.transport {
        Transport::Stdio {
            command, args, env, ..
        } => (
            McpTransport::Stdio {
                command: PathBuf::from(command),
                args: args.clone(),
                cwd: server.transport.working_dir().map(Path::to_owned),
            },
            env,
        ),
        Transport::Http { url, headers } => {
            if let Some((place, client, homes)) = reused {
                shared.proxied.push(Proxied {
                    index: shared.servers.len(),
                    place,
                    url: url.clone(),
                    headers: headers
                        .iter()
                        .filter(|(name, _)| !name.eq_ignore_ascii_case("authorization"))
                        .cloned()
                        .collect(),
                    client,
                    homes: homes.clone(),
                });
            }
            (McpTransport::Http { url: url.clone() }, headers)
        }
    };
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| (name.clone(), value.expose().to_owned()))
        .collect();
    shared.secrets.extend(
        env.iter()
            .map(|(_, value)| value.clone())
            .filter(|v| !v.is_empty()),
    );
    shared.servers.push(McpServer {
        name,
        transport,
        env,
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
        prompt_tools: Vec::new(),
    });
}

/// `name`, or the first `name-<n>` not in `taken`, then taken.
fn free_name(taken: &mut HashSet<String>, name: &str) -> String {
    let mut free = name.to_owned();
    let mut n = 2;
    while taken.contains(&free) {
        free = format!("{name}-{n}");
        n += 1;
    }
    taken.insert(free.clone());
    free
}

/// The servers `ids` name that `session`'s agent doesn't have by itself, as it gets them: a
/// name that is taken (by Brigadier's own, one of the agent's, or another named one) gets a
/// number. Values go in [`Shared::secrets`] too. A server that signs in goes through the
/// sign-in proxy when another stored sign-in serves it, and so does one of the agent's own
/// that its own sign-in doesn't serve: Codex gets it under its own name (the entry the thread's
/// config gives wins over the user's), and Claude, which keeps its own entry over a given one
/// of the same name, goes without its own and gets it under a new name. `quiet` reads never
/// ask the user; `asking` ones may.
fn named_servers(
    catalog: &Catalog,
    ids: &[String],
    session: &Session<'_>,
    switches: &Switches<'_>,
    shared: &mut Shared,
    quiet: &Prober<'_>,
    asking: &Prober<'_>,
) {
    let mut native: Vec<&Server> = catalog
        .servers
        .iter()
        .filter(|server| catalog.server_native(server, session))
        .collect();
    let mut taken: HashSet<String> = RESERVED.iter().map(|name| (*name).to_owned()).collect();
    taken.extend(native.iter().map(|server| server.native_name()));
    // A switched-off copy of its own is no copy it has: its name stays taken all the same.
    native.retain(|server| !switches.server_off(&server.id));
    for id in ids {
        let Some(server) = catalog.server(id) else {
            tracing::warn!(id, "a task's integration is no longer configured; left out");
            continue;
        };
        if switches.server_off(id) {
            tracing::info!(id, "a task's integration was switched off since; left out");
            continue;
        }
        // The same server under another source (one set up in both Claude apps) counts as
        // the agent's own.
        if native
            .iter()
            .any(|own| own.name == server.name && own.transport == server.transport)
        {
            continue;
        }
        let name = free_name(&mut taken, &server.name);
        let reused = server
            .signs_in
            .then(|| reused_sign_in(catalog, server, session, asking))
            .flatten();
        give(
            shared,
            name,
            server,
            reused.map(|(place, client)| (place, client, asking.homes())),
        );
    }
    let mut seen = HashSet::new();
    for server in native {
        if !server.signs_in
            || RESERVED.contains(&server.name.as_str())
            || !seen.insert(server.name.as_str())
            || !may_reuse(catalog, server, quiet)
        {
            continue;
        }
        let Some((place, client)) = reused_sign_in(catalog, server, session, asking) else {
            continue;
        };
        let name = match session.agent {
            Agent::Codex => server.name.clone(),
            Agent::Claude => {
                if !shared.switched_off.servers.contains(&server.name) {
                    shared.switched_off.servers.push(server.name.clone());
                }
                free_name(&mut taken, &server.name)
            }
        };
        give(shared, name, server, Some((place, client, asking.homes())));
    }
}

/// A plugin folder in `data_dir` with the switched-on skills `session`'s agent doesn't find by
/// itself, each linked to its folder; `None` when there are none. One folder per set of skills
/// (named by its hash), made once and never changed, so a worker running on it keeps it as is.
/// Two skills of one name are kept apart: the second is `<name>-2`, a folder of links to its
/// files with its `SKILL.md` renamed.
fn shared_skills(
    catalog: &Catalog,
    session: &Session<'_>,
    switches: &Switches<'_>,
    data_dir: &Path,
) -> std::io::Result<Option<PathBuf>> {
    let mut chosen = Vec::new();
    let mut names = HashSet::new();
    for skill in &catalog.skills {
        if switches.skill_off(&skill.id) || catalog.skill_native(skill, session) {
            continue;
        }
        let base = folder_name(&skill.name);
        let mut name = base.clone();
        let mut n = 2;
        while !names.insert(name.clone()) {
            name = format!("{base}-{n}");
            n += 1;
        }
        chosen.push((name, skill));
    }
    if chosen.is_empty() {
        return Ok(None);
    }
    // A renamed skill's folder holds a copy of its `SKILL.md` and links to the files it had:
    // a change to either is a new folder.
    let mut hasher = blake3::Hasher::new();
    for (name, skill) in &chosen {
        for part in [name.as_str(), &skill.dir.to_string_lossy(), &skill.name] {
            hasher.update(part.as_bytes());
            hasher.update(&[0]);
        }
        if *name != folder_name(&skill.name) {
            hasher.update(&std::fs::read(skill.skill_md()).unwrap_or_default());
            let mut files: Vec<_> = std::fs::read_dir(&skill.dir)
                .map(|entries| entries.flatten().map(|entry| entry.file_name()).collect())
                .unwrap_or_default();
            files.sort();
            for file in files {
                hasher.update(file.as_encoded_bytes());
                hasher.update(&[0]);
            }
        }
    }
    let agent = match session.agent {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
    };
    let root = data_dir.join("shared-skills");
    let plugin = root.join(format!("{agent}-{}", &hasher.finalize().to_hex()[..16]));
    if plugin.join(".claude-plugin/plugin.json").is_file() {
        return Ok(Some(plugin));
    }
    std::fs::create_dir_all(&root)?;
    let partial = root.join(format!(".partial-{}", uuid::Uuid::new_v4().simple()));
    let made = (|| {
        std::fs::create_dir_all(partial.join(".claude-plugin"))?;
        let manifest = serde_json::json!({
            "name": PLUGIN_NAME,
            "description": "The user's skills from their other agents, shared by Brigadier.",
            "version": env!("CARGO_PKG_VERSION"),
        });
        std::fs::write(
            partial.join(".claude-plugin/plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        )?;
        let skills = partial.join("skills");
        std::fs::create_dir_all(&skills)?;
        for (name, skill) in &chosen {
            if *name == folder_name(&skill.name) {
                link(&skill.dir, &skills.join(name))?;
            } else {
                renamed(skill, name, &skills.join(name))?;
            }
        }
        std::fs::rename(&partial, &plugin)
    })();
    if let Err(err) = made {
        let _ = std::fs::remove_dir_all(&partial);
        // Another worker made the same folder meanwhile.
        if !plugin.join(".claude-plugin/plugin.json").is_file() {
            return Err(err);
        }
    }
    Ok(Some(plugin))
}

/// A skill's name as a folder name.
fn folder_name(name: &str) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    if name.trim_matches('-').is_empty() {
        "skill".into()
    } else {
        name
    }
}

/// `skill` under the name `name` at `at`: its files linked, its `SKILL.md` a copy with the
/// frontmatter's name replaced.
fn renamed(skill: &brigadier_registry::Skill, name: &str, at: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(at)?;
    for entry in std::fs::read_dir(&skill.dir)?.flatten() {
        if entry.file_name() != "SKILL.md" {
            link(&entry.path(), &at.join(entry.file_name()))?;
        }
    }
    let text = std::fs::read_to_string(skill.skill_md())?;
    let mut done = false;
    let text: String = text
        .split_inclusive('\n')
        .map(|line| {
            if !done && line.trim_start().starts_with("name:") {
                done = true;
                format!("name: {name}\n")
            } else {
                line.to_owned()
            }
        })
        .collect();
    std::fs::write(at.join("SKILL.md"), text)
}

fn link(target: &Path, at: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, at);
    #[cfg(windows)]
    return if target.is_dir() {
        std::os::windows::fs::symlink_dir(target, at)
    } else {
        std::os::windows::fs::symlink_file(target, at)
    };
}

/// The thread's short list of the servers a worker gets only when named: every switched-on
/// server that isn't native to both Claude and Codex workers in `project`, by name with the
/// agents that have it. Empty when there are none.
pub(crate) fn thread_section(
    catalog: &Catalog,
    project: Option<&Path>,
    switches: &Switches<'_>,
) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for server in &catalog.servers {
        if switches.server_off(&server.id) || RESERVED.contains(&server.name.as_str()) {
            continue;
        }
        let native = server_native_to(catalog, server, project);
        if native.len() == 2 || !seen.insert(server.name.clone()) {
            continue;
        }
        // Agents that have a same-named copy of it count too; a different server of the name
        // is given by its id.
        let same: Vec<&Server> = catalog
            .servers
            .iter()
            .filter(|other| other.name == server.name && !switches.server_off(&other.id))
            .collect();
        let differs = same.iter().any(|other| other.transport != server.transport);
        let mut agents: Vec<&str> = Vec::new();
        for other in &same {
            for provider in server_native_to(catalog, other, project) {
                let label = match provider {
                    ProviderKind::Claude => "Claude",
                    ProviderKind::Codex => "Codex",
                };
                if !agents.contains(&label) {
                    agents.push(label);
                }
            }
        }
        if agents.len() == 2 && !differs {
            continue;
        }
        if differs {
            for other in &same {
                lines.push(format!("- `{}`: {}", other.id, other.source.label()));
            }
        } else if agents.is_empty() {
            lines.push(format!(
                "- {}: neither ({})",
                server.name,
                server.source.label()
            ));
        } else {
            lines.push(format!("- {}: {}", server.name, agents.join(", ")));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "MCP servers a worker gets only when its task names them (delegate_task's `integrations`, by name or by the id given): each worker already has its own agent's, and the skills of every agent. Name only what the task needs: Codex loads every tool of each server it gets. A server only one agent has, by name and the agents that have it:\n{}",
        lines.join("\n")
    )
}

/// `registry`'s catalog for sessions in `repo`, with the items of the projects at `others` that
/// the user uses in every project. A skill switched off by `switches` isn't taken, so it can't
/// stand in for a switched-on copy of it from another project. A Claude local-scope server is
/// its folder's own: sessions elsewhere get it only once the user uses it in every project.
fn catalog_for(
    registry: &Registry,
    repo: Option<&Path>,
    others: &[PathBuf],
    switches: &Switches<'_>,
) -> Catalog {
    let settings = switches.settings;
    let mut catalog = registry.catalog(repo);
    catalog.servers.retain(|server| {
        server.source != Source::ClaudeLocal
            || server.project.as_deref() == repo
            || settings.servers_everywhere.contains(&server.id)
    });
    for other in others {
        registry.adopt(
            &mut catalog,
            other,
            |id| settings.servers_everywhere.iter().any(|kept| kept == id),
            |id| {
                settings.skills_everywhere.iter().any(|kept| kept == id) && !switches.skill_off(id)
            },
        );
    }
    catalog
}

/// Settings' view of the catalog sessions in `repo` get ([`catalog_for`]), or without a `repo`
/// the user's, with every folder's local-scope servers.
fn settings_view(
    registry: &Registry,
    repo: Option<&Path>,
    others: &[PathBuf],
    switches: &Switches<'_>,
    auth: &McpAuth,
) -> IntegrationsView {
    // The user's catalog keeps every local-scope server, so the list finds it under its project.
    let catalog = match repo {
        Some(_) => catalog_for(registry, repo, others, switches),
        None => registry.catalog(None),
    };
    // Settings polls: its reads never ask the user.
    let stores = auth.prober(registry.roots(), catalog.codex_store, true);
    view(&catalog, repo, switches.settings, &stores)
}

impl SessionManager {
    /// The catalog of servers and skills, kept current by its watcher.
    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Tests: reads the catalog from `roots` instead. The flows that do run on Unix only.
    #[cfg(all(test, unix))]
    pub(crate) fn set_registry(&self, roots: Roots) {
        *self.registry.write().unwrap_or_else(|p| p.into_inner()) = Registry::new(roots);
    }

    /// The session's repository, while the user trusts it: the project whose setup its thread
    /// and workers load.
    pub(crate) fn trusted_repo(&self, id: &ConversationId) -> Option<PathBuf> {
        if self.repo_trust(id) != Some(true) {
            return None;
        }
        match &self.core.conversation(id).ok()?.setup {
            Some(crate::model::Setup::Session { repo, .. }) => Some(PathBuf::from(repo)),
            _ => None,
        }
    }

    /// The project session `id` is in.
    fn session_project(&self, id: &ConversationId) -> Option<ProjectId> {
        self.core.conversation(id).ok()?.project_id
    }

    /// The folders of the user's other projects, while trusted, that hold an item the user
    /// uses in every project ([`Settings::servers_everywhere`], [`Settings::skills_everywhere`]);
    /// `repo` is the session's.
    fn everywhere_repos(&self, settings: &Settings, repo: Option<&Path>) -> Vec<PathBuf> {
        if settings.servers_everywhere.is_empty() && settings.skills_everywhere.is_empty() {
            return Vec::new();
        }
        self.core
            .catalog()
            .projects
            .into_iter()
            .filter_map(|project| {
                let path = project.repos.first()?.path.clone();
                // An item's id holds its project's folder: `claude:project:<folder>:<name>`.
                let holds = |id: &String| id.contains(&format!(":{path}:"));
                (project.counts_trusted(&path)
                    && repo != Some(Path::new(&path))
                    && (settings.servers_everywhere.iter().any(holds)
                        || settings.skills_everywhere.iter().any(holds)))
                .then(|| PathBuf::from(path))
            })
            .collect()
    }

    /// Settings' catalog: the user's servers and skills, and once the user trusts `project`'s
    /// folder (its first repository), the catalog its sessions get: its own, with the other
    /// projects' items used everywhere and its exceptions.
    pub async fn integrations_view(&self, project: Option<ProjectId>) -> Result<IntegrationsView> {
        let repo = match &project {
            Some(id) => {
                let project = self.core.project(id)?;
                let repo = project
                    .repos
                    .first()
                    .map(|repo| repo.path.clone())
                    .ok_or_else(|| Error::Invalid("the project has no repository".into()))?;
                project.counts_trusted(&repo).then(|| PathBuf::from(repo))
            }
            None => None,
        };
        let (registry, settings, auth) =
            (self.registry(), self.core.settings(), self.mcp_auth.clone());
        let others = match &repo {
            Some(repo) => self.everywhere_repos(&settings, Some(repo)),
            None => Vec::new(),
        };
        let project = repo.as_ref().and(project);
        super::blocking(move || {
            let switches = Switches {
                settings: &settings,
                project: project.as_ref(),
            };
            Ok(settings_view(
                &registry,
                repo.as_deref(),
                &others,
                &switches,
                &auth,
            ))
        })
        .await
    }

    /// `project`'s first repository, when given: refused while the user doesn't trust its
    /// folder.
    fn trusted_project_repo(&self, project: Option<ProjectId>) -> Result<Option<PathBuf>> {
        let Some(id) = project else {
            return Ok(None);
        };
        let project = self.core.project(&id)?;
        let repo = project
            .repos
            .first()
            .map(|repo| repo.path.clone())
            .ok_or_else(|| Error::Invalid("the project has no repository".into()))?;
        if !project.counts_trusted(&repo) {
            return Err(Error::Invalid(
                "the project's folder isn't trusted: its servers aren't read".into(),
            ));
        }
        Ok(Some(PathBuf::from(repo)))
    }

    /// The server `server_id` that signs in with OAuth, of `project`'s catalog when given and
    /// trusted (else the user's), with that catalog.
    async fn signing_server(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
    ) -> Result<(Catalog, Server)> {
        let repo = self.trusted_project_repo(project)?;
        let registry = self.registry();
        let server_id = server_id.to_owned();
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let server = catalog
                .server(&server_id)
                .cloned()
                .ok_or_else(|| Error::Invalid(format!("no MCP server `{server_id}` here")))?;
            if !server.signs_in {
                return Err(Error::Invalid(format!(
                    "`{}` doesn't sign in with OAuth",
                    server.name
                )));
            }
            Ok((catalog, server))
        })
        .await
    }

    /// Where Brigadier keeps its own sign-in to `server`.
    fn own_place(server: &Server) -> Result<Place> {
        Place::of(Owner::Brigadier, server, CodexStore::default())
            .ok_or_else(|| Error::Invalid(format!("`{}` isn't a remote server", server.name)))
    }

    /// Starts a sign-in in Brigadier to the server `server_id` (of `project`'s catalog, when
    /// given and trusted), in the user's browser (`open`): its id at once, for
    /// [`Self::mcp_sign_in`]. The sign-in then serves every agent's workers. One already
    /// waiting for the server is kept: its page opens again and its id comes back.
    pub async fn sign_in_mcp_server(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
        open: Open,
    ) -> Result<String> {
        if !self.mcp_auth.keeps_sign_ins() {
            return Err(Error::Invalid(
                "Sign-ins can't be kept on this system: Brigadier keeps them in the macOS Keychain."
                    .into(),
            ));
        }
        let (_, server) = self.signing_server(server_id, project).await?;
        let homes = Homes::new(self.registry().roots());
        let auth = self.mcp_auth.clone();
        let (id, cancelled) = auth.begin_sign_in(Self::own_place(&server)?, open);
        if let Some(cancelled) = cancelled {
            let running = id.clone();
            self.spawn(async move {
                auth.sign_in(&running, &server, homes, cancelled).await;
            });
        }
        Ok(id)
    }

    /// How sign-in `id` ([`Self::sign_in_mcp_server`]) went.
    pub fn mcp_sign_in(&self, id: &str) -> Result<McpSignInStatus> {
        self.mcp_auth
            .sign_in_status(id)
            .ok_or_else(|| Error::Invalid(format!("no sign-in `{id}`")))
    }

    /// Stops sign-in `id` while it waits for the browser: nothing is kept, and it reads as
    /// cancelled.
    pub fn cancel_mcp_sign_in(&self, id: &str) -> Result<()> {
        self.mcp_auth.cancel_sign_in(id).map_err(Error::Invalid)
    }

    /// Opens sign-in `id`'s page in the browser again, while it waits.
    pub fn reopen_mcp_sign_in(&self, id: &str) -> Result<()> {
        self.mcp_auth.reopen_sign_in(id).map_err(Error::Invalid)
    }

    /// Deletes Brigadier's own sign-in to the server `server_id`, if it has one: never Claude's
    /// or Codex's.
    pub async fn sign_out_mcp_server(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
    ) -> Result<()> {
        let (_, server) = self.signing_server(server_id, project).await?;
        let homes = Homes::new(self.registry().roots());
        self.mcp_auth
            .sign_out(&Self::own_place(&server)?, homes)
            .await
            .map_err(Error::Invalid)
    }

    /// Reads the agents' stored sign-ins to the server `server_id` again, though the user
    /// refused it before: macOS asks them again. Returns at once; the catalog shows the
    /// outcome.
    pub async fn ask_mcp_sign_in_again(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
    ) -> Result<()> {
        let (catalog, server) = self.signing_server(server_id, project).await?;
        let (registry, auth) = (self.registry(), self.mcp_auth.clone());
        let id = identity(&server.transport);
        let entries: Vec<(Agent, Server)> = catalog
            .servers
            .into_iter()
            .filter(|entry| entry.signs_in && identity(&entry.transport) == id)
            .filter_map(|entry| Some((owner_agent(entry.source)?, entry)))
            .collect();
        let asking = auth.prober(registry.roots(), catalog.codex_store, false);
        let places: Vec<Place> = entries
            .iter()
            .filter_map(|(agent, entry)| asking.place((*agent).into(), entry))
            .collect();
        drop(asking);
        auth.ask_again(&places);
        let codex_store = catalog.codex_store;
        self.spawn(async move {
            let _ = super::blocking(move || {
                let asking = auth.prober(registry.roots(), codex_store, false);
                for (agent, entry) in &entries {
                    asking.probe((*agent).into(), entry);
                }
                Ok(())
            })
            .await;
        });
        Ok(())
    }

    /// The ids of the servers a task's `integrations` name, in the session's trusted project.
    pub(crate) async fn resolve_integrations(
        &self,
        id: &ConversationId,
        entries: &[String],
    ) -> Result<Vec<String>> {
        if entries.iter().all(|entry| entry.trim().is_empty()) {
            return Ok(Vec::new());
        }
        let (registry, settings, repo) =
            (self.registry(), self.core.settings(), self.trusted_repo(id));
        let (project, others) = (
            self.session_project(id),
            self.everywhere_repos(&settings, repo.as_deref()),
        );
        let entries = entries.to_vec();
        super::blocking(move || {
            let switches = Switches {
                settings: &settings,
                project: project.as_ref(),
            };
            let catalog = catalog_for(&registry, repo.as_deref(), &others, &switches);
            resolve(&catalog, &entries, &switches).map_err(Error::Invalid)
        })
        .await
    }

    /// What a worker of `provider` starting in `cwd` gets of the catalog: its task's named
    /// servers, the shared skills, and the switches. `checkout`: its worktree. `grant`: its
    /// key, the only one its routes through the sign-in proxy take.
    pub(crate) async fn worker_shared(
        &self,
        conversation: &ConversationId,
        ids: &[String],
        provider: ProviderKind,
        cwd: &Path,
        checkout: Option<&Path>,
        grant: &str,
    ) -> Shared {
        let (registry, settings, repo, auth) = (
            self.registry(),
            self.core.settings(),
            self.trusted_repo(conversation),
            self.mcp_auth.clone(),
        );
        let (project, others) = (
            self.session_project(conversation),
            self.everywhere_repos(&settings, repo.as_deref()),
        );
        let (ids, cwd, data_dir) = (ids.to_vec(), cwd.to_owned(), self.data_dir.clone());
        let in_checkout = checkout == Some(cwd.as_path());
        let has_checkout = checkout.is_some();
        let made = super::blocking(move || {
            // A task can name a server of the project, but a worker without a checkout starts
            // without the project's setup.
            let switches = Switches {
                settings: &settings,
                project: project.as_ref(),
            };
            let catalog = catalog_for(&registry, repo.as_deref(), &others, &switches);
            let session = Session {
                agent: agent(provider),
                cwd: &cwd,
                project: worker_project(repo.as_deref(), has_checkout),
                in_checkout,
            };
            let mut shared = Shared {
                switched_off: switched_off(&catalog, &session, &switches),
                ..Shared::default()
            };
            let quiet = auth.prober(registry.roots(), catalog.codex_store, true);
            let asking = auth.prober(registry.roots(), catalog.codex_store, false);
            named_servers(
                &catalog,
                &ids,
                &session,
                &switches,
                &mut shared,
                &quiet,
                &asking,
            );
            shared.skills =
                shared_skills(&catalog, &session, &switches, &data_dir).unwrap_or_else(|err| {
                    tracing::warn!(error = %err, "the shared skills' folder couldn't be made");
                    None
                });
            Ok(shared)
        })
        .await;
        let mut shared = made.unwrap_or_default();
        for proxied in std::mem::take(&mut shared.proxied) {
            let route = Route {
                grant: grant.to_owned(),
                place: proxied.place,
                url: proxied.url,
                headers: proxied.headers,
                client: proxied.client,
                homes: proxied.homes,
            };
            match self.mcp_auth.route(route).await {
                Ok(url) => {
                    let server = &mut shared.servers[proxied.index];
                    server.transport = McpTransport::Http { url };
                    server.env = vec![("Authorization".into(), format!("Bearer {grant}"))];
                }
                Err(reason) => tracing::warn!(
                    server = %shared.servers[proxied.index].name,
                    %reason,
                    "a reused MCP sign-in can't be served: the server goes as configured"
                ),
            }
        }
        shared
    }

    /// What a thread of `provider` starting in `cwd` goes without of its agent's own setup.
    pub(crate) async fn thread_switched_off(
        &self,
        conversation: &ConversationId,
        provider: ProviderKind,
        cwd: &Path,
    ) -> SwitchedOff {
        let (registry, settings, repo) = (
            self.registry(),
            self.core.settings(),
            self.trusted_repo(conversation),
        );
        let project = self.session_project(conversation);
        // A thread runs in its workspace, a checkout of the project.
        let in_checkout = self
            .recorded_workspace(conversation)
            .is_some_and(|workspace| workspace.path == cwd);
        let cwd = cwd.to_owned();
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let session = Session {
                agent: agent(provider),
                cwd: &cwd,
                project: repo.as_deref(),
                in_checkout,
            };
            let switches = Switches {
                settings: &settings,
                project: project.as_ref(),
            };
            Ok(switched_off(&catalog, &session, &switches))
        })
        .await
        .unwrap_or_default()
    }

    /// The thread's list of servers a worker gets only when named ([`thread_section`]).
    pub(crate) async fn thread_integrations(&self, conversation: &ConversationId) -> String {
        let (registry, settings, repo) = (
            self.registry(),
            self.core.settings(),
            self.trusted_repo(conversation),
        );
        let (project, others) = (
            self.session_project(conversation),
            self.everywhere_repos(&settings, repo.as_deref()),
        );
        super::blocking(move || {
            let switches = Switches {
                settings: &settings,
                project: project.as_ref(),
            };
            let catalog = catalog_for(&registry, repo.as_deref(), &others, &switches);
            Ok(thread_section(&catalog, repo.as_deref(), &switches))
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::mcp_auth::{McpAuth, Memory};

    fn home() -> PathBuf {
        let home =
            std::env::temp_dir().join(format!("integrations-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&home).unwrap();
        home
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn roots(home: &Path) -> Roots {
        Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap()
    }

    #[test]
    fn resolve_refuses_a_switched_off_name_and_prefers_a_switched_on_copy() {
        let home = home();
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers": {"railway": {"command": "npx", "args": ["railway"]},
                               "lin": {"command": "lin-claude"}}}"#,
        );
        write(
            &home.join(".codex/config.toml"),
            "[mcp_servers.lin]\ncommand = \"lin-codex\"\n",
        );
        let catalog = Catalog::read(&roots(&home), None);
        let settings = Settings {
            mcp_servers_off: vec!["claude:user:railway".into(), "claude:user:lin".into()],
            ..Settings::default()
        };
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let resolve = |entry: &str| resolve(&catalog, &[entry.to_owned()], &switches);

        // The only server of the name is switched off.
        let err = resolve("railway").unwrap_err();
        assert!(err.contains("is switched off for Brigadier"), "{err}");
        // Of two different servers of one name, only the switched-on one counts.
        assert_eq!(resolve("lin").unwrap(), ["codex:user:lin"]);
        let err = resolve("claude:user:lin").unwrap_err();
        assert!(err.contains("is switched off for Brigadier"), "{err}");
        let err = resolve("nowhere").unwrap_err();
        assert!(err.contains("unknown integration"), "{err}");
    }

    #[test]
    fn a_project_turns_a_default_off_or_on_for_itself_only() {
        let home = home();
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers": {"railway": {"command": "railway"}, "pencil": {"command": "pencil"}}}"#,
        );
        let catalog = Catalog::read(&roots(&home), None);
        let (here, there) = (ProjectId("here".into()), ProjectId("there".into()));
        let settings = Settings {
            mcp_servers_off: vec!["claude:user:railway".into()],
            project_switches: vec![crate::model::ProjectSwitches {
                project_id: here.clone(),
                servers_on: vec!["claude:user:railway".into()],
                servers_off: vec!["claude:user:pencil".into()],
                skills_on: Vec::new(),
                skills_off: Vec::new(),
            }],
            ..Settings::default()
        };
        let resolve = |project: &ProjectId, entry: &str| {
            let switches = Switches {
                settings: &settings,
                project: Some(project),
            };
            resolve(&catalog, &[entry.to_owned()], &switches)
        };
        assert_eq!(resolve(&here, "railway").unwrap(), ["claude:user:railway"]);
        let err = resolve(&here, "pencil").unwrap_err();
        assert!(err.contains("is switched off for Brigadier"), "{err}");
        // Another project goes by the defaults.
        let err = resolve(&there, "railway").unwrap_err();
        assert!(err.contains("is switched off for Brigadier"), "{err}");
        assert_eq!(resolve(&there, "pencil").unwrap(), ["claude:user:pencil"]);
        assert!(settings.server_off("claude:user:railway", None));
        assert!(!settings.server_off("claude:user:pencil", None));
    }

    #[test]
    fn another_projects_item_used_everywhere_reaches_workers_of_this_one() {
        let home = home();
        let (repo, other) = (home.join("repo"), home.join("steady"));
        write(
            &other.join(".mcp.json"),
            // A server and a skill of one name share an id: the skill used everywhere doesn't
            // take the server along.
            r#"{"mcpServers": {"steady-api": {"command": "steady"}, "steady-db": {"command": "db"},
                               "deploy": {"command": "deploy"}}}"#,
        );
        write(
            &other.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys.\n---\n",
        );
        write(
            &other.join(".claude/skills/seed/SKILL.md"),
            "---\nname: seed\ndescription: Seeds.\n---\n",
        );
        std::fs::create_dir_all(&repo).unwrap();
        let registry = Registry::new(roots(&home));
        let theirs = Catalog::read(&roots(&home), Some(&other));
        let id = |name: &str| {
            theirs
                .servers
                .iter()
                .map(|server| (&server.name, &server.id))
                .chain(theirs.skills.iter().map(|skill| (&skill.name, &skill.id)))
                .find(|(found, _)| *found == name)
                .unwrap()
                .1
                .clone()
        };
        let settings = Settings {
            servers_everywhere: vec![id("steady-api")],
            skills_everywhere: vec![id("deploy")],
            ..Settings::default()
        };
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let catalog = catalog_for(
            &registry,
            Some(&repo),
            std::slice::from_ref(&other),
            &switches,
        );
        let names = |found: Vec<&String>| found.into_iter().cloned().collect::<Vec<_>>();
        assert_eq!(
            names(catalog.servers.iter().map(|server| &server.name).collect()),
            ["steady-api"]
        );
        assert_eq!(
            names(catalog.skills.iter().map(|skill| &skill.name).collect()),
            ["deploy"]
        );
        // Not its own project's, so a worker here gets the server only when named, and the
        // skill from Brigadier.
        let session = Session {
            agent: Agent::Claude,
            cwd: &repo,
            project: Some(&repo),
            in_checkout: true,
        };
        let server = catalog.server(&id("steady-api")).unwrap();
        assert!(!catalog.server_native(server, &session));
        assert!(!catalog.skill_native(&catalog.skills[0], &session));
        assert_eq!(
            resolve(&catalog, &["steady-api".into()], &switches).unwrap(),
            [id("steady-api")]
        );
        let data = home.join("data");
        let plugin = shared_skills(&catalog, &session, &switches, &data)
            .unwrap()
            .expect("the shared skills");
        assert!(plugin.join("skills/deploy/SKILL.md").is_file());
        assert!(!plugin.join("skills/seed").exists());

        // This project has the same skill, file for file: one skill, which its workers find.
        write(
            &repo.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys.\n---\n",
        );
        let catalog = catalog_for(
            &registry,
            Some(&repo),
            std::slice::from_ref(&other),
            &switches,
        );
        assert_eq!(catalog.skills.len(), 1);
        assert!(catalog.skill_native(&catalog.skills[0], &session));

        // Two other projects use the same skill everywhere, the first switched off: the
        // second's copy is given.
        let third = home.join("third");
        write(
            &third.join(".claude/skills/deploy/SKILL.md"),
            "---\nname: deploy\ndescription: Deploys.\n---\n",
        );
        let thirds = Catalog::read(&roots(&home), Some(&third));
        let theirs_id = thirds.skills[0].id.clone();
        let settings = Settings {
            skills_everywhere: vec![id("deploy"), theirs_id.clone()],
            skills_off: vec![id("deploy")],
            ..Settings::default()
        };
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let elsewhere = home.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let catalog = catalog_for(
            &registry,
            Some(&elsewhere),
            &[other.clone(), third.clone()],
            &switches,
        );
        let ids: Vec<&String> = catalog.skills.iter().map(|skill| &skill.id).collect();
        assert_eq!(ids, [&theirs_id]);
    }

    #[test]
    fn a_projects_settings_view_is_the_catalog_its_sessions_get() {
        let home = home();
        let (repo, other) = (home.join("repo"), home.join("steady"));
        write(
            &other.join(".mcp.json"),
            r#"{"mcpServers": {"steady-api": {"command": "steady"}}}"#,
        );
        let skill = "---\nname: deploy\ndescription: Deploys.\n---\n";
        write(&other.join(".claude/skills/deploy/SKILL.md"), skill);
        // This project has the same skill, file for file.
        write(&repo.join(".claude/skills/deploy/SKILL.md"), skill);
        let registry = Registry::new(roots(&home));
        let theirs = Catalog::read(&roots(&home), Some(&other));
        let settings = Settings {
            servers_everywhere: vec![theirs.servers[0].id.clone()],
            skills_everywhere: vec![theirs.skills[0].id.clone()],
            ..Settings::default()
        };
        let project = ProjectId("repo".into());
        let switches = Switches {
            settings: &settings,
            project: Some(&project),
        };
        let auth = McpAuth::new(Arc::new(Memory::default()), Arc::new(|_: &str| false));
        let view = settings_view(
            &registry,
            Some(&repo),
            std::slice::from_ref(&other),
            &switches,
            &auth,
        );
        // The other project's server is there, but Claude in this project doesn't load it.
        let server = view
            .servers
            .iter()
            .find(|server| server.name == "steady-api")
            .expect("the server used everywhere");
        assert!(server.native_to.is_empty());
        // The identical skill is one, this project's own copy, which its workers find.
        let deploys: Vec<&SkillView> = view
            .skills
            .iter()
            .filter(|skill| skill.name == "deploy")
            .collect();
        assert_eq!(deploys.len(), 1);
        assert_ne!(deploys[0].id, theirs.skills[0].id);
        assert!(deploys[0].native_to.contains(&ProviderKind::Claude));
        // The user's view alone doesn't take it in.
        let user = settings_view(&registry, None, &[], &switches, &auth);
        assert!(
            user.servers
                .iter()
                .all(|server| server.name != "steady-api")
        );
    }

    #[test]
    fn a_local_scope_server_stays_in_its_folder_until_used_everywhere() {
        let home = home();
        let (a, b) = (home.join("a"), home.join("b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        write(
            &home.join(".claude.json"),
            &format!(
                r#"{{"projects": {{"{}": {{"mcpServers": {{"local-db": {{"command": "db"}}}}}}}}}}"#,
                a.display()
            ),
        );
        let registry = Registry::new(roots(&home));
        let id = registry.catalog(None).find("local-db").unwrap().id.clone();
        let named = |settings: &Settings, repo: &Path| {
            let switches = Switches {
                settings,
                project: None,
            };
            let catalog = catalog_for(&registry, Some(repo), &[], &switches);
            resolve(&catalog, &["local-db".into()], &switches)
        };
        let settings = Settings::default();
        assert_eq!(named(&settings, &a).unwrap(), std::slice::from_ref(&id));
        assert!(named(&settings, &b).is_err());
        let everywhere = Settings {
            servers_everywhere: vec![id.clone()],
            ..Settings::default()
        };
        assert_eq!(named(&everywhere, &b).unwrap(), std::slice::from_ref(&id));
        // Settings' list still finds it, under its folder.
        let auth = McpAuth::new(Arc::new(Memory::default()), Arc::new(|_: &str| false));
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let user = settings_view(&registry, None, &[], &switches, &auth);
        assert!(user.servers.iter().any(|server| server.id == id));
        let in_b = settings_view(&registry, Some(&b), &[], &switches, &auth);
        assert!(in_b.servers.iter().all(|server| server.id != id));
    }

    #[test]
    fn a_worker_without_a_checkout_is_given_its_projects_servers() {
        let home = home();
        let repo = home.join("repo");
        write(
            &repo.join(".mcp.json"),
            r#"{"mcpServers": {"db": {"command": "db-server"}}}"#,
        );
        let roots = roots(&home);
        let catalog = Catalog::read(&roots, Some(&repo));
        let id = catalog
            .servers
            .iter()
            .find(|server| server.name == "db")
            .expect("the project's server")
            .id
            .clone();
        assert_eq!(worker_project(Some(&repo), false), None);
        assert_eq!(worker_project(Some(&repo), true), Some(repo.as_path()));
        let auth = McpAuth::new(Arc::new(Memory::default()), Arc::new(|_: &str| false));
        let given = |has_checkout| {
            let session = Session {
                agent: Agent::Claude,
                cwd: &repo,
                project: worker_project(Some(&repo), has_checkout),
                in_checkout: has_checkout,
            };
            let mut shared = Shared::default();
            let quiet = auth.prober(&roots, catalog.codex_store, true);
            let asking = auth.prober(&roots, catalog.codex_store, false);
            named_servers(
                &catalog,
                std::slice::from_ref(&id),
                &session,
                &Switches {
                    settings: &Settings::default(),
                    project: None,
                },
                &mut shared,
                &quiet,
                &asking,
            );
            shared
                .servers
                .into_iter()
                .map(|server| server.name)
                .collect::<Vec<_>>()
        };
        // Its CLI starts outside the project, so the project's server is given.
        assert_eq!(given(false), ["db"]);
        // In a checkout its CLI loads it by itself.
        assert!(given(true).is_empty());
    }

    #[test]
    fn a_reused_sign_in_renews_with_the_client_of_the_entry_it_was_made_for() {
        let home = home();
        let url = "https://mcp.example.com/mcp";
        write(
            &home.join(".claude.json"),
            &format!(r#"{{"mcpServers": {{"linear": {{"type": "http", "url": "{url}"}}}}}}"#),
        );
        write(
            &home.join(".codex/config.toml"),
            &format!(
                "mcp_oauth_credentials_store = \"file\"\n\n[mcp_servers.linear]\nurl = \"{url}\"\n\n[mcp_servers.linear.oauth]\nclient_id = \"codex-client\"\nclient_secret = \"codex-secret\"\n"
            ),
        );
        let roots = roots(&home);
        let catalog = Catalog::read(&roots, None);
        let codex = catalog.server("codex:user:linear").unwrap();
        let key = brigadier_registry::codex_keys(codex)[0].clone();
        write(
            &home.join(".codex/.credentials.json"),
            &format!(
                r#"{{"{key}": {{"server_name": "linear", "server_url": "{url}", "client_id": "codex-client",
                    "access_token": "codex-access", "expires_at": 32503680000000, "refresh_token": "codex-refresh"}}}}"#
            ),
        );
        let auth = McpAuth::new(Arc::new(Memory::default()), Arc::new(|_: &str| false));
        let asking = auth.prober(&roots, catalog.codex_store, false);
        let session = Session {
            agent: Agent::Claude,
            cwd: &home,
            project: None,
            in_checkout: false,
        };
        let claude = catalog.server("claude:user:linear").unwrap();
        let (place, client) = reused_sign_in(&catalog, claude, &session, &asking).unwrap();
        assert!(matches!(place, Place::Codex { .. }));
        // Codex keeps its configured client secret in its config, not with its tokens.
        let client = client.expect("Codex's configured client");
        assert_eq!(client.client_id.as_deref(), Some("codex-client"));
        assert_eq!(client.client_secret.unwrap().expose(), "codex-secret");
    }

    #[test]
    fn the_copy_key_tells_values_apart_and_holds_none() {
        let stdio = |value: &str| Transport::Stdio {
            command: "npx".into(),
            args: vec!["server".into()],
            env: vec![("API_KEY".into(), Secret::new(value))],
            cwd: None,
            base: None,
        };
        let http = |value: &str| Transport::Http {
            url: "https://mcp.example.com/mcp".into(),
            headers: vec![("X-Tenant".into(), Secret::new(value))],
        };
        assert_eq!(copy_key(&stdio("sk-one")), copy_key(&stdio("sk-one")));
        assert_ne!(copy_key(&stdio("sk-one")), copy_key(&stdio("sk-two")));
        assert_eq!(copy_key(&http("acme")), copy_key(&http("acme")));
        assert_ne!(copy_key(&http("acme")), copy_key(&http("globex")));
        assert_ne!(copy_key(&stdio("acme")), copy_key(&http("acme")));
        for value in ["sk-one", "acme"] {
            assert!(!copy_key(&stdio(value)).contains(value));
            assert!(!copy_key(&http(value)).contains(value));
        }
    }

    fn skill(dir: &Path, name: &str) {
        write(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: d\n---\n"),
        );
    }

    /// A Claude plugin and a Codex plugin, each with a skill and a server, switched off for
    /// Brigadier: each agent's own switch takes them away; the view names the plugins.
    #[test]
    fn plugin_skills_and_servers_switch_off_by_their_agents_names() {
        let home = std::env::temp_dir().join(format!("brigadier-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let home = {
            std::fs::create_dir_all(&home).unwrap();
            std::fs::canonicalize(&home).unwrap()
        };
        let claude = home.join(".claude/plugins/cache/market/docs/1.0.0");
        write(
            &claude.join(".claude-plugin/plugin.json"),
            r#"{"name": "docs"}"#,
        );
        skill(&claude.join("skills/write"), "write");
        write(
            &claude.join(".mcp.json"),
            r#"{"mcpServers": {"server": {"command": "s"}}}"#,
        );
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &format!(
                r#"{{"plugins": {{"docs@market": [{{"scope": "user", "installPath": "{}"}}]}}}}"#,
                claude.display()
            ),
        );
        let codex = home.join(".codex/plugins/cache/openai/sheets/2.0.0");
        write(
            &codex.join(".codex-plugin/plugin.json"),
            r#"{"name": "sheets"}"#,
        );
        skill(&codex.join("skills/chart"), "chart");
        write(
            &codex.join(".mcp.json"),
            r#"{"mcpServers": {"calc": {"command": "c"}}}"#,
        );
        write(
            &home.join(".codex/config.toml"),
            "[plugins.\"sheets@openai\"]\nenabled = true\n",
        );
        let roots = Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap();
        let catalog = Catalog::read(&roots, None);
        let settings = Settings {
            skills_off: catalog
                .skills
                .iter()
                .map(|skill| skill.id.clone())
                .collect(),
            mcp_servers_off: catalog
                .servers
                .iter()
                .map(|server| server.id.clone())
                .collect(),
            ..Settings::default()
        };
        assert_eq!(settings.skills_off.len(), 2, "{settings:?}");
        assert_eq!(settings.mcp_servers_off.len(), 2, "{settings:?}");
        let session = |agent| Session {
            agent,
            cwd: &home,
            project: None,
            in_checkout: false,
        };
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let off = switched_off(&catalog, &session(Agent::Claude), &switches);
        assert_eq!(off.plugin_skills, ["docs:write"]);
        assert!(off.skill_names.is_empty());
        assert_eq!(off.servers, ["plugin_docs_server"]);
        let off = switched_off(&catalog, &session(Agent::Codex), &switches);
        assert_eq!(
            off.plugin_servers,
            [("sheets@openai".to_owned(), "calc".to_owned())]
        );
        assert!(off.servers.is_empty());
        assert_eq!(off.skill_files, [codex.join("skills/chart/SKILL.md")]);

        let auth = McpAuth::new(Arc::new(Memory::default()), Arc::new(|_: &str| false));
        let stores = auth.prober(&roots, catalog.codex_store, true);
        let view = view(&catalog, None, &Settings::default(), &stores);
        let chart = view
            .skills
            .iter()
            .find(|skill| skill.name == "chart")
            .unwrap();
        assert_eq!(chart.plugin.as_deref(), Some("sheets"));
        assert_eq!(chart.native_to, [ProviderKind::Codex]);
        assert_eq!(chart.locations[0].plugin.as_deref(), Some("sheets@openai"));
        let write = view
            .skills
            .iter()
            .find(|skill| skill.name == "write")
            .unwrap();
        assert!(!write.claude_off_by_name);
        let server = view
            .servers
            .iter()
            .find(|server| server.name == "server")
            .unwrap();
        assert_eq!(server.source, "Claude Code (plugin)");
        assert_eq!(server.plugin.as_deref(), Some("docs@market"));
        assert_eq!(server.file, claude.join(".mcp.json").display().to_string());
        let _ = std::fs::remove_dir_all(&home);
    }
}
