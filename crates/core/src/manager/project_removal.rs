//! Removing a project (sidebar → Remove project…): it leaves Brigadier, never the user's
//! files. A preview comes first and shows what goes: its conversations (archived ones too),
//! their worktrees, the branches Brigadier created for them (how each stands against the
//! branch its work goes to) and its Brain. Nothing goes while one of its conversations works.
//!
//! Its conversations are deleted as [`SessionManager::delete`] does, but keeping every branch;
//! then only the branches the user picked go, each at the tip the preview showed (merged ones
//! only while still merged). The ones left are recorded as kept, so Storage can offer them
//! later. The Brain goes to the Trash unless the user keeps it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use brigadier_providers::Artifact;
use brigadier_sandbox::removal;

use super::disk::{branch_standing, brigadier_branch, delete_branch_checked, same_path_in};
use super::{SessionManager, blocking};
use crate::model::{Conversation, ConversationId, Environment, KeptBranch, ProjectId, Setup};
use crate::overnight::OvernightRun;
use crate::storage::{
    BranchChoice, ProjectRemoval, RemovalBranch, RemovalConversation, RemovalWorktree,
    RemoveProjectReport, UnlandedBranch,
};
use crate::work::Task;
use crate::{Error, Result};

/// A Brigadier branch a conversation created, and the branch its work goes to.
#[derive(Debug, Clone)]
pub(super) struct BranchRecord {
    repo: PathBuf,
    name: String,
    target: String,
    /// Its tip, when a kept-branch record says where it pointed.
    tip: Option<String>,
}

impl BranchRecord {
    /// A branch a kept-branch record names, at the tip it had then.
    pub(super) fn kept(repo: String, kept: KeptBranch) -> Self {
        Self {
            repo: PathBuf::from(repo),
            name: kept.name,
            target: kept.target,
            tip: Some(kept.tip),
        }
    }
}

/// What removing a project takes, with what the removal itself needs.
struct Plan {
    preview: ProjectRemoval,
    /// Every conversation of the project, side chats included.
    conversations: Vec<ConversationId>,
}

impl SessionManager {
    /// What removing a project takes with it.
    pub async fn preview_remove_project(&self, id: ProjectId) -> Result<ProjectRemoval> {
        Ok(self.removal_plan(&id).await?.preview)
    }

    /// Removes a project from Brigadier. Its conversations are deleted (workers stopped,
    /// worktrees removed with their uncommitted work kept as WIP commits, CLI files and
    /// processes gone); of Brigadier's branches only those in `delete` go, at the tips they
    /// were shown with; its Brain goes to the Trash unless `keep_brain`. The repository's files
    /// and the user's own branches are never touched. Refused while any of its conversations
    /// works.
    pub async fn remove_project(
        &self,
        id: ProjectId,
        delete: Vec<BranchChoice>,
        keep_brain: bool,
    ) -> Result<RemoveProjectReport> {
        let plan = self.removal_plan(&id).await?;
        if let Some(why) = plan.preview.blocked {
            return Err(Error::Invalid(why));
        }
        for conversation in plan.conversations {
            // A side chat went with its parent.
            if self.core.conversation(&conversation).is_err() {
                continue;
            }
            self.delete_conversation(conversation, false, false).await?;
        }
        self.close_project_brain(&id).await;
        self.core.forget_project(id.clone()).await?;
        tracing::info!(project = %id, "project removed");

        let git = self.git.clone();
        let branches = plan.preview.branches;
        let (kept, mut failures) =
            blocking(move || Ok(settle_branches(&git, branches, &delete))).await?;
        self.record_kept(&kept).await;

        let brain_trashed_bytes = if keep_brain {
            0
        } else {
            match self.trash_project_brain(&id).await {
                Ok(bytes) => bytes,
                Err(err) => {
                    failures.push(format!("Its Brain stays ({err}); Storage offers it later."));
                    0
                }
            }
        };
        // Its task worktrees lived here; they went with its conversations.
        let worktrees = self.owned_dir("worktrees", &id.0);
        let _ = blocking(move || {
            let _ = std::fs::remove_dir(&worktrees);
            Ok(())
        })
        .await;
        Ok(RemoveProjectReport {
            kept_branches: kept,
            brain_trashed_bytes,
            failures,
        })
    }

