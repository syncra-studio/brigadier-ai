//! The CLI's own question tool and plan mode, in the user's cards (harness D7, D28). A question
//! the thread's CLI asks with its own tool (Claude's `AskUserQuestion`, Codex's
//! `request_user_input`) is the thread's question card, with its "Asking questions" and
//! "Waiting for your answer" rows; the answer goes back as the tool's result. A plan the CLI
//! proposes from its own plan mode is the plan card: "Build it" takes the session out of plan
//! mode and tells it to go ahead, and the user may reply instead. Brigadier only plumbs.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_providers::CliQuestion;
use tokio_util::sync::CancellationToken;

use super::SessionManager;
use super::cards::CardAnswer;
use super::conversation::{Cli, ConvLive};
use crate::Result;
use crate::model::{ConversationId, Message, Setup};
use crate::work::{ApprovalSubject, CardId, CardState, QuestionItem, QuestionKind, QuestionOption};

/// What "Build it" says to the thread, as the user's own message.
pub(crate) const BUILD_IT: &str = "Build it.";
/// Why an older plan card closed.
const NEWER_PLAN: &str = "A newer plan replaced it.";
const REPLIED: &str = "You replied instead.";

/// The CLI questions whose answer goes back to the CLI, by conversation and the CLI's own id.
#[derive(Default)]
pub(crate) struct CliQuestions {
    next: AtomicU64,
    waiting: Mutex<HashMap<(ConversationId, String), (u64, CancellationToken)>>,
}

impl CliQuestions {
    fn add(&self, key: (ConversationId, String)) -> (u64, CancellationToken) {
        let token = CancellationToken::new();
        let tag = self.next.fetch_add(1, Ordering::Relaxed);
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        // The same id again is a new CLI of the conversation asking: the old one is gone.
        if let Some((_, old)) = waiting.insert(key, (tag, token.clone())) {
            old.cancel();
        }
        (tag, token)
    }

    fn forget(&self, key: &(ConversationId, String), tag: u64) {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        if waiting.get(key).is_some_and(|(held, _)| *held == tag) {
            waiting.remove(key);
        }
    }

    /// The CLI took its question back unanswered: its card stays, and an answer given later
    /// reaches the thread as a message, as any answer of the thread's does.
    pub(crate) fn withdraw(&self, conversation_id: &ConversationId, id: &str) {
        let key = (conversation_id.clone(), id.to_owned());
        if let Some((_, token)) = self
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key)
        {
            token.cancel();
        }
    }
}

/// A CLI's question as the card asks it.
fn question_item(question: &CliQuestion) -> QuestionItem {
    QuestionItem {
        text: question.question.clone(),
        options: question
            .options
            .iter()
            .map(|option| QuestionOption {
                label: option.label.clone(),
                description: option
                    .description
                    .as_deref()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned),
            })
            .collect(),
        recommended: question.recommended,
        multi_select: question.multi_select,
    }
}

/// An answer's picks: a multi-select question's options, as the card joins them with ", ",
/// else the one answer (a chosen option, or the user's own words).
pub(crate) fn picks(item: &QuestionItem, answer: &str) -> Vec<String> {
    let is_option = |label: &str| item.options.iter().any(|option| option.label == label);
    if item.multi_select && !is_option(answer) {
        let parts: Vec<&str> = answer.split(", ").collect();
        if parts.len() > 1 && parts.iter().all(|part| is_option(part)) {
            return parts.into_iter().map(str::to_owned).collect();
        }
    }
    vec![answer.to_owned()]
}

impl SessionManager {
    /// The thread's CLI asks with its own question tool: the thread's question card, whose
    /// answer goes back to the CLI as the tool's result (harness D7).
    pub(super) async fn cli_questions_asked(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        id: String,
        questions: Vec<CliQuestion>,
    ) {
        let items: Vec<QuestionItem> = questions.iter().map(question_item).collect();
        let title = match questions.as_slice() {
            [only] => only
                .header
                .as_deref()
                .map(|header| super::brains::one_line(header, 80))
                .filter(|header| !header.is_empty()),
            _ => None,
        };
        // Held before the card opens, as `brigadier ask` holds it: a quick answer reaches the
        // CLI, never the thread.
        let card = CardId::generate();
        let hold = self.cli_asks.hold(card.clone());
        let answered = match self
            .open_round(
                card.clone(),
                &conv.id,
                None,
                QuestionKind::Orchestrator,
                items,
                title,
            )
            .await
        {
            Ok((_, answered)) => answered,
            Err(err) => {
                tracing::warn!(conversation = %conv.id, error = %err, "could not open the CLI's question card");
                let _ = cli.session.answer_questions(id, None).await;
                return;
            }
        };
        let key = (conv.id.clone(), id.clone());
        let (tag, withdrawn) = self.cli_questions.add(key.clone());
        let this = self.arc();
        let cli = cli.clone();
        tokio::spawn(async move {
            let _hold = hold;
            let answer = tokio::select! {
                answer = answered => Some(answer),
                () = withdrawn.cancelled() => None,
                () = cli.ended.cancelled() => None,
            };
            this.cli_questions.forget(&key, tag);
            let Some(answer) = answer else {
                return;
            };
            let answers = match answer {
                Ok(CardAnswer::Answered) => this.card_picks(&key.0, &card).await,
                // Closed without an answer (the request that asked was redone).
                _ => None,
            };
            if let Err(err) = cli.session.answer_questions(id, answers).await {
                tracing::warn!(conversation = %key.0, error = %err, "could not answer the CLI's question");
            }
        });
    }

