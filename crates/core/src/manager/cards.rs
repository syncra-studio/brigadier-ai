//! Cards the user answers: approvals (CLI requests, outward commands, landings, finishing a
//! session, orchestrator actions), questions and plans.
//!
//! A card is an event on the conversation stream; whoever waits for its answer holds a
//! waiter. Answering a card is a UI-only request: no grant can reach it. Cards still pending
//! when the daemon starts are expired, since nobody waits for them any more.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use brigadier_providers::{ApprovalDecision, Decider, ProviderEvent};
use tokio::sync::oneshot;

use super::SessionManager;
use super::conversation::Envelope;
use crate::model::{ConversationId, DomainEvent, Setup};
use crate::tools::GateAnswer;
use crate::work::{
    Approval, ApprovalSubject, CardId, CardState, InjectionKind, Plan, PlanApprover, PlanState,
    Question, QuestionKind, TaskId,
};
use crate::{Error, Result, now_ms};

/// How long an outward command waits for the user before it is declined.
const GATE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long a one-shot pass for an already-approved command stays valid.
const PASS_TTL_MS: i64 = 2 * 60 * 1000;

/// What a waiting card receives.
#[derive(Debug, Clone)]
pub(crate) enum CardAnswer {
    Decision(ApprovalDecision),
    /// A question was answered (the answer is on the card).
    Answered,
}

/// A command the user already allowed in the CLI's own prompt: the gate lets it through
/// once, so the user is not asked twice.
struct Pass {
    task_id: Option<TaskId>,
    argv: Vec<String>,
    cwd: String,
    expires_ms: i64,
}

#[derive(Default)]
pub(crate) struct Waiters {
    cards: Mutex<HashMap<CardId, Vec<oneshot::Sender<CardAnswer>>>>,
    passes: Mutex<Vec<Pass>>,
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

    fn add_pass(&self, pass: Pass) {
        let mut passes = self.passes.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        passes.retain(|pass| pass.expires_ms > now);
        passes.push(pass);
    }

    fn consume_pass(&self, task_id: &Option<TaskId>, argv: &[String], cwd: &str) -> bool {
        let mut passes = self.passes.lock().unwrap_or_else(|p| p.into_inner());
        let now = now_ms();
        passes.retain(|pass| pass.expires_ms > now);
        let found = passes.iter().position(|pass| {
            &pass.task_id == task_id && pass.cwd == cwd && same_command(&pass.argv, argv)
        });
        found.map(|index| passes.remove(index)).is_some()
    }
}

