//! The composer's catalog is the launch resolver's catalog. Opening a draft never starts a CLI.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_providers::{CliCommand, CliCommandKind, ProviderKind};
use brigadier_registry::Catalog;

use super::{SessionManager, plugins};
use crate::accounts::AccountRef;
use crate::model::{ConversationId, ProjectId, Setup};
use crate::{Error, Result};

/// Optional live enrichment, bound to the exact choices/catalog snapshot it came from.
#[derive(Default)]
pub(super) struct Probes(Mutex<HashMap<ProbeKey, Vec<CliCommand>>>);
type ProbeKey = (AccountRef, Option<PathBuf>, String);

impl Probes {
    fn fresh(&self, key: &ProbeKey) -> Vec<CliCommand> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .cloned()
            .unwrap_or_default()
    }

    fn keep(&self, key: ProbeKey, commands: Vec<CliCommand>) {
        let mut cache = self.0.lock().unwrap_or_else(|p| p.into_inner());
        cache.retain(|known, _| known.0 != key.0 || known.1 != key.1 || known.2 == key.2);
        cache.insert(key, commands);
    }
}

/// Remote prompt discovery is supplied by the daemon gateway; never launch stdio servers here.
fn gateway_prompts(_account: &AccountRef, _project: Option<&Path>) -> Vec<CliCommand> {
    Vec::new()
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
        let registered: Vec<_> = self
            .core
            .catalog()
            .projects
            .into_iter()
            .flat_map(|project| {
                project
                    .repos
                    .into_iter()
                    .map(|repo| PathBuf::from(repo.path))
            })
            .collect();
        let rows = Catalog::read_rows(&roots, &registered);
        let choices = plugins::choices::Choices::load(&self.data_dir).map_err(Error::Invalid)?;
        let version = brigadier_registry::catalog::fingerprint(
            format!(
                "{}:{resolved:?}",
                plugins::CatalogView::new(&rows, &choices).version
            )
            .as_bytes(),
        );
        Ok((resolved, version))
    }

    pub(super) async fn conversation_skills(
        &self,
        id: &ConversationId,
        account: Option<&AccountRef>,
    ) -> Result<plugins::resolve::Resolved> {
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
        let (resolved, version) = self
            .command_resolution(&account, project.as_deref(), cwd.as_deref(), trusted)
            .await?;
        let key = (account.clone(), project.clone(), version);
        if account.provider == ProviderKind::Claude
            && let Some(cli) = cli
        {
            self.cli_command_probes.keep(
                key.clone(),
                live_prompts(account.provider, cli.session.commands().await),
            );
        }
        let mut prompts = self.cli_command_probes.fresh(&key);
        prompts.extend(gateway_prompts(&account, project.as_deref()));
        prompts.retain(|prompt| {
            prompt
                .source
                .as_ref()
                .is_some_and(|name| resolved.servers.iter().any(|server| &server.name == name))
        });
        let mut seen = std::collections::HashSet::new();
        prompts.retain(|prompt| seen.insert(prompt.invocation.clone()));
        Ok(menu(account.provider, &resolved, prompts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugins::resolve::{Resolved, ResolvedSkill};

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
    fn choice_catalog_account_and_project_changes_invalidate_enrichment() {
        let cache = Probes::default();
        let key = (
            AccountRef::own(ProviderKind::Claude),
            Some(PathBuf::from("/project")),
            "before".to_owned(),
        );
        cache.keep(key.clone(), vec![CliCommand::default()]);
        assert_eq!(cache.fresh(&key).len(), 1);
        let changed = (key.0.clone(), key.1.clone(), "after".to_owned());
        assert!(cache.fresh(&changed).is_empty());
        cache.keep(changed, Vec::new());
        assert!(cache.fresh(&key).is_empty());
        assert!(
            cache
                .fresh(&(
                    AccountRef::new(ProviderKind::Claude, Some("other".into())),
                    key.1.clone(),
                    key.2.clone()
                ))
                .is_empty()
        );
        assert!(
            cache
                .fresh(&(key.0, Some(PathBuf::from("/other")), key.2))
                .is_empty()
        );
    }
}
