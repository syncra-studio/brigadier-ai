//! Reviews scaled to risk (BUILD-PLAN B2): the thread's own changes for a request are measured
//! together, not per commit, and get one review by the other vendor only once they are big
//! enough or marked risky.
//!
//! - **The unreviewed range** ([`UnreviewedRange`]) holds every change the thread made for its
//!   newest request since the last one a review covered. In a worktree it runs from the commit
//!   before the first of them to the last commit looked at; on a local checkout, where the
//!   thread leaves its changes uncommitted, from a snapshot of the checkout's files when the
//!   request started (or after the last review) to a snapshot of them now
//!   ([`brigadier_git::Repo::snapshot_files`]: a commit no ref names, written without the
//!   user's index, HEAD, refs or files). It is recorded, so a restart keeps it.
//! - **The size line** ([`REVIEW_LINE`]): once the range changes that many lines of code, or
//!   more than that many files of code (tests and docs aside, [`counts_as_a_file`]), one review
//!   of the whole range starts in the background, and the range starts again after it. A
//!   smaller range gets none: the thread's own checks cover it.
//! - **The risky mark**: `run_check`'s `risky` makes the range owe a review whatever its size.
//! - **Before the final answer** (C2): `run_check`, which the thread calls before it answers,
//!   brings the range up to date first, its uncommitted changes in a worktree included (from
//!   a snapshot, whose review covers the commit with the same files that follows); when that starts a review (or one of the request's
//!   runs), its reply says so and asks the thread to wait for the findings. The end of every
//!   thread turn does the same, as the backstop.
//! - **On a passing check** (C10): while the thread's `run_check` runs, or on the files its
//!   newest one failed on, no review starts; the range waits, and the check's pass starts its
//!   review from the check's own reply (or the message it sends from the background). A
//!   reviewer reading work the thread is still fixing spends findings on what its checks say.
//! - **Reviews off** ("Review the work"): no review starts, and nothing waits for one.
//!
//! Worker landings keep their own review, and the thread's commits that fix findings their
//! re-check ([`SessionManager::thread_fixes`]): neither counts in the range.

use std::path::Path;

use brigadier_git::Oid;

use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent, Environment, Setup};
use crate::work::{ReviewFor, ReviewKind, ReviewRun, ReviewState, UnreviewedRange};

/// Where an unreviewed range gets its review: this many changed lines of code (added and
/// removed) or more, or more than this many files of code, tests and docs aside (the user,
/// Q48). A cheap rule, tuned from measurements (V1).
pub(crate) const REVIEW_LINE: SizeLine = SizeLine {
    lines: 150,
    files: 5,
};

/// The size line of [`REVIEW_LINE`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct SizeLine {
    pub lines: u32,
    pub files: usize,
}

impl SizeLine {
    /// Whether `changed` lines over `files` files cross it.
    pub(crate) fn crossed(self, changed: u32, files: usize) -> bool {
        changed >= self.lines || files > self.files
    }
}

/// How big a range is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Size {
    /// The lines of code changed, tests and docs aside.
    lines: u32,
    /// The files of code changed, tests and docs aside.
    files: usize,
}

/// Whether a changed file is code, which the size line counts. A test or a doc that goes with
/// a change doesn't make it big enough for a review: a file under a `test`, `tests`,
/// `__tests__` or `docs` folder, a `*_test(s).rs`, a `*.test.{ts,tsx,js,jsx}` or a `*.md`.
pub(crate) fn counts_as_a_file(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    if parts
        .iter()
        .any(|dir| matches!(*dir, "test" | "tests" | "__tests__" | "docs"))
    {
        return false;
    }
    let lower = name.to_ascii_lowercase();
    !(lower.ends_with(".md")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || [".test.ts", ".test.tsx", ".test.js", ".test.jsx"]
            .iter()
            .any(|end| lower.ends_with(end)))
}

/// A review of the thread's own work for `request` that still runs.
/// Where the thread's newest `run_check` stands ([`SessionManager::thread_checks`]). In
/// memory only: after a restart a review starts as it did before C10.
#[derive(Debug, Clone)]
pub(crate) struct ThreadCheck {
    /// Which check it is: one that ends after a newer one started changes nothing.
    run: u64,
    /// The tree of the files it runs (or ran) on, when git could tell.
    tree: Option<String>,
    state: CheckState,
}

