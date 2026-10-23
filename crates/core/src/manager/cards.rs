//! Cards the user answers: approvals (CLI requests, landings, finishing a session,
//! orchestrator actions) and questions.
//!
//! A card is an event on the conversation stream; whoever waits for its answer holds a
//! waiter. Answering a card is a UI-only request: no grant can reach it. Cards still pending
//! when the daemon starts are expired, since nobody waits for them any more.

use std::collections::HashMap;
use std::sync::Mutex;

use brigadier_providers::policy::Similar;
use brigadier_providers::{ApprovalDecision, ApprovalRequest, Decider, ProviderEvent};
use tokio::sync::oneshot;

use super::SessionManager;
use super::conversation::{Envelope, SendOutcome};
use crate::model::{ConversationId, DomainEvent, Setup};
use crate::work::{
    AnswerOrigin, Approval, ApprovalSubject, CardId, CardState, InjectionKind, Question,
    QuestionItem, QuestionKind, TaskId,
};
use crate::{Error, Result, now_ms};

/// The id of an approval Brigadier asks itself (no CLI waits on it, as for closing a tab):
/// its answer goes to the call waiting on the card.
pub(crate) const OWN_APPROVAL: &str = "brigadier-escalation:";

/// What a waiting card receives.
#[derive(Debug, Clone)]
pub(crate) enum CardAnswer {
    Decision(ApprovalDecision),
    /// A question was answered (the answer is on the card).
    Answered,
}

#[derive(Default)]
pub(crate) struct Waiters {
    cards: Mutex<HashMap<CardId, Vec<oneshot::Sender<CardAnswer>>>>,
    /// What the user allowed with "Allow … for this session", per conversation: every worker of
    /// the conversation, a successor after a handoff included, gets it without asking.
    similar: Mutex<HashMap<ConversationId, Similar>>,
}

impl Waiters {
    /// Waits for a card's answer; several may wait for the same card.
    pub(crate) fn register(&self, id: CardId) -> oneshot::Receiver<CardAnswer> {
        let (tx, rx) = oneshot::channel();
        self.cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(id)
            .or_default()
            .push(tx);
        rx
    }

    /// How many calls wait for card `id`, for the flow tests.
    #[cfg(test)]
    pub(crate) fn waiting_on(&self, id: &CardId) -> usize {
        self.cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .map_or(0, Vec::len)
    }

    fn take(&self, id: &CardId) -> Vec<oneshot::Sender<CardAnswer>> {
        self.cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
            .unwrap_or_default()
    }

    pub(crate) fn is_waited_for(&self, id: &CardId) -> bool {
        self.cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .is_some_and(|waiters| waiters.iter().any(|w| !w.is_closed()))
    }

    fn answer(&self, id: &CardId, answer: CardAnswer) {
        for waiter in self.take(id) {
            let _ = waiter.send(answer.clone());
        }
    }

    /// Whether the user already allowed something similar to `request` in the conversation.
    pub(crate) fn similar_allowed(
        &self,
        conversation_id: &ConversationId,
        request: &brigadier_providers::ApprovalRequest,
        workspace: Option<&std::path::Path>,
    ) -> bool {
        self.similar
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(conversation_id)
            .is_some_and(|similar| similar.covers(request, workspace))
    }

    /// Allows requests similar to `request` for the rest of the conversation.
    pub(crate) fn allow_similar(
        &self,
        conversation_id: &ConversationId,
        request: &brigadier_providers::ApprovalRequest,
    ) {
        self.similar
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(conversation_id.clone())
            .or_default()
            .allow(request);
    }
}

