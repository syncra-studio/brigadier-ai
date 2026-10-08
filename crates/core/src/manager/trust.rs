//! "Do you trust this folder?": asked once for each repository folder of a project.
//!
//! - **Trusted.** The CLIs' own trust prompts are answered for the folder in their settings
//!   ([`brigadier_providers::trust`]), so "Open in terminal" asks nothing: one entry for the
//!   repository's top folder covers its worktrees in both CLIs. A terminal that starts outside
//!   the checkout (a read-only Codex worker's scratch folder) gets an entry for that folder,
//!   owned by its task. Every entry is recorded in the cleanup ledger, with what it replaced,
//!   before it is written; Don't trust, the project's removal or the task's end puts back what
//!   was there. A CLI not set up on this machine is left alone.
//! - **Not trusted.** Every session on the folder runs under Ask for approval, an overnight
//!   run's too ([`SessionManager::permission`]); workers launch, resume and open in a
//!   terminal with Ask's access ([`SessionManager::effective_access`]). A worker at work above
//!   it is stopped, and the thread told why; an idle one resumes under Ask; the thread's own
//!   CLI restarts under Ask at its next turn.
//! - **Not asked yet** (a project from before this question): sessions run as their level
//!   says, and the app asks.

use std::path::Path;

use brigadier_providers::Artifact;
use brigadier_providers::trust::{self, TrustCli, Written};

use super::workers::access_for;
use super::{SessionManager, blocking};
use crate::model::{ConversationId, FolderTrustReport, PermissionLevel, Project, ProjectId, Setup};
use crate::work::{Task, WorkerAccess};
use crate::{Error, Result};

/// The ledger owner of the folder trust a project's answer wrote.
fn owner(project: &ProjectId) -> String {
    format!("trust:{project}")
}

/// How often a write starts over when the entry changed between its look and its write.
const ATTEMPTS: usize = 3;

impl SessionManager {
    /// The user's answer for the folder the session works in, if they were asked.
    pub(crate) fn repo_trust(&self, id: &ConversationId) -> Option<bool> {
        let conversation = self.core.conversation(id).ok()?;
        let Some(Setup::Session { repo, .. }) = &conversation.setup else {
            return None;
        };
        let project = self.core.project(conversation.project_id.as_ref()?).ok()?;
        project.trusts(repo)
    }

    /// The access a worker launches with: the task's, or Ask for approval's for its kind
    /// when the user doesn't trust the folder.
    pub(crate) fn effective_access(&self, task: &Task) -> WorkerAccess {
        if self.repo_trust(&task.conversation_id) == Some(false) {
            access_for(task.kind, PermissionLevel::AskForApproval)
        } else {
            task.access
        }
    }

    /// Records the user's answer for the project's folder `path` (its first repository when
    /// `None`) and applies it.
    pub async fn set_folder_trust(
        &self,
        id: ProjectId,
        path: Option<String>,
        trusted: bool,
    ) -> Result<FolderTrustReport> {
        let project = self.core.project(&id)?;
        let path = match path {
            Some(path) => path,
            None => project
                .repos
                .first()
                .map(|repo| repo.path.clone())
                .ok_or_else(|| Error::Invalid("the project has no folder".into()))?,
        };
        let project = self.core.set_folder_trust(&id, &path, trusted).await?;
        let failures = if trusted {
            self.write_cli_trust(&owner(&id), &path, &[TrustCli::Claude, TrustCli::Codex])
                .await
        } else {
            let failures = self.release_cli_trust(&project, &path).await;
            self.hold_to_ask(&project, &path).await;
            failures
        };
        tracing::info!(project = %id, path, trusted, ?failures, "folder trust decided");
        Ok(FolderTrustReport { project, failures })
    }

