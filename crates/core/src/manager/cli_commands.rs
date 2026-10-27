//! The composer's catalog is the launch resolver's catalog. Opening a draft never starts a CLI.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_providers::{CliCommand, CliCommandKind, ProviderKind};

use super::{SessionManager, plugins};
use crate::accounts::AccountRef;
use crate::model::{ConversationId, ProjectId, Setup};
use crate::{Error, Result};

/// Last live Claude prompt metadata, persisted by account/project; never credentials.
#[derive(Default)]
pub(super) struct Probes {
    prompts: Mutex<HashMap<ProbeKey, Vec<CliCommand>>>,
    launches: Mutex<HashMap<ConversationId, (String, AccountRef, plugins::resolve::Resolved)>>,
}
type ProbeKey = (AccountRef, Option<PathBuf>);
impl Probes {
    fn fresh(&self, data: &Path, key: &ProbeKey) -> Vec<CliCommand> {
        let mut cache = self.prompts.lock().unwrap_or_else(|p| p.into_inner());
        if cache.is_empty()
            && let Ok(bytes) = std::fs::read(data.join("claude-prompts.json"))
            && let Ok(entries) = serde_json::from_slice::<Vec<(ProbeKey, Vec<CliCommand>)>>(&bytes)
        {
            cache.extend(entries);
        }
        cache.get(key).cloned().unwrap_or_default()
    }
    fn keep(&self, data: &Path, key: ProbeKey, commands: Vec<CliCommand>) {
        // Merge disk entries first, so updating one account never drops another account's cache.
        let _ = self.fresh(data, &key);
        let mut cache = self.prompts.lock().unwrap_or_else(|p| p.into_inner());
        cache.insert(key, commands);
        let entries: Vec<_> = cache.iter().collect();
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            std::fs::create_dir_all(data)?;
            let temp = data.join(format!(".prompts-{}.tmp", uuid::Uuid::new_v4()));
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temp)?;
            file.write_all(&serde_json::to_vec(&entries).map_err(std::io::Error::other)?)?;
            file.sync_all()?;
            std::fs::rename(temp, data.join("claude-prompts.json"))
        };
        if write().is_err() {
            tracing::warn!("could not save Claude prompt metadata");
        }
    }
    pub(super) fn launch(
        &self,
        id: &ConversationId,
        owner: &str,
        account: &AccountRef,
        resolved: &plugins::resolve::Resolved,
    ) {
        self.launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                id.clone(),
                (owner.into(), account.clone(), resolved.clone()),
            );
    }
    pub(super) fn end(&self, id: &ConversationId, owner: &str) {
        let mut launches = self.launches.lock().unwrap_or_else(|p| p.into_inner());
        if launches.get(id).is_some_and(|entry| entry.0 == owner) {
            launches.remove(id);
        }
    }
    fn snapshot(
        &self,
        id: &ConversationId,
        account: Option<&AccountRef>,
    ) -> Option<plugins::resolve::Resolved> {
        self.launches
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .filter(|entry| account.is_none_or(|account| account == &entry.1))
            .map(|entry| entry.2.clone())
    }
}

/// Version only launch identities and paths: never bearer/header/env values or capabilities.
fn command_version(resolved: &plugins::resolve::Resolved) -> String {
    let skills: Vec<_> = resolved
        .skills
        .iter()
        .map(|s| (&s.row_id, &s.invocation, &s.path))
        .collect();
    let servers: Vec<_> = resolved
        .servers
        .iter()
        .map(|s| {
            (
                &s.name,
                s.gateway.as_ref().map(|g| (&g.row_id, &g.resource)),
            )
        })
        .collect();
    brigadier_registry::catalog::fingerprint(
        serde_json::to_string(&(skills, servers))
            .expect("identities serialize")
            .as_bytes(),
    )
}

fn same_server(a: &brigadier_providers::McpServer, b: &brigadier_providers::McpServer) -> bool {
    fn normalized(server: &brigadier_providers::McpServer) -> brigadier_providers::McpServer {
        let mut server = server.clone();
        if let Some(gateway) = &mut server.gateway {
            gateway.url = gateway.resource.clone();
            gateway.secret_header.1.clear();
            server.transport = brigadier_providers::McpTransport::Http {
                url: gateway.resource.clone(),
            };
            server.env.clear();
        }
        server
    }
    normalized(a) == normalized(b)
}

