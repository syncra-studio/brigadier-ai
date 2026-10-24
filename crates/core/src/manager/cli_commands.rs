//! What a conversation's CLI runs from the composer's `/` menu: its built-in commands, skills,
//! plugin commands and MCP prompts, as the CLI itself reports them ([`CliCommand`]). A
//! conversation whose CLI runs is asked through its session; one whose CLI hasn't started (a
//! new conversation, or one at rest) through its provider, in the folder its CLI would start in.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use brigadier_providers::{CliCommand, ProviderKind};

use super::SessionManager;
use crate::accounts::AccountRef;
use crate::model::{ConversationId, ProjectId, Setup};
use crate::{Error, Result};

/// How long a provider's answer for a folder is kept: each one starts the CLI.
const PROBE_FRESH: Duration = Duration::from_secs(120);

/// The providers' answers for folders no session runs in, by account, folder and trust.
#[derive(Default)]
pub(super) struct Probes(Mutex<HashMap<ProbeKey, (Instant, Vec<CliCommand>)>>);

type ProbeKey = (AccountRef, Option<PathBuf>, bool);

impl Probes {
    fn fresh(&self, key: &ProbeKey) -> Option<Vec<CliCommand>> {
        let probes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        probes
            .get(key)
            .filter(|(at, _)| at.elapsed() < PROBE_FRESH)
            .map(|(_, commands)| commands.clone())
    }

    fn keep(&self, key: ProbeKey, commands: Vec<CliCommand>) {
        let mut probes = self.0.lock().unwrap_or_else(|p| p.into_inner());
        probes.retain(|_, (at, _)| at.elapsed() < PROBE_FRESH);
        probes.insert(key, (Instant::now(), commands));
    }
}

impl SessionManager {
    /// What conversation `id`'s CLI runs from a message, or with no conversation what a new
    /// one's would: `provider`'s CLI, in project `project`'s folder `repo` (its first one when
    /// absent) for a new session.
    pub async fn cli_commands(
        &self,
        id: Option<ConversationId>,
        provider: Option<ProviderKind>,
        project: Option<ProjectId>,
        repo: Option<String>,
    ) -> Result<Vec<CliCommand>> {
        let Some(id) = id else {
            let provider = provider.ok_or_else(|| {
                Error::Invalid("a new conversation's commands need its CLI".into())
            })?;
            let account = self.runtime.launch_account(provider, None);
            let (folder, trusted) = match project {
                Some(project) => {
                    let project = self.core.project(&project)?;
                    let repo = repo
                        .or_else(|| project.repos.first().map(|repo| repo.path.clone()))
                        .ok_or_else(|| Error::Invalid("the project has no repository".into()))?;
                    let trusted = project.counts_trusted(&repo);
                    (Some(PathBuf::from(repo)), trusted)
                }
                None => (None, false),
            };
            return self.probe_commands(account, folder, trusted).await;
        };
        let live = self.convs_lock().get(&id).cloned();
        if let Some(cli) = match live {
            Some(conv) => conv.cli().await,
            None => None,
        } {
            let commands = cli.session.commands().await;
            if !commands.is_empty() {
                return Ok(commands);
            }
        }
        let conversation = self.core.conversation(&id)?;
        let choice = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator.clone(),
            Some(Setup::Chat { model, .. }) => model.clone(),
            None => self.default_chat_choice(),
        };
        let account = self.runtime.account_for(&choice);
        // A session's CLI starts in its checkout; a Chat's in a folder of Brigadier's own.
        let folder = match &conversation.setup {
            Some(Setup::Session { repo, .. }) => Some(PathBuf::from(
                self.checkout_dir(&id)
                    .ok()
                    .filter(|checkout| Path::new(checkout).is_dir())
                    .unwrap_or_else(|| repo.clone()),
            )),
            _ => None,
        };
        let trusted = self.repo_trust(&id) == Some(true);
        self.probe_commands(account, folder, trusted).await
    }

    /// [`brigadier_providers::Provider::commands`] on `account`, kept a while per folder.
    async fn probe_commands(
        &self,
        account: AccountRef,
        folder: Option<PathBuf>,
        trusted: bool,
    ) -> Result<Vec<CliCommand>> {
        let key = (account, folder, trusted);
        if let Some(commands) = self.cli_command_probes.fresh(&key) {
            return Ok(commands);
        }
        let provider = self.runtime.provider_for(&key.0)?;
        let commands = provider
            .commands(key.1.clone(), trusted)
            .await
            .map_err(|err| Error::Provider(err.to_string()))?;
        self.cli_command_probes.keep(key, commands.clone());
        Ok(commands)
    }
}
