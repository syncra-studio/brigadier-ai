//! "Continue on <other CLI>", asked for (S38c, D46): the seam S38 2b runs when every account
//! of a provider is out, started on request, by the user or by the thread through
//! `brigadier continue-on`. The CLI it runs on writes its hand-off note first when it can
//! still work (the S22 nudge); one at its limit can't, and the session moves at once without
//! one, as 2b does. Either way the thread shows the divider and the successor carries on at
//! once on the other CLI, on the model the user's rules allow (D25). A model or account change
//! within one CLI is no move: it still continues the same session (D14).

use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::{Cli, ConvLive};
use crate::manager::SessionManager;
use crate::manager::seam::Phase;
use crate::model::{ContinueWhen, ConversationId, ConversationKind, Lifecycle, ModelChoice, Setup};

impl SessionManager {
    /// Moves session `id` to another CLI, `to`: on `model` when given, else on the first model
    /// the user's rules allow there (D25). While a turn runs it moves once the turn ends, only
    /// with `after_turn`. Refused, in plain words, when that CLI can't take it or the rules
    /// allow no model there.
    pub async fn continue_on(
        &self,
        id: &ConversationId,
        to: ProviderKind,
        model: Option<ModelChoice>,
        after_turn: bool,
    ) -> Result<(ContinueWhen, ModelChoice), String> {
        let conversation = self.core.conversation(id).map_err(|err| err.to_string())?;
        if conversation.kind != ConversationKind::Session {
            return Err(
                "only a session continues on another CLI; a Chat changes its model in its picker"
                    .into(),
            );
        }
        if conversation.lifecycle == Lifecycle::Archived {
            return Err("the session is archived".into());
        }
        let Some(Setup::Session { orchestrator, .. }) = &conversation.setup else {
            return Err("the session hasn't started yet: pick its model instead".into());
        };
        let conv = self.conv(id).map_err(|err| err.to_string())?;
        let _retrying = conv.retry.lock().await;
        let mut file = self.read_seam_of(id).await;
        if orchestrator.provider == to {
            // Asked back before a move came: it stays.
            if file.moving.take().is_some() {
                self.write_seam_of(id, file)
                    .await
                    .map_err(|err| err.to_string())?;
            }
            return Ok((ContinueWhen::Already, orchestrator.clone()));
        }
        // Asked again for the move already coming: unchanged, unless another model is named.
        if let Some(moving) = file.moving.as_ref().filter(|moving| {
            moving.provider == to && model.as_ref().is_none_or(|model| model == *moving)
        }) {
            return Ok((ContinueWhen::AfterTurn, moving.clone()));
        }
        if let Some(why) = self.agent_unready(to) {
            return Err(why);
        }
        let choice = match model {
            Some(choice) if choice.provider != to => {
                return Err(format!("that model isn't a {} model", to.label()));
            }
            Some(choice) => {
                self.check_choice(&choice).map_err(|err| err.to_string())?;
                choice
            }
            None => self.successor_model(to)?,
        };
        if file.phase != Phase::Live {
            return Err("its context is being handed over already: ask again once the new session has started".into());
        }
        let busy = {
            let state = conv.state.lock().await;
            state.busy || state.hand_over.is_some() || (!state.waiting && state.has_cli_work())
        };
        if busy && !after_turn {
            return Err(format!(
                "a turn is running: it can continue on {} after this turn (`brigadier continue-on {} --after-turn`)",
                to.label(),
                cli_word(to)
            ));
        }
        let request = {
            let state = conv.state.lock().await;
            state
                .waited_request
                .clone()
                .or_else(|| state.request.clone())
        };
        self.prepare_provider_transition(
            &conv,
            orchestrator.clone(),
            choice.clone(),
            crate::work::ProviderTransitionCause::ManualSwitch,
            request,
        )
        .await;
        file.moving = Some(choice.clone());
        self.write_seam_of(id, file)
            .await
            .map_err(|err| err.to_string())?;
        crate::logging::private_trace!(conv.private_logs, info, "asked to continue on another CLI"; conversation = %id, provider = ?to);
        if busy {
            return Ok((ContinueWhen::AfterTurn, choice));
        }
        let cli = conv.state.lock().await.cli.clone();
        self.begin_move(&conv, cli.as_ref()).await;
        Ok((ContinueWhen::Now, choice))
    }

