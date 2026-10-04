//! The run's own branch and worktree (PLAN.md §10.5), made at Start from the base's committed
//! tip. The user's uncommitted files and the session's own worktree stay out of it; workers'
//! changes land on its branch, and only the user's Merge brings verified work into the base.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use brigadier_git::WorktreeSpec;
use brigadier_providers::Artifact;

use super::super::{SessionManager, blocking, git_error};
use crate::model::{Environment, Setup};
use crate::overnight::{OvernightRun, RunWorkspace};
use crate::{Error, Result};

impl SessionManager {
    /// Makes the run's branch and worktree, or finds them again for a continued run (whose
    /// branch carries over; its worktree may have been removed since).
    pub(crate) async fn prepare_run_workspace(&self, run: &OvernightRun) -> Result<RunWorkspace> {
        let conversation = self.core.conversation(&run.conversation_id)?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = &conversation.setup
        else {
            return Err(Error::Invalid("overnight runs belong to a session".into()));
        };
        let repo = PathBuf::from(repo);
        let project = conversation
            .project_id
            .as_ref()
            .map(|id| id.0.clone())
            .unwrap_or_else(|| "none".into());
        let path = run.workspace.as_ref().map_or_else(
            || {
                self.owned_dir("worktrees", &project)
                    .join(format!("overnight-{}", run.id.short()))
            },
            |workspace| PathBuf::from(&workspace.path),
        );
        self.runtime
            .ledger()
            .record(
                &run_owner(run),
                Artifact::Worktree {
                    repo: repo.to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                },
            )
            .await?;
        let (base, carried) = match &run.workspace {
            Some(workspace) => (workspace.base.clone(), Some(workspace.clone())),
            None => (base_branch(environment), None),
        };
        let branch = carried
            .as_ref()
            .map(|workspace| workspace.branch.clone())
            .unwrap_or_else(|| run_branch(run));
        let git = self.git.clone();
        let (worktree, name) = (path.clone(), branch.clone());
        let base_commit = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let base_commit = repo
                .branch_tip(&base)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {base} does not exist")))?;
            if worktree.exists() {
                let canonical = std::fs::canonicalize(&worktree)
                    .map_err(|err| Error::Invalid(err.to_string()))?;
                let registered = repo.worktrees().map_err(git_error)?.into_iter().find(|item| {
                    item.path == worktree ||
                        std::fs::canonicalize(&item.path).is_ok_and(|path| path == canonical)
                });
                if !registered.is_some_and(|item| item.branch.as_deref() == Some(name.as_str())) {
                    return Err(Error::Invalid(format!(
                        "The run's worktree {} no longer holds branch {name}. Restore that checkout before continuing.",
                        worktree.display()
                    )));
                }
                return Ok(base_commit);
            }
            if let Some(parent) = worktree.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            let spec = match repo.branch_tip(&name).map_err(git_error)? {
                Some(_) => WorktreeSpec::Branch { name },
                None => WorktreeSpec::NewBranch {
                    name,
                    start: base_commit.clone(),
                },
            };
            repo.add_worktree(&worktree, spec).map_err(git_error)?;
            Ok(base_commit)
        })
        .await?;
        Ok(match carried {
            Some(workspace) => RunWorkspace {
                path: path.to_string_lossy().into_owned(),
                ..workspace
            },
            None => RunWorkspace {
                base: base_branch(environment),
                base_commit: base_commit.0,
                branch,
                path: path.to_string_lossy().into_owned(),
            },
        })
    }
}

impl SessionManager {
    /// A closing session lets go of its runs' worktrees (PLAN.md §10.5 keeps them for
    /// Continue and Merge while the session lives; Continue after a restore makes the worktree
    /// again from the run branch). Changes in one are kept as a WIP commit on its run branch
    /// first; one whose changes can't be kept stays, with everything in it. Each worktree goes
    /// once, for every run segment that shared it, and never while a run still uses it.
    pub(crate) async fn release_run_worktrees(&self, runs: &[OvernightRun]) {
        let ledger = self.runtime.ledger();
        let held = releasable_worktrees(runs, &ledger.owners(), |path| {
            std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
        });
        for worktree in held {
            let (git, path) = (self.git.clone(), PathBuf::from(&worktree.path));
            let kept = blocking(move || {
                if path.exists() {
                    super::super::disk::keep_changes(&git, &path)?;
                }
                Ok(())
            })
            .await;
            if let Err(err) = kept {
                tracing::error!(worktree = %worktree.path, error = %err, "could not keep a run worktree's changes; it stays");
                continue;
            }
            for owner in &worktree.owners {
                let leftovers = ledger.dispose(owner).await;
                if !leftovers.is_clean() {
                    tracing::warn!(
                        owner,
                        ?leftovers,
                        "some leftovers will be retried at the next launch"
                    );
                }
            }
        }
    }
}

/// A run worktree that may go, and every ledger owner (one per run segment) holding it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HeldWorktree {
    pub path: String,
    pub owners: Vec<String>,
}

