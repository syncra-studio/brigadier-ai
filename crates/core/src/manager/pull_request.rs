//! The pinned card's pull request row ("Existing pull request"): the GitHub pull
//! request of a session's branch, looked up with the user's `gh`. Read only: the row never
//! creates or edits one; the thread opens and updates pull requests itself as it judges (Q61).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;

use brigadier_git::Oid;

use super::conversation::ConvLive;
use super::{SessionManager, blocking, git_error};
use crate::Result;
use crate::model::ConversationId;
use crate::work::{PullRequest, PullRequestCi, PullRequestState};

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
    /// Set while GitHub's auto-merge waits to merge it (`gh pr merge --auto`).
    #[serde(default)]
    auto_merge_request: Option<serde_json::Value>,
    /// The card's lookup only: its checks, review decision and whether it merges cleanly.
    #[serde(default)]
    status_check_rollup: Option<Vec<GhCheck>>,
    #[serde(default)]
    review_decision: Option<String>,
    #[serde(default)]
    mergeable: Option<String>,
}

/// One of a pull request's checks: a check run (status and conclusion) or a commit status
/// (state).
#[derive(Deserialize, Default)]
#[serde(default)]
struct GhCheck {
    status: Option<String>,
    conclusion: Option<String>,
    state: Option<String>,
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
    /// GitHub's auto-merge is on for it.
    auto_merge: bool,
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
            Ok(
                match run_gh(&gh.program, &checkout, &branch, &gh.vars, true) {
                    Ok(found) => found.map(|found| {
                        let mut pr = found.pr;
                        if matches!(pr.state, PullRequestState::Open | PullRequestState::Draft) {
                            match unresolved_threads(&gh.program, &checkout, &pr, &gh.vars) {
                                Ok(count) => pr.unresolved = count,
                                Err(reason) => tracing::debug!(reason, "no review threads from gh"),
                            }
                        }
                        pr
                    }),
                    Err(reason) => {
                        tracing::debug!(branch, reason, "no pull request from gh");
                        None
                    }
                },
            )
        })
        .await
    }
}

/// The pull request of `checkout`'s session branch `name`, when that branch was pushed: none
/// when it wasn't (no `gh` call), an error when `gh` couldn't tell. Only a branch that was
/// pushed can have a pull request. A push without `-u` leaves no upstream, but its
/// remote-tracking branch; a clone that fetches only main leaves no remote-tracking branch,
/// but the upstream `-u` set.
async fn lookup_pushed(
    git: brigadier_git::Git,
    checkout: PathBuf,
    name: String,
    gh: super::ci::Gh,
) -> Option<std::result::Result<Option<FoundPr>, String>> {
    blocking(move || {
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
        Ok(Some(run_gh(&gh.program, &checkout, &name, &gh.vars, false)))
    })
    .await
    .ok()
    .flatten()
}

/// How often a connect may look up a ready session's pull request again.
const PR_LOOKUP_EVERY: Duration = Duration::from_secs(120);

impl SessionManager {
    /// Looks `id`'s pull request up in the background (a ready session, read at a connect),
    /// at most every [`PR_LOOKUP_EVERY`]: a pull request opened, closed or merged on GitHub
    /// between its turns reaches its row. A turn's start and end look it up anyway
    /// ([`Self::reconcile_pr_merge`]).
    pub(super) fn refresh_pull_request_soon(&self, id: &ConversationId) {
        {
            let mut looked_up = self.pr_looked_up.lock().unwrap();
            if looked_up
                .get(id)
                .is_some_and(|at| at.elapsed() < PR_LOOKUP_EVERY)
            {
                return;
            }
            looked_up.insert(id.clone(), Instant::now());
        }
        let (manager, id) = (self.arc(), id.clone());
        self.spawn(async move {
            manager.session_pr(&id).await;
        });
    }
}

/// A session's worktree and the pull request of its pushed branch.
struct SessionPr {
    found: FoundPr,
    repo: String,
    base: String,
    branch: String,
    worktree: String,
}

/// How often a pull request left open with the session's work ready is looked at: soon while
/// GitHub's auto-merge is on for it, or in the first half hour (a merge queue takes it at once,
/// and `gh` doesn't show it in one), else later; for a day at most.
#[cfg(not(test))]
const PR_WATCH: [Duration; 3] = [
    Duration::from_secs(30),
    Duration::from_secs(120),
    Duration::from_secs(24 * 60 * 60),
];
#[cfg(test)]
const PR_WATCH: [Duration; 3] = [
    Duration::from_millis(100),
    Duration::from_millis(100),
    Duration::from_secs(120),
];
const PR_WATCH_SOON_FOR: Duration = Duration::from_secs(30 * 60);

