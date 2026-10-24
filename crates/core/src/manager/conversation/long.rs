//! A long session's thread compacting in place behind `BRIGADIER_LONG_SESSIONS`
//! ([`crate::manager::long_sessions`]; run2-D2, run2-D15). Claude compacts when Brigadier asks,
//! at its point at a turn end (run2-D5), with the keep-instructions in its arm (run2-D7); its
//! own auto-compaction is the backstop. Codex compacts by itself at its own point (run2-D6).

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::{Cli, ConvLive, ConvState};
use crate::manager::SessionManager;
use crate::work::RunState;

/// A long session's state between turns.
#[derive(Debug, Default)]
pub(super) struct LongLive {
    /// A turn ended past the point: the next turn is Brigadier's `/compact` (Claude).
    pub compact_due: bool,
}

impl SessionManager {
    /// A turn of a long session's CLI is ending: past the point, with Claude, its next turn
    /// compacts (run2-D5). Not after a compaction turn: its size is the one before.
    pub(super) async fn long_turn_end(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) {
        if cli.provider != ProviderKind::Claude || self.long_mode(conv, cli.provider).is_none() {
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
            .and_then(|mode| mode.compact_instructions());
        self.set_run(&conv.id, RunState::Running, None).await;
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
    /// says, else unknown until it reports one.
    pub(super) async fn long_compacted(&self, conv: &Arc<ConvLive>, tokens_after: Option<i64>) {
        let mut state = conv.state.lock().await;
        let window = state.context.and_then(|(_, window)| window);
        state.context = tokens_after.map(|used| (used, window));
    }
}
