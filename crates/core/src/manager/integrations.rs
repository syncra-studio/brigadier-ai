//! MCP servers and skills shared between the agents (PLAN.md §6 Phase 7), from the catalog
//! `brigadier_registry` reads of every agent's own configuration:
//!
//! - Each thread and worker keeps its own agent's servers and skills, as its CLI loads them.
//! - What the user switched off for Brigadier (Settings' `mcpServersOff` and `skillsOff`, by
//!   catalog id) goes to no thread or worker, their agent's own included
//!   (docs/research/phase7-probes.md). A change takes effect at each session's next start or
//!   resume.
//!
//! Values of a server's environment and headers go only into the CLI's environment (or a Codex
//! thread's config, which stays off disk) and the redactor: the view shows their names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use brigadier_providers::{ProviderKind, SwitchedOff};
use brigadier_registry::adapter::adapters;
use brigadier_registry::{
    Agent, Catalog, Registry, Roots, Secret, Server, Session, SkillRoot, Source, Transport,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::SessionManager;
use super::mcp_login::{Clis, McpSignInProgress, McpSignInStatus, Open, Target};
use crate::model::{ConversationId, ProjectId, Settings};
use crate::{Error, Result};

/// Brigadier's own servers' names, which no shared server may take.
const RESERVED: &[&str] = &[super::computer_use::SERVER_NAME];

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
    /// The name its tools are called by.
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
    /// A remote server that signs in with OAuth: whether each agent that has it is signed in,
    /// as its CLI's own list says. `None`: it doesn't sign in.
    pub sign_in: Option<SignInView>,
    /// The sign-in to it going on, if one is: its id, as `SignInMcpServer` gave it.
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

/// Whether each agent that has a remote server is signed in to it; `None` for one that doesn't
/// have it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SignInView {
    pub claude: Option<SignInState>,
    pub codex: Option<SignInState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SignInState {
    /// Its CLI's own list says it is signed in (or needs none).
    SignedIn,
    /// Its CLI's own list says it needs a sign-in.
    Needed,
    /// Not known yet: its CLI's list is being read.
    Unknown,
}

fn provider(agent: Agent) -> ProviderKind {
    super::mcp_login::provider(agent)
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

/// Every agent, in the adapters' order.
fn agents() -> impl Iterator<Item = ProviderKind> {
    adapters()
        .into_iter()
        .map(|adapter| provider(adapter.agent()))
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

/// The agents whose workers in `project` have `server` by themselves.
fn server_native_to(
    catalog: &Catalog,
    server: &Server,
    project: Option<&Path>,
) -> Vec<ProviderKind> {
    agents()
        .filter(|provider| catalog.server_native(server, &worker_session(*provider, project)))
        .collect()
}

/// How a server's sign-ins stand, for the view: per agent, and the sign-in going on.
type SignIns<'a> = &'a dyn Fn(&Server) -> (Option<SignInView>, Option<String>);

/// Settings' view of `catalog`, read for `project`, with each remote server's sign-ins.
pub(crate) fn view(
    catalog: &Catalog,
    project: Option<&Path>,
    settings: &Settings,
    sign_ins: SignIns<'_>,
) -> IntegrationsView {
    let names = |pairs: &[(String, brigadier_registry::Secret)]| {
        pairs.iter().map(|(name, _)| name.clone()).collect()
    };
    let servers = catalog
        .servers
        .iter()
        .map(|server| {
            let (sign_in, pending_sign_in) = if server.signs_in {
                sign_ins(server)
            } else {
                (None, None)
            };
            McpServerView {
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
                sign_in,
                pending_sign_in,
                native_to: server_native_to(catalog, server, project),
                off: settings.mcp_servers_off.contains(&server.id),
                copy_key: copy_key(&server.transport),
            }
        })
        .collect();
    let skills = catalog
        .skills
        .iter()
        .map(|skill| {
            let native_to: Vec<ProviderKind> = agents()
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
                        native_to: agents()
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

/// What of its agent's own setup `session` goes without: what the user switched off.
pub(crate) fn switched_off(
    catalog: &Catalog,
    session: &Session<'_>,
    switches: &Switches<'_>,
) -> SwitchedOff {
    switched_off_by(catalog, session, &|id| switches.server_off(id), &|skill| {
        switches.skill_off(&skill.id)
    })
}

/// What of its agent's own setup a lean worker (`brigadier start --lean`, G23) goes without:
/// every MCP server, and every skill but those `keep` names.
pub(crate) fn lean_switched_off(
    catalog: &Catalog,
    session: &Session<'_>,
    keep: &[String],
) -> SwitchedOff {
    switched_off_by(catalog, session, &|_| true, &|skill| {
        !keep
            .iter()
            .any(|name| *name == skill.name || *name == skill.id)
    })
}

fn switched_off_by(
    catalog: &Catalog,
    session: &Session<'_>,
    server_off: &dyn Fn(&str) -> bool,
    skill_off: &dyn Fn(&brigadier_registry::Skill) -> bool,
) -> SwitchedOff {
    let mut off = SwitchedOff::default();
    for server in &catalog.servers {
        // A user's server under Brigadier's own name isn't one Brigadier gives: switching it
        // off must not take Brigadier's own away.
        if !server_off(&server.id)
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
        if !skill_off(skill) || !catalog.skill_native(skill, session) {
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
    sign_ins: SignIns<'_>,
) -> IntegrationsView {
    // The user's catalog keeps every local-scope server, so the list finds it under its project.
    let catalog = match repo {
        Some(_) => catalog_for(registry, repo, others, switches),
        None => registry.catalog(None),
    };
    view(&catalog, repo, switches.settings, sign_ins)
}

/// The sign-in target of `server`, listed in `project`.
fn target(server: &Server, project: Option<&Path>) -> Target {
    Target {
        server: server.name.clone(),
        plugin: server.plugin.as_ref().map(|plugin| plugin.name.clone()),
        project: project.map(Path::to_owned),
    }
}

impl SessionManager {
    /// The catalog of servers and skills, kept current by its watcher.
    pub(crate) fn registry(&self) -> Arc<Registry> {
        self.registry
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
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
        let (registry, settings, logins, clis) = (
            self.registry(),
            self.core.settings(),
            self.mcp_logins.clone(),
            self.clis(),
        );
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
            // Each CLI's own list, as last read (Settings polls; a stale one is read again in
            // the background).
            let sign_ins = |server: &Server| {
                let target = target(server, repo.as_deref());
                let state = |agent: Agent| {
                    let adapter = brigadier_registry::adapter::adapter(agent);
                    let Some(listed) = logins.listed(&clis, agent, repo.as_deref()) else {
                        return Some(SignInState::Unknown);
                    };
                    let plugin = target.plugin.as_deref();
                    listed
                        .iter()
                        .find(|entry| adapter.is_server(&entry.name, &target.server, plugin))
                        .map(|entry| {
                            if entry.needs_sign_in {
                                SignInState::Needed
                            } else {
                                SignInState::SignedIn
                            }
                        })
                };
                (
                    Some(SignInView {
                        claude: state(Agent::Claude),
                        codex: state(Agent::Codex),
                    }),
                    logins.pending(&target),
                )
            };
            Ok(settings_view(
                &registry,
                repo.as_deref(),
                &others,
                &switches,
                &sign_ins,
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

    /// How the agents' CLIs are run for their MCP commands: the login shell's environment and
    /// each one's binary, the user's choice included.
    pub(crate) fn clis(&self) -> Clis {
        let env = self.runtime.cli_env().clone();
        Clis {
            env: env
                .vars()
                .into_iter()
                .filter_map(|(name, value)| {
                    Some((name.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
            binary: Arc::new(move |agent| env.resolve(provider(agent))),
        }
    }

    /// The server `server_id` that signs in with OAuth, of `project`'s catalog when given and
    /// trusted (else the user's), as a sign-in target.
    async fn signing_server(&self, server_id: &str, project: Option<ProjectId>) -> Result<Target> {
        let repo = self.trusted_project_repo(project)?;
        let registry = self.registry();
        let server_id = server_id.to_owned();
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let server = catalog
                .server(&server_id)
                .ok_or_else(|| Error::Invalid(format!("no MCP server `{server_id}` here")))?;
            if !server.signs_in {
                return Err(Error::Invalid(format!(
                    "`{}` doesn't sign in with OAuth",
                    server.name
                )));
            }
            Ok(target(server, repo.as_deref()))
        })
        .await
    }

    /// Signs every agent that has the server `server_id` (of `project`'s catalog, when given
    /// and trusted) and needs it in to it, one after another, each with its CLI's own `mcp
    /// login`; each sign-in page opens in the user's browser (`open`). Its id at once, for
    /// [`Self::mcp_sign_in`]. One already going for the server is kept: its page opens again
    /// and its id comes back.
    pub async fn sign_in_mcp_server(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
        open: Open,
    ) -> Result<String> {
        let target = self.signing_server(server_id, project).await?;
        Ok(self.mcp_logins.start(self.clis(), target, open))
    }

    /// How sign-in `id` ([`Self::sign_in_mcp_server`]) went, all agents at once.
    pub fn mcp_sign_in(&self, id: &str) -> Result<McpSignInStatus> {
        Ok(self.mcp_sign_in_progress(id)?.status())
    }

    /// How sign-in `id` goes, agent by agent.
    pub fn mcp_sign_in_progress(&self, id: &str) -> Result<McpSignInProgress> {
        self.mcp_logins
            .progress(id)
            .ok_or_else(|| Error::Invalid(format!("no sign-in `{id}`")))
    }

    /// Stops sign-in `id`: the agent signing in now is stopped, the ones after it skipped.
    pub fn cancel_mcp_sign_in(&self, id: &str) -> Result<()> {
        self.mcp_logins.cancel(id).map_err(Error::Invalid)
    }

    /// Opens sign-in `id`'s current page in the browser again.
    pub fn reopen_mcp_sign_in(&self, id: &str) -> Result<()> {
        self.mcp_logins.reopen(id).map_err(Error::Invalid)
    }

    /// Signs every agent that has the server `server_id` out of it, with its CLI's own `mcp
    /// logout`.
    pub async fn sign_out_mcp_server(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
    ) -> Result<()> {
        let target = self.signing_server(server_id, project).await?;
        let (logins, clis) = (self.mcp_logins.clone(), self.clis());
        super::blocking(move || logins.sign_out(&clis, &target).map_err(Error::Invalid)).await
    }

    /// Reads the agents' own lists again, so the next [`Self::integrations_view`] shows who
    /// needs a sign-in as of now.
    pub async fn ask_mcp_sign_in_again(
        &self,
        server_id: &str,
        project: Option<ProjectId>,
    ) -> Result<()> {
        let target = self.signing_server(server_id, project).await?;
        let (logins, clis) = (self.mcp_logins.clone(), self.clis());
        super::blocking(move || {
            for adapter in adapters() {
                logins
                    .list(&clis, adapter, target.project.as_deref())
                    .map_err(Error::Invalid)?;
            }
            Ok(())
        })
        .await
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

    /// What a lean worker in `cwd` goes without ([`lean_switched_off`]): its agent's every MCP
    /// server and every skill but those `keep` names.
    pub(crate) async fn lean_worker_switched_off(
        &self,
        conversation: &ConversationId,
        provider: ProviderKind,
        cwd: &Path,
        keep: &[String],
    ) -> SwitchedOff {
        let (registry, repo) = (self.registry(), self.trusted_repo(conversation));
        let (cwd, keep) = (cwd.to_owned(), keep.to_vec());
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let session = Session {
                agent: agent(provider),
                cwd: &cwd,
                project: repo.as_deref(),
                in_checkout: true,
            };
            Ok(lean_switched_off(&catalog, &session, &keep))
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No sign-in read: the tests' servers don't sign in.
    fn no_sign_ins(_: &Server) -> (Option<SignInView>, Option<String>) {
        (None, None)
    }

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
    fn a_project_turns_a_default_off_or_on_for_itself_only() {
        let home = home();
        write(
            &home.join(".claude.json"),
            r#"{"mcpServers": {"railway": {"command": "railway"}, "pencil": {"command": "pencil"}}}"#,
        );
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
        // This project turns them over; another goes by the defaults.
        assert!(!settings.server_off("claude:user:railway", Some(&here)));
        assert!(settings.server_off("claude:user:pencil", Some(&here)));
        assert!(settings.server_off("claude:user:railway", Some(&there)));
        assert!(!settings.server_off("claude:user:pencil", Some(&there)));
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
        let view = settings_view(
            &registry,
            Some(&repo),
            std::slice::from_ref(&other),
            &switches,
            &no_sign_ins,
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
        let user = settings_view(&registry, None, &[], &switches, &no_sign_ins);
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
            catalog.server(&id).is_some()
        };
        let settings = Settings::default();
        assert!(named(&settings, &a));
        assert!(!named(&settings, &b));
        let everywhere = Settings {
            servers_everywhere: vec![id.clone()],
            ..Settings::default()
        };
        assert!(named(&everywhere, &b));
        // Settings' list still finds it, under its folder.
        let switches = Switches {
            settings: &settings,
            project: None,
        };
        let user = settings_view(&registry, None, &[], &switches, &no_sign_ins);
        assert!(user.servers.iter().any(|server| server.id == id));
        let in_b = settings_view(&registry, Some(&b), &[], &switches, &no_sign_ins);
        assert!(in_b.servers.iter().all(|server| server.id != id));
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
        // A lean worker (G23) goes without them all with nothing switched off, but the skills
        // it keeps.
        let lean = lean_switched_off(&catalog, &session(Agent::Claude), &[]);
        assert_eq!(lean.plugin_skills, ["docs:write"]);
        assert_eq!(lean.servers, ["plugin_docs_server"]);
        let lean = lean_switched_off(&catalog, &session(Agent::Claude), &["write".to_owned()]);
        assert!(lean.plugin_skills.is_empty(), "{lean:?}");
        assert_eq!(lean.servers, ["plugin_docs_server"]);
        let lean = lean_switched_off(&catalog, &session(Agent::Codex), &[]);
        assert_eq!(lean.skill_files, [codex.join("skills/chart/SKILL.md")]);

        let view = view(&catalog, None, &Settings::default(), &no_sign_ins);
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
