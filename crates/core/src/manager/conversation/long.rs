//! A long session's thread compacting in place, the default (D15; [`crate::manager::long_sessions`];
//! run2-D2). Claude compacts when Brigadier asks, at its point at a turn end (run2-D5), with
//! the keep-instructions (run2-D7); its own auto-compaction is the backstop. Codex compacts by
//! itself at its own point (run2-D6). Nothing goes back into the conversation after a compaction
//! (D13): the thread pulls what it needs with `brigadier context`. After
//! [`SESSION_HANDOFF_AFTER`] compactions in one CLI session (D16), or when compaction fails or
//! loops (run2-D10), the session goes on in a fresh one at its seam.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderError, ProviderKind, TurnStatus};
use serde::{Deserialize, Serialize};

use super::{Cli, ConvLive, ConvState};
use crate::manager::SessionManager;
use crate::manager::long_sessions::SESSION_HANDOFF_AFTER;
use crate::manager::seam::Phase;
use crate::work::RunState;

/// A long session's state between turns.
#[derive(Debug, Default)]
pub(super) struct LongLive {
    /// A turn ended past the point: the next turn is Brigadier's `/compact` (Claude).
    pub compact_due: bool,
    /// Why the session goes on in a fresh one (run2-D10, D16), until its seam is under way.
    pub fallback: Option<String>,
    /// Its context was too long for a turn and a compaction is to bring it back under: a
    /// second time before a turn completes, the seam takes over (review item 5).
    pub recovering: bool,
    /// The request whose turn was too long for the context: it goes again by itself once
    /// the compaction is done, as plain Claude Code's reactive compaction retries it.
    pub retry_request: Option<String>,
}

/// Claude's stop when its auto-compaction refills the context three times in a row
/// ("Autocompact is thrashing", CLI-INTERNALS §1.1).
pub(crate) fn thrashing(message: &str) -> bool {
    message.to_lowercase().contains("autocompact is thrashing")
}

/// `record/long.json`: the compactions in a row of the CLI generation it names (D16), kept
/// across restarts so a restart doesn't reset the count.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LongFile {
    /// The CLI generation counted (`seam.json`'s); another one counts from none.
    #[serde(default)]
    pub generation: u32,
    #[serde(default)]
    pub compactions: u32,
}

impl LongFile {
    /// One more compaction in generation `generation`: how many it has had in a row.
    pub(crate) fn count(&mut self, generation: u32) -> u32 {
        if self.generation != generation {
            self.generation = generation;
            self.compactions = 0;
        }
        self.compactions += 1;
        self.compactions
    }
}

fn long_path(record: &Path) -> PathBuf {
    record.join("long.json")
}

