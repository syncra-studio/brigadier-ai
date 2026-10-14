//! The thread's commands that outlast a tool call (`run`, `run_check`).
//!
//! A tool call holds the thread: while it runs, the thread can't sort a follow-up or read a
//! message steered to it, and the user waits on a silent row. So a command of the thread's
//! that is still going after [`FOREGROUND`] goes to the background: the call returns at once
//! with its id, the command goes on, owned by the session rather than the thread's CLI (a
//! closed or replaced CLI doesn't end it), and its result comes to the thread as a message in
//! the request that started it, which works until then. The whole operation moves, waits
//! included: a check waiting for the same check or for the build lease goes too.
//!
//! - **Recorded** as a [`BackgroundRun`] when it goes to the background, so the request works
//!   and the app shows it; a restart that cuts one off records it interrupted and tells the
//!   thread, which may run it again.
//! - **Stopped** with the user's Stop and the session's archive or delete, telling nothing.

use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use super::SessionManager;
use super::conversation::Envelope;
use crate::Result;
use crate::model::{ConversationId, DomainEvent};
use crate::now_ms;
use crate::work::{BackgroundRun, BackgroundRunState, InjectionKind};

/// How long a command of the thread's may hold its tool call before it goes to the
/// background. Tests wait less.
pub(crate) const FOREGROUND: Duration = Duration::from_secs(if cfg!(test) { 2 } else { 60 });