impl SessionManager {
    /// The conversation's "Allow … for this session" grants, to carry over (a worker handoff
    /// keeps them anyway: they belong to the conversation, not to a CLI session).
    pub fn snapshot_grants(&self, conversation_id: &ConversationId) -> Similar {
        self.waiters
            .similar
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(conversation_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Puts back grants taken with [`Self::snapshot_grants`], merged into what the
    /// conversation has now.
    pub fn restore_grants(&self, conversation_id: &ConversationId, grants: Similar) {
        self.waiters
            .similar
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .entry(conversation_id.clone())
            .or_default()
            .merge(grants);
    }

    /// Opens the user's card for a request of `cli`, unless that CLI session has ended: a
    /// request answered late (the user away from its card) may find it closed
    /// (taken over, retired), and its answer would have nowhere to go. A card opened as it ended
    /// is expired at once; one opened before it is expired by whoever closed it, if they do.
    /// `None` when no card waits.
    pub(crate) async fn open_cli_card(
        &self,
        cli: &super::conversation::Cli,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        request: ApprovalRequest,
    ) -> Result<Option<Approval>> {
        if cli.ended.is_cancelled() {
            return Ok(None);
        }
        let (approval, _) = self
            .open_approval(conversation_id, task_id, ApprovalSubject::Cli { request })
            .await?;
        if cli.ended.is_cancelled() {
            self.settle_approval(
                &approval,
                CardState::Expired {
                    reason: "The session that asked has ended.".into(),
                },
            )
            .await;
            return Ok(None);
        }
        Ok(Some(approval))
    }

    /// Opens an approval card and returns a receiver for its answer.
    pub(crate) async fn open_approval(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        subject: ApprovalSubject,
    ) -> Result<(Approval, oneshot::Receiver<CardAnswer>)> {
        let request_id = self.request_for(conversation_id).await;
        let approval = Approval {
            id: CardId::generate(),
            conversation_id: conversation_id.clone(),
            task_id,
            request_id,
            position: 0,
            subject,
            state: CardState::Pending,
            created_at_ms: now_ms(),
            resolved_at_ms: None,
        };
        let rx = self.waiters.register(approval.id.clone());
        self.store_approval(&approval).await?;
        Ok((approval, rx))
    }

    async fn store_approval(&self, approval: &Approval) -> Result<()> {
        self.core
            .record_conversation(
                &approval.conversation_id,
                vec![DomainEvent::ApprovalUpdated {
                    approval: approval.clone(),
                }],
            )
            .await?;
        self.settle_requests(&approval.conversation_id).await;
        Ok(())
    }

    /// Settles an approval card without the user (policy, timeout, a task that ended).
    pub(crate) async fn settle_approval(&self, approval: &Approval, state: CardState) {
        let mut approval = approval.clone();
        approval.state = state;
        approval.resolved_at_ms = Some(now_ms());
        self.waiters.take(&approval.id);
        if let Err(err) = self.store_approval(&approval).await {
            tracing::warn!(card = %approval.id, error = %err, "could not settle a card");
        }
    }

    /// The user answered an approval card.
    pub async fn answer_card(
        &self,
        conversation_id: ConversationId,
        card_id: CardId,
        decision: ApprovalDecision,
    ) -> Result<()> {
        let board = self.core.board(&conversation_id).await?;
        let mut approval = board
            .approvals
            .get(&card_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("card {card_id}")))?;
        if approval.state != CardState::Pending {
            return Err(Error::Invalid("this card was already answered".into()));
        }
        // Only a CLI's own session grant: Brigadier's gates ask every time.
        if decision == ApprovalDecision::AllowSimilar
            && !matches!(&approval.subject, ApprovalSubject::Cli { request } if request.grant.is_some())
        {
            return Err(Error::Invalid(
                "only a worker's own command can be allowed again".into(),
            ));
        }
        approval.state = match &decision {
            ApprovalDecision::Allow => CardState::Allowed {
                by: Decider::User,
                similar: false,
            },
            ApprovalDecision::AllowSimilar => CardState::Allowed {
                by: Decider::User,
                similar: true,
            },
            ApprovalDecision::Deny { message } => CardState::Denied {
                by: Decider::User,
                message: (!message.trim().is_empty()).then(|| message.clone()),
            },
        };
        approval.resolved_at_ms = Some(now_ms());
        self.store_approval(&approval).await?;

        match &approval.subject {
            // A worker's request, or the thread's own (no task). A Deny always tells the model
            // why, even when the user gave no reason: an empty one fails its whole turn.
            ApprovalSubject::Cli { request } => {
                let decision = match &decision {
                    ApprovalDecision::Deny { message } => ApprovalDecision::Deny {
                        message: brigadier_providers::user_deny_message(message),
                    },
                    other => other.clone(),
                };
                match &approval.task_id {
                    Some(task_id) => {
                        self.answer_worker_approval(task_id, request, decision)
                            .await?;
                    }
                    None => {
                        self.answer_thread_approval(&conversation_id, request, decision)
                            .await?;
                    }
                }
            }
            ApprovalSubject::Action { action, .. } => {
                let text = match &decision {
                    ApprovalDecision::Allow | ApprovalDecision::AllowSimilar => {
                        format!("[decision] The user approved: {action}")
                    }
                    ApprovalDecision::Deny { message } => format!(
                        "[decision] The user declined: {action}{}",
                        if message.trim().is_empty() {
                            String::new()
                        } else {
                            format!(" ({message})")
                        }
                    ),
                };
                self.deliver_for(
                    &conversation_id,
                    Envelope {
                        kind: InjectionKind::Decision,
                        label: "approval decision".into(),
                        task_id: approval.task_id.clone(),
                        text,
                    },
                    approval.request_id.clone(),
                )
                .await;
            }
            ApprovalSubject::Browser { .. } => {
                self.answer_browser_card(&approval, &decision).await;
            }
            // A message to another session from before sessions stopped messaging each other:
            // nothing sends it any more.
            ApprovalSubject::PeerMessage { .. }
            | ApprovalSubject::OutwardCommand { .. }
            | ApprovalSubject::Landing { .. }
            | ApprovalSubject::FinishSession { .. }
            | ApprovalSubject::Outline { .. } => {}
        }
        self.waiters
            .answer(&card_id, CardAnswer::Decision(decision.clone()));
        Ok(())
    }

    /// Opens a card that asks a round of questions, answered together.
    pub(crate) async fn open_round(
        &self,
        id: CardId,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        kind: QuestionKind,
        items: Vec<QuestionItem>,
        title: Option<String>,
    ) -> Result<(Question, oneshot::Receiver<CardAnswer>)> {
        let text = items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.open_card(
            id,
            conversation_id,
            task_id,
            kind,
            text,
            Vec::new(),
            None,
            items,
            title,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_card(
        &self,
        id: CardId,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        kind: QuestionKind,
        text: String,
        options: Vec<String>,
        recommended: Option<u32>,
        items: Vec<QuestionItem>,
        title: Option<String>,
    ) -> Result<(Question, oneshot::Receiver<CardAnswer>)> {
        let request_id = self.request_for(conversation_id).await;
        let question = Question {
            id,
            conversation_id: conversation_id.clone(),
            task_id,
            request_id,
            position: 0,
            kind,
            text,
            recommended,
            options,
            items,
            answer: None,
            answers: Vec::new(),
            title,
            created_at_ms: now_ms(),
            answered_at_ms: None,
            words_id: None,
            answered_by: None,
        };
        let rx = self.waiters.register(question.id.clone());
        self.core
            .record_conversation(
                conversation_id,
                vec![DomainEvent::QuestionUpdated {
                    question: question.clone(),
                }],
            )
            .await?;
        self.settle_requests(conversation_id).await;
        Ok((question, rx))
    }

    /// The user answered a question card: one answer per question of its round. `origin`: the
    /// client that sent it and how the user gave it, kept with the answer.
    pub async fn answer_question(
        &self,
        conversation_id: ConversationId,
        card_id: CardId,
        answers: Vec<String>,
        origin: Option<AnswerOrigin>,
    ) -> Result<()> {
        let board = self.core.board(&conversation_id).await?;
        let mut question = board
            .questions
            .get(&card_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("question {card_id}")))?;
        if !question.is_open() {
            return Err(Error::Invalid("this question was already answered".into()));
        }
        let round = question.round();
        let mut answers: Vec<String> = answers.iter().map(|a| a.trim().to_owned()).collect();
        if answers.len() != round.len() {
            return Err(Error::Invalid(format!(
                "this card asks {} questions, and {} answers came",
                round.len(),
                answers.len()
            )));
        }
        if answers.iter().any(String::is_empty) {
            return Err(Error::Invalid("an answer is empty".into()));
        }
        // A merge card answered in the user's own words: the card records "Not yet", and their
        // words are a new request, which carries that decision.
        let typed = match &question.kind {
            QuestionKind::Merge { base, .. }
                if !round[0]
                    .options
                    .iter()
                    .any(|option| option.label == answers[0]) =>
            {
                let not_yet = round[0]
                    .options
                    .iter()
                    .find(|option| !merge_chosen(&option.label, base))
                    .map_or_else(|| "Not yet".to_owned(), |option| option.label.clone());
                Some(std::mem::replace(&mut answers[0], not_yet))
            }
            _ => None,
        };
        // A single question's answer is the answer itself; a round's lists each question.
        let answer = match answers.as_slice() {
            [only] => only.clone(),
            _ => round_answer(&round, &answers),
        };
        question.answer = Some(answer.clone());
        if !question.items.is_empty() {
            question.answers = answers.clone();
        }
        question.answered_at_ms = Some(now_ms());
        question.answered_by = origin;
        // Until its words are sent, the card's own id marks them on their way.
        question.words_id = typed.as_ref().map(|_| card_id.to_string());
        // Recorded before it is delivered: until the answer is in the inbox, the request
        // counts as carried, or a settle in between would end it (its card answered, nothing
        // queued) and open its time again with the answer's turn.
        // A merge card's answer starts no work: merge cards are retired (BUILD-PLAN A4).
        // A card `brigadier ask` waits on answers it, not the thread.
        let delivers =
            matches!(&question.kind, QuestionKind::Orchestrator) && !self.cli_asks.waits(&card_id);
        if delivers {
            self.answering(&conversation_id, &card_id, question.request_id.clone())
                .await;
        }
        // The uncommitted-changes card is closed and its answer kept under the lock its asking
        // holds: a task that asks meanwhile sees either the open card or the answer (#461).
        let asking = match &question.kind {
            QuestionKind::UncommittedChanges { .. } => Some(self.uncommitted_asks.lock().await),
            _ => None,
        };
        let recorded = self
            .core
            .record_conversation(
                &conversation_id,
                vec![DomainEvent::QuestionUpdated {
                    question: question.clone(),
                }],
            )
            .await;
        if let Err(err) = recorded {
            self.answer_delivered(&conversation_id, &card_id).await;
            return Err(err);
        }
        match &question.kind {
            QuestionKind::Orchestrator => {
                for (item, answer) in round.iter().zip(&answers) {
                    self.learn_user_decision(
                        &conversation_id,
                        format!("question:{}:{}", question.id, item.text),
                        format!("{} → {answer}", item.text),
                        format!(
                            "The thread asked the user: {}\nThe user answered: {answer}",
                            item.text
                        ),
                    );
                }
                let text = if round.len() == 1 {
                    format!(
                        "[answer] You asked the user: \"{}\"\nThe user answered: {answer}\n[card {}]",
                        round[0].text, question.id
                    )
                } else {
                    format!(
                        "[answer] You asked the user:\n{answer}\n[card {}]",
                        question.id
                    )
                };
                if delivers {
                    self.deliver_for(
                        &conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "user answer".into(),
                            task_id: question.task_id.clone(),
                            text,
                        },
                        question.request_id.clone(),
                    )
                    .await;
                }
            }
            QuestionKind::Merge { branch, base, .. } if let Some(typed) = typed => {
                self.conv(&conversation_id)?
                    .note(format!(
                        "[answer] The user answered the merge card for `{branch}` into `{base}` in their own words, in the message below."
                    ))
                    .await;
                self.waiters.answer(&card_id, CardAnswer::Answered);
                self.settle_requests(&conversation_id).await;
                let sent = self
                    .send_message(
                        conversation_id.clone(),
                        typed,
                        Vec::new(),
                        Vec::new(),
                        false,
                        None,
                    )
                    .await?;
                // The card points at the message its words went as.
                question.words_id = Some(match sent {
                    SendOutcome::Sent(message) => message.id,
                    SendOutcome::Queued(item) => item.id,
                });
                self.core
                    .record_conversation(
                        &conversation_id,
                        vec![DomainEvent::QuestionUpdated { question }],
                    )
                    .await?;
                return Ok(());
            }
            QuestionKind::Merge { branch, base, .. } => {
                self.conv(&conversation_id)?
                    .note(format!(
                        "[answer] The user answered the merge card for `{branch}` into `{base}`: \"{}\"\n[card {}]",
                        answers[0], question.id
                    ))
                    .await;
                self.waiters.answer(&card_id, CardAnswer::Answered);
                self.settle_requests(&conversation_id).await;
                return Ok(());
            }
            QuestionKind::UncommittedChanges { .. } => {
                let see = answer_is_yes(&answer);
                let conversation = self.core.conversation(&conversation_id)?;
                if let Some(Setup::Session {
                    repo,
                    environment,
                    permission,
                    orchestrator,
                    plan_mode,
                    ..
                }) = conversation.setup
                {
                    self.core
                        .set_setup(
                            conversation_id.clone(),
                            Setup::Session {
                                repo,
                                environment,
                                permission,
                                orchestrator,
                                workers_see_uncommitted: Some(see),
                                plan_mode,
                            },
                        )
                        .await?;
                }
            }
        }
        if delivers {
            self.answer_delivered(&conversation_id, &card_id).await;
        }
        self.waiters.answer(&card_id, CardAnswer::Answered);
        drop(asking);
        self.settle_requests(&conversation_id).await;
        Ok(())
    }

    /// Closes a question nobody needs answered any more (the user redid the request that
    /// asked it): answered, with no answer.
    pub(crate) async fn withdraw_question(&self, question: &Question) {
        let mut question = question.clone();
        question.answered_at_ms = Some(now_ms());
        self.waiters.take(&question.id);
        let conversation_id = question.conversation_id.clone();
        if let Err(err) = self
            .core
            .record_conversation(
                &conversation_id,
                vec![DomainEvent::QuestionUpdated { question }],
            )
            .await
        {
            tracing::warn!(error = %err, "could not withdraw a question");
        }
    }

    /// Expires cards nobody can wait for any more (after a restart).
    pub(crate) async fn expire_stale_cards(&self, conversation_id: &ConversationId) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        for approval in board.approvals.values() {
            if approval.state != CardState::Pending || self.waiters.is_waited_for(&approval.id) {
                continue;
            }
            match &approval.subject {
                // Answering it delivers the decision itself; nothing needs to wait.
                ApprovalSubject::Action { .. } => continue,
                // Its Send delivers the stored text: the card comes back after a restart.
                ApprovalSubject::PeerMessage { .. } => continue,
                // Answering it tells the agent, which makes its call again on a yes.
                ApprovalSubject::Browser { .. } => continue,
                // The landing that asked is gone; `recover` tells the orchestrator to accept
                // the task again.
                ApprovalSubject::Landing { .. } => {}
                // Leads post no outlines any more: nothing starts on it.
                ApprovalSubject::Outline { .. } => {}
                // A merge approval from before merging was asked for in words: nothing merges on
                // it, and merge cards are retired (BUILD-PLAN A4).
                ApprovalSubject::FinishSession { branch, base, .. } => {
                    self.deliver_for(
                        conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "finish session".into(),
                            task_id: None,
                            text: format!(
                                "[not finished] The user had not answered whether to merge `{branch}` into `{base}` when Brigadier restarted; nothing was merged. Don't ask on a card: the user merges by asking in their own words; you may say once, in plain text, that the work is ready."
                            ),
                        },
                        approval.request_id.clone(),
                    )
                    .await;
                }
                _ => {}
            }
            self.settle_approval(
                approval,
                CardState::Expired {
                    reason: "The session that asked has ended.".into(),
                },
            )
            .await;
        }
    }

    /// Records a worker approval answered on the user's behalf or by the user.
    pub(crate) async fn record_worker_resolution(
        &self,
        task_id: &TaskId,
        approval_id: String,
        decision: ApprovalDecision,
        decided_by: Decider,
    ) {
        self.record_worker_event(
            task_id,
            ProviderEvent::ApprovalResolved {
                id: approval_id,
                decision,
                decided_by,
            },
        )
        .await;
    }
}

fn answer_is_yes(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer.starts_with("yes") || answer.starts_with("include") || answer.starts_with("show")
}

/// The label of a merge card's yes.
pub(crate) fn merge_label(base: &str) -> String {
    format!("Merge into {base}")
}

/// The label of a merge card's yes when the branch conflicts with the base.
pub(crate) const MERGE_RESOLVING: &str = "Merge & resolve conflicts";

/// Whether a merge card's answer chose the merge.
pub(crate) fn merge_chosen(answer: &str, base: &str) -> bool {
    let answer = answer.trim();
    answer == merge_label(base) || answer == MERGE_RESOLVING
}

/// A round's answers as the asker reads them: each question, then its answer.
fn round_answer(round: &[QuestionItem], answers: &[String]) -> String {
    round
        .iter()
        .zip(answers)
        .enumerate()
        .map(|(index, (item, answer))| format!("{}. {}\n   → {answer}", index + 1, item.text))
        .collect::<Vec<_>>()
        .join("\n")
}