/// Numbers the thread's checks in the order they start.
static CHECK_RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckState {
    Running,
    Passed,
    Failed,
}

/// A thread check under way: [`Self::ended`] records how it ended. Dropped without that (the
/// check stopped or errored), it leaves nothing, so no review waits on it.
pub(crate) struct CheckUnderWay {
    manager: std::sync::Weak<SessionManager>,
    id: ConversationId,
    run: u64,
    tree: Option<String>,
    ended: bool,
}

impl CheckUnderWay {
    /// The tree of the files the check started on, when git could tell.
    pub(crate) fn tree(&self) -> Option<&str> {
        self.tree.as_deref()
    }

    /// Records how the check ended, unless a newer check of the thread started since: the
    /// newest check is the one that holds a review back or lets it start.
    pub(crate) fn ended(mut self, passed: bool) {
        self.ended = true;
        let state = if passed {
            CheckState::Passed
        } else {
            CheckState::Failed
        };
        if let Some(manager) = self.manager.upgrade() {
            let mut checks = manager.thread_checks.lock().unwrap();
            if let Some(check) = checks.get_mut(&self.id)
                && check.run == self.run
            {
                check.state = state;
            }
        }
    }
}

impl Drop for CheckUnderWay {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        if let Some(manager) = self.manager.upgrade() {
            let mut checks = manager.thread_checks.lock().unwrap();
            if checks
                .get(&self.id)
                .is_some_and(|check| check.run == self.run)
            {
                checks.remove(&self.id);
            }
        }
    }
}

fn running_review<'a>(board: &'a crate::board::Board, request: &str) -> Option<&'a ReviewRun> {
    board.reviews.values().find(|review| {
        review.kind == ReviewKind::Code
            && review.task_id.is_none()
            && review.notify == ReviewFor::Orchestrator
            && review.state == ReviewState::Running
            && review.request_id.as_deref() == Some(request)
    })
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

impl SessionManager {
    /// The session's local checkout, when its thread works there: its repository and branch.
    /// The thread's `run_check` in the checkout at `tree` starts: until it ends, no review of
    /// its own changes starts (BUILD-PLAN C10).
    pub(crate) async fn thread_check_started(
        &self,
        id: &ConversationId,
        tree: Option<&Path>,
    ) -> CheckUnderWay {
        let files = match tree {
            Some(tree) => {
                let (git, tree) = (self.git.clone(), tree.to_owned());
                blocking(move || {
                    git.open_worktree(&tree)
                        .and_then(|worktree| worktree.checkout_tree())
                        .map_err(git_error)
                })
                .await
                .ok()
                .map(|tree| tree.0)
            }
            None => None,
        };
        let run = CHECK_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.thread_checks.lock().unwrap().insert(
            id.clone(),
            ThreadCheck {
                run,
                tree: files.clone(),
                state: CheckState::Running,
            },
        );
        CheckUnderWay {
            manager: self.me.clone(),
            id: id.clone(),
            run,
            tree: files,
            ended: false,
        }
    }

