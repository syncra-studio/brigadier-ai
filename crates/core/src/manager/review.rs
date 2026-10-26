//! The side panel's Review tab: what a session changed, by scope, as a unified
//! patch with its files. It only reads: nothing is staged, committed or written.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use brigadier_git::{ChangeKind, Oid, Repo};

use super::worker_source::worker_range;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, Environment, Setup};
use crate::work::{ReviewCommit, ReviewDiff, ReviewFile, ReviewFileStatus, ReviewScope};
use crate::{Error, Result};

/// Context lines that make git's patch carry whole files, so unchanged lines can be expanded.
const WHOLE_FILES: u32 = 1_000_000;
/// Context lines when whole files aren't loaded (git's default).
const CONTEXT: u32 = 3;
/// A whole-file patch larger than this is sent with `CONTEXT` lines instead.
const MAX_WHOLE_PATCH: usize = 4 * 1024 * 1024;
/// The commits offered under "Committed".
const COMMITS: usize = 20;

impl SessionManager {
    /// A session's changes in `scope`: whole files unless `whole_files` is off, whitespace
    /// changes left out with `ignore_whitespace`.
    pub async fn review_diff(
        &self,
        id: &ConversationId,
        scope: ReviewScope,
        whole_files: bool,
        ignore_whitespace: bool,
    ) -> Result<ReviewDiff> {
        // A worker's changes are read in its own worktree, against the branch it merges into.
        let (checkout, branch, base) = if let ReviewScope::Worker { task_id } = &scope {
            let worker = self.worker_checkout(id, task_id).await?;
            (worker.path, worker.branch, Some(worker.base))
        } else {
            let conversation = self.core.conversation(id)?;
            let archived = conversation.lifecycle == crate::model::Lifecycle::Archived;
            let Some(Setup::Session {
                repo, environment, ..
            }) = conversation.setup
            else {
                return Err(Error::Invalid(
                    "only a session has changes to review".into(),
                ));
            };
            match environment {
                Environment::LocalCheckout { branch } => (repo, Some(branch), None),
                Environment::NewWorktree {
                    base, branch, path, ..
                } => (
                    if archived { repo } else { path.unwrap_or(repo) },
                    Some(branch),
                    Some(base),
                ),
            }
        };
        let session = id.0.clone();
        let mut snapshot_turn = None;
        let scope = match scope {
            ReviewScope::LastTurn { request_id } => {
                let board = self.core.board(id).await?;
                let request_id = request_id.or_else(|| {
                    board
                        .requests
                        .values()
                        .filter(|request| request.changes.is_some())
                        .max_by_key(|request| request.started_at_ms)
                        .map(|request| request.id.clone())
                });
                if let Some(request) = &request_id {
                    snapshot_turn = self
                        .snapshot_turn_record(id, request)
                        .await?
                        .map(|turn| turn.number);
                }
                ReviewScope::LastTurn { request_id }
            }
            other => other,
        };
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(Path::new(&checkout)).map_err(git_error)?;
            let base = match base {
                Some(base) => Some(base),
                None => repo.default_branch().map_err(git_error)?,
            };
            let mut untracked = BTreeSet::new();
            let range: Option<(Oid, Oid)> = match &scope {
                ReviewScope::LastTurn { .. } => match snapshot_turn {
                    Some(turn) => Some((
                        repo.turn_tree(&session, turn, false).map_err(git_error)?,
                        repo.turn_tree(&session, turn, true).map_err(git_error)?,
                    )),
                    None => None,
                },
                ReviewScope::Uncommitted | ReviewScope::Unstaged | ReviewScope::Staged => {
                    let trees = repo.checkout_trees().map_err(git_error)?;
                    match scope {
                        ReviewScope::Uncommitted => {
                            untracked.extend(trees.untracked);
                            Some((trees.head_tree, trees.files))
                        }
                        ReviewScope::Unstaged => {
                            untracked.extend(trees.untracked);
                            Some((trees.staged, trees.files))
                        }
                        _ => Some((trees.head_tree, trees.staged)),
                    }
                }
                ReviewScope::Commit { commit, commits } => {
                    let commit = repo.resolve(commit).map_err(git_error)?;
                    // A fast-forward's commits reach back from its head (#411).
                    let back = commits.filter(|&n| n > 1).unwrap_or(1);
                    let parent = match repo
                        .resolve(&format!("{}~{back}", commit.0))
                        .or_else(|_| repo.resolve(&format!("{}^", commit.0)))
                    {
                        Ok(parent) => parent,
                        Err(_) => repo.empty_tree().map_err(git_error)?,
                    };
                    Some((parent, commit))
                }
                ReviewScope::Branch => {
                    let tip = match &branch {
                        Some(branch) => repo.branch_tip(branch).map_err(git_error)?,
                        None => None,
                    };
                    let left = match &base {
                        Some(base) => match repo.branch_tip(base).map_err(git_error)? {
                            Some(_) => Some(repo.newer_tip(base).map_err(git_error)?.0),
                            None => None,
                        },
                        None => None,
                    };
                    match (left, tip) {
                        (Some(left), Some(tip)) => {
                            Some((repo.merge_base(&left, &tip).map_err(git_error)?, tip))
                        }
                        _ => None,
                    }
                }
                ReviewScope::Worker { .. } => {
                    let (from, to, files) =
                        worker_range(&repo, base.as_deref().unwrap_or_default())?;
                    untracked.extend(files);
                    Some((from, to))
                }
            };
            let commits = match repo.resolve("HEAD") {
                Ok(head) => repo
                    .log(&head, COMMITS)
                    .map_err(git_error)?
                    .into_iter()
                    .map(|info| ReviewCommit {
                        commit: info.commit.0,
                        subject: info.subject,
                        at_ms: info.at_ms,
                    })
                    .collect(),
                Err(_) => Vec::new(),
            };
            let mut review = ReviewDiff {
                scope,
                files: Vec::new(),
                insertions: 0,
                deletions: 0,
                patch: String::new(),
                full_files: whole_files,
                commits,
                branch,
                base,
            };
            let Some((from, to)) = range else {
                return Ok(review);
            };
            (review.files, review.insertions, review.deletions) =
                review_files(&repo, &from, &to, &untracked)?;
            let context = if whole_files { WHOLE_FILES } else { CONTEXT };
            review.patch = repo
                .review_patch(&from, &to, context, ignore_whitespace)
                .map_err(git_error)?;
            if whole_files && review.patch.len() > MAX_WHOLE_PATCH {
                review.patch = repo
                    .review_patch(&from, &to, CONTEXT, ignore_whitespace)
                    .map_err(git_error)?;
                review.full_files = false;
            }
            if let Some(turn) = snapshot_turn {
                let own = repo.turn_diff(&session, turn).map_err(git_error)?;
                review
                    .files
                    .retain(|file| own.stat.files.iter().any(|own| own.path == file.path));
                review.insertions = own.stat.insertions;
                review.deletions = own.stat.deletions;
                // What merges brought in isn't the turn's own: the patch keeps its own paths,
                // with the review's options.
                if !own.merged.is_empty() {
                    review.patch = if own.paths.is_empty() {
                        String::new()
                    } else {
                        repo.review_patch_of(
                            &from,
                            &to,
                            if review.full_files {
                                WHOLE_FILES
                            } else {
                                CONTEXT
                            },
                            ignore_whitespace,
                            &own.paths,
                        )
                        .map_err(git_error)?
                    };
                }
            }
            Ok(review)
        })
        .await
    }
}

