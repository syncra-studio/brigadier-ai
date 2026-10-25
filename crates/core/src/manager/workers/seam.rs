//! A worker's hand-off to a fresh CLI session (D14). Its CLI compacts by itself. After
//! [`WORKER_HANDOFF_AFTER`] compactions in one CLI session, once its turn ends (and its session
//! heard that end as any other), it is asked for a hand-off note in a turn of its own; its
//! CLI then closes, and the next message its session sends starts a fresh session of the same
//! CLI as the same worker, in the same workspace, from the note and the `brigadier context`
//! pointer, with nothing else put in (D13). A compaction that fails or keeps refilling the
//! context cuts its work off instead: its session hears nothing of that turn (run2-D44), the
//! note is asked for at once, and the successor carries on by itself. A context too long for
//! the model, which a note turn would overflow too, hands off without a note. The note is
//! never steered into a running turn: a steer that reached the CLI after its turn ended would
//! start the note turn behind the successor's back.
//!
//! The successor starts as [`Origin::NewAs`], which never reruns the workspace's setup script
//! or copies its secrets again (those run for a worker's first start only). The worker's
//! `record/seam.json` is the hand-off's truth across restarts, as a session's is
//! ([`crate::manager::seam`]): each phase is written before what it leads to, so a restart at
//! any point goes on from it without starting two successors or sending anything twice.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use brigadier_providers::{ErrorKind, Origin, ProviderError, TurnInput, TurnStatus};

use super::TaskLive;
use crate::manager::conversation::Cli;
use crate::manager::conversation::long::{LongFile, read_long, thrashing, write_long};
use crate::manager::long_sessions::WORKER_HANDOFF_AFTER;
use crate::manager::seam::{
    Boot, CARRY_ON, LONG_NOTE, Phase, Reason, SeamEnd, SeamFile, TailBudget, bootstrap, complete,
    handoff_line, note_path, nudge, read_seam, start_path, write_seam,
};
use crate::manager::{SessionManager, blocking};
use crate::work::{AttemptEnd, Task};
use crate::{Error, Result, now_ms};

/// Why a worker ended its generation without a note, when it simply didn't write one.
const NO_NOTE: &str = "it ended its turn without writing one";
/// …and when Brigadier restarted in its note turn.
const RESTARTED: &str = "Brigadier restarted before it wrote one";

/// A worker's hand-off under way, in memory. `carry_on`: its work was cut off, so its
/// successor starts at once and goes on by itself; else it starts with the next message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SeamLive {
    /// The note is asked for once the running turn ends.
    Owed { carry_on: bool },
    /// Its note turn runs: messages wait for the successor.
    Nudged { carry_on: bool },
    /// It hands off without a note when its turn ends (`death`: why).
    Due { death: String },
}

impl SeamLive {
    /// Messages its session sends now wait for its successor.
    pub(super) fn holds(&self) -> bool {
        !matches!(self, Self::Owed { .. })
    }
}