    /// Why `provider` can't take a session now, in plain words; `None` when it can.
    fn agent_unready(&self, provider: ProviderKind) -> Option<String> {
        let settings = self.core.settings();
        let label = provider.label();
        if !crate::routing::availability::provider_on(&settings, provider) {
            return Some(format!("{label} is turned off in Settings › Providers"));
        }
        if !self.runtime.provider_signed_in(provider) {
            return Some(format!("{label} isn't installed or signed in"));
        }
        crate::accounts::ready(
            &settings,
            provider,
            None,
            &self.runtime.account_candidates(provider),
            &[],
            crate::now_ms(),
        )
        .is_none()
        .then(|| format!("every {label} account is at its usage limit"))
    }

    /// The move asked for starts, with no turn running: the CLI it runs on writes its note
    /// (the seam's own nudge), and the seam that closes it moves the session. A CLI at its limit
    /// can't: the session moves at once without a note, as S38 2b's does.
    pub(super) async fn begin_move(&self, conv: &Arc<ConvLive>, cli: Option<&Arc<Cli>>) {
        let mut file = self.read_seam_of(&conv.id).await;
        let Some(to) = file.moving.clone() else {
            return;
        };
        let Some(from) = self
            .core
            .conversation(&conv.id)
            .ok()
            .and_then(|conversation| super::setup_choice(&conversation))
        else {
            return;
        };
        if from.provider == to.provider || file.phase != Phase::Live {
            return;
        }
        if !self.choice_available(&from).await {
            file.moving = None;
            if let Err(err) = self.write_seam_of(&conv.id, file).await {
                crate::logging::private_trace!(conv.private_logs, warn, "could not record the move"; conversation = %conv.id, error = %err);
                return;
            }
            if conv.state.lock().await.waiting {
                if let Err(err) = self.stop_waiting(conv).await {
                    tracing::error!(error = %err, "could not end quota wait for manual move");
                    return;
                }
            }
            self.seam_to_agent(conv, from.provider, to, Vec::new(), false)
                .await;
            return;
        }
        let used = conv.state.lock().await.context.map_or(0, |(used, _)| used);
        self.seam_nudge(conv, cli, used, false).await;
    }

    /// The move asked for, taken: a usage limit's hand-over goes there (S38c), before the
    /// automatic switch's pick.
    pub(super) async fn take_move(
        &self,
        conv: &ConvLive,
        from: ProviderKind,
    ) -> Option<ModelChoice> {
        let mut file = self.read_seam_of(&conv.id).await;
        let to = file.moving.take()?;
        if let Err(err) = self.write_seam_of(&conv.id, file).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not record the move"; conversation = %conv.id, error = %err);
            return None;
        }
        (to.provider != from).then_some(to)
    }

    /// The session's model becomes `to`, on the other CLI, as its seam closes; answers that
    /// CLI, or `None` when the setup could not change (it then goes on where it was).
    pub(super) async fn move_setup(
        &self,
        conv: &ConvLive,
        to: ModelChoice,
    ) -> Option<ProviderKind> {
        let conversation = self.core.conversation(&conv.id).ok()?;
        let mut setup = conversation.setup?;
        let Setup::Session { orchestrator, .. } = &mut setup else {
            return None;
        };
        if orchestrator.provider == to.provider {
            return None;
        }
        let provider = to.provider;
        *orchestrator = to;
        match self.core.set_setup(conv.id.clone(), setup).await {
            Ok(_) => Some(provider),
            Err(err) => {
                crate::logging::private_trace!(conv.private_logs, warn, "could not move the session to another CLI"; conversation = %conv.id, error = %err);
                None
            }
        }
    }
}

/// How `brigadier continue-on` names a CLI.
pub(crate) fn cli_word(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
    }
}