    /// An answered card's picks, question by question.
    async fn card_picks(
        &self,
        conversation_id: &ConversationId,
        card: &CardId,
    ) -> Option<Vec<Vec<String>>> {
        let board = self.core.board(conversation_id).await.ok()?;
        let question = board.questions.get(card)?;
        let round = question.round();
        let answers = if question.answers.is_empty() {
            vec![question.answer.clone()?]
        } else {
            question.answers.clone()
        };
        Some(
            round
                .iter()
                .zip(&answers)
                .map(|(item, answer)| picks(item, answer))
                .collect(),
        )
    }

    /// The CLI proposed its plan from its own plan mode: the plan card, replacing an older one
    /// still open (harness D7).
    pub(super) async fn plan_proposed(&self, conversation_id: &ConversationId, plan: &str) {
        let plan = plan.trim();
        if plan.is_empty() {
            return;
        }
        self.expire_plans(conversation_id, NEWER_PLAN).await;
        // Its answer needs nobody waiting: "Build it" sends the user's message itself.
        if let Err(err) = self
            .open_approval(
                conversation_id,
                None,
                ApprovalSubject::Plan {
                    plan: plan.to_owned(),
                },
            )
            .await
        {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not open the plan card");
        }
    }

    /// The user replied rather than build the plan: its card closes.
    pub(super) async fn plan_replied(&self, conversation_id: &ConversationId) {
        self.expire_plans(conversation_id, REPLIED).await;
    }

    async fn expire_plans(&self, conversation_id: &ConversationId, reason: &str) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        for approval in board.approvals.values().filter(|approval| {
            approval.state == CardState::Pending
                && matches!(approval.subject, ApprovalSubject::Plan { .. })
        }) {
            self.settle_approval(
                approval,
                CardState::Expired {
                    reason: reason.into(),
                },
            )
            .await;
        }
    }

    /// "Plan before building" (harness D42, an experiment the user opted into): with the
    /// setting on, the user's new request to a session starts in its CLI's plan mode, so the
    /// thread asks its round and shows a plan before it changes anything. Not for "Build it",
    /// nor while the session's workers still work (the thread would have no plan card to leave
    /// plan mode by, mid-build).
    pub(super) async fn plan_first(&self, id: &ConversationId, users: &[Message]) {
        if !self.core.settings().plan_first
            || users.iter().any(|message| message.text.trim() == BUILD_IT)
            || self.plan_mode(id)
            || self.has_running_workers(id).await
        {
            return;
        }
        let Ok(Some(mut setup)) = self.core.conversation(id).map(|c| c.setup) else {
            return;
        };
        let Setup::Session { plan_mode, .. } = &mut setup else {
            return;
        };
        *plan_mode = true;
        match self.set_setup(id.clone(), setup).await {
            Ok(_) => {
                tracing::info!(experiment = "D42 plan-first", conversation = %id, "a new request starts in plan mode")
            }
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not start the request in plan mode")
            }
        }
    }

    /// "Build it": the session leaves plan mode, and the user's go-ahead goes to the thread.
    pub(super) async fn build_plan(&self, conversation_id: &ConversationId) -> Result<()> {
        let mut setup = self.core.conversation(conversation_id)?.setup;
        if let Some(Setup::Session { plan_mode, .. }) = &mut setup
            && *plan_mode
        {
            *plan_mode = false;
            self.set_setup(conversation_id.clone(), setup.expect("a session's setup"))
                .await?;
        }
        self.send_message(
            conversation_id.clone(),
            BUILD_IT.into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(multi_select: bool) -> QuestionItem {
        QuestionItem {
            text: "Which?".into(),
            options: ["red", "green", "a, b"]
                .into_iter()
                .map(|label| QuestionOption {
                    label: label.into(),
                    description: None,
                })
                .collect(),
            recommended: Some(0),
            multi_select,
        }
    }

    #[test]
    fn a_multi_select_answer_names_each_option_it_picked() {
        assert_eq!(picks(&item(true), "red, green"), ["red", "green"]);
        // An option whose label holds ", " is one pick.
        assert_eq!(picks(&item(true), "a, b"), ["a, b"]);
        // The user's own words, as typed.
        assert_eq!(picks(&item(true), "red, purple"), ["red, purple"]);
        // One pick per question otherwise.
        assert_eq!(picks(&item(false), "red, green"), ["red, green"]);
    }
}
