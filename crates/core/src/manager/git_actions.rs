//! The pinned card's Git actions: the user's own commit and push in a session's
//! checkout. They are never automatic (PLAN §4); a commit waits for any landing on the same
//! repository (the landing lock). A blank commit message is written by the cheapest model of
//! the session's provider, in a throwaway CLI session that is disposed of right after.

use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_git::{Oid, Repo, SourceEntry, SourceKind};
use brigadier_providers::{
    Access, Origin, ProviderEvent, ProviderKind, Role, SessionSpec, Started, ToolSet, TurnInput,
};

use super::landing::diff_stat_of;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, Setup};
use crate::work::{CommitOutcome, GitState, SourceFile, SourceScope, SourceState, SourceStatus};
use crate::{Error, Result};

/// How much of the patch the message writer reads.
const PATCH_CHARS: usize = 12_000;
/// How long the message writer may take before the commit gives up.
const WRITE_TIMEOUT: Duration = Duration::from_secs(90);

const WRITER_PROMPT: &str = "You write git commit messages. Reply with the message only: a \
subject line in the imperative mood of at most 72 characters, then, only when the change needs \
it, a blank line and a short body wrapped at 72 characters. No quotes, no code fences, no \
trailers.";

impl SessionManager {
    /// The session's checkout: its path and the provider its orchestrator runs on.
    fn checkout(&self, id: &ConversationId) -> Result<(PathBuf, ProviderKind)> {
        let Some(Setup::Session {
            repo,
            environment,
            orchestrator,
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid("only a session has a checkout".into()));
        };
        let path = match environment {
            Environment::LocalCheckout { .. } => repo,
            Environment::NewWorktree { path, .. } => path
                .ok_or_else(|| Error::Invalid("the session's worktree doesn't exist yet".into()))?,
        };
        Ok((PathBuf::from(path), orchestrator.provider))
    }

