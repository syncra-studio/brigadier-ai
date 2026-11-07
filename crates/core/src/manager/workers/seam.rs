//! A worker's hand-off to a fresh CLI session (D14), the same cycle as a thread's. Its CLI
//! compacts by itself. At its [`HANDOFF_AFTER`]th compaction in one CLI session it is asked for
//! a hand-off note at once, steered into the turn that runs, as a thread is; its CLI then closes
//! and a fresh session of the same CLI goes on by itself as the same worker, in the same
//! workspace, from the note and the `brigadier context` pointer, with nothing else put in (D13).
//! Its session hears nothing until the work's turn ends (run2-D44). A compaction that fails or
//! keeps refilling the context ends its turn first: the note is asked for then. A context too
//! long for the model, which a note turn would overflow too, hands off without a note. A steer
//! that reached the CLI just after its turn ended starts the note turn instead, which is as
//! good: the turn end finds the hand-off under way, and closing the CLI ends what still runs.
//!
//! The successor starts as [`Origin::NewAs`], which never reruns the workspace's setup script
//! or copies its secrets again (those run for a worker's first start only). The worker's
//! `record/seam.json` is the hand-off's truth across restarts, as a session's is
//! ([`crate::manager::seam`]): each phase is written before what it leads to, so a restart at
//! any point goes on from it without starting two successors or sending anything twice.
//!
//! What its session sends meanwhile is acknowledged, so it is kept first: in
//! `record/seam-held.json`, each with a once-only mark ([`super::once`]). A restart restores it
//! and its successor starts at once with it. A usage limit that calls the hand-off off sends it
//! with the worker's next input. A note turn that never ends has [`SeamConfig::hold`] once
//! somebody waits on it, a message or a refused pause or switch.
//!
//! [`SeamConfig::hold`]: crate::manager::seam::SeamConfig::hold

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use brigadier_providers::{ErrorKind, InputPart, Origin, ProviderError, TurnInput, TurnStatus};

use super::once::{mark_in, marked_once, once_mark, without_mark};
use super::{TaskLive, TaskLiveState, explicitly_paused};
use crate::manager::conversation::Cli;
use crate::manager::conversation::long::{LongFile, read_long, thrashing, write_long};
use crate::manager::long_sessions::HANDOFF_AFTER;
use crate::manager::seam::{
    Boot, CARRY_ON, LONG_NOTE, NOTE_FIXES, Phase, Reason, SeamEnd, SeamFile, TailBudget, bootstrap,
    check_note, complete, handoff_line, note_path, nudge, read_seam, start_path, write_seam,
};
use crate::manager::session_dir::write_atomic;
use crate::manager::{SessionManager, blocking};
use crate::work::{AttemptEnd, Task, TaskState};
use crate::{Error, Result, now_ms};

/// Why a worker ended its generation without a note, when it simply didn't write one.
const NO_NOTE: &str = "it ended its turn without writing one";
/// …and when Brigadier restarted in its note turn.
const RESTARTED: &str = "Brigadier restarted before it wrote one";
/// What a worker's hand-off holds for its successor, beside its `seam.json`: only its state
/// lock's holder writes it, so it never overwrites a phase.
const HELD_FILE: &str = "seam-held.json";

/// A worker's hand-off under way, in memory. Its successor starts at once and goes on by itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SeamLive {
    /// The note is asked for once the running turn ends.
    Owed,
    /// It was asked for its note: messages wait for the successor. `fixes`: how many times it
    /// was asked to fix a note that failed goal 13's check.
    Nudged { fixes: u32 },
    /// Its note is asked for once more once the running turn ends, the last time.
    NudgeAgain,
    /// It hands off without a note when its turn ends (`death`: why).
    Due { death: String },
}

impl SeamLive {
    /// Messages its session sends now wait for its successor.
    pub(super) fn holds(&self) -> bool {
        !matches!(self, Self::Owed)
    }
}

