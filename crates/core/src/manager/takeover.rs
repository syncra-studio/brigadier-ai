//! "Open in terminal" (THREAD-PLAN.md Q5): the user continues a worker's own CLI session in a
//! terminal, then the worker reports what was done there.
//!
//! - **Reserved first.** A per-task lock is taken before anything else, and the task is marked
//!   taken over before its headless CLI is closed: from then on no headless launch registers
//!   (`launch_admitted` refuses it), so a retry, a hand-off, a quota timer or a message never
//!   runs the session beside the terminal. Claude has no lock of its own: two writers would
//!   fork its transcript and the terminal's turns would be lost (spike, check 2). A second
//!   open finds the terminal and reattaches; a failed open puts everything back.
//! - **One writer.** The headless CLI has exited before the terminal starts.
//! - **Its own grant.** The headless worker's grant goes with its CLI; the terminal gets a fresh
//!   one under an owner of its own, revoked when it ends.
//! - **Handed back** when the terminal ends: the same native session resumes headless (never a
//!   size hand-off first: a fresh session's hand-off wouldn't hold the terminal's turns) and is
//!   asked for its report.
//! - **A deliberate end never hands back.** Stop, archive and delete record why first, then end
//!   the terminal and wait for its process to be gone, then dispose of the task.
//! - **A restart** keeps the task's worktree and session: the terminal's process (recorded in
//!   the ledger) is ended by the launch sweep if it survived, then the session is handed back,
//!   or the deliberate end that was under way finishes.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use brigadier_providers::model::Origin;
use brigadier_providers::{Artifact, ProviderKind, TerminalCommand, TurnInput};
use tokio::sync::oneshot;

use super::SessionManager;
use super::workers::TaskLive;
use crate::work::{Takeover, Task, TaskId, TaskState};
use crate::{Error, Result, now_ms};

/// Where Brigadier's terminals run (the daemon's pseudo terminals).
pub trait TerminalHost: Send + Sync {
    /// Starts `command` in a new terminal of `conversation` under `key`, sized
    /// `cols` × `rows`; the program runs directly, not through a shell.
    fn start(
        &self,
        conversation: &str,
        key: &str,
        command: TerminalCommand,
        cols: u16,
        rows: u16,
    ) -> Result<HostedTerminal>;

    /// Ends the terminal's process with its tree and resolves once it has exited (at once
    /// when it already has).
    fn terminate(&self, id: &str) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
}

/// A terminal a [`TerminalHost`] started.
pub struct HostedTerminal {
    pub id: String,
    pub pid: Option<u32>,
    /// The process's start time, which tells it apart from a later one with the same pid.
    pub started_at_ms: Option<f64>,
    /// Resolves when its process has ended.
    pub exited: oneshot::Receiver<()>,
}

/// The terminal a task's session is open in.
pub struct WorkerTerminal {
    pub terminal_id: String,
    /// Just started: all its output streams to whoever subscribed before (a reattached one
    /// has a past to show).
    pub fresh: bool,
    pub provider: ProviderKind,
}

/// A task's open terminal, as its live state keeps it.
pub(crate) struct LiveTakeover {
    pub terminal_id: String,
    /// Counts the terminals opened for the task, so an old one's end hands nothing back.
    pub generation: u64,
}

/// The owner of a terminal's grant and the ledger records it is kept under.
fn grant_owner(task: &TaskId) -> String {
    format!("takeover:{task}")
}

fn task_owner(task: &TaskId) -> String {
    format!("task:{task}")
}

/// The states a worker may be taken over from: its session exists and nothing lands it now.
fn can_take_over(state: TaskState) -> bool {
    matches!(
        state,
        TaskState::Running | TaskState::Blocked | TaskState::Paused | TaskState::Reported
    )
}

impl SessionManager {
    /// Where terminals run; the daemon sets it once at start.
    pub fn set_terminal_host(&self, host: Arc<dyn TerminalHost>) {
        let _ = self.terminal_host.set(host);
    }