impl SessionManager {
    /// One compaction of the worker's CLI ended: the [`WORKER_HANDOFF_AFTER`]th in this CLI
    /// session owes a hand-off once its turn ends; a failed one cuts its work off (D14).
    pub(super) async fn worker_compacted(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        error: Option<&str>,
    ) {
        if error.is_some() {
            Self::owe_seam(live, cli, SeamLive::Owed { carry_on: true }).await;
            return;
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        let record = self.worker_record(&task);
        let count = blocking(move || {
            let generation = read_seam(&record).generation;
            let mut file: LongFile = read_long(&record);
            let count = file.count(generation);
            write_long(&record, &file);
            Ok(count)
        })
        .await
        .unwrap_or(0);
        if count >= WORKER_HANDOFF_AFTER {
            Self::owe_seam(live, cli, SeamLive::Owed { carry_on: false }).await;
        }
    }

    /// A worker turn's error the hand-off takes, rather than its session: Claude's thrash stop
    /// owes the note; a context too long for the model hands off without one (D14).
    pub(super) async fn worker_seam_error(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        error: &ProviderError,
    ) -> bool {
        let seam = if error.will_retry {
            return false;
        } else if thrashing(&error.message) {
            SeamLive::Owed { carry_on: true }
        } else if error.kind == ErrorKind::ContextWindow {
            SeamLive::Due {
                death: "its context was too long for the model".into(),
            }
        } else {
            return false;
        };
        Self::owe_seam(live, cli, seam).await
    }

    /// Records the hand-off `seam` owed by `cli`, the worker's running CLI: a cut-off owes
    /// more than a count does. Whether it is owed now.
    async fn owe_seam(live: &Arc<TaskLive>, cli: &Arc<Cli>, seam: SeamLive) -> bool {
        let mut state = live.state.lock().await;
        if !state.cli.as_ref().is_some_and(|now| Arc::ptr_eq(now, cli)) {
            return false;
        }
        match (&state.seam, &seam) {
            (None | Some(SeamLive::Owed { carry_on: false }), _) => state.seam = Some(seam),
            (Some(SeamLive::Owed { carry_on: true }), SeamLive::Due { .. }) => {
                state.seam = Some(seam);
            }
            _ => {}
        }
        true
    }

    /// The worker's turn ended with a hand-off under way (`seam`). Owed: a count's turn ends as
    /// any other, and a cut-off one unheard; then its note is asked for in a turn of its own.
    /// Its note turn's end, or a turn that overflowed, closes its CLI: the successor starts at
    /// once when its work was cut off or a message waits, else with the next one. A usage
    /// limit calls the hand-off off: the turn ends as any other, and its session hears it.
    pub(super) async fn worker_seam_turn_end(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        seam: SeamLive,
        status: TurnStatus,
    ) {
        let record = self.worker_record(task);
        let cutoff = live.take_cutoff().await;
        if let Some(limit @ AttemptEnd::Limit { .. }) = cutoff {
            live.state.lock().await.seam = None;
            let at = record.clone();
            let _ = blocking(move || {
                let mut file = read_seam(&at);
                file.phase = Phase::Live;
                write_seam(&at, &file)
            })
            .await;
            live.set_cutoff(limit).await;
            return self.plain_turn_ended(live, task, status).await;
        }
        let (carry_on, death) = match seam {
            SeamLive::Owed { carry_on } => {
                if !carry_on {
                    // Its session's messages wait for the successor from before it hears the
                    // turn end: one sent at once would start a turn the note's then cuts off.
                    live.state.lock().await.seam = Some(SeamLive::Nudged { carry_on });
                    self.plain_turn_ended(live, task, status).await;
                }
                if self.worker_seam_nudge(live, task, carry_on).await {
                    return;
                }
                // Asked nothing, it hands off without a note.
                (carry_on, Some("it couldn't be asked for one".to_owned()))
            }
            SeamLive::Nudged { carry_on } => (carry_on, None),
            SeamLive::Due { death } => (true, Some(death)),
        };
        let predecessor = live.cli().await.map(|cli| cli.session.native_id());
        let at = record.clone();
        let closed = blocking(move || {
            let mut file = read_seam(&at);
            let written = note_path(&at, file.generation).is_file();
            let (nudged_at_ms, predecessor) = match &file.phase {
                Phase::Nudged {
                    at_ms, predecessor, ..
                } => (Some(*at_ms), predecessor.clone()),
                _ => (None, predecessor),
            };
            let death = death.or_else(|| (!written).then(|| NO_NOTE.to_owned()));
            file.phase = Phase::Closed {
                end: closed_end(predecessor, nudged_at_ms, death),
            };
            write_seam(&at, &file)
        })
        .await;
        if let Err(err) = closed {
            tracing::warn!(task = %task.id, error = %err, "could not close the worker's generation");
            live.state.lock().await.seam = None;
            if carry_on {
                self.plain_turn_ended(live, task, status).await;
            }
            return;
        }
        live.close_cli().await;
        let _reroute = live.reroute.lock().await;
        let waiting = !live.state.lock().await.seam_held.is_empty();
        if !carry_on && !waiting {
            // Its successor starts with the next message.
            live.state.lock().await.seam = None;
            live.allow_revival().await;
            return;
        }
        if let Err(err) = self.worker_successor(live, task, Vec::new()).await {
            self.plain_failed(task, format!("Its fresh session could not start: {err}."))
                .await;
        }
    }

    /// Asks the worker for its hand-off note in a turn of its own (its last one has ended):
    /// whether it was asked.
    async fn worker_seam_nudge(&self, live: &Arc<TaskLive>, task: &Task, carry_on: bool) -> bool {
        let Some(cli) = live.cli().await else {
            return false;
        };
        let record = self.worker_record(task);
        let predecessor = cli.session.native_id();
        let at = record.clone();
        let note = blocking(move || {
            let mut file = read_seam(&at);
            file.phase = Phase::Nudged {
                at_ms: now_ms(),
                tokens: 0,
                predecessor: Some(predecessor),
            };
            write_seam(&at, &file)?;
            let note = note_path(&at, file.generation);
            if let Some(dir) = note.parent() {
                std::fs::create_dir_all(dir).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            Ok(note)
        })
        .await;
        let note = match note {
            Ok(note) => note,
            Err(err) => {
                tracing::warn!(task = %live.id, error = %err, "could not record the worker's hand-off");
                return false;
            }
        };
        let text = format!("{}{LONG_NOTE}", nudge(&note));
        let mut state = live.state.lock().await;
        state.seam = Some(SeamLive::Nudged { carry_on });
        state.busy = true;
        if let Err(err) = cli.session.send(TurnInput::text(text)).await {
            tracing::warn!(task = %live.id, error = %err, "could not ask the worker for its note");
            state.busy = false;
            return false;
        }
        true
    }

    /// Starts the successor of the worker's closed generation (the caller holds its
    /// `reroute`), its first input the hand-off, then the messages its session sent meanwhile
    /// and `messages`, else to carry on. Nothing else goes in (D13).
    pub(super) async fn worker_successor(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        messages: Vec<String>,
    ) -> Result<()> {
        let record = self.worker_record(task);
        let file = {
            let at = record.clone();
            blocking(move || Ok(read_seam(&at))).await?
        };
        let Phase::Closed { end } = file.phase.clone() else {
            return Err(Error::Invalid("no hand-off is under way".into()));
        };
        let mut held = {
            let mut state = live.state.lock().await;
            state.seam = None;
            std::mem::take(&mut state.seam_held)
        };
        held.extend(messages);
        let provider = task.route.choice.provider;
        let reserved = uuid::Uuid::new_v4().to_string();
        let key = task.id.0.clone();
        let (at, carried, reserving) = (record.clone(), held.clone(), reserved.clone());
        let first = blocking(move || {
            let mut file = read_seam(&at);
            let n = file.generation;
            file.phase = Phase::Starting {
                end: end.clone(),
                successor: reserving,
                provider,
                messages: carried.clone(),
            };
            write_seam(&at, &file)?;
            let note_at = note_path(&at, n);
            let note = std::fs::read_to_string(&note_at).ok();
            let start = start_path(&at, n);
            let text = bootstrap(&Boot {
                n,
                tail: TailBudget {
                    reason: Reason::Size,
                    hot_tokens: None,
                    target_window: None,
                },
                key: &key,
                folder: &at,
                lineage: &[],
                note_path: &note_at,
                note: note.as_deref(),
                start_path: &start,
                death: match &note {
                    Some(_) => None,
                    None => Some(end.death.as_deref().unwrap_or(NO_NOTE)),
                },
                entries: &[],
                blobs: &at,
            });
            if let Some(parent) = start.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            std::fs::write(&start, &text).map_err(|err| Error::Invalid(err.to_string()))?;
            let then = if carried.is_empty() {
                CARRY_ON.to_owned()
            } else {
                carried.join("\n\n")
            };
            Ok((n, format!("{text}\n\n{then}")))
        })
        .await;
        let (n, first) = match first {
            Ok(first) => first,
            Err(err) => {
                live.state.lock().await.seam_held = held;
                return Err(err);
            }
        };
        let launched = self
            .launch_successor(live, task, reserved, TurnInput::text(first), &record)
            .await;
        let at = record.clone();
        let done = launched.is_ok();
        let _ = blocking(move || {
            let mut file = read_seam(&at);
            let Phase::Starting {
                end,
                successor,
                provider,
                messages,
            } = file.phase.clone()
            else {
                return Ok(());
            };
            if done {
                complete(&mut file, &at, n, successor, provider, end, messages);
            } else {
                file.phase = Phase::Closed { end };
            }
            write_seam(&at, &file)
        })
        .await;
        if !done {
            live.state.lock().await.seam_held = held;
        }
        launched
    }

    /// After a restart, before the worker's cut-off turn is told: where its hand-off stood
    /// goes on from its `seam.json`. A note turn the restart cut off ends its generation, with
    /// the note it wrote or without; a successor that was starting is asked whether it took
    /// its hand-off, and only if it didn't do the messages it carried go again, with the next
    /// successor. A closed generation's successor starts with the next message it is sent.
    pub(crate) async fn recover_worker_seam(&self, task: &Task) {
        let record = self.worker_record(task);
        let at = record.clone();
        let Ok(mut file) = blocking(move || Ok(read_seam(&at))).await else {
            return;
        };
        match file.phase.clone() {
            Phase::Live | Phase::Closed { .. } => {}
            Phase::Nudged {
                at_ms, predecessor, ..
            } => {
                let written = note_path(&record, file.generation).is_file();
                file.phase = Phase::Closed {
                    end: closed_end(
                        predecessor,
                        Some(at_ms),
                        (!written).then(|| RESTARTED.to_owned()),
                    ),
                };
                self.write_worker_seam(&record, file).await;
            }
            Phase::Starting {
                end,
                successor,
                provider,
                messages,
            } => {
                let line = handoff_line(file.generation, &task.id.0);
                if self.runtime.took_input(provider, &successor, &line).await {
                    let n = file.generation;
                    complete(
                        &mut file,
                        &record,
                        n,
                        successor.clone(),
                        provider,
                        end,
                        messages,
                    );
                    self.write_worker_seam(&record, file).await;
                    let _ = self
                        .update_task(&task.conversation_id, &task.id, |t| {
                            t.native_session = Some(successor);
                        })
                        .await;
                    return;
                }
                file.phase = Phase::Closed { end };
                self.write_worker_seam(&record, file).await;
                self.task_live(task).state.lock().await.seam_held = messages;
            }
        }
    }

    async fn write_worker_seam(&self, record: &Path, file: SeamFile) {
        let at = record.to_owned();
        if let Err(err) = blocking(move || write_seam(&at, &file)).await {
            tracing::warn!(error = %err, "could not record the worker's hand-off");
        }
    }

    /// Whether the worker's generation closed with no successor yet: its next message starts
    /// one.
    pub(super) async fn worker_seam_closed(&self, task: &Task) -> bool {
        let record = self.worker_record(task);
        blocking(move || Ok(matches!(read_seam(&record).phase, Phase::Closed { .. })))
            .await
            .unwrap_or(false)
    }

    /// Starts the worker's successor under the id `reserved`, sending it `first` once its CLI
    /// is up; its `Starting` phase names the id the CLI took first (Codex picks its own), so a
    /// restart asks the right session whether it took its hand-off.
    // Not `async`: a successor's launch is reached from its predecessor's, through its
    // events, so its future's type is named here rather than inferred.
    fn launch_successor<'a>(
        &'a self,
        live: &'a Arc<TaskLive>,
        task: &'a Task,
        reserved: String,
        first: TurnInput,
        record: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let fence = self.enter(&task.conversation_id)?;
            let origin = Origin::NewAs {
                native_id: reserved,
            };
            let launched = self
                .launch_admitted(live, task, origin, first, &fence, Some(record))
                .await;
            if launched.is_ok() && fence.cut_off() {
                live.close_cli().await;
                return Err(crate::manager::closing::closing_error());
            }
            launched
        })
    }
}

/// How a worker's generation ended: asked for its note at `nudged_at_ms`, or not; `death`:
/// why it wrote none.
fn closed_end(
    predecessor: Option<String>,
    nudged_at_ms: Option<i64>,
    death: Option<String>,
) -> SeamEnd {
    SeamEnd {
        predecessor,
        hot_tokens: None,
        nudged_at_ms,
        closed_at_ms: now_ms(),
        death,
        stopped: false,
        shown: Vec::new(),
        moved_to: None,
    }
}

/// Points the reserved successor at the id its CLI took, before its first input goes.
pub(super) fn name_successor(
    record: &Path,
    native_id: &str,
    provider: brigadier_providers::ProviderKind,
) {
    let mut file = read_seam(record);
    if let Phase::Starting {
        successor,
        provider: was,
        ..
    } = &mut file.phase
        && (successor != native_id || *was != provider)
    {
        *successor = native_id.to_owned();
        *was = provider;
        if let Err(err) = write_seam(record, &file) {
            tracing::warn!(error = %err, "could not record the worker's successor");
        }
    }
}
