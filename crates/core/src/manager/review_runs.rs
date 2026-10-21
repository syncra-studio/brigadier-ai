//! One-shot reviews (THREAD-PLAN.md §2 Q12): every landing gets one read-only review by the
//! vendor other than its author's; a worker may ask for one of its own work (`request_review`);
//! a lead's outline gets one in the background (a plan review). Nothing waits for one: the
//! findings reach whoever asked as a message, also after the session was merged.
//!
//! Each review is a [`ReviewRun`] on the conversation's stream, run in a detached checkout of
//! its own at the reviewed commit. That checkout belongs to the review (`review:<id>` in the
//! cleanup ledger), so landing a task, merging the session or starting its next branch never
//! removes it under the reviewer. A range is reviewed once: a landing whose range a worker's
//! review already covered starts no second one.
//!
//! Codex reviews run as `codex exec review` ([`brigadier_review::run_codex`]); Claude reviews
//! run through the adapter, read-only, with only the tools that read a change. Their tokens go
//! to the routing store under the task `review:<id>`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use brigadier_git::{Oid, WorktreeSpec};
use brigadier_providers::{
    Access, AllowedModels, ApprovalDecision, Artifact, Origin, ProviderEvent, ProviderKind, Role,
    SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_review::{CodexRun, Review, Subject};
use brigadier_router::{Author, Decision, Needs, Pin, QualityTier, TaskCategory};

use super::conversation::Envelope;
use super::prompts::FIXES_TRAILER;
use super::usage::TokenOwner;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent};
use crate::routing::TokenMeter;
use crate::work::{
    Finding, FindingStatus, InjectionKind, OrchestratorStepKind, ReviewFor, ReviewKind, ReviewRun,
    ReviewState, Task, TaskId, TaskKind, TaskState, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// The label of the envelope that tells the thread how `review` ended, `of` its task
/// (`task-3`), as its injection is logged once a turn takes it. Findings and a review that
/// couldn't run carry the review's number, so a restart tells which the thread took.
fn report_label(review: &ReviewRun, of: &str) -> String {
    let label = report_base_label(review, of);
    if review.number > 0 && review.state != ReviewState::Clean {
        format!("{label} · R{}", review.number)
    } else {
        label
    }
}

/// [`report_label`] without the review's number, as builds before it logged it.
fn report_base_label(review: &ReviewRun, of: &str) -> String {
    match (review.kind, review.task_id.is_some()) {
        (ReviewKind::Code, true) => format!("review {of}"),
        (ReviewKind::Plan, true) => format!("plan review {of}"),
        (ReviewKind::Code, false) if review.state == ReviewState::Clean => {
            super::conversation::CLEAN_REVIEW.to_owned()
        }
        (ReviewKind::Code, false) => "review of your commits".to_owned(),
        (ReviewKind::Plan, false) => "plan review of your plan".to_owned(),
    }
}

/// A one-shot review's time box.
const REVIEW_TIME: Duration = Duration::from_secs(30 * 60);
/// The most of a review's text a message carries; all of it stays in the review's blob.
const MESSAGE_MAX: usize = 12_000;
/// What a worker waiting for its own review's findings is blocked on.
pub(crate) const WAITING_FOR_REVIEW: &str = "Waiting for its code review";
/// `request_review`'s answer while the user has reviews off.
const REVIEWS_OFF_CODE: &str = "Reviews are off (the user's setting): no review runs, and nothing waits for one. Check your work yourself; once your checks pass and your evidence is in, call submit_report.";
/// `review_plan`'s answer while the user has reviews off.
const REVIEWS_OFF_PLAN: &str = "Reviews are off (the user's setting): no plan review runs. Judge the plan yourself and carry on.";

/// A review to start.
struct NewReview {
    conversation_id: ConversationId,
    request_id: Option<String>,
    task_id: Option<TaskId>,
    kind: ReviewKind,
    base: Oid,
    tip: Oid,
    /// Who wrote the work: the review goes to the other vendor, never this model.
    author: Author,
    notify: ReviewFor,
    /// The repository the reviewed commit is in.
    repo: PathBuf,
    /// A plan review's brief and outline.
    plan: Option<(String, String)>,
    /// A re-check's findings: the commits fix them (SESSION-FIXES-PLAN.md Q6.1).
    recheck: Option<String>,
}

/// What of a landing a worker's own review read that it didn't see the end of
/// (`SessionManager::pass_own_review`).
enum Passed {
    /// All of it: it is the landing's review.
    Whole,
    /// Up to this commit: what came after is reviewed alone.
    Upto(Oid),
    No,
}

/// How much of a landing its worker's own review covers.
enum Covers {
    /// All of it: no review of its own.
    All,
    /// Up to this commit, reviewed clean: what came after is reviewed alone.
    Upto(Oid),
    Nothing,
}

/// The review findings the thread's own commits fix (`SessionManager::thread_fixes`).
pub(crate) struct ThreadFix {
    pub fixed: Vec<Finding>,
    /// All of them are findings of the work since the session's last merge: a held merge's
    /// consent covers the fix.
    pub since_merge: bool,
}

/// A review whose end isn't told yet, counted on its conversation until dropped.
struct Telling(std::sync::Arc<super::conversation::ConvLive>);

impl Telling {
    fn new(conv: std::sync::Arc<super::conversation::ConvLive>) -> Self {
        conv.reviews_telling
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Self(conv)
    }
}

impl Drop for Telling {
    fn drop(&mut self) {
        self.0
            .reviews_telling
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The cleanup-ledger owner of a review's checkout and processes; also the task its tokens
/// are counted under.
fn review_owner(id: &str) -> String {
    format!("review:{id}")
}

/// The thread's "Reviewed {worker}'s change" row for a code review that ended with a verdict;
/// none for a plan review, one that could not run, or one of no task's work.
fn reviewed_step(review: &ReviewRun) -> Option<OrchestratorStepKind> {
    if review.kind != ReviewKind::Code {
        return None;
    }
    let findings = match review.state {
        ReviewState::Clean => 0,
        ReviewState::Findings { count } => count,
        ReviewState::Running | ReviewState::Failed { .. } => return None,
    };
    Some(OrchestratorStepKind::Reviewed {
        task_ids: vec![review.task_id.clone()?],
        findings,
    })
}

/// The vendor that reviews `author`'s work.
fn other_vendor(author: ProviderKind) -> ProviderKind {
    match author {
        ProviderKind::Claude => ProviderKind::Codex,
        ProviderKind::Codex => ProviderKind::Claude,
    }
}

/// The code review of `base..tip` among `reviews`, if that range has one. One that could not
/// run doesn't count: the range is reviewed again.
fn review_of<'a>(
    reviews: impl IntoIterator<Item = &'a ReviewRun>,
    base: &str,
    tip: &str,
) -> Option<&'a ReviewRun> {
    reviews.into_iter().find(|review| {
        review.kind == ReviewKind::Code
            && review.base == base
            && review.tip == tip
            && !matches!(review.state, ReviewState::Failed { .. })
    })
}

/// The landed tip whose batch `review` belongs to: the landing it is part of, else its own tip.
pub(crate) fn batch_tip(review: &ReviewRun) -> &str {
    review.landing.as_deref().unwrap_or(&review.tip)
}

/// Whether the thread still owes `review`'s findings a decision: none recorded, or one kept
/// open for a fix to come.
fn findings_unsettled(review: &ReviewRun) -> bool {
    review
        .items
        .iter()
        .any(|item| matches!(item.status, FindingStatus::New | FindingStatus::Open { .. }))
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

/// At most [`MESSAGE_MAX`] bytes of `text`, cut at a character boundary.
fn clipped(text: &str) -> String {
    if text.len() <= MESSAGE_MAX {
        return text.to_owned();
    }
    let mut end = MESSAGE_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[… the rest of the review is cut]", &text[..end])
}

/// The number the next review of a conversation gets: one more than the highest so far.
fn next_number<'a>(reviews: impl IntoIterator<Item = &'a ReviewRun>) -> u32 {
    reviews
        .into_iter()
        .map(|review| review.number)
        .max()
        .unwrap_or(0)
        .saturating_add(1)
}

/// The findings of review `number`'s `text`, as items (`R{number}.{n}`), each location in the
/// project's own terms: a path in the review's `checkout` relative to it, a one-line range one
/// line. A text that names a priority but can't be split is one item that points at the review.
pub(crate) fn items_of(number: u32, text: &str, checkout: Option<&Path>) -> Vec<Finding> {
    let mut items: Vec<Finding> = brigadier_review::split_findings(text)
        .into_iter()
        .enumerate()
        .map(|(n, found)| Finding {
            id: format!("R{number}.{}", n + 1),
            severity: Some(found.severity),
            title: found.title,
            location: found
                .location
                .map(|location| project_location(&location, checkout)),
            status: FindingStatus::New,
            high: false,
        })
        .collect();
    if items.is_empty() && text.contains("[P") && !text.trim().eq_ignore_ascii_case("no findings.")
    {
        items.push(Finding {
            id: format!("R{number}.1"),
            severity: None,
            title: "See the review".into(),
            location: None,
            status: FindingStatus::New,
            high: false,
        });
    }
    items
}

/// The name of review `review`'s checkout and scratch folders.
fn review_dir_name(review: &ReviewRun) -> String {
    format!(
        "review-{}",
        &review.id[review.id.len().saturating_sub(12)..]
    )
}

/// `location` (`path:line`) with the review's `checkout` taken off its path, and "19-19" as "19".
fn project_location(location: &str, checkout: Option<&Path>) -> String {
    let mut location = location;
    if let Some(root) = checkout.and_then(Path::to_str)
        && let Some(inside) = location
            .strip_prefix(root.trim_end_matches('/'))
            .and_then(|rest| rest.strip_prefix('/'))
    {
        location = inside;
    }
    match location.rsplit_once(':') {
        Some((path, lines))
            if lines
                .split_once('-')
                .is_some_and(|(from, to)| from == to && !from.is_empty()) =>
        {
            format!(
                "{path}:{}",
                lines.split_once('-').map_or(lines, |(from, _)| from)
            )
        }
        _ => location.to_owned(),
    }
}

/// The task of `board` named `task-7`, `Task 7` or `7`.
fn named_task<'a>(named: &str, board: &'a crate::board::Board) -> Option<&'a crate::work::Task> {
    let digits = named
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .trim_end_matches(|c: char| !c.is_ascii_digit());
    let number: u32 = digits.parse().ok()?;
    board.tasks.values().find(|task| task.number == number)
}

/// A finding's status in a word or two, as the thread's replies and the merge's result say it.
pub(crate) fn status_word(status: &FindingStatus) -> String {
    match status {
        FindingStatus::New => "not recorded".into(),
        FindingStatus::Fixed { task, checked, .. } => {
            let fixed = match task {
                Some(task) => format!("fixed in {task}"),
                None => "fixed".into(),
            };
            if checked.is_some() {
                fixed
            } else {
                format!("{fixed} (unverified: not checked in the merged code)")
            }
        }
        FindingStatus::Open { .. } => "open".into(),
        FindingStatus::Dismissed { why } => format!("dismissed: {why}"),
    }
}

/// What `record` makes of finding `id`, or why it can't be recorded: a fix credited to a task
/// that hasn't landed fixed nothing in the merged code yet, and a dismissal says why. A fix
/// with no `checked` is stored as is: it shows as unverified.
fn recorded_status(
    id: &str,
    record: &crate::tools::FindingRecord,
    board: &crate::board::Board,
) -> std::result::Result<FindingStatus, String> {
    use crate::tools::FindingVerdict;
    let said = |text: &Option<String>| {
        text.as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    let why = said(&record.why);
    match record.status {
        FindingVerdict::Fixed => {
            let task = match record.task.as_deref().map(str::trim) {
                None | Some("") => None,
                Some(named) => match named_task(named, board) {
                    Some(task) if task.landed.is_none() => {
                        return Err(format!(
                            "{id}: task-{} hasn't landed, so it fixed nothing in the merged code yet; record it open until it lands",
                            task.number
                        ));
                    }
                    Some(task) => Some(format!("task-{}", task.number)),
                    None => return Err(format!("{id}: {named} is no task of this session")),
                },
            };
            Ok(FindingStatus::Fixed {
                task,
                why,
                checked: said(&record.checked),
            })
        }
        FindingVerdict::Open => Ok(FindingStatus::Open { why }),
        FindingVerdict::Dismissed => match why {
            Some(why) => Ok(FindingStatus::Dismissed { why }),
            None => Err(format!("{id}: say why you dismiss it")),
        },
    }
}

/// A finding recorded fixed without a check of the fix in the merged code.
pub(crate) fn unverified(status: &FindingStatus) -> bool {
    matches!(status, FindingStatus::Fixed { checked: None, .. })
}

/// What a code review's message to the thread ends with: its findings by id, and how to
/// record them.
fn record_line(items: &[Finding]) -> String {
    format!(
        "Its findings:\n{}\nA finding that asks for something the request didn't, or contradicts it, is no bug: dismiss it with that reason (mention it to the user if it matters). Record what became of each that no run_check `fixes` records with record_findings, all in one call: fixed (in task-N, or by you, with how you checked it in the merged code), open, or dismissed and why. If you record any with record_findings and it isn't in your tool list, look it up in the same message as your first fix or check, not in a call of its own. If your answer is already written: {}",
        listed(items),
        super::prompts::LATE_EVENT
    )
}

/// One line per finding, with its id, for the thread to record each by.
/// A session and a reviewed range, as the calls that wait on its review claim it.
type ReplyKey = (ConversationId, String, String);

/// The claims of the calls that wait on a review's outcome ([`SessionManager::review_replies`]).
#[derive(Default)]
pub(super) struct ReplyClaims {
    /// By session and range: how many calls wait, and whether one said the outcome.
    waiting: HashMap<ReplyKey, (usize, bool)>,
    /// Ranges whose review ended while calls waited, its message skipped for them, and none
    /// said it yet: how many of them have yet to finish. The last one cancelled tells it
    /// (#332).
    stood_aside: HashMap<ReplyKey, usize>,
}

impl ReplyClaims {
    /// How many ranges calls wait on now.
    #[cfg(all(test, unix))]
    pub(super) fn waiting(&self) -> usize {
        self.waiting.len()
    }

    /// One call's claim on `key` given back; whether it was still there.
    fn give_back(&mut self, key: &ReplyKey) -> bool {
        let Some((waiting, said)) = self.waiting.get_mut(key) else {
            return false;
        };
        *waiting -= 1;
        if *waiting == 0 && !*said {
            self.waiting.remove(key);
        }
        true
    }

    /// The review of `key` ended: whether calls waited on it, its message then skipped.
    fn stand_aside(&mut self, key: &ReplyKey) -> bool {
        let Some((waiting, said)) = self.waiting.remove(key) else {
            return false;
        };
        if !said && waiting > 0 {
            self.stood_aside.insert(key.clone(), waiting);
        }
        true
    }
}

/// A request_review call's claim on a review's outcome ([`SessionManager::review_replies`]):
/// dropped while still armed (the call ends without saying it, or is cancelled), the review's
/// message is delivered as usual again; one whose end already stood aside for the call is
/// told now, by the last such call, unless another said it (#332).
struct ReplyWait<'a> {
    manager: &'a SessionManager,
    key: ReplyKey,
    armed: bool,
    /// The review it waits on, once its call waits for it.
    review: Option<String>,
}