/// Remote prompt discovery is supplied by the daemon gateway; never launch stdio servers here.
fn gateway_prompts(
    gateway: &std::sync::Arc<super::mcp_gateway::McpGatewayService>,
    account: &AccountRef,
    resolved: &plugins::resolve::Resolved,
) -> Vec<CliCommand> {
    if account.provider != ProviderKind::Claude {
        return Vec::new();
    }
    // The caller already resolved this account's current project, trust and choices.
    // Reusing that snapshot prevents cached prompts from bypassing off/scope decisions.
    gateway.cached_prompts(&resolved.servers)
}

fn builtins(provider: ProviderKind) -> Vec<CliCommand> {
    // Codex protocol: thread/compact/start (ThreadCompactStartParams). Review, rollback,
    // logout, etc. have no composer execution mapping and must not be sent as model text.
    let names: &[(&str, &str)] = match provider {
        ProviderKind::Codex => &[("compact", "Compact this conversation")],
        ProviderKind::Claude => &[
            ("compact", "Compact this conversation"),
            ("context", "Show context usage"),
            ("cost", "Show session usage"),
            ("help", "Show Claude Code help"),
        ],
    };
    names
        .iter()
        .map(|(name, description)| CliCommand {
            kind: Some(CliCommandKind::Command),
            source: Some(
                match provider {
                    ProviderKind::Claude => "Claude Code",
                    ProviderKind::Codex => "Codex",
                }
                .into(),
            ),
            invocation: Some(if provider == ProviderKind::Codex {
                "thread/compact/start".into()
            } else {
                name.to_string()
            }),
            agent_only: Some(provider),
            name: name.to_string(),
            text: format!("/{name}"),
            description: Some(description.to_string()),
            argument_hint: None,
        })
        .collect()
}

fn live_prompts(provider: ProviderKind, commands: Vec<CliCommand>) -> Vec<CliCommand> {
    if provider != ProviderKind::Claude {
        return Vec::new();
    }
    commands
        .into_iter()
        .filter_map(|mut command| {
            let raw = command.name.trim_end_matches(" (MCP)");
            let (server, prompt) = raw
                .strip_prefix("mcp__")
                .and_then(|name| name.split_once("__"))
                .or_else(|| {
                    command
                        .name
                        .ends_with(" (MCP)")
                        .then(|| raw.split_once(':'))
                        .flatten()
                })?;
            if server == "brigadier" || server.starts_with("brigadier-") {
                return None;
            }
            let invocation = format!("mcp__{server}__{prompt}");
            command.source = Some(server.to_owned());
            command.name = prompt.to_owned();
            command.text = format!("/{invocation}");
            command.invocation = Some(invocation);
            command.kind = Some(CliCommandKind::Prompt);
            command.agent_only = Some(ProviderKind::Claude);
            Some(command)
        })
        .collect()
}

fn menu(
    provider: ProviderKind,
    resolved: &plugins::resolve::Resolved,
    prompts: Vec<CliCommand>,
) -> Vec<CliCommand> {
    let mut commands: Vec<_> = resolved
        .skills
        .iter()
        .map(|skill| CliCommand {
            kind: Some(CliCommandKind::Skill),
            source: Some(skill.row_id.clone()),
            invocation: Some(skill.invocation.clone()),
            agent_only: skill
                .row_id
                .ends_with(":different-per-agent")
                .then_some(provider),
            name: skill
                .display
                .rsplit(':')
                .next()
                .unwrap_or(&skill.display)
                .to_owned(),
            text: format!("/{}", skill.invocation),
            description: None,
            argument_hint: None,
        })
        .collect();
    commands.sort_by(|a, b| a.name.cmp(&b.name));
    commands.extend(builtins(provider));
    if provider == ProviderKind::Claude {
        commands.extend(prompts);
    }
    commands
}

/// Skill chips are typed references, never a second slash command in the message body.
pub(super) fn without_skill_tokens(text: &str, mentions: &[crate::model::Mention]) -> String {
    let mut text = text.to_owned();
    for mention in mentions {
        let crate::model::Mention::Skill { row_id } = mention else {
            continue;
        };
        let Some(name) = row_id
            .strip_prefix("skill:")
            .and_then(|rest| rest.rsplit_once(':').map(|(name, _)| name))
        else {
            continue;
        };
        let token = format!("/{name}");
        let spans: Vec<_> = text
            .match_indices(&token)
            .filter_map(|(at, _)| {
                let before = text[..at].chars().next_back();
                let after = text[at + token.len()..].chars().next();
                (before.is_none_or(char::is_whitespace)
                    && after.is_none_or(|ch| !ch.is_alphanumeric() && !"_-./".contains(ch)))
                .then_some(at)
            })
            .collect();
        for at in spans.into_iter().rev() {
            text.replace_range(at..at + token.len(), "");
        }
    }
    text
}

