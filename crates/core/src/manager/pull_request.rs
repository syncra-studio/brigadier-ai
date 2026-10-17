//! The pinned card's pull request row ("Existing pull request"): the GitHub pull
//! request of a session's branch, looked up with the user's `gh`. Read only: the row never
//! creates or edits one; the thread opens and updates pull requests itself as it judges (Q61).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{SessionManager, blocking, git_error};
use crate::Result;
use crate::model::ConversationId;
use crate::work::{PullRequest, PullRequestState};

/// How long `gh` may take (it asks GitHub) before the row is left out.
const GH_TIMEOUT: Duration = Duration::from_secs(10);
const GH_POLL: Duration = Duration::from_millis(50);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u32,
    title: String,
    url: String,
    state: String,
    is_draft: bool,
}

impl SessionManager {
    /// The pull request of the session checkout's branch, newest first; none when there is
    /// none, the branch isn't on GitHub, or `gh` is missing or signed out.
    pub async fn pull_request(&self, id: &ConversationId) -> Result<Option<PullRequest>> {
        let checkout = PathBuf::from(self.checkout_dir(id)?);
        let env = self.runtime.cli_env().clone();
        let Some(gh) = env.which("gh") else {
            return Ok(None);
        };
        let git = self.git.clone();
        blocking(move || {
            let branch = git
                .open(&checkout)
                .and_then(|repo| repo.state())
                .map_err(git_error)?
                .current_branch;
            let Some(branch) = branch else {
                return Ok(None);
            };
            Ok(match run_gh(&gh, &checkout, &branch, &env.vars()) {
                Ok(found) => found,
                Err(reason) => {
                    tracing::debug!(branch, reason, "no pull request from gh");
                    None
                }
            })
        })
        .await
    }
}

fn run_gh(
    gh: &Path,
    checkout: &Path,
    branch: &str,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::result::Result<Option<PullRequest>, String> {
    let mut child = Command::new(gh)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "all",
            "--limit",
            "1",
            "--json",
            "number,title,url,state,isDraft",
        ])
        .current_dir(checkout)
        .env_clear()
        .envs(env.iter().map(|(key, value)| (key, value)))
        .env("GH_PROMPT_DISABLED", "1")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| err.to_string())?;
    // Read on the side, so a full pipe can't stall `gh` while the deadline is watched.
    let mut stdout = child.stdout.take().ok_or("no output")?;
    let reader = std::thread::spawn(move || {
        let mut text = String::new();
        stdout.read_to_string(&mut text).map(|_| text)
    });
    let deadline = Instant::now() + GH_TIMEOUT;
    let status = loop {
        match child.try_wait().map_err(|err| err.to_string())? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("gh timed out".into());
            }
            None => std::thread::sleep(GH_POLL),
        }
    };
    let text = reader
        .join()
        .map_err(|_| "reading gh's output failed".to_owned())?
        .map_err(|err| err.to_string())?;
    if !status.success() {
        return Err(format!("gh exited with {status}"));
    }
    let found: Vec<GhPullRequest> = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    Ok(found.into_iter().next().map(|pr| PullRequest {
        number: pr.number,
        title: pr.title,
        url: pr.url,
        state: match (pr.state.as_str(), pr.is_draft) {
            ("MERGED", _) => PullRequestState::Merged,
            ("CLOSED", _) => PullRequestState::Closed,
            (_, true) => PullRequestState::Draft,
            _ => PullRequestState::Open,
        },
    }))
}
