//! The pinned card's pull request row ("Existing pull request"): the GitHub pull
//! request of a session's branch, looked up with the user's `gh`. Read only: the row never
//! creates or edits one; the thread opens and updates pull requests itself as it judges (Q61).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

use brigadier_git::Oid;

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
    #[serde(default)]
    head_ref_oid: Option<String>,
    #[serde(default)]
    base_ref_name: Option<String>,
    #[serde(default)]
    merge_commit: Option<GhCommit>,
}

#[derive(Deserialize)]
struct GhCommit {
    oid: String,
}

/// A branch's pull request as `gh` gives it, with the head it merged and the commit its
/// merge made on the base (for squash and rebase merges too).
struct FoundPr {
    pr: PullRequest,
    /// The branch it targets.
    base: Option<String>,
    head: Option<String>,
    merge_commit: Option<String>,
}

impl SessionManager {
    /// The pull request of the session checkout's branch, newest first; none when there is
    /// none, the branch isn't on GitHub, or `gh` is missing or signed out.
    pub async fn pull_request(&self, id: &ConversationId) -> Result<Option<PullRequest>> {
        let checkout = PathBuf::from(self.checkout_dir(id)?);
        let Some(gh) = self.gh() else {
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
            Ok(match run_gh(&gh.program, &checkout, &branch, &gh.vars) {
                Ok(found) => found.map(|found| found.pr),
                Err(reason) => {
                    tracing::debug!(branch, reason, "no pull request from gh");
                    None
                }
            })
        })
        .await
    }
}

impl SessionManager {
    /// A request of `id` ended: when its session branch's pull request was merged on GitHub
    /// (by the thread's `gh pr merge` or the user), the merge is recorded once, as the local
    /// path records it, with the head it merged and the commit it made on the base, and the
    /// session branch moves onto the new base, so the next request starts from it rather than
    /// from commits a squash or rebase merge rewrote. A branch with changes or commits since
    /// the merged head stays where it is: the thread sees it ahead of the base. A pull request
    /// into another branch than the session's base isn't its merge, and one whose merge commit
    /// can't be fetched yet is left for a later request's end. `request`: the one ending, whose
    /// turn may have merged it.
    pub(super) async fn reconcile_pr_merge(&self, id: &ConversationId, request: &str) {
        if self.overnight.active.get(id).is_some() {
            return;
        }
        let Ok(conversation) = self.core.conversation(id) else {
            return;
        };
        let Some(crate::model::Setup::Session {
            repo,
            environment:
                crate::model::Environment::NewWorktree {
                    base,
                    branch,
                    path: Some(worktree),
                    ..
                },
            ..
        }) = conversation.setup
        else {
            return;
        };
        let Some(gh) = self.gh() else {
            return;
        };
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let _once = conv.pr_merge.lock().await;
        let (git, checkout, name) = (self.git.clone(), PathBuf::from(&worktree), branch.clone());
        // Only a branch that was pushed can have a pull request: no `gh` call otherwise. A push
        // without `-u` leaves no upstream, but its remote-tracking branch; a clone that fetches
        // only main leaves no remote-tracking branch, but the upstream `-u` set.
        let found = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            let state = repo.remote_state(&name).map_err(git_error)?;
            let pushed = state.upstream.is_some()
                || repo.upstream_configured(&name).map_err(git_error)?
                || state.remote.is_some_and(|remote| {
                    repo.resolve(&format!("refs/remotes/{remote}/{name}"))
                        .is_ok()
                });
            if !pushed {
                return Ok(None);
            }
            Ok(run_gh(&gh.program, &checkout, &name, &gh.vars)
                .ok()
                .flatten())
        })
        .await
        .ok()
        .flatten();
        let Some(FoundPr {
            pr,
            head: Some(head),
            merge_commit: Some(merge_commit),
            ..
        }) = found.filter(|found| {
            found.pr.state == PullRequestState::Merged && found.base.as_deref() == Some(&base)
        })
        else {
            return;
        };
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, crate::work::OrchestratorStepKind::Merged { pr: Some(n), .. } if *n == pr.number)
        }) {
            return;
        }
        // The base as GitHub left it, here too.
        self.sync_base(&repo, &base).await;
        let (git, checkout, merged, records, own, ready) = (
            self.git.clone(),
            PathBuf::from(&worktree),
            merge_commit.clone(),
            self.ref_records.clone(),
            branch.clone(),
            board.unmerged_ready(),
        );
        let head_oid = Oid(head.clone());
        let moved = blocking(move || {
            let repo = git.open(&checkout).map_err(git_error)?;
            // Not fetched (the fetch failed): nothing moves or is recorded until it is.
            let onto = repo.resolve(&merged).map_err(git_error)?;
            // Counted from where the base was before the merge: a merge commit's own head is
            // already in it.
            let commits = repo
                .resolve(&format!("{}^1", onto.0))
                .and_then(|before| repo.merge_base(&head_oid, &before))
                .and_then(|from| repo.count_commits(&from, &head_oid))
                .unwrap_or(0);
            // The requests whose ready head it merged.
            let covered: Vec<String> = ready
                .into_iter()
                .filter(|(_, ready)| {
                    let ready = Oid(ready.clone());
                    repo.merge_base(&ready, &head_oid)
                        .is_ok_and(|base| base == ready)
                })
                .map(|(request, _)| request)
                .collect();
            let clean = repo.state().map_err(git_error)?.dirty_files.is_empty();
            let at_head = repo.resolve("HEAD").map_err(git_error)? == head_oid;
            // Under the ref lock, its new tip recorded: no ref check puts the old one back.
            let moved = clean
                && at_head
                && super::refcheck::engine_rewrite(&repo, &records, &own, || {
                    repo.reset_keep(&onto, &head_oid)
                })
                .is_ok();
            Ok((commits, moved, onto, covered, at_head))
        })
        .await;
        let (commits, moved, onto, mut requests, at_head) = match moved {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "a merged pull request couldn't be reconciled");
                return;
            }
        };
        tracing::info!(conversation = %id, pr = pr.number, moved, onto = %onto.0, "a merged pull request recorded");
        // The ending request's work is in it only while its head is the merged one.
        if at_head && !requests.iter().any(|covered| covered == request) {
            requests.push(request.to_owned());
        }
        self.orchestrator_step_in(
            id,
            requests.last().cloned(),
            crate::work::OrchestratorStepKind::Merged {
                branch,
                base,
                commits,
                asked_in: None,
                upstream: None,
                head: Some(head),
                merge_commit: Some(onto.0),
                pr: Some(pr.number),
                auto: self.auto_merge_in(id),
                requests,
            },
        )
        .await;
    }
}

fn run_gh(
    gh: &Path,
    checkout: &Path,
    branch: &str,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::result::Result<Option<FoundPr>, String> {
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
            "number,title,url,state,isDraft,headRefOid,baseRefName,mergeCommit",
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
    Ok(found.into_iter().next().map(|pr| FoundPr {
        base: pr.base_ref_name,
        head: pr.head_ref_oid,
        merge_commit: pr.merge_commit.map(|commit| commit.oid),
        pr: PullRequest {
            number: pr.number,
            title: pr.title,
            url: pr.url,
            state: match (pr.state.as_str(), pr.is_draft) {
                ("MERGED", _) => PullRequestState::Merged,
                ("CLOSED", _) => PullRequestState::Closed,
                (_, true) => PullRequestState::Draft,
                _ => PullRequestState::Open,
            },
        },
    }))
}
