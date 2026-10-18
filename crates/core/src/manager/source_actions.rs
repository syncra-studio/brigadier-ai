//! The Source panel's own git, past staging and committing: merging a branch in and resolving
//! its conflicts file by file, aborting it, switching, rebasing, creating, renaming and
//! deleting branches, stashing, pulling, pushing and fetching, and writing a commit message
//! into the box. Each is the user's own act on the checkout the panel shows (a session's, or
//! its project's folder before it starts); git's refusals come back as errors in its own
//! words. The git commands they run that could change something are kept per checkout for the
//! panel's Git Output.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_git::{Git, GitRun, Repo};

use super::git_actions::source_state_of;
use super::{SessionManager, blocking, git_error};
use crate::Result;
use crate::model::{Environment, Setup};
use crate::work::{
    ConflictFile, KeptWork, SourceAction, SourceBranch, SourceGitRun, SourceState, WorkspaceTarget,
};

/// How many commands a checkout's Git Output keeps, the newest.
const GIT_OUTPUT_KEPT: usize = 300;

/// Each checkout's Git Output, oldest first; in memory, from the daemon's start.
#[derive(Debug, Default)]
pub(crate) struct SourceOutput(Mutex<HashMap<PathBuf, VecDeque<SourceGitRun>>>);

/// Where one act's git commands go before they join the checkout's Git Output.
pub(super) type RunLog = Arc<Mutex<Vec<GitRun>>>;

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
        if matches!(target, WorkspaceTarget::Conversation { .. }) {
            match action {
                SourceAction::Checkout { .. } | SourceAction::CreateBranch { .. } => {
                    return Err(crate::Error::Invalid(
                        "a session works on its own branch, so it can't switch to another".into(),
                    ));
                }
                SourceAction::RenameBranch { .. } => {
                    return Err(crate::Error::Invalid(
                        "a session works on its own branch, so it can't rename it".into(),
                    ));
                }
                _ => {}
            }
        }
        let _fence = match target {
            WorkspaceTarget::Conversation { conversation_id } => Some(self.enter(conversation_id)?),
            WorkspaceTarget::Project { .. } => None,
        };
        let path = self.source_dir(target)?;
        // Brigadier's branches stay: the sessions' and their tasks'.
        let guarded = matches!(
            action,
            SourceAction::DeleteBranch { .. } | SourceAction::RenameBranch { .. }
        );
        let candidates = if guarded {
            self.engine_branches(target, &path).await
        } else {
            Vec::new()
        };
        let (git, log) = self.logged_git();
        let records = self.ref_records.clone();
        let dir = path.clone();
        let done = blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            let mut kept = in_repo(&git, &repo, candidates);
            if guarded {
                kept.extend(records.branches_of(repo.common_dir()));
            }
            // Told apart regardless of case: a case-blind disk takes `Main` for `main`.
            let kept: HashSet<String> = kept.iter().map(|branch| branch.to_lowercase()).collect();
            let kept_name = |branch: &String| kept.contains(&branch.to_lowercase());
            let current = || repo.state().map(|state| state.current_branch);
            // A rename renames the branch looked at here, or nothing.
            let renamed = match &action {
                SourceAction::RenameBranch { .. } => {
                    Some(current().map_err(git_error)?.ok_or_else(|| {
                        crate::Error::Invalid("the checkout is not on a branch".into())
                    })?)
                }
                _ => None,
            };
            let refused = match &action {
                SourceAction::DeleteBranch { name, .. } if kept_name(name) => Some(name.clone()),
                SourceAction::RenameBranch { .. } => {
                    renamed.clone().filter(|branch| kept_name(branch))
                }
                _ => None,
            };
            if let Some(branch) = refused {
                return Err(crate::Error::Invalid(format!(
                    "a session works on {branch}, so Brigadier keeps it"
                )));
            }
            let acted = match &action {
                SourceAction::RenameBranch { name } => {
                    repo.rename_current_branch(renamed.as_deref().unwrap_or_default(), name)
                }
                // The new tip is recorded under the ref lock, so no ref check takes the
                // user's rebase for damage and puts the old tip back.
                SourceAction::Rebase { branch } => match current().map_err(git_error)? {
                    Some(own) => super::refcheck::engine_rewrite(&repo, &records, &own, || {
                        repo.rebase_onto(&own, branch)
                    }),
                    None => Err(brigadier_git::Error::Invalid(
                        "the checkout is not on a branch".into(),
                    )),
                },
                _ => act(&repo, action),
            };
            acted.map_err(|err| match err {
                // What git said, as the user would read it in a terminal.
                brigadier_git::Error::Command { stderr, .. } if !stderr.is_empty() => {
                    crate::Error::Invalid(stderr)
                }
                other => git_error(other),
            })?;
            source_state_of(&repo)
        })
        .await;
        self.file_output(&path, &log);
        done
    }

    /// The checkout's Git Output: the git commands the panel ran there, oldest first.
    pub async fn source_git_output(&self, target: &WorkspaceTarget) -> Result<Vec<SourceGitRun>> {
        let path = self.source_dir(target)?;
        let output = self
            .source_output
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        Ok(output
            .get(&path)
            .map(|runs| runs.iter().cloned().collect())
            .unwrap_or_default())
    }

    /// A git whose commands that could change something go to a log of their own, for
    /// [`Self::file_output`].
    pub(super) fn logged_git(&self) -> (Git, RunLog) {
        let log = RunLog::default();
        (self.git.logging(log.clone()), log)
    }

    /// Adds what an act ran in the checkout at `path` to its Git Output.
    pub(super) fn file_output(&self, path: &Path, log: &RunLog) {
        let runs = std::mem::take(&mut *log.lock().unwrap_or_else(|e| e.into_inner()));
        if runs.is_empty() {
            return;
        }
        let mut output = self
            .source_output
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let kept = output.entry(path.to_owned()).or_default();
        kept.extend(runs.into_iter().map(|run| SourceGitRun {
            at_ms: run.at_ms,
            command: run.command,
            output: run.output,
            ok: run.ok,
        }));
        while kept.len() > GIT_OUTPUT_KEPT {
            kept.pop_front();
        }
    }

    /// The branches Brigadier works on, each with a checkout of its repository: every
    /// session's (an archived one's too, for its restore) and the base it merges into, its
    /// tasks' (a finished task's kept branch too) and its overnight runs'.
    /// Only sessions of the target's repository are looked at (their folder named alike; a
    /// linked checkout of it is told apart later, by git).
    async fn engine_branches(
        &self,
        target: &WorkspaceTarget,
        checkout: &Path,
    ) -> Vec<(PathBuf, String)> {
        let canonical =
            |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
        let own_repo = match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                match self
                    .core
                    .conversation(conversation_id)
                    .ok()
                    .and_then(|c| c.setup)
                {
                    Some(Setup::Session { repo, .. }) => PathBuf::from(repo),
                    _ => checkout.to_owned(),
                }
            }
            WorkspaceTarget::Project { .. } => checkout.to_owned(),
        };
        let own_repo = canonical(&own_repo);
        let mut branches = Vec::new();
        for conversation in self.core.catalog().conversations {
            let Some(Setup::Session {
                repo, environment, ..
            }) = &conversation.setup
            else {
                continue;
            };
            if canonical(Path::new(repo)) != own_repo {
                continue;
            }
            match environment {
                Environment::LocalCheckout { branch } => {
                    branches.push((PathBuf::from(repo), branch.clone()));
                }
                Environment::NewWorktree { branch, base, .. } => {
                    branches.push((PathBuf::from(repo), branch.clone()));
                    branches.push((PathBuf::from(repo), base.clone()));
                }
            }
            let Ok(board) = self.core.board(&conversation.id).await else {
                continue;
            };
            for task in board.tasks.values() {
                let Ok(repo) = self.task_repo(task) else {
                    continue;
                };
                let own = task.workspace.as_ref().and_then(|w| w.branch.clone());
                // Kept work: its branch, or the branch its patch was restored as.
                let kept = match &task.kept {
                    Some(KeptWork::Branch { branch, .. }) => Some(branch.clone()),
                    Some(KeptWork::Diff { restored, .. }) => restored.clone(),
                    None => None,
                };
                branches.extend(
                    own.into_iter()
                        .chain(kept)
                        .map(|branch| (repo.clone(), branch)),
                );
            }
            for run in board.runs.values() {
                if let Some(workspace) = &run.workspace {
                    branches.push((PathBuf::from(repo), workspace.branch.clone()));
                }
            }
        }
        branches
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