/// The worktrees `runs`' owners hold, one entry per place (`real` resolves a path through
/// symbolic links), except a worktree a run still active uses or one an owner outside these
/// runs holds too.
pub(crate) fn releasable_worktrees(
    runs: &[OvernightRun],
    owners: &[(String, Vec<Artifact>, bool)],
    real: impl Fn(&str) -> PathBuf,
) -> Vec<HeldWorktree> {
    let mine: HashSet<String> = runs.iter().map(run_owner).collect();
    let in_use: HashSet<PathBuf> = runs
        .iter()
        .filter(|run| run.state.is_active())
        .filter_map(|run| run.workspace.as_ref())
        .map(|workspace| real(&workspace.path))
        .collect();
    let mut held: BTreeMap<PathBuf, HeldWorktree> = BTreeMap::new();
    let mut foreign = HashSet::new();
    for (owner, artifacts, _) in owners {
        for artifact in artifacts {
            let Artifact::Worktree { path, .. } = artifact else {
                continue;
            };
            let place = real(path);
            if !mine.contains(owner) {
                foreign.insert(place);
                continue;
            }
            let entry = held.entry(place).or_insert_with(|| HeldWorktree {
                path: path.clone(),
                owners: Vec::new(),
            });
            if !entry.owners.contains(owner) {
                entry.owners.push(owner.clone());
            }
        }
    }
    held.into_iter()
        .filter(|(place, _)| !in_use.contains(place) && !foreign.contains(place))
        .map(|(_, worktree)| worktree)
        .collect()
}

/// Who owns a run's worktree and evidence in the cleanup ledger.
pub(crate) fn run_owner(run: &OvernightRun) -> String {
    format!("overnight:{}", run.id)
}

/// The branch a run starts from: the session's own branch (the picked branch of a local
/// checkout, a worktree session's branch once it exists, else its base).
fn base_branch(environment: &Environment) -> String {
    match environment {
        Environment::LocalCheckout { branch } => branch.clone(),
        Environment::NewWorktree {
            branch, base, path, ..
        } => {
            if path.is_some() {
                branch.clone()
            } else {
                base.clone()
            }
        }
    }
}

/// `overnight/<date>-<slug>-<short id>`.
fn run_branch(run: &OvernightRun) -> String {
    let date = jiff::Timestamp::from_millisecond(run.started_at_ms.unwrap_or(run.created_at_ms))
        .map(|at| at.to_zoned(jiff::tz::TimeZone::system()).date().to_string())
        .unwrap_or_else(|_| "run".into());
    let slug: String = run
        .name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    let slug: String = slug.chars().take(32).collect();
    let slug = slug.trim_end_matches('-');
    let short = run.id.short();
    if slug.is_empty() {
        format!("overnight/{date}-{short}")
    } else {
        format!("overnight/{date}-{slug}-{short}")
    }
}

impl SessionManager {
    /// The branch a run task's work lands on: its run's branch, once Start made it.
    pub(crate) async fn run_target(
        &self,
        conversation_id: &crate::model::ConversationId,
        context: &crate::overnight::RunTaskContext,
    ) -> Result<String> {
        let board = self.core.board(conversation_id).await?;
        board
            .runs
            .get(&context.run_id)
            .and_then(|run| run.workspace.as_ref())
            .map(|workspace| workspace.branch.clone())
            .ok_or_else(|| Error::Invalid("the overnight run has no branch yet".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ConversationId, OvernightRunId};
    use crate::overnight::OvernightState;

    fn segment(id: &str, state: OvernightState, path: &str) -> OvernightRun {
        // The night of 2026-10-03 (the app's fixture of it), as a segment of its own.
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../apps/desktop/src/fixtures/boards/overnight-2026-10-03.json"
        ))
        .expect("the fixture");
        let mut run: OvernightRun = serde_json::from_value(
            fixture["overnight"]
                .as_object()
                .and_then(|runs| runs.values().next())
                .cloned()
                .expect("the run"),
        )
        .expect("a run");
        run.id = OvernightRunId(id.into());
        run.conversation_id = ConversationId("c1".into());
        run.state = state;
        run.workspace = Some(RunWorkspace {
            base: "main".into(),
            base_commit: "abc".into(),
            branch: "overnight/2026-10-03-faster-runs-1".into(),
            path: path.into(),
        });
        run
    }

    fn holds(owner: &str, path: &str) -> (String, Vec<Artifact>, bool) {
        (
            owner.into(),
            vec![Artifact::Worktree {
                repo: "/repo".into(),
                path: path.into(),
            }],
            false,
        )
    }

    #[test]
    fn segments_sharing_a_worktree_release_it_once_and_never_while_one_runs() {
        // Continue's second segment works in the first one's worktree (once through a link).
        let runs = [
            segment("r1", OvernightState::Finished, "/wt/overnight-1"),
            segment("r2", OvernightState::Finished, "/link/overnight-1"),
        ];
        let owners = [
            holds("overnight:r1", "/wt/overnight-1"),
            holds("overnight:r2", "/link/overnight-1"),
            holds("session:c1", "/wt/session-1"),
        ];
        let real = |path: &str| PathBuf::from(path.replace("/link/", "/wt/"));
        assert_eq!(
            releasable_worktrees(&runs, &owners, real),
            [HeldWorktree {
                path: "/wt/overnight-1".into(),
                owners: vec!["overnight:r1".into(), "overnight:r2".into()],
            }]
        );
        // While the second segment still runs, the shared worktree stays for both.
        let running = [
            runs[0].clone(),
            segment("r2", OvernightState::Running, "/link/overnight-1"),
        ];
        assert!(releasable_worktrees(&running, &owners, real).is_empty());
        // Nor does it go while an owner beyond these runs holds it.
        let shared = [
            holds("overnight:r1", "/wt/overnight-1"),
            holds("task:t9", "/wt/overnight-1"),
        ];
        assert!(releasable_worktrees(&runs[..1], &shared, real).is_empty());
    }
}
