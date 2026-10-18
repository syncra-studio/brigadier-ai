//! A fresh base: new tab and worker branches start from what the remote has, not from a local
//! branch that fell behind it. Before a worktree is made, its base's remote is fetched (at most
//! once per repository in [`FETCH_EVERY`], stopped after [`FETCH_PATIENCE`]; offline or no
//! remote keeps the local tip), and the work starts from the newer of the local base and its
//! upstream: the upstream when the local one is only behind it, the local one otherwise, so the
//! user's own unpushed commits always win.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use brigadier_git::{Oid, UpstreamGap};

use super::{SessionManager, blocking, git_error};
use crate::Result;

/// How long making a worktree waits for its base's fetch.
const FETCH_PATIENCE: Duration = Duration::from_secs(10);
/// A repository fetched this recently isn't fetched again for the next worktree.
const FETCH_EVERY: Duration = Duration::from_secs(30);

/// When each repository was last fetched for a new worktree.
fn fetched() -> &'static Mutex<HashMap<PathBuf, Instant>> {
    static FETCHED: OnceLock<Mutex<HashMap<PathBuf, Instant>>> = OnceLock::new();
    FETCHED.get_or_init(Mutex::default)
}

impl SessionManager {
    /// Where new work from local `branch` of `repo` starts, after fetching its remote: the
    /// upstream's tip with its gap when local `branch` is only behind it, else its own tip.
    pub(super) async fn fresh_tip(
        &self,
        repo: &Path,
        branch: &str,
    ) -> Result<(Oid, Option<UpstreamGap>)> {
        let (git, path, name) = (self.git.clone(), repo.to_owned(), branch.to_owned());
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let due = {
                let mut fetched = fetched().lock().unwrap_or_else(|e| e.into_inner());
                let due = fetched
                    .get(&path)
                    .is_none_or(|at| at.elapsed() >= FETCH_EVERY);
                if due {
                    fetched.insert(path.clone(), Instant::now());
                }
                due
            };
            if due && let Err(err) = repo.fetch_remote_of(&name, FETCH_PATIENCE) {
                tracing::info!(branch = %name, %err, "couldn't fetch the base's remote; starting from what is here");
            }
            repo.newer_tip(&name).map_err(git_error)
        })
        .await
    }
}

/// The thread's note when its branch started from the upstream rather than its local base.
pub(super) fn started_from_upstream_line(base: &str, gap: &UpstreamGap) -> String {
    format!(
        "[git] Your branch starts from `{}` at {}, the newest `{base}`: the local `{base}` is {} commit{} behind it.",
        gap.name,
        &gap.tip.0[..gap.tip.0.len().min(10)],
        gap.behind,
        if gap.behind == 1 { "" } else { "s" },
    )
}