impl SessionManager {
    /// The pull request of `id`'s session branch, when it has a worktree whose branch was
    /// pushed and `gh` finds one. What `gh` answered is recorded for the session's row.
    async fn session_pr(&self, id: &ConversationId) -> Option<SessionPr> {
        let conversation = self.core.conversation(id).ok()?;
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
            return None;
        };
        let gh = self.gh()?;
        let (git, checkout, name) = (self.git.clone(), PathBuf::from(&worktree), branch.clone());
        let found = lookup_pushed(git, checkout, name, gh).await;
        // What it was seen as, for its row ("PR open"): only when `gh` answered.
        if let Some(Ok(seen)) = &found
            && let Err(err) = self
                .core
                .record_pull_request(id, seen.as_ref().map(|found| found.pr.clone()))
                .await
        {
            tracing::debug!(conversation = %id, error = %err, "could not record a pull request");
        }
        let found = found.and_then(|found| found.ok()).flatten()?;
        Some(SessionPr {
            found,
            repo,
            base,
            branch,
            worktree,
        })
    }

    /// Watches the session's open pull request while its work waits ready to merge: once
    /// GitHub merged it (a merge queue, auto-merge after its checks, or the user), the merge
    /// is recorded at the session's next idle moment ([`ConvLive::pr_merge_due`]): never
    /// under a running request, whose end records it itself. One watch per session.
    fn watch_pr(&self, id: &ConversationId) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        if conv
            .pr_watch
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let manager = self.arc();
        self.spawn(async move {
            manager.watching_pr(&conv).await;
            conv.pr_watch
                .store(false, std::sync::atomic::Ordering::SeqCst);
        });
    }

    async fn watching_pr(&self, conv: &Arc<ConvLive>) {
        let [soon, later, longest] = PR_WATCH;
        let start = Instant::now();
        let mut wait = soon;
        loop {
            tokio::time::sleep(wait).await;
            if self.admit().is_err() || !self.is_attached(conv) || start.elapsed() >= longest {
                return;
            }
            // Recorded or merged here meanwhile: nothing left to watch for.
            let ready = self
                .core
                .board(&conv.id)
                .await
                .is_ok_and(|board| !board.unmerged_ready().is_empty());
            if !ready {
                return;
            }
            let Some(SessionPr { found, base, .. }) = self.session_pr(&conv.id).await else {
                // `gh` failed this time, or the branch has no pull request (any more).
                wait = later;
                continue;
            };
            if found.base.as_deref() != Some(&base) {
                return;
            }
            match found.pr.state {
                PullRequestState::Merged => {
                    conv.pr_merge_due
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    self.kick(conv);
                    return;
                }
                PullRequestState::Open | PullRequestState::Draft => {
                    wait = if found.auto_merge || start.elapsed() < PR_WATCH_SOON_FOR {
                        soon
                    } else {
                        later
                    };
                }
                PullRequestState::Closed => return,
            }
        }
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
    /// turn may have merged it; none when a new request is about to start (the user merged it
    /// on GitHub while the tab was idle), so that one starts from the new base too. Sibling tabs
    /// hear of the merge as of a local one, and the thread that its remote branch may still
    /// hold the commits the merge rewrote.
    pub(super) async fn reconcile_pr_merge(&self, id: &ConversationId, request: Option<&str>) {
        if self.overnight.active.get(id).is_some() {
            return;
        }
        let Ok(conv) = self.conv(id) else {
            return;
        };
        let _once = conv.pr_merge.lock().await;
        let Some(SessionPr {
            found,
            repo,
            base,
            branch,
            worktree,
        }) = self.session_pr(id).await
        else {
            return;
        };
        // Left open (queued, or waiting for its checks with auto-merge on): watched, so its
        // merge is recorded when GitHub lands it, not only at the next request.
        if found.pr.state == PullRequestState::Open && found.base.as_deref() == Some(&base) {
            self.watch_pr(id);
        }
        let found = Some(found);
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
        if let Some(request) = request
            && at_head
            && !requests.iter().any(|covered| covered == request)
        {
            requests.push(request.to_owned());
        }
        let (named, onto_base) = (branch.clone(), base.clone());
        let auto = self.auto_merge_in(id);
        let record = self.merge_record(id, &worktree, &branch, &head, auto).await;
        // A checkout moved to the merge has the base's other work too: that is merged as well.
        let merged = if moved { &onto.0 } else { &head };
        self.merged_pending(id, &requests, merged).await;
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
                merge_commit: Some(onto.0.clone()),
                pr: Some(pr.number),
                auto,
                requests,
                record,
            },
        )
        .await;
        self.tell_merged(id, &onto_base, &onto.0).await;
        // GitHub may keep the pull request's branch: a push of the moved branch to it is no
        // fast-forward then. A fact for the thread to judge.
        if moved && let Ok(conv) = self.conv(id) {
            let short = &onto.0[..onto.0.len().min(12)];
            conv.hold_fact(format!(
                "[pull request] PR #{} was merged on GitHub; your branch `{named}` now starts from `{onto_base}` at {short}. If GitHub kept the branch, its copy there still holds the commits before the merge: a push of new work to it needs `--force-with-lease`, or use a new branch.",
                pr.number,
            ))
            .await;
        }
    }
}

