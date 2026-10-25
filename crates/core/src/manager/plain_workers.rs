//! Plain CLI workers (STRIP-DOWN-PLAN S13): what `brigadier start`, `send`, `wait` and `close`
//! do. A worker is a plain Claude Code or Codex session of its own, in a worktree of its own by
//! default. Brigadier starts it, passes it messages, tells its session what its CLI did, and
//! closes it; it routes, briefs, checks and lands nothing for it.
//!
//! - It belongs to the session or Chat that started it (G19, S28): it shows in that session's
//!   Workers row, rail and worker cards, never in the sidebar, and only that session may
//!   message, wait on or close it.
//! - A Chat's worker works in the repository `--repo` names (a worktree of it, or its checkout
//!   with `--local`), or without one in a folder of its own with no git.
//! - Its worktree starts from the session's current branch unless `--base` names another
//!   (G21), recorded in its folder's `record/workspace.json` and as the branch's
//!   `gh-merge-base`. It merges back with plain git, as the session decides.
//! - It runs at its session's permission level unless `--permission` says otherwise (Q8). A
//!   permission prompt of its CLI opens the usual card under Waiting on you, and reaches the
//!   session as a `needs-input` event; `brigadier send <id> --allow|--deny` answers it through
//!   the CLI's own permission protocol.
//! - What its parent hears (S29, O1): a turn's end with its last message, a permission prompt,
//!   a question or a sign-in, a failed or stopped turn, its CLI ending, a usage limit, its CLI
//!   compacting its context, and 20 minutes without a word while it works. Each event is kept
//!   until heard, once: through a `brigadier wait` the parent keeps running in the background,
//!   or the safety net's message once its turn ended ([`super::worker_events`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use brigadier_cli::models::Guidance;
use brigadier_cli::protocol::{
    self as cli, Answer, Capability, CliReply, CloseOutput, ErrorCode, EventKind, SendOutput,
    StartOutput, StartRequest, WaitEvent, WaitOutput,
};
use brigadier_git::WorktreeSpec;
use brigadier_providers::{ApprovalDecision, ProviderKind, TurnStatus};

use super::cli::{CliCaller, Target, short, worker_row};
use super::models::same_work;
use super::session_dir::{WorkspaceRecord, write_atomic};
use super::worker_events::HeldEvents;
use super::workers::{TaskLive, Workspace, task_branch};
use super::{SessionManager, blocking, git_error};
use crate::model::{
    Conversation, ConversationId, Environment, ModelChoice, PermissionLevel, Setup,
};
use crate::work::{
    Approval, ApprovalSubject, AttachmentRef, AttemptEnd, CardId, CardState, MockupPart,
    PlainWorker, Task, TaskId, TaskState, WorkerStepKind,
};
use crate::{Error, Result, now_ms};

/// How much of a prompt a worker's title takes when `--title` gives none.
const TITLE_MAX: usize = 60;

/// How long a message sent to a worker still starting waits for its CLI.
const CLI_START: Duration = Duration::from_secs(60);
/// What a worker's turn a restart cut off is told as it goes on by itself (D9 a).
const RESUMED_AFTER_RESTART: &str = "[Brigadier restarted while you were working on this, and \
your turn goes on now. Your last steps may have finished after your record of them: check git \
status and git log first, then carry on from where it stands.]";

/// A new event of worker `task`'s (its own id given when it is kept).
pub(super) fn event(task: &TaskId, kind: EventKind) -> WaitEvent {
    WaitEvent {
        id: short(&task.0),
        event_id: String::new(),
        event: kind,
        message: None,
        asks: None,
        resets_at_ms: None,
        at_ms: now_ms(),
        git: None,
        commands: None,
    }
}

fn permission_level(permission: cli::Permission) -> PermissionLevel {
    match permission {
        cli::Permission::Ask => PermissionLevel::Ask,
        cli::Permission::AcceptEdits => PermissionLevel::AcceptEdits,
        cli::Permission::FullAccess => PermissionLevel::Full,
    }
}

fn permission_word(level: PermissionLevel) -> cli::Permission {
    match level {
        PermissionLevel::Ask => cli::Permission::Ask,
        PermissionLevel::AcceptEdits => cli::Permission::AcceptEdits,
        PermissionLevel::Full => cli::Permission::FullAccess,
    }
}

/// A title from the prompt's first line.
fn title_of(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Worker");
    if line.chars().count() <= TITLE_MAX {
        return line.to_owned();
    }
    let cut: String = line.chars().take(TITLE_MAX).collect();
    match cut.rfind(' ') {
        Some(at) if at > TITLE_MAX / 2 => format!("{}…", &cut[..at]),
        _ => format!("{cut}…"),
    }
}

fn refused(message: impl Into<String>) -> CliReply {
    CliReply::error(ErrorCode::Refused, message)
}

/// The kind of work an independent review is (`models_guidance.md`).
const REVIEW_WORK: &str = "Independent review";
/// `--reviewing session`: the session's own work, such as the job's plan.
const SESSION_AUTHOR: &str = "session";

