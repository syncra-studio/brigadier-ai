//! The side panel's Review tab: what a session changed, by scope, as a unified
//! patch with its files. It only reads: nothing is staged, committed or written.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use brigadier_git::{ChangeKind, Oid};

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
        let Some(Setup::Session {
            repo, environment, ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "only a session has changes to review".into(),
            ));
        };
        let (checkout, branch, base) = match environment {
            Environment::LocalCheckout { branch } => (repo, branch, None),
            Environment::NewWorktree {
                base, branch, path, ..
            } => (path.unwrap_or(repo), branch, Some(base)),
        };
        // "Last Turn" is what a request's workers landed: the given request, or the latest
        // that landed anything.
        let mut recorded_range = None;
        let (scope, landed) = match scope {
            ReviewScope::LastTurn { request_id } => {
                let board = self.core.board(id).await?;
                let landed_by = |request: &str| -> Vec<Oid> {
                    board
                        .tasks
                        .values()
                        .filter(|task| task.request_id.as_deref() == Some(request))
                        .filter_map(|task| task.landed.clone().map(Oid))
                        .collect()
                };
                let request_id = request_id.or_else(|| {
                    board
                        .requests
                        .values()
                        .filter(|request| {
                            request.changes.is_some() || !landed_by(&request.id).is_empty()
                        })
                        .max_by_key(|request| request.started_at_ms)
                        .map(|request| request.id.clone())
                });
                if let Some(request) = &request_id
                    && let Some(record) = self.change_record(id, request).await?
                {
                    recorded_range = Some(record.commit);
                }
                let landed = request_id.as_deref().map(landed_by).unwrap_or_default();
                (ReviewScope::LastTurn { request_id }, landed)
            }
            ReviewScope::Commit { commit } => {
                if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(Error::Invalid(format!("`{commit}` is not a commit id")));
                }
                (ReviewScope::Commit { commit }, Vec::new())
            }
            other => (other, Vec::new()),
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
                ReviewScope::LastTurn { .. } if recorded_range.is_some() => {
                    let tip = recorded_range.as_ref().expect("record range");
                    Some((
                        repo.resolve(&format!("{tip}^")).map_err(git_error)?,
                        Oid(tip.clone()),
                    ))
                }
                ReviewScope::LastTurn { .. } if landed.is_empty() => None,
                ReviewScope::LastTurn { .. } => Some(repo.changes_of(&landed).map_err(git_error)?),
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
                ReviewScope::Commit { commit } => {
                    let commit = repo.resolve(commit).map_err(git_error)?;
                    let parent = match repo.resolve(&format!("{}^", commit.0)) {
                        Ok(parent) => parent,
                        Err(_) => repo.empty_tree().map_err(git_error)?,
                    };
                    Some((parent, commit))
                }
                ReviewScope::Branch => {
                    let tip = repo.branch_tip(&branch).map_err(git_error)?;
                    let left = match &base {
                        Some(base) => repo.branch_tip(base).map_err(git_error)?,
                        None => None,
                    };
                    match (left, tip) {
                        (Some(left), Some(tip)) => {
                            Some((repo.merge_base(&left, &tip).map_err(git_error)?, tip))
                        }
                        _ => None,
                    }
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
                branch: Some(branch),
                base,
            };
            let Some((from, to)) = range else {
                return Ok(review);
            };
            let stat = repo.diff_stat(&from, &to).map_err(git_error)?;
            let stats: HashMap<&str, _> = stat
                .files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect();
            review.files = repo
                .changes(&from, &to, &untracked)
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
            review.insertions = stat.insertions;
            review.deletions = stat.deletions;
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
            Ok(review)
        })
        .await
    }
}