    /// "Open in terminal": continues the task's worker session in a terminal (reattaching to
    /// the one already open).
    pub async fn open_worker_terminal(
        &self,
        task_id: TaskId,
        cols: u16,
        rows: u16,
    ) -> Result<WorkerTerminal> {
        let host = self
            .terminal_host
            .get()
            .cloned()
            .ok_or_else(|| Error::Invalid("terminals aren't available here".into()))?;
        let conversation_id = self.conversation_of_task(&task_id).await?;
        let _fence = self.enter(&conversation_id)?;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        let live = self.task_live(&task);
        let mut reservation = live.takeover.lock().await;
        let task = self.task_by_id(&conversation_id, &task_id).await?;
        let provider = task.route.choice.provider;
        if let Some(open) = &*reservation {
            return Ok(WorkerTerminal {
                terminal_id: open.terminal_id.clone(),
                fresh: false,
                provider,
            });
        }
        if !can_take_over(task.state) {
            return Err(Error::Invalid(format!(
                "task-{} can't open in a terminal while it is {}",
                task.number,
                state_words(task.state)
            )));
        }
        let native_id = self.last_worker_native_id(&task).await.ok_or_else(|| {
            Error::Invalid(format!(
                "task-{}'s worker has no session to continue yet",
                task.number
            ))
        })?;
        // Reserved before anything waits: no headless CLI registers from here on.
        live.set_taken_over(true).await;
        let from = task.state;
        live.close_cli().await;
        let marked = self
            .update_task(&conversation_id, &task_id, |t| {
                t.state = TaskState::TakenOver;
                t.blocked_reason = None;
                t.takeover = Some(Takeover {
                    native_id: native_id.clone(),
                    since_ms: now_ms(),
                    from,
                    pid: None,
                    started_at_ms: None,
                    ending: None,
                });
            })
            .await;
        let task = match marked {
            Ok(task) => task,
            Err(err) => {
                self.give_back(&live, &task, from).await;
                return Err(err);
            }
        };
        let generation = live.generation().await;
        match self
            .start_terminal(&*host, &task, &native_id, cols, rows)
            .await
        {
            Ok(terminal) => {
                let terminal_id = terminal.id.clone();
                let _ = self
                    .update_task(&conversation_id, &task_id, |t| {
                        if let Some(takeover) = &mut t.takeover {
                            takeover.pid = terminal.pid;
                            takeover.started_at_ms = terminal.started_at_ms;
                        }
                    })
                    .await;
                *reservation = Some(LiveTakeover {
                    terminal_id: terminal_id.clone(),
                    generation,
                });
                drop(reservation);
                let manager = self.arc();
                let (live, exited) = (live.clone(), terminal.exited);
                self.spawn(async move {
                    let _ = exited.await;
                    manager.terminal_ended(&live, generation).await;
                });
                if let Ok(conv) = self.conv(&conversation_id) {
                    conv.note(format!(
                        "[worker] The user opened task-{} \"{}\" in their terminal and works with it there. It reports when they close the terminal; messages to it wait until then.",
                        task.number, task.title
                    ))
                    .await;
                }
                Ok(WorkerTerminal {
                    terminal_id,
                    fresh: true,
                    provider,
                })
            }
            Err(err) => {
                self.grants.revoke_owner(&grant_owner(&task_id));
                let task = self
                    .update_task(&conversation_id, &task_id, |t| {
                        t.state = from;
                        t.takeover = None;
                    })
                    .await
                    .unwrap_or(task);
                self.give_back(&live, &task, from).await;
                Err(err)
            }
        }
    }