/// Whose work a review covers ([`SessionManager::review_authors`]).
#[derive(Default)]
struct ReviewAuthors {
    /// Their shown ids.
    ids: Vec<String>,
    /// How the reviewer's brief names each.
    names: Vec<String>,
    /// The providers their work ran on.
    providers: Vec<ProviderKind>,
}

impl ReviewAuthors {
    /// The line a reviewer's brief ends with: where its authors' facts are, to pull
    /// (harness D13, D22, P1).
    fn brief_line(&self) -> String {
        format!(
            "You review the work of {}. Before you start, run `brigadier show <id>` for each \
             ({}: its branch, commits and the commands it ran) and `brigadier context` (the \
             decisions and open questions).",
            self.names.join(" and "),
            self.ids.join(", ")
        )
    }
}

fn provider_of(agent: cli::Agent) -> ProviderKind {
    match agent {
        cli::Agent::Claude => ProviderKind::Claude,
        cli::Agent::Codex => ProviderKind::Codex,
    }
}

fn agent_of(provider: ProviderKind) -> cli::Agent {
    match provider {
        ProviderKind::Claude => cli::Agent::Claude,
        ProviderKind::Codex => cli::Agent::Codex,
    }
}

impl SessionManager {
    /// `brigadier start`: a new plain worker of the calling session.
    pub(super) async fn cli_start(
        &self,
        caller: &CliCaller,
        start: StartRequest,
    ) -> Result<CliReply> {
        if caller.task_id.is_some() {
            return Ok(refused("refused: a worker can't start workers of its own"));
        }
        let conversation = self.core.conversation(&caller.conversation_id)?;
        // A session's workers work in its repository; a Chat's in the one `--repo` names, or
        // in a folder of their own (S28).
        let (own, place) = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => {
                if start.repo.is_some() {
                    return Ok(refused(
                        "refused: --repo is for a Chat's worker; a session's workers work in \
                         its own repository",
                    ));
                }
                (orchestrator, None)
            }
            Some(Setup::Chat { model, .. }) if conversation.side_of.is_none() => {
                let place = match &start.repo {
                    Some(repo) => match self.chat_repo(repo).await {
                        Ok(top) => Some(top),
                        Err(message) => return Ok(CliReply::error(ErrorCode::NotFound, message)),
                    },
                    None if start.local => {
                        return Ok(refused(
                            "refused: --local works in a repository's checkout; give --repo",
                        ));
                    }
                    None if start.base.is_some() => {
                        return Ok(refused("refused: --base needs a repository; give --repo"));
                    }
                    None => None,
                };
                (model, place)
            }
            _ => {
                return Ok(refused(
                    "refused: a side chat starts no workers; its session does",
                ));
            }
        };
        // harness D22: an independent review knows whose work it covers (`--reviewing`, the
        // session's own by default) and runs on another ready provider than theirs.
        let reviewing = start.reviewing.clone();
        let work = start
            .work
            .clone()
            .or_else(|| (!reviewing.is_empty()).then(|| REVIEW_WORK.to_owned()));
        let review = if !reviewing.is_empty()
            || work
                .as_deref()
                .is_some_and(|work| same_work(work, REVIEW_WORK))
        {
            match self
                .review_authors(caller, &conversation, own.provider, &reviewing)
                .await
            {
                Ok(authors) => Some(authors),
                Err(reply) => return Ok(reply),
            }
        } else {
            None
        };
        // S32: `--work` names a kind of work of the rules file, whose rows are an order: the
        // first the user's rules let start is the row. The flags given win field by field, the
        // row fills in the rest, and only then do the user's rules judge the whole choice. A
        // review's rows are each provider's model, not an order: a row on another provider
        // than the authors' comes first, and its model must be free now (harness D22).
        let row = match work.as_deref() {
            None => None,
            Some(work) => {
                let rows = self.work_rows(work)?;
                let mut order: Vec<&Guidance> = rows.iter().collect();
                if let Some(authors) = &review {
                    order.retain(|row| start.agent.is_none_or(|agent| agent == row.agent));
                    order.sort_by_key(|row| authors.providers.contains(&provider_of(row.agent)));
                }
                let mut fit = None;
                for row in order {
                    let provider = provider_of(row.agent);
                    let model =
                        (row.model != "default").then(|| self.listed_model(provider, &row.model));
                    if self
                        .model_refusal(
                            provider,
                            model.as_deref(),
                            row.effort.as_deref(),
                            Some(work),
                        )?
                        .is_some()
                    {
                        continue;
                    }
                    let free = review.is_none()
                        || self
                            .choice_available(&ModelChoice {
                                provider,
                                model,
                                effort: row.effort.clone(),
                                fast: None,
                                account: None,
                            })
                            .await;
                    if free {
                        fit = Some(row.clone());
                        break;
                    }
                }
                match fit.or_else(|| rows.into_iter().next()) {
                    Some(row) => Some(row),
                    None => {
                        return Ok(CliReply::error(
                            ErrorCode::NotFound,
                            format!(
                                "no row for the work \"{work}\"; `brigadier models` lists each \
                                 kind of work"
                            ),
                        ));
                    }
                }
            }
        };
        let mut picked = Vec::new();
        let agent = start.agent.or_else(|| {
            let row = row.as_ref()?;
            picked.push("agent".to_owned());
            Some(row.agent)
        });
        let provider = match agent {
            Some(cli::Agent::Claude) => ProviderKind::Claude,
            Some(cli::Agent::Codex) => ProviderKind::Codex,
            None => own.provider,
        };
        let chat = matches!(conversation.setup, Some(Setup::Chat { .. }));
        let model = match (start.model, &row) {
            (Some(model), _) => Some(model),
            // A row's model is its agent's: another agent keeps its own default.
            (None, Some(row)) if agent == Some(row.agent) => {
                picked.push("model".to_owned());
                (row.model != "default").then(|| self.listed_model(provider, &row.model))
            }
            (None, _) => None,
        };
        let effort = start.effort.or_else(|| {
            let effort = row.as_ref()?.effort.clone()?;
            picked.push("effort".to_owned());
            Some(effort)
        });
        if let Some(rule) = self.model_refusal(
            provider,
            model.as_deref(),
            effort.as_deref(),
            work.as_deref(),
        )? {
            return Ok(refused(rule));
        }
        // Q8: the session's own level unless `--permission` says.
        let permission = match start.permission {
            Some(permission) => permission_level(permission),
            None => self.permission(&caller.conversation_id),
        };
        let base = match (start.local, chat, &place) {
            (true, ..) | (false, true, None) => None,
            (false, ..) => match self
                .plain_base(&conversation, place.as_deref(), start.base.as_deref())
                .await
            {
                Ok(base) => Some(base),
                Err(message) => return Ok(CliReply::error(ErrorCode::NotFound, message)),
            },
        };
        // D18: the servers it is granted, checked now, each kept by the name its CLI knows.
        let folder = place
            .as_deref()
            .map(std::path::PathBuf::from)
            .or_else(|| self.trusted_repo(&caller.conversation_id))
            .unwrap_or_else(|| {
                self.session_dir(&caller.conversation_id)
                    .root()
                    .to_path_buf()
            });
        let mcps = match self
            .worker_grants_named(&caller.conversation_id, provider, &folder, &start.mcps)
            .await
        {
            Ok(mcps) => mcps,
            Err(message) => return Ok(CliReply::error(ErrorCode::NotFound, message)),
        };
        let title = start.title.unwrap_or_else(|| title_of(&start.prompt));
        let prompt = match &review {
            Some(authors) => format!("{}\n\n{}", start.prompt, authors.brief_line()),
            None => start.prompt,
        };
        let choice = ModelChoice {
            provider,
            model,
            effort,
            fast: None,
            account: None,
        };
        let plain = PlainWorker {
            permission,
            local: start.local,
            repo: place,
            base: base.clone(),
            mcps,
            computer_use: start.with.contains(&Capability::ComputerUse),
            closed: false,
            auto_resumed: false,
        };
        let mockup = start.mockup.then_some(MockupPart::Builds);
        let task = crate::heap(|| {
            self.create_plain_task(
                &caller.conversation_id,
                title,
                prompt,
                choice,
                plain,
                mockup,
            )
        })
        .await?;
        self.orchestrator_step(
            &caller.conversation_id,
            crate::work::OrchestratorStepKind::Created {
                task_id: task.id.clone(),
            },
        )
        .await;
        Ok(CliReply::ok(&StartOutput {
            worker: worker_row(&task, &Default::default()),
            permission: permission_word(permission),
            base,
            row: row.map(|row| cli::StartRow {
                work: row.work,
                source: row.source,
                picked,
            }),
            review: review.map(|authors| cli::StartReview {
                other_provider: !authors.providers.contains(&provider),
                of: authors.ids,
                authors: authors.providers.into_iter().map(agent_of).collect(),
            }),
        }))
    }

    /// Whose work an independent review covers: each `--reviewing` id, a worker of this
    /// session or `session` (this session itself, also when none is given), with the
    /// provider it ran on (harness D22).
    async fn review_authors(
        &self,
        caller: &CliCaller,
        conversation: &Conversation,
        own: ProviderKind,
        ids: &[String],
    ) -> std::result::Result<ReviewAuthors, CliReply> {
        let mut authors = ReviewAuthors::default();
        let session = |authors: &mut ReviewAuthors| {
            authors.ids.push(short(&conversation.id.0));
            authors
                .names
                .push(format!("this session {}", short(&conversation.id.0)));
            authors.providers.push(own);
        };
        if ids.is_empty() {
            session(&mut authors);
        }
        for id in ids {
            if id.eq_ignore_ascii_case(SESSION_AUTHOR) {
                session(&mut authors);
                continue;
            }
            match self.cli_target(caller, id).await? {
                Target::Worker(task) if task.conversation_id == caller.conversation_id => {
                    authors.ids.push(short(&task.id.0));
                    authors
                        .names
                        .push(format!("worker {} {}", task.number, short(&task.id.0)));
                    authors.providers.push(task.route.choice.provider);
                }
                Target::Session(other) if other.id == caller.conversation_id => {
                    session(&mut authors);
                }
                _ => {
                    return Err(refused(format!(
                        "refused: --reviewing takes this session's own workers, or `session`; \
                         {id} is neither"
                    )));
                }
            }
        }
        let mut seen = Vec::new();
        authors.providers.retain(|provider| {
            let new = !seen.contains(provider);
            seen.push(*provider);
            new
        });
        Ok(authors)
    }

    /// The top folder of the repository `path` is in, for a Chat's worker (`--repo`).
    async fn chat_repo(&self, path: &str) -> std::result::Result<String, String> {
        let (git, path) = (self.git.clone(), path.to_owned());
        blocking(move || {
            let repo = git.open(std::path::Path::new(&path)).map_err(|_| {
                Error::NotFound(format!(
                    "--repo {path}: not the top folder of a git repository"
                ))
            })?;
            Ok(repo.root().display().to_string())
        })
        .await
        .map_err(|err| match err {
            Error::NotFound(message) => message,
            other => other.to_string(),
        })
    }

    /// The branch a worker's worktree starts from: `given`, or the branch the checkout has out
    /// now (G21): `conversation`'s, or a Chat's `repo`. Refused when there is no such branch.
    async fn plain_base(
        &self,
        conversation: &Conversation,
        repo: Option<&str>,
        given: Option<&str>,
    ) -> std::result::Result<String, String> {
        let checkout = match repo {
            Some(repo) => repo.to_owned(),
            None => self
                .checkout_dir(&conversation.id)
                .map_err(|err| err.to_string())?,
        };
        let recorded = match &conversation.setup {
            Some(Setup::Session {
                environment:
                    Environment::LocalCheckout { branch } | Environment::NewWorktree { branch, .. },
                ..
            }) => Some(branch.clone()),
            _ => None,
        };
        let (git, given) = (self.git.clone(), given.map(str::to_owned));
        blocking(move || {
            let repo = git
                .open(std::path::Path::new(&checkout))
                .map_err(git_error)?;
            let branch = match given {
                Some(branch) => branch,
                None => match repo.current_branch().map_err(git_error)?.or(recorded) {
                    Some(branch) => branch,
                    None => {
                        return Err(Error::Invalid(
                            "the checkout has no branch out (a detached HEAD): give --base".into(),
                        ));
                    }
                },
            };
            match repo.branch_tip(&branch).map_err(git_error)? {
                Some(_) => Ok(branch),
                None => Err(Error::NotFound(format!(
                    "no branch {branch:?} in this repository"
                ))),
            }
        })
        .await
        .map_err(|err| match err {
            Error::NotFound(message) | Error::Invalid(message) => message,
            other => other.to_string(),
        })
    }

    /// A plain worker's workspace: a new worktree on its own branch from its base, or its
    /// session's own checkout with `--local` (a Chat's worker: `--repo`'s checkout), or with no
    /// repository a folder of its own (S28).
    pub(super) async fn plain_workspace(
        &self,
        owner: &str,
        task: &Task,
        plain: &PlainWorker,
        conversation: &Conversation,
        repo: Option<PathBuf>,
        scratch: PathBuf,
    ) -> Result<Workspace> {
        let worker_dir = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("work");
        let Some(repo) = repo else {
            self.prepare_owned_dir(owner, &worker_dir, true).await?;
            return Ok(Workspace {
                repo: None,
                worktree: Some(worker_dir),
                branch: None,
                base: None,
                on_snapshot: false,
                target: None,
                scratch,
                warmed: Vec::new(),
                merge: None,
            });
        };
        let empty = |worktree: Option<PathBuf>| Workspace {
            repo: Some(repo.clone()),
            worktree,
            branch: None,
            base: None,
            on_snapshot: false,
            target: None,
            scratch: scratch.clone(),
            warmed: Vec::new(),
            merge: None,
        };
        let Some(base) = plain.base.clone().filter(|_| !plain.local) else {
            let checkout = match &plain.repo {
                Some(repo) => PathBuf::from(repo),
                None => PathBuf::from(self.checkout_dir(&conversation.id)?),
            };
            return Ok(empty(Some(checkout)));
        };
        let (git, repo_path, branch) = (self.git.clone(), repo.clone(), base.clone());
        let start = blocking(move || {
            git.open(&repo_path)
                .map_err(git_error)?
                .branch_tip(&branch)
                .map_err(git_error)?
                .ok_or_else(|| Error::Invalid(format!("branch {branch} does not exist")))
        })
        .await?;
        let branch = task_branch(
            &self.branch_prefix(&task.conversation_id).await,
            &task.conversation_id,
            task.number,
            &task.title,
        );
        let worktree = worker_dir;
        let warmed = self
            .make_worktree(
                owner,
                &repo,
                &worktree,
                WorktreeSpec::NewBranch {
                    name: branch.clone(),
                    start: start.clone(),
                },
                true,
                &task.id,
            )
            .await?;
        self.note_branch_made(&repo, &branch).await;
        // What `gh pr create` and the session read as the branch it merges into.
        let (git, repo_path, name, onto) =
            (self.git.clone(), repo.clone(), branch.clone(), base.clone());
        blocking(move || {
            git.open(&repo_path)
                .map_err(git_error)?
                .set_merge_base(&name, &onto)
                .map_err(git_error)
        })
        .await?;
        Ok(Workspace {
            worktree: Some(worktree),
            branch: Some(branch),
            base: Some(start),
            target: Some(base),
            warmed,
            ..empty(None)
        })
    }

    /// Writes a plain worker's `record/workspace.json` in its folder, as a session's (S7).
    pub(super) async fn record_plain_workspace(&self, task: &Task) {
        let Some(plain) = &task.plain else {
            return;
        };
        let setup = self
            .core
            .conversation(&task.conversation_id)
            .ok()
            .and_then(|c| c.setup);
        let record = match (&task.workspace, plain.local, &setup) {
            // A Chat's worker without a repository (S28).
            (_, _, Some(Setup::Chat { .. })) if plain.repo.is_none() => WorkspaceRecord {
                environment: "folder",
                branch: String::new(),
                base: None,
            },
            (Some(workspace), false, _) => WorkspaceRecord {
                environment: "newWorktree",
                branch: workspace.branch.clone().unwrap_or_default(),
                base: plain.base.clone(),
            },
            _ => {
                let branch = match (&setup, &plain.repo) {
                    (_, Some(repo)) => {
                        let (git, repo) = (self.git.clone(), PathBuf::from(repo));
                        blocking(move || {
                            Ok(git
                                .open(&repo)
                                .ok()
                                .and_then(|repo| repo.current_branch().ok().flatten()))
                        })
                        .await
                        .ok()
                        .flatten()
                        .unwrap_or_default()
                    }
                    (
                        Some(Setup::Session {
                            environment:
                                Environment::LocalCheckout { branch }
                                | Environment::NewWorktree { branch, .. },
                            ..
                        }),
                        None,
                    ) => branch.clone(),
                    _ => String::new(),
                };
                WorkspaceRecord {
                    environment: "localCheckout",
                    branch,
                    base: None,
                }
            }
        };
        let dir = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("record");
        let written = blocking(move || {
            std::fs::create_dir_all(&dir).map_err(|err| Error::Invalid(err.to_string()))?;
            let text = serde_json::to_string_pretty(&record)
                .map_err(|err| Error::Invalid(err.to_string()))?;
            write_atomic(&dir.join("workspace.json"), text.as_bytes())
        })
        .await;
        if let Err(err) = written {
            tracing::warn!(task = %task.id, error = %err, "could not record the worker's workspace");
        }
    }

    /// A plain worker's turn ended: it waits for its session's next message, and its session's
    /// `brigadier wait` hears it, with its last message, or the usage limit that ended it.
    pub(super) async fn plain_turn_ended(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        status: TurnStatus,
    ) {
        let cutoff = live.take_cutoff().await;
        let message = live.last_message().await;
        let (state, blocked, kind) = match (&cutoff, status) {
            (Some(AttemptEnd::Limit { .. }), _) => (
                TaskState::Paused,
                Some("Its model hit a usage limit.".to_owned()),
                EventKind::UsageLimit,
            ),
            (None, TurnStatus::Completed) => (TaskState::Reported, None, EventKind::TurnEnded),
            (_, TurnStatus::Interrupted) => (TaskState::Reported, None, EventKind::Interrupted),
            _ => (TaskState::Reported, None, EventKind::Failed),
        };
        // D44/D47: Reported means it can take another message, not that this turn succeeded.
        let step = match kind {
            EventKind::Failed => Some(WorkerStepKind::Failed),
            EventKind::Interrupted => Some(WorkerStepKind::Stopped),
            _ => None,
        };
        let error = match &cutoff {
            Some(AttemptEnd::Error { message, .. }) => Some(message.clone()),
            _ => None,
        };
        // A mockup's images show in its session's Mockup tab as each turn ends.
        let outputs = match task.mockup {
            Some(_) => self.final_outputs(task).await,
            None => None,
        };
        // The turn finished, one way or another: a later cut-off may go on by itself again. An
        // interrupted one (a quit closing its CLI) is still the turn that was cut off (D9 a).
        let finished = status != TurnStatus::Interrupted;
        let updated = self
            .update_task_with_step(&task.conversation_id, &task.id, step, |task| {
                task.state = state;
                task.blocked_reason = blocked.clone();
                if finished && let Some(plain) = &mut task.plain {
                    plain.auto_resumed = false;
                }
                if error.is_some() {
                    task.error = error.clone();
                }
                if let Some(outputs) = outputs {
                    task.outputs = outputs;
                }
            })
            .await;
        if let Err(err) = updated {
            tracing::warn!(task = %task.id, error = %err, "could not record a worker's turn end");
        }
        let mut event = event(&task.id, kind);
        match &cutoff {
            Some(AttemptEnd::Limit { limit }) => {
                event.message = Some(match &limit.window {
                    Some(window) => format!("its {window} usage limit"),
                    None => "a usage limit".to_owned(),
                });
                event.resets_at_ms = limit.resets_at_ms;
            }
            _ => event.message = error.or(message),
        }
        // What git and its CLI showed as the turn ended, beside its own words (harness P1).
        self.with_facts(task, &mut event).await;
        self.worker_event(&task.conversation_id, &task.id, None, event);
    }

    /// A plain worker's CLI asks permission: the card under Waiting on you is open, and its
    /// session hears it.
    pub(super) fn plain_needs_input(&self, live: &TaskLive, card: &Approval, what: &str) {
        self.plain_asks(&live.conversation_id, &live.id, Some(card.id.clone()), what);
    }

    /// A plain worker asks something of the user (`card`: its question's card), or one of its
    /// MCP servers wants a sign-in: its session hears it.
    pub(super) fn plain_asks(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        card: Option<CardId>,
        what: &str,
    ) {
        let mut event = event(task, EventKind::NeedsInput);
        event.asks = Some(what.to_owned());
        self.worker_event(conversation, task, card, event);
    }

    /// A plain worker's CLI compacted its context.
    pub(super) fn plain_fresh_context(&self, live: &TaskLive) {
        self.worker_event(
            &live.conversation_id,
            &live.id,
            None,
            event(&live.id, EventKind::FreshContext),
        );
    }

    /// A plain worker's turn a restart cut off goes on by itself, once (D9 a): its CLI session
    /// resumed with a word to check git status and git log first, or, cut off before its
    /// session began, started again on its prompt in its own worktree. False when it went on
    /// by itself before (that turn cut off twice): it waits for its session's message.
    /// Should it not go on, its session hears so, as of any cut-off worker.
    pub(super) async fn plain_auto_resume(&self, task: &Task) -> bool {
        if task.plain.as_ref().is_none_or(|plain| plain.auto_resumed) {
            return false;
        }
        let marked = self
            .update_task(&task.conversation_id, &task.id, |task| {
                if let Some(plain) = &mut task.plain {
                    plain.auto_resumed = true;
                }
            })
            .await;
        let task = match marked {
            Ok(task) => task,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not mark a worker going on by itself");
                return false;
            }
        };
        let manager = self.arc();
        self.spawn(async move {
            let live = manager.task_live(&task);
            let began = manager.last_worker_native_id(&task).await.is_some();
            let went = if began {
                manager
                    .send_to_worker(&task.conversation_id, &task, RESUMED_AFTER_RESTART.into())
                    .await
            } else {
                manager.relaunch_worker(&live, &task).await
            };
            let Err(err) = went else {
                return;
            };
            tracing::info!(task = %task.id, error = %err, "a cut-off worker waits for a message");
            if began {
                let _ = manager
                    .update_task(&task.conversation_id, &task.id, |t| {
                        if !t.state.is_final() {
                            t.state = TaskState::Reported;
                            t.blocked_reason = None;
                        }
                    })
                    .await;
                manager.plain_cut_off(&task).await;
            } else {
                manager
                    .plain_failed(
                        &task,
                        format!(
                            "Brigadier quit as it started, and it could not start again: {err}."
                        ),
                    )
                    .await;
            }
        });
        true
    }

    /// A plain worker's CLI was working when Brigadier quit: its session hears it stopped,
    /// with what git and its CLI showed of the cut-off turn (harness P1).
    pub(super) async fn plain_cut_off(&self, task: &Task) {
        let mut event = event(&task.id, EventKind::Interrupted);
        event.message =
            Some("Brigadier restarted while it worked. Send it a message to carry on.".into());
        self.with_facts(task, &mut event).await;
        self.worker_event(&task.conversation_id, &task.id, None, event);
    }

    /// A plain worker's CLI ended on its own: the worker is gone, and its session hears why.
    /// Its unfinished work is kept on its branch, as a stop keeps it.
    pub(super) async fn plain_gone(&self, task: &Task, exit: &str) {
        self.plain_failed(task, format!("Its CLI ended: {exit}."))
            .await;
    }

    /// A plain worker failed (`reason`): it is gone, and its session hears why. Its unfinished
    /// work is kept on its branch, as a stop keeps it.
    pub(super) async fn plain_failed(&self, task: &Task, reason: String) {
        let _ = self
            .update_task(&task.conversation_id, &task.id, |task| {
                task.error = Some(reason.clone());
            })
            .await;
        if let Some(live) = self.existing_task_live(&task.id) {
            live.close_cli().await;
        }
        // Heard before the worker turns final: a `brigadier wait` that sees it final with
        // nothing queued ends empty and would miss it.
        let mut event = event(&task.id, EventKind::Gone);
        event.message = Some(reason);
        self.worker_event(&task.conversation_id, &task.id, None, event);
        self.dispose_task(task, TaskState::Failed).await;
    }

    /// `text` to a plain worker, as `brigadier send` sends it: one sent right after `brigadier
    /// start` reaches its CLI once that is up. True when it went into a running turn.
    async fn send_plain(&self, task: &Task, text: String) -> Result<bool> {
        let live = self.task_live(task);
        if task.state == TaskState::Starting {
            let up = tokio::time::Instant::now() + CLI_START;
            while live.cli().await.is_none() && tokio::time::Instant::now() < up {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let queued = live.busy().await;
        self.message_worker(&task.conversation_id, task, text)
            .await?;
        Ok(queued)
    }

    /// The user's notes on a mockup, from its session's Mockup tab: `text` and the marked
    /// screenshots (`attachments`, written in its scratch folder and named by path) go to the
    /// mockup's worker as `brigadier send` sends, and its mockup counts a new version.
    pub async fn send_mockup_notes(
        &self,
        task_id: TaskId,
        text: String,
        attachments: Vec<AttachmentRef>,
    ) -> Result<()> {
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        if task.mockup != Some(MockupPart::Builds) {
            return Err(Error::Invalid(format!(
                "worker {} builds no mockup",
                task.number
            )));
        }
        let Some(workspace) = &task.workspace else {
            return Err(Error::Invalid(format!(
                "worker {} hasn't started yet",
                task.number
            )));
        };
        let files = self
            .attachment_files(&attachments, &PathBuf::from(&workspace.scratch))
            .await;
        if files.len() < attachments.len() {
            return Err(Error::Invalid(
                "the marked screenshots could not be saved".into(),
            ));
        }
        let mut text = text;
        if !files.is_empty() {
            text.push_str("\n\nThe marked screenshots:");
            for file in &files {
                text.push_str(&format!("\n- {}", file.path.display()));
            }
        }
        self.send_plain(&task, text).await?;
        self.update_task(&conversation_id, &task_id, |task| task.rework_rounds += 1)
            .await?;
        Ok(())
    }

    /// One of the caller's own workers, by an id `brigadier workers` showed; another
    /// session's worker, or a session, is refused.
    pub(super) async fn own_worker(
        &self,
        caller: &CliCaller,
        id: &str,
    ) -> std::result::Result<Task, CliReply> {
        if caller.task_id.is_some() {
            return Err(refused("refused: a worker has no workers of its own"));
        }
        match self.cli_target(caller, id).await? {
            Target::Worker(task) if task.conversation_id == caller.conversation_id => {
                if task.plain.as_ref().is_some_and(|plain| plain.closed) {
                    return Err(CliReply::error(
                        ErrorCode::NotFound,
                        format!("worker {} was closed", short(&task.id.0)),
                    ));
                }
                Ok(*task)
            }
            Target::Worker(task) => Err(refused(format!(
                "refused: {} is another session's worker; send, wait and close act only on \
                 this session's own workers",
                short(&task.id.0)
            ))),
            Target::Session(session) => Err(refused(format!(
                "refused: {} is a session, not a worker; close acts only on this session's own \
                 workers",
                short(&session.id.0)
            ))),
        }
    }

    /// `brigadier send`: a message to one of the caller's workers, queued into its turn when
    /// it works; or the answer to its CLI's open permission prompt; or a message to another
    /// session, as that session's (S30).
    pub(super) async fn cli_send(
        &self,
        caller: &CliCaller,
        id: &str,
        text: Option<String>,
        answer: Option<Answer>,
    ) -> Result<CliReply> {
        if caller.task_id.is_none()
            && let Ok(Target::Session(to)) = self.cli_target(caller, id).await
        {
            return Ok(self
                .message_another_session(caller, &to, text, answer)
                .await);
        }
        let task = match self.own_worker(caller, id).await {
            Ok(task) => task,
            Err(reply) => return Ok(reply),
        };
        let shown = short(&task.id.0);
        if let Some(answer) = answer {
            let board = self.core.board(&task.conversation_id).await?;
            let Some(card) = board
                .approvals
                .values()
                .filter(|card| {
                    card.task_id.as_ref() == Some(&task.id)
                        && card.state == CardState::Pending
                        && matches!(card.subject, ApprovalSubject::Cli { .. })
                })
                .min_by_key(|card| card.created_at_ms)
            else {
                return Ok(CliReply::error(
                    ErrorCode::NotFound,
                    format!("worker {shown} has no open permission prompt"),
                ));
            };
            let decision = match answer {
                Answer::Allow => ApprovalDecision::Allow,
                Answer::Deny => ApprovalDecision::Deny {
                    message: String::new(),
                },
            };
            self.answer_card(task.conversation_id.clone(), card.id.clone(), decision)
                .await?;
            return Ok(CliReply::ok(&SendOutput {
                id: shown,
                queued: false,
                answered: Some(answer),
                message: None,
            }));
        }
        let text = text.unwrap_or_default();
        let step = super::brains::cut(text.trim(), crate::work::STEP_TEXT_MAX);
        let queued = self.send_plain(&task, text).await?;
        // "Messaged ‹worker›" in its session's thread, opening to what it said.
        self.orchestrator_step(
            &caller.conversation_id,
            crate::work::OrchestratorStepKind::Messaged {
                task_id: task.id.clone(),
                text: Some(step),
            },
        )
        .await;
        Ok(CliReply::ok(&SendOutput {
            id: shown,
            queued,
            answered: None,
            message: None,
        }))
    }

    /// `brigadier send <session> <text>`: the text reaches session `to` as the caller's.
    async fn message_another_session(
        &self,
        caller: &CliCaller,
        to: &Conversation,
        text: Option<String>,
        answer: Option<Answer>,
    ) -> CliReply {
        let shown = short(&to.id.0);
        if to.id == caller.conversation_id {
            return refused(format!("refused: {shown} is this session"));
        }
        if answer.is_some() {
            return refused(format!(
                "refused: {shown} is a session; --allow and --deny answer a worker's prompt"
            ));
        }
        let from = match self.core.conversation(&caller.conversation_id) {
            Ok(from) => from,
            Err(err) => return CliReply::error(ErrorCode::Failed, err.to_string()),
        };
        let (message, queued) = self
            .message_session(&from, to, &text.unwrap_or_default())
            .await;
        CliReply::ok(&SendOutput {
            id: shown,
            queued,
            answered: None,
            message: Some(message),
        })
    }

    /// `brigadier wait`: the caller's workers' next events, as soon as there is one, or none
    /// when the time is up or no worker waited for can send one any more. The events are held
    /// for it until its `brigadier` printed them ([`HeldEvents`]).
    pub(super) async fn cli_wait(
        &self,
        caller: &CliCaller,
        ids: &[String],
        timeout_secs: Option<u32>,
    ) -> Result<(CliReply, Option<HeldEvents>)> {
        if caller.task_id.is_some() {
            return Ok((
                refused("refused: a worker has no workers to wait for"),
                None,
            ));
        }
        let mut sessions = Vec::new();
        for id in ids {
            if let Ok(Target::Session(session)) = self.cli_target(caller, id).await {
                if session.id == caller.conversation_id {
                    return Ok((
                        refused(format!("refused: {} is this session", short(&session.id.0))),
                        None,
                    ));
                }
                sessions.push(*session);
            }
        }
        if !sessions.is_empty() {
            if sessions.len() < ids.len() {
                return Ok((
                    refused("refused: wait on workers or on sessions, not both"),
                    None,
                ));
            }
            let out = self
                .wait_for_sessions(caller, &sessions, timeout_secs)
                .await;
            return Ok((CliReply::ok(&out), None));
        }
        let mut tasks = HashSet::new();
        for id in ids {
            match self.own_worker(caller, id).await {
                Ok(task) => {
                    tasks.insert(task.id);
                }
                Err(reply) => return Ok((reply, None)),
            }
        }
        let parent = &caller.conversation_id;
        let deadline = timeout_secs
            .map(|secs| tokio::time::Instant::now() + Duration::from_secs(u64::from(secs)));
        let holder = self.worker_events.wait_holder();
        let waiter = self.events_waiter(parent);
        let ended = loop {
            let added = self.worker_events.added.notified();
            tokio::pin!(added);
            added.as_mut().enable();
            // Held and answered with no await in between: a wait cut off takes nothing.
            let events = self.hold_for_wait(parent, &tasks, &holder).await?;
            if !events.is_empty() {
                return Ok((
                    CliReply::ok(&WaitOutput {
                        events,
                        timed_out: false,
                    }),
                    Some(self.held_events(parent, holder)),
                ));
            }
            let board = self.core.board(parent).await?;
            let can_send = board.tasks.values().any(|task| {
                task.plain.as_ref().is_some_and(|plain| !plain.closed)
                    && !task.state.is_final()
                    && (tasks.is_empty() || tasks.contains(&task.id))
            });
            if !can_send {
                break false;
            }
            match deadline {
                Some(deadline) => {
                    tokio::select! {
                        () = added => {}
                        () = tokio::time::sleep_until(deadline) => break true,
                    }
                }
                None => added.await,
            }
        };
        drop(waiter);
        Ok((
            CliReply::ok(&WaitOutput {
                events: Vec::new(),
                timed_out: ended,
            }),
            None,
        ))
    }

    /// `brigadier close`: stops one of the caller's workers for good. Unfinished changes are
    /// kept as a commit on its branch, as a stop keeps them; it leaves `brigadier workers`.
    pub(super) async fn cli_close(&self, caller: &CliCaller, id: &str) -> Result<CliReply> {
        let task = match self.own_worker(caller, id).await {
            Ok(task) => task,
            Err(reply) => return Ok(reply),
        };
        // A worker closed after a turn that ended done is done, not stopped: its row reads so
        // after a restart too, when no CLI event of its is held any more.
        let done = self.worker_turn_done(&task.id);
        let closed = self
            .stop_worker(
                &task.conversation_id,
                &task,
                "Closed by its session.".to_owned(),
            )
            .await?;
        if task.plain.is_some() {
            self.update_task(&task.conversation_id, &task.id, |task| {
                if let Some(plain) = &mut task.plain {
                    plain.closed = true;
                }
                if done && task.state == TaskState::Stopped {
                    task.state = TaskState::Done;
                }
            })
            .await?;
            self.forget_worker_events(&task.conversation_id, &task.id);
        }
        Ok(CliReply::ok(&CloseOutput {
            id: short(&task.id.0),
            closed,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_comes_from_the_prompts_first_line() {
        assert_eq!(
            title_of("\n  Fix the login page\nand more"),
            "Fix the login page"
        );
        let long = "Write a long report on every module of the code base and what each one does";
        let title = title_of(long);
        assert!(
            title.ends_with('…') && title.chars().count() <= TITLE_MAX + 1,
            "{title}"
        );
        assert_eq!(title_of("   "), "Worker");
    }
}