impl SessionManager {
    /// One compaction of the worker's CLI ended: the [`HANDOFF_AFTER`]th in this CLI session, or
    /// one that leaves its context above half its point, hands off now, as a thread's does; a
    /// failed one once its turn ends (D14).
    pub(super) async fn worker_compacted(
        &self,
        live: &Arc<TaskLive>,
        cli: &Arc<Cli>,
        tokens_after: Option<i64>,
        error: Option<&str>,
    ) {
        if error.is_some() {
            Self::owe_seam(live, cli, SeamLive::Owed).await;
            return;
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        // Still above half its point, it would compact again almost at once.
        let window = self
            .worker_context(&task)
            .and_then(|size| size.window_tokens);
        if let Some(after) = tokens_after
            && after > self.seam_config().threshold(window, None) / 2
        {
            self.worker_handoff_now(live, cli, &task).await;
            return;
        }
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
        if count >= HANDOFF_AFTER {
            self.worker_handoff_now(live, cli, &task).await;
        }
    }

    /// Asks for the note in the turn that runs, as a thread's seam does; with no turn to steer
    /// into, once its turn ends. Spawned: a CLI's answer to the steer comes through the events
    /// this was called from.
    async fn worker_handoff_now(&self, live: &Arc<TaskLive>, cli: &Arc<Cli>, task: &Task) {
        let running = {
            let mut state = live.state.lock().await;
            let running = state.seam.is_none()
                && state.cli.as_ref().is_some_and(|now| Arc::ptr_eq(now, cli))
                && state.busy
                && !state.compacting
                && !state.stopping;
            if running {
                state.seam = Some(SeamLive::Nudged { fixes: 0 });
            }
            running
        };
        if !running {
            Self::owe_seam(live, cli, SeamLive::Owed).await;
            return;
        }
        let (manager, live, cli, task) = (self.arc(), live.clone(), cli.clone(), task.clone());
        self.spawn(async move {
            let steered = match manager.worker_note_ask(&cli, &task).await {
                Some(text) => cli.session.steer_running(TurnInput::text(text)).await,
                None => Ok(false),
            };
            if !matches!(steered, Ok(true)) {
                // Still running: its turn's end asks. Over: that end took the hand-off on.
                let mut state = live.state.lock().await;
                if state.busy && state.seam == Some(SeamLive::Nudged { fixes: 0 }) {
                    state.seam = Some(SeamLive::Owed);
                }
            }
        });
    }

    /// Compacts the idle worker of `task` now through its CLI's own compaction, in a turn of its
    /// own its session doesn't hear: its end counts as any compaction (goal 2's proofs).
    #[cfg(debug_assertions)]
    pub(crate) async fn debug_compact_worker(&self, task: &crate::work::TaskId) -> Result<()> {
        let live = self.tasks_lock().get(task).cloned();
        let Some(live) = live else {
            return Err(Error::Invalid("that worker isn't running".into()));
        };
        let mut state = live.state.lock().await;
        let Some(cli) = state.cli.clone().filter(|cli| cli.session.can_compact()) else {
            return Err(Error::Invalid(
                "its CLI doesn't run or can't compact".into(),
            ));
        };
        if state.busy || state.seam.is_some() {
            return Err(Error::Invalid("wait until the worker is idle".into()));
        }
        state.busy = true;
        state.compacting = true;
        drop(state);
        if let Err(err) = cli.session.compact(None).await {
            let mut state = live.state.lock().await;
            state.busy = false;
            state.compacting = false;
            return Err(Error::Provider(err.to_string()));
        }
        Ok(())
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
            SeamLive::Owed
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
            (None, _) | (Some(SeamLive::Owed), SeamLive::Due { .. }) => state.seam = Some(seam),
            // A cut-off's turn end may come before its CLI took the steered note request: asked
            // once more, in a turn of its own; a cut-off of that one ends its generation.
            (Some(SeamLive::Nudged { fixes: 0 }), _) => state.seam = Some(SeamLive::NudgeAgain),
            _ => {}
        }
        true
    }

