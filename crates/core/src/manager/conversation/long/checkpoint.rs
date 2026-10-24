//! A long session's checkpoint at the compactions Brigadier triggers (P2b, run2-D9; the record
//! is [`crate::manager::long_sessions::checkpoint`]). Codex is asked for it in a turn of its
//! own at its point, then compacts (`thread/compact/start`) and gets it back beside the
//! decisions; no turn of the user's comes in between. Claude's steered summary carries it
//! (Claude can't be asked first), and Brigadier keeps that section as the record. A compaction
//! with no fresh checkpoint marks it missing: the one before is stale.
//!
//! Where a Codex run stands is kept in `record/long.json` ([`CheckpointRun`]), so a restart
//! goes on from it: asked → compact without asking again; compacting → compact again (an
//! extra compaction costs little; a lost checkpoint costs the work).

use std::path::Path;
use std::sync::Arc;

use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};

use super::super::{Cli, ConvLive, ConvState, Envelope};
use super::{read_long, write_long};
use crate::manager::SessionManager;
use crate::manager::long_sessions::LongSessions;
use crate::manager::long_sessions::checkpoint::{self, Source};
use crate::now_ms;
use crate::work::InjectionKind;

/// A Codex checkpoint run: asked for, then compacting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckpointRun {
    /// When it was asked for: a checkpoint written since is this compaction's.
    pub since_ms: i64,
    pub phase: CheckpointPhase,
    /// The checkpoint the compaction puts back, once compacting (`None`: none was written).
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CheckpointPhase {
    /// The thread is asked to write it, in a turn of its own.
    Asked,
    /// Codex is to compact, or compacting.
    Compacting,
}

/// The checkpoint written since `since_ms`; none, marked missing.
fn fresh(record: &Path, since_ms: i64) -> Option<String> {
    let text = checkpoint::written_since(record, since_ms);
    if text.is_none() {
        missing(record);
    }
    text
}

/// Records that a compaction found no fresh checkpoint.
fn missing(record: &Path) {
    if let Err(err) = checkpoint::mark_missing(record) {
        tracing::warn!(error = %err, "could not mark the checkpoint missing");
    }
}

impl SessionManager {
    /// A Codex turn of a checkpoint arm ended. The checkpoint turn's end: Codex compacts next,
    /// with the checkpoint or, none written, without it (marked missing). Brigadier's
    /// compaction turn's end with no compaction: the run ends. Past the point: the next turn
    /// asks for the checkpoint.
    pub(in crate::manager::conversation) async fn checkpoint_turn_end(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        mode: LongSessions,
    ) {
        if !mode.checkpoint || cli.provider != ProviderKind::Codex {
            return;
        }
        let record = self.session_dir(&conv.id).record();
        let mut state = conv.state.lock().await;
        let mut file = read_long(&record);
        if std::mem::take(&mut state.long.asking) {
            // Codex compacted by itself during the turn: that compaction took the run.
            let Some(run) = file
                .checkpoint
                .as_mut()
                .filter(|run| run.phase == CheckpointPhase::Asked)
            else {
                return;
            };
            run.text = fresh(&record, run.since_ms);
            run.phase = CheckpointPhase::Compacting;
            write_long(&record, &file);
            state.long.compact_due = true;
            return;
        }
        if state.compacting {
            if file.checkpoint.take().is_some() {
                write_long(&record, &file);
            }
            return;
        }
        if file.checkpoint.is_some() || state.long.fallback.is_some() {
            return;
        }
        if let Some((used, window)) = state.context
            && used >= self.seam_config().threshold(window, None)
        {
            file.checkpoint = Some(CheckpointRun {
                since_ms: now_ms(),
                phase: CheckpointPhase::Asked,
                text: None,
            });
            write_long(&record, &file);
            state.long.ask_due = true;
        }
    }

    /// The checkpoint request due as a turn of its own, taken for the next turn; while it
    /// runs, nothing of the user's joins it.
    pub(in crate::manager::conversation) fn take_due_ask(
        state: &mut ConvState,
    ) -> Option<Envelope> {
        if !std::mem::take(&mut state.long.ask_due) {
            return None;
        }
        state.long.asking = true;
        Some(Envelope {
            kind: InjectionKind::Seam,
            label: "checkpoint".into(),
            task_id: None,
            text: checkpoint::ask(),
        })
    }

    /// Codex compacted a checkpoint arm's session: the checkpoint that goes back beside the
    /// decisions. Brigadier's compaction: the one its run took. Codex's own during the
    /// checkpoint turn: the one written by now, and Brigadier's compaction is called off.
    /// Codex's own otherwise: none, and the one before is stale.
    pub(in crate::manager::conversation) async fn checkpoint_compacted(
        &self,
        conv: &Arc<ConvLive>,
        record: &Path,
    ) -> Option<String> {
        let mut file = read_long(record);
        let run = file.checkpoint.take();
        let text = match &run {
            // Its checkpoint turn's end already marked one not written.
            Some(run) if run.phase == CheckpointPhase::Compacting => run.text.clone(),
            Some(run) => {
                let mut state = conv.state.lock().await;
                state.long.ask_due = false;
                state.long.compact_due = false;
                drop(state);
                fresh(record, run.since_ms)
            }
            None => {
                missing(record);
                None
            }
        };
        if run.is_some() {
            write_long(record, &file);
        }
        text
    }

    /// A Codex checkpoint arm's CLI started: a run a restart cut off goes on. Asked: Codex
    /// compacts with what was written (the request isn't sent again). Compacting: it compacts.
    pub(in crate::manager::conversation) async fn checkpoint_started(
        &self,
        conv: &Arc<ConvLive>,
        mode: LongSessions,
    ) {
        if !mode.checkpoint {
            return;
        }
        let record = self.session_dir(&conv.id).record();
        let mut file = read_long(&record);
        let Some(run) = file.checkpoint.as_mut() else {
            return;
        };
        if run.phase == CheckpointPhase::Asked {
            run.text = fresh(&record, run.since_ms);
            run.phase = CheckpointPhase::Compacting;
            write_long(&record, &file);
        }
        {
            let mut state = conv.state.lock().await;
            state.long.ask_due = false;
            state.long.asking = false;
            state.long.compact_due = true;
        }
        self.kick(conv);
    }

    /// A Claude checkpoint arm compacted: Brigadier's steered summary carries the checkpoint,
    /// kept as the record; its own compaction carries none (the one before is stale).
    pub(in crate::manager::conversation) fn checkpoint_from_summary(
        &self,
        conv: &ConvLive,
        cli: &Cli,
        compaction: &str,
        ours: bool,
    ) {
        let record = self.session_dir(&conv.id).record();
        let section = ours
            .then(|| cli.session.compact_summary(compaction))
            .and_then(|summary| {
                summary
                    .inspect_err(|err| tracing::debug!(error = %err, "could not read the summary"))
                    .ok()
                    .flatten()
            })
            .and_then(|summary| checkpoint::summary_section(&summary));
        let kept = section.is_some_and(|text| {
            checkpoint::set(&record, &text, Source::Summary)
                .inspect_err(|err| tracing::warn!(error = %err, "could not keep the checkpoint"))
                .is_ok()
        });
        if !kept {
            missing(&record);
        }
    }
}