    /// Records the Brigadier branches a deleted conversation left behind as kept, as they stand
    /// now, so Storage can offer them later.
    pub(super) async fn record_left_branches(&self, records: Vec<BranchRecord>) {
        let git = self.git.clone();
        let standing = blocking(move || {
            let mut seen = HashSet::new();
            Ok(records
                .into_iter()
                .filter(|record| seen.insert((record.repo.clone(), record.name.clone())))
                .filter_map(|record| {
                    let repo = git.open(&record.repo).ok()?;
                    branch_standing(&repo, &record.name, &record.target, None, &[])
                })
                .collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default();
        self.record_kept(&standing).await;
    }

    async fn record_kept(&self, branches: &[RemovalBranch]) {
        let mut by_repo: HashMap<&str, Vec<KeptBranch>> = HashMap::new();
        for branch in branches {
            by_repo
                .entry(branch.repo.as_str())
                .or_default()
                .push(KeptBranch {
                    name: branch.name.clone(),
                    target: branch.target.clone(),
                    tip: branch.tip.clone(),
                });
        }
        for (repo, kept) in by_repo {
            if let Err(err) = self
                .runtime
                .ledger()
                .record_kept_branches(repo.to_owned(), kept)
                .await
            {
                tracing::warn!(repo, error = %err, "could not record kept branches");
            }
        }
    }

    /// The branches deleting `ids` takes that hold work that never landed, or whose standing
    /// can't be told. A worktree's uncommitted changes count: they become a WIP commit on its
    /// branch before the worktree goes.
    pub async fn preview_delete(&self, ids: &[ConversationId]) -> Vec<UnlandedBranch> {
        let mut records = Vec::new();
        let mut worktrees = Vec::new();
        for id in ids {
            let Ok(conversation) = self.core.conversation(id) else {
                continue;
            };
            let board = self.core.board(id).await.unwrap_or_default();
            let tasks = board.sorted_tasks();
            records.extend(branch_records(&conversation, &tasks, board.runs.values()));
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
        let git = self.git.clone();
        blocking(move || Ok(unlanded(&git, &worktrees, records)))
            .await
            .unwrap_or_default()
    }

    /// Whether a turn, work waiting for one, or a worker of the conversation is running.
    async fn conversation_running(&self, id: &ConversationId) -> bool {
        let live = self.convs_lock().get(id).cloned();
        if let Some(conv) = live
            && conv.is_busy().await
        {
            return true;
        }
        self.has_running_workers(id).await
    }

    async fn removal_plan(&self, id: &ProjectId) -> Result<Plan> {
        let project = self.core.project(id)?;
        let catalog = self.core.catalog();
        let owners = self.runtime.ledger().owners();
        let mut listed = Vec::new();
        let mut all = Vec::new();
        let mut working = Vec::new();
        let mut worktrees: Vec<PathBuf> = Vec::new();
        let mut records = Vec::new();
        for conversation in catalog
            .conversations
            .iter()
            .filter(|c| c.project_id.as_ref() == Some(id))
        {
            let board = self.core.board(&conversation.id).await.unwrap_or_default();
            let tasks = board.sorted_tasks();
            let running = self.conversation_running(&conversation.id).await;
            if running {
                working.push(conversation.title.clone());
            }
            all.push(conversation.id.clone());
            if conversation.side_of.is_none() {
                listed.push(RemovalConversation {
                    id: conversation.id.clone(),
                    title: conversation.title.clone(),
                    kind: conversation.kind,
                    archived: conversation.lifecycle == crate::model::Lifecycle::Archived,
                    running,
                });
            }
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
            let mut mine: HashSet<String> = ["orch", "chat", "session"]
                .iter()
                .map(|kind| format!("{kind}:{}", conversation.id))
                .collect();
            mine.extend(board.runs.values().map(super::overnight::run_owner));
            for task in &tasks {
                mine.insert(format!("task:{}", task.id));
                if let Some(path) = task.workspace.as_ref().and_then(|w| w.worktree.as_ref()) {
                    worktrees.push(PathBuf::from(path));
                }
            }
            for (owner, artifacts, _) in &owners {
                if mine.contains(owner) {
                    worktrees.extend(artifacts.iter().filter_map(|artifact| match artifact {
                        Artifact::Worktree { path, .. } => Some(PathBuf::from(path)),
                        _ => None,
                    }));
                }
            }
        }
        // Branches kept when its conversations were deleted, in repositories no other project
        // uses (so they can only be this project's).
        let repos: HashSet<&str> = project.repos.iter().map(|r| r.path.as_str()).collect();
        let shared: HashSet<&str> = catalog
            .projects
            .iter()
            .filter(|other| other.id != *id)
            .flat_map(|other| other.repos.iter().map(|r| r.path.as_str()))
            .collect();
        for (repo, kept) in self.kept_branches().await? {
            if repos.contains(repo.as_str()) && !shared.contains(repo.as_str()) {
                records.push(BranchRecord::kept(repo, kept));
            }
        }
        let (git, brain) = (self.git.clone(), self.project_brain_dir(id));
        let (worktrees, branches, brain_bytes) = blocking(move || {
            let (worktrees, branches) = survey(&git, worktrees, records);
            Ok((worktrees, branches, removal::allocated_size(&brain)))
        })
        .await?;
        let blocked = (!working.is_empty()).then(|| {
            format!(
                "{} {} working. Stop {} or let {} finish first.",
                working
                    .iter()
                    .map(|title| format!("“{title}”"))
                    .collect::<Vec<_>>()
                    .join(", "),
                if working.len() == 1 { "is" } else { "are" },
                if working.len() == 1 { "it" } else { "them" },
                if working.len() == 1 { "it" } else { "them" },
            )
        });
        Ok(Plan {
            preview: ProjectRemoval {
                project_id: id.clone(),
                name: project.name,
                repo: project.repos.first().map(|repo| repo.path.clone()),
                conversations: listed,
                worktrees,
                branches,
                brain_bytes,
                blocked,
            },
            conversations: all,
        })
    }
}

/// How `worktrees` (all about to go) and the branches in `records` stand: sizes, uncommitted
/// changes, and each branch against the branch its work goes to.
pub(super) fn survey(
    git: &brigadier_git::Git,
    mut worktrees: Vec<PathBuf>,
    records: Vec<BranchRecord>,
) -> (Vec<RemovalWorktree>, Vec<RemovalBranch>) {
    let mut seen = HashSet::new();
    worktrees.retain(|path| seen.insert(path.clone()) && path.exists());
    let listed: Vec<RemovalWorktree> = worktrees
        .iter()
        .map(|path| RemovalWorktree {
            path: path.display().to_string(),
            bytes: removal::allocated_size(path),
            has_changes: git
                .open(path)
                .and_then(|worktree| worktree.state())
                .map_or(true, |state| !state.dirty_files.is_empty()),
        })
        .collect();
    let dirty: Vec<PathBuf> = listed
        .iter()
        .filter(|w| w.has_changes)
        .map(|w| PathBuf::from(&w.path))
        .collect();
    let mut seen = HashSet::new();
    let mut branches = Vec::new();
    for record in records {
        if !seen.insert((record.repo.clone(), record.name.clone())) {
            continue;
        }
        let Ok(repo) = git.open(&record.repo) else {
            continue;
        };
        let Some(mut standing) = branch_standing(
            &repo,
            &record.name,
            &record.target,
            record.tip.as_deref(),
            &worktrees,
        ) else {
            continue;
        };
        // Its worktree's changes become a WIP commit on it before the worktree goes.
        let gains_wip = repo.worktrees().is_ok_and(|list| {
            list.iter().any(|w| {
                w.branch.as_deref() == Some(record.name.as_str()) && same_path_in(&w.path, &dirty)
            })
        });
        if gains_wip {
            standing.merged = false;
            standing.ahead += 1;
        }
        branches.push(standing);
    }
    (listed, branches)
}

/// Which of the branches in `records` hold work their target doesn't have, or can't be told.
fn unlanded(
    git: &brigadier_git::Git,
    worktrees: &[PathBuf],
    records: Vec<BranchRecord>,
) -> Vec<UnlandedBranch> {
    let dirty: Vec<PathBuf> = worktrees
        .iter()
        .filter(|path| path.exists())
        .filter(|path| {
            git.open(path)
                .and_then(|worktree| worktree.state())
                .map_or(true, |state| !state.dirty_files.is_empty())
        })
        .cloned()
        .collect();
    let mut seen = HashSet::new();
    let mut listed = Vec::new();
    for record in records {
        if !seen.insert((record.repo.clone(), record.name.clone())) {
            continue;
        }
        let unknown = |record: &BranchRecord| UnlandedBranch {
            repo: record.repo.display().to_string(),
            name: record.name.clone(),
            unknown: true,
        };
        let Ok(repo) = git.open(&record.repo) else {
            listed.push(unknown(&record));
            continue;
        };
        let tip = match repo.branch_tip(&record.name) {
            Ok(Some(tip)) => tip,
            // Already gone: nothing of it is lost.
            Ok(None) => continue,
            Err(_) => {
                listed.push(unknown(&record));
                continue;
            }
        };
        // A worker's branch targets its session's branch; once that is gone, its work would
        // land on the repository's default branch.
        let target = match repo.branch_tip(&record.target) {
            Ok(Some(target_tip)) => Some((record.target.clone(), target_tip)),
            _ => repo.default_branch().ok().flatten().and_then(|fallback| {
                let tip = repo.branch_tip(&fallback).ok().flatten()?;
                Some((fallback, tip))
            }),
        };
        let Some((target, target_tip)) = target else {
            listed.push(unknown(&record));
            continue;
        };
        let gains_wip = repo.worktrees().is_ok_and(|list| {
            list.iter().any(|w| {
                w.branch.as_deref() == Some(record.name.as_str()) && same_path_in(&w.path, &dirty)
            })
        });
        let landed = match (
            repo.is_merged(&record.name, &target),
            repo.count_commits(&target_tip, &tip),
        ) {
            (Ok(merged), Ok(ahead)) => merged && ahead == 0 && !gains_wip,
            _ => {
                listed.push(unknown(&record));
                continue;
            }
        };
        if !landed {
            listed.push(UnlandedBranch {
                repo: repo.root().display().to_string(),
                name: record.name,
                unknown: false,
            });
        }
    }
    listed
}

/// Deletes the branches the user `picked` (each at the tip it was listed with; a merged one
/// only while still merged); the others stay. The ones that stay, as they stand now, and what
/// failed.
pub(super) fn settle_branches(
    git: &brigadier_git::Git,
    branches: Vec<RemovalBranch>,
    picked: &[BranchChoice],
) -> (Vec<RemovalBranch>, Vec<String>) {
    let (mut kept, mut failures) = (Vec::new(), Vec::new());
    for branch in branches {
        let repo = Path::new(&branch.repo);
        let chosen = !branch.checked_out
            && picked.iter().any(|choice| {
                choice.repo == branch.repo && choice.name == branch.name && choice.tip == branch.tip
            });
        if chosen {
            let listed = KeptBranch {
                name: branch.name.clone(),
                target: branch.target.clone(),
                tip: branch.tip.clone(),
            };
            match delete_branch_checked(git, repo, &listed, branch.merged) {
                Ok(()) => continue,
                Err(err) => failures.push(format!("{} stays: {err}", branch.name)),
            }
        }
        // As it stands now: a worktree's changes may have become a WIP commit on it.
        if let Some(now) = git
            .open(repo)
            .ok()
            .and_then(|repo| branch_standing(&repo, &branch.name, &branch.target, None, &[]))
        {
            kept.push(now);
        }
    }
    (kept, failures)
}

/// The Brigadier branches a conversation created: its session branch, its tasks' branches and
/// its overnight runs' branches, each with the branch its work goes to.
pub(super) fn branch_records<'r>(
    conversation: &Conversation,
    tasks: &[Task],
    runs: impl IntoIterator<Item = &'r OvernightRun>,
) -> Vec<BranchRecord> {
    let Some(Setup::Session {
        repo, environment, ..
    }) = &conversation.setup
    else {
        return Vec::new();
    };
    let (session_target, session_branch) = match environment {
        Environment::NewWorktree { base, branch, .. } => (base, Some(branch)),
        Environment::LocalCheckout { branch, .. } => (branch, None),
    };
    let record = |name: &String, target: &String| BranchRecord {
        repo: PathBuf::from(repo),
        name: name.clone(),
        target: target.clone(),
        tip: None,
    };
    session_branch
        .map(|branch| record(branch, session_target))
        .into_iter()
        .chain(tasks.iter().filter_map(|task| {
            let workspace = task.workspace.as_ref()?;
            let branch = workspace.branch.as_ref()?;
            Some(record(
                branch,
                workspace.target.as_ref().unwrap_or(session_target),
            ))
        }))
        .chain(runs.into_iter().filter_map(|run| {
            let workspace = run.workspace.as_ref()?;
            Some(record(&workspace.branch, &workspace.base))
        }))
        .filter(|record| brigadier_branch(&record.name))
        .collect()
}
