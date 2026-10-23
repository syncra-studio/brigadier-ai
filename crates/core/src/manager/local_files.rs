//! The thread's changes on the user's own checkout, where it leaves them uncommitted (UI audit
//! M6): snapshots of the checkout's files when each of its requests starts and as each of its
//! turns ends ([`brigadier_git::Repo::snapshot_files`]: commits no ref names, written without the
//! user's index, HEAD, refs or files). Recorded, so a restart keeps them; the thread's self-edit
//! metric counts its uncommitted change from them ([`super::thread_metrics`]). Plain
//! bookkeeping: nothing starts.

use std::path::Path;

use brigadier_git::Oid;

use super::{SessionManager, blocking, git_error};
use crate::board::Authored;
use crate::model::{ConversationId, DomainEvent, Environment, Setup};

impl SessionManager {
    /// The session's local checkout, when its thread works there: its repository and branch.
    pub(crate) fn local_checkout(
        &self,
        id: &ConversationId,
    ) -> Option<(std::path::PathBuf, String)> {
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
    async fn snapshot_local(&self, repo: &Path) -> crate::Result<Oid> {
        let (git, repo) = (self.git.clone(), repo.to_owned());
        blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .snapshot_files()
                .map_err(git_error)
        })
        .await
    }

    /// The snapshots recorded for `request` on `branch`: its start and the last one.
    async fn recorded_files(
        &self,
        id: &ConversationId,
        request: &str,
        branch: &str,
    ) -> Option<(String, String)> {
        self.core
            .board(id)
            .await
            .ok()?
            .thread_authored
            .iter()
            .rev()
            .find_map(|known| match known {
                Authored::Files {
                    request_id,
                    branch: on,
                    start,
                    tip,
                } if request_id == request && on == branch => Some((start.clone(), tip.clone())),
                _ => None,
            })
    }

    async fn store_files(
        &self,
        id: &ConversationId,
        request: &str,
        branch: &str,
        start: &str,
        tip: &str,
    ) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::ThreadFilesSeen {
                    conversation_id: id.clone(),
                    request_id: request.to_owned(),
                    branch: branch.to_owned(),
                    start: start.to_owned(),
                    tip: tip.to_owned(),
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record the thread's changes on the local checkout");
        }
    }

    /// A turn of `request` starts on a local checkout: the request's start is recorded as a
    /// snapshot of the checkout's files, once, so "since the request started" is exact.
    pub(crate) async fn note_request_start(&self, id: &ConversationId, request: Option<&str>) {
        let (Some(request), Some((repo, branch))) = (request, self.local_checkout(id)) else {
            return;
        };
        if self.recorded_files(id, request, &branch).await.is_some() {
            return;
        }
        let start = match self.snapshot_local(&repo).await {
            Ok(start) => start,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        self.store_files(id, request, &branch, &start.0, &start.0)
            .await;
    }

    /// The end of a thread turn on a local checkout: the request's record moves to a snapshot
    /// of the checkout's files as they stand, uncommitted (the thread's self-edit metric counts
    /// them).
    pub(crate) async fn record_local_changes(&self, id: &ConversationId) {
        let Some((repo, branch)) = self.local_checkout(id) else {
            return;
        };
        let Some(request) = self.request_for(id).await else {
            return;
        };
        let Some((start, tip)) = self.recorded_files(id, &request, &branch).await else {
            return;
        };
        let now = match self.snapshot_local(&repo).await {
            Ok(now) => now,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        if tip != now.0 {
            self.store_files(id, &request, &branch, &start, &now.0)
                .await;
        }
    }
}
