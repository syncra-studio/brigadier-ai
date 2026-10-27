//! The agents' MCP servers and skills as sessions meet them: each thread and worker keeps its
//! resolved catalog items. A naked worker gets only explicit server grants; OAuth uses the
//! daemon gateway with an explicit per-CLI fallback.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use brigadier_providers::McpTransport;
use brigadier_providers::{McpServer, ProviderKind, SwitchedOff};
use brigadier_registry::{Agent, Registry, Roots};
#[cfg(test)]
use brigadier_registry::{Catalog, Session, SkillRoot, Source, Transport};

use super::SessionManager;
use super::mcp_login::Clis;
use crate::model::ConversationId;
use crate::{Error, Result};

/// Brigadier's own servers' names, which a worker keeps.
#[cfg(test)]
pub(crate) const RESERVED: &[&str] = &[super::computer_use::SERVER_NAME];

fn provider(agent: Agent) -> ProviderKind {
    super::mcp_login::provider(agent)
}

pub(crate) fn agent(provider: ProviderKind) -> Agent {
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

/// What of its agent's own setup a naked worker (`brigadier start`, D13) goes without: every
/// skill, and every MCP server but those `granted` names (D18).
#[cfg(test)]
pub(crate) fn naked_switched_off(
    catalog: &Catalog,
    session: &Session<'_>,
    granted: &[String],
) -> SwitchedOff {
    let mut off = SwitchedOff::default();
    for server in &catalog.servers {
        // A user's server under Brigadier's own name isn't one Brigadier gives: going without
        // it must not take Brigadier's own away.
        if RESERVED.contains(&server.name.as_str())
            || !catalog.server_native(server, session)
            || granted.contains(&server.native_name())
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
        if !catalog.skill_native(skill, session) {
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

/// The servers `names` grant a worker (`brigadier start --mcp`, D18), each its agent's own
/// (`session`'s), under the name `brigadier context mcps` prints for it, which is the name its
/// CLI knows it by. Each is given to the CLI as Brigadier's own servers are, since a naked
/// worker loads none of the user's config. A name none of them has is refused.
#[cfg(test)]
pub(crate) fn granted_servers(
    catalog: &Catalog,
    session: &Session<'_>,
    names: &[String],
) -> std::result::Result<Vec<McpServer>, String> {
    let mut granted: Vec<McpServer> = Vec::new();
    for name in names {
        let server = catalog
            .servers
            .iter()
            .filter(|server| !RESERVED.contains(&server.name.as_str()))
            .find(|server| {
                (server.native_name() == *name || server.name == *name || server.id == *name)
                    && catalog.server_native(server, session)
            })
            .ok_or_else(|| {
                format!(
                    "no MCP server {name:?} of your agent's here; `brigadier context mcps` lists \
                     them"
                )
            })?;
        let name = server.native_name();
        if granted.iter().any(|given| given.name == name) {
            continue;
        }
        let expose = |values: &[(String, brigadier_registry::Secret)]| {
            values
                .iter()
                .map(|(name, value)| (name.clone(), value.expose().to_owned()))
                .collect()
        };
        let (transport, env) = match &server.transport {
            Transport::Stdio {
                command, args, env, ..
            } => (
                McpTransport::Stdio {
                    command: PathBuf::from(command),
                    args: args.clone(),
                    cwd: server.transport.working_dir().map(Path::to_owned),
                },
                expose(env),
            ),
            Transport::Http { url, headers } => {
                (McpTransport::Http { url: url.clone() }, expose(headers))
            }
        };
        granted.push(McpServer {
            // B: launch fills gateway metadata for resolved remote servers.
            gateway: None,
            name,
            transport,
            env,
            tool_timeout_secs: None,
            trusted: false,
            always_load: false,
        });
    }
    Ok(granted)
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

    /// What a naked worker in `cwd` goes without ([`naked_switched_off`]), and the servers
    /// `grants` give it ([`granted_servers`]): a grant whose server is gone since is left out.
    pub(crate) async fn naked_worker_setup(
        &self,
        conversation: &ConversationId,
        account: &crate::accounts::AccountRef,
        cwd: &Path,
        names: &[String],
    ) -> Result<(SwitchedOff, Vec<McpServer>)> {
        let repo = self.trusted_repo(conversation);
        let trusted = self.repo_trust(conversation) == Some(true);
        let resolved = self
            .resolve_plugins(
                account,
                repo.as_deref(),
                cwd,
                trusted,
                super::plugins::resolve::Role::Thread,
            )
            .await?;
        for warning in &resolved.warnings {
            self.notice(
                conversation,
                brigadier_providers::NoticeLevel::Warning,
                &warning.message,
            )
            .await;
        }
        let available_names: Vec<_> = names
            .iter()
            .filter(|name| {
                !resolved
                    .warnings
                    .iter()
                    .any(|warning| warning.server == **name)
            })
            .cloned()
            .collect();
        let grants = select_grants(&resolved.servers, &available_names).map_err(Error::Invalid)?;
        let resolved = self
            .resolve_plugins(
                account,
                repo.as_deref(),
                cwd,
                trusted,
                super::plugins::resolve::Role::Worker { grants: &grants },
            )
            .await?;
        Ok((resolved.off, resolved.servers))
    }

    /// The names `brigadier start --mcp` gave, each as its CLI knows it, or why one can't be
    /// given ([`granted_servers`]), for a worker of `conversation` on `provider` in `cwd`.
    pub(crate) async fn worker_grants_named(
        &self,
        conversation: &ConversationId,
        provider: ProviderKind,
        cwd: &Path,
        names: &[String],
    ) -> std::result::Result<Vec<String>, String> {
        if names.is_empty() {
            return Ok(Vec::new());
        }
        let repo = self.trusted_repo(conversation);
        let resolved = self
            .resolve_plugins(
                &crate::accounts::AccountRef::own(provider),
                repo.as_deref(),
                cwd,
                self.repo_trust(conversation) == Some(true),
                super::plugins::resolve::Role::Thread,
            )
            .await
            .map_err(|error| error.to_string())?;
        for warning in &resolved.warnings {
            self.notice(
                conversation,
                brigadier_providers::NoticeLevel::Warning,
                &warning.message,
            )
            .await;
        }
        let available_names: Vec<_> = names
            .iter()
            .filter(|name| {
                !resolved
                    .warnings
                    .iter()
                    .any(|warning| warning.server == **name)
            })
            .cloned()
            .collect();
        select_grants(&resolved.servers, &available_names)
            .map(|servers| servers.into_iter().map(|server| server.name).collect())
    }
}

fn select_grants(
    servers: &[McpServer],
    names: &[String],
) -> std::result::Result<Vec<McpServer>, String> {
    let mut grants = Vec::new();
    for name in names {
        let server = servers
            .iter()
            .find(|server| server.name == *name)
            .ok_or_else(|| {
                format!("no enabled MCP server {name:?} here; `brigadier context mcps` lists them")
            })?;
        if !grants
            .iter()
            .any(|known: &McpServer| known.name == server.name)
        {
            grants.push(server.clone());
        }
    }
    Ok(grants)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill(dir: &Path, name: &str) {
        write(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: d\n---\n"),
        );
    }

    /// A Claude plugin and a Codex plugin, each with a skill and a server: a naked worker goes
    /// without them by its own agent's names, but the servers it was granted.
    #[test]
    fn a_naked_worker_goes_without_plugins_by_their_agents_names() {
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
        let session = |agent| Session {
            agent,
            cwd: &home,
            project: None,
            in_checkout: false,
        };
        let lean = naked_switched_off(&catalog, &session(Agent::Claude), &[]);
        assert_eq!(lean.plugin_skills, ["docs:write"]);
        assert!(lean.skill_names.is_empty());
        assert_eq!(lean.servers, ["plugin_docs_server"]);
        // A server granted (D18) is given, not switched off; its plugin's skill still goes.
        let granted = vec!["plugin_docs_server".to_owned()];
        let lean = naked_switched_off(&catalog, &session(Agent::Claude), &granted);
        assert_eq!(lean.plugin_skills, ["docs:write"]);
        assert!(lean.servers.is_empty(), "{lean:?}");
        let given = granted_servers(&catalog, &session(Agent::Claude), &granted).unwrap();
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].name, "plugin_docs_server");
        assert!(matches!(
            &given[0].transport,
            McpTransport::Stdio { command, .. } if command == &PathBuf::from("s")
        ));
        // By its own name too; another agent's server, or none, is refused.
        let by_name = granted_servers(&catalog, &session(Agent::Claude), &["server".into()]);
        assert_eq!(by_name.unwrap()[0].name, "plugin_docs_server");
        let other = granted_servers(&catalog, &session(Agent::Claude), &["calc".into()]);
        assert!(other.unwrap_err().contains("`brigadier context mcps`"));
        let lean = naked_switched_off(&catalog, &session(Agent::Codex), &[]);
        assert_eq!(
            lean.plugin_servers,
            [("sheets@openai".to_owned(), "calc".to_owned())]
        );
        assert!(lean.servers.is_empty());
        assert_eq!(lean.skill_files, [codex.join("skills/chart/SKILL.md")]);
        let _ = std::fs::remove_dir_all(&home);
    }
}