    /// Starts the worker's session in a terminal: the same spec as its headless process, a
    /// fresh grant, in its worktree when it has one.
    async fn start_terminal(
        &self,
        host: &dyn TerminalHost,
        task: &Task,
        native_id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<HostedTerminal> {
        let subject = match &task.subject {
            Some(id) => self.task_by_id(&task.conversation_id, id).await.ok(),
            None => None,
        };
        let session = self
            .worker_session(
                task,
                subject.as_ref(),
                Origin::Resume {
                    native_id: native_id.to_owned(),
                },
                None,
                &grant_owner(&task.id),
            )
            .await?;
        let provider = task.route.choice.provider;
        // Claude finds the session from any folder (spike, check 4), so the terminal opens in
        // the task's worktree; a Codex thread keeps its own folder (a read-only Codex worker
        // works from its scratch folder), where its sandbox was set up.
        let cwd: PathBuf = match (provider, &session.worktree) {
            (ProviderKind::Claude, Some(worktree)) => worktree.clone(),
            _ => session.cwd.clone(),
        };
        let command = self
            .runtime
            .terminal_command(provider, session.spec, cwd)
            .await?;
        let terminal = host.start(
            &task.conversation_id.0,
            &format!("worker:{}", task.id),
            command,
            cols,
            rows,
        )?;
        if let Some(pid) = terminal.pid {
            // A daemon that dies leaves it to the launch sweep, which ends it.
            let recorded = self
                .runtime
                .ledger()
                .record(
                    &task_owner(&task.id),
                    Artifact::Process {
                        pid,
                        started_at_ms: terminal.started_at_ms,
                    },
                )
                .await;
            if let Err(err) = recorded {
                tracing::warn!(task = %task.id, error = %err, "could not record a worker's terminal");
            }
        }
        Ok(terminal)
    }

    /// An open that didn't finish: the task is as before, and a worker that was working goes
    /// on.
    async fn give_back(&self, live: &Arc<TaskLive>, task: &Task, from: TaskState) {
        live.set_taken_over(false).await;
        live.allow_revival().await;
        if matches!(from, TaskState::Running | TaskState::Blocked) {
            let text = "[terminal] The terminal didn't open. Carry on with your task.".to_owned();
            if let Err(err) = self.resume_session(live, task, text).await {
                tracing::warn!(task = %task.id, error = %err, "could not resume a worker after a failed takeover");
            }
        }
    }

    /// The terminal ended (the user closed it, or it exited): unless the task is being ended
    /// on purpose, its session goes back to its headless worker, which reports.
    pub(crate) async fn terminal_ended(&self, live: &Arc<TaskLive>, generation: u64) {
        // Brigadier quits (its terminals close with it): the next start hands it back.
        if self.admit().is_err() {
            return;
        }
        // Its session is being archived or deleted: that cleanup ends the task (and waits for
        // a hand-back already under way, which holds this guard).
        let Ok(_guard) = self.enter(&live.conversation_id) else {
            return;
        };
        let mut reservation = live.takeover.lock().await;
        if reservation
            .as_ref()
            .is_none_or(|open| open.generation != generation)
        {
            return;
        }
        *reservation = None;
        self.grants.revoke_owner(&grant_owner(&live.id));
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        self.forget_terminal(&task).await;
        // A deliberate end took the reservation before ending the terminal: it never gets
        // here.
        let Some(takeover) = task.takeover.clone() else {
            return;
        };
        if task.state.is_final() {
            return;
        }
        self.hand_back_session(live, &task, takeover).await;
    }

    /// Resumes the session the terminal continued, headless, and asks for its report.
    async fn hand_back_session(&self, live: &Arc<TaskLive>, task: &Task, takeover: Takeover) {
        let task = match self
            .update_task(&task.conversation_id, &task.id, |t| {
                t.takeover = None;
                t.state = TaskState::Running;
                // What it reported before the terminal is replaced by its next report.
                t.candidate = None;
            })
            .await
        {
            Ok(task) => task,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not hand a worker back");
                return;
            }
        };
        live.set_taken_over(false).await;
        live.allow_revival().await;
        let mut text = "[terminal] The user continued this task with you in their terminal and has closed it. Submit your report now (submit_report): what was done in the terminal, by you and by the user, and where the work stands.".to_owned();
        let held = live.take_held_messages().await;
        if !held.is_empty() {
            text.push_str(
                "\n\nThe orchestrator's messages while the terminal was open, oldest first:\n",
            );
            for message in held {
                text.push_str("\n- ");
                text.push_str(&message.replace('\n', "\n  "));
            }
        }
        let resumed = self
            .launch_worker(
                live,
                &task,
                None,
                Origin::Resume {
                    native_id: takeover.native_id,
                },
                TurnInput::text(text),
            )
            .await;
        if let Err(err) = resumed {
            tracing::warn!(task = %task.id, error = %err, "could not resume a worker after its terminal");
            self.worker_failed(
                &task,
                &format!("Its session couldn't be resumed after the terminal: {err}"),
            )
            .await;
        }
    }

