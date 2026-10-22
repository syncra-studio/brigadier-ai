//! The thread's own changes for a request, recorded together (BUILD-PLAN B2): plain
//! bookkeeping, which starts no review and no check. Reviews are the agents' call
//! (`request_review`).
//!
//! - **The unreviewed range** ([`UnreviewedRange`]) holds every change the thread made for its
//!   newest request since the last one a review covered. In a worktree it runs from the commit
//!   before the first of them to the last commit looked at; on a local checkout, where the
//!   thread leaves its changes uncommitted, from a snapshot of the checkout's files when the
//!   request started (or after the last review) to a snapshot of them now
//!   ([`brigadier_git::Repo::snapshot_files`]: a commit no ref names, written without the
//!   user's index, HEAD, refs or files). It is recorded, so a restart keeps it, and the
//!   thread's self-edit metric reads a local checkout's from it.
//! - **A review** (`request_review`) covers the range up to the commit it reads: it starts
//!   again after it.

use std::path::Path;

use brigadier_git::Oid;

use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent, Environment, Setup};
use crate::work::UnreviewedRange;

/// A thread check under way: the tree of the files it started on, which its commit and its
/// fixes are measured against.
pub(crate) struct CheckUnderWay {
    tree: Option<String>,
}

impl CheckUnderWay {
    /// The tree of the files the check started on, when git could tell.
    pub(crate) fn tree(&self) -> Option<&str> {
        self.tree.as_deref()
    }
}

impl SessionManager {
    /// The thread's `run_check` in the checkout at `tree` starts: the tree of its files then.
    pub(crate) async fn thread_check_started(&self, tree: Option<&Path>) -> CheckUnderWay {
        let files = match tree {
            Some(tree) => {
                let (git, tree) = (self.git.clone(), tree.to_owned());
                blocking(move || {
                    git.open_worktree(&tree)
                        .and_then(|worktree| worktree.checkout_tree())
                        .map_err(git_error)
                })
                .await
                .ok()
                .map(|tree| tree.0)
            }
            None => None,
        };
        CheckUnderWay { tree: files }
    }

    /// The session's local checkout, when its thread works there: its repository and branch.
    pub(crate) fn local_checkout(
        &self,
        id: &ConversationId,
    ) -> Option<(std::path::PathBuf, String)> {
        if self
            .overnight
            .active
            .get(id)
            .and_then(|active| active.workspace)
            .is_some()
        {
            return None;
        }
        match self.core.conversation(id).ok()?.setup? {
            Setup::Session {
                repo,
                environment: Environment::LocalCheckout { branch },
                ..
            } => Some((repo.into(), branch)),
            _ => None,
        }
    }

    /// A snapshot of the local checkout `repo`'s files as they stand.
    pub(crate) async fn snapshot_local(&self, repo: &Path) -> crate::Result<Oid> {
        let (git, repo) = (self.git.clone(), repo.to_owned());
        blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .snapshot_files()
                .map_err(git_error)
        })
        .await
    }

    async fn store_unreviewed(&self, id: &ConversationId, range: Option<UnreviewedRange>) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::UnreviewedChanged {
                    conversation_id: id.clone(),
                    range,
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record the thread's unreviewed changes");
        }
    }

    /// The range of `request` on `branch` as recorded, if it is that one.
    async fn recorded_range(
        &self,
        id: &ConversationId,
        request: &str,
        branch: &str,
    ) -> Option<UnreviewedRange> {
        self.core
            .board(id)
            .await
            .ok()?
            .unreviewed
            .map(|range| *range)
            .filter(|range| range.request_id == request && range.branch == branch)
    }

    /// The thread committed `base..tip` on `branch` for its running request: they join the
    /// request's unreviewed range. Commits that don't follow the range (a landing or the user's
    /// own commits came between) start it again from `base`.
    pub(crate) async fn thread_commits_unreviewed(
        &self,
        id: &ConversationId,
        branch: &str,
        base: &Oid,
        tip: &Oid,
    ) {
        let request = self.request_for(id, None).await.unwrap_or_default();
        let mut range = match self.recorded_range(id, &request, branch).await {
            Some(range) if !range.local && range.tip == base.0 => range,
            _ => UnreviewedRange {
                request_id: request,
                branch: branch.to_owned(),
                base: base.0.clone(),
                tip: base.0.clone(),
                local: false,
                start: None,
            },
        };
        range.tip = tip.0.clone();
        self.store_unreviewed(id, Some(range)).await;
    }

    /// `request_review` reviews up to `tip`: the thread's unreviewed range up to there is
    /// covered, and starts again after it.
    pub(crate) async fn mark_reviewed_to(&self, id: &ConversationId, tip: &Oid) {
        let Some(mut range) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.unreviewed)
            .map(|range| *range)
            .filter(|range| !range.local && range.tip == tip.0)
        else {
            return;
        };
        range.base = tip.0.clone();
        self.store_unreviewed(id, Some(range)).await;
    }

    /// `branch` moved to `tip` past what the thread made (a landing, the user's own commits, a
    /// rewritten history): its range starts again from there.
    pub(crate) async fn unreviewed_moved(&self, id: &ConversationId, branch: &str, tip: &Oid) {
        let Some(mut range) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.unreviewed)
            .map(|range| *range)
            .filter(|range| range.branch == branch && !range.local && range.tip != tip.0)
        else {
            return;
        };
        range.base = tip.0.clone();
        range.tip = tip.0.clone();
        self.store_unreviewed(id, Some(range)).await;
    }

    /// A turn of `request` starts on a local checkout: the request's start is recorded as a
    /// snapshot of the checkout's files, once, so "since the request started" is exact.
    pub(crate) async fn note_request_start(&self, id: &ConversationId, request: Option<&str>) {
        let (Some(request), Some((repo, branch))) = (request, self.local_checkout(id)) else {
            return;
        };
        if self.recorded_range(id, request, &branch).await.is_some() {
            return;
        }
        let start = match self.snapshot_local(&repo).await {
            Ok(start) => start,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        self.store_unreviewed(
            id,
            Some(UnreviewedRange {
                request_id: request.to_owned(),
                branch,
                base: start.0.clone(),
                tip: start.0.clone(),
                local: true,
                start: Some(start.0),
            }),
        )
        .await;
    }

    /// The end of a thread turn on a local checkout: the request's range moves to a snapshot of
    /// the checkout's files as they stand, uncommitted (the thread's self-edit metric counts
    /// them).
    pub(crate) async fn record_local_changes(&self, id: &ConversationId) {
        let Some((repo, branch)) = self.local_checkout(id) else {
            return;
        };
        let Some(request) = self.request_for(id, None).await else {
            return;
        };
        let Some(mut range) = self.recorded_range(id, &request, &branch).await else {
            return;
        };
        let now = match self.snapshot_local(&repo).await {
            Ok(now) => now,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        if range.tip != now.0 {
            range.tip = now.0;
            self.store_unreviewed(id, Some(range)).await;
        }
    }
}