/// The names among `branches` whose checkout is of `repo`'s repository.
fn in_repo(git: &Git, repo: &Repo, branches: Vec<(PathBuf, String)>) -> HashSet<String> {
    let mut same: HashMap<PathBuf, bool> = HashMap::new();
    branches
        .into_iter()
        .filter(|(checkout, _)| {
            *same.entry(checkout.clone()).or_insert_with(|| {
                git.open(checkout)
                    .is_ok_and(|other| other.common_dir() == repo.common_dir())
            })
        })
        .map(|(_, branch)| branch)
        .collect()
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
        // Rebased above, under the ref lock.
        SourceAction::Rebase { .. } => Ok(()),
        SourceAction::CreateBranch { name, from } => repo.create_and_switch(&name, from.as_deref()),
        // Renamed above, with the branch it saw checked out.
        SourceAction::RenameBranch { .. } => Ok(()),
        SourceAction::DeleteBranch { name, force } => repo.delete_branch(&name, force),
        SourceAction::Stash { untracked } => repo.stash(untracked),
        SourceAction::ApplyStash { pop } => repo.apply_stash(pop).map(drop),
        SourceAction::DropStash => repo.drop_stash(),
        SourceAction::Resolve {
            path,
            expected,
            text,
            stage,
        } => repo.resolve_conflict(&path, expected.as_deref(), &text, stage),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use brigadier_git::Git;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn repo_at(dir: &Path) {
        std::fs::create_dir_all(dir).unwrap();
        run(dir, &["init", "-q", "-b", "main"]);
        run(
            dir,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "Start",
            ],
        );
    }

    /// A session's branch is kept only in its own repository: another project's branch of the
    /// same name is the user's to rename or delete.
    #[test]
    fn kept_branches_are_those_of_the_checkouts_own_repository() {
        let root =
            std::env::temp_dir().join(format!("brigadier-source-keep-{}", std::process::id()));
        let (mine, other) = (root.join("mine"), root.join("other"));
        repo_at(&mine);
        repo_at(&other);
        let linked = root.join("linked");
        run(
            &mine,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "session",
                linked.to_str().unwrap(),
            ],
        );
        let git = Git::new(PathBuf::from("git"), std::env::vars_os().collect());
        let repo = git.open(&mine).unwrap();
        let kept = super::in_repo(
            &git,
            &repo,
            vec![
                (linked.clone(), "session".into()),
                (other.clone(), "main".into()),
                (root.join("gone"), "lost".into()),
            ],
        );
        assert_eq!(kept, ["session".to_owned()].into_iter().collect());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