impl<'a> ReplyWait<'a> {
    fn new(manager: &'a SessionManager, key: ReplyKey) -> Self {
        manager
            .reply_claims()
            .waiting
            .entry(key.clone())
            .or_default()
            .0 += 1;
        Self {
            manager,
            key,
            armed: true,
            review: None,
        }
    }

    /// Waits on `review`: a call cancelled after its end stood aside has it told as a message.
    fn on(mut self, review: &str) -> Self {
        self.review = Some(review.to_owned());
        self
    }

    /// The reply says the outcome: its message stays undelivered.
    fn said(mut self) {
        self.armed = false;
        let mut claims = self.manager.reply_claims();
        match claims.waiting.get_mut(&self.key) {
            Some((_, said)) => *said = true,
            None => {
                claims.stood_aside.remove(&self.key);
            }
        }
    }

    /// Whether the review's end has yet to stand aside for the calls that wait on it.
    fn standing(&self) -> bool {
        self.manager.reply_claims().waiting.contains_key(&self.key)
    }

    /// Gives the claim back; whether it was still there: false when the review's end already
    /// stood aside for this call, whose reply then says it. Another call's claim on the same
    /// review stays.
    fn release(mut self) -> bool {
        self.armed = false;
        let mut claims = self.manager.reply_claims();
        if claims.give_back(&self.key) {
            return true;
        }
        claims.stood_aside.remove(&self.key);
        false
    }
}

impl Drop for ReplyWait<'_> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let tell = {
            let mut claims = self.manager.reply_claims();
            if claims.give_back(&self.key) {
                false
            } else if self.review.is_none() {
                // Not waiting on it yet: the call's reply says the review it started.
                claims.stood_aside.remove(&self.key);
                false
            } else if let Some(left) = claims.stood_aside.get_mut(&self.key) {
                *left -= 1;
                *left == 0 && claims.stood_aside.remove(&self.key).is_some()
            } else {
                false
            }
        };
        // Cancelled just as the review ended, its message skipped for this call: the thread
        // hears it as a message after all (#332), as run_check's does (#193).
        if tell
            && let Some(review) = self.review.take()
            && let Some(manager) = self.manager.me.upgrade()
        {
            let conversation = self.key.0.clone();
            self.manager.spawn(async move {
                manager.tell_unsaid_review(&conversation, &review).await;
            });
        }
    }
}

impl SessionManager {
    fn reply_claims(&self) -> std::sync::MutexGuard<'_, ReplyClaims> {
        self.review_replies
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }
}

/// Where `start..commit` is reviewed up to: following a chain of reviews from `start`, each
/// starting where the last ended (or before it) and ending on the way to `commit`; with the
/// last of them.
fn covered_upto(
    repo: &brigadier_git::Repo,
    reviews: &[ReviewRun],
    start: &Oid,
    commit: &Oid,
) -> (Oid, Option<ReviewRun>) {
    // A review of a snapshot with the candidate's very files (an uncommitted change it read
    // before the thread committed it) covers the candidate, when what comes before its base
    // is covered too: a landing's review of only its own commits leaves the thread's earlier
    // ones out.
    let tree = repo.tree_of(&commit.0).ok();
    if let Some(same) = reviews
        .iter()
        .filter(|review| tree.is_some() && repo.tree_of(&review.tip).ok() == tree)
        .filter(|review| {
            let base = Oid(review.base.clone());
            base == *start
                || repo.ancestor(&base, start).unwrap_or(false)
                || (repo.ancestor(start, &base).unwrap_or(false)
                    && chain_upto(repo, reviews, start, &base).0 == base)
        })
        .max_by_key(|review| (review.started_at_ms, review.number))
    {
        return (commit.clone(), Some(same.clone()));
    }
    chain_upto(repo, reviews, start, commit)
}

/// [`covered_upto`] along commits only: the chain of reviews from `start` towards `commit`.
fn chain_upto(
    repo: &brigadier_git::Repo,
    reviews: &[ReviewRun],
    start: &Oid,
    commit: &Oid,
) -> (Oid, Option<ReviewRun>) {
    let mut from = start.clone();
    let mut last: Option<ReviewRun> = None;
    loop {
        let on_the_way = |tip: &str| {
            let tip = Oid(tip.to_owned());
            tip != from
                && repo.ancestor(&from, &tip).unwrap_or(false)
                && repo.ancestor(&tip, commit).unwrap_or(false)
        };
        let next = reviews
            .iter()
            .filter(|review| {
                let base = Oid(review.base.clone());
                (base == from || repo.ancestor(&base, &from).unwrap_or(false))
                    && on_the_way(&review.tip)
            })
            .max_by_key(|review| (review.started_at_ms, review.number));
        match next {
            Some(review) => {
                from = Oid(review.tip.clone());
                last = Some(review.clone());
            }
            None => return (from, last),
        }
    }
}