pub(crate) fn read_long(record: &Path) -> LongFile {
    std::fs::read(long_path(record))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn write_long(record: &Path, file: &LongFile) {
    let written = serde_json::to_vec_pretty(file)
        .map_err(|err| crate::Error::Invalid(err.to_string()))
        .and_then(|bytes| crate::manager::session_dir::write_atomic(&long_path(record), &bytes));
    if let Err(err) = written {
        tracing::warn!(error = %err, "could not record the compactions");
    }
}

/// Told when the session goes on in a fresh one (run2-D10), with why: one of several failures.
fn fallback_notice(why: &str) -> String {
    format!(
        "This session goes on in a fresh one, which picks up from its hand-off note, because {why}."
    )
}

impl SessionManager {
    /// A turn of a long session's CLI is ending: past the point, with Claude, its next turn
    /// compacts (run2-D5). Not after a compaction turn: its size is the one before. A hand-off
    /// still owed is asked for again.
    pub(super) async fn long_turn_end(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        status: TurnStatus,
    ) {
        if self.long_mode(conv, cli.provider).is_none() {
            return;
        }
        {
            // A turn that went through after a recovering compaction: recovered.
            let mut state = conv.state.lock().await;
            if status == TurnStatus::Completed && !state.compacting && state.turn_error.is_none() {
                state.long.recovering = false;
            }
        }
        let owed = conv.state.lock().await.long.fallback.clone();
        if let Some(why) = owed {
            self.long_fallback(conv, cli, &why).await;
        }
        if cli.provider != ProviderKind::Claude {
            return;
        }
        let mut state = conv.state.lock().await;
        if state.compacting {
            return;
        }
        if let Some((used, window)) = state.context
            && used >= self.seam_config().threshold(window, None)
        {
            state.long.compact_due = true;
        }
    }

    /// The compaction due before anything else the next turn would take: the CLI to compact,
    /// with the turn begun as one of its own for no request.
    pub(super) fn take_due_compaction(state: &mut ConvState) -> Option<Arc<Cli>> {
        if !std::mem::take(&mut state.long.compact_due) {
            return None;
        }
        let cli = state.cli.clone().filter(|cli| cli.session.can_compact())?;
        state.busy = true;
        state.interrupted = false;
        state.compacting = true;
        state.request = None;
        state.turn_error = None;
        state.unshown_error = None;
        state.limit_hit = None;
        state.in_turn.clear();
        Some(cli)
    }

    /// Asks `cli` to compact, with what its summary must keep (run2-D7). The turn ends as any
    /// other; messages sent meanwhile wait for the next.
    pub(super) async fn long_compact(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        let instructions = self
            .long_mode(conv, cli.provider)
            .filter(|_| cli.provider == ProviderKind::Claude)
            .and_then(|mode| mode.compact_instructions());
        self.set_run(&conv.id, RunState::Running, None).await;
        // Claude keeps it as the user's message `/compact …`: Brigadier's, never a bubble (L12).
        if cli.provider == ProviderKind::Claude {
            let words = match &instructions {
                Some(instructions) => format!("/compact {instructions}"),
                None => "/compact".to_owned(),
            };
            self.note_sent(&conv.id, &brigadier_providers::TurnInput::text(words), &[]);
        }
        if let Err(err) = cli.session.compact(instructions).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not compact the session"; conversation = %conv.id, error = %err);
            {
                let mut state = conv.state.lock().await;
                state.busy = false;
                state.compacting = false;
            }
            self.set_run(&conv.id, RunState::Idle, None).await;
            self.kick(conv);
        }
    }

    /// The CLI compacted a long session's context: its size is the one after, when the CLI
    /// says, else unknown until it reports one. Nothing goes back into the conversation (D13).
    /// A failed compaction, one that leaves the context near its point, or the
    /// [`SESSION_HANDOFF_AFTER`]th in this CLI session (D16) hands off to a fresh one.
    pub(super) async fn long_compacted(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        tokens_after: Option<i64>,
        error: Option<&str>,
    ) {
        let window = {
            let mut state = conv.state.lock().await;
            let window = state.context.and_then(|(_, window)| window);
            state.context = tokens_after.map(|used| (used, window));
            window
        };
        if let Some(error) = error {
            let why = format!("its compaction failed: {error}");
            return self.long_fallback(conv, cli, &why).await;
        }
        // Still above half its point after compacting, it would compact again almost at once:
        // the churn the seam showed (DIAGNOSIS §1.3). Half is proposed (PROPOSALS §1, point 5)
        // and waits for the test round.
        if let Some(after) = tokens_after
            && after > self.seam_config().threshold(window, None) / 2
        {
            return self
                .long_fallback(
                    conv,
                    cli,
                    "its context stayed above half its point after compacting",
                )
                .await;
        }
        let generation = self.read_seam_of(&conv.id).await.generation;
        let record = self.session_dir(&conv.id).record();
        let count = crate::manager::blocking(move || {
            let mut file = read_long(&record);
            let count = file.count(generation);
            write_long(&record, &file);
            Ok(count)
        })
        .await
        .unwrap_or(0);
        if count >= SESSION_HANDOFF_AFTER {
            let why = format!("it compacted {count} times in a row");
            self.long_handoff(conv, cli, &why, false).await;
        }
    }

    /// A long session's turn failed past what compaction handles. Claude's thrash stop: the
    /// seam. A context too long for the turn: the CLI compacts first, and only when that
    /// doesn't bring it back under does the seam take the request on (review item 5).
    pub(super) async fn long_error(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        error: &ProviderError,
    ) {
        if thrashing(&error.message) {
            return self
                .long_fallback(conv, cli, "its compaction kept refilling its context")
                .await;
        }
        let mut state = conv.state.lock().await;
        if !std::mem::replace(&mut state.long.recovering, true) {
            state.long.compact_due = true;
            state.long.retry_request = state.request.clone();
        } else {
            drop(state);
            self.seam_too_long(conv).await;
        }
    }

    /// The too-long turn ended: its messages go again, first after the compaction, and its
    /// error is no notice (nothing is asked of the user).
    pub(super) fn long_retry(state: &mut ConvState) {
        state.long.retry_request = None;
        state.unshown_error = None;
        let mut pending = std::mem::take(&mut state.in_turn);
        pending.append(&mut state.pending);
        state.pending = pending;
    }

    /// The rare fallback (run2-D10): the session goes on in a fresh one at its seam, from its
    /// hand-off note. Told once; asked again at each turn's end until the seam is under way
    /// (it waits for running commands and open permissions).
    pub(super) async fn long_fallback(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, why: &str) {
        self.long_handoff(conv, cli, why, true).await;
    }

    /// The session goes on in a fresh one at its seam, for `why`; `tell`: the thread shows a
    /// notice saying so (a failure), else only the divider (the D16 count, run2-D44).
    async fn long_handoff(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, why: &str, tell: bool) {
        if self.read_seam_of(&conv.id).await.phase != Phase::Live {
            conv.state.lock().await.long.fallback = None;
            return;
        }
        let first = conv
            .state
            .lock()
            .await
            .long
            .fallback
            .replace(why.to_owned())
            .is_none();
        if first {
            crate::logging::private_trace!(conv.private_logs, info, "the long session goes on in a fresh one"; conversation = %conv.id, why = %why);
        }
        if first && tell {
            self.notice(&conv.id, NoticeLevel::Warning, &fallback_notice(why))
                .await;
        }
        let used = conv.state.lock().await.context.map_or(0, |(used, _)| used);
        let busy = conv.state.lock().await.busy;
        self.seam_nudge(conv, Some(cli), used, busy).await;
    }
}
