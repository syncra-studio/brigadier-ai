//! The thread's in-turn overflow guard (BUILD-PLAN D2.3b). The thread does the work itself
//! (Q35), so one long turn can reach its model's limit before it ends, and rebirth was only
//! considered between turns.
//!
//! - **Steer.** The first context report past the steer line, mid-turn, asks the thread to
//!   commit what it has finished; again after each further stretch of growth.
//! - **Hard line.** Past [`knowledge::hard_line`], or when its CLI begins compacting (refused by
//!   the session's hook), or on "prompt is too long", the turn is stopped and the thread is
//!   reborn at once: a fresh session from a briefing that names the step it was in, the last
//!   commit and the whole transcript (outputs kept whole, D2.1), continues the request. The
//!   request never fails for it.
//! - **Durable.** The stop is recorded ([`OrchestratorEntry::StoppedForRebirth`]) before the
//!   CLI is stopped. A restart before the rebirth finishes it and continues the request
//!   instead of failing it as cut off.

use std::sync::Arc;

use brigadier_providers::{ProviderKind, TurnInput};

use super::{Cli, ConvLive};
use crate::knowledge::{self, RebirthTrigger};
use crate::manager::SessionManager;
use crate::manager::rebirth::{BriefingPlan, HANDOFF_AFTER_CLOSE, HandoffPurpose};
use crate::model::{ConversationId, DomainEvent, Message, streams};
use crate::routing::WriteCause;
use crate::work::{InjectionKind, OrchestratorEntry, RequestState, RunState};
use crate::{knowledge::rebirth_thresholds, now_ms};

/// The steer that asks the thread, mid-turn, to commit what it has.
fn steer_text(tokens: i64, hard: i64) -> String {
    format!(
        "[Brigadier] Your context is now about {}k tokens, near this model's limit. Commit the \
         finished steps of this request now (not broken work), and keep each next step small. \
         If this turn grows past about {}k tokens, Brigadier stops it and continues the request \
         in a fresh session from a briefing, so keep the workspace where a fresh session can \
         pick it up. Don't stop to tell the user about this.",
        tokens / 1_000,
        hard / 1_000
    )
}

/// Where a thread on `provider` with `window` is first asked to commit: at its rebirth swap
/// threshold, and a tenth below the hard line at the latest.
pub(super) fn steer_line(provider: ProviderKind, window: Option<i64>) -> i64 {
    let hard = knowledge::hard_line(provider, window);
    rebirth_thresholds(provider, window)
        .swap_tokens
        .min(hard - hard / 10)
}

/// Why the thread's turn is stopped for a rebirth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StopWhy {
    /// Its context passed the hard line.
    HardLine,
    /// Its CLI began compacting and the session's hook refused it.
    CompactionRefused,
    /// Its model refused the context ("prompt is too long"): the turn already failed.
    ContextWindow,
}

impl StopWhy {
    fn words(self, tokens: i64) -> String {
        match self {
            Self::HardLine => format!(
                "its context passed the hard line at about {}k tokens",
                tokens / 1_000
            ),
            Self::CompactionRefused => "its CLI began compacting its context".into(),
            Self::ContextWindow => "its model refused the context as too long".into(),
        }
    }
}

impl SessionManager {
    /// A session thread reported its context size mid-turn: past the steer line it is asked
    /// to commit; past the hard line its turn stops for a rebirth.
    pub(super) async fn thread_context(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        tokens: i64,
        window: Option<i64>,
    ) {
        // Without its model's window the lines aren't known: a guess (the 200k default) would
        // stop a big-window thread far too early. Its model refusing the context, or its CLI
        // beginning to compact, still rebirths it.
        if window.is_none() {
            return;
        }
        let hard = knowledge::hard_line(cli.provider, window);
        if tokens >= hard {
            self.stop_for_rebirth(conv, cli, tokens, StopWhy::HardLine)
                .await;
            return;
        }
        let line = steer_line(cli.provider, window);
        if tokens < line {
            return;
        }
        let stretch = knowledge::steer_stretch(line, hard);
        let steer = {
            let mut state = conv.state.lock().await;
            let due = state.busy
                && state.stopping_for_rebirth.is_none()
                && state.steered_at.is_none_or(|at| tokens >= at + stretch);
            if due {
                state.steered_at = Some(tokens);
            }
            due
        };
        if !steer {
            return;
        }
        crate::logging::private_trace!(conv.private_logs, info, "asking the thread to commit near its limit"; conversation = %conv.id, tokens);
        // Not from inside the CLI's own event pump: a steer may wait on the CLI.
        let cli = cli.clone();
        self.spawn(async move {
            if let Err(err) = cli
                .session
                .steer(TurnInput::text(steer_text(tokens, hard)))
                .await
            {
                tracing::warn!(error = %err, "could not ask the thread to commit");
            }
        });
    }