    /// After a restart: folders the user trusts have their CLI entries (one whose write was
    /// cut short is written now), and entries no answer stands behind any more are removed.
    pub(super) async fn reconcile_trust(&self) {
        let projects = self.core.catalog().projects;
        let ledger = self.runtime.ledger();
        for (holder, artifacts, _) in ledger.owners() {
            let Some(project) = holder.strip_prefix("trust:") else {
                continue;
            };
            let stale: Vec<Artifact> = artifacts
                .into_iter()
                .filter(|artifact| match artifact {
                    Artifact::CliTrust { folder, .. } => !projects
                        .iter()
                        .any(|p| p.id.0 == project && p.trusts(folder) == Some(true)),
                    _ => false,
                })
                .collect();
            if !stale.is_empty() {
                let left = ledger.release(&holder, stale).await;
                if !left.is_clean() {
                    tracing::warn!(owner = holder, failures = ?left.failures, "folder trust left in place");
                }
            }
        }
        for project in &projects {
            for folder in project.trust.iter().filter(|folder| folder.trusted) {
                let failures = self
                    .write_cli_trust(
                        &owner(&project.id),
                        &folder.path,
                        &[TrustCli::Claude, TrustCli::Codex],
                    )
                    .await;
                if !failures.is_empty() {
                    tracing::warn!(project = %project.id, folder = folder.path, ?failures, "folder trust not written");
                }
            }
        }
    }

    /// The project is going: the folder trust its answers wrote goes first. What can't be put
    /// back stays recorded (and is retried at the next start).
    pub(super) async fn forget_project_trust(&self, id: &ProjectId) -> Vec<String> {
        self.runtime.ledger().dispose(&owner(id)).await.failures
    }

    /// A terminal of a trusted folder's task that starts outside the checkout (a read-only
    /// Codex worker's scratch folder): `cli` trusts that folder too, until the task ends.
    pub(super) async fn trust_terminal_folder(&self, task: &Task, cli: TrustCli, cwd: &Path) {
        if self.repo_trust(&task.conversation_id) != Some(true) {
            return;
        }
        let failures = self
            .write_cli_trust(
                &format!("task:{}", task.id),
                &cwd.display().to_string(),
                &[cli],
            )
            .await;
        if !failures.is_empty() {
            tracing::warn!(task = %task.id, ?failures, "a terminal's folder trust was not written");
        }
    }

    /// The file `cli` keeps folder trust in, for the CLIs' environment.
    fn trust_file(&self, cli: TrustCli) -> Option<std::path::PathBuf> {
        #[cfg(test)]
        {
            let home = self.trust_home.get()?;
            let env = brigadier_providers::cli::CliEnv::from_vars([(
                std::ffi::OsString::from("HOME"),
                home.clone().into_os_string(),
            )]);
            cli.file(&env)
        }
        #[cfg(not(test))]
        cli.file(self.runtime.cli_env())
    }

    /// Writes `folder`'s trust in each of `clis`' settings, recorded under `holder` first.
    /// Returns what failed, by CLI.
    async fn write_cli_trust(&self, holder: &str, folder: &str, clis: &[TrustCli]) -> Vec<String> {
        let mut failures = Vec::new();
        for &cli in clis {
            let Some(file) = self.trust_file(cli) else {
                continue;
            };
            if !set_up(cli, &file) {
                continue;
            }
            if let Err(err) = self.write_one(holder, cli, &file, folder).await {
                failures.push(format!("{}: {err}", cli.label()));
            }
        }
        failures
    }

    async fn write_one(
        &self,
        holder: &str,
        cli: TrustCli,
        file: &Path,
        folder: &str,
    ) -> Result<()> {
        let ledger = self.runtime.ledger();
        for _ in 0..ATTEMPTS {
            let (path, name) = (file.to_owned(), folder.to_owned());
            let Some(before) =
                blocking(move || trust::plan(cli, &path, &name).map_err(provider_error)).await?
            else {
                return Ok(());
            };
            let artifact = Artifact::CliTrust {
                cli,
                file: file.display().to_string(),
                folder: folder.to_owned(),
                before: before.clone(),
            };
            ledger.record(holder, artifact.clone()).await?;
            let (path, name) = (file.to_owned(), folder.to_owned());
            let written =
                blocking(move || trust::write(cli, &path, &name, &before).map_err(provider_error))
                    .await;
            match written {
                Ok(Written::Trusted) => return Ok(()),
                // Trusted meanwhile by someone else, or changed: not Brigadier's to undo.
                Ok(Written::AlreadyTrusted) => {
                    ledger.unrecord(holder, artifact).await?;
                    return Ok(());
                }
                Ok(Written::Changed(_)) => ledger.unrecord(holder, artifact).await?,
                Err(err) => {
                    ledger.unrecord(holder, artifact).await?;
                    return Err(err);
                }
            }
        }
        Err(Error::Invalid(format!(
            "{} kept changing; try again",
            file.display()
        )))
    }