    /// Where the thread's newest check stands: "running", "passed" or "failed" (for the flow
    /// tests, which run on unix only).
    #[cfg(all(test, unix))]
    pub(crate) fn thread_check_state(&self, id: &ConversationId) -> Option<&'static str> {
        self.thread_checks
            .lock()
            .unwrap()
            .get(id)
            .map(|check| match check.state {
                CheckState::Running => "running",
                CheckState::Passed => "passed",
                CheckState::Failed => "failed",
            })
    }

    /// Whether the thread's checks let the review of `tip` (a commit, or a snapshot of its
    /// files) start: not while one of them runs, nor on the files the newest one failed on.
    async fn checks_allow_review(&self, id: &ConversationId, repo: &Path, tip: &str) -> bool {
        let Some(check) = self.thread_checks.lock().unwrap().get(id).cloned() else {
            return true;
        };
        match check.state {
            CheckState::Running => false,
            CheckState::Passed => true,
            CheckState::Failed => {
                let Some(failed) = check.tree else {
                    return false;
                };
                let (git, repo, tip) = (self.git.clone(), repo.to_owned(), tip.to_owned());
                let files = blocking(move || {
                    git.open(&repo)
                        .and_then(|repo| repo.tree_of(&tip))
                        .map_err(git_error)
                })
                .await;
                files.is_ok_and(|files| files.0 != failed)
            }
        }
    }

    pub(crate) fn local_checkout(
        &self,
        id: &ConversationId,
    ) -> Option<(std::path::PathBuf, String)> {
        if self
            .overnight
            .active
            .get(id)
            .and_then(|active| active.workspace)
            .is_some()
        {
            return None;
        }
        match self.core.conversation(id).ok()?.setup? {
            Setup::Session {
                repo,
                environment: Environment::LocalCheckout { branch },
                ..
            } => Some((repo.into(), branch)),
            _ => None,
        }
    }

    /// A snapshot of the local checkout `repo`'s files as they stand.
    pub(crate) async fn snapshot_local(&self, repo: &Path) -> crate::Result<Oid> {
        let (git, repo) = (self.git.clone(), repo.to_owned());
        blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .snapshot_files()
                .map_err(git_error)
        })
        .await
    }

    /// How big `base..tip` of `repo` is.
    async fn range_size(&self, repo: &Path, base: &str, tip: &str) -> Option<Size> {
        let (git, repo, base, tip) = (
            self.git.clone(),
            repo.to_owned(),
            Oid(base.to_owned()),
            Oid(tip.to_owned()),
        );
        let stat = blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .diff_stat(&base, &tip)
                .map_err(git_error)
        })
        .await
        .ok()?;
        let code: Vec<_> = stat
            .files
            .iter()
            .filter(|file| counts_as_a_file(&file.path))
            .collect();
        Some(Size {
            lines: code.iter().fold(0u32, |sum, file| {
                sum.saturating_add(file.insertions)
                    .saturating_add(file.deletions)
            }),
            files: code.len(),
        })
    }

    /// What a review of the range `base..tip` reads: a reviewer diffs from the merge base, and a
    /// snapshot is a sibling of what comes after it (two snapshots of a checkout on its HEAD, or
    /// a worktree's snapshot and the commit that follows), so `tip` goes on `base` first (its own
    /// diff would show what the snapshot held as new again, and miss a file put back). `tip`
    /// itself when `base` is its ancestor, or when that can't be made.
    async fn on_base(&self, repo: &Path, base: &str, tip: &str) -> Oid {
        let (git, at, from, to) = (
            self.git.clone(),
            repo.to_owned(),
            Oid(base.to_owned()),
            Oid(tip.to_owned()),
        );
        match blocking(move || {
            git.open(&at)
                .map_err(git_error)?
                .on_base(&from, &to)
                .map_err(git_error)
        })
        .await
        {
            Ok(linked) => linked,
            Err(err) => {
                tracing::warn!(error = %err, "could not put a change on its base for its review");
                Oid(tip.to_owned())
            }
        }
    }

    /// Whether commits `a` and `b` of `repo` hold the same files.
    async fn same_files(&self, repo: &Path, a: &str, b: &str) -> bool {
        let (git, repo, a, b) = (
            self.git.clone(),
            repo.to_owned(),
            a.to_owned(),
            b.to_owned(),
        );
        blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            Ok(repo.tree_of(&a).map_err(git_error)? == repo.tree_of(&b).map_err(git_error)?)
        })
        .await
        .unwrap_or(false)
    }

    async fn store_unreviewed(&self, id: &ConversationId, range: Option<UnreviewedRange>) {
        if let Err(err) = self
            .core
            .record_conversation(
                id,
                vec![DomainEvent::UnreviewedChanged {
                    conversation_id: id.clone(),
                    range,
                }],
            )
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not record the thread's unreviewed changes");
        }
    }

    /// The range of `request` on `branch` as recorded, if it is that one.
    async fn recorded_range(
        &self,
        id: &ConversationId,
        request: &str,
        branch: &str,
    ) -> Option<UnreviewedRange> {
        self.core
            .board(id)
            .await
            .ok()?
            .unreviewed
            .map(|range| *range)
            .filter(|range| range.request_id == request && range.branch == branch)
    }

    /// The thread committed `base..tip` on `branch` of `repo` for its running request (not
    /// fixes of findings): they join the request's unreviewed range, which gets its review if
    /// it now crosses the line or is marked risky. Commits that don't follow the range (a
    /// landing or the user's own commits came between) start it again from `base`: what it held
    /// before stayed under the line.
    pub(crate) async fn thread_commits_unreviewed(
        &self,
        id: &ConversationId,
        repo: &Path,
        branch: &str,
        base: &Oid,
        tip: &Oid,
    ) {
        let request = self.request_for(id, None).await.unwrap_or_default();
        let known = self.recorded_range(id, &request, branch).await;
        let risky = known.as_ref().and_then(|range| range.risky.clone());
        // The thread committed what a review of its snapshot covered: nothing new.
        if let Some(range) = known.as_ref().filter(|range| range.snapshot)
            && self.same_files(repo, &range.tip, &tip.0).await
        {
            let range = UnreviewedRange {
                base: tip.0.clone(),
                tip: tip.0.clone(),
                snapshot: false,
                ..range.clone()
            };
            self.store_unreviewed(id, Some(range)).await;
            return;
        }
        let mut range = match known {
            Some(range) if !range.local && !range.snapshot && range.tip == base.0 => range,
            // The commit holds more than the snapshot a review read: only what changed since
            // that snapshot is new, never the whole range again.
            Some(range) if !range.local && range.snapshot => UnreviewedRange {
                base: range.tip.clone(),
                snapshot: false,
                ..range
            },
            _ => UnreviewedRange {
                request_id: request,
                branch: branch.to_owned(),
                base: base.0.clone(),
                tip: base.0.clone(),
                local: false,
                start: None,
                risky,
                snapshot: false,
            },
        };
        range.tip = tip.0.clone();
        self.review_range_if_due(id, repo, range).await;
    }

    /// `review_candidate` reviews up to `tip`: the thread's unreviewed range up to there is
    /// covered, and starts again after it.
    pub(crate) async fn mark_reviewed_to(&self, id: &ConversationId, tip: &Oid) {
        let Some(mut range) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.unreviewed)
            .map(|range| *range)
            .filter(|range| !range.local && range.tip == tip.0)
        else {
            return;
        };
        range.base = tip.0.clone();
        range.risky = None;
        self.store_unreviewed(id, Some(range)).await;
    }

    /// `branch` moved to `tip` past what the thread made (a landing, the user's own commits, a
    /// rewritten history) or past its fixes of findings: its range goes on from there. Fixes
    /// right after the last review stay out of it; following unreviewed work, they join it.
    pub(crate) async fn unreviewed_moved(
        &self,
        id: &ConversationId,
        branch: &str,
        from: Option<&Oid>,
        tip: &Oid,
    ) {
        let Some(mut range) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.unreviewed)
            .map(|range| *range)
            .filter(|range| range.branch == branch && !range.local && range.tip != tip.0)
        else {
            return;
        };
        let follows = from.is_some_and(|from| from.0 == range.tip);
        if !(follows && range.base != range.tip) {
            range.base = tip.0.clone();
        }
        range.tip = tip.0.clone();
        self.store_unreviewed(id, Some(range)).await;
    }

    /// Stores `range`, after starting its review when it crosses the line or is marked risky:
    /// then it starts again after what the review covers.
    async fn review_range_if_due(
        &self,
        id: &ConversationId,
        repo: &Path,
        mut range: UnreviewedRange,
    ) {
        if range.base != range.tip
            && range.risky.is_none()
            && self.fixes_findings(id, repo, &range).await
        {
            // The fix of the last review's findings (left uncommitted on the local checkout, or
            // a commit without the fix trailer): no review of its own (no review rounds), and
            // the range goes on after it.
            range.base = range.tip.clone();
        }
        if range.base != range.tip && self.reviews_on() {
            let due = range.risky.is_some()
                || self
                    .range_size(repo, &range.base, &range.tip)
                    .await
                    .is_some_and(|size| REVIEW_LINE.crossed(size.lines, size.files));
            if due && self.checks_allow_review(id, repo, &range.tip).await {
                // A range from a snapshot (the local checkout's, or a worktree's uncommitted
                // files a review read) has no commit of its base in its tip's history.
                let tip = self.on_base(repo, &range.base, &range.tip).await;
                crate::heap(|| {
                    self.review_thread_commits(
                        id,
                        Oid(range.base.clone()),
                        tip,
                        repo.to_owned(),
                        Some(range.request_id.clone()).filter(|request| !request.is_empty()),
                    )
                })
                .await;
                range.base = range.tip.clone();
                range.risky = None;
            }
        }
        self.store_unreviewed(id, Some(range)).await;
    }

    /// Whether `range` starts where the request's last review of the thread's own changes ended
    /// with findings: what changed since is their fix, as a fix commit with the trailer is.
    async fn fixes_findings(
        &self,
        id: &ConversationId,
        repo: &Path,
        range: &UnreviewedRange,
    ) -> bool {
        let Some(last) = self.core.board(id).await.ok().and_then(|board| {
            board
                .reviews
                .values()
                .filter(|review| {
                    review.kind == ReviewKind::Code
                        && review.task_id.is_none()
                        && review.request_id.as_deref() == Some(range.request_id.as_str())
                })
                .max_by_key(|review| (review.started_at_ms, review.number))
                .cloned()
        }) else {
            return false;
        };
        matches!(last.state, ReviewState::Findings { .. })
            && self.same_files(repo, &last.tip, &range.base).await
    }

    /// A turn of `request` starts on a local checkout: the request's start is recorded as a
    /// snapshot of the checkout's files, once, so "since the request started" is exact.
    pub(crate) async fn note_request_start(&self, id: &ConversationId, request: Option<&str>) {
        let (Some(request), Some((repo, branch))) = (request, self.local_checkout(id)) else {
            return;
        };
        if self.recorded_range(id, request, &branch).await.is_some() {
            return;
        }
        let start = match self.snapshot_local(&repo).await {
            Ok(start) => start,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        self.store_unreviewed(
            id,
            Some(UnreviewedRange {
                request_id: request.to_owned(),
                branch,
                base: start.0.clone(),
                tip: start.0.clone(),
                local: true,
                start: Some(start.0),
                risky: None,
                snapshot: false,
            }),
        )
        .await;
    }

    /// The thread's changes on a local checkout, uncommitted, as they stand at the end of a turn
    /// or when it calls `run_check`: the range moves to a snapshot of them, which gets its review
    /// if it crosses the line or is marked risky; and when the request changed the checkout's
    /// files, the session's full checks are due on them (they run once the request settles, on
    /// a snapshot taken then, never in the user's checkout).
    pub(crate) async fn local_changes(&self, id: &ConversationId, checks: bool) {
        let Some((repo, branch)) = self.local_checkout(id) else {
            return;
        };
        let Some(request) = self.request_for(id, None).await else {
            return;
        };
        let Some(mut range) = self.recorded_range(id, &request, &branch).await else {
            return;
        };
        let now = match self.snapshot_local(&repo).await {
            Ok(now) => now,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not take a snapshot of the local checkout");
                return;
            }
        };
        let changed = range.start.as_deref() != Some(now.0.as_str());
        if range.tip != now.0 {
            range.tip = now.0.clone();
            self.review_range_if_due(id, &repo, range).await;
        } else {
            self.review_recorded(id).await;
        }
        if checks && changed {
            self.checks_due(id, &repo, &branch, &now, Some(request))
                .await;
        }
    }

    /// The running request's range when it holds changes no review covered: its review starts
    /// now when it is due (marked risky, or over the line and held back by a check that ran or
    /// failed then, C10) and the thread's checks allow it.
    async fn review_recorded(&self, id: &ConversationId) {
        let Some(request) = self.request_for(id, None).await else {
            return;
        };
        let Some(range) = self.core.board(id).await.ok().and_then(|board| {
            board
                .unreviewed
                .map(|range| *range)
                .filter(|range| range.request_id == request && range.base != range.tip)
        }) else {
            return;
        };
        let repo = if range.local {
            self.local_checkout(id).map(|(repo, _)| repo)
        } else {
            self.recorded_workspace(id).map(|workspace| workspace.repo)
        };
        if let Some(repo) = repo {
            self.review_range_if_due(id, &repo, range).await;
        }
    }

    /// The end of a thread turn (the backstop of [`Self::review_before_answer`]): a local
    /// checkout's changes are looked at, and a range that is due gets its review, unless one of
    /// the thread's checks still runs or failed on its files.
    pub(crate) async fn unreviewed_at_turn_end(&self, id: &ConversationId) {
        if self.local_checkout(id).is_some() {
            self.local_changes(id, true).await;
        } else {
            self.review_recorded(id).await;
        }
    }

    /// `run_check`'s `risky`: the thread marks its change for `request` risky (persistence,
    /// security, migrations, concurrency, deletion, public API), so its range owes a review
    /// whatever its size.
    pub(crate) async fn mark_risky(&self, id: &ConversationId, why: &str) {
        let why = why.trim();
        if why.is_empty() {
            return;
        }
        let Some(request) = self.request_for(id, None).await else {
            return;
        };
        let local = self.local_checkout(id);
        let (repo, branch) = match &local {
            Some((repo, branch)) => (repo.clone(), branch.clone()),
            None => match self.recorded_workspace(id) {
                Some(workspace) => (workspace.repo, workspace.branch),
                None => return,
            },
        };
        let mut range = match self.recorded_range(id, &request, &branch).await {
            Some(range) => range,
            None => {
                // Where the request's changes start: the branch as last looked at, or the local
                // checkout's HEAD (its start wasn't recorded: a session older than this).
                let board = self.core.board(id).await.ok();
                let seen = board.and_then(|board| board.thread_tips.get(&branch).cloned());
                let (git, at, name, is_local) = (
                    self.git.clone(),
                    repo.clone(),
                    branch.clone(),
                    local.is_some(),
                );
                let base = match seen.filter(|_| !is_local) {
                    Some(seen) => Ok(Oid(seen)),
                    None => {
                        blocking(move || {
                            let repo = git.open(&at).map_err(git_error)?;
                            if is_local {
                                repo.resolve("HEAD").map_err(git_error)
                            } else {
                                repo.branch_tip(&name).map_err(git_error)?.ok_or_else(|| {
                                    crate::Error::Invalid(format!("branch {name} is gone"))
                                })
                            }
                        })
                        .await
                    }
                };
                let Ok(base) = base else {
                    return;
                };
                UnreviewedRange {
                    request_id: request,
                    branch,
                    base: base.0.clone(),
                    tip: base.0.clone(),
                    local: local.is_some(),
                    start: local.is_some().then(|| base.0.clone()),
                    risky: None,
                    snapshot: false,
                }
            }
        };
        range.risky = Some(why.to_owned());
        self.store_unreviewed(id, Some(range)).await;
    }

    /// Before the answer in a worktree, the changes the thread hasn't committed yet count too:
    /// it often runs its checks before it commits. When they take the request's range over the
    /// line, or the range is marked risky, the review of a snapshot of them starts now, before
    /// the answer; the commit that follows with the same files needs none of its own.
    async fn review_uncommitted(&self, id: &ConversationId) {
        if !self.reviews_on() || self.local_checkout(id).is_some() {
            return;
        }
        let (Some(request), Some(workspace)) = (
            self.request_for(id, None).await,
            self.recorded_workspace(id),
        ) else {
            return;
        };
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        if running_review(&board, &request).is_some() {
            return;
        }
        let (git, path) = (self.git.clone(), workspace.path.clone());
        let Ok((head, snapshot)) = blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            Ok((
                repo.resolve("HEAD").map_err(git_error)?,
                repo.snapshot_files().map_err(git_error)?,
            ))
        })
        .await
        else {
            return;
        };
        if snapshot == head {
            return;
        }
        let known = self.recorded_range(id, &request, &workspace.branch).await;
        // The same files on the same commit make the same snapshot: reviewed already.
        if known
            .as_ref()
            .is_some_and(|range| range.snapshot && range.tip == snapshot.0)
        {
            return;
        }
        let risky = known.as_ref().and_then(|range| range.risky.clone());
        let base = match known {
            Some(range) if !range.snapshot && range.tip == head.0 => range.base,
            _ => head.0,
        };
        let due = risky.is_some()
            || self
                .range_size(&workspace.repo, &base, &snapshot.0)
                .await
                .is_some_and(|size| REVIEW_LINE.crossed(size.lines, size.files));
        if !due
            || !self
                .checks_allow_review(id, &workspace.repo, &snapshot.0)
                .await
        {
            return;
        }
        crate::heap(|| {
            self.review_thread_commits(
                id,
                Oid(base.clone()),
                snapshot.clone(),
                workspace.repo.clone(),
                Some(request.clone()),
            )
        })
        .await;
        self.store_unreviewed(
            id,
            Some(UnreviewedRange {
                request_id: request,
                branch: workspace.branch,
                base: snapshot.0.clone(),
                tip: snapshot.0,
                local: false,
                start: None,
                risky: None,
                snapshot: true,
            }),
        )
        .await;
    }

    /// Before the thread answers (`run_check`): its range is brought up to date, and reviewed
    /// when it crosses the line or is marked risky. While a review of its work for the running
    /// request runs, the line its reply ends with: wait for the findings. None with reviews
    /// off, and for work under the line. With `hold`, a running review is waited for until
    /// then, unless the call is cancelled, and one that ends by then is said in the reply
    /// instead (#193).
    pub(crate) async fn review_before_answer(
        &self,
        id: &ConversationId,
        hold: Option<(tokio::time::Instant, &tokio_util::sync::CancellationToken)>,
    ) -> Option<String> {
        if !self.reviews_on() || self.recorded_workspace(id).is_none() {
            return None;
        }
        if self.local_checkout(id).is_some() {
            self.local_changes(id, false).await;
        } else {
            self.scan_thread_commits(id, true).await;
            self.review_recorded(id).await;
            self.review_uncommitted(id).await;
        }
        let request = self.request_for(id, None).await?;
        let board = self.core.board(id).await.ok()?;
        let review = running_review(&board, &request)?;
        // One that ended since is told as any review is: its findings come as a message, still
        // to wait for, and a clean one leaves nothing to wait for.
        if !self.await_review(id, &review.id).await
            && !self.core.board(id).await.is_ok_and(|board| {
                board
                    .reviews
                    .get(&review.id)
                    .is_some_and(|ended| matches!(ended.state, ReviewState::Findings { .. }))
            })
        {
            return None;
        }
        if let Some(hold) = hold
            && let Some(ended) = self.review_in_reply(id, review, hold).await
        {
            return ended;
        }
        Some(format!(
            "[review running] {} reviews your change for this request ({}..{}) in the background: it is over the review line, or you marked it risky. Don't give your final answer yet: end this turn with exactly {} and nothing else. Its findings come as a [review of your commits …] message (or word that it found nothing); fix the valid ones, then answer.",
            review.reviewer.label(),
            short(&review.base),
            short(&review.tip),
            super::prompts::QUIET,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_line_is_150_lines_or_more_than_five_files_of_code() {
        assert!(!REVIEW_LINE.crossed(10, 1));
        assert!(!REVIEW_LINE.crossed(149, 5));
        assert!(REVIEW_LINE.crossed(150, 1));
        assert!(REVIEW_LINE.crossed(6, 6));
        assert!(!REVIEW_LINE.crossed(40, 3));
    }

    #[test]
    fn tests_and_docs_dont_count_toward_the_files() {
        for path in [
            "src/tax.ts",
            "crates/core/src/lib.rs",
            "README",
            "testing.rs",
        ] {
            assert!(counts_as_a_file(path), "{path}");
        }
        for path in [
            "src/tax.test.ts",
            "src/App.test.tsx",
            "crates/core/src/manager/flow/review_tiers_tests.rs",
            "crates/git/tests/repo.rs",
            "src/__tests__/a.js",
            "test/fixture.json",
            "docs/rates.html",
            "rates.md",
            "docs/research/PLAN.MD",
        ] {
            assert!(!counts_as_a_file(path), "{path}");
        }
    }
}
