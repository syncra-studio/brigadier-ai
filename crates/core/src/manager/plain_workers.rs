//! Plain CLI workers (STRIP-DOWN-PLAN S13): what `brigadier start`, `send`, `wait` and `close`
//! do. A worker is a plain Claude Code or Codex session of its own, in a worktree of its own by
//! default. Brigadier starts it, passes it messages, tells its session what its CLI did, and
//! closes it; it routes, briefs, checks and lands nothing for it.
//!
//! - It belongs to the session that started it (G19): it shows in that session's Workers row,
//!   rail and worker cards, never in the sidebar, and only that session may message, wait on
//!   or close it.
//! - Its worktree starts from the session's current branch unless `--base` names another
//!   (G21), recorded in its folder's `record/workspace.json` and as the branch's
//!   `gh-merge-base`. It merges back with plain git, as the session decides.
//! - It runs at its session's permission level unless `--permission` says otherwise (Q8). A
//!   permission prompt of its CLI opens the usual card under Waiting on you, and reaches the
//!   session as a `needs-input` event; `brigadier send <id> --allow|--deny` answers it through
//!   the CLI's own permission protocol.
//! - What `brigadier wait` hears: a turn's end with its last message, a permission prompt, its
//!   CLI ending, a usage limit, and its CLI compacting its context. Each event is handed out
//!   once; they live in memory, so a restart forgets the ones nobody took.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_cli::protocol::{
    self as cli, Answer, CliReply, CloseOutput, ErrorCode, EventKind, SendOutput, StartOutput,
    StartRequest, TurnEnd, WaitEvent, WaitOutput,
};
use brigadier_git::WorktreeSpec;
use brigadier_providers::{ApprovalDecision, ProviderKind, TurnStatus};

use super::cli::{CliCaller, Target, short, worker_row};
use super::session_dir::{WorkspaceRecord, write_atomic};
use super::workers::{TaskExtra, TaskLive, Workspace, task_branch};
use super::{SessionManager, blocking, git_error};
use crate::model::{Conversation, ConversationId, Environment, PermissionLevel, Setup};
use crate::work::{
    Approval, ApprovalSubject, AttemptEnd, CardId, CardState, PlainWorker, Task, TaskId, TaskKind,
    TaskState,
};
use crate::{Error, Result, now_ms};

/// How much of a prompt a worker's title takes when `--title` gives none.
const TITLE_MAX: usize = 60;

/// How long a message sent to a worker still starting waits for its CLI.
const CLI_START: Duration = Duration::from_secs(60);

/// What a plain worker's CLI did that its session's `brigadier wait` hasn't taken yet.
#[derive(Default)]
pub(crate) struct WorkerEvents {
    queue: Mutex<Vec<Pending>>,
    /// Wakes every waiting `brigadier wait`.
    added: tokio::sync::Notify,
}

struct Pending {
    conversation: ConversationId,
    task: TaskId,
    /// The card a `needs-input` event stands for: once it is answered, the event is stale.
    card: Option<CardId>,
    event: WaitEvent,
}

impl WorkerEvents {
    fn push(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        card: Option<CardId>,
        event: WaitEvent,
    ) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(Pending {
                conversation: conversation.clone(),
                task: task.clone(),
                card,
                event,
            });
        self.added.notify_waiters();
    }

    /// Takes `conversation`'s events of `tasks` (all of its workers' when empty), oldest first.
    fn take(&self, conversation: &ConversationId, tasks: &HashSet<TaskId>) -> Vec<Pending> {
        let mut queue = self
            .queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (taken, kept) = std::mem::take(&mut *queue)
            .into_iter()
            .partition(|pending| {
                pending.conversation == *conversation
                    && (tasks.is_empty() || tasks.contains(&pending.task))
            });
        *queue = kept;
        taken
    }

    /// Drops what a closed worker left untaken.
    fn forget(&self, task: &TaskId) {
        self.queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|pending| pending.task != *task);
    }
}

