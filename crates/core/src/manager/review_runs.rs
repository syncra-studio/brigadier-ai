//! One-shot reviews (THREAD-PLAN.md §2 Q12): every landing gets one read-only review by the
//! vendor other than its author's; a worker may ask for one of its own work (`review_code`);
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

use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_git::{Oid, WorktreeSpec};
use brigadier_providers::{
    Access, AllowedModels, ApprovalDecision, Artifact, Origin, ProviderEvent, ProviderKind, Role,
    SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_review::{CodexRun, Review, Subject};
use brigadier_router::{Author, Decision, Needs, Pin, QualityTier, TaskCategory};

use super::conversation::Envelope;
use super::usage::TokenOwner;
use super::{SessionManager, blocking, git_error};
use crate::model::{ConversationId, DomainEvent};
use crate::routing::TokenMeter;
use crate::work::{
    InjectionKind, OrchestratorStepKind, ReviewFor, ReviewKind, ReviewRun, ReviewState, Task,
    TaskId, TaskKind, TaskState,
};
use crate::{Error, Result, now_ms};

/// A one-shot review's time box.
const REVIEW_TIME: Duration = Duration::from_secs(30 * 60);
/// The most of a review's text a message carries; all of it stays in the review's blob.
const MESSAGE_MAX: usize = 12_000;
/// What a worker waiting for its own review's findings is blocked on.
pub(crate) const WAITING_FOR_REVIEW: &str = "Waiting for its code review";

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
            })
            .await;
        match started {
            Ok((review, true)) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "a landing could not be reviewed");
            }
        }
    }

    /// The review of commits the thread made itself, `base..tip`, by the vendor other than
    /// the thread's. None when that range has one already.
    pub(crate) async fn review_thread_commits(
        &self,
        conversation_id: &ConversationId,
        base: Oid,
        tip: Oid,
        repo: PathBuf,
    ) {
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
            })
            .await;
        match started {
            Ok((review, true)) if matches!(review.state, ReviewState::Failed { .. }) => {
                self.tell_review(&review, None).await;
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(conversation = %conversation_id, error = %err, "the thread's commits could not be reviewed");
            }
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
            })
            .await;
        match started {
            Ok((review, true)) if matches!(review.state, ReviewState::Failed { .. }) => {
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
        let (review, _) = self
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
            })
            .await?;
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

    /// `review_code`: one review of the caller's committed work, from where it started, by
    /// the vendor other than its phase's author. Returns at once; the findings arrive as a
    /// message.
    pub(crate) async fn review_code(
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
        if !matches!(task.state, TaskState::Starting | TaskState::Running) {
            return Err(Error::Invalid(format!(
                "task-{} is {:?}: ask for the review while you work, before your report",
                task.number, task.state
            )));
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
        let (review, started) = self
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
            })
            .await?;
        let reviewer = review.reviewer.label();
        Ok(match (&review.state, started) {
            (ReviewState::Running, true) => format!(
                "Started a review of your committed work by {reviewer}. It returns now: gather your evidence and run your checks meanwhile. Its findings arrive as a message from Brigadier; if you finish everything else first, end your turn and they start your next one. Fix what you agree with and commit, then report."
            ),
            (ReviewState::Running, false) if review.notify == ReviewFor::Worker { task_id: task.id.clone() } => {
                "This exact work is being reviewed already; the findings arrive as a message."
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
                    "This exact work was reviewed already; its findings went to the orchestrator."
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
    /// One whose reviewer can't be routed is recorded failed, not started.
    async fn start_review(&self, new: NewReview) -> Result<(ReviewRun, bool)> {
        // A closing conversation starts nothing more.
        drop(self.enter(&new.conversation_id)?);
        let _held = self.reviews.lock().await;
        if new.kind == ReviewKind::Code {
            let board = self.core.board(&new.conversation_id).await?;
            if let Some(known) = review_of(board.reviews.values(), &new.base.0, &new.tip.0) {
                let mut known = known.clone();
                if known.notify != new.notify
                    && let ReviewFor::Worker { task_id } = &known.notify
                    && !self.still_works(&known.conversation_id, task_id).await
                {
                    known.notify = new.notify.clone();
                    self.store_review(&known).await?;
                }
                return Ok((known, false));
            }
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
        };
        let effort = match self.reviewer_model(&new, reviewer).await {
            Ok((model, effort)) => {
                review.reviewer_model = Some(model);
                effort
            }
            Err(why) => {
                review.state = ReviewState::Failed { reason: why };
                review.ended_at_ms = Some(now_ms());
                self.store_review(&review).await?;
                return Ok((review, true));
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
                    .run_review(&started, &new.repo, new.plan.as_ref(), effort, stop)
                    .await
            };
            manager
                .running_reviews
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .remove(&started.id);
            manager.finish_review(started, outcome).await;
        });
        Ok((review, true))
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
                needs: Needs::default(),
                pin: Some(Pin {
                    provider: Some(reviewer),
                    model: None,
                    effort: Some(brigadier_review::EFFORT.to_owned()),
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

    /// Runs `review` in a checkout of its own and removes everything it made after, also when
    /// `stop` ends it early (its conversation closed).
    async fn run_review(
        &self,
        review: &ReviewRun,
        repo: &Path,
        plan: Option<&(String, String)>,
        effort: Option<String>,
        stop: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Review, String> {
        let owner = review_owner(&review.id);
        let project = self
            .core
            .conversation(&review.conversation_id)
            .ok()
            .and_then(|conversation| conversation.project_id)
            .map_or_else(|| "none".to_owned(), |id| id.0);
        let name = format!(
            "review-{}",
            &review.id[review.id.len().saturating_sub(12)..]
        );
        let checkout = self.owned_dir("worktrees", &project).join(&name);
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
                self.review_in(review, &owner, &checkout, &scratch, plan, effort, stop)
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
        effort: Option<String>,
        stop: tokio_util::sync::CancellationToken,
    ) -> std::result::Result<Review, String> {
        let subject = match plan {
            Some((brief, outline)) => Subject::Plan { brief, outline },
            None => Subject::Code { base: &review.base },
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
            access: Access::ReadOnly,
            append_system_prompt: Some(brigadier_review::REVIEW_ROLE.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::Review,
            add_dirs: Vec::new(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: true,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: true,
            // It hands nothing on: no sub-agents.
            allowed_models: Some(AllowedModels::default()),
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
        };
        let counted_as = TaskId(review_owner(&review.id));
        let meter = TokenMeter::default();
        meter.turn_started(now_ms());
        let Started {
            session,
            mut events,
        } = self
            .runtime
            .start_hosted(owner, review.reviewer, spec)
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
                        self.runtime.note_quota_snapshot(quota).await;
                    }
                    ProviderEvent::Usage {
                        total,
                        last: latest,
                    } => {
                        self.note_tokens(
                            &meter,
                            review.reviewer,
                            review.reviewer_model.as_deref(),
                            TokenOwner::Task(&review.conversation_id, &counted_as),
                            &total,
                            latest.as_ref(),
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
                ended.state = match found.findings {
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
    }

    /// Hands a review's outcome to whoever asked: the worker while it still works, else the
    /// orchestrator. A clean review of a landing or an outline is no news for the orchestrator:
    /// the context card shows it, and finish_session's answer says it.
    pub(super) async fn tell_review(&self, review: &ReviewRun, text: Option<&str>) {
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
                    "[review of your work · {reviewer} found {count}]\n{}\n[/review]\nFix each finding you agree with and commit; for one you don't, say why in your report. There are no review rounds: once your fixes are committed, your evidence is in and the checks they touch pass, report at once; don't verify again.",
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
        let number = match &review.task_id {
            Some(task_id) => self
                .task_by_id(&review.conversation_id, task_id)
                .await
                .ok()
                .map(|task| task.number),
            None => None,
        };
        let of = number.map_or_else(|| "the session".to_owned(), |n| format!("task-{n}"));
        let range = format!("{}..{}", short(&review.base), short(&review.tip));
        let text = match (review.kind, &review.state, text) {
            // The thread's own commits and plans.
            (ReviewKind::Code, ReviewState::Findings { count }, Some(text))
                if review.task_id.is_none() =>
            {
                format!(
                    "[review of your commits · {reviewer} found {count} in {range}]\n{}\n[/review]\nThis is the one background review of the commits you made yourself. Fix what you agree with (a tiny fix of your own, or delegate one), or tell the user why not. There are no review rounds.",
                    clipped(text)
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
                "[review {of} · {reviewer} found {count} in {range}]\n{}\n[/review]\nThis is the one background review of what landed. Fix what you agree with (delegate a fix, or a tiny fix of your own), or tell the user why not. There are no review rounds.",
                clipped(text)
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
            // Clean, or still running: nothing to say.
            _ => return,
        };
        let request = self
            .request_for(&review.conversation_id, review.task_id.as_ref())
            .await;
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
                label: match (review.kind, review.task_id.is_some()) {
                    (ReviewKind::Code, true) => format!("review {of}"),
                    (ReviewKind::Plan, true) => format!("plan review {of}"),
                    (ReviewKind::Code, false) => "review of your commits".to_owned(),
                    (ReviewKind::Plan, false) => "plan review of your plan".to_owned(),
                },
                task_id: review.task_id.clone(),
                text,
            },
            request,
        )
        .await;
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
        }
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
        }
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
}
