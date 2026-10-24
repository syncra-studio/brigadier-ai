//! A long session's thread compacting in place behind `BRIGADIER_LONG_SESSIONS`
//! ([`crate::manager::long_sessions`]; run2-D2, run2-D15). Claude compacts when Brigadier asks,
//! at its point at a turn end (run2-D5), with the keep-instructions in its arm (run2-D7); its
//! own auto-compaction is the backstop. Codex compacts by itself at its own point (run2-D6).

pub(crate) mod checkpoint;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderError, ProviderKind, TurnStatus, restore_mark};
use serde::{Deserialize, Serialize};

use super::{Cli, ConvLive, ConvState};
use crate::manager::SessionManager;
use crate::manager::long_sessions::active_entries;
use crate::manager::long_sessions::checkpoint::restored;
use crate::manager::long_sessions::restore::{lookup_line, received_whole, restoration};
use crate::manager::seam::Phase;
use crate::work::RunState;

/// A long session's state between turns.
#[derive(Debug, Default)]
pub(super) struct LongLive {
    /// A turn ended past the point: the next turn is Brigadier's `/compact` (Claude).
    pub compact_due: bool,
    /// The compaction whose put-back's receipt the turn's end checks (its CLI's id for it).
    pub receipt_due: Option<String>,
    /// Why the session goes on in a fresh one (run2-D10), until its seam is under way.
    pub fallback: Option<String>,
    /// Its context was too long for a turn and a compaction is to bring it back under: a
    /// second time before a turn completes, the seam takes over (review item 5).
    pub recovering: bool,
    /// The request whose turn was too long for the context: it goes again by itself once
    /// the compaction is done, as plain Claude Code's reactive compaction retries it.
    pub retry_request: Option<String>,
    /// The checkpoint request is the next turn (Codex, run2-D9).
    pub ask_due: bool,
    /// The checkpoint request's turn runs: nothing of the user's joins it.
    pub asking: bool,
    /// Claude compacted in this turn (its id; whether Brigadier asked): the checkpoint its
    /// summary carries is kept at the turn's end.
    pub summary_due: Option<(String, bool)>,
}

/// Claude's stop when its auto-compaction refills the context three times in a row
/// ("Autocompact is thrashing", CLI-INTERNALS §1.1).
pub(super) fn thrashing(message: &str) -> bool {
    message.to_lowercase().contains("autocompact is thrashing")
}

/// `record/long.json`: where a long session's put-back stands, so a retry or a restart
/// reconciles with the CLI's own file instead of sending it twice (review item 2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LongFile {
    /// The last put-back: the compaction it follows (the CLI's id) and where it stands.
    #[serde(default)]
    pub put_back: Option<PutBack>,
    /// The user was told once that with hooks off nothing goes back after Claude compacts.
    #[serde(default)]
    pub hooks_off_told: bool,
    /// A Codex checkpoint run under way (run2-D9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<checkpoint::CheckpointRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutBack {
    pub compaction: String,
    /// The entries it carries ([`crate::manager::long_sessions::restore::fingerprint`]).
    pub fingerprint: String,
    pub state: PutBackState,
    /// The checkpoint it carries beside the decisions (Codex, run2-D9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