fn event(task: &TaskId, kind: EventKind) -> WaitEvent {
    WaitEvent {
        id: short(&task.0),
        event: kind,
        message: None,
        status: None,
        asks: None,
        resets_at_ms: None,
        at_ms: now_ms(),
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
        let Some(Setup::Session { orchestrator, .. }) = &conversation.setup else {
            return Ok(refused(
                "refused: only a session with a project starts workers; a Chat has no repository",
            ));
        };
        let provider = match start.agent {
            Some(cli::Agent::Claude) => ProviderKind::Claude,
            Some(cli::Agent::Codex) => ProviderKind::Codex,
            None => orchestrator.provider,
        };
        if let Some(rule) =
            self.model_refusal(provider, start.model.as_deref(), start.effort.as_deref())?
        {
            return Ok(refused(rule));
        }
        // Q8: the session's own level unless `--permission` says.
        let permission = match start.permission {
            Some(permission) => permission_level(permission),
            None => self.permission(&caller.conversation_id),
        };
        let base = match start.local {
            true => None,
            false => match self.plain_base(&conversation, start.base.as_deref()).await {
                Ok(base) => Some(base),
                Err(message) => return Ok(CliReply::error(ErrorCode::NotFound, message)),
            },
        };
        let title = start.title.unwrap_or_else(|| title_of(&start.prompt));
        let pin = brigadier_router::Pin {
            provider: Some(provider),
            model: start.model,
            effort: start.effort,
            by_user: false,
        };
        let plain = PlainWorker {
            permission,
            local: start.local,
            base: base.clone(),
            lean: start.lean,
            skills: start.skills,
            closed: false,
        };
        let task = crate::heap(|| {
            self.create_task_as(
                &caller.conversation_id,
                title,
                TaskKind::Implement,
                start.prompt,
                Some(pin),
                None,
                Vec::new(),
                None,
                None,
                Vec::new(),
                None,
                None,
                Vec::new(),
                TaskExtra {
                    plain: Some(plain),
                    ..Default::default()
                },
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
        }))
    }

    /// The branch a worker's worktree starts from: `given`, or the branch `conversation`'s
    /// checkout has out now (G21). Refused when there is no such branch.
    async fn plain_base(
        &self,
        conversation: &Conversation,
        given: Option<&str>,
    ) -> std::result::Result<String, String> {
        let checkout = self
            .checkout_dir(&conversation.id)
            .map_err(|err| err.to_string())?;
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
                            "this session's checkout has no branch out (a detached HEAD): \
                             give --base"
                                .into(),
                        ));
                    }
                },
            };
            match repo.branch_tip(&branch).map_err(git_error)? {
                Some(_) => Ok(branch),
                None => Err(Error::NotFound(format!(
                    "no branch {branch:?} in this project"
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
    /// session's own checkout with `--local`.
    pub(super) async fn plain_workspace(
        &self,
        owner: &str,
        task: &Task,
        plain: &PlainWorker,
        conversation: &Conversation,
        repo: PathBuf,
        scratch: PathBuf,
    ) -> Result<Workspace> {
        let empty = |worktree: Option<PathBuf>| Workspace {
            repo: repo.clone(),
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
            let checkout = PathBuf::from(self.checkout_dir(&conversation.id)?);
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
        let worktree = self
            .session_dir(&task.conversation_id)
            .worker(task)
            .join("work");
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
        let record = match (&task.workspace, plain.local) {
            (Some(workspace), false) => WorkspaceRecord {
                environment: "newWorktree",
                branch: workspace.branch.clone().unwrap_or_default(),
                base: plain.base.clone(),
            },
            _ => {
                let branch = match self
                    .core
                    .conversation(&task.conversation_id)
                    .map(|c| c.setup)
                {
                    Ok(Some(Setup::Session {
                        environment:
                            Environment::LocalCheckout { branch }
                            | Environment::NewWorktree { branch, .. },
                        ..
                    })) => branch,
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
        let (state, blocked, kind) = match &cutoff {
            Some(AttemptEnd::Limit { .. }) => (
                TaskState::Paused,
                Some("Its model hit a usage limit.".to_owned()),
                EventKind::UsageLimit,
            ),
            _ => (TaskState::Reported, None, EventKind::TurnEnded),
        };
        let error = match &cutoff {
            Some(AttemptEnd::Error { message, .. }) => Some(message.clone()),
            _ => None,
        };
        let updated = self
            .update_task(&task.conversation_id, &task.id, |task| {
                task.state = state;
                task.blocked_reason = blocked.clone();
                if error.is_some() {
                    task.error = error.clone();
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
            _ => {
                event.message = error.or(message);
                event.status = Some(match status {
                    TurnStatus::Completed if cutoff.is_none() => TurnEnd::Completed,
                    TurnStatus::Interrupted => TurnEnd::Interrupted,
                    _ => TurnEnd::Failed,
                });
            }
        }
        self.worker_events
            .push(&task.conversation_id, &task.id, None, event);
    }

    /// A plain worker's CLI asks permission: the card under Waiting on you is open, and its
    /// session hears it.
    pub(super) fn plain_needs_input(&self, live: &TaskLive, card: &Approval, what: &str) {
        let mut event = event(&live.id, EventKind::NeedsInput);
        event.asks = Some(what.to_owned());
        self.worker_events.push(
            &live.conversation_id,
            &live.id,
            Some(card.id.clone()),
            event,
        );
    }

    /// A plain worker's CLI compacted its context.
    pub(super) fn plain_fresh_context(&self, live: &TaskLive) {
        self.worker_events.push(
            &live.conversation_id,
            &live.id,
            None,
            event(&live.id, EventKind::FreshContext),
        );
    }

    /// A plain worker's CLI ended on its own: the worker is gone, and its session hears why.
    /// Its unfinished work is kept on its branch, as a stop keeps it.
    pub(super) async fn plain_gone(&self, task: &Task, exit: &str) {
        let reason = format!("Its CLI ended: {exit}.");
        let _ = self
            .update_task(&task.conversation_id, &task.id, |task| {
                task.error = Some(reason.clone());
            })
            .await;
        // Heard before the worker turns final: a `brigadier wait` that sees it final with
        // nothing queued ends empty and would miss it.
        let mut event = event(&task.id, EventKind::Gone);
        event.message = Some(reason);
        self.worker_events
            .push(&task.conversation_id, &task.id, None, event);
        self.dispose_task(task, TaskState::Failed).await;
    }

    /// One of the caller's own workers, by an id `brigadier workers` showed; another
    /// session's worker, or a session, is refused.
    async fn own_worker(
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
                "refused: {} is a session, not a worker; sessions don't message each other",
                short(&session.id.0)
            ))),
        }
    }

    /// `brigadier send`: a message to one of the caller's workers, queued into its turn when
    /// it works; or the answer to its CLI's open permission prompt.
    pub(super) async fn cli_send(
        &self,
        caller: &CliCaller,
        id: &str,
        text: Option<String>,
        answer: Option<Answer>,
    ) -> Result<CliReply> {
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
            }));
        }
        let text = text.unwrap_or_default();
        let live = self.task_live(&task);
        // Sent right after `brigadier start`: it reaches the CLI once that is up.
        if task.state == TaskState::Starting {
            let up = tokio::time::Instant::now() + CLI_START;
            while live.cli().await.is_none() && tokio::time::Instant::now() < up {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let queued = live.busy().await;
        self.message_worker(&task.conversation_id, &task, text, "its session")
            .await?;
        Ok(CliReply::ok(&SendOutput {
            id: shown,
            queued,
            answered: None,
        }))
    }

    /// `brigadier wait`: the caller's workers' next events, as soon as there is one, or none
    /// when the time is up or no worker waited for can send one any more.
    pub(super) async fn cli_wait(
        &self,
        caller: &CliCaller,
        ids: &[String],
        timeout_secs: Option<u32>,
    ) -> Result<CliReply> {
        if caller.task_id.is_some() {
            return Ok(refused("refused: a worker has no workers to wait for"));
        }
        let mut tasks = HashSet::new();
        for id in ids {
            match self.own_worker(caller, id).await {
                Ok(task) => {
                    tasks.insert(task.id);
                }
                Err(reply) => return Ok(reply),
            }
        }
        let deadline = timeout_secs
            .map(|secs| tokio::time::Instant::now() + Duration::from_secs(u64::from(secs)));
        loop {
            let added = self.worker_events.added.notified();
            tokio::pin!(added);
            added.as_mut().enable();
            let board = self.core.board(&caller.conversation_id).await?;
            let events: Vec<WaitEvent> = self
                .worker_events
                .take(&caller.conversation_id, &tasks)
                .into_iter()
                // A prompt answered meanwhile, on its card, asks nothing any more.
                .filter(|pending| {
                    pending.card.as_ref().is_none_or(|card| {
                        board
                            .approvals
                            .get(card)
                            .is_some_and(|card| card.state == CardState::Pending)
                    })
                })
                .map(|pending| pending.event)
                .collect();
            if !events.is_empty() {
                return Ok(CliReply::ok(&WaitOutput {
                    events,
                    timed_out: false,
                }));
            }
            let can_send = board.tasks.values().any(|task| {
                task.plain.as_ref().is_some_and(|plain| !plain.closed)
                    && !task.state.is_final()
                    && (tasks.is_empty() || tasks.contains(&task.id))
            });
            if !can_send {
                return Ok(CliReply::ok(&WaitOutput {
                    events: Vec::new(),
                    timed_out: false,
                }));
            }
            match deadline {
                Some(deadline) => {
                    tokio::select! {
                        () = added => {}
                        () = tokio::time::sleep_until(deadline) => {
                            return Ok(CliReply::ok(&WaitOutput {
                                events: Vec::new(),
                                timed_out: true,
                            }));
                        }
                    }
                }
                None => added.await,
            }
        }
    }

    /// `brigadier close`: stops one of the caller's workers for good. Unfinished changes are
    /// kept as a commit on its branch, as a stop keeps them; it leaves `brigadier workers`.
    pub(super) async fn cli_close(&self, caller: &CliCaller, id: &str) -> Result<CliReply> {
        let task = match self.own_worker(caller, id).await {
            Ok(task) => task,
            Err(reply) => return Ok(reply),
        };
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
            })
            .await?;
            self.worker_events.forget(&task.id);
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

    #[test]
    fn events_are_taken_once_and_only_by_their_session() {
        let events = WorkerEvents::default();
        let (mine, other) = (ConversationId("c1".into()), ConversationId("c2".into()));
        let (a, b) = (TaskId("t-aaaaaaaa".into()), TaskId("t-bbbbbbbb".into()));
        events.push(&mine, &a, None, event(&a, EventKind::TurnEnded));
        events.push(&mine, &b, None, event(&b, EventKind::NeedsInput));
        events.push(&other, &b, None, event(&b, EventKind::Gone));
        let only_a: HashSet<TaskId> = [a.clone()].into();
        assert_eq!(events.take(&mine, &only_a).len(), 1);
        assert!(events.take(&mine, &only_a).is_empty(), "taken once");
        let all = events.take(&mine, &HashSet::new());
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].event.event, EventKind::NeedsInput);
        events.forget(&b);
        assert!(events.take(&other, &HashSet::new()).is_empty());
    }
}