impl SessionManager {
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
            ApprovalSubject::Cli { request } => {
                let task_id = approval
                    .task_id
                    .clone()
                    .ok_or_else(|| Error::Invalid("the card has no task".into()))?;
                if !matches!(decision, ApprovalDecision::Deny { .. })
                    && let Some(command) = &request.command
                    && brigadier_providers::policy::is_outward(command)
                {
                    // The CLI asked (its own ask rule); the gate must not ask again, for any
                    // outward command in the line, as the gate sees it (`cd repo && git push
                    // 2>&1 | tail` reaches it as `git push` in `repo`). Claude names no cwd: its
                    // commands run where the worker runs.
                    let cwd = match &request.cwd {
                        Some(cwd) => Some(PathBuf::from(cwd)),
                        None => match self.existing_task_live(&task_id) {
                            Some(live) => live.cwd().await,
                            None => None,
                        },
                    };
                    for (dir, argv) in brigadier_providers::policy::outward_commands(command) {
                        let cwd = match dir {
                            None => cwd.clone(),
                            Some(dir) => moved_to(cwd.as_deref(), &dir),
                        };
                        self.waiters.add_pass(Pass {
                            task_id: Some(task_id.clone()),
                            argv,
                            cwd: cwd.map(|cwd| resolved(&cwd)).unwrap_or_default(),
                            expires_ms: now_ms() + PASS_TTL_MS,
                        });
                    }
                }
                self.answer_worker_approval(&task_id, request, decision.clone())
                    .await?;
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
            ApprovalSubject::OutwardCommand { .. }
            | ApprovalSubject::Landing { .. }
            | ApprovalSubject::FinishSession { .. } => {}
        }
        self.waiters
            .answer(&card_id, CardAnswer::Decision(decision.clone()));
        Ok(())
    }

    /// Asks the user whether an outward command may run (the gate), bound to exactly this
    /// command line and folder.
    pub(crate) async fn ask_outward_command(
        &self,
        conversation_id: ConversationId,
        task_id: Option<TaskId>,
        argv: Vec<String>,
        cwd: String,
    ) -> GateAnswer {
        // An overnight run: nobody is there to ask, and no earlier pass counts.
        if let Some(message) = self
            .unattended_outward(&conversation_id, task_id.as_ref(), &argv)
            .await
        {
            return GateAnswer::Deny { message };
        }
        if self
            .waiters
            .consume_pass(&task_id, &argv, &resolved(Path::new(&cwd)))
        {
            return GateAnswer::Allow;
        }
        let subject = ApprovalSubject::OutwardCommand {
            argv: argv.clone(),
            cwd,
        };
        let (approval, rx) = match self
            .open_approval(&conversation_id, task_id.clone(), subject)
            .await
        {
            Ok(opened) => opened,
            Err(err) => {
                return GateAnswer::Deny {
                    message: format!("Brigadier could not ask you: {err}"),
                };
            }
        };
        if let Some(task_id) = &task_id {
            self.set_task_blocked(
                task_id,
                Some(format!("Waiting for approval: {}", argv.join(" "))),
            )
            .await;
        }
        let answer = tokio::time::timeout(GATE_TIMEOUT, rx).await;
        if let Some(task_id) = &task_id {
            self.set_task_blocked(task_id, None).await;
        }
        match answer {
            Ok(Ok(CardAnswer::Decision(
                ApprovalDecision::Allow | ApprovalDecision::AllowSimilar,
            ))) => GateAnswer::Allow,
            Ok(Ok(CardAnswer::Decision(ApprovalDecision::Deny { message }))) => GateAnswer::Deny {
                message: if message.trim().is_empty() {
                    "you declined this command".into()
                } else {
                    format!("you declined this command: {message}")
                },
            },
            Ok(Ok(CardAnswer::Answered)) | Ok(Err(_)) => GateAnswer::Deny {
                message: "the approval was withdrawn".into(),
            },
            Err(_) => {
                self.settle_approval(
                    &approval,
                    CardState::Expired {
                        reason: "Nobody answered in 15 minutes.".into(),
                    },
                )
                .await;
                GateAnswer::Deny {
                    message: "nobody answered the approval in 15 minutes".into(),
                }
            }
        }
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
        let request_id = self.request_for(conversation_id, task_id.as_ref()).await;
        let question = Question {
            id: CardId::generate(),
            conversation_id: conversation_id.clone(),
            task_id,
            request_id,
            position: 0,
            kind,
            text,
            // Only an index that names one of the options.
            recommended: recommended.filter(|&index| (index as usize) < options.len()),
            options,
            answer: None,
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

    /// The user answered a question card.
    pub async fn answer_question(
        &self,
        conversation_id: ConversationId,
        card_id: CardId,
        answer: String,
    ) -> Result<()> {
        let board = self.core.board(&conversation_id).await?;
        let mut question = board
            .questions
            .get(&card_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("question {card_id}")))?;
        if question.answer.is_some() || question.answered_at_ms.is_some() {
            return Err(Error::Invalid("this question was already answered".into()));
        }
        let answer = answer.trim().to_owned();
        if answer.is_empty() {
            return Err(Error::Invalid("the answer is empty".into()));
        }
        question.answer = Some(answer.clone());
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
                self.learn_user_decision(
                    &conversation_id,
                    format!("question:{}", question.id),
                    format!("{} → {answer}", question.text),
                    format!(
                        "The orchestrator asked the user: {}\nThe user answered: {answer}",
                        question.text
                    ),
                );
                self.deliver_for(
                    &conversation_id,
                    Envelope {
                        kind: InjectionKind::Decision,
                        label: "user answer".into(),
                        task_id: question.task_id.clone(),
                        text: format!(
                            "[answer] You asked the user: \"{}\"\nThe user answered: {answer}",
                            question.text
                        ),
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
        // A review still running is moot once the user decides.
        let plan = self
            .change_plan(&conversation_id, &card_id, |plan| {
                if !matches!(plan.state, PlanState::Proposed | PlanState::InReview { .. }) {
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
        let mut text = if approve {
            format!(
                "[decision] The user approved the plan \"{}\". Go ahead, and pass each step's number as `step` when you delegate it.",
                plan.title
            )
        } else {
            format!(
                "[decision] The user rejected the plan \"{}\"{}",
                plan.title,
                message
                    .map(|m| format!(": {m}"))
                    .unwrap_or_else(|| ".".into())
            )
        };
        text.push_str(&super::plan_gates::review_for_decision(&plan));
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
                ApprovalSubject::Action { .. } => continue,
                // The landing that asked is gone; `recover` tells the orchestrator to accept
                // the task again.
                ApprovalSubject::Landing { .. } => {}
                // Nothing would merge on a click any more: the orchestrator asks again.
                ApprovalSubject::FinishSession { branch, base, .. } => {
                    self.deliver_for(
                        conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "finish session".into(),
                            task_id: None,
                            text: format!(
                                "[not finished] The user had not answered whether to merge `{branch}` into `{base}` when Brigadier restarted; nothing was merged. Call finish_session again."
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

/// A folder as a pass is bound to it: resolved through symlinks (`/tmp` is `/private/tmp` on
/// macOS), so the CLI's spelling and the gate's `getcwd()` compare equal.
fn resolved(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_owned())
        .to_string_lossy()
        .into_owned()
}

/// The folder `cd <dir>` moves to from `cwd`; `None` when it can't be told (`cd -`, or a
/// relative folder from an unknown one).
fn moved_to(cwd: Option<&Path>, dir: &str) -> Option<PathBuf> {
    let home = || std::env::var_os("HOME").map(PathBuf::from);
    if dir == "~" {
        home()
    } else if let Some(rest) = dir.strip_prefix("~/") {
        home().map(|home| home.join(rest))
    } else if dir == "-" {
        None
    } else if Path::new(dir).is_absolute() {
        Some(PathBuf::from(dir))
    } else {
        cwd.map(|cwd| cwd.join(dir))
    }
}

fn answer_is_yes(answer: &str) -> bool {
    let answer = answer.trim().to_lowercase();
    answer.starts_with("yes") || answer.starts_with("include") || answer.starts_with("show")
}

/// Whether two command lines are the same command (the program compared by file name).
fn same_command(a: &[String], b: &[String]) -> bool {
    let program = |argv: &[String]| {
        argv.first()
            .map(|p| p.rsplit('/').next().unwrap_or(p).to_owned())
    };
    a.len() == b.len() && program(a) == program(b) && a[1..] == b[1..]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cd_in_the_line_moves_the_pass_to_its_folder() {
        let cwd = Path::new("/work/repo");
        assert_eq!(
            moved_to(Some(cwd), "../other"),
            Some(PathBuf::from("/work/repo/../other"))
        );
        assert_eq!(moved_to(Some(cwd), "/abs"), Some(PathBuf::from("/abs")));
        assert_eq!(moved_to(None, "sub"), None);
        assert_eq!(moved_to(Some(cwd), "-"), None);
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(moved_to(None, "~/x"), Some(PathBuf::from(home).join("x")));
        }
    }
}