    /// Puts back what the trust of `path` replaced: the project's entry and its tasks'
    /// terminal folders. Returns what failed.
    async fn release_cli_trust(&self, project: &Project, path: &str) -> Vec<String> {
        let ledger = self.runtime.ledger();
        let mut holders = vec![(owner(&project.id), Some(path.to_owned()))];
        for id in self.sessions_on(project, path) {
            for task in self.core.tasks(&id).await.unwrap_or_default() {
                holders.push((format!("task:{}", task.id), None));
            }
        }
        let mut failures = Vec::new();
        for (holder, only) in holders {
            let entries: Vec<Artifact> = ledger
                .artifacts(&holder)
                .into_iter()
                .filter(|artifact| match artifact {
                    Artifact::CliTrust { folder, .. } => {
                        only.as_ref().is_none_or(|only| folder == only)
                    }
                    _ => false,
                })
                .collect();
            if !entries.is_empty() {
                failures.extend(ledger.release(&holder, entries).await.failures);
            }
        }
        failures
    }

    /// The project's sessions that work on `path`.
    fn sessions_on(&self, project: &Project, path: &str) -> Vec<ConversationId> {
        self.core
            .catalog()
            .conversations
            .into_iter()
            .filter(|conversation| conversation.project_id.as_ref() == Some(&project.id))
            .filter(|conversation| {
                matches!(&conversation.setup, Some(Setup::Session { repo, .. }) if repo == path)
            })
            .map(|conversation| conversation.id)
            .collect()
    }

    /// The folder is no longer trusted: a worker whose CLI runs above Ask for approval (its
    /// access was set when it launched) is stopped mid-turn, and its thread told why; an idle
    /// one's CLI closes, to resume under Ask. A terminal the user has open goes on: they are
    /// at it; it hands back under Ask.
    async fn hold_to_ask(&self, project: &Project, path: &str) {
        for id in self.sessions_on(project, path) {
            let saved = match self.core.conversation(&id).map(|c| c.setup) {
                Ok(Some(Setup::Session { permission, .. })) => permission,
                _ => continue,
            };
            let above = saved != PermissionLevel::AskForApproval
                || self.overnight.active.get(&id).is_some();
            for task in self.core.tasks(&id).await.unwrap_or_default() {
                if task.state.is_final() {
                    continue;
                }
                let Some(live) = self.existing_task_live(&task.id) else {
                    continue;
                };
                if live.cli().await.is_none() {
                    continue;
                }
                if !above && task.access == access_for(task.kind, PermissionLevel::AskForApproval) {
                    continue;
                }
                // Idle (a reported worker): its CLI closes as on hibernation, and its next
                // turn resumes the session under Ask.
                if !live.busy().await {
                    live.close_cli().await;
                    live.allow_revival().await;
                    continue;
                }
                match self.stop_task(task.id.clone()).await {
                    Ok(()) => {
                        if let Ok(conv) = self.conv(&id) {
                            conv.note(format!(
                                "[worker] task-{} \"{}\" was stopped: the user no longer trusts this folder, so all work here now runs under Ask for approval. Start it again if it is still needed.",
                                task.number, task.title
                            ))
                            .await;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(task = %task.id, error = %err, "could not stop a worker of an untrusted folder");
                    }
                }
            }
        }
    }
}

fn provider_error(err: brigadier_providers::Error) -> Error {
    Error::Provider(err.to_string())
}

/// Whether the CLI is set up here: Claude has its settings file, Codex its folder. A CLI
/// that isn't gets nothing written; the next start writes it once it is.
fn set_up(cli: TrustCli, file: &Path) -> bool {
    match cli {
        TrustCli::Claude => file.exists(),
        TrustCli::Codex => file.parent().is_some_and(Path::is_dir),
    }
}
