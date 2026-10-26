//! User-started Fusion in top-level Chats. Offers record a card, never a consult.

use super::SessionManager;
use super::conversation::SendOutcome;
use crate::model::{Conversation, ConversationId, ConversationKind, Lifecycle};
use crate::tools::OfferFusion;
use crate::work::OrchestratorStepKind;
use crate::{Error, Result};

fn fusion_chat(conversation: &Conversation) -> Result<()> {
    if conversation.kind != ConversationKind::Chat
        || conversation.side_of.is_some()
        || conversation.tab_chat.is_some()
    {
        return Err(Error::Invalid(
            "Fusion is not available to this session: use a top-level Chat.".into(),
        ));
    }
    if conversation.lifecycle == Lifecycle::Archived {
        return Err(Error::Invalid(
            "This Chat is archived; restore it first.".into(),
        ));
    }
    Ok(())
}

fn question(text: &str) -> Option<&str> {
    let text = text.trim_start();
    ["/fusion", "$fusion"].into_iter().find_map(|prefix| {
        text.strip_prefix(prefix)
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
            .map(str::trim)
    })
}

impl SessionManager {
    /// The Fusion button's message, also used by a clicked offer. It still uses the normal
    /// send queue and lanes; the prefix only invokes the skill in the selected CLI.
    pub fn fusion_message(&self, id: &ConversationId, text: String) -> Result<String> {
        let conversation = self.core.conversation(id)?;
        fusion_chat(&conversation)?;
        let provider = conversation
            .setup
            .as_ref()
            .map(|setup| setup.choice().provider)
            .unwrap_or_else(|| self.default_chat_choice().provider);
        Ok(provider.skill_invocation("fusion", question(&text).unwrap_or(&text)))
    }

    pub(super) fn normalize_fusion_message(
        &self,
        id: &ConversationId,
        text: String,
    ) -> Result<String> {
        if question(&text).is_some() {
            self.fusion_message(id, text)
        } else {
            Ok(text)
        }
    }

    pub(crate) async fn offer_fusion(
        &self,
        id: &ConversationId,
        args: OfferFusion,
    ) -> Result<String> {
        let question = args.question.trim();
        if question.is_empty() {
            return Err(Error::Invalid(
                "The Fusion question must not be empty.".into(),
            ));
        }
        let conv = self.conv(id)?;
        let _offer = conv.fusion_offer.lock().await;
        fusion_chat(&self.core.conversation(id)?)?;
        let request = self.request_for(id).await;
        let board = self.core.board(id).await?;
        if board.orchestrator_steps.iter().any(|step| {
            step.request_id == request
                && matches!(step.kind, OrchestratorStepKind::FusionOffered { .. })
        }) {
            return Err(Error::Invalid(
                "Fusion was already offered for this request: don't offer it again.".into(),
            ));
        }
        self.store_step(
            id,
            request,
            OrchestratorStepKind::FusionOffered {
                id: uuid::Uuid::now_v7().to_string(),
                question: question.into(),
            },
        )
        .await?;
        Ok("The user sees a Fusion card. Nothing starts until they click Ask. Never start Fusion yourself; carry on with the request.".into())
    }

    pub async fn start_offered_fusion(
        &self,
        id: &ConversationId,
        offer_id: &str,
    ) -> Result<SendOutcome> {
        let conv = self.conv(id)?;
        let _offer = conv.fusion_offer.lock().await;
        fusion_chat(&self.core.conversation(id)?)?;
        let board = self.core.board(id).await?;
        let offer = board.orchestrator_steps.iter().find(|step| matches!(&step.kind, OrchestratorStepKind::FusionOffered { id, .. } if id == offer_id))
            .ok_or_else(|| Error::NotFound(format!("fusion offer {offer_id}")))?;
        if board.orchestrator_steps.iter().any(|step| matches!(&step.kind, OrchestratorStepKind::FusionOfferTaken { id } if id == offer_id)) {
            return Err(Error::Invalid("This Fusion was already asked.".into()));
        }
        if !self.in_view(id, &board).await?(offer) {
            return Err(Error::Invalid(
                "This offer is from an earlier answer: ask again for it.".into(),
            ));
        }
        let OrchestratorStepKind::FusionOffered { question, .. } = &offer.kind else {
            unreachable!("found as an offer")
        };
        let text = self.fusion_message(id, question.clone())?;
        // Record consent before sending, so repeated clicks can never start it twice.
        self.store_step(
            id,
            offer.request_id.clone(),
            OrchestratorStepKind::FusionOfferTaken {
                id: offer_id.into(),
            },
        )
        .await?;
        self.send_message(id.clone(), text, Vec::new(), Vec::new(), false, None).await.map_err(|err| Error::Invalid(format!("Fusion was asked, but its message couldn't be sent ({err}). Send /fusion in this Chat to try again.")))
    }
}
