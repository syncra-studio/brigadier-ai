//! A worker's changes in the Source panel and in the Review tab: what its branch changed since
//! it left the branch it merges into, committed or not, read in its own worktree. It only
//! reads: the worker writes there.

use std::collections::BTreeSet;
use std::path::Path;

use brigadier_git::{Oid, Repo};

use super::review::review_files;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, TaskId};
use crate::work::WorkerSource;
use crate::{Error, Result};

/// Where a worker works: its worktree, its branch and the branch it merges into.
pub(super) struct WorkerCheckout {
    pub path: String,
    pub branch: Option<String>,
    pub base: String,
}

/// A worker's range in its worktree `repo`: from where it left `base` (or the base's upstream,
/// when newer) to its files as they stand, with the files git doesn't track yet.
pub(super) fn worker_range(repo: &Repo, base: &str) -> Result<(Oid, Oid, BTreeSet<String>)> {
    let from = repo.worker_base(base).map_err(git_error)?;
    let trees = repo.checkout_trees().map_err(git_error)?;
    Ok((from, trees.files, trees.untracked.into_iter().collect()))
}

impl SessionManager {
    /// Where the conversation's worker `task_id` works, while it has a worktree. Its base is
    /// the one its +N −N counts from: the branch it targets, else the session's.
    pub(super) async fn worker_checkout(
        &self,
        id: &ConversationId,
        task_id: &TaskId,
    ) -> Result<WorkerCheckout> {
        let board = self.core.board(id).await?;
        let task = board
            .tasks
            .get(task_id)
            .ok_or_else(|| Error::NotFound(format!("worker {task_id}")))?;
        let workspace = task.workspace.as_ref();
        let path = workspace
            .and_then(|workspace| workspace.worktree.clone())
            .ok_or_else(|| Error::Invalid("this worker has no worktree".into()))?;
        let base = workspace
            .and_then(|workspace| workspace.target.clone())
            .or_else(|| self.changes_checkout(id).and_then(|(_, branch)| branch))
            .ok_or_else(|| Error::Invalid("this worker has no branch to merge into".into()))?;
        Ok(WorkerCheckout {
            path,
            branch: workspace.and_then(|workspace| workspace.branch.clone()),
            base,
        })
    }

    /// The files the conversation's worker `task_id` changed, as its +N −N counts them.
    pub async fn worker_source(
        &self,
        id: &ConversationId,
        task_id: &TaskId,
    ) -> Result<WorkerSource> {
        let WorkerCheckout { path, branch, base } = self.worker_checkout(id, task_id).await?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(Path::new(&path)).map_err(git_error)?;
            let (from, to, untracked) = worker_range(&repo, &base)?;
            let (files, insertions, deletions) = review_files(&repo, &from, &to, &untracked)?;
            Ok(WorkerSource {
                branch,
                base,
                files,
                insertions,
                deletions,
            })
        })
        .await
    }
}
