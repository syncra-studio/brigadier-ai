//! Cards the user answers: approvals (CLI requests, landings, finishing a session,
//! orchestrator actions), questions and plans.
//!
//! A card is an event on the conversation stream; whoever waits for its answer holds a
//! waiter. Answering a card is a UI-only request: no grant can reach it. Cards still pending
//! when the daemon starts are expired, since nobody waits for them any more.

use std::collections::HashMap;
use std::sync::Mutex;

use brigadier_providers::policy::Similar;
use brigadier_providers::{ApprovalDecision, Decider, ProviderEvent};
use tokio::sync::oneshot;

use super::SessionManager;
use super::conversation::Envelope;
use crate::model::{ConversationId, DomainEvent, Setup};
use crate::work::{
    Approval, ApprovalSubject, CardId, CardState, InjectionKind, Plan, PlanApprover, PlanState,
    Question, QuestionItem, QuestionKind, TaskId,
};
use crate::{Error, Result, now_ms};

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
    /// What the user allowed with "Allow similar commands", per conversation: every worker of
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

    fn take(&self, id: &CardId) -> Vec<oneshot::Sender<CardAnswer>> {
        self.cards
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(id)
            .unwrap_or_default()
    }

    fn is_waited_for(&self, id: &CardId) -> bool {
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
    ) -> bool {
        self.similar
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(conversation_id)
            .is_some_and(|similar| similar.covers(request))
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
    /// The conversation's "Allow similar commands" grants, to carry over (a worker handoff
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

    /// Opens an approval card and returns a receiver for its answer.
    pub(crate) async fn open_approval(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        subject: ApprovalSubject,
    ) -> Result<(Approval, oneshot::Receiver<CardAnswer>)> {
        let request_id = self.request_for(conversation_id, task_id.as_ref()).await;
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
            // A worker's request, or the thread's own (no task).
            ApprovalSubject::Cli { request } => match &approval.task_id {
                Some(task_id) => {
                    self.answer_worker_approval(task_id, request, decision.clone())
                        .await?;
                }
                None => {
                    self.answer_thread_approval(&conversation_id, request, decision.clone())
                        .await?;
                }
            },
            // The call waiting on it gets the answer.
            ApprovalSubject::Action { live: true, .. } => {}
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
            ApprovalSubject::OutwardCommand { .. }
            | ApprovalSubject::Landing { .. }
            | ApprovalSubject::FinishSession { .. }
            | ApprovalSubject::Outline { .. } => {}
        }
        self.waiters
            .answer(&card_id, CardAnswer::Decision(decision.clone()));
        Ok(())
    }

    /// Opens a question card for the user.
    pub(crate) async fn open_question(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        kind: QuestionKind,
        text: String,
        options: Vec<String>,
        recommended: Option<u32>,
    ) -> Result<(Question, oneshot::Receiver<CardAnswer>)> {
        // Only an index that names one of the options.
        let recommended = recommended.filter(|&index| (index as usize) < options.len());
        self.open_card(
            conversation_id,
            task_id,
            kind,
            text,
            options,
            recommended,
            Vec::new(),
        )
        .await
    }

    /// Opens a card that asks a round of questions, answered together.
    pub(crate) async fn open_round(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        kind: QuestionKind,
        items: Vec<QuestionItem>,
    ) -> Result<(Question, oneshot::Receiver<CardAnswer>)> {
        let text = items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.open_card(
            conversation_id,
            task_id,
            kind,
            text,
            Vec::new(),
            None,
            items,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn open_card(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<TaskId>,
        kind: QuestionKind,
        text: String,
        options: Vec<String>,
        recommended: Option<u32>,
        items: Vec<QuestionItem>,
    ) -> Result<(Question, oneshot::Receiver<CardAnswer>)> {
        let request_id = self.request_for(conversation_id, task_id.as_ref()).await;
        let question = Question {
            id: CardId::generate(),
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
            created_at_ms: now_ms(),
            answered_at_ms: None,
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

    /// The user answered a question card: one answer per question of its round.
    pub async fn answer_question(
        &self,
        conversation_id: ConversationId,
        card_id: CardId,
        answers: Vec<String>,
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
        let answers: Vec<String> = answers.iter().map(|a| a.trim().to_owned()).collect();
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
        self.core
            .record_conversation(
                &conversation_id,
                vec![DomainEvent::QuestionUpdated {
                    question: question.clone(),
                }],
            )
            .await?;
        match &question.kind {
            QuestionKind::Orchestrator => {
                for (item, answer) in round.iter().zip(&answers) {
                    self.learn_user_decision(
                        &conversation_id,
                        format!("question:{}:{}", question.id, item.text),
                        format!("{} → {answer}", item.text),
                        format!(
                            "The orchestrator asked the user: {}\nThe user answered: {answer}",
                            item.text
                        ),
                    );
                }
                let text = if round.len() == 1 {
                    format!(
                        "[answer] You asked the user: \"{}\"\nThe user answered: {answer}",
                        round[0].text
                    )
                } else {
                    format!("[answer] You asked the user:\n{answer}")
                };
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
            QuestionKind::Merge {
                branch,
                base,
                conflicted,
                ..
            } => {
                let text = if merge_chosen(&answers[0], base) && *conflicted {
                    format!(
                        "[answer] The user chose to merge `{branch}` into `{base}` and have its conflicts resolved. Call finish_session now, without user_words: it names the merge task that resolves them, and the merge goes through on this answer once that lands."
                    )
                } else if merge_chosen(&answers[0], base) {
                    format!(
                        "[answer] The user chose to merge `{branch}` into `{base}`. Call finish_session now, without user_words."
                    )
                } else {
                    format!(
                        "[answer] The user doesn't want `{branch}` merged into `{base}` yet: \"{}\". Don't merge, and don't ask again until they bring it up.",
                        answers[0]
                    )
                };
                self.deliver_for(
                    &conversation_id,
                    Envelope {
                        kind: InjectionKind::Decision,
                        label: "user answer".into(),
                        task_id: None,
                        text,
                    },
                    question.request_id.clone(),
                )
                .await;
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
        self.waiters.answer(&card_id, CardAnswer::Answered);
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

    /// Records a plan card.
    pub(crate) async fn store_plan(&self, plan: &Plan) -> Result<()> {
        self.core
            .record_conversation(
                &plan.conversation_id,
                vec![DomainEvent::PlanUpdated { plan: plan.clone() }],
            )
            .await?;
        self.settle_requests(&plan.conversation_id).await;
        Ok(())
    }

    /// The user approved or rejected a plan.
    pub async fn decide_plan(
        &self,
        conversation_id: ConversationId,
        card_id: CardId,
        approve: bool,
        message: Option<String>,
    ) -> Result<()> {
        let plan = self
            .change_plan(&conversation_id, &card_id, |plan| {
                if plan.state != PlanState::Proposed {
                    return Err(Error::Invalid("this plan was already decided".into()));
                }
                plan.state = if approve {
                    PlanState::Approved {
                        by: PlanApprover::User,
                    }
                } else {
                    PlanState::Rejected {
                        message: message.clone(),
                    }
                };
                plan.decided_at_ms = Some(now_ms());
                Ok(())
            })
            .await?;
        // Approving a plan leaves plan mode, the same as accepting the plan to implement it.
        if approve
            && let Some(mut setup) = self.core.conversation(&conversation_id)?.setup
            && let Setup::Session { plan_mode, .. } = &mut setup
            && *plan_mode
        {
            *plan_mode = false;
            self.core.set_setup(conversation_id.clone(), setup).await?;
        }
        let steps: Vec<String> = plan
            .steps
            .iter()
            .enumerate()
            .map(|(number, step)| format!("{}. {}", number + 1, step.title))
            .collect();
        self.learn_user_decision(
            &conversation_id,
            format!("plan:{}", plan.id),
            format!(
                "The user {} the plan \"{}\"",
                if approve { "approved" } else { "rejected" },
                plan.title
            ),
            format!(
                "{}\nSteps:\n{}",
                message
                    .as_deref()
                    .map(|m| format!("Their note: {m}"))
                    .unwrap_or_default(),
                steps.join("\n")
            ),
        );
        // A lead that outlined in plan mode has its go-ahead in the user's yes: one answer.
        let mut started = Vec::new();
        if approve {
            let board = self.core.board(&conversation_id).await?;
            let mut waiting: Vec<_> = board
                .tasks
                .values()
                .filter(|task| {
                    task.request_id == plan.request_id
                        && task.state == crate::work::TaskState::Blocked
                        && task.blocked_reason.as_deref()
                            == Some(super::phases::WAITING_FOR_GO_AHEAD)
                })
                .cloned()
                .collect();
            waiting.sort_by_key(|task| task.number);
            // The plan the user said yes to may differ from the outline: revised on their
            // changes, or with checks and assumptions the outline left out. It wins.
            let approved = plan.body.as_deref().map(|body| {
                format!("The plan the user approved, which wins over your outline:\n{body}")
            });
            for lead in waiting {
                match self.go_ahead(&lead, approved.clone()).await {
                    Ok(()) => started.push(format!("task-{}", lead.number)),
                    Err(err) => {
                        tracing::warn!(task = %lead.id, error = %err, "could not start an outline");
                    }
                }
            }
        }
        let text = if approve {
            let how = if !started.is_empty() {
                format!(
                    "Brigadier gave {} the go-ahead on its outline: it builds now.",
                    started.join(" and ")
                )
            } else if plan.steps.len() > 1 {
                "Delegate each phase's lead in order (delegate_task, kind implement, `phase` its number), each once the one before it has landed.".to_owned()
            } else {
                "Delegate its lead with `phase: 1` (delegate_task, kind implement), or make a tiny change yourself.".to_owned()
            };
            format!(
                "[decision] The user chose \u{201c}Yes, implement this plan\u{201d} for \u{201c}{}\u{201d}, and plan mode is off. Build it now, as the plan says. {how}",
                plan.title
            )
        } else {
            match message.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                Some(changes) => format!(
                    "[decision] The user did not take the plan \u{201c}{}\u{201d} as it is. What they want changed: \u{201c}{changes}\u{201d}. Revise the plan to match and propose it again with propose_plan; build nothing yet.",
                    plan.title
                ),
                None => format!(
                    "[decision] The user did not take the plan \u{201c}{}\u{201d}. Build nothing; wait for what they say next.",
                    plan.title
                ),
            }
        };
        self.deliver_for(
            &conversation_id,
            Envelope {
                kind: InjectionKind::Decision,
                label: "plan decision".into(),
                task_id: None,
                text,
            },
            plan.request_id.clone(),
        )
        .await;
        Ok(())
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
                ApprovalSubject::Action { live: false, .. } => continue,
                // The landing that asked is gone; `recover` tells the orchestrator to accept
                // the task again.
                ApprovalSubject::Landing { .. } => {}
                // Nothing would start on a click any more: the orchestrator asks again.
                ApprovalSubject::Outline { title, .. } => {
                    self.deliver_for(
                        conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "outline".into(),
                            task_id: approval.task_id.clone(),
                            text: format!(
                                "[not decided] The user had not answered whether to start the plan “{title}” when Brigadier restarted. Call approve_outline again if its lead still waits."
                            ),
                        },
                        approval.request_id.clone(),
                    )
                    .await;
                }
                // A merge card from before merging was asked for in words: the thread asks again.
                ApprovalSubject::FinishSession { branch, base, .. } => {
                    self.deliver_for(
                        conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "finish session".into(),
                            task_id: None,
                            text: format!(
                                "[not finished] The user had not answered whether to merge `{branch}` into `{base}` when Brigadier restarted; nothing was merged. Ask them again with propose_merge."
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