/// The branch's pull request; `card` adds what the pinned card's buttons need (checks, review
/// decision, conflicts), which the merge watch has no use for.
fn run_gh(
    gh: &Path,
    checkout: &Path,
    branch: &str,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
    card: bool,
) -> std::result::Result<Option<FoundPr>, String> {
    let mut fields =
        "number,title,url,state,isDraft,headRefOid,baseRefName,mergeCommit,autoMergeRequest"
            .to_owned();
    if card {
        fields.push_str(",statusCheckRollup,reviewDecision,mergeable");
    }
    let text = gh_output(
        gh,
        checkout,
        &[
            "pr", "list", "--head", branch, "--state", "all", "--limit", "1", "--json", &fields,
        ],
        env,
    )?;
    let found: Vec<GhPullRequest> = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    Ok(found.into_iter().next().map(found_pr))
}

fn found_pr(pr: GhPullRequest) -> FoundPr {
    let merge_commit = pr.merge_commit.map(|commit| commit.oid);
    FoundPr {
        base: pr.base_ref_name.clone(),
        head: pr.head_ref_oid,
        merge_commit: merge_commit.clone(),
        auto_merge: pr
            .auto_merge_request
            .is_some_and(|request| !request.is_null()),
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
            base: pr.base_ref_name,
            merge_commit,
            ci: pr.status_check_rollup.as_deref().and_then(checks_state),
            conflicts: pr.mergeable.as_deref() == Some("CONFLICTING"),
            changes_requested: pr.review_decision.as_deref() == Some("CHANGES_REQUESTED"),
            unresolved: 0,
        },
    }
}

