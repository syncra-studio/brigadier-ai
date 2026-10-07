//! The ordinary composer path: proposals and directive edits are code-owned, while source
//! interpretation uses the normal read-only orchestrator. Start remains app-only.
use super::super::SessionManager;
use super::super::conversation::SendOutcome;
use super::{directives, infos, phases_of};
use crate::model::{ConversationId, DomainEvent, Setup};
use crate::overnight::{OvernightState, ProposedPhase, ProposedPlan, ProposedSource};
use crate::tools::ProposeOvernight;
use crate::work::{AttachmentRef, Mention, RequestState};
use crate::{Error, Result};

impl SessionManager {
    pub(crate) async fn overnight_message(
        &self,
        id: &ConversationId,
        text: &str,
        attachments: &[AttachmentRef],
        mentions: &[Mention],
    ) -> Result<Option<SendOutcome>> {
        let board = self.core.board(id).await?;
        let current = board
            .runs
            .values()
            .filter(|run| run.state != OvernightState::Superseded)
            .max_by_key(|run| (run.created_at_ms, run.segment));
        let intent = directives::unattended(text);
        let continuing = directives::continuation(text)
            && current.is_some_and(|run| run.state == OvernightState::Finished);
        let steering = current.is_some_and(|run| {
            (run.state.is_active() && run.state != OvernightState::Reporting)
                || run.state == OvernightState::Proposed
        });
        if !intent && !continuing && !steering {
            return Ok(None);
        }
        let conv = self.conv(id)?;
        let command = format!("words-{}", uuid::Uuid::new_v4());
        if let Some(run) = current.filter(|_| steering && (!intent || board.active_run().is_some()))
        {
            self.steer_overnight(id.clone(), run.id.clone(), command, text.to_owned())
                .await?;
            let message = self
                .core
                .append_user_message(
                    id.clone(),
                    text.to_owned(),
                    attachments.to_vec(),
                    mentions.to_vec(),
                )
                .await?;
            if run.state.is_active() {
                // User words go straight to the phase lead; no follow-up classification wait.
                let working = self.working_request(id).await;
                self.join_working(&conv, message.clone(), working).await;
            } else {
                self.prepare_proposal_turn(&conv, message.clone()).await;
            }
            return Ok(Some(SendOutcome::Sent(message)));
        }
        let run = if continuing {
            self.continue_overnight(
                id.clone(),
                current.unwrap().id.clone(),
                command,
                text.to_owned(),
            )
            .await?
        } else {
            self.propose_overnight(id.clone(), command, text.to_owned(), None)
                .await?
        };
        let message = self
            .core
            .append_user_message(
                id.clone(),
                text.to_owned(),
                attachments.to_vec(),
                mentions.to_vec(),
            )
            .await?;
        // A source/phase brief needs interpretation before the final preview. A bare goal
        // needs no pre-Start model call: Phase 0 plans it once the user commits to the run.
        let interpret = !continuing
            && (text.to_lowercase().contains("phase")
                || text.contains(".md")
                || !attachments.is_empty()
                || !mentions.is_empty()
                || !board.plans.is_empty());
        if interpret {
            self.prepare_proposal_turn(&conv, message.clone()).await;
        } else {
            self.set_proposal_request_done(id, &message).await?;
        }
        tracing::info!(run = %run.id, "overnight proposal from user words; waiting for Start");
        Ok(Some(SendOutcome::Sent(message)))
    }

    async fn set_proposal_request_done(
        &self,
        id: &ConversationId,
        message: &crate::model::Message,
    ) -> Result<()> {
        if let Some(request) = &message.request_id {
            let board = self.core.board(id).await?;
            if let Some(mut record) = board.requests.get(request).cloned() {
                record.moved_to(RequestState::Done, false, crate::now_ms());
                self.core
                    .record_conversation(id, vec![DomainEvent::RequestUpdated { request: record }])
                    .await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn check_overnight_proposal(&self, id: &ConversationId) -> Result<()> {
        if self
            .core
            .board(id)
            .await?
            .runs
            .values()
            .any(|run| run.state == OvernightState::Proposed)
        {
            return Err(Error::Invalid("The overnight proposal is waiting for the user's Start. Only read-only preparation is allowed.".into()));
        }
        Ok(())
    }

    pub(crate) async fn interpret_overnight(
        &self,
        id: &ConversationId,
        args: ProposeOvernight,
    ) -> Result<String> {
        let run_id = crate::model::OvernightRunId(args.run_id);
        let plan = ProposedPlan {
            name: args.name,
            goal: Some(args.goal),
            rules: Some(args.rules),
            phases: args
                .phases
                .into_iter()
                .map(|phase| ProposedPhase {
                    number: phase.number,
                    name: phase.name,
                    scope: phase.scope,
                    done_when: phase.done_when,
                    depends_on: phase.depends_on,
                })
                .collect(),
            sources: args
                .sources
                .into_iter()
                .map(|source| ProposedSource {
                    path: source.path,
                    sections: source.sections,
                })
                .collect(),
        };
        if plan.phases.iter().any(|phase| {
            phase.name.trim().is_empty()
                || phase.scope.trim().is_empty()
                || phase.done_when.is_empty()
        }) {
            return Err(Error::Invalid(
                "Each source phase needs its name, scope and done-when criteria.".into(),
            ));
        }
        let mut numbers = std::collections::HashSet::new();
        for (index, phase) in plan.phases.iter().enumerate() {
            let number = phase.number.unwrap_or(index as u32 + 1);
            if number == 0
                || !numbers.insert(number)
                || phase
                    .done_when
                    .iter()
                    .any(|criterion| criterion.trim().is_empty())
            {
                return Err(Error::Invalid(
                    "Source phases need unique positive numbers and non-empty done-when criteria."
                        .into(),
                ));
            }
        }
        let conversation = self.core.conversation(id)?;
        let Some(Setup::Session { repo, .. }) = conversation.setup else {
            return Err(Error::Invalid(
                "An overnight proposal needs a repository.".into(),
            ));
        };
        let sources = self
            .snapshot_sources(std::path::Path::new(&repo), &plan)
            .await;
        let run = self.change_run(id, &run_id, format!("interpret-{}", uuid::Uuid::new_v4()), |run, _| {
            if run.state != OvernightState::Proposed || run.revision != args.revision {
                return Err(Error::Invalid("The proposal changed or started meanwhile. Do not replace it; use the current briefing.".into()));
            }
            if run.predecessor.is_some() {
                return Err(Error::Invalid("This proposal continues an earlier run: its phases and what they reached stay as they are. Reply with exactly [quiet].".into()));
            }
            run.name = plan.name.clone();
            run.goal = plan.goal.clone().unwrap_or_else(|| run.words.clone());
            run.rules = plan.rules.clone().unwrap_or_default();
            run.phases = phases_of(&plan);
            run.sources = sources;
            let clock = directives::Clock::system();
            run.problems = directives::parse(&run.words, &clock).problems;
            let numbered = (!run.phases.is_empty()).then(|| infos(&run.phases));
            run.problems.extend(directives::check(&run.directives, numbered.as_deref(), &[], &clock, true));
            run.revision += 1;
            Ok(())
        }).await?;
        Ok(format!(
            "Proposal {} revision {} is shown on the one plan card. Nothing starts until the user's Start. Reply with exactly [quiet].",
            run.id, run.revision
        ))
    }
}
