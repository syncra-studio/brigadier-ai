//! Several sessions in one folder (goal 9): a session started on an existing branch works where
//! that branch is checked out, beside any session already there, or in a worktree of its own
//! on it. Nothing is enforced between them; the session let in is told once that it shares
//! the folder. A shared folder, its branch and what runs in it outlive any one session's
//! archive or delete: the last session using a Brigadier-made worktree lets it go, and a
//! folder Brigadier didn't make is never removed.

use std::path::{Path, PathBuf};

use brigadier_providers::Artifact;

use super::{SessionManager, blocking, git_error};
use crate::model::{Conversation, ConversationId, Environment, Lifecycle, Setup};
use crate::{Error, Result};

/// What the session let into a shared folder hears with its first message (D123, Q3). It
/// names no session and carries nothing of its context (D78).
pub(super) fn shared_line(branch: &str) -> String {
    format!(
        "[workspace] Another session also works in this folder on `{branch}`: check `git status` before you commit and commit only the files you changed."
    )
}

/// Where a session on an existing branch works, found when it is created.
pub(super) struct ExistingPlace {
    pub environment: Environment,
    /// The folder is a linked worktree whose removal some other owner records: the new session
    /// is recorded as using it too, so it stays while either does.
    record: Option<(String, String)>,
    /// Checked out nowhere: its worktree is made now, on the branch itself.
    make: bool,
}

/// The folder a session works in, once it has one.
pub(super) fn folder_of(setup: Option<&Setup>) -> Option<PathBuf> {
    match setup? {
        Setup::Session {
            repo,
            environment: Environment::LocalCheckout { .. },
            ..
        } => Some(PathBuf::from(repo)),
        Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        } => Some(PathBuf::from(path)),
        _ => None,
    }
}