    /// Resumes the worker's last session with `text`, the way a revival does but never as a
    /// size hand-off.
    async fn resume_session(&self, live: &Arc<TaskLive>, task: &Task, text: String) -> Result<()> {
        let native_id = self
            .last_worker_native_id(task)
            .await
            .ok_or_else(|| Error::Invalid("no session to resume".into()))?;
        self.launch_worker(
            live,
            task,
            None,
            Origin::Resume { native_id },
            TurnInput::text(text),
        )
        .await
    }

    /// Before a task is disposed of (Stop, archive, delete, a failure): a terminal it is open
    /// in ends first, with nothing handed back. Why is recorded before the terminal is closed,
    /// and this returns once its process is gone.
    pub(crate) async fn end_takeover(&self, task: &Task, why: &str) {
        let Some(live) = self.existing_task_live(&task.id) else {
            if task.takeover.is_some() {
                self.forget_terminal(task).await;
            }
            return;
        };
        let mut reservation = live.takeover.lock().await;
        let Ok(now) = self.task_by_id(&task.conversation_id, &task.id).await else {
            return;
        };
        if now.takeover.is_none() && reservation.is_none() {
            return;
        }
        if now.takeover.is_some() {
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    if let Some(takeover) = &mut t.takeover {
                        takeover.ending = Some(why.to_owned());
                    }
                })
                .await;
        }
        if let (Some(open), Some(host)) = (reservation.take(), self.terminal_host.get()) {
            host.terminate(&open.terminal_id).await;
        }
        self.grants.revoke_owner(&grant_owner(&task.id));
        self.forget_terminal(&now).await;
        live.set_taken_over(false).await;
    }

    /// The terminal's process record, once it has ended (or to end it: a restart's survivor).
    async fn forget_terminal(&self, task: &Task) {
        if let Some(Takeover {
            pid: Some(pid),
            started_at_ms,
            ..
        }) = &task.takeover
        {
            self.runtime
                .ledger()
                .forget(
                    &task_owner(&task.id),
                    Artifact::Process {
                        pid: *pid,
                        started_at_ms: *started_at_ms,
                    },
                )
                .await;
        }
    }

    /// After a restart: the terminal is gone (the launch sweep ended it if it survived). A
    /// deliberate end that was under way finishes; otherwise the session is handed back.
    pub(super) async fn recover_takeover(&self, task: &Task) {
        self.forget_terminal(task).await;
        self.grants.revoke_owner(&grant_owner(&task.id));
        let Some(takeover) = task.takeover.clone() else {
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    t.state = TaskState::Reported;
                })
                .await;
            return;
        };
        if takeover.ending.is_some() {
            self.dispose_task(task, TaskState::Stopped).await;
            return;
        }
        let live = self.task_live(task);
        let manager = self.arc();
        let task = task.clone();
        self.spawn(async move { manager.hand_back_session(&live, &task, takeover).await });
    }

    /// A message for a worker whose session is open in the user's terminal waits for its
    /// report request; `None` when it isn't open there.
    pub(crate) async fn hold_for_terminal(&self, task: &Task, message: &str) -> Option<String> {
        let live = self.existing_task_live(&task.id)?;
        if !live.held_for_terminal(message).await {
            return None;
        }
        Some(format!(
            "task-{} is open in the user's terminal. Your message goes to it when they close the terminal, with the request for its report.",
            task.number
        ))
    }
}

/// A task state in a few words, for a refusal.
fn state_words(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "waiting to start",
        TaskState::Starting => "starting",
        TaskState::Landing => "landing",
        TaskState::ReadyToLand => "ready to land",
        TaskState::TakenOver => "open in a terminal",
        _ => "over",
    }
}