fn listed(items: &[Finding]) -> String {
    items
        .iter()
        .map(|item| {
            let severity = item
                .severity
                .as_deref()
                .map_or_else(String::new, |s| format!("[{s}] "));
            let location = item
                .location
                .as_deref()
                .map_or_else(String::new, |l| format!(" — {l}"));
            format!("{}: {severity}{}{location}", item.id, item.title)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl SessionManager {
    /// The review of what a landing put on its target, `from..tip`, by the vendor other than
    /// the phase's author. None when that range has one already.
    pub(crate) async fn review_landing(&self, task: &Task, from: Oid, tip: Oid) {
        let repo = match self.task_repo(task) {
            Ok(repo) => repo,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "a landing could not be reviewed");
                return;
            }
        };
        let landed = tip.0.clone();
        // Its worker's own review that hadn't reached it when it reported belongs to this
        // landing: the thread gets its findings, and only the commits after it are reviewed.
        let from = match self.pass_own_review(task, &repo, &from, &tip).await {
            Passed::Whole => return,
            Passed::Upto(reviewed) => reviewed,
            Passed::No => from,
        };
        // Its worker's own review is the change's one review (SESSION-FIXES-PLAN.md Q6.1).
        let from = match self.own_review_covers(task, &repo, &from, &tip).await {
            Covers::All => {
                self.settle_review_batch(&task.conversation_id, &landed)
                    .await;
                return;
            }
            Covers::Upto(reviewed) => reviewed,
            Covers::Nothing => from,
        };
        let author = self.phase_author(task).await;
        let started = self
            .start_review(NewReview {
                conversation_id: task.conversation_id.clone(),
                request_id: task.request_id.clone(),
                task_id: Some(task.id.clone()),
                kind: ReviewKind::Code,
                base: from,
                tip,
                author: Author {
                    provider: author.route.choice.provider,
                    model: author.route.choice.model.clone(),
                },
                notify: ReviewFor::Orchestrator,
                repo,
                plan: None,
                recheck: None,
            })
            .await;
        match started {
            Ok(Some((review, true))) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
                self.settle_review_batch(&task.conversation_id, &review.tip)
                    .await;
            }
            // The range was reviewed before it landed, for its worker, who weighed the
            // findings: none come back to the thread now.
            Ok(Some((review, false))) if review.state != ReviewState::Running => {
                self.settle_review_batch(&task.conversation_id, &review.tip)
                    .await;
            }
            // Reviews are off: no findings will come back for a fix.
            Ok(None) => {
                self.settle_review_batch(&task.conversation_id, &landed)
                    .await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "a landing could not be reviewed");
                self.settle_review_batch(&task.conversation_id, &landed)
                    .await;
            }
        }
    }

    /// The review of `task`'s own committed work, from `from`, that hadn't reached its worker
    /// when it reported (still running, or its outcome went to the thread), made part of the
    /// landing at `tip`: its findings are the thread's. `Whole` when it read all of the
    /// landing, `Upto` the commit it read up to, after which the rest gets a review of its
    /// own; `No` when there is none, or its commits were rewritten.
    async fn pass_own_review(&self, task: &Task, repo: &Path, from: &Oid, tip: &Oid) -> Passed {
        let own = ReviewFor::Worker {
            task_id: task.id.clone(),
        };
        let held = self.reviews.lock().await;
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return Passed::No;
        };
        let Some(mut review) = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code
                    && review.task_id.as_ref() == Some(&task.id)
                    && review.base == from.0
                    && review.landing.is_none()
                    && match &review.state {
                        ReviewState::Running => review.notify == own,
                        ReviewState::Failed { .. } => false,
                        // Passed to the thread when it ended after the worker's report.
                        _ => review.notify == ReviewFor::Orchestrator,
                    }
            })
            .max_by_key(|review| review.number)
            .cloned()
        else {
            return Passed::No;
        };
        let reviewed = Oid(review.tip.clone());
        if reviewed != *tip {
            let (git, repo, at, landed) = (
                self.git.clone(),
                repo.to_owned(),
                reviewed.clone(),
                tip.clone(),
            );
            let after = blocking(move || {
                git.open(&repo)
                    .map_err(git_error)?
                    .ancestor(&at, &landed)
                    .map_err(git_error)
            })
            .await;
            if !matches!(after, Ok(true)) {
                return Passed::No;
            }
            review.landing = Some(tip.0.clone());
        }
        review.notify = ReviewFor::Orchestrator;
        if let Err(err) = self.store_review(&review).await {
            tracing::warn!(review = %review.id, error = %err, "a worker's own review could not pass to its landing");
            return Passed::No;
        }
        drop(held);
        // One that read all of the landing and ended already, with nothing left to decide, has
        // nothing to come back for. One that read part of it waits for the review of the rest,
        // which settles the batch when it ends.
        if reviewed == *tip && review.state != ReviewState::Running && !findings_unsettled(&review)
        {
            self.settle_review_batch(&task.conversation_id, &tip.0)
                .await;
        }
        if reviewed == *tip {
            Passed::Whole
        } else {
            Passed::Upto(reviewed)
        }
    }

    /// How much of `task`'s landing, `from..tip`, its worker's own review (request_review)
    /// covers: the newest that ended with a verdict, from the same start. Its range landed as
    /// it was reviewed, or with commits after it: after findings those are the worker's fixes,
    /// which get no review of their own; after a clean review they are work nobody read yet,
    /// reviewed alone. A review that couldn't run, still runs, or whose commits were rewritten
    /// covers nothing.
    async fn own_review_covers(&self, task: &Task, repo: &Path, from: &Oid, tip: &Oid) -> Covers {
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return Covers::Nothing;
        };
        let own = ReviewFor::Worker {
            task_id: task.id.clone(),
        };
        let Some(review) = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code
                    && review.notify == own
                    && review.base == from.0
                    && matches!(
                        review.state,
                        ReviewState::Clean | ReviewState::Findings { .. }
                    )
            })
            .max_by_key(|review| review.number)
        else {
            return Covers::Nothing;
        };
        let reviewed = Oid(review.tip.clone());
        if reviewed == *tip {
            return Covers::All;
        }
        let (git, repo, at, landed) = (
            self.git.clone(),
            repo.to_owned(),
            reviewed.clone(),
            tip.clone(),
        );
        let after = blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .ancestor(&at, &landed)
                .map_err(git_error)
        })
        .await;
        match (after, &review.state) {
            (Ok(true), ReviewState::Findings { .. }) => Covers::All,
            (Ok(true), _) => Covers::Upto(reviewed),
            _ => Covers::Nothing,
        }
    }

    /// The review findings the thread's commits `base..tip` fix, when each of them names some
    /// with the [`FIXES_TRAILER`] and all it names are findings of the session's work: then
    /// they get no review of their own, only the re-check of those it recorded high. A held
    /// merge's consent covers them when those findings are all of the work since its last
    /// merge (`since_merge`). `None` otherwise.
    pub(crate) async fn thread_fixes(
        &self,
        conversation_id: &ConversationId,
        repo: &Path,
        base: &Oid,
        tip: &Oid,
    ) -> Option<ThreadFix> {
        let (git, repo, from, to) = (self.git.clone(), repo.to_owned(), base.clone(), tip.clone());
        let named = blocking(move || {
            git.open(&repo)
                .map_err(git_error)?
                .trailer_values(&from, &to, FIXES_TRAILER)
                .map_err(git_error)
        })
        .await
        .ok()?;
        let mut ids: Vec<String> = Vec::new();
        for commit in &named {
            let before = ids.len();
            ids.extend(
                commit
                    .iter()
                    .flat_map(|value| value.split([',', ';', ' ']))
                    .map(|id| id.trim().to_ascii_uppercase())
                    .filter(|id| !id.is_empty()),
            );
            if ids.len() == before {
                return None;
            }
        }
        if ids.is_empty() {
            return None;
        }
        let board = self.core.board(conversation_id).await.ok()?;
        // Named findings are looked up across the whole conversation: a review that ran when
        // the user merged reports after the merge, and its fix is still only a fix. Whether a
        // held merge's consent covers the fix is the merge batch's question, asked apart.
        let findings: Vec<&Finding> = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code && review.notify == ReviewFor::Orchestrator
            })
            .flat_map(|review| &review.items)
            .collect();
        let batch: HashSet<&str> = super::landing::reviews_since_merge(&board)
            .into_iter()
            .flat_map(|review| &review.items)
            .map(|item| item.id.as_str())
            .collect();
        let mut fixed: Vec<Finding> = Vec::new();
        let mut since_merge = true;
        for id in ids {
            let found = findings.iter().find(|item| item.id == id)?;
            since_merge &= batch.contains(id.as_str());
            if !fixed.iter().any(|item| item.id == id) {
                fixed.push((*found).clone());
            }
        }
        Some(ThreadFix { fixed, since_merge })
    }

    /// The one targeted re-check of the thread's commits `base..tip` that fix `fixed`: of those
    /// it recorded high and didn't dismiss, none when there are none (SESSION-FIXES-PLAN.md
    /// Q6.1, as for a fix task's landing).
    pub(crate) async fn recheck_thread_fix(
        &self,
        conversation_id: &ConversationId,
        base: Oid,
        tip: Oid,
        repo: PathBuf,
        fixed: &[Finding],
    ) {
        let high: Vec<Finding> = fixed
            .iter()
            .filter(|item| item.high && !matches!(item.status, FindingStatus::Dismissed { .. }))
            .cloned()
            .collect();
        if high.is_empty() {
            return;
        }
        let author = self.thread_author(conversation_id).await;
        let started = self
            .start_review(NewReview {
                conversation_id: conversation_id.clone(),
                request_id: self.request_for(conversation_id, None).await,
                task_id: None,
                kind: ReviewKind::Code,
                base,
                tip,
                author,
                notify: ReviewFor::Orchestrator,
                repo,
                plan: None,
                recheck: Some(listed(&high)),
            })
            .await;
        match started {
            Ok(Some((review, true))) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(conversation = %conversation_id, error = %err, "the thread's fix could not be re-checked");
            }
        }
    }

    /// The one targeted re-check of `fix`'s landing, `from..tip`, when the thread marked a
    /// finding of the review of `subject`'s landing high: whether those findings are fixed,
    /// and what the fix broke. None otherwise (SESSION-FIXES-PLAN.md Q6.1).
    pub(crate) async fn recheck_fix(&self, fix: &Task, subject: &Task, from: Oid, tip: Oid) {
        let Ok(board) = self.core.board(&fix.conversation_id).await else {
            return;
        };
        let high: Vec<Finding> = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code
                    && subject.landed.as_deref() == Some(batch_tip(review))
            })
            .flat_map(|review| &review.items)
            .filter(|item| item.high && !matches!(item.status, FindingStatus::Dismissed { .. }))
            .cloned()
            .collect();
        if high.is_empty() {
            return;
        }
        let repo = match self.task_repo(fix) {
            Ok(repo) => repo,
            Err(err) => {
                tracing::warn!(task = %fix.id, error = %err, "a fix could not be re-checked");
                return;
            }
        };
        let author = self.phase_author(fix).await;
        let started = self
            .start_review(NewReview {
                conversation_id: fix.conversation_id.clone(),
                request_id: fix.request_id.clone(),
                task_id: Some(fix.id.clone()),
                kind: ReviewKind::Code,
                base: from,
                tip,
                author: Author {
                    provider: author.route.choice.provider,
                    model: author.route.choice.model.clone(),
                },
                notify: ReviewFor::Orchestrator,
                repo,
                plan: None,
                recheck: Some(listed(&high)),
            })
            .await;
        match started {
            Ok(Some((review, true))) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %fix.id, error = %err, "a fix could not be re-checked");
            }
        }
    }

    /// The review of changes the thread made itself for `request`, `base..tip`, by the vendor
    /// other than the thread's. None when that range has one already.
    pub(crate) async fn review_thread_commits(
        &self,
        conversation_id: &ConversationId,
        base: Oid,
        tip: Oid,
        repo: PathBuf,
        request: Option<String>,
    ) {
        let author = self.thread_author(conversation_id).await;
        let request = match request {
            Some(request) => Some(request),
            None => self.request_for(conversation_id, None).await,
        };
        let started = self
            .start_review(NewReview {
                conversation_id: conversation_id.clone(),
                request_id: request,
                task_id: None,
                kind: ReviewKind::Code,
                base,
                tip,
                author,
                notify: ReviewFor::Orchestrator,
                repo,
                plan: None,
                recheck: None,
            })
            .await;
        match started {
            Ok(Some((review, true))) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(conversation = %conversation_id, error = %err, "the thread's commits could not be reviewed");
            }
        }
    }

    /// `request_review`: the other vendor's review of the thread's candidate commit, small
    /// changes too. What reviews already cover is reused: the range starts where a chain of
    /// earlier reviews from the merge base ends, and after findings it re-checks their fix
    /// alone (#138). It waits up to `wait_seconds` for the findings and returns them in its
    /// reply instead of a message (#137). No reviewer, or reviews off, is a fact it states.
    pub(crate) async fn request_review(
        &self,
        id: &ConversationId,
        args: crate::tools::RequestReview,
    ) -> Result<String> {
        // What the thread committed joins its unreviewed range first, so the range this review
        // covers isn't reviewed again by itself.
        self.scan_thread_commits(id, true).await;
        let (checkout, base) = self.own_checkout(id).await?;
        let (git, path, wanted) = (self.git.clone(), checkout.clone(), args.commit.clone());
        let board = self.core.board(id).await?;
        let reviews: Vec<ReviewRun> = board
            .reviews
            .values()
            .filter(|review| {
                review.kind == ReviewKind::Code
                    && !matches!(review.state, ReviewState::Failed { .. })
            })
            .cloned()
            .collect();
        let (commit, from, last) = blocking(move || {
            let repo = git.open(Path::new(&path)).map_err(git_error)?;
            let (commit, merge_base) =
                super::phases::candidate_at(&repo, wanted.as_deref(), base.as_deref())
                    .map_err(git_error)?;
            let start = match merge_base {
                Some(merge_base) => merge_base,
                None => repo.resolve(&format!("{}^", commit.0)).map_err(git_error)?,
            };
            let (from, last) = covered_upto(&repo, &reviews, &start, &commit);
            Ok((commit, from, last))
        })
        .await?;
        let short = &commit.0[..commit.0.len().min(12)];
        if from == commit {
            // Reviewed already, to this very commit: its outcome, or a wait for it.
            if let Some(last) = last {
                return self.reply_for(id, last, None, args.wait_seconds).await;
            }
        }
        if !self.reviews_on() {
            return Ok("Reviews are off (the user's setting): no review ran. If you merge, say in the merge message that it wasn't reviewed.".into());
        }
        // After findings, the commits since are their fix: the review re-checks those.
        let recheck = last
            .as_ref()
            .filter(|last| matches!(last.state, ReviewState::Findings { .. }))
            .map(|last| {
                let open: Vec<Finding> = last
                    .items
                    .iter()
                    .filter(|item| !matches!(item.status, FindingStatus::Dismissed { .. }))
                    .cloned()
                    .collect();
                listed(&open)
            })
            .filter(|listed| !listed.is_empty());
        let claim = ReplyWait::new(self, (id.clone(), from.0.clone(), commit.0.clone()));
        let author = self.thread_author(id).await;
        let started = self
            .start_review(NewReview {
                conversation_id: id.clone(),
                request_id: self.request_for(id, None).await,
                task_id: None,
                kind: ReviewKind::Code,
                base: from.clone(),
                tip: commit.clone(),
                author,
                notify: ReviewFor::Orchestrator,
                repo: PathBuf::from(&checkout),
                plan: None,
                recheck,
            })
            .await;
        let review = match started {
            Ok(Some((review, _))) => review,
            Ok(None) => {
                return Ok("Reviews are off (the user's setting): no review ran. If you merge, say in the merge message that it wasn't reviewed.".into());
            }
            Err(err) => return Err(err),
        };
        self.mark_reviewed_to(id, &commit).await;
        if let ReviewState::Failed { reason } = &review.state {
            return Ok(format!(
                "No other-vendor review could run for {short}: {reason}. Whether to merge without it or wait is yours; if you merge, say so in the merge message."
            ));
        }
        self.reply_for(id, review, Some(claim), args.wait_seconds)
            .await
    }

    /// Waits up to `wait_seconds` (default 600, at most 1700) for `review` to end while the
    /// thread's turn runs, and returns its outcome; its message then stays undelivered. One
    /// still running then is said so, and its message comes later as usual.
    async fn reply_for(
        &self,
        id: &ConversationId,
        review: ReviewRun,
        claim: Option<ReplyWait<'_>>,
        wait_seconds: Option<u64>,
    ) -> Result<String> {
        // Ended before this call: its message, if any, went out already.
        let claim = match (claim, review.state == ReviewState::Running) {
            (Some(claim), true) => Some(claim.on(&review.id)),
            (None, true) => Some(
                ReplyWait::new(self, (id.clone(), review.base.clone(), review.tip.clone()))
                    .on(&review.id),
            ),
            (_, false) => None,
        };
        let wait = Duration::from_secs(wait_seconds.unwrap_or(600).min(1700));
        let deadline = tokio::time::Instant::now() + wait;
        let ended = |board: &crate::board::Board| {
            board
                .reviews
                .get(&review.id)
                .filter(|now| now.state != ReviewState::Running)
                .cloned()
        };
        loop {
            if let Some(now) = ended(&self.core.board(id).await?) {
                if let Some(claim) = claim {
                    claim.said();
                }
                return Ok(self.review_reply(&now).await);
            }
            if tokio::time::Instant::now() >= deadline || !self.thread_turn_running(id).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        // Still running: its message is delivered when it ends, clean too, unless it ended
        // just now and its delivery already stood aside for this reply.
        let delivers = claim.is_none_or(ReplyWait::release);
        if !delivers && let Some(now) = ended(&self.core.board(id).await?) {
            return Ok(self.review_reply(&now).await);
        }
        self.await_review(id, &review.id).await;
        Ok(format!(
            "{} still reviews {}..{}: its findings arrive as a message.",
            review.reviewer.label(),
            short(&review.base),
            short(&review.tip)
        ))
    }

    /// `run_check`'s wait for `review`, a review of the thread's own change it waits on, until
    /// `until` while the thread's turn runs and the call isn't `cancel`led (#193). Ended by
    /// then: what its message would have said (none when nothing), which then stays
    /// undelivered. None when it still runs, its message to come, or the call was cancelled,
    /// its message delivered.
    pub(crate) async fn review_in_reply(
        &self,
        id: &ConversationId,
        review: &ReviewRun,
        (until, cancel): (tokio::time::Instant, &CancellationToken),
    ) -> Option<Option<String>> {
        // Claimed under the lock its end is stored under, so it ends either before the claim
        // (none taken) or after it, standing aside for this call.
        let claim = {
            let _held = self.reviews.lock().await;
            let running = self.core.board(id).await.ok().is_some_and(|board| {
                board
                    .reviews
                    .get(&review.id)
                    .is_some_and(|now| now.state == ReviewState::Running)
            });
            (running && tokio::time::Instant::now() < until).then(|| {
                ReplyWait::new(self, (id.clone(), review.base.clone(), review.tip.clone()))
            })
        }?;
        loop {
            if !claim.standing() {
                claim.said();
                break;
            }
            if tokio::time::Instant::now() >= until
                || cancel.is_cancelled()
                || !self.thread_turn_running(id).await
            {
                if claim.release() {
                    return None;
                }
                // It ended just now, its end already standing aside: said here all the same.
                break;
            }
            tokio::select! {
                () = tokio::time::sleep(Duration::from_millis(100)) => {}
                () = cancel.cancelled() => {}
            }
        }
        let ended = self
            .core
            .board(id)
            .await
            .ok()?
            .reviews
            .get(&review.id)?
            .clone();
        if ended.state == ReviewState::Running {
            return None;
        }
        let text = self.review_text(&ended).await;
        if cancel.is_cancelled() {
            // The call ends with no reply to carry it: the thread hears it as a message after
            // all, its claim gone.
            self.tell_review_now(&ended, text.as_deref(), None).await;
            return None;
        }
        Some(self.orchestrator_word(&ended, text.as_deref()).await)
    }

    /// Review `review`'s end stood aside for a request_review call that was cancelled before
    /// it said it: the thread hears it as a message after all (#332).
    async fn tell_unsaid_review(&self, id: &ConversationId, review: &str) {
        let Some(ended) = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.reviews.get(review).cloned())
            .filter(|ended| ended.state != ReviewState::Running)
        else {
            return;
        };
        let text = self.review_text(&ended).await;
        self.tell_review_now(&ended, text.as_deref(), None).await;
    }

    /// An ended review of the thread's candidate, as request_review's reply.
    async fn review_reply(&self, review: &ReviewRun) -> String {
        let reviewer = review.reviewer.label();
        let range = format!("{}..{}", short(&review.base), short(&review.tip));
        match &review.state {
            ReviewState::Findings { count } => format!(
                "[review of your commits · {reviewer} found {count} in {range}]\n{}\n[/review]\nFix each one you agree with, or answer it in one line; the merge message says how each was settled.\n{}",
                clipped(&self.review_text(review).await.unwrap_or_default()),
                record_line(&review.items)
            ),
            ReviewState::Failed { reason } => format!(
                "No other-vendor review could run for {range}: {reason}. Whether to merge without it or wait is yours; if you merge, say so in the merge message."
            ),
            _ => format!(
                "[review of your commits · {reviewer} found nothing in {range}] The review of your candidate found nothing."
            ),
        }
    }

    /// The plan review of a lead's outline, in the background: the orchestrator gives the
    /// go-ahead meanwhile and hears the findings when they come.
    pub(crate) async fn review_plan(&self, lead: &Task, outline: &str) {
        let Some(base) = lead.workspace.as_ref().and_then(|w| w.base.clone()) else {
            tracing::warn!(task = %lead.id, "an outline without a checkout gets no plan review");
            return;
        };
        let repo = match self.task_repo(lead) {
            Ok(repo) => repo,
            Err(err) => {
                tracing::warn!(task = %lead.id, error = %err, "an outline could not be reviewed");
                return;
            }
        };
        let mut brief = lead.spec.clone();
        for message in &lead.messages {
            brief.push_str(&format!("\n---\n{message}"));
        }
        let started = self
            .start_review(NewReview {
                conversation_id: lead.conversation_id.clone(),
                request_id: lead.request_id.clone(),
                task_id: Some(lead.id.clone()),
                kind: ReviewKind::Plan,
                base: Oid(base.clone()),
                tip: Oid(base),
                author: Author {
                    provider: lead.route.choice.provider,
                    model: lead.route.choice.model.clone(),
                },
                notify: ReviewFor::Orchestrator,
                repo,
                plan: Some((brief, outline.to_owned())),
                recheck: None,
            })
            .await;
        match started {
            Ok(Some((review, true))) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %lead.id, error = %err, "an outline could not be reviewed");
            }
        }
    }

    /// `review_plan`: one review of the thread's own plan, against its brief, by the vendor
    /// other than the thread's, in a checkout of its workspace's tip. Returns at once; the
    /// findings arrive as a `[plan review …]` message.
    pub(crate) async fn review_thread_plan(
        &self,
        conversation_id: &ConversationId,
        args: crate::tools::ReviewPlan,
    ) -> Result<String> {
        if args.plan.trim().is_empty() {
            return Err(Error::Invalid("The plan is empty.".into()));
        }
        let workspace = self
            .effective_workspace(conversation_id)
            .await?
            .ok_or_else(|| {
                Error::Invalid("Only a session's thread asks for a plan review.".into())
            })?;
        let (git, path) = (self.git.clone(), workspace.path.clone());
        let tip = blocking(move || {
            git.open_worktree(&path)
                .map_err(git_error)?
                .head()
                .map_err(git_error)
        })
        .await?;
        let author = self.thread_author(conversation_id).await;
        let Some((review, _)) = self
            .start_review(NewReview {
                conversation_id: conversation_id.clone(),
                request_id: self.request_for(conversation_id, None).await,
                task_id: None,
                kind: ReviewKind::Plan,
                base: tip.clone(),
                tip,
                author,
                notify: ReviewFor::Orchestrator,
                repo: workspace.repo,
                plan: Some((args.brief, args.plan)),
                recheck: None,
            })
            .await?
        else {
            return Ok(REVIEWS_OFF_PLAN.to_owned());
        };
        let reviewer = review.reviewer.label();
        Ok(match &review.state {
            ReviewState::Failed { reason } => {
                format!("The plan review could not start: {reason}. Judge the plan yourself.")
            }
            _ => format!(
                "Started a review of your plan by {reviewer}. It returns now: carry on. Its findings arrive as a [plan review …] message."
            ),
        })
    }

    /// The thread's model, as an author whose work the other vendor reviews: the one its CLI
    /// runs on, else the session's choice.
    pub(crate) async fn thread_author(&self, conversation_id: &ConversationId) -> Author {
        let running = match self.conv(conversation_id) {
            Ok(conv) => conv.live_cli().await.map(|cli| cli.model.clone()),
            Err(_) => None,
        };
        let choice = running.or_else(|| {
            self.core
                .conversation(conversation_id)
                .ok()
                .and_then(|conversation| conversation.setup)
                .map(|setup| setup.choice().clone())
        });
        match choice {
            Some(choice) => Author {
                provider: choice.provider,
                model: choice.model,
            },
            None => Author {
                provider: ProviderKind::Claude,
                model: None,
            },
        }
    }

    /// `request_review`: one review of the caller's committed work, from where it started, by
    /// the vendor other than its phase's author. Returns at once; the findings arrive as a
    /// message.
    pub(crate) async fn worker_request_review(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
    ) -> Result<String> {
        let task = self.task_by_id(conversation_id, task_id).await?;
        if !task.kind.writes() || task.kind == TaskKind::Merge {
            return Err(Error::Invalid(
                "Only a worker that builds a change asks for its review.".into(),
            ));
        }
        if self.held_by_plan_mode(&task).await {
            return Err(Error::Invalid(super::phases::PLAN_MODE_HOLD.into()));
        }
        // A resumed lead still has its old instructions to ask for one (SESSION-FIXES-PLAN.md
        // Q6.1): its fix's landing has the high findings re-checked instead.
        if task.role == Some(WorkerRole::Fix)
            && let Some(subject) = &task.subject
            && self
                .task_by_id(conversation_id, subject)
                .await
                .is_ok_and(|subject| subject.landed.is_some())
        {
            return Err(Error::Invalid(super::review_hold::NO_FIX_REVIEW.into()));
        }
        if !matches!(task.state, TaskState::Starting | TaskState::Running) {
            return Err(Error::Invalid(format!(
                "task-{} is {:?}: ask for the review while you work, before your report",
                task.number, task.state
            )));
        }
        if !self.reviews_on() {
            return Ok(REVIEWS_OFF_CODE.to_owned());
        }
        // Every new file but litter: no report names them yet, and the landing checks their
        // provenance over the whole range anyway.
        self.commit_leftovers(
            &task,
            &[".".to_owned()],
            &format!(
                "{}\n\nWork in progress, committed for its review.",
                task.title
            ),
        )
        .await?;
        let workspace = task
            .workspace
            .clone()
            .ok_or_else(|| Error::Invalid("the task has no workspace".into()))?;
        let (Some(worktree), Some(base)) = (workspace.worktree, workspace.base) else {
            return Err(Error::Invalid("the task has no worktree".into()));
        };
        let git = self.git.clone();
        let tip = blocking(move || {
            git.open_worktree(Path::new(&worktree))
                .map_err(git_error)?
                .head()
                .map_err(git_error)
        })
        .await?;
        if tip.0 == base {
            return Err(Error::Invalid(
                "Nothing is committed since your work started: there is nothing to review yet."
                    .into(),
            ));
        }
        let author = self.phase_author(&task).await;
        let repo = self.task_repo(&task)?;
        let Some((review, started)) = self
            .start_review(NewReview {
                conversation_id: conversation_id.clone(),
                request_id: task.request_id.clone(),
                task_id: Some(task.id.clone()),
                kind: ReviewKind::Code,
                base: Oid(base),
                tip,
                author: Author {
                    provider: author.route.choice.provider,
                    model: author.route.choice.model.clone(),
                },
                notify: ReviewFor::Worker {
                    task_id: task.id.clone(),
                },
                repo,
                plan: None,
                recheck: None,
            })
            .await?
        else {
            return Ok(REVIEWS_OFF_CODE.to_owned());
        };
        let reviewer = review.reviewer.label();
        let waits = task.role == Some(WorkerRole::Verifier);
        Ok(match (&review.state, started) {
            (ReviewState::Running, true) if waits => format!(
                "Started a review of your committed work by {reviewer}. It returns now: gather your evidence and run your checks meanwhile. Its findings arrive as a message from Brigadier; if you finish everything else first, end your turn and they start your next one. Fix what you agree with and commit, then report."
            ),
            (ReviewState::Running, true) => format!(
                "Started a review of your committed work by {reviewer}. It returns now: gather your evidence and run your checks meanwhile, and don't wait for it. Findings that arrive before your report come as a message from Brigadier: fix what you agree with and commit. Once your checks pass and your evidence is in, report; later findings go to the session's main thread."
            ),
            (ReviewState::Running, false) if review.notify == ReviewFor::Worker { task_id: task.id.clone() } && waits => {
                "This exact work is being reviewed already; the findings arrive as a message."
                    .to_owned()
            }
            (ReviewState::Running, false) if review.notify == ReviewFor::Worker { task_id: task.id.clone() } => {
                "This exact work is being reviewed already; findings that arrive before your report come as a message. Don't wait for them."
                    .to_owned()
            }
            (ReviewState::Running, false) => {
                "This exact work is being reviewed already for someone else, who gets its findings. Check your work yourself meanwhile and report."
                    .to_owned()
            }
            (ReviewState::Clean, _) => {
                "This exact work was reviewed already and the reviewer found nothing. Carry on."
                    .to_owned()
            }
            (ReviewState::Findings { .. }, _) => match self.review_text(&review).await {
                Some(text) => format!(
                    "This exact work was reviewed already:\n[review]\n{}\n[/review]\nFix each finding you agree with and commit; for one you don't, say why in your report.",
                    clipped(&text)
                ),
                None => {
                    "This exact work was reviewed already; its findings went to the session's main thread."
                        .to_owned()
                }
            },
            (ReviewState::Failed { reason }, _) => format!(
                "The review could not run: {reason}. Review your diff yourself, carefully, and say so in your report."
            ),
        })
    }

    /// Records a review and starts it, or finds the one its range already has (a code review;
    /// then `false`). That one passes to the new asker when the worker it was for no longer
    /// works: a verifier on its lead's last commit, or the landing of a worker's reviewed tip.
    /// One whose reviewer can't be routed is recorded failed, not started. None while the
    /// user has reviews off: nothing is recorded or started.
    async fn start_review(&self, new: NewReview) -> Result<Option<(ReviewRun, bool)>> {
        // A closing conversation starts nothing more.
        drop(self.enter(&new.conversation_id)?);
        // Held until the review is recorded and spawned, as switching reviews holds it: a
        // review starts entirely before the switch or not at all after it.
        let _held = self.reviews.lock().await;
        let board = self.core.board(&new.conversation_id).await?;
        // A range reviewed already keeps its review whatever the switch says now: a landed
        // worker stays held for its findings until they settle.
        if new.kind == ReviewKind::Code
            && let Some(known) = review_of(board.reviews.values(), &new.base.0, &new.tip.0)
        {
            let mut known = known.clone();
            if known.notify != new.notify
                && let ReviewFor::Worker { task_id } = &known.notify
                && !self.still_works(&known.conversation_id, task_id).await
            {
                known.notify = new.notify.clone();
                self.store_review(&known).await?;
            }
            return Ok(Some((known, false)));
        }
        if !self.reviews_on() {
            return Ok(None);
        }
        let reviewer = other_vendor(new.author.provider);
        let mut review = ReviewRun {
            id: uuid::Uuid::now_v7().to_string(),
            conversation_id: new.conversation_id.clone(),
            request_id: new.request_id.clone(),
            task_id: new.task_id.clone(),
            kind: new.kind,
            base: new.base.0.clone(),
            tip: new.tip.0.clone(),
            author: new.author.provider,
            reviewer,
            reviewer_model: None,
            notify: new.notify.clone(),
            state: ReviewState::Running,
            started_at_ms: now_ms(),
            ended_at_ms: None,
            findings: None,
            number: next_number(board.reviews.values()),
            items: Vec::new(),
            landing: None,
            answer_waits: false,
        };
        let picked = self.reviewer_model(&new, reviewer).await;
        // Picking the reviewer took time: the switch may have been turned off meanwhile, and
        // it waits for this lock only once the setting is written.
        if !self.reviews_on() {
            return Ok(None);
        }
        let effort = match picked {
            Ok((model, effort)) => {
                review.reviewer_model = Some(model);
                effort
            }
            Err(why) => {
                review.state = ReviewState::Failed { reason: why };
                review.ended_at_ms = Some(now_ms());
                self.store_review(&review).await?;
                return Ok(Some((review, true)));
            }
        };
        self.store_review(&review).await?;
        let manager = self.arc();
        let started = review.clone();
        let stop = tokio_util::sync::CancellationToken::new();
        self.running_reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                review.id.clone(),
                (review.conversation_id.clone(), stop.clone()),
            );
        self.spawn(async move {
            let outcome = if manager.is_closing(&started.conversation_id) {
                Err(brigadier_review::STOPPED.to_owned())
            } else {
                manager
                    .run_review(
                        &started,
                        &new.repo,
                        new.plan.as_ref(),
                        new.recheck.as_deref(),
                        effort,
                        stop,
                    )
                    .await
            };
            manager
                .running_reviews
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&started.id);
            manager.finish_review(started, outcome).await;
        });
        Ok(Some((review, true)))
    }

    /// The model (and effort) that reviews `new`: the router's pick of `reviewer`'s models,
    /// at a vendor's best, never the author's model. Why none can, otherwise.
    async fn reviewer_model(
        &self,
        new: &NewReview,
        reviewer: ProviderKind,
    ) -> std::result::Result<(String, Option<String>), String> {
        let project = self
            .core
            .conversation(&new.conversation_id)
            .ok()
            .and_then(|conversation| conversation.project_id);
        let (preview, _) = self
            .preview(&super::routing::Ask {
                category: TaskCategory::Review,
                areas: &[],
                floor: QualityTier::Frontier,
                floor_binds: false,
                needs: Needs::default(),
                pin: Some(Pin {
                    provider: Some(reviewer),
                    model: None,
                    effort: Some(brigadier_review::EFFORT.to_owned()),
                    by_user: false,
                }),
                hold_pin: true,
                avoid: Some(new.author.clone()),
                distinct_from: Vec::new(),
                exclude: &[],
                project_id: project.as_ref(),
                trial: super::routing::Trial::Never,
            })
            .await;
        match preview.decision {
            Decision::Run(routed) if routed.provider == reviewer => Ok((
                routed.model,
                routed
                    .effort
                    .or_else(|| Some(brigadier_review::EFFORT.to_owned())),
            )),
            Decision::Run(_) => Err(format!("no {} model may review it now", reviewer.label())),
            Decision::Wait(waiting) => Err(format!(
                "no {} model can review it now ({})",
                reviewer.label(),
                waiting.reason
            )),
        }
    }

    /// Where review `review` checks out the code it reads: a worktree of its own, removed after.
    fn review_worktree(&self, review: &ReviewRun) -> PathBuf {
        let project = self
            .core
            .conversation(&review.conversation_id)
            .ok()
            .and_then(|conversation| conversation.project_id)
            .map_or_else(|| "none".to_owned(), |id| id.0);
        self.owned_dir("worktrees", &project)
            .join(review_dir_name(review))
    }

    /// Runs `review` in a checkout of its own and removes everything it made after, also when
    /// `stop` ends it early (its conversation closed).
    async fn run_review(
        &self,
        review: &ReviewRun,
        repo: &Path,
        plan: Option<&(String, String)>,
        recheck: Option<&str>,
        effort: Option<String>,
        stop: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Review, String> {
        let owner = review_owner(&review.id);
        let name = review_dir_name(review);
        let checkout = self.review_worktree(review);
        let scratch = self.owned_dir("scratch", &name);
        // Its checkout is made to the end even when it is stopped meanwhile, so that what is
        // disposed after is all there is.
        let outcome = match self
            .set_up_review(review, &owner, repo, &checkout, &scratch)
            .await
        {
            Err(why) => Err(why),
            Ok(()) if stop.is_cancelled() => Err(brigadier_review::STOPPED.to_owned()),
            Ok(()) => {
                self.review_in(
                    review, &owner, &checkout, &scratch, plan, recheck, effort, stop,
                )
                .await
            }
        };
        let leftovers = self.runtime.ledger().dispose(&owner).await;
        if !leftovers.is_clean() {
            tracing::warn!(review = %review.id, failures = ?leftovers.failures, "a review's checkout is not removed yet");
        }
        outcome
    }

    /// Makes `review`'s detached checkout at its tip, and its scratch folder.
    async fn set_up_review(
        &self,
        review: &ReviewRun,
        owner: &str,
        repo: &Path,
        checkout: &Path,
        scratch: &Path,
    ) -> std::result::Result<(), String> {
        let setting_up = |err: Error| format!("its checkout could not be made: {err}");
        let ledger = self.runtime.ledger();
        // Recorded before it exists: a restart removes what is left of it.
        ledger
            .record(
                owner,
                Artifact::Worktree {
                    repo: repo.to_string_lossy().into_owned(),
                    path: checkout.to_string_lossy().into_owned(),
                },
            )
            .await
            .map_err(setting_up)?;
        ledger
            .record(
                owner,
                Artifact::ProcessesIn {
                    dir: checkout.to_string_lossy().into_owned(),
                },
            )
            .await
            .map_err(setting_up)?;
        self.prepare_owned_dir(owner, scratch)
            .await
            .map_err(setting_up)?;
        let (git, repo_path, path, at) = (
            self.git.clone(),
            repo.to_owned(),
            checkout.to_owned(),
            Oid(review.tip.clone()),
        );
        blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            git.open(&repo_path)
                .map_err(git_error)?
                .add_worktree(&path, WorktreeSpec::Detached { at })
                .map_err(git_error)?;
            super::sidecars::stand_in_at(&git, &path);
            Ok(())
        })
        .await
        .map_err(setting_up)
    }

    /// Runs `review` in its checkout: Codex as `codex exec`, Claude through its adapter.
    #[allow(clippy::too_many_arguments)]
    async fn review_in(
        &self,
        review: &ReviewRun,
        owner: &str,
        checkout: &Path,
        scratch: &Path,
        plan: Option<&(String, String)>,
        recheck: Option<&str>,
        effort: Option<String>,
        stop: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Review, String> {
        let subject = match (plan, recheck) {
            (Some((brief, outline)), _) => Subject::Plan { brief, outline },
            (None, Some(findings)) => Subject::Recheck {
                base: &review.base,
                findings,
            },
            (None, None) => Subject::Code { base: &review.base },
        };
        if self.reviews_hosted(review.reviewer) {
            return tokio::select! {
                outcome = self.review_hosted(review, owner, checkout, subject, effort) => outcome,
                () = stop.cancelled() => Err(brigadier_review::STOPPED.to_owned()),
            };
        }
        let meter = TokenMeter::default();
        meter.turn_started(now_ms());
        let ran = brigadier_review::run_codex(CodexRun {
            platform: self.runtime.platform().clone(),
            env: self.runtime.cli_env(),
            cwd: checkout,
            output: &scratch.join("review.md"),
            model: review.reviewer_model.as_deref(),
            subject,
            ledger: self.runtime.ledger().handle(owner.to_owned()),
            time: REVIEW_TIME,
            stop,
        })
        .await;
        // What it used counts whether or not it came to a review (Claude's is counted as it
        // goes).
        if let Some(usage) = &ran.usage {
            self.note_tokens(
                &meter,
                review.reviewer,
                review.reviewer_model.as_deref(),
                TokenOwner::Task(&review.conversation_id, &TaskId(review_owner(&review.id))),
                usage,
                None,
                &[],
            )
            .await;
        }
        ran.outcome
    }

    /// Whether `reviewer`'s reviews run through its adapter: Claude's always; Codex's run as
    /// `codex exec`, except with scripted CLIs in tests.
    fn reviews_hosted(&self, reviewer: ProviderKind) -> bool {
        #[cfg(test)]
        if self.runtime.faked() {
            return true;
        }
        reviewer == ProviderKind::Claude
    }

    /// A review through the reviewer's adapter: read-only, with only the tools that read a
    /// change, and nobody to ask (`dontAsk`: what isn't allowed is denied).
    async fn review_hosted(
        &self,
        review: &ReviewRun,
        owner: &str,
        checkout: &Path,
        subject: Subject<'_>,
        effort: Option<String>,
    ) -> std::result::Result<Review, String> {
        let spec = SessionSpec {
            cwd: checkout.to_owned(),
            model: review.reviewer_model.clone(),
            effort,
            fast: false,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(brigadier_review::REVIEW_ROLE.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::Review,
            skill_plugins: Vec::new(),
            add_dirs: Vec::new(),
            project: None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: true,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: Some(checkout.to_owned()),
            auto_compact: true,
            // It hands nothing on: no sub-agents.
            allowed_models: Some(AllowedModels::default()),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            deny_tools: None,
            protection: Default::default(),
        };
        let counted_as = TaskId(review_owner(&review.id));
        let account = self
            .runtime
            .launch_account(review.reviewer, review.reviewer_model.as_deref());
        let meter = TokenMeter::default().on_account(account.account.clone());
        meter.turn_started(now_ms());
        let Started {
            session,
            mut events,
        } = self
            .runtime
            .start_hosted(owner, &account, spec)
            .await
            .map_err(|err| format!("its CLI didn't start: {err}"))?;
        let turn = async {
            session
                .send(TurnInput::text(brigadier_review::claude_prompt(subject)))
                .await
                .map_err(|err| format!("it didn't start: {err}"))?;
            let mut last = String::new();
            while let Some(event) = events.recv().await {
                match event {
                    ProviderEvent::Message {
                        role: Role::Assistant,
                        text,
                        ..
                    } => last = text,
                    ProviderEvent::RateLimits { quota } => {
                        self.runtime
                            .note_quota_snapshot(&account, quota.clone())
                            .await;
                    }
                    ProviderEvent::Usage {
                        total,
                        last: latest,
                        calls,
                    } => {
                        self.note_tokens(
                            &meter,
                            review.reviewer,
                            review.reviewer_model.as_deref(),
                            TokenOwner::Task(&review.conversation_id, &counted_as),
                            &total,
                            latest.as_ref(),
                            &calls,
                        )
                        .await;
                    }
                    ProviderEvent::ApprovalRequested { request } => {
                        let _ = session
                            .answer(
                                request.id,
                                ApprovalDecision::Deny {
                                    message: "Declined: a review only reads the change.".into(),
                                },
                            )
                            .await;
                    }
                    ProviderEvent::TurnCompleted { status, .. } => {
                        return match status {
                            TurnStatus::Completed => Ok(last),
                            TurnStatus::Failed | TurnStatus::Interrupted => {
                                Err("its turn did not finish".to_owned())
                            }
                        };
                    }
                    ProviderEvent::Exited { .. } => return Err("its CLI exited".to_owned()),
                    _ => {}
                }
            }
            Err("its CLI went away".to_owned())
        };
        let reply = tokio::time::timeout(REVIEW_TIME, turn)
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "it ran out of its {}-minute time box",
                    REVIEW_TIME.as_secs() / 60
                ))
            });
        session.close().await;
        let text = reply?.trim().to_owned();
        if text.is_empty() {
            return Err("it gave no review".to_owned());
        }
        Ok(Review {
            findings: brigadier_review::count_findings(&text),
            text,
        })
    }

    /// Whether `task_id` still works: a review's news reaches it as a message.
    async fn still_works(&self, conversation_id: &ConversationId, task_id: &TaskId) -> bool {
        self.task_by_id(conversation_id, task_id)
            .await
            .is_ok_and(|task| {
                matches!(
                    task.state,
                    TaskState::Starting | TaskState::Running | TaskState::Blocked
                )
            })
    }

    /// Records how `review` ended and tells whoever asked.
    async fn finish_review(&self, review: ReviewRun, outcome: std::result::Result<Review, String>) {
        let id = review.conversation_id.clone();
        // From before its end is stored until its findings are queued for the thread, the
        // session's full checks wait as for a running review (BUILD-PLAN C3).
        let telling = self.conv(&id).ok().map(Telling::new);
        crate::heap(|| self.end_review(review, outcome)).await;
        drop(telling);
        self.wake_checks(&id);
    }

    async fn end_review(&self, review: ReviewRun, outcome: std::result::Result<Review, String>) {
        let mut ended = review;
        ended.ended_at_ms = Some(now_ms());
        let text = match outcome {
            Ok(found) => {
                match self
                    .core
                    .store()
                    .blobs()
                    .put(found.text.clone().into_bytes())
                    .await
                {
                    Ok(hash) => ended.findings = Some(hash.to_string()),
                    Err(err) => {
                        tracing::warn!(review = %ended.id, error = %err, "could not keep a review's text");
                    }
                }
                let checkout = self.review_worktree(&ended);
                ended.items = items_of(ended.number, &found.text, Some(&checkout));
                ended.state = match u32::try_from(ended.items.len()).unwrap_or(u32::MAX) {
                    0 => ReviewState::Clean,
                    count => ReviewState::Findings { count },
                };
                Some(found.text)
            }
            Err(reason) => {
                ended.state = ReviewState::Failed { reason };
                None
            }
        };
        tracing::info!(review = %ended.id, kind = ?ended.kind, state = ?ended.state, "one-shot review ended");
        // A conversation deleted meanwhile keeps nothing more.
        if self
            .core
            .conversation(&ended.conversation_id)
            .ok()
            .is_none_or(|conversation| conversation.deleting)
        {
            return;
        }
        {
            // Whoever it passed to meanwhile hears it ([`Self::start_review`]).
            let _held = self.reviews.lock().await;
            if let Ok(board) = self.core.board(&ended.conversation_id).await
                && let Some(now) = board.reviews.get(&ended.id)
            {
                ended.notify = now.notify.clone();
                ended.landing = now.landing.clone();
                ended.answer_waits = now.answer_waits;
            }
            // A worker that reported before its review ended never reads the findings: the
            // thread gets them, as it gets a landing review's.
            if let ReviewFor::Worker { task_id } = &ended.notify
                && ended.kind == ReviewKind::Code
                && !self.still_works(&ended.conversation_id, task_id).await
            {
                ended.notify = ReviewFor::Orchestrator;
            }
            if let Err(err) = self.store_review(&ended).await {
                tracing::warn!(review = %ended.id, error = %err, "could not record a review's end");
            }
        }
        if let Some(step) = reviewed_step(&ended) {
            let request = match &ended.request_id {
                Some(request) => Some(request.clone()),
                None => {
                    self.request_for(&ended.conversation_id, ended.task_id.as_ref())
                        .await
                }
            };
            self.orchestrator_step_in(&ended.conversation_id, request, step)
                .await;
        }
        self.tell_review(&ended, text.as_deref()).await;
        // Nothing comes back from a landing's review that found nothing or couldn't run.
        if ended.kind == ReviewKind::Code
            && matches!(ended.state, ReviewState::Clean | ReviewState::Failed { .. })
        {
            self.settle_review_batch(&ended.conversation_id, batch_tip(&ended))
                .await;
        }
    }

    /// Hands a review's outcome to whoever asked: the worker while it still works, else the
    /// orchestrator. A clean review of a landing or an outline is no news for the orchestrator:
    /// the context card shows it, and finish_session's answer says it.
    pub(super) async fn tell_review(&self, review: &ReviewRun, text: Option<&str>) {
        self.tell_review_filed(review, text, None).await;
    }

    /// [`Self::tell_review`], its word filed under `request` when given, else under the
    /// request new work is filed under now.
    async fn tell_review_filed(
        &self,
        review: &ReviewRun,
        text: Option<&str>,
        request: Option<String>,
    ) {
        self.tell_review_now(review, text, request).await;
        // An overnight phase that waited for this review to settle (BUILD-PLAN E2).
        crate::heap(|| self.settle_wait_over(review)).await;
        // A merge held for the review's end looks again (SESSION-FIXES-PLAN.md Q3), on its
        // own: a review that fails at once ends inside finish_session, which holds the merge
        // lock this takes. Then the requests settle: one that waited for this review is over
        // unless that look, or the review's findings, woke the thread for it. A clean review
        // tells nobody, so nothing else would end it.
        let (manager, id) = (self.arc(), review.conversation_id.clone());
        self.spawn(async move {
            manager.check_held_merge(&id).await;
            manager.settle_requests(&id).await;
        });
    }

    /// What the thread hears of `review`'s end when it isn't a worker's to hear (`text`, its
    /// findings): none for a clean one it doesn't wait for, or one still running.
    async fn orchestrator_word(&self, review: &ReviewRun, text: Option<&str>) -> Option<String> {
        let reviewer = review.reviewer.label();
        let number = match &review.task_id {
            Some(task_id) => self
                .task_by_id(&review.conversation_id, task_id)
                .await
                .ok()
                .map(|task| task.number),
            None => None,
        };
        let of = number.map_or_else(|| "the session".to_owned(), |n| format!("task-{n}"));
        // A worker's own review whose worker reported before it ended: its work may not have
        // landed yet.
        let unlanded = match &review.task_id {
            Some(task_id) => self
                .task_by_id(&review.conversation_id, task_id)
                .await
                .is_ok_and(|task| task.landed.is_none() && task.state == TaskState::Reported),
            None => false,
        };
        let range = format!("{}..{}", short(&review.base), short(&review.tip));
        // How a fix is made: on the user's own checkout a fix is a change like any other (UI
        // audit M6, issue #33), left uncommitted unless the user asks.
        let local = self.local_checkout(&review.conversation_id).is_some();
        let fix_rule = if local {
            "This is the user's own checkout: make the fix yourself and leave it uncommitted, with the one line saying so, unless the user asked you to commit it; it gets no review of its own. run_check's `fixes` (their ids) records them fixed when its check passes."
                .to_owned()
        } else {
            format!(
                "Commit a fix of yours, with nothing else in it, with run_check's `commit` and `fixes` (their ids; `high` names those that lose data or break a build or a security boundary, whose fix then gets one re-check): on a pass it commits with `--trailer \"{FIXES_TRAILER}: <their ids>\"` and records them fixed, checked by that check; such a commit gets no review of its own."
            )
        };
        let text = match (review.kind, &review.state, text) {
            // The thread's own commits and plans.
            (ReviewKind::Code, ReviewState::Findings { count }, Some(text))
                if review.task_id.is_none() =>
            {
                format!(
                    "[review of your commits · {reviewer} found {count} in {range}]\n{}\n[/review]\nThis is the one background review of the {} you made yourself. Fix what you agree with ({}), or tell the user why not. {fix_rule} There are no review rounds.\n{}",
                    clipped(text),
                    if local { "changes" } else { "commits" },
                    if local {
                        "a fix of your own"
                    } else {
                        "a tiny fix of your own, or delegate one"
                    },
                    record_line(&review.items)
                )
            }
            (ReviewKind::Plan, ReviewState::Findings { count }, Some(text))
                if review.task_id.is_none() =>
            {
                format!(
                    "[plan review of your plan · {reviewer} found {count}]\n{}\n[/plan review]\nWeigh them against the brief (it wins any conflict; never reopen what is settled), and change your plan, or the briefs you send, for the ones you agree with. There are no review rounds.",
                    clipped(text)
                )
            }
            (ReviewKind::Code, ReviewState::Failed { reason }, _) if review.task_id.is_none() => {
                format!(
                    "[review of your commits · could not run] The background review of {range} could not run: {reason}. Nothing waits on it; judge yourself whether your change needs a closer look."
                )
            }
            (ReviewKind::Plan, ReviewState::Failed { reason }, _) if review.task_id.is_none() => {
                format!(
                    "[plan review of your plan · could not run] The plan review could not run: {reason}. Judge the plan yourself."
                )
            }
            (ReviewKind::Code, ReviewState::Findings { count }, Some(text)) => format!(
                "[review {of} · {reviewer} found {count} in {range}]\n{}\n[/review]\nThis is the one background review of what landed{}. Fix what you agree with ({}), or tell the user why not. {fix_rule}{} There are no review rounds.\n{}",
                clipped(text),
                if unlanded {
                    " (its worker reported before it ended: land the work as usual, and its fixes come after)"
                } else {
                    ""
                },
                if local {
                    "a fix of your own"
                } else {
                    "delegate a fix, or a tiny fix of your own"
                },
                if local {
                    ""
                } else {
                    " A held merge still covers it."
                },
                record_line(&review.items)
            ),
            (ReviewKind::Plan, ReviewState::Findings { count }, Some(text)) => format!(
                "[plan review {of} · {reviewer} found {count}]\n{}\n[/plan review]\nWeigh them against the brief (it wins any conflict; never reopen what is settled). If {of} still waits for its go-ahead, put the ones you agree with in approve_outline's corrections; if it has it, send them with message_worker. There are no review rounds.",
                clipped(text)
            ),
            (ReviewKind::Code, ReviewState::Failed { reason }, _) => format!(
                "[review {of} · could not run] The background review of {range} could not run: {reason}. Nothing waits on it; judge yourself whether the change needs a closer look."
            ),
            (ReviewKind::Plan, ReviewState::Failed { reason }, _) => format!(
                "[plan review {of} · could not run] The plan review could not run: {reason}. Judge the outline yourself."
            ),
            // A clean review of the thread's own changes is news only when its answer waits
            // for it (BUILD-PLAN B2: `run_check` told it to).
            (ReviewKind::Code, ReviewState::Clean, _)
                if review.task_id.is_none() && self.answer_waits_for(review).await =>
            {
                format!(
                    "[review of your commits · {reviewer} found nothing in {range}] The background review of your change found nothing."
                )
            }
            // Clean, or still running: nothing to say.
            _ => return None,
        };
        Some(text)
    }

    async fn tell_review_now(&self, review: &ReviewRun, text: Option<&str>, filed: Option<String>) {
        // A request_review or run_check call that waited on it said its outcome in its reply.
        if review.notify == ReviewFor::Orchestrator
            && self.reply_claims().stand_aside(&(
                review.conversation_id.clone(),
                review.base.clone(),
                review.tip.clone(),
            ))
        {
            return;
        }
        let review_text = text;
        let reviewer = review.reviewer.label();
        if let ReviewFor::Worker { task_id } = &review.notify
            && let Ok(task) = self.task_by_id(&review.conversation_id, task_id).await
            && matches!(
                task.state,
                TaskState::Starting | TaskState::Running | TaskState::Blocked
            )
        {
            let message = match (&review.state, text) {
                (ReviewState::Findings { count }, Some(text)) => format!(
                    "[review of your work · {reviewer} found {count}]\n{}\n[/review]\nFix each finding you agree with and commit; for one you don't, say why in your report. One that asks for something your task didn't, or contradicts it, is no bug: note it in your report, don't build it. There are no review rounds: once your fixes are committed, your evidence is in and the checks they touch pass, report at once; don't verify again.",
                    clipped(text)
                ),
                (ReviewState::Failed { reason }, _) => format!(
                    "[review of your work] The review could not run: {reason}. Review your diff yourself, carefully, and say so in your report."
                ),
                _ => format!(
                    "[review of your work] {reviewer} found nothing. Once your checks pass and your evidence is in, report at once; don't verify again."
                ),
            };
            if task.blocked_reason.as_deref() == Some(WAITING_FOR_REVIEW) {
                self.set_task_blocked(&task.id, None).await;
            }
            let now = self
                .task_by_id(&review.conversation_id, task_id)
                .await
                .unwrap_or(task);
            let rounds = now.rework_rounds;
            match self
                .tell_worker(&review.conversation_id, &now, message, "Brigadier")
                .await
            {
                Ok(_) => {
                    // Waking a worker that waited for its own review is no rework round.
                    let _ = self
                        .update_task(&review.conversation_id, task_id, |t| {
                            t.rework_rounds = t.rework_rounds.min(rounds);
                        })
                        .await;
                    return;
                }
                Err(err) => {
                    tracing::info!(review = %review.id, error = %err, "a review's worker took no message; the orchestrator hears it");
                }
            }
        }
        let of = match &review.task_id {
            Some(task_id) => self
                .task_by_id(&review.conversation_id, task_id)
                .await
                .ok()
                .map_or_else(
                    || "the session".to_owned(),
                    |task| format!("task-{}", task.number),
                ),
            None => "the session".to_owned(),
        };
        let Some(text) = self.orchestrator_word(review, text).await else {
            return;
        };
        let range = format!("{}..{}", short(&review.base), short(&review.tip));
        let request = match filed {
            Some(request) => Some(request),
            None => {
                self.request_for(&review.conversation_id, review.task_id.as_ref())
                    .await
            }
        };
        // An overnight run that has ended wakes nobody: its report was written before this
        // review ended, so the user reads the outcome in the thread instead.
        if let Some(request) = &request
            && self
                .core
                .board(&review.conversation_id)
                .await
                .is_ok_and(|board| super::requests::ended_run_request(&board, request))
        {
            let note = match &review.state {
                ReviewState::Findings { count } => format!(
                    "The background review of {of} ({range}) ended after the run's report: {reviewer} found {count}.\n{}",
                    clipped(review_text.unwrap_or_default())
                ),
                ReviewState::Failed { reason } => format!(
                    "The background review of {of} ({range}) ended after the run's report: it could not run ({reason})."
                ),
                _ => return,
            };
            self.notice(
                &review.conversation_id,
                brigadier_providers::NoticeLevel::Warning,
                &note,
            )
            .await;
            return;
        }
        self.deliver_for(
            &review.conversation_id,
            Envelope {
                kind: InjectionKind::Report,
                label: report_label(review, &of),
                task_id: review.task_id.clone(),
                text,
            },
            request,
        )
        .await;
    }

    /// Whether `run_check` told the thread to wait for `review` before it answers, and its
    /// request still works: then its clean end is news too. One the user stopped meanwhile
    /// stays stopped, rather than being woken to answer.
    async fn answer_waits_for(&self, review: &ReviewRun) -> bool {
        let (true, Some(request)) = (review.answer_waits, review.request_id.as_deref()) else {
            return false;
        };
        let working = self
            .core
            .board(&review.conversation_id)
            .await
            .is_ok_and(|board| {
                board
                    .requests
                    .get(request)
                    .is_some_and(|of| of.state == crate::work::RequestState::Working)
            });
        working
            && match self.conv(&review.conversation_id) {
                Ok(conv) => !conv.request_ended(request).await,
                Err(_) => false,
            }
    }

    /// `run_check` tells the thread to wait for `review`, a review of its own change for its
    /// running request: recorded on the review, under the lock its end is stored under, so
    /// the end sees it whenever it comes, and after a restart too. False when the review is
    /// no longer running.
    pub(crate) async fn await_review(&self, id: &ConversationId, review: &str) -> bool {
        let _held = self.reviews.lock().await;
        let Some(mut running) = self.core.board(id).await.ok().and_then(|board| {
            board
                .reviews
                .get(review)
                .filter(|review| review.state == ReviewState::Running)
                .cloned()
        }) else {
            return false;
        };
        if running.answer_waits {
            return true;
        }
        running.answer_waits = true;
        match self.store_review(&running).await {
            Ok(()) => true,
            Err(err) => {
                tracing::warn!(review = %running.id, error = %err, "could not record that the answer waits for a review");
                false
            }
        }
    }

    /// The ids in `run_check`'s `fixes` that are no finding of this session, checked before the
    /// check runs: a commit made with an unknown id in its Fixes trailer can't be corrected by
    /// running the check again, as nothing would be left to commit.
    pub(crate) async fn unknown_findings(
        &self,
        conversation_id: &ConversationId,
        fixes: &[String],
    ) -> Result<Vec<String>> {
        let board = self.core.board(conversation_id).await?;
        Ok(fixes
            .iter()
            .filter(|id| {
                !board
                    .reviews
                    .values()
                    .any(|review| review.items.iter().any(|item| &item.id == *id))
            })
            .cloned()
            .collect())
    }

    /// Marks the findings `high` of high severity, before `run_check` commits their fix: the
    /// scan of the thread's new commits may read the fix commit before the check's call ends,
    /// and decides its re-check from the mark then.
    pub(crate) async fn mark_high(
        &self,
        conversation_id: &ConversationId,
        high: &[String],
    ) -> Result<()> {
        if high.is_empty() {
            return Ok(());
        }
        let _held = self.reviews.lock().await;
        let board = self.core.board(conversation_id).await?;
        for review in board.reviews.values() {
            if !review
                .items
                .iter()
                .any(|item| high.contains(&item.id) && !item.high)
            {
                continue;
            }
            let mut review = review.clone();
            for item in &mut review.items {
                if high.contains(&item.id) {
                    item.high = true;
                }
            }
            self.store_review(&review).await?;
        }
        Ok(())
    }

    /// `run_check`'s `fixes` (BUILD-PLAN O4): the review findings the thread's check fixes,
    /// recorded fixed and checked by that check (`command`, its output `alias`) when it passed
    /// and, on a session branch, its commit was made; `not_fixed` says why not otherwise. Those
    /// in `high` are marked high. What became of them, in one line.
    pub(crate) async fn record_fixed_by_check(
        &self,
        conversation_id: &ConversationId,
        fixes: &[String],
        high: &[String],
        not_fixed: Option<&str>,
        command: &str,
        alias: &str,
    ) -> String {
        let ids = fixes.join(", ");
        if let Some(why) = not_fixed {
            return format!("Not recorded fixed: {ids}, as {why}.");
        }
        let items = fixes
            .iter()
            .map(|id| crate::tools::FindingRecord {
                id: id.clone(),
                status: crate::tools::FindingVerdict::Fixed,
                task: None,
                checked: Some(format!(
                    "`{command}` passed after the fix [evidence: {alias}]"
                )),
                why: None,
                // Marked high, its fix gets one targeted re-check (SESSION-FIXES-PLAN.md Q6.1).
                high: high.contains(id).then_some(true),
            })
            .collect();
        match self
            .record_findings(conversation_id, crate::tools::RecordFindings { items })
            .await
        {
            Ok(reply) => reply,
            Err(err) => format!("Findings not recorded: {err}"),
        }
    }

    /// `record_findings`: the thread's word on what became of review findings. Every id must
    /// be one of the conversation's; nothing is stored when one isn't. Brigadier stores and
    /// shows what the thread says, and judges none of it.
    pub(crate) async fn record_findings(
        &self,
        conversation_id: &ConversationId,
        args: crate::tools::RecordFindings,
    ) -> Result<String> {
        if args.items.is_empty() {
            return Err(Error::Invalid(
                "`items` is empty: list every finding of the review.".into(),
            ));
        }
        let _held = self.reviews.lock().await;
        let board = self.core.board(conversation_id).await?;
        let mut changed: Vec<ReviewRun> = Vec::new();
        let mut problems = Vec::new();
        let mut said = Vec::new();
        for record in &args.items {
            let id = record.id.trim().to_ascii_uppercase();
            let found = changed
                .iter()
                .position(|review| review.items.iter().any(|item| item.id == id));
            let index = match found {
                Some(index) => index,
                None => match board
                    .reviews
                    .values()
                    .find(|review| review.items.iter().any(|item| item.id == id))
                {
                    Some(review) => {
                        changed.push(review.clone());
                        changed.len() - 1
                    }
                    None => {
                        problems.push(format!("{id} is no finding of this session"));
                        continue;
                    }
                },
            };
            let status = match recorded_status(&id, record, &board) {
                Ok(status) => status,
                Err(problem) => {
                    problems.push(problem);
                    continue;
                }
            };
            let item = changed[index]
                .items
                .iter_mut()
                .find(|item| item.id == id)
                .expect("found above");
            said.push(format!("{id} {}", status_word(&status)));
            item.status = status;
            if let Some(high) = record.high {
                item.high = high;
            }
        }
        if !problems.is_empty() {
            return Err(Error::Invalid(format!(
                "Nothing was recorded: {}. Record them all again in one call.",
                problems.join("; ")
            )));
        }
        for review in &changed {
            self.store_review(review).await?;
        }
        drop(_held);
        // A review the thread left nothing open of has no fix to come back for.
        for review in &changed {
            if review.kind == ReviewKind::Code
                && review.items.iter().all(|item| {
                    !matches!(item.status, FindingStatus::New | FindingStatus::Open { .. })
                })
            {
                self.settle_review_batch(conversation_id, batch_tip(review))
                    .await;
            }
        }
        let unrecorded: Vec<String> = changed
            .iter()
            .flat_map(|review| &review.items)
            .filter(|item| item.status == FindingStatus::New)
            .map(|item| item.id.clone())
            .collect();
        let unverified: Vec<String> = changed
            .iter()
            .flat_map(|review| &review.items)
            .filter(|item| unverified(&item.status))
            .map(|item| item.id.clone())
            .collect();
        let mut reply = format!("Recorded: {}.", said.join(", "));
        if !unrecorded.is_empty() {
            reply.push_str(&format!(" Still not recorded: {}.", unrecorded.join(", ")));
        }
        if !unverified.is_empty() {
            reply.push_str(&format!(
                " Unverified: {}. The user sees them as unverified, not as clean: tell them so plainly, or check each in the merged code and record it again with `checked`.",
                unverified.join(", ")
            ));
        }
        // A merge held for these findings looks again.
        self.check_held_merge(conversation_id).await;
        Ok(reply)
    }

    /// Whether the tab merges its work on its own (Settings › Git, Q62).
    pub(crate) fn auto_merge_on(&self) -> bool {
        self.core.settings().auto_merge
    }

    /// [`Self::auto_merge_on`] for session `id`: never while an overnight run goes in it, whose
    /// verified work only the user merges.
    pub(crate) fn auto_merge_in(&self, id: &crate::model::ConversationId) -> bool {
        self.auto_merge_on() && self.overnight.active.get(id).is_none()
    }

    /// Whether the user has the work reviewed (Settings' "Review the work").
    pub(crate) fn reviews_on(&self) -> bool {
        self.core.settings().review_work
    }

    /// The user switched "Review the work" to `on`. Off: a worker waiting for its own review's
    /// findings goes on at once; a review still running may finish and send them, but nothing
    /// waits on it. Either way a worker that builds hears it at once while it works (its brief
    /// said otherwise), one between turns with its next message, and the thread in a note
    /// before its next turn.
    pub async fn reviews_switched(&self, on: bool) {
        let workers: Vec<_> = self.tasks_lock().values().cloned().collect();
        let mut tell = Vec::new();
        {
            // Ordered with a review starting and a worker starting to wait for one: each sees
            // the setting from before the switch, then this releases it, or from after it.
            let _switch = self.reviews.lock().await;
            if self.reviews_on() != on {
                // Switched again meanwhile: that call handles it.
                return;
            }
            for live in workers {
                let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
                    continue;
                };
                if task.kind != TaskKind::Implement || task.state.is_final() {
                    continue;
                }
                let waiting = task.blocked_reason.as_deref() == Some(WAITING_FOR_REVIEW);
                if !on && waiting {
                    self.set_task_blocked(&task.id, None).await;
                    tell.push((live, task, true));
                } else if live.busy().await || (!on && self.review_pending(&task).await) {
                    // At work, or between turns about to wait for its review: it was nudged
                    // to report instead, and hears why.
                    tell.push((live, task, false));
                }
            }
        }
        if !on {
            // A merge held for a review still running goes ahead now.
            let held: Vec<ConversationId> = self.convs_lock().keys().cloned().collect();
            for id in held {
                self.check_held_merge(&id).await;
            }
        }
        for (live, task, released) in tell {
            let text = match live.reviews_note(&task, on) {
                Some(note) => note.to_owned(),
                None if released => "Nothing waits on your review now: once your checks pass and your evidence is in, call submit_report.".to_owned(),
                None => continue,
            };
            let now = self
                .task_by_id(&task.conversation_id, &task.id)
                .await
                .unwrap_or(task);
            let rounds = now.rework_rounds;
            match self
                .tell_worker(&now.conversation_id, &now, text, "Brigadier")
                .await
            {
                // A note is no rework round.
                Ok(_) => {
                    let _ = self
                        .update_task(&now.conversation_id, &now.id, |t| {
                            t.rework_rounds = t.rework_rounds.min(rounds);
                        })
                        .await;
                }
                Err(err) => {
                    tracing::info!(task = %now.id, error = %err, "a worker took no note that reviews were switched");
                }
            }
        }
    }

    /// Ends `conversation_id`'s running reviews: it is closing.
    pub(super) fn stop_reviews(&self, conversation_id: &ConversationId) {
        for (owner, stop) in self
            .running_reviews
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
        {
            if owner == conversation_id {
                stop.cancel();
            }
        }
    }

    /// Whether `task` asked for a review of its own work that still runs.
    pub(crate) async fn review_pending(&self, task: &Task) -> bool {
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return false;
        };
        board.reviews.values().any(|review| {
            review.state == ReviewState::Running
                && review.notify
                    == ReviewFor::Worker {
                        task_id: task.id.clone(),
                    }
        })
    }

    /// After a restart: a review that was running was cut off. It is recorded failed, what
    /// it made is removed, and the orchestrator hears it.
    pub(super) async fn recover_reviews(&self, conversation_id: &ConversationId) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        // A clean review the thread's answer waited for, whose word was still on its way to
        // the thread when Brigadier stopped: it goes again, so the request gets its answer
        // instead of ending with none. (A request whose turn the stop cut off reads
        // interrupted by now, and isn't working.)
        if let Some(clean) = board.latest_request().and_then(|latest| {
            let newest = board
                .reviews
                .values()
                .filter(|review| {
                    review.kind == ReviewKind::Code
                        && review.task_id.is_none()
                        && review.request_id.as_deref() == Some(latest.id.as_str())
                })
                .max_by_key(|review| (review.started_at_ms, review.number))?;
            (latest.state == crate::work::RequestState::Working
                && newest.answer_waits
                && newest.state == ReviewState::Clean)
                .then(|| newest.clone())
        }) && !self
            .inbox_holds(
                conversation_id,
                InjectionKind::Report,
                super::conversation::CLEAN_REVIEW,
                None,
                clean.request_id.as_deref(),
            )
            .await
        {
            self.tell_review(&clean, None).await;
        }
        self.recover_unheard_reviews(conversation_id, &board).await;
        let running: Vec<ReviewRun> = board
            .reviews
            .values()
            .filter(|review| review.state == ReviewState::Running)
            .cloned()
            .collect();
        for mut review in running {
            let _ = self
                .runtime
                .ledger()
                .dispose(&review_owner(&review.id))
                .await;
            review.state = ReviewState::Failed {
                reason: "Brigadier restarted before it finished".into(),
            };
            review.ended_at_ms = Some(now_ms());
            if let Err(err) = self.store_review(&review).await {
                tracing::warn!(review = %review.id, error = %err, "could not record a cut-off review");
                continue;
            }
            // Its worker was stopped by the restart: the orchestrator hears it instead.
            review.notify = ReviewFor::Orchestrator;
            self.tell_review(&review, None).await;
            if review.kind == ReviewKind::Code {
                self.settle_review_batch(conversation_id, batch_tip(&review))
                    .await;
            }
        }
    }

    /// Findings, or a review that couldn't run, whose word to the thread was still in its
    /// inbox when Brigadier stopped (the inbox lives in memory, #69): each goes again, once,
    /// while its request still works for it and is the one the word is filed under (a task's
    /// own, else the newest). One a turn took is logged in the orchestrator's stream after
    /// the review ended, by its label (which names the review) and task.
    async fn recover_unheard_reviews(
        &self,
        conversation_id: &ConversationId,
        board: &crate::board::Board,
    ) {
        // As `request_for` files it: no turn runs at startup.
        let latest = board.latest_request().map(|request| request.id.clone());
        let request_of = |review: &ReviewRun| match review.task_id.as_ref() {
            Some(task) => board
                .tasks
                .get(task)
                .and_then(|task| task.request_id.clone())
                .or_else(|| latest.clone()),
            None => latest.clone(),
        };
        let mut owed: Vec<&ReviewRun> = board
            .reviews
            .values()
            .filter(|review| {
                matches!(
                    review.state,
                    ReviewState::Findings { .. } | ReviewState::Failed { .. }
                ) && review.notify == ReviewFor::Orchestrator
                    && review.ended_at_ms.is_some()
            })
            .filter(|review| {
                let filed = request_of(review);
                let own = review.request_id.is_none() || review.request_id == filed;
                own && filed.is_some_and(|request| {
                    board
                        .requests
                        .get(&request)
                        .is_some_and(|of| of.state == crate::work::RequestState::Working)
                        && !super::requests::ended_run_request(board, &request)
                })
            })
            .collect();
        let Some(since) = owed.iter().filter_map(|review| review.ended_at_ms).min() else {
            return;
        };
        let taken = self.reports_taken_since(conversation_id, since).await;
        owed.retain(|review| {
            let of = review
                .task_id
                .as_ref()
                .and_then(|id| board.tasks.get(id))
                .map_or_else(
                    || "the session".to_owned(),
                    |task| format!("task-{}", task.number),
                );
            // A report logged before labels named their review counts for any of its label.
            let (label, older) = (report_label(review, &of), report_base_label(review, &of));
            !taken.iter().any(|(at_ms, taken_label, task_id)| {
                Some(*at_ms) >= review.ended_at_ms
                    && (*taken_label == label || *taken_label == older)
                    && *task_id == review.task_id
            })
        });
        owed.sort_by_key(|review| (review.ended_at_ms, review.number));
        for review in owed {
            let of = review
                .task_id
                .as_ref()
                .and_then(|id| board.tasks.get(id))
                .map_or_else(
                    || "the session".to_owned(),
                    |task| format!("task-{}", task.number),
                );
            if self
                .inbox_holds(
                    conversation_id,
                    InjectionKind::Report,
                    &report_label(review, &of),
                    review.task_id.as_ref(),
                    request_of(review).as_deref(),
                )
                .await
            {
                continue;
            }
            let text = self.review_text(review).await;
            if matches!(review.state, ReviewState::Findings { .. }) && text.is_none() {
                continue;
            }
            tracing::info!(review = %review.id, "a review's word the restart lost goes to the thread again");
            // Filed under the request checked above, even if a turn started for another.
            self.tell_review_filed(review, text.as_deref(), request_of(review))
                .await;
        }
    }

    /// The reports a turn of the thread took since `since_ms`: when, their label and task.
    async fn reports_taken_since(
        &self,
        id: &ConversationId,
        since_ms: i64,
    ) -> Vec<(i64, String, Option<TaskId>)> {
        let mut taken = Vec::new();
        let mut before = None;
        loop {
            let Ok(page) = self
                .core
                .store()
                .read_stream(
                    crate::model::streams::orchestrator(id),
                    brigadier_store::StreamPage {
                        before,
                        kinds: vec!["orchestrator.logged".into()],
                        limit: 500,
                    },
                )
                .await
            else {
                break;
            };
            let Some(last) = page.last() else {
                break;
            };
            before = Some(last.stream_seq);
            let mut older = false;
            for stored in &page {
                older |= stored.at_ms < since_ms;
                if stored.at_ms >= since_ms
                    && let Ok(DomainEvent::OrchestratorLogged {
                        entry: crate::work::OrchestratorEntry::Injection { injection },
                        ..
                    }) = serde_json::from_str::<DomainEvent>(stored.payload.get())
                    && injection.kind == InjectionKind::Report
                {
                    taken.push((stored.at_ms, injection.label, injection.task_id));
                }
            }
            if older {
                break;
            }
        }
        taken
    }

    /// A review's full text, from its blob.
    async fn review_text(&self, review: &ReviewRun) -> Option<String> {
        let hash = review.findings.as_ref()?.parse().ok()?;
        let bytes = self.core.store().blobs().get(hash).await.ok()??;
        String::from_utf8(bytes).ok()
    }

    async fn store_review(&self, review: &ReviewRun) -> Result<()> {
        self.core
            .record_conversation(
                &review.conversation_id,
                vec![DomainEvent::ReviewUpdated {
                    review: review.clone(),
                }],
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reviewer_is_always_the_other_vendor() {
        assert_eq!(other_vendor(ProviderKind::Claude), ProviderKind::Codex);
        assert_eq!(other_vendor(ProviderKind::Codex), ProviderKind::Claude);
    }

    fn review(id: &str, kind: ReviewKind, base: &str, tip: &str) -> ReviewRun {
        ReviewRun {
            id: id.into(),
            conversation_id: ConversationId("c1".into()),
            request_id: None,
            task_id: None,
            kind,
            base: base.into(),
            tip: tip.into(),
            author: ProviderKind::Claude,
            reviewer: ProviderKind::Codex,
            reviewer_model: Some("gpt-6.1-sol".into()),
            notify: ReviewFor::Orchestrator,
            state: ReviewState::Running,
            started_at_ms: 0,
            ended_at_ms: None,
            findings: None,
            number: 0,
            items: Vec::new(),
            landing: None,
            answer_waits: false,
        }
    }

    /// Q7: a review with three findings stores three items, numbered by the review; a text
    /// that names a priority but can't be split is one item that points at the review.
    #[test]
    fn a_reviews_findings_are_stored_as_items() {
        let text = "- [P1] Guard the empty list — src/a.rs:10\n  Fix.\n- [P2] Old events fail — src/b.rs:3\n- [P3] Typo — README.md:4";
        let items = items_of(3, text, None);
        let ids: Vec<_> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, ["R3.1", "R3.2", "R3.3"]);
        assert_eq!(items[1].severity.as_deref(), Some("P2"));
        assert_eq!(items[1].title, "Old events fail");
        assert_eq!(items[1].location.as_deref(), Some("src/b.rs:3"));
        assert!(items.iter().all(|item| item.status == FindingStatus::New));
        assert!(items_of(1, "No findings.", None).is_empty());
        assert!(items_of(1, "Looks right to me.", None).is_empty());
        let unsplit = items_of(2, "Two problems, see [P1] and [P2] in the summary.", None);
        assert_eq!(unsplit.len(), 1);
        assert_eq!(unsplit[0].title, "See the review");
        assert_eq!(unsplit[0].severity, None);
        assert_eq!(next_number([&review("a", ReviewKind::Code, "a", "b")]), 1);
        // A path in the review's own checkout reads as the project's (#413).
        let checkout =
            Path::new("/Users/me/Library/Application Support/x/worktrees/p/review-72d3a50237ec");
        let codex = "- [P3] Preserve years below 100 — /Users/me/Library/Application Support/x/worktrees/p/review-72d3a50237ec/src/validate.js:19-19\n- [P2] Span — /elsewhere/a.rs:3-5";
        let items = items_of(1, codex, Some(checkout));
        assert_eq!(items[0].title, "Preserve years below 100");
        assert_eq!(items[0].location.as_deref(), Some("src/validate.js:19"));
        assert_eq!(items[1].location.as_deref(), Some("/elsewhere/a.rs:3-5"));
    }

    /// Q7: the merge's result counts only what the thread recorded for the reviews since the
    /// previous merge; nothing sums across two merges.
    #[test]
    fn a_merges_result_lists_only_its_own_open_findings() {
        use crate::work::{OrchestratorStep, OrchestratorStepKind};
        let mut board = crate::board::Board::default();
        let mut first = review("first", ReviewKind::Code, "a", "b");
        first.number = 1;
        first.started_at_ms = 10;
        first.state = ReviewState::Findings { count: 3 };
        first.items = items_of(
            1,
            "- [P1] One — a:1\n- [P2] Two — a:2\n- [P3] Three — a:3",
            None,
        );
        first.items[0].status = FindingStatus::Open { why: None };
        first.items[1].status = FindingStatus::Open {
            why: Some("needs the user's call".into()),
        };
        first.items[2].status = FindingStatus::Fixed {
            task: Some("task-2".into()),
            why: None,
            checked: Some("a:3 at the tip guards it now".into()),
        };
        board.reviews.insert(first.id.clone(), first);
        let merged = super::super::landing::merged_findings(&board, false);
        assert_eq!(
            merged,
            " Open review findings, as you recorded them (not fixed): R1.1 One; R1.2 Two (needs the user's call). Tell the user, as a list."
        );
        // The first merge, then a second piece of work whose review found two the thread
        // hasn't recorded yet: only those two count, never 2 + 3.
        board.orchestrator_steps.push(OrchestratorStep {
            request_id: None,
            kind: OrchestratorStepKind::Merged {
                branch: "b".into(),
                base: "main".into(),
                commits: 1,
                asked_in: None,
                upstream: None,
                head: None,
                merge_commit: None,
                pr: None,
                auto: false,
                requests: Vec::new(),
                record: None,
            },
            at_ms: 20,
            position: 0,
        });
        let mut second = review("second", ReviewKind::Code, "b", "c");
        second.number = 2;
        second.started_at_ms = 30;
        second.state = ReviewState::Findings { count: 2 };
        second.items = items_of(2, "- [P2] Four — a:4\n- [P2] Five — a:5", None);
        board.reviews.insert(second.id.clone(), second);
        let merged = super::super::landing::merged_findings(&board, false);
        assert_eq!(
            merged,
            " Findings you haven't recorded: R2.1, R2.2. Record them with record_findings, and tell the user which stay open."
        );
        // Recorded fixed on a report's word: unverified, never "all fixed".
        let second = board.reviews.get_mut("second").unwrap();
        second.items[0].status = FindingStatus::Fixed {
            task: None,
            why: None,
            checked: None,
        };
        second.items[1].status = FindingStatus::Dismissed {
            why: "by design".into(),
        };
        assert_eq!(
            super::super::landing::merged_findings(&board, false),
            " Recorded fixed but never checked in the merged code (unverified): R2.1 Four. Tell the user plainly that these are unverified, never that the review is clean; or check each in the merged code and record it again with `checked`."
        );
        // Checked in the merged code: nothing open.
        let second = board.reviews.get_mut("second").unwrap();
        second.items[0].status = FindingStatus::Fixed {
            task: None,
            why: None,
            checked: Some("ran its test at the tip: passes".into()),
        };
        assert_eq!(
            super::super::landing::merged_findings(&board, false),
            " Its review's findings are all fixed or dismissed, as you recorded them."
        );
        // A review stored before findings were items is never reported clean.
        let mut legacy = review("legacy", ReviewKind::Code, "c", "d");
        legacy.started_at_ms = 40;
        legacy.state = ReviewState::Findings { count: 2 };
        board.reviews.insert(legacy.id.clone(), legacy.clone());
        assert_eq!(
            super::super::landing::merged_findings(&board, false),
            " Its review's findings are all fixed or dismissed, as you recorded them. A review from before findings were recorded found 2 things, never recorded: tell the user they may still be open."
        );
        board.reviews.retain(|id, _| id == "legacy");
        assert_eq!(
            super::super::landing::merged_findings(&board, false),
            " A review from before findings were recorded found 2 things, never recorded: tell the user they may still be open."
        );
    }

    #[test]
    fn a_range_is_reviewed_once() {
        let reviews = [
            review("plan", ReviewKind::Plan, "a1", "b2"),
            review("code", ReviewKind::Code, "a1", "b2"),
            review("other", ReviewKind::Code, "a1", "c3"),
        ];
        // A landing of the range a worker's review covered finds that review.
        assert_eq!(
            review_of(&reviews, "a1", "b2").map(|review| review.id.as_str()),
            Some("code")
        );
        assert_eq!(
            review_of(&reviews, "a1", "c3").map(|review| review.id.as_str()),
            Some("other")
        );
        // The same tip from another base is another range; an outline's review is no
        // review of code.
        assert!(review_of(&reviews, "b2", "c3").is_none());
        assert!(review_of(&reviews[..1], "a1", "b2").is_none());
    }

    #[test]
    fn a_review_that_could_not_run_leaves_its_range_to_be_reviewed() {
        let mut failed = review("failed", ReviewKind::Code, "a1", "b2");
        failed.state = ReviewState::Failed {
            reason: "no Codex model can review it now".into(),
        };
        assert!(review_of([&failed], "a1", "b2").is_none());
        let again = review("again", ReviewKind::Code, "a1", "b2");
        assert_eq!(
            review_of([&failed, &again], "a1", "b2").map(|review| review.id.as_str()),
            Some("again")
        );
    }

    #[test]
    fn a_review_counts_its_tokens_under_its_own_owner() {
        assert_eq!(review_owner("01a1"), "review:01a1");
        assert_eq!(short("0123456789abcdef"), "0123456789");
        assert_eq!(short("abc"), "abc");
    }

    #[test]
    fn a_long_review_is_clipped_at_a_character_boundary() {
        let long = "é".repeat(MESSAGE_MAX);
        let cut = clipped(&long);
        assert!(cut.len() < long.len());
        assert!(cut.ends_with("[… the rest of the review is cut]"));
        assert_eq!(clipped("short"), "short");
    }

    fn record(value: serde_json::Value) -> crate::tools::FindingRecord {
        serde_json::from_value(value).expect("a record")
    }

    /// A fix credited to a task counts only once that task has landed, and a fix with no
    /// check in the merged code is stored as unverified.
    #[test]
    fn a_fix_is_recorded_unverified_without_a_check_and_refused_before_its_task_lands() {
        let mut task: crate::work::Task = serde_json::from_value(serde_json::json!({
            "id": "t9",
            "conversationId": "c1",
            "number": 9,
            "position": 0,
            "title": "Fix the loader",
            "kind": "implement",
            "spec": "Fix it.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        let mut board = crate::board::Board::default();
        board.tasks.insert(task.id.clone(), task.clone());
        let fixed_in_9 =
            record(serde_json::json!({"id": "R1.1", "status": "fixed", "task": "task-9"}));
        let refused = recorded_status("R1.1", &fixed_in_9, &board).unwrap_err();
        assert!(refused.contains("task-9 hasn't landed"), "{refused}");
        task.landed = Some("abc123".into());
        board.tasks.insert(task.id.clone(), task);
        let status = recorded_status("R1.1", &fixed_in_9, &board).unwrap();
        assert_eq!(
            status,
            FindingStatus::Fixed {
                task: Some("task-9".into()),
                why: None,
                checked: None
            }
        );
        assert!(unverified(&status));
        assert_eq!(
            status_word(&status),
            "fixed in task-9 (unverified: not checked in the merged code)"
        );
        let checked = record(serde_json::json!({
            "id": "R1.1", "status": "fixed", "task": "Task 9",
            "checked": " loader.rs:12 at the tip reads the old format ",
        }));
        let status = recorded_status("R1.1", &checked, &board).unwrap();
        assert_eq!(
            status,
            FindingStatus::Fixed {
                task: Some("task-9".into()),
                why: None,
                checked: Some("loader.rs:12 at the tip reads the old format".into())
            }
        );
        assert!(!unverified(&status));
        assert_eq!(status_word(&status), "fixed in task-9");
        // A blank check is no check.
        let blank = record(serde_json::json!({"id": "R1.1", "status": "fixed", "checked": "  "}));
        assert!(unverified(
            &recorded_status("R1.1", &blank, &board).unwrap()
        ));
        let unknown =
            record(serde_json::json!({"id": "R1.1", "status": "fixed", "task": "task-4"}));
        assert!(
            recorded_status("R1.1", &unknown, &board)
                .unwrap_err()
                .contains("task-4 is no task")
        );
    }

    /// A fix stored before checks were recorded reads as unverified, never as clean.
    #[test]
    fn an_older_stored_fix_reads_as_unverified() {
        let status: FindingStatus =
            serde_json::from_value(serde_json::json!({"type": "fixed", "task": null, "why": null}))
                .expect("an older fix");
        assert_eq!(
            status,
            FindingStatus::Fixed {
                task: None,
                why: None,
                checked: None
            }
        );
        assert!(unverified(&status));
    }

    /// A review whose tip has the candidate's files covers it only with what comes before its
    /// base covered too: a landing's review of its own commits leaves the thread's earlier
    /// commit to review (phase 3 verify).
    #[test]
    fn a_review_of_the_same_files_covers_the_candidate_only_from_where_it_started() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-covered-upto-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let git_in = |args: &[&str]| {
            let out = crate::test_git_command()
                .args(["-c", "user.name=T", "-c", "user.email=t@example.com"])
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {out:?}");
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        let commit = |file: &str| {
            std::fs::write(dir.join(file), file).unwrap();
            git_in(&["add", "-A"]);
            git_in(&["commit", "-q", "-m", file]);
            Oid(git_in(&["rev-parse", "HEAD"]))
        };
        git_in(&["init", "-q", "-b", "main"]);
        let base = commit("base.txt");
        let own = commit("own.txt");
        let landed = commit("landed.txt");
        let repo = brigadier_git::Git::new("git".into(), std::env::vars_os().collect())
            .open(&dir)
            .unwrap();
        // The landing's review: own..landed, the thread's own commit left out.
        let mut landing = review("landing", ReviewKind::Code, &own.0, &landed.0);
        landing.number = 1;
        let (from, last) = covered_upto(&repo, std::slice::from_ref(&landing), &base, &landed);
        assert_eq!(from, base);
        assert!(last.is_none());
        // With the thread's own commit reviewed as well, the whole range is.
        let mut earlier = review("own", ReviewKind::Code, &base.0, &own.0);
        earlier.number = 2;
        let (from, last) = covered_upto(&repo, &[earlier, landing.clone()], &base, &landed);
        assert_eq!(from, landed);
        assert_eq!(last.map(|review| review.id), Some("landing".to_owned()));
        // A review from the base itself covers it alone.
        let mut whole = review("whole", ReviewKind::Code, &base.0, &landed.0);
        whole.number = 3;
        let (from, _) = covered_upto(&repo, &[whole], &base, &landed);
        assert_eq!(from, landed);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