    /// Stops the thread's running turn for a rebirth: recorded first, then the turn is
    /// interrupted (unless it already failed). Its end continues the request in a fresh
    /// session ([`Self::rebirth_mid_turn`]).
    /// Whether the stop began: not when one already did, when the thread isn't working, or
    /// when this session is itself a fresh one continuing the request that hasn't made a call
    /// yet (stopping it again would never end).
    pub(super) async fn stop_for_rebirth(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        tokens: i64,
        why: StopWhy,
    ) -> bool {
        let request = {
            let mut state = conv.state.lock().await;
            if state.stopping_for_rebirth.is_some() || !state.busy {
                return false;
            }
            if state.reborn_unproven.is_some() && state.reborn_unproven == state.request {
                tracing::warn!(conversation = %conv.id, tokens, ?why, "a fresh session reached its model's limit before any call: not reborn again");
                return false;
            }
            state.stopping_for_rebirth = Some(tokens);
            state.request.clone()
        };
        tracing::info!(conversation = %conv.id, tokens, ?why, "stopping the thread's turn for a rebirth");
        self.log_orchestrator(
            &conv.id,
            OrchestratorEntry::StoppedForRebirth {
                request_id: request,
                at_tokens: tokens,
                reason: why.words(tokens),
            },
        )
        .await;
        #[cfg(test)]
        {
            let pause = conv.rebirth_stop_pause.lock().unwrap().clone();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        if why != StopWhy::ContextWindow {
            // Not from inside the CLI's own event pump.
            let cli = cli.clone();
            self.spawn(async move {
                if let Err(err) = cli.session.interrupt().await {
                    tracing::warn!(error = %err, "could not stop the thread's turn");
                }
            });
        }
        true
    }

    /// The turn stopped for a rebirth is over: the CLI closes, and the next turn starts a
    /// fresh session from a briefing that says where the stopped one stood, with the turn's
    /// messages sent again (the fresh session never had them).
    pub(super) async fn rebirth_mid_turn(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        tokens: i64,
        carried: Vec<Message>,
    ) {
        conv.restarting(WriteCause::Rebirth).await;
        conv.close_cli().await;
        let stopped = self
            .stopped_text(&conv.id, cli.provider, tokens, None)
            .await;
        // A note written earlier at the prepare threshold still helps (one still being written
        // is of an older context); else the closed session writes one now in a fork, as a
        // swap's does, so what it checked and found outlives the verbatim tail. No turn starts
        // meanwhile: the hand-over holds them.
        let (earlier, window) = {
            let mut state = conv.state.lock().await;
            let window = state.context.and_then(|(_, window)| window);
            (state.rebirth.take().filter(|prep| prep.ready()), window)
        };
        let prep = match earlier {
            Some(prep) => prep,
            None => {
                let prep = self.prepare_rebirth(
                    &conv.id,
                    cli.provider,
                    cli.model.clone(),
                    Some(cli.session.native_id()),
                    (tokens, window),
                    HandoffPurpose::Threshold,
                );
                prep.closed();
                prep.note(HANDOFF_AFTER_CLOSE).await;
                prep
            }
        };
        let user_stopped = {
            let mut state = conv.state.lock().await;
            state.checkpoint = None;
            state.context = None;
            state.fresh = true;
            state.reseed = false;
            state.briefing = Some(BriefingPlan {
                trigger: RebirthTrigger::MidTurn,
                prep: Some(prep),
                at_tokens: tokens,
                window,
                swap_started_at_ms: now_ms(),
                stopped: Some(stopped),
            });
            let over = state.hand_over.take();
            let user_stopped = over.as_ref().is_some_and(|over| over.stopped);
            state.reborn_unproven = over.and_then(|over| over.request).filter(|_| !user_stopped);
            if !user_stopped {
                let mut pending = carried;
                pending.append(&mut state.pending);
                state.pending = pending;
                state.carry_peers();
            }
            user_stopped
        };
        // Stopped by the user meanwhile: its turn is over, as a stopped turn's is.
        if user_stopped {
            self.set_run(&conv.id, RunState::Idle, None).await;
        }
        self.kick(conv);
    }

    /// The briefing's first part after a stop: where the stopped turn stood, the last commit,
    /// and the stopped session's whole transcript, exported now with every output whole. After
    /// a restart the request's message isn't sent again, so `request` is quoted here.
    async fn stopped_text(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        tokens: i64,
        request: Option<&str>,
    ) -> String {
        let dir = self.owned_dir("orch", &id.0).join("stopped");
        let export = self.export_thread_transcript(id, &dir).await;
        let commit = self.thread_last_commit(id).await;
        let mut text = format!(
            "[Where your earlier session stopped]\nBrigadier stopped your earlier session's turn \
             at about {}k tokens, near the {provider} model's limit, in the middle of the \
             request below, and you continue it here in a fresh session. Carry on with the \
             request without asking the user; don't mention the restart.",
            tokens / 1_000
        );
        if let Some(request) = request
            && let Some(words) = self.request_words(id, request).await
        {
            text.push_str(&format!("\n- The request, in the user's words: {words}"));
        }
        if let Some(step) = &export.last_step {
            text.push_str(&format!("\n- The step it was in: {step}."));
        }
        match commit {
            Some(commit) => text.push_str(&format!("\n- The last commit: {commit}.")),
            None => text.push_str("\n- The last commit: unknown."),
        }
        text.push_str(&format!(
            "\n- Its whole transcript: {} ({}). Search it with rg for any detail rather than \
             redoing work.\n- Check git status and git diff first: the step may be half done. \
             Keep what is right; don't redo what is committed.",
            dir.join("transcript.md").display(),
            export.summary("it")
        ));
        text
    }

    /// The words of the user's message that started `request`.
    async fn request_words(&self, id: &ConversationId, request: &str) -> Option<String> {
        let head = self.core.head(id).await.ok()??;
        let branch = self.core.branch(id, &head).await.ok()?;
        let message = branch.into_iter().find(|message| message.id == request)?;
        let text = match &message.blob {
            Some(blob) => self
                .core
                .read_blob_text(blob.clone())
                .await
                .unwrap_or(message.text),
            None => message.text,
        };
        Some(text)
    }

    /// The thread's workspace's last commit, in a line.
    async fn thread_last_commit(&self, id: &ConversationId) -> Option<String> {
        let workspace = self.effective_workspace(id).await.ok()??;
        self.last_commit(std::path::PathBuf::from(&workspace.path))
            .await
    }

    /// After a restart: a thread whose turn was stopped for a rebirth that never happened
    /// (the daemon went down in between) is reborn now, and its request goes on rather than
    /// failing as cut off. Answers whether it was.
    pub(crate) async fn recover_stopped_rebirth(&self, id: &ConversationId) -> bool {
        let Some((request, tokens)) = self.unfinished_stop(id).await else {
            return false;
        };
        let Ok(board) = self.core.board(id).await else {
            return false;
        };
        let live = board.requests.get(&request).is_some_and(|known| {
            matches!(known.state, RequestState::Working | RequestState::Waiting)
        });
        if !live {
            return false;
        }
        // The cut-off turn is this one: it isn't failed.
        let _ = self.core.take_cut_off_request(id).await;
        let Ok(conv) = self.conv(id) else {
            return false;
        };
        let provider = self
            .core
            .conversation(id)
            .ok()
            .and_then(|conversation| super::setup_choice(&conversation))
            .map_or(ProviderKind::Claude, |choice| choice.provider);
        let stopped = self
            .stopped_text(id, provider, tokens, Some(&request))
            .await;
        {
            let mut state = conv.state.lock().await;
            state.fresh = true;
            state.reseed = false;
            state.reborn_unproven = Some(request.clone());
            state.briefing = Some(BriefingPlan {
                trigger: RebirthTrigger::MidTurn,
                prep: None,
                at_tokens: tokens,
                window: None,
                swap_started_at_ms: now_ms(),
                stopped: Some(stopped),
            });
        }
        tracing::info!(conversation = %id, "a thread stopped for a rebirth before the restart is reborn now");
        let envelope = super::Envelope {
            kind: InjectionKind::Resume,
            label: "continue after a stop".into(),
            task_id: None,
            text: "[Continue the request below from where your earlier session stopped.]".into(),
        };
        self.deliver_for(id, envelope, Some(request)).await;
        true
    }

    /// The newest stop for a rebirth with no rebirth after it: its request and context.
    async fn unfinished_stop(&self, id: &ConversationId) -> Option<(String, i64)> {
        let page = self
            .core
            .store()
            .read_stream(
                streams::orchestrator(id),
                brigadier_store::StreamPage {
                    before: None,
                    kinds: vec!["orchestrator.logged".into()],
                    limit: 500,
                },
            )
            .await
            .ok()?;
        for stored in &page {
            match serde_json::from_str::<DomainEvent>(stored.payload.get()) {
                Ok(DomainEvent::OrchestratorLogged {
                    entry: OrchestratorEntry::Rebirth { .. },
                    ..
                }) => return None,
                Ok(DomainEvent::OrchestratorLogged {
                    entry:
                        OrchestratorEntry::StoppedForRebirth {
                            request_id,
                            at_tokens,
                            ..
                        },
                    ..
                }) => return Some((request_id?, at_tokens)),
                _ => {}
            }
        }
        None
    }
}