    /// What the commit popover shows: the checkout's branch, what is staged and uncommitted,
    /// and where it pushes.
    pub async fn git_state(&self, id: &ConversationId) -> Result<GitState> {
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo.state().map_err(git_error)?.current_branch;
            let trees = repo.checkout_trees().map_err(git_error)?;
            let staged = repo
                .diff_stat(&trees.head_tree, &trees.staged)
                .map_err(git_error)?;
            let uncommitted = repo
                .diff_stat(&trees.head_tree, &trees.files)
                .map_err(git_error)?;
            let remote = match &branch {
                Some(branch) => Some(repo.remote_state(branch).map_err(git_error)?),
                None => None,
            };
            Ok(GitState {
                branch,
                staged: diff_stat_of(&staged),
                uncommitted: diff_stat_of(&uncommitted),
                remote: remote.as_ref().and_then(|state| state.remote.clone()),
                upstream: remote.as_ref().and_then(|state| state.upstream.clone()),
                ahead: remote.map_or(0, |state| state.ahead),
            })
        })
        .await
    }

    /// What the Source panel lists: the checkout's staged and unstaged files, and where its
    /// branch pushes.
    pub async fn source_state(&self, id: &ConversationId) -> Result<SourceState> {
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || source_state_of(&git.open(&path).map_err(git_error)?)).await
    }

    /// The user's staging of the listed changes (every change with `None`).
    pub async fn stage_files(
        &self,
        id: &ConversationId,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        self.change_source(id, move |repo| repo.stage(paths.as_deref()))
            .await
    }

    /// The user's unstaging of the listed staged files (every one with `None`).
    pub async fn unstage_files(
        &self,
        id: &ConversationId,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        self.change_source(id, move |repo| repo.unstage(paths.as_deref()))
            .await
    }

    /// The user's Discard of the listed files on one side (all of that side with `None`).
    pub async fn discard_files(
        &self,
        id: &ConversationId,
        scope: SourceScope,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        let staged = scope == SourceScope::Staged;
        self.change_source(id, move |repo| repo.discard(staged, paths.as_deref()))
            .await
    }

    /// Runs one of the Source panel's changes in the checkout, then reads it again.
    async fn change_source(
        &self,
        id: &ConversationId,
        change: impl FnOnce(&Repo) -> brigadier_git::Result<()> + Send + 'static,
    ) -> Result<SourceState> {
        self.admit()?;
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            change(&repo).map_err(git_error)?;
            source_state_of(&repo)
        })
        .await
    }

    /// The user's commit of the checkout's changes (with `include_unstaged`, every change),
    /// then, with `push`, a push of the branch. A blank `message` is written for them.
    pub async fn commit_changes(
        &self,
        id: &ConversationId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
    ) -> Result<CommitOutcome> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, provider) = self.checkout(id)?;
        let message = match message.map(|text| text.trim().to_owned()) {
            Some(text) if !text.is_empty() => text,
            _ => {
                let patch = self.pending_patch(&path, include_unstaged).await?;
                self.write_commit_message(id, provider, &patch).await?
            }
        };
        let message = if self.core.settings().omit_ai_coauthors {
            brigadier_git::strip_ai_coauthors(&message)
        } else {
            message
        };
        let git = self.git.clone();
        let text = message.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            let commit = repo
                .commit_changes(&text, include_unstaged)
                .map_err(git_error)?;
            if push {
                repo.push(&branch).map_err(git_error)?;
            }
            Ok(CommitOutcome {
                commit: commit.0,
                message: text,
                branch,
                pushed: push,
            })
        })
        .await
    }

    /// The user's push of the checkout's branch.
    pub async fn push_changes(&self, id: &ConversationId) -> Result<String> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            repo.push(&branch).map_err(git_error)?;
            Ok(branch)
        })
        .await
    }

    /// The patch a commit would record, for the message writer.
    async fn pending_patch(&self, path: &Path, include_unstaged: bool) -> Result<String> {
        let git = self.git.clone();
        let path = path.to_owned();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let trees = repo.checkout_trees().map_err(git_error)?;
            let to: Oid = if include_unstaged {
                trees.files
            } else {
                trees.staged
            };
            if to == trees.head_tree {
                return Err(Error::Invalid("there are no changes to commit".into()));
            }
            repo.review_patch(&trees.head_tree, &to, 3, false)
                .map_err(git_error)
        })
        .await
    }

    /// A commit message for `patch`, from the cheapest model of `provider` in a throwaway
    /// session with no tools.
    async fn write_commit_message(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        patch: &str,
    ) -> Result<String> {
        let (model, effort) = self.cheapest(provider)?;
        let run = uuid::Uuid::now_v7().to_string();
        let owner = format!("gen:{run}");
        let dir = self.owned_dir("scratch", &run);
        tokio::fs::create_dir_all(&dir).await.map_err(|err| {
            Error::Invalid(format!("could not prepare the message writer: {err}"))
        })?;
        let spec = SessionSpec {
            cwd: dir.clone(),
            model: Some(model),
            effort,
            fast: false,
            origin: Origin::New,
            access: Access::ReadOnly,
            append_system_prompt: Some(WRITER_PROMPT.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: true,
            allowed_models: None,
            auto_review: false,
            omit_ai_coauthors: false,
        };
        let shown: String = patch.chars().take(PATCH_CHARS).collect();
        let cut = if shown.len() < patch.len() {
            "\n[the rest of the diff is cut]"
        } else {
            ""
        };
        let prompt = format!("Write the commit message for this diff:\n\n{shown}{cut}");
        let written = async {
            let Started {
                session,
                mut events,
            } = self.runtime.start_hosted(&owner, provider, spec).await?;
            let reply = async {
                session.send(TurnInput::text(prompt)).await.map_err(|err| {
                    Error::Provider(format!("the message writer didn't start: {err}"))
                })?;
                let mut text = None;
                while let Some(event) = events.recv().await {
                    match event {
                        ProviderEvent::Message {
                            role: Role::Assistant,
                            text: reply,
                            ..
                        } => text = Some(reply),
                        ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => break,
                        _ => {}
                    }
                }
                text.ok_or_else(|| Error::Provider("the message writer replied nothing".into()))
            };
            let reply = tokio::time::timeout(WRITE_TIMEOUT, reply)
                .await
                .unwrap_or_else(|_| {
                    Err(Error::Provider("the message writer took too long".into()))
                });
            session.close().await;
            reply
        }
        .await;
        let _ = self.runtime.ledger().dispose(&owner).await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        let message = clean_message(&written?);
        if message.is_empty() {
            tracing::warn!(conversation = %id, "the message writer's reply was empty");
            return Err(Error::Provider("the message writer replied nothing".into()));
        }
        Ok(message)
    }

    /// The provider's cheapest model for a one-off chore, at low effort where it has efforts.
    /// Only a model the user left available: when its list has none, the chore isn't done.
    pub(super) fn cheapest(&self, provider: ProviderKind) -> Result<(String, Option<String>)> {
        let family = match provider {
            ProviderKind::Claude => "haiku",
            ProviderKind::Codex => "luna",
        };
        let settings = self.core.settings();
        let available =
            |id: &str| crate::routing::availability::model_available(&settings, provider, id);
        let models = self
            .runtime
            .overview(provider)
            .and_then(|overview| overview.models)
            .map(|catalog| catalog.models)
            .unwrap_or_default();
        // Before its list is read, the family's alias stands in for it.
        let model = if models.is_empty() {
            available(family).then(|| family.to_owned())
        } else {
            models
                .iter()
                .find(|model| model.id.contains(family) && available(&model.id))
                .or_else(|| {
                    models
                        .iter()
                        .find(|model| !model.legacy && available(&model.id))
                })
                .map(|model| model.id.clone())
        };
        let model = model.ok_or_else(|| {
            Error::Invalid(format!(
                "{} has no model available. Turn one on in Settings › Providers.",
                provider.label()
            ))
        })?;
        let effort = (provider == ProviderKind::Codex).then(|| "low".to_owned());
        Ok((model, effort))
    }
}