impl SessionManager {
    async fn command_resolution(
        &self,
        account: &AccountRef,
        project: Option<&Path>,
        cwd: Option<&Path>,
        trusted: bool,
    ) -> Result<(plugins::resolve::Resolved, String)> {
        let roots = self.registry().roots().clone();
        let cwd = cwd.unwrap_or(&roots.home);
        let resolved = self
            .resolve_plugins(
                account,
                project,
                cwd,
                trusted,
                plugins::resolve::Role::Thread,
            )
            .await?;
        let version = command_version(&resolved);
        Ok((resolved, version))
    }

    pub(super) async fn conversation_skills(
        &self,
        id: &ConversationId,
        account: Option<&AccountRef>,
    ) -> Result<plugins::resolve::Resolved> {
        if let Some(snapshot) = self.cli_command_probes.snapshot(id, account) {
            return Ok(snapshot);
        }
        let conversation = self.core.conversation(id)?;
        let choice = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator.clone(),
            Some(Setup::Chat { model, .. }) => model.clone(),
            None => self.default_chat_choice(),
        };
        let project = match &conversation.setup {
            Some(Setup::Session { repo, .. }) => Some(PathBuf::from(repo)),
            _ => None,
        };
        let cwd = self
            .checkout_dir(id)
            .ok()
            .map(PathBuf::from)
            .filter(|path| path.is_dir())
            .or_else(|| project.clone());
        self.command_resolution(
            account.unwrap_or(&self.runtime.account_for(&choice)),
            project.as_deref(),
            cwd.as_deref(),
            self.repo_trust(id) == Some(true),
        )
        .await
        .map(|(resolved, _)| resolved)
    }

    /// Resolve persisted row identities again for every send, steer, queue delivery or resume.
    pub(super) async fn skill_inputs(
        &self,
        id: &ConversationId,
        mentions: &[crate::model::Mention],
        account: Option<&AccountRef>,
    ) -> Result<Vec<brigadier_providers::InputPart>> {
        let selected: Vec<_> = mentions
            .iter()
            .filter_map(|mention| match mention {
                crate::model::Mention::Skill { row_id } => Some(row_id),
                _ => None,
            })
            .collect();
        if selected.is_empty() {
            return Ok(Vec::new());
        }
        let live_account = if account.is_none() {
            let live = self.convs_lock().get(id).cloned();
            match live {
                Some(conv) => conv.cli().await.map(|cli| cli.account.clone()),
                None => None,
            }
        } else {
            None
        };
        let account = account.or(live_account.as_ref());
        let conversation = self.core.conversation(id)?;
        let provider = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator.provider,
            Some(Setup::Chat { model, .. }) => model.provider,
            None => self.default_chat_choice().provider,
        };
        let provider = account.map_or(provider, |account| account.provider);
        if provider == ProviderKind::Claude && selected.len() > 1 {
            return Err(Error::Invalid(
                "Claude accepts one skill per message; remove the extra skill chips".into(),
            ));
        }
        let resolved = self.conversation_skills(id, account).await?;
        selected.into_iter().map(|row_id| {
            let skill = resolved.skills.iter().find(|skill| &skill.row_id == row_id)
                .ok_or_else(|| Error::Invalid("the selected skill is no longer available; remove its chip and choose it again".into()))?;
            if !skill.path.is_file() || skill.invocation.is_empty() || skill.invocation.chars().any(char::is_whitespace) || skill.invocation.starts_with('/') {
                return Err(Error::Invalid(format!("the selected skill {} is no longer valid; choose it again", skill.display)));
            }
            Ok(brigadier_providers::InputPart::Skill { invocation: skill.invocation.clone(), path: skill.path.clone() })
        }).collect()
    }

    /// Claude adds the invocation before every frame at serialization; record the same words
    /// and shift authored ranges so frame text never becomes the user's text in the thread.
    pub(super) fn note_skill_sent(
        &self,
        id: &ConversationId,
        input: &brigadier_providers::TurnInput,
        user: &[(usize, usize)],
        provider: ProviderKind,
    ) {
        if provider == ProviderKind::Claude
            && let Some(invocation) = input.parts.iter().find_map(|part| match part {
                brigadier_providers::InputPart::Skill { invocation, .. } => Some(invocation),
                _ => None,
            })
        {
            let prefix = format!("/{invocation}\n");
            let shift = super::cli_thread::solid_len(&prefix);
            let mut noted = input.clone();
            noted
                .parts
                .insert(0, brigadier_providers::InputPart::Text(prefix));
            let mut authored = vec![(0, shift)];
            authored.extend(user.iter().map(|(from, to)| (from + shift, to + shift)));
            self.note_sent(id, &noted, &authored);
        } else {
            self.note_sent(id, input, user);
        }
    }

    pub(super) async fn remember_claude_prompts(
        &self,
        id: &ConversationId,
        cli: &super::conversation::Cli,
    ) {
        if cli.provider != ProviderKind::Claude {
            return;
        }
        let project = match self.core.conversation(id).ok().and_then(|c| c.setup) {
            Some(Setup::Session { repo, .. }) => Some(PathBuf::from(repo)),
            _ => None,
        };
        let commands = cli.session.commands().await;
        if !commands.is_empty() {
            self.cli_command_probes.keep(
                &self.data_dir,
                (cli.account.clone(), project),
                live_prompts(cli.provider, commands),
            );
        }
    }

    pub async fn cli_commands(
        &self,
        id: Option<ConversationId>,
        provider: Option<ProviderKind>,
        project: Option<ProjectId>,
        repo: Option<String>,
    ) -> Result<Vec<CliCommand>> {
        let live = id
            .as_ref()
            .and_then(|id| self.convs_lock().get(id).cloned());
        let cli = match live {
            Some(conv) => conv.cli().await,
            None => None,
        };
        let (account, project, cwd, trusted) = if let Some(id) = &id {
            let conversation = self.core.conversation(id)?;
            let choice = match &conversation.setup {
                Some(Setup::Session { orchestrator, .. }) => orchestrator.clone(),
                Some(Setup::Chat { model, .. }) => model.clone(),
                None => self.default_chat_choice(),
            };
            let project = match &conversation.setup {
                Some(Setup::Session { repo, .. }) => Some(PathBuf::from(repo)),
                _ => None,
            };
            let cwd = self
                .checkout_dir(id)
                .ok()
                .map(PathBuf::from)
                .filter(|path| path.is_dir())
                .or_else(|| project.clone());
            (
                cli.as_ref()
                    .map(|cli| cli.account.clone())
                    .unwrap_or_else(|| self.runtime.account_for(&choice)),
                project,
                cwd,
                self.repo_trust(id) == Some(true),
            )
        } else {
            let provider = provider.ok_or_else(|| {
                Error::Invalid("a new conversation's commands need its CLI".into())
            })?;
            let (folder, trusted) = if let Some(project) = project {
                let project = self.core.project(&project)?;
                let repo = repo
                    .or_else(|| project.repos.first().map(|repo| repo.path.clone()))
                    .ok_or_else(|| Error::Invalid("the project has no repository".into()))?;
                let trusted = project.counts_trusted(&repo);
                (Some(PathBuf::from(repo)), trusted)
            } else {
                (None, false)
            };
            (
                self.runtime.launch_account(provider, None),
                folder.clone(),
                folder,
                trusted,
            )
        };
        let (current, _version) = self
            .command_resolution(&account, project.as_deref(), cwd.as_deref(), trusted)
            .await?;
        let key = (account.clone(), project.clone());
        let resolved = cli
            .as_ref()
            .map_or_else(|| current.clone(), |cli| cli.plugins.clone());
        if account.provider == ProviderKind::Claude
            && let Some(cli) = &cli
        {
            let commands = cli.session.commands().await;
            if !commands.is_empty() {
                self.cli_command_probes.keep(
                    &self.data_dir,
                    key.clone(),
                    live_prompts(account.provider, commands),
                );
            }
        }
        let mut prompts = self.cli_command_probes.fresh(&self.data_dir, &key);
        prompts.extend(gateway_prompts(self.mcp_gateway(), &account, &resolved));
        prompts.retain(|prompt| {
            prompt
                .source
                .as_ref()
                .is_some_and(|name| resolved.servers.iter().any(|server| &server.name == name))
        });
        let mut seen = std::collections::HashSet::new();
        prompts.retain(|prompt| seen.insert(prompt.invocation.clone()));
        let mut commands = menu(account.provider, &resolved, prompts);
        if cli.is_some() {
            for command in &mut commands {
                if command.kind == Some(CliCommandKind::Skill)
                    && !current.skills.iter().any(|s| {
                        Some(&s.row_id) == command.source.as_ref()
                            && Some(&s.invocation) == command.invocation.as_ref()
                    })
                {
                    command.description = Some("applies to new sessions".into());
                }
                if command.kind == Some(CliCommandKind::Prompt)
                    && resolved
                        .servers
                        .iter()
                        .find(|s| Some(&s.name) == command.source.as_ref())
                        .is_some_and(|old| !current.servers.iter().any(|new| same_server(old, new)))
                {
                    command.description = Some("applies to new sessions".into());
                }
            }
            for skill in &current.skills {
                if !resolved.skills.iter().any(|s| s.row_id == skill.row_id) {
                    let mut item = menu(
                        account.provider,
                        &plugins::resolve::Resolved {
                            skills: vec![skill.clone()],
                            ..Default::default()
                        },
                        Vec::new(),
                    )
                    .remove(0);
                    item.text.clear();
                    item.description = Some("applies to new sessions".into());
                    commands.push(item);
                }
            }
        }
        Ok(commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_registry::Catalog;
    use plugins::resolve::{Resolved, ResolvedSkill};

    struct PromptCatalog {
        root: PathBuf,
        roots: brigadier_registry::Roots,
        project: PathBuf,
        other: PathBuf,
        rows: Vec<brigadier_registry::catalog::Row>,
    }
    impl PromptCatalog {
        fn new(resource: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("gateway-prompt-menu-{}", uuid::Uuid::new_v4()));
            let home = root.join("home");
            let project = root.join("project");
            let other = root.join("other-project");
            for path in [&home, &project, &other] {
                std::fs::create_dir_all(path).unwrap();
            }
            let roots = brigadier_registry::Roots::from_vars([(
                "HOME".to_owned(),
                home.to_string_lossy().into_owned(),
            )])
            .unwrap();
            std::fs::write(
                &roots.claude_json,
                serde_json::json!({"mcpServers":{
                    "db":{"type":"http", "url":resource},
                    "local":{"command":"must-never-be-spawned"}
                }})
                .to_string(),
            )
            .unwrap();
            let rows = Catalog::read_rows(&roots, &[project.clone(), other.clone()]);
            Self {
                root,
                roots,
                project,
                other,
                rows,
            }
        }
        fn row(&self) -> &str {
            &self.rows.iter().find(|row| row.name == "db").unwrap().id
        }
        fn resolve(
            &self,
            gateway: &super::super::mcp_gateway::McpGatewayService,
            project: &Path,
            choices: &plugins::choices::Choices,
        ) -> Resolved {
            plugins::resolve::resolve(plugins::resolve::Context {
                agent: brigadier_registry::Agent::Claude,
                account_home: &self.roots,
                project_root: Some(project),
                cwd: project,
                trusted: true,
                role: plugins::resolve::Role::Thread,
                rows: &self.rows,
                choices,
                gateway: Some(gateway),
            })
        }
    }
    impl Drop for PromptCatalog {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    #[tokio::test]
    async fn gateway_prompts_arrive_on_second_call_and_follow_current_on_and_project_scope() {
        use super::super::mcp_gateway::{McpGatewayService, store::MemoryStore, tests::Fixture};
        use plugins::choices::{Choice, Choices, Global, Scope};
        let fixture = Fixture::new().await;
        fixture.set_prompts(&["review"]);
        let gateway =
            McpGatewayService::with_store(std::sync::Arc::new(MemoryStore::default()), None)
                .unwrap();
        let catalog = PromptCatalog::new(&fixture.resource());
        let account = AccountRef::own(ProviderKind::Claude);
        let mut choices = Choices::default();
        // No grant: no discovery, no browser, and no stdio process for the local server.
        let resolved = catalog.resolve(&gateway, &catalog.project, &choices);
        assert!(gateway_prompts(&gateway, &account, &resolved).is_empty());
        assert_eq!(fixture.prompt_lists(), 0);
        fixture.signed_in(&gateway).await;
        let resolved = catalog.resolve(&gateway, &catalog.project, &choices);
        assert!(gateway_prompts(&gateway, &account, &resolved).is_empty());
        gateway.wait_for_prompt_cache().await;
        let prompts = gateway_prompts(&gateway, &account, &resolved);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "review");
        assert_eq!(prompts[0].invocation.as_deref(), Some("mcp__db__review"));
        assert_eq!(prompts[0].text, "/mcp__db__review");
        assert_eq!(prompts[0].source.as_deref(), Some("db"));
        assert_eq!(prompts[0].kind, Some(CliCommandKind::Prompt));
        assert_eq!(prompts[0].agent_only, Some(ProviderKind::Claude));
        assert_eq!(prompts[0].description.as_deref(), Some("A remote prompt"));
        assert_eq!(prompts[0].argument_hint.as_deref(), Some("<topic>"));
        assert_eq!(fixture.prompt_lists(), 1);
        // A distinct account reuses the same row/grant cache, not another request.
        let other_account = AccountRef::new(ProviderKind::Claude, Some("acct-two".into()));
        assert_eq!(
            gateway_prompts(&gateway, &other_account, &resolved),
            prompts
        );
        assert!(
            gateway_prompts(&gateway, &AccountRef::own(ProviderKind::Codex), &resolved).is_empty()
        );
        choices.0.insert(
            catalog.row().into(),
            Choice {
                removed: None,
                on: false,
                scope: Scope::Global(Global::Global),
            },
        );
        let off = catalog.resolve(&gateway, &catalog.project, &choices);
        assert!(gateway_prompts(&gateway, &account, &off).is_empty());
        choices.0.insert(
            catalog.row().into(),
            Choice {
                removed: None,
                on: true,
                scope: Scope::Projects(vec![catalog.project.clone()]),
            },
        );
        let outside = catalog.resolve(&gateway, &catalog.other, &choices);
        assert!(gateway_prompts(&gateway, &account, &outside).is_empty());
        let inside = catalog.resolve(&gateway, &catalog.project, &choices);
        assert_eq!(gateway_prompts(&gateway, &account, &inside), prompts);
        assert_eq!(fixture.prompt_lists(), 1);
    }

    fn skills() -> Resolved {
        Resolved {
            skills: vec![
                ResolvedSkill {
                    row_id: "skill:proj-agents:a".into(),
                    display: "proj-agents".into(),
                    invocation: "proj-agents".into(),
                    path: "/resolved/proj-agents/SKILL.md".into(),
                    native: true,
                },
                ResolvedSkill {
                    row_id: "skill:proj-hello:b".into(),
                    display: "proj-hello".into(),
                    invocation: "plugin:proj-hello".into(),
                    path: "/resolved/proj-hello/SKILL.md".into(),
                    native: true,
                },
                ResolvedSkill {
                    row_id: "skill:higgsfield-generate:c".into(),
                    display: "higgsfield-generate:higgsfield-generate".into(),
                    invocation: "higgsfield-generate:higgsfield-generate".into(),
                    path: "/resolved/higgsfield/SKILL.md".into(),
                    native: true,
                },
            ],
            ..Resolved::default()
        }
    }

    #[test]
    fn grouped_catalog_keeps_plain_names_and_exact_skill_identity_for_both_clis() {
        for provider in ProviderKind::ALL {
            let commands = menu(provider, &skills(), Vec::new());
            let hello = commands
                .iter()
                .find(|command| command.name == "proj-hello")
                .unwrap();
            assert_eq!(hello.kind, Some(CliCommandKind::Skill));
            assert_eq!(hello.source.as_deref(), Some("skill:proj-hello:b"));
            assert_eq!(hello.invocation.as_deref(), Some("plugin:proj-hello"));
            assert!(commands.iter().all(|command| !command.name.contains(':')));
            assert!(
                commands
                    .iter()
                    .any(|command| command.name == "higgsfield-generate")
            );
            if provider == ProviderKind::Codex {
                let commands: Vec<_> = commands
                    .iter()
                    .filter(|command| command.kind == Some(CliCommandKind::Command))
                    .collect();
                assert_eq!(commands.len(), 1);
                assert_eq!(
                    commands[0].invocation.as_deref(),
                    Some("thread/compact/start")
                );
            }
        }
    }

    #[test]
    fn live_prompts_are_plain_claude_only_and_agent_tagged() {
        let listed = vec![
            CliCommand {
                name: "mcp__db__greet".into(),
                ..CliCommand::default()
            },
            CliCommand {
                name: "db:review (MCP)".into(),
                ..CliCommand::default()
            },
            CliCommand {
                name: "skill:skill".into(),
                ..CliCommand::default()
            },
        ];
        let prompts = live_prompts(ProviderKind::Claude, listed.clone());
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "greet");
        assert_eq!(prompts[0].invocation.as_deref(), Some("mcp__db__greet"));
        assert!(
            prompts
                .iter()
                .all(|command| command.kind == Some(CliCommandKind::Prompt)
                    && command.agent_only == Some(ProviderKind::Claude))
        );
        assert!(live_prompts(ProviderKind::Codex, listed).is_empty());
        assert!(
            menu(ProviderKind::Codex, &Resolved::default(), prompts)
                .iter()
                .all(|command| command.kind != Some(CliCommandKind::Prompt))
        );
    }

    #[tokio::test]
    async fn draft_and_message_use_same_resolver_and_proj_hello_never_selects_proj_agents() {
        use crate::manager::conversation::FilesAt;
        use crate::manager::flow::{Flow, Options, Reply};
        use crate::model::{ConversationKind, Mention};
        use brigadier_providers::InputPart;
        use std::sync::Arc;
        for provider in ProviderKind::ALL {
            let flow = Flow::start(
                "skill-chip-resolution",
                Options {
                    thread: provider,
                    files: vec![
                        (
                            ".claude/skills/proj-hello/SKILL.md",
                            "---\nname: proj-hello\ndescription: hello skill\n---\nHELLO",
                        ),
                        (
                            ".claude/skills/proj-agents/SKILL.md",
                            "---\nname: proj-agents\ndescription: agents skill\n---\nBANANA",
                        ),
                    ],
                    ..Options::default()
                },
                Arc::new(|_| Box::pin(async { Reply::text("ok") })),
            )
            .await;
            let commands = flow
                .manager
                .cli_commands(Some(flow.conversation.clone()), None, None, None)
                .await
                .unwrap();
            let hello = commands
                .iter()
                .find(|command| command.name == "proj-hello")
                .unwrap();
            let conv = flow.manager.conv(&flow.conversation).unwrap();
            assert!(conv.cli().await.is_none(), "opening a draft started a CLI");
            let mentions = vec![Mention::Skill {
                row_id: hello.source.clone().unwrap(),
            }];
            let (parts, _) = flow
                .manager
                .message_input(
                    &FilesAt::Conversation(&conv),
                    &flow.conversation,
                    ConversationKind::Session,
                    "/proj-hello explain",
                    &[],
                    &mentions,
                    &mut HashMap::new(),
                    &mut Vec::new(),
                    None,
                )
                .await
                .unwrap();
            let resolved = flow
                .manager
                .conversation_skills(&flow.conversation, None)
                .await
                .unwrap();
            let selected = resolved
                .skills
                .iter()
                .find(|skill| {
                    skill.row_id
                        == mentions
                            .iter()
                            .find_map(|mention| match mention {
                                Mention::Skill { row_id } => Some(row_id.as_str()),
                                _ => None,
                            })
                            .unwrap()
                })
                .unwrap();
            assert!(parts.contains(&InputPart::Skill {
                invocation: selected.invocation.clone(),
                path: selected.path.clone()
            }));
            assert!(selected.path.ends_with("proj-hello/SKILL.md"));
            assert!(parts.iter().all(|part| !matches!(part, InputPart::Skill { path, .. } if path.ends_with("proj-agents/SKILL.md"))));
            assert!(
                parts.iter().all(
                    |part| !matches!(part, InputPart::Text(text) if text.contains("/proj-hello"))
                ),
                "the chip also became model text"
            );
            if provider == ProviderKind::Claude {
                let framed = brigadier_providers::TurnInput {
                    parts: vec![
                        InputPart::Text("[frame]".into()),
                        InputPart::Text("explain".into()),
                        InputPart::Skill {
                            invocation: selected.invocation.clone(),
                            path: selected.path.clone(),
                        },
                    ],
                    files: vec![],
                };
                flow.manager
                    .note_skill_sent(&flow.conversation, &framed, &[(7, 14)], provider);
                assert_eq!(
                    flow.manager.noted_said(
                        &flow.conversation,
                        &format!("/{}\n[frame]explain", selected.invocation)
                    ),
                    Some(brigadier_providers::session_file::Said::Both {
                        user: format!("/{}\n\nexplain", selected.invocation)
                    })
                );
            }
            let mut choices = plugins::choices::Choices::default();
            choices.0.insert(
                hello.source.clone().unwrap(),
                plugins::choices::Choice {
                    removed: None,
                    on: false,
                    scope: plugins::choices::Scope::Global(plugins::choices::Global::Global),
                },
            );
            choices.save(&flow.manager.data_dir).unwrap();
            let changed = flow
                .manager
                .cli_commands(Some(flow.conversation.clone()), None, None, None)
                .await
                .unwrap();
            assert!(
                changed.iter().all(|command| command.name != "proj-hello"),
                "choice change left a cached skill"
            );
            assert!(
                flow.manager
                    .skill_inputs(&flow.conversation, &mentions, None)
                    .await
                    .is_err()
            );
            let rejected = flow
                .manager
                .send_message(
                    flow.conversation.clone(),
                    "/proj-hello explain".into(),
                    vec![],
                    mentions,
                    true,
                    None,
                )
                .await;
            assert!(
                rejected.is_err(),
                "a disabled chip was accepted for steer/send"
            );
            assert!(conv.cli().await.is_none(), "rejected input started a CLI");
            flow.stop().await;
        }
    }

    #[test]
    fn live_prompt_metadata_persists_per_account_and_project() {
        let dir = std::env::temp_dir().join(format!("prompt-persistence-{}", uuid::Uuid::new_v4()));
        let key = (
            AccountRef::own(ProviderKind::Claude),
            Some(PathBuf::from("/project")),
        );
        let cache = Probes::default();
        cache.keep(
            &dir,
            key.clone(),
            vec![CliCommand {
                name: "review".into(),
                ..Default::default()
            }],
        );
        let restarted = Probes::default();
        assert_eq!(restarted.fresh(&dir, &key)[0].name, "review");
        assert!(
            restarted
                .fresh(&dir, &(key.0.clone(), Some(PathBuf::from("/other"))))
                .is_empty()
        );
        assert!(
            restarted
                .fresh(
                    &dir,
                    &(
                        AccountRef::new(ProviderKind::Claude, Some("other".into())),
                        key.1
                    )
                )
                .is_empty()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn live_skill_menu_and_sends_keep_launch_choices_until_session_end() {
        use crate::manager::flow::{Flow, Options, Reply};
        use crate::model::Mention;
        let flow = Flow::start(
            "launch-skill-snapshot",
            Options {
                files: vec![(".claude/skills/old/SKILL.md", "---\nname: old\n---\nOLD")],
                ..Default::default()
            },
            std::sync::Arc::new(|_| Box::pin(async { Reply::text("ok") })),
        )
        .await;
        flow.say("start").await;
        flow.settled().await;
        let row = flow
            .manager
            .plugin_catalog()
            .unwrap()
            .rows
            .into_iter()
            .find(|r| r.name == "old")
            .unwrap();
        flow.manager.set_plugin_choice(&row.id, false).unwrap();
        let file = flow.manager.data_dir.join("library/skills/new/SKILL.md");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "---\nname: new\n---\nNEW").unwrap();
        let commands = flow
            .manager
            .cli_commands(Some(flow.conversation.clone()), None, None, None)
            .await
            .unwrap();
        let old = commands.iter().find(|c| c.name == "old").unwrap();
        assert_eq!(old.description.as_deref(), Some("applies to new sessions"));
        assert!(!old.text.is_empty());
        let new = commands.iter().find(|c| c.name == "new").unwrap();
        assert_eq!(new.description.as_deref(), Some("applies to new sessions"));
        assert!(new.text.is_empty());
        let cli = flow
            .manager
            .conv(&flow.conversation)
            .unwrap()
            .cli()
            .await
            .unwrap();
        assert!(
            flow.manager
                .skill_inputs(
                    &flow.conversation,
                    &[Mention::Skill { row_id: row.id }],
                    Some(&cli.account)
                )
                .await
                .is_ok()
        );
        assert!(
            flow.manager
                .skill_inputs(
                    &flow.conversation,
                    &[Mention::Skill {
                        row_id: new.source.clone().unwrap()
                    }],
                    Some(&cli.account)
                )
                .await
                .is_err()
        );
        cli.session.close().await;
        cli.ended.cancelled().await;
        let commands = flow
            .manager
            .cli_commands(Some(flow.conversation.clone()), None, None, None)
            .await
            .unwrap();
        assert!(!commands.iter().any(|c| c.name == "old"));
        assert!(
            !commands
                .iter()
                .find(|c| c.name == "new")
                .unwrap()
                .text
                .is_empty()
        );
        flow.stop().await;
    }

    #[test]
    fn command_versions_exclude_credentials_and_capabilities() {
        let mut resolved = skills();
        let server = brigadier_providers::McpServer {
            name: "db".into(),
            gateway: None,
            transport: brigadier_providers::McpTransport::Http {
                url: "https://resource.invalid/mcp".into(),
            },
            env: vec![("Authorization".into(), "before".into())],
            tool_timeout_secs: None,
            trusted: false,
            always_load: false,
        };
        resolved.servers.push(server);
        let before = command_version(&resolved);
        resolved.servers[0].env[0].1 = "after".into();
        assert_eq!(before, command_version(&resolved));
    }
}