fn same(a: &Path, b: &Path) -> bool {
    a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

/// The folders to sweep for a closing session's leftover processes: `root`, less the `keeps`
/// other sessions still work in, as the fewest folders beside them.
pub(super) fn sweep_dirs(root: &Path, keeps: &[PathBuf]) -> Vec<PathBuf> {
    if keeps.iter().any(|keep| keep == root) {
        return Vec::new();
    }
    if !keeps.iter().any(|keep| keep.starts_with(root)) {
        return vec![root.to_owned()];
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|entry| sweep_dirs(&entry.path(), keeps))
        .collect()
}

impl SessionManager {
    /// The other open sessions that work in `folder`.
    pub(super) fn sharers(&self, folder: &Path, except: &ConversationId) -> Vec<Conversation> {
        self.core
            .catalog()
            .conversations
            .into_iter()
            .filter(|other| {
                &other.id != except
                    && other.lifecycle != Lifecycle::Archived
                    && !other.deleting
                    && folder_of(other.setup.as_ref()).is_some_and(|at| same(&at, folder))
            })
            .collect()
    }

    /// A New worktree session's folder is one it joined (goal 9) rather than its own: it never
    /// resets, removes or remakes it.
    pub(super) fn joined(&self, conversation: &Conversation, path: &Path) -> bool {
        !same(path, &self.session_worktree_path(conversation)) && !path.starts_with(&self.data_dir)
    }

    /// Where a session on the existing `branch` of `repo` works: the folder that has it checked
    /// out, or a worktree of its own on it. It merges into what the folder's session merges
    /// into, else the target git records for the branch, else nowhere until the user says
    /// (its base is the branch itself). Called holding `shared_workspaces`.
    pub(super) async fn existing_place(&self, repo: &str, branch: &str) -> Result<ExistingPlace> {
        // As the composer offers them: a running worker's branch is that worker's.
        if self
            .worker_branches(Path::new(repo))
            .await
            .iter()
            .any(|b| b == branch)
        {
            return Err(Error::Invalid(format!(
                "A running worker works on `{branch}`. Pick another branch."
            )));
        }
        let (git, at, name) = (self.git.clone(), PathBuf::from(repo), branch.to_owned());
        let (folder, main, recorded) = blocking(move || {
            let repo = git.open(&at).map_err(git_error)?;
            if repo.branch_tip(&name).map_err(git_error)?.is_none() {
                return Err(Error::Invalid(format!(
                    "The branch `{name}` isn't in this repository any more. Pick another one."
                )));
            }
            let folder = repo
                .worktrees()
                .map_err(git_error)?
                .into_iter()
                .find(|worktree| !worktree.prunable && worktree.branch.as_deref() == Some(&name))
                .map(|worktree| worktree.path);
            let main = folder
                .as_deref()
                .is_some_and(|path| same(path, repo.root()));
            let recorded = repo.merge_base_of(&name).map_err(git_error)?;
            Ok((folder, main, recorded))
        })
        .await?;
        if main {
            return Ok(ExistingPlace {
                environment: Environment::LocalCheckout {
                    branch: branch.to_owned(),
                },
                record: None,
                make: false,
            });
        }
        // The folder's session, if one works there: its path as it records it (paths are
        // compared as written elsewhere), and its target.
        let session = folder.as_deref().and_then(|folder| {
            self.core
                .catalog()
                .conversations
                .into_iter()
                .find_map(|other| match other.setup {
                    Some(Setup::Session {
                        environment:
                            Environment::NewWorktree {
                                base,
                                path: Some(path),
                                ..
                            },
                        ..
                    }) if same(Path::new(&path), folder) => Some((path, base)),
                    _ => None,
                })
        });
        let (path, session_base) = match session {
            Some((path, base)) => (Some(path), Some(base)),
            None => (
                folder.map(|folder| folder.to_string_lossy().into_owned()),
                None,
            ),
        };
        let base = session_base
            .filter(|base| base != branch)
            .or(recorded.filter(|base| base != branch))
            .unwrap_or_else(|| branch.to_owned());
        let record = path.as_ref().and_then(|path| {
            let held = self.runtime.ledger().owners().into_iter().any(|(_, artifacts, _)| {
                artifacts.iter().any(|artifact| {
                    matches!(artifact, Artifact::Worktree { path: at, .. } if same(Path::new(at), Path::new(path)))
                })
            });
            held.then(|| (repo.to_owned(), path.clone()))
        });
        Ok(ExistingPlace {
            make: path.is_none(),
            environment: Environment::NewWorktree {
                base,
                branch: branch.to_owned(),
                path,
                start: None,
            },
            record,
        })
    }

    /// Lets the session just created into its place: recorded as using a shared worktree, or
    /// its own worktree made on the branch.
    /// Called holding `shared_workspaces`.
    pub(super) async fn let_into(
        &self,
        created: Conversation,
        place: ExistingPlace,
    ) -> Result<Conversation> {
        let id = created.id.clone();
        if let Some((repo, path)) = place.record {
            self.runtime
                .ledger()
                .record(&format!("session:{id}"), Artifact::Worktree { repo, path })
                .await?;
        }
        if place.make
            && let Some(Setup::Session {
                repo, environment, ..
            }) = &created.setup
        {
            self.ensure_target(&id, Path::new(repo), environment)
                .await?;
        }
        self.core.conversation(&id)
    }

    /// A new session working in a folder another open session works in hears so once, with
    /// its first message.
    pub(super) async fn tell_if_shared(&self, created: &Conversation) -> Result<()> {
        if let Some(folder) = folder_of(created.setup.as_ref())
            && !self.sharers(&folder, &created.id).is_empty()
            && let Some(Setup::Session { environment, .. }) = &created.setup
        {
            let branch = match environment {
                Environment::LocalCheckout { branch } | Environment::NewWorktree { branch, .. } => {
                    branch
                }
            };
            self.conv(&created.id)?.note(shared_line(branch)).await;
        }
        Ok(())
    }

    /// A joined folder is checked before each turn: still there and still on the session's
    /// branch, or the turn doesn't start (nothing is made or switched in its place).
    pub(super) async fn check_joined(&self, conversation: &Conversation) -> Result<()> {
        let Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    branch,
                    path: Some(path),
                    ..
                },
            ..
        }) = &conversation.setup
        else {
            return Ok(());
        };
        if !self.joined(conversation, Path::new(path)) {
            return Ok(());
        }
        let (git, at) = (self.git.clone(), PathBuf::from(path));
        let current = blocking(move || {
            git.open(&at)
                .map_err(git_error)?
                .current_branch()
                .map_err(git_error)
        })
        .await
        .ok()
        .flatten();
        if current.as_deref() == Some(branch.as_str()) {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "This session's folder {path} is gone or no longer on `{branch}`. Start a new session there from the composer."
            )))
        }
    }

    /// The branches running workers work on in `repo`, so the composer offers no session there.
    pub(super) async fn worker_branches(&self, repo: &Path) -> Vec<String> {
        let mut branches = Vec::new();
        for conversation in self.core.catalog().conversations {
            if conversation.lifecycle == Lifecycle::Archived
                || !matches!(&conversation.setup, Some(Setup::Session { repo: at, .. }) if same(Path::new(at), repo))
            {
                continue;
            }
            let Ok(tasks) = self.core.tasks(&conversation.id).await else {
                continue;
            };
            branches.extend(
                tasks
                    .into_iter()
                    .filter(|task| !task.state.is_final())
                    .filter_map(|task| task.workspace.and_then(|workspace| workspace.branch)),
            );
        }
        branches
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closing_sessions_sweep_leaves_out_only_the_folders_others_work_in() {
        let temp = std::env::temp_dir().join(format!("shared-sweep-{}", uuid::Uuid::new_v4()));
        let root = temp.join("s1");
        for dir in ["work/src", "tmp", "workers/1/work", "workers/2/work"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        assert_eq!(sweep_dirs(&root, &[]), vec![root.clone()]);
        assert!(sweep_dirs(&root, std::slice::from_ref(&root)).is_empty());
        let mut swept = sweep_dirs(&root, &[root.join("work"), root.join("workers/1/work")]);
        swept.sort();
        assert_eq!(swept, vec![root.join("tmp"), root.join("workers/2")]);
        assert_eq!(
            sweep_dirs(&root, &[temp.join("elsewhere")]),
            vec![root.clone()]
        );
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