/// All of a pull request's checks as one state: failing when any failed, else pending while
/// any runs, else passing; none when it has no checks.
fn checks_state(checks: &[GhCheck]) -> Option<PullRequestCi> {
    if checks.is_empty() {
        return None;
    }
    let failed = |check: &GhCheck| {
        matches!(
            check.conclusion.as_deref(),
            Some("FAILURE" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE")
        ) || matches!(check.state.as_deref(), Some("FAILURE" | "ERROR"))
    };
    let running = |check: &GhCheck| {
        check
            .status
            .as_deref()
            .is_some_and(|status| status != "COMPLETED")
            || matches!(check.state.as_deref(), Some("PENDING" | "EXPECTED"))
    };
    Some(if checks.iter().any(failed) {
        PullRequestCi::Failing
    } else if checks.iter().any(running) {
        PullRequestCi::Pending
    } else {
        PullRequestCi::Passing
    })
}

const REVIEW_THREADS: &str = "query($owner: String!, $name: String!, $number: Int!, $endCursor: String) { repository(owner: $owner, name: $name) { pullRequest(number: $number) { reviewThreads(first: 100, after: $endCursor) { nodes { isResolved } pageInfo { hasNextPage endCursor } } } } }";

/// How many of the pull request's review threads aren't resolved, every page of them: an
/// approved pull request can still have comments waiting.
fn unresolved_threads(
    gh: &Path,
    checkout: &Path,
    pr: &PullRequest,
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::result::Result<u32, String> {
    // https://host/owner/name/pull/N
    let parts: Vec<&str> = pr.url.split('/').collect();
    let [_, _, host, owner, name, ..] = parts.as_slice() else {
        return Err(format!("no repository in {}", pr.url));
    };
    let (query, owner, name, number) = (
        format!("query={REVIEW_THREADS}"),
        format!("owner={owner}"),
        format!("name={name}"),
        format!("number={}", pr.number),
    );
    let text = gh_output(
        gh,
        checkout,
        &[
            "api",
            "graphql",
            "--hostname",
            host,
            "--paginate",
            "--slurp",
            "-f",
            &query,
            "-f",
            &owner,
            "-f",
            &name,
            "-F",
            &number,
        ],
        env,
    )?;
    count_unresolved(&text)
}

/// Counts the unresolved threads in `gh api graphql --paginate --slurp`'s pages.
fn count_unresolved(text: &str) -> std::result::Result<u32, String> {
    let pages: Vec<serde_json::Value> =
        serde_json::from_str(text).map_err(|err| err.to_string())?;
    let mut count = 0;
    for page in &pages {
        let nodes = page
            .pointer("/data/repository/pullRequest/reviewThreads/nodes")
            .and_then(|nodes| nodes.as_array())
            .ok_or("no review threads in gh's answer")?;
        count += nodes
            .iter()
            .filter(|node| node.get("isResolved") == Some(&serde_json::Value::Bool(false)))
            .count();
    }
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

/// Runs `gh` with `args` in `checkout`, with its own environment and a deadline; its output.
fn gh_output(
    gh: &Path,
    checkout: &Path,
    args: &[&str],
    env: &[(std::ffi::OsString, std::ffi::OsString)],
) -> std::result::Result<String, String> {
    let mut child = Command::new(gh)
        .args(args)
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
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_read_as_failing_then_pending_then_passing() {
        let rollup = |json: &str| {
            let checks: Vec<GhCheck> = serde_json::from_str(json).unwrap();
            checks_state(&checks)
        };
        assert_eq!(rollup("[]"), None);
        assert_eq!(
            rollup(
                r#"[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"},{"__typename":"StatusContext","state":"SUCCESS"},{"status":"COMPLETED","conclusion":"SKIPPED"}]"#
            ),
            Some(PullRequestCi::Passing)
        );
        assert_eq!(
            rollup(
                r#"[{"status":"COMPLETED","conclusion":"SUCCESS"},{"status":"IN_PROGRESS","conclusion":""}]"#
            ),
            Some(PullRequestCi::Pending)
        );
        assert_eq!(
            rollup(r#"[{"state":"PENDING"},{"status":"COMPLETED","conclusion":"SUCCESS"}]"#),
            Some(PullRequestCi::Pending)
        );
        assert_eq!(
            rollup(r#"[{"status":"IN_PROGRESS"},{"status":"COMPLETED","conclusion":"FAILURE"}]"#),
            Some(PullRequestCi::Failing)
        );
        assert_eq!(
            rollup(r#"[{"state":"ERROR"}]"#),
            Some(PullRequestCi::Failing)
        );
    }

    #[test]
    fn an_approved_pull_request_with_conflicts_and_open_threads_reads_so() {
        let pr: GhPullRequest = serde_json::from_str(
            r#"{"number":7,"title":"Add notes","url":"https://github.com/o/n/pull/7","state":"OPEN","isDraft":false,"baseRefName":"main","reviewDecision":"APPROVED","mergeable":"CONFLICTING","statusCheckRollup":[]}"#,
        )
        .unwrap();
        let pr = found_pr(pr).pr;
        assert!(pr.conflicts);
        assert!(!pr.changes_requested);
        assert_eq!(pr.ci, None);
        assert_eq!(pr.base.as_deref(), Some("main"));
    }

    #[test]
    fn unresolved_threads_are_counted_on_every_page() {
        let page = |resolved: &[bool]| {
            serde_json::json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
                "nodes": resolved.iter().map(|done| serde_json::json!({ "isResolved": done })).collect::<Vec<_>>(),
                "pageInfo": { "hasNextPage": false, "endCursor": null },
            } } } } })
        };
        let pages = serde_json::json!([page(&[true, false, true]), page(&[false, false])]);
        assert_eq!(count_unresolved(&pages.to_string()), Ok(3));
        assert_eq!(count_unresolved("[]"), Ok(0));
        assert!(count_unresolved(r#"[{"errors":[{"message":"no"}]}]"#).is_err());
    }
}