    /// The worker's turn ended with a hand-off under way (`seam`), unheard by its session. Owed:
    /// its note is asked for in a turn of its own. Its note's turn's end, or a turn that
    /// overflowed, closes its CLI and the successor starts at once. A usage limit calls the
    /// hand-off off: the turn ends as any other, and its session hears it.
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
            // What it holds stays kept, for the worker's next input.
            live.state.lock().await.seam_over();
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
        let death = match seam {
            SeamLive::Owed | SeamLive::NudgeAgain => {
                let fixes = u32::from(seam == SeamLive::NudgeAgain);
                if self.worker_seam_nudge(live, task, fixes).await {
                    return;
                }
                // Asked nothing, it hands off without a note.
                Some("it couldn't be asked for one".to_owned())
            }
            SeamLive::Nudged { fixes } => {
                // Goal 13 (core-goals D9): a note that leaves out an open item, or no note, is
                // fixed by the worker itself while it can still act.
                if status == TurnStatus::Completed
                    && fixes < NOTE_FIXES
                    && self.worker_note_fix(live, task, fixes).await
                {
                    return;
                }
                None
            }
            SeamLive::Due { death } => Some(death),
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
            {
                let mut state = live.state.lock().await;
                state.seam_over();
                state.stopping = false;
            }
            self.plain_turn_ended(live, task, status).await;
            return;
        }
        live.close_cli().await;
        let _reroute = live.reroute.lock().await;
        if let Err(err) = self.worker_successor(live, task, Vec::new()).await {
            {
                let mut state = live.state.lock().await;
                state.stopping = false;
                state.busy = false;
            }
            self.no_successor(task, &err).await;
        }
    }

    /// Asks the worker for its hand-off note in a turn of its own (its last one has ended):
    /// whether it was asked.
    async fn worker_seam_nudge(&self, live: &Arc<TaskLive>, task: &Task, fixes: u32) -> bool {
        let Some(cli) = live.cli().await else {
            return false;
        };
        let Some(text) = self.worker_note_ask(&cli, task).await else {
            return false;
        };
        {
            let mut state = live.state.lock().await;
            state.seam = Some(SeamLive::Nudged { fixes });
            state.busy = true;
        }
        if let Err(err) = cli.session.send(TurnInput::text(text)).await {
            tracing::warn!(task = %live.id, error = %err, "could not ask the worker for its note");
            let mut state = live.state.lock().await;
            // The hold ran out meanwhile, and ends its generation itself.
            if state.stopping {
                return true;
            }
            state.busy = false;
            return false;
        }
        true
    }

    /// Records that the worker is being asked for its note: the words to ask it with.
    async fn worker_note_ask(&self, cli: &Arc<Cli>, task: &Task) -> Option<String> {
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
                tracing::warn!(task = %task.id, error = %err, "could not record the worker's hand-off");
                return None;
            }
        };
        Some(format!("{}{LONG_NOTE}", nudge(&note)))
    }

    /// Asks the worker, in a turn of its own, to fix its note when it fails goal 13's check:
    /// whether it was asked.
    async fn worker_note_fix(&self, live: &Arc<TaskLive>, task: &Task, fixes: u32) -> bool {
        let Some(cli) = live.cli().await else {
            return false;
        };
        let record = self.worker_record(task);
        let fix = blocking(move || {
            let note = note_path(&record, read_seam(&record).generation);
            Ok(check_note(&record, &note).fix(&note))
        })
        .await;
        let Ok(Some(fix)) = fix else {
            return false;
        };
        {
            let mut state = live.state.lock().await;
            state.seam = Some(SeamLive::Nudged { fixes: fixes + 1 });
            state.busy = true;
        }
        if let Err(err) = cli.session.send(TurnInput::text(fix)).await {
            tracing::warn!(task = %live.id, error = %err, "could not ask the worker to fix its note");
            let mut state = live.state.lock().await;
            if state.stopping {
                return true;
            }
            state.seam = Some(SeamLive::Nudged { fixes });
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
        self.held_for_input(live, task).await;
        let waited = {
            let mut state = live.state.lock().await;
            state.seam_over();
            std::mem::take(&mut state.seam_held)
        };
        let mut held = waited.clone();
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
            let check = check_note(&at, &note_at).line();
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
                check: &check,
                replayed: &[],
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
                live.state.lock().await.seam_held = waited;
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
        // What came with this call goes back to its caller, told it didn't go. Else its side file
        // empties only now: a crash before leaves it beside what `seam.json` recorded.
        let mut state = live.state.lock().await;
        if done {
            save_held(&record, &state.seam_held).await;
        } else {
            state.seam_held = waited;
        }
        drop(state);
        launched
    }

    /// After a restart, before the worker's cut-off turn is told: where its hand-off stood goes
    /// on from its `seam.json`, as in [`Self::settle_worker_seam`]. A message that waits for a
    /// successor was acknowledged, so in an `active` session the successor starts now, with it
    /// (or with a resume on its way, or the next message, when the worker can't go on by itself).
    /// Whether it starts: its cut-off turn then has nothing more to tell.
    pub(crate) async fn recover_worker_seam(&self, task: &Task, active: bool) -> bool {
        self.settle_worker_seam(task).await;
        let resuming = task
            .plain
            .as_ref()
            .is_some_and(|plain| plain.resume_input.is_some());
        let waiting = !self.task_live(task).state.lock().await.seam_held.is_empty();
        // Its session never heard the work's turn end: the work goes on by itself, as a thread's.
        let working = matches!(
            task.state,
            TaskState::Running | TaskState::Starting | TaskState::Blocked
        );
        if !active
            || !(waiting || working)
            || resuming
            || explicitly_paused(task)
            || !self.worker_seam_closed(task).await
        {
            return false;
        }
        let (manager, task) = (self.arc(), task.clone());
        self.spawn(async move { manager.start_restored_successor(&task).await });
        true
    }

    /// Starts the successor that takes what a restart restored, unless the worker moved on.
    async fn start_restored_successor(&self, task: &Task) {
        let live = self.task_live(task);
        let _managed = live.manage.lock().await;
        let _settled = live.settle.lock().await;
        let _reroute = live.reroute.lock().await;
        let task = match self.task_by_id(&task.conversation_id, &task.id).await {
            Ok(task) if !task.state.is_final() && !explicitly_paused(&task) => task,
            _ => return,
        };
        if !self.worker_seam_closed(&task).await {
            return;
        }
        let Err(err) = self.worker_successor(&live, &task, Vec::new()).await else {
            return;
        };
        self.no_successor(&task, &err).await;
    }

    /// Its successor couldn't start: what waits is kept for its next message, which tries again.
    /// A cut-off turn nothing goes on from is told as any other is.
    async fn no_successor(&self, task: &Task, err: &Error) {
        tracing::warn!(task = %task.id, error = %err, "could not start a worker's successor");
        if matches!(task.state, TaskState::Running | TaskState::Blocked) {
            let _ = self
                .update_task(&task.conversation_id, &task.id, |t| {
                    t.state = TaskState::Reported;
                    t.blocked_reason = None;
                    t.mockup_drawing = false;
                })
                .await;
            self.plain_cut_off(task).await;
        }
    }

    /// Where its hand-off stood goes on from its `seam.json`. A note turn that was cut off ends
    /// its generation, with the note it wrote or without; a successor that was starting is asked
    /// whether it took its hand-off, and only if it didn't do the messages it carried go again,
    /// with the next successor. What the side file keeps comes back, without what its last
    /// successor carried: the side file empties after that is recorded.
    async fn settle_worker_seam(&self, task: &Task) {
        let record = self.worker_record(task);
        let at = record.clone();
        let Ok((mut file, saved)) = blocking(move || Ok((read_seam(&at), read_held(&at)))).await
        else {
            return;
        };
        let last = file
            .seams
            .last()
            .map(|seam| seam.messages.clone())
            .unwrap_or_default();
        let mut carried = Vec::new();
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
                        messages.clone(),
                    );
                    let unheard = self.unheard(task, saved, &messages).await;
                    self.keep_held(task, &record, unheard).await;
                    self.write_worker_seam(&record, file).await;
                    let _ = self
                        .update_task(&task.conversation_id, &task.id, |t| {
                            t.native_session = Some(successor);
                        })
                        .await;
                    return;
                }
                file.phase = Phase::Closed { end };
                carried = messages;
            }
        }
        let held = without_each(saved, &last);
        // In the order they came, each once. Kept before its phase moves on, so nothing rests on
        // a phase that no longer names it.
        let unheard = self.unheard(task, held, &carried).await;
        let held = carried.into_iter().chain(unheard).collect::<Vec<_>>();
        self.take_rows_held(task, &held).await;
        self.keep_held(task, &record, held).await;
        self.write_worker_seam(&record, file).await;
    }

    /// `saved`, without what `gone` carried and what its CLI already took (its mark says, so a
    /// crash between a send and the side file's emptying sends nothing twice).
    async fn unheard(&self, task: &Task, saved: Vec<String>, gone: &[String]) -> Vec<String> {
        let mut unheard = Vec::new();
        for text in without_each(saved, gone) {
            let took = match mark_in(&text) {
                Some(mark) => self.worker_took(task, mark).await,
                None => false,
            };
            if !took {
                unheard.push(text);
            }
        }
        unheard
    }

    /// A row may have been kept here and not yet left the queue when a crash came: it goes with
    /// the rest of what is kept, not from the queue as well.
    async fn take_rows_held(&self, task: &Task, held: &[String]) {
        if held.is_empty() {
            return;
        }
        let Ok(board) = self.core.board(&task.conversation_id).await else {
            return;
        };
        for row in board
            .queue
            .items
            .iter()
            .filter(|row| row.worker.as_ref() == Some(&task.id))
            .filter(|row| {
                let mark = once_mark(&row.id);
                held.iter().any(|text| text.contains(&mark))
            })
        {
            if let Err(err) = self.core.take_queued(&task.conversation_id, &row.id).await {
                tracing::warn!(task = %task.id, error = %err, "could not take a message kept for its worker out of the queue");
            }
        }
    }

    /// `held` is what the worker's hand-off keeps now, on disk and in memory.
    async fn keep_held(&self, task: &Task, record: &Path, held: Vec<String>) {
        let live = self.task_live(task);
        let mut state = live.state.lock().await;
        save_held(record, &held).await;
        state.seam_held = held;
    }

    /// Keeps `text`, which a hand-off holds, for its successor: on disk before its sender hears
    /// it went, with a once-only mark. A message that can't be kept isn't taken. The caller holds
    /// the state lock, so the hand-off can't end meanwhile.
    pub(super) async fn hold_for_successor(
        &self,
        state: &mut TaskLiveState,
        task: &Task,
        text: String,
    ) -> Result<()> {
        // A queued message has its row's mark already.
        let text = if without_mark(&text) == text {
            marked_once(&text, &uuid::Uuid::new_v4().to_string())
        } else {
            text
        };
        let mut held = state.seam_held.clone();
        held.push(text);
        let (record, saving) = (self.worker_record(task), held.clone());
        blocking(move || write_held(&record, &saving)).await?;
        state.seam_held = held;
        Ok(())
    }

    /// What waits for the worker's next input, from memory or, after a restart that didn't look
    /// (a quota wait goes on by itself), from its side file.
    pub(super) async fn held_for_input(&self, live: &TaskLive, task: &Task) -> Vec<String> {
        let held = live.state.lock().await.seam_held.clone();
        if !held.is_empty() {
            return held;
        }
        let record = self.worker_record(task);
        let at = record.clone();
        let Ok((saved, file)) = blocking(move || Ok((read_held(&at), read_seam(&at)))).await else {
            return Vec::new();
        };
        if saved.is_empty() {
            return Vec::new();
        }
        let last = file
            .seams
            .last()
            .map(|seam| seam.messages.clone())
            .unwrap_or_default();
        let held = self.unheard(task, saved, &last).await;
        self.take_rows_held(task, &held).await;
        self.keep_held(task, &record, held.clone()).await;
        held
    }

    /// The first `count` of what waited went to its CLI.
    pub(super) async fn held_went(&self, live: &TaskLive, task: &Task, count: usize) {
        if count == 0 {
            return;
        }
        let record = self.worker_record(task);
        let mut state = live.state.lock().await;
        let count = count.min(state.seam_held.len());
        state.seam_held.drain(..count);
        save_held(&record, &state.seam_held).await;
    }

    /// Somebody waits on a hand-off that holds messages: when [`hold`] runs out, its turn ends
    /// whatever its CLI is doing, so it never waits on a note turn that hung.
    ///
    /// [`hold`]: crate::manager::seam::SeamConfig::hold
    pub(super) async fn arm_worker_seam_hold(&self, task: &Task) {
        let live = self.task_live(task);
        let timer = {
            let mut state = live.state.lock().await;
            if state.seam_hold_armed || !state.seam.as_ref().is_some_and(SeamLive::holds) {
                return;
            }
            state.seam_hold_armed = true;
            state.seam_hold_timer += 1;
            state.seam_hold_timer
        };
        let hold = self.seam_config().hold;
        let (manager, id, conversation) =
            (self.arc(), task.id.clone(), task.conversation_id.clone());
        self.spawn(async move {
            let seam = loop {
                tokio::time::sleep(hold).await;
                let mut state = live.state.lock().await;
                if state.seam_hold_timer != timer
                    || state.stopping
                    || !state.seam.as_ref().is_some_and(SeamLive::holds)
                {
                    return;
                }
                // Between its turns, or ending one anyway: it isn't hung.
                if !state.busy || state.cutoff.is_some() {
                    continue;
                }
                // Stopping: a turn end that comes now finds the generation taken care of.
                state.stopping = true;
                state.busy = false;
                break state.seam.clone().unwrap();
            };
            manager.hold_ran_out(live, conversation, id, seam).await;
        });
    }

    /// The hold on a hand-off ran out: its generation ends as a failed note turn's does.
    // Not `async`: this is reached from the hand-off's own turn end, through a message, so its
    // future's type is named here rather than inferred.
    fn hold_ran_out(
        self: Arc<Self>,
        live: Arc<TaskLive>,
        conversation: crate::model::ConversationId,
        id: crate::work::TaskId,
        seam: SeamLive,
    ) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async move {
            let Ok(task) = self.task_by_id(&conversation, &id).await else {
                return;
            };
            if task.state.is_final() {
                return;
            }
            tracing::warn!(task = %id, "a worker's hand-off ran past its hold; its generation ends");
            self.worker_seam_turn_end(&live, &task, seam, TurnStatus::Failed)
                .await;
        })
    }

    async fn write_worker_seam(&self, record: &Path, file: SeamFile) {
        let at = record.to_owned();
        if let Err(err) = blocking(move || write_seam(&at, &file)).await {
            tracing::warn!(error = %err, "could not record the worker's hand-off");
        }
    }

    /// Its CLI ended unasked during a hand-off that holds messages: they would wait for nobody.
    /// Its generation closes as after a restart, and the next message starts its successor.
    pub(crate) async fn close_held_seam(&self, live: &TaskLive, task: &Task) {
        let held = {
            let mut state = live.state.lock().await;
            let held = state.seam.as_ref().is_some_and(SeamLive::holds);
            if held {
                state.seam_over();
            }
            held
        };
        if held {
            self.settle_worker_seam(task).await;
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

/// What the worker's hand-off kept in its side file.
fn read_held(record: &Path) -> Vec<String> {
    std::fs::read(record.join(HELD_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// Its side file holds `held`; there is none when nothing waits.
fn write_held(record: &Path, held: &[String]) -> Result<()> {
    let path = record.join(HELD_FILE);
    if held.is_empty() {
        return match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Invalid(err.to_string())),
        };
    }
    std::fs::create_dir_all(record).map_err(|err| Error::Invalid(err.to_string()))?;
    let text = serde_json::to_string_pretty(held).map_err(|err| Error::Invalid(err.to_string()))?;
    write_atomic(&path, text.as_bytes())
}

/// [`write_held`], where a failure can only be logged: what memory holds still goes.
async fn save_held(record: &Path, held: &[String]) {
    let (at, held) = (record.to_owned(), held.to_owned());
    if let Err(err) = blocking(move || write_held(&at, &held)).await {
        tracing::warn!(error = %err, "could not record what a worker's hand-off holds");
    }
}

/// `saved` without one copy of each of `gone`: the same message may have been sent twice.
fn without_each(mut saved: Vec<String>, gone: &[String]) -> Vec<String> {
    for text in gone {
        if let Some(at) = saved.iter().position(|kept| kept == text) {
            saved.remove(at);
        }
    }
    saved
}

/// `input` after what waited for it, in the order they came; a slash command or a skill must stay
/// first to run, so what waited follows those.
pub(super) fn with_held(mut input: TurnInput, held: &[String]) -> TurnInput {
    if held.is_empty() {
        return input;
    }
    let held = held.join("\n\n");
    match input.parts.first_mut() {
        Some(InputPart::Text(text)) if !text.trim_start().starts_with('/') => {
            *text = format!("{held}\n\n{text}");
        }
        _ => input.parts.push(InputPart::Text(held)),
    }
    input
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