/// What a put-back carries, each piece whole: the ledger's active entries in session record
/// `record`, then the checkpoint `checkpoint` when it carries one.
fn put_back_entries(record: &Path, checkpoint: Option<&str>) -> Vec<String> {
    let mut entries = active_entries(record);
    entries.extend(checkpoint.map(restored));
    entries
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum PutBackState {
    /// About to go, or gone with no answer: the CLI's file says whether it arrived.
    Sending,
    /// The CLI took it; its file hasn't shown it yet.
    Sent,
    /// The CLI's file holds every entry whole, once.
    Received,
    /// It could not go back whole: the session went on in a fresh one.
    Failed,
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

pub(super) fn write_long(record: &Path, file: &LongFile) {
    let written = serde_json::to_vec_pretty(file)
        .map_err(|err| crate::Error::Invalid(err.to_string()))
        .and_then(|bytes| crate::manager::session_dir::write_atomic(&long_path(record), &bytes));
    if let Err(err) = written {
        tracing::warn!(error = %err, "could not record the put-back");
    }
}

/// Codex's put-back after compaction `compaction`: the parts as one note, marked so its
/// receipt finds it (step 0: one note of 75,995 characters came back whole).
fn codex_note(parts: &[String], compaction: &str, fingerprint: &str) -> String {
    format!(
        "{}\n<!-- {} {fingerprint} -->",
        parts.join("\n\n"),
        restore_mark(compaction)
    )
}

/// What the thread is told when the decisions could not go back whole (no notice when they
/// did: the divider is enough, D13).
const NOT_RESTORED: &str = "The session's decisions could not be put back whole after it \
compacted, so it goes on in a fresh session that carries them.";

/// Told once, with Claude's hooks off in the user's settings (G17: their setting wins).
const HOOKS_OFF: &str = "Claude's hooks are off in your settings, so after Claude compacts \
this session its decisions are not put back word for word; its summary carries them.";

impl SessionManager {
    /// A turn of a long session's CLI is ending: past the point, with Claude, its next turn
    /// compacts (run2-D5). Not after a compaction turn: its size is the one before.
    pub(super) async fn long_turn_end(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        status: TurnStatus,
    ) {
        let Some(mode) = self.long_mode(conv, cli.provider) else {
            return;
        };
        let summary_due = conv.state.lock().await.long.summary_due.take();
        if let Some((compaction, ours)) = summary_due.filter(|_| mode.checkpoint) {
            self.checkpoint_from_summary(conv, cli, &compaction, ours);
        }
        {
            // A turn that went through after a recovering compaction: recovered.
            let mut state = conv.state.lock().await;
            if status == TurnStatus::Completed && !state.compacting && state.turn_error.is_none() {
                state.long.recovering = false;
            }
        }
        self.long_receipts(conv, cli).await;
        if let Some(mode) = self.long_mode(conv, cli.provider) {
            self.checkpoint_turn_end(conv, cli, mode).await;
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

    /// Asks `cli` to compact, with what its summary must keep in this arm (run2-D7). The turn
    /// ends as any other; messages sent meanwhile wait for the next.
    pub(super) async fn long_compact(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        let instructions = self
            .long_mode(conv, cli.provider)
            .filter(|_| cli.provider == ProviderKind::Claude)
            .and_then(|mode| mode.compact_instructions());
        self.set_run(&conv.id, RunState::Running, None).await;
        if let Err(err) = cli.session.compact(instructions).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not compact the session"; conversation = %conv.id, error = %err);
            {
                let mut state = conv.state.lock().await;
                state.busy = false;
                state.compacting = false;
            }
            // A checkpoint run waiting on it ends: the checkpoint stays on record.
            let record = self.session_dir(&conv.id).record();
            let mut file = read_long(&record);
            if file.checkpoint.take().is_some() {
                write_long(&record, &file);
            }
            self.set_run(&conv.id, RunState::Idle, None).await;
            self.kick(conv);
        }
    }

    /// The CLI compacted a long session's context: its size is the one after, when the CLI
    /// says, else unknown until it reports one. Its decisions go back (run2-D8): Claude's
    /// through its hooks, whose receipt the turn's end reads; Codex's as one note now, even
    /// mid-turn.
    pub(super) async fn long_compacted(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        tokens_after: Option<i64>,
        compaction: Option<&str>,
        error: Option<&str>,
    ) {
        let window = {
            let mut state = conv.state.lock().await;
            let window = state.context.and_then(|(_, window)| window);
            state.context = tokens_after.map(|used| (used, window));
            if let Some(compaction) = compaction.filter(|_| cli.provider == ProviderKind::Claude) {
                state.long.summary_due = Some((compaction.to_owned(), state.compacting));
            }
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
        let Some(mode) = self.long_mode(conv, cli.provider) else {
            return;
        };
        let Some(compaction) = compaction.filter(|_| mode.restore) else {
            return;
        };
        match cli.provider {
            ProviderKind::Codex => {
                let record = self.session_dir(&conv.id).record();
                let checkpoint = if mode.checkpoint {
                    self.checkpoint_compacted(conv, &record).await
                } else {
                    None
                };
                let manager = self.arc();
                let (conv, cli, compaction) = (conv.clone(), cli.clone(), compaction.to_owned());
                // Beside the event loop: the CLI's answer comes through it.
                self.spawn(
                    async move { manager.put_back(&conv, &cli, &compaction, checkpoint).await },
                );
            }
            _ => conv.state.lock().await.long.receipt_due = Some(compaction.to_owned()),
        }
    }

    /// Puts the decisions back after Codex's compaction `compaction`, exactly once: before
    /// sending, and before any retry, the rollout says whether it already arrived.
    pub(super) async fn put_back(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        compaction: &str,
        checkpoint: Option<String>,
    ) {
        let record = self.session_dir(&conv.id).record();
        let entries = put_back_entries(&record, checkpoint.as_deref());
        let back = restoration(&entries, &lookup_line());
        let mut file = read_long(&record);
        if file
            .put_back
            .as_ref()
            .is_some_and(|put| put.compaction == compaction && put.state == PutBackState::Received)
        {
            return;
        }
        let set = |file: &mut LongFile, state| {
            file.put_back = Some(PutBack {
                compaction: compaction.to_owned(),
                fingerprint: back.fingerprint.clone(),
                state,
                checkpoint: checkpoint.clone(),
            });
            write_long(&record, file);
        };
        if !back.whole {
            set(&mut file, PutBackState::Failed);
            self.long_fallback(conv, cli, "its decisions are too long to go back whole")
                .await;
            return;
        }
        let note = codex_note(&back.parts, compaction, &back.fingerprint);
        // Sent at most twice, each time only if the rollout doesn't hold it yet.
        const SENDS: usize = 2;
        for attempt in 0..=SENDS {
            match self.receipt(cli, compaction, &entries) {
                Receipt::Whole => return set(&mut file, PutBackState::Received),
                Receipt::Broken => break,
                Receipt::None => {}
            }
            if attempt == SENDS {
                break;
            }
            set(&mut file, PutBackState::Sending);
            match cli.session.put_back(note.clone()).await {
                Ok(()) => {
                    // Codex writes it to its rollout as it takes it; a receipt not there yet
                    // is read again at the turn's end.
                    if self.receipt(cli, compaction, &entries) == Receipt::Whole {
                        return set(&mut file, PutBackState::Received);
                    }
                    set(&mut file, PutBackState::Sent);
                    conv.state.lock().await.long.receipt_due = Some(compaction.to_owned());
                    return;
                }
                Err(err) => {
                    crate::logging::private_trace!(conv.private_logs, warn, "could not put the decisions back"; conversation = %conv.id, error = %err);
                }
            }
        }
        set(&mut file, PutBackState::Failed);
        self.long_fallback(conv, cli, "its decisions could not be put back whole")
            .await;
    }

    /// Whether the put-back after `compaction` reached the model: every entry whole, once.
    fn receipt(&self, cli: &Arc<Cli>, compaction: &str, entries: &[String]) -> Receipt {
        let received = match cli.session.put_back_received(compaction) {
            Ok(received) => received,
            Err(err) => {
                tracing::debug!(error = %err, "could not read the put-back's receipt");
                return Receipt::None;
            }
        };
        if received.is_empty() {
            Receipt::None
        } else if received_whole(entries, &received.join("\n"))
            && (cli.provider != ProviderKind::Codex || received.len() == 1)
        {
            Receipt::Whole
        } else {
            Receipt::Broken
        }
    }

    /// A long session's CLI started (or resumed): a put-back a restart cut off is reconciled
    /// with its file, and sent only if it never arrived.
    pub(super) async fn long_started(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        let Some(mode) = self.long_mode(conv, cli.provider) else {
            return;
        };
        if cli.provider != ProviderKind::Codex {
            return;
        }
        let record = self.session_dir(&conv.id).record();
        if let Some(put) = read_long(&record).put_back
            && matches!(put.state, PutBackState::Sending | PutBackState::Sent)
        {
            let manager = self.arc();
            let (conv, cli) = (conv.clone(), cli.clone());
            self.spawn(async move {
                manager
                    .put_back(&conv, &cli, &put.compaction, put.checkpoint)
                    .await
            });
        }
        self.checkpoint_started(conv, mode).await;
    }

    /// The put-back's receipt at a turn's end, and the fallback still owed.
    async fn long_receipts(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        let due = conv.state.lock().await.long.receipt_due.take();
        if let Some(compaction) = due {
            let record = self.session_dir(&conv.id).record();
            if cli.provider == ProviderKind::Claude && !cli.session.runs_hooks() {
                let mut file = read_long(&record);
                if !file.hooks_off_told {
                    file.hooks_off_told = true;
                    write_long(&record, &file);
                    self.notice(&conv.id, NoticeLevel::Info, HOOKS_OFF).await;
                }
            } else {
                let mut file = read_long(&record);
                // What Codex's put-back carried beside the decisions.
                let checkpoint = file
                    .put_back
                    .take()
                    .filter(|put| put.compaction == compaction)
                    .and_then(|put| put.checkpoint);
                let entries = put_back_entries(&record, checkpoint.as_deref());
                let receipt = self.receipt(cli, &compaction, &entries);
                let fingerprint = crate::manager::long_sessions::restore::fingerprint(&entries);
                let state = match receipt {
                    Receipt::Whole => PutBackState::Received,
                    _ => PutBackState::Failed,
                };
                file.put_back = Some(PutBack {
                    compaction,
                    fingerprint,
                    state,
                    checkpoint,
                });
                write_long(&record, &file);
                if state == PutBackState::Failed {
                    self.long_fallback(conv, cli, "its decisions did not all arrive whole")
                        .await;
                }
            }
        }
        let owed = conv.state.lock().await.long.fallback.clone();
        if let Some(why) = owed {
            self.long_fallback(conv, cli, &why).await;
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

    /// The rare fallback (run2-D10): the session goes on in a fresh one at its seam, which
    /// carries the decisions whole. Told once; asked again at each turn's end until the seam
    /// is under way (it waits for running commands and open permissions).
    pub(super) async fn long_fallback(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, why: &str) {
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
            self.notice(&conv.id, NoticeLevel::Warning, NOT_RESTORED)
                .await;
        }
        let used = conv.state.lock().await.context.map_or(0, |(used, _)| used);
        let busy = conv.state.lock().await.busy;
        self.seam_nudge(conv, Some(cli), used, busy).await;
    }
}

/// What the CLI's file says of a put-back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Receipt {
    /// Not there.
    None,
    /// Every entry whole, once.
    Whole,
    /// There, but cut, missing an entry or more than once.
    Broken,
}