/// The files changed from `from` to `to`, each with its counts (those in `untracked` marked
/// so), and the counts of all of them.
pub(super) fn review_files(
    repo: &Repo,
    from: &Oid,
    to: &Oid,
    untracked: &BTreeSet<String>,
) -> Result<(Vec<ReviewFile>, u32, u32)> {
    let stat = repo.diff_stat(from, to).map_err(git_error)?;
    let stats: HashMap<&str, _> = stat
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let files = repo
        .changes(from, to, untracked)
        .map_err(git_error)?
        .into_iter()
        .map(|change| {
            let counts = stats.get(change.path.as_str());
            let (status, from) = match change.kind {
                _ if change.untracked => (ReviewFileStatus::Untracked, None),
                ChangeKind::Added => (ReviewFileStatus::Added, None),
                ChangeKind::Modified => (ReviewFileStatus::Modified, None),
                ChangeKind::Deleted => (ReviewFileStatus::Deleted, None),
                ChangeKind::Renamed { from } => (ReviewFileStatus::Renamed, Some(from)),
                ChangeKind::TypeChanged => (ReviewFileStatus::TypeChanged, None),
            };
            ReviewFile {
                insertions: counts.map_or(0, |file| file.insertions),
                deletions: counts.map_or(0, |file| file.deletions),
                binary: counts.is_some_and(|file| file.binary),
                path: change.path,
                from,
                status,
            }
        })
        .collect();
    Ok((files, stat.insertions, stat.deletions))
}
