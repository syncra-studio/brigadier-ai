//! Uninstalling Brigadier, the session manager's part: the branches it would leave in the
//! user's repositories, and the teardown. Nothing new starts once the teardown begins; every
//! conversation winds down as archiving does (workers stopped, their unfinished work kept as
//! WIP commits, CLI sessions ended), then everything the cleanup ledger records goes:
//! worktrees (through git, their uncommitted changes committed first), scratch folders, CLI
//! session files and Codex threads and trust entries, processes. A worktree whose changes
//! can't be kept stays, with everything its owner holds.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use brigadier_providers::Artifact;

use super::disk::keep_changes;
use super::project_removal::{BranchRecord, branch_records, settle_branches, survey};
use super::{SessionManager, blocking, brains};
use crate::Result;
use crate::model::{Environment, Lifecycle, Setup};
use crate::storage::{BranchChoice, RemovalBranch, RemovalWorktree};

/// What the teardown left behind.
#[derive(Debug, Default)]
pub struct TearDown {
    /// Worktrees kept because their uncommitted changes couldn't be, with why.
    pub kept_worktrees: Vec<String>,
    pub failures: Vec<String>,
}

impl SessionManager {
    /// Every worktree Brigadier made, and every Brigadier branch in the user's repositories as
    /// it will stand once those worktrees are gone.
    pub async fn uninstall_survey(&self) -> Result<(Vec<RemovalWorktree>, Vec<RemovalBranch>)> {
        let catalog = self.core.catalog();
        let mut records: Vec<BranchRecord> = Vec::new();
        let mut worktrees: Vec<PathBuf> = self
            .runtime
            .ledger()
            .owners()
            .into_iter()
            .flat_map(|(_, artifacts, _)| artifacts)
            .filter_map(|artifact| match artifact {
                Artifact::Worktree { path, .. } => Some(PathBuf::from(path)),
                _ => None,
            })
            .collect();
        for conversation in &catalog.conversations {
            let board = self.core.board(&conversation.id).await.unwrap_or_default();
            let tasks = board.sorted_tasks();
            records.extend(branch_records(conversation, &tasks, board.runs.values()));
            if let Some(Setup::Session {
                environment:
                    Environment::NewWorktree {
                        path: Some(path), ..
                    },
                ..
            }) = &conversation.setup
            {
                worktrees.push(PathBuf::from(path));
            }
            worktrees.extend(
                tasks
                    .iter()
                    .filter_map(|task| task.workspace.as_ref()?.worktree.as_ref())
                    .map(PathBuf::from),
            );
        }
        records.extend(
            self.kept_branches()
                .await?
                .into_iter()
                .map(|(repo, kept)| BranchRecord::kept(repo, kept)),
        );
        let git = self.git.clone();
        blocking(move || Ok(survey(&git, worktrees, records))).await
    }

    /// Stops everything and removes what every conversation created. Transcripts and the
    /// rest of the data directory stay; the daemon's caller decides about those.
    pub async fn tear_down(&self) -> TearDown {
        let mut outcome = TearDown::default();
        self.admitting.store(false, Ordering::Release);
        self.brains.jobs.stop("Brigadier is being uninstalled");
        brains::stop_watchers(self.brains.shutdown()).await;
        for conversation in self.core.catalog().conversations {
            if conversation.lifecycle != Lifecycle::Archived {
                self.wind_down(&conversation).await;
            }
        }
        for (owner, artifacts, _) in self.runtime.ledger().owners() {
            let worktrees: Vec<PathBuf> = artifacts
                .iter()
                .filter_map(|artifact| match artifact {
                    Artifact::Worktree { path, .. } => Some(PathBuf::from(path)),
                    _ => None,
                })
                .filter(|path| path.exists())
                .collect();
            let git = self.git.clone();
            let checked = worktrees.clone();
            let unkept = blocking(move || {
                Ok(checked
                    .into_iter()
                    .filter_map(|path| {
                        keep_changes(&git, &path)
                            .err()
                            .map(|err| format!("{}: {err}", path.display()))
                    })
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
            if !unkept.is_empty() {
                outcome.kept_worktrees.extend(unkept);
                continue;
            }
            let leftovers = self.runtime.ledger().dispose(&owner).await;
            outcome.failures.extend(leftovers.failures);
            // A worktree git couldn't remove stays, and so does the folder holding it: only git
            // removes worktrees.
            outcome.kept_worktrees.extend(
                worktrees
                    .iter()
                    .filter(|path| path.exists())
                    .map(|path| format!("{}: git couldn't remove it", path.display())),
            );
        }
        outcome
    }

    /// Deletes the picked branches of [`Self::uninstall_survey`]; the ones left, as they
    /// stand now, and what failed.
    pub async fn settle_uninstall_branches(
        &self,
        branches: Vec<RemovalBranch>,
        picked: Vec<BranchChoice>,
    ) -> (Vec<RemovalBranch>, Vec<String>) {
        let git = self.git.clone();
        blocking(move || Ok(settle_branches(&git, branches, &picked)))
            .await
            .unwrap_or_else(|err| (Vec::new(), vec![err.to_string()]))
    }
}
