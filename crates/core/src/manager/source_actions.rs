//! The Source panel's own git, past staging and committing: merging a branch in and resolving
//! its conflicts file by file, aborting it, switching branches, pulling, pushing and fetching,
//! and writing a commit message into the box. Each is the user's own act on the checkout the
//! panel shows (a session's, or its project's folder before it starts); git's refusals come
//! back as errors in its own words.

use std::path::PathBuf;

use brigadier_git::Repo;

use super::git_actions::source_state_of;
use super::{SessionManager, blocking, git_error};
use crate::Result;
use crate::work::{ConflictFile, SourceAction, SourceBranch, SourceState, WorkspaceTarget};

impl SessionManager {
    /// The checkout a Source panel acts on: a session's own (it must exist), or a project's
    /// folder before its session starts.
    fn source_dir(&self, target: &WorkspaceTarget) -> Result<PathBuf> {
        match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                self.checkout(conversation_id).map(|(path, _)| path)
            }
            WorkspaceTarget::Project { project_id } => self.project_checkout(project_id),
        }
    }

    /// The branches to merge from or switch to: local ones, then remote ones, newest first.
    pub async fn source_branches(&self, target: &WorkspaceTarget) -> Result<Vec<SourceBranch>> {
        let path = self.source_dir(target)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            Ok(repo
                .branch_refs()
                .map_err(git_error)?
                .into_iter()
                .map(|branch| SourceBranch {
                    name: branch.name,
                    remote: branch.remote,
                    at_ms: branch.date.saturating_mul(1000),
                    author: branch.author,
                    subject: branch.subject,
                })
                .collect())
        })
        .await
    }

    /// One of the Source panel's git actions, then the checkout as it is after it. A merge or
    /// pull that stops on conflicts isn't an error: the state shows the merge in progress.
    pub async fn source_action(
        &self,
        target: &WorkspaceTarget,
        action: SourceAction,
    ) -> Result<SourceState> {
        self.admit()?;
        // Its landings and its finish go to the branch its setup names.
        if matches!(target, WorkspaceTarget::Conversation { .. })
            && matches!(action, SourceAction::Checkout { .. })
        {
            return Err(crate::Error::Invalid(
                "a session works on its own branch, so it can't switch to another".into(),
            ));
        }
        let _fence = match target {
            WorkspaceTarget::Conversation { conversation_id } => Some(self.enter(conversation_id)?),
            WorkspaceTarget::Project { .. } => None,
        };
        let path = self.source_dir(target)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            act(&repo, action).map_err(|err| match err {
                // What git said, as the user would read it in a terminal.
                brigadier_git::Error::Command { stderr, .. } if !stderr.is_empty() => {
                    crate::Error::Invalid(stderr)
                }
                other => git_error(other),
            })?;
            source_state_of(&repo)
        })
        .await
    }

    /// A conflicted file's versions and its two sides' names, for its conflict and merge
    /// editor tabs.
    pub async fn conflict_file(
        &self,
        target: &WorkspaceTarget,
        path: String,
    ) -> Result<ConflictFile> {
        let dir = self.source_dir(target)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            let file = repo.conflict_file(&path).map_err(git_error)?;
            let short = |rev: &str| {
                repo.read_only(&["rev-parse", "--verify", "--quiet", "--short", rev])
                    .map(|out| out.trim().to_owned())
                    .unwrap_or_default()
            };
            let branch = repo.state().map_err(git_error)?.current_branch;
            let incoming = repo
                .merging()
                .map_err(git_error)?
                .map(|merge| merge.incoming);
            let label = |commit: String, name: Option<String>| match name {
                Some(name) if !commit.is_empty() => format!("{commit} • {name}"),
                Some(name) => name,
                None => commit,
            };
            Ok(ConflictFile {
                text: file.text,
                current: file.current,
                incoming: file.incoming,
                merged: file.merged,
                current_label: label(short("HEAD"), branch),
                incoming_label: label(short("MERGE_HEAD"), incoming),
            })
        })
        .await
    }

    /// A commit message for what is staged, written into the panel's box without
    /// committing; with it, why the user's chosen model didn't write it, when it didn't.
    pub async fn write_source_message(
        &self,
        target: &WorkspaceTarget,
    ) -> Result<(String, Option<String>)> {
        self.admit()?;
        let path = self.source_dir(target)?;
        let project = match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                self.core.conversation(conversation_id)?.project_id
            }
            WorkspaceTarget::Project { project_id } => Some(project_id.clone()),
        };
        let patch = self.pending_patch(&path, false).await?;
        let (message, note) = self
            .write_commit_message(project.as_ref(), &path, &patch)
            .await?;
        let message = if self.core.settings().omit_ai_coauthors {
            brigadier_git::strip_ai_coauthors(&message)
        } else {
            message
        };
        Ok((message, note))
    }
}

fn act(repo: &Repo, action: SourceAction) -> brigadier_git::Result<()> {
    let branch = || {
        repo.state()?
            .current_branch
            .ok_or_else(|| brigadier_git::Error::Invalid("the checkout is not on a branch".into()))
    };
    match action {
        SourceAction::Merge { branch } => repo.merge_branch(&branch).map(drop),
        SourceAction::AbortMerge => repo.abort_merge(),
        SourceAction::Checkout { branch } => repo.switch_branch(&branch),
        SourceAction::Pull => repo.pull().map(drop),
        SourceAction::Push => repo.push(&branch()?),
        SourceAction::Fetch => repo.fetch(),
        SourceAction::Resolve {
            path,
            expected,
            text,
            stage,
        } => repo.resolve_conflict(&path, expected.as_deref(), &text, stage),
    }
}