/// A checkout's Source panel state.
fn source_state_of(repo: &Repo) -> Result<SourceState> {
    let changes = repo.source_changes().map_err(git_error)?;
    let branch = repo.state().map_err(git_error)?.current_branch;
    let remote = match &branch {
        Some(branch) => Some(repo.remote_state(branch).map_err(git_error)?),
        None => None,
    };
    let files = |entries: Vec<SourceEntry>| {
        entries
            .into_iter()
            .map(|entry| SourceFile {
                path: entry.path,
                old_path: entry.old_path,
                status: match entry.kind {
                    SourceKind::Modified => SourceStatus::Modified,
                    SourceKind::Added => SourceStatus::Added,
                    SourceKind::Deleted => SourceStatus::Deleted,
                    SourceKind::Renamed => SourceStatus::Renamed,
                    SourceKind::Copied => SourceStatus::Copied,
                    SourceKind::TypeChanged => SourceStatus::TypeChanged,
                    SourceKind::Untracked => SourceStatus::Untracked,
                    SourceKind::Conflicted => SourceStatus::Conflicted,
                },
            })
            .collect()
    };
    Ok(SourceState {
        branch,
        remote: remote.as_ref().and_then(|state| state.remote.clone()),
        upstream: remote.as_ref().and_then(|state| state.upstream.clone()),
        ahead: remote.map_or(0, |state| state.ahead),
        staged: files(changes.staged),
        changes: files(changes.changes),
    })
}

/// The writer's reply as a commit message: no code fences or surrounding quotes, and no
/// trailing blank lines.
fn clean_message(reply: &str) -> String {
    let lines: Vec<&str> = reply
        .trim()
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    lines
        .join("\n")
        .trim()
        .trim_matches(|c| c == '"' || c == '`')
        .trim()
        .to_owned()
}