/// `run-<n>`'s number (0 for anything else).
pub(crate) fn background_run_number(id: &str) -> u64 {
    id.strip_prefix("run-")
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// The cleanup-ledger owner of a session's background runs: they outlive the thread's CLI and
/// end with the session.
pub(crate) fn background_owner(id: &ConversationId) -> String {
    format!("background:{id}")
}

/// What ends each running background run, by conversation and id.
#[derive(Default)]
pub(crate) struct BackgroundRuns {
    running: std::sync::Mutex<HashMap<(ConversationId, String), CancellationToken>>,
    /// Held while a run's number is taken and its start recorded.
    numbering: tokio::sync::Mutex<()>,
}

impl SessionManager {
    /// Runs `work`, the thread's `tool` call of `command` (its status and reply), in its tool
    /// call while it ends within [`FOREGROUND`], else in the background: its reply then comes
    /// as a message.
    /// `cancel` ends the work; the thread's CLI ending (`cli_ended`) cancels it only while it
    /// still holds the call.
    pub(super) async fn in_foreground<F>(
        &self,
        id: &ConversationId,
        tool: &str,
        command: &str,
        cli_ended: CancellationToken,
        cancel: CancellationToken,
        work: F,
    ) -> Result<String>
    where
        F: Future<Output = Result<(String, String)>> + Send + 'static,
    {
        // Whose answer it serves: the turn's request, taken before the call can outlive it.
        let request = match self.conv(id) {
            Ok(conv) => conv.running_request().await,
            Err(_) => None,
        };
        let mut handle = tokio::spawn(work);
        let joined = tokio::select! {
            joined = &mut handle => Some(joined),
            () = cli_ended.cancelled() => {
                cancel.cancel();
                Some((&mut handle).await)
            }
            () = tokio::time::sleep(FOREGROUND) => None,
        };
        if let Some(joined) = joined {
            return joined
                .map_err(|err| crate::Error::Invalid(err.to_string()))?
                .map(|(_, reply)| reply);
        }
        let run = {
            let _numbering = self.background_runs.numbering.lock().await;
            let number = self
                .core
                .board(id)
                .await
                .map(|board| {
                    board
                        .background_runs
                        .keys()
                        .map(|id| background_run_number(id))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0)
                + 1;
            let run = BackgroundRun {
                id: format!("run-{number}"),
                conversation_id: id.clone(),
                request_id: request,
                tool: tool.to_owned(),
                command: command.to_owned(),
                started_at_ms: now_ms(),
                ended_at_ms: None,
                state: BackgroundRunState::Running,
            };
            self.background_runs
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert((id.clone(), run.id.clone()), cancel.clone());
            self.store_background_run(&run).await;
            run
        };
        let reply = format!(
            "[{id} still runs after {secs} s, so it goes on in the background: its result comes \
             to you as a message in this request, which works until then. Don't wait for it or \
             run it again. Tell the user in one line what runs and roughly how long, then carry \
             on or reply {quiet}.]",
            id = run.id,
            secs = FOREGROUND.as_secs(),
            quiet = super::prompts::QUIET,
        );
        let manager = self.arc();
        self.spawn(async move {
            let result = handle
                .await
                .unwrap_or_else(|err| Err(crate::Error::Invalid(err.to_string())));
            manager.background_run_ended(run, result, cancel).await;
        });
        Ok(reply)
    }

    /// A background run ended: recorded, and unless the user's Stop or the session's close
    /// ended it, its result goes to the thread.
    async fn background_run_ended(
        &self,
        mut run: BackgroundRun,
        result: Result<(String, String)>,
        cancel: CancellationToken,
    ) {
        self.background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&(run.conversation_id.clone(), run.id.clone()));
        run.ended_at_ms = Some(now_ms());
        if cancel.is_cancelled() {
            run.state = BackgroundRunState::Stopped;
            self.store_background_run(&run).await;
            self.settle_requests(&run.conversation_id).await;
            return;
        }
        let (status, text) = match result {
            Ok(done) => done,
            Err(err) => ("couldn't run".to_owned(), format!("[couldn't run: {err}]")),
        };
        run.state = BackgroundRunState::Finished { status };
        self.store_background_run(&run).await;
        let envelope = Envelope {
            kind: InjectionKind::BackgroundRun,
            label: format!("{} finished", run.id),
            task_id: None,
            text: format!(
                "[{} finished · {} `{}`]\n{text}",
                run.id, run.tool, run.command
            ),
        };
        self.deliver_for(&run.conversation_id, envelope, run.request_id.clone())
            .await;
    }

    /// Ends the session's background runs (the user's Stop, an archive or delete): each is
    /// recorded stopped and tells nothing.
    pub(crate) fn stop_background_runs(&self, id: &ConversationId) {
        let running = self
            .background_runs
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for ((of, _), cancel) in running.iter() {
            if of == id {
                cancel.cancel();
            }
        }
    }

    /// At startup: a background run the restart cut off is recorded so, and the thread hears
    /// it in the run's request, so it can run it again.
    pub(crate) async fn recover_background_runs(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let mut cut: Vec<BackgroundRun> = board
            .background_runs
            .values()
            .filter(|run| run.state == BackgroundRunState::Running)
            .cloned()
            .collect();
        cut.sort_by_key(|run| background_run_number(&run.id));
        for mut run in cut {
            run.state = BackgroundRunState::Interrupted {
                reason: "a restart".into(),
            };
            run.ended_at_ms = Some(now_ms());
            self.store_background_run(&run).await;
            let envelope = Envelope {
                kind: InjectionKind::BackgroundRun,
                label: format!("{} cut off", run.id),
                task_id: None,
                text: format!(
                    "[{} was cut off by a restart before it finished · {} `{}`: no result came. \
                     Run it again if you still need it.]",
                    run.id, run.tool, run.command
                ),
            };
            self.deliver_for(id, envelope, run.request_id.clone()).await;
        }
    }

    async fn store_background_run(&self, run: &BackgroundRun) {
        if let Err(err) = self
            .core
            .record_conversation(
                &run.conversation_id,
                vec![DomainEvent::BackgroundRunUpdated { run: run.clone() }],
            )
            .await
        {
            tracing::warn!(run = %run.id, error = %err, "could not record a background run");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_runs_number_is_read_from_its_id() {
        assert_eq!(background_run_number("run-12"), 12);
        assert_eq!(background_run_number("check-1"), 0);
    }
}
