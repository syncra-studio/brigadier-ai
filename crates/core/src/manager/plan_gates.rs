//! Plan review: plans of two or more steps get an independent reviewer from another vendor
//! than the orchestrator's, except for eligible first small interactive plans. Risky plans
//! get two reviewers on different models, and both must approve before Brigadier approves
//! the plan on the user's behalf.
//!
//! A plan's review is a gate round (`Plan.gate`), like a change's, decided once every
//! reviewer has a result:
//!
//! - **Approved:** the plan is approved (`PlanApprover::Review`); the reviewers' notes go to
//!   the orchestrator with the decision.
//! - **Changes asked:** the findings are numbered F1, F2, … and the plan is `Revising`. The
//!   orchestrator proposes the revision with `revises` and an answer to each finding; the
//!   revision is always reviewed again, with the findings and the answers in view.
//! - **Still not right after [`PLAN_ROUNDS`] rounds:** nothing is approved; the orchestrator
//!   asks the user or rescopes the work. Whatever it proposes next for the request is
//!   reviewed, and the same steps again are refused.
//! - **No result** (a reviewer failed or was stopped): the plan is turned down, and the
//!   orchestrator may propose it again (with any findings a finished reviewer gave).
//!
//! Findings count as each reviewer's result arrives. Under "Ask for approval" the user's card
//! shows at once and the review runs beside it: its findings and notes show on the card as
//! they come, and the user's decision carries them.
//!
//! A plan whose review is under way or asked for changes holds back the request's write
//! tasks. After a restart, a round's unfinished reviewers are replaced (finished ones keep
//! their result), and a plan waiting for its revision is asked for again.

use std::collections::HashMap;

use brigadier_providers::ProviderKind;
use brigadier_router::Author;

use super::SessionManager;
use super::conversation::Envelope;
use super::gates::{outcome_of, review_result};
use crate::model::{CardId, ConversationId, PermissionLevel, Setup};
use crate::work::{
    Finding, FindingResponse, Gate, GateLink, GateMember, GateOutcome, GateOwner, GateResult,
    GateRole, InjectionKind, Plan, PlanApprover, PlanState, PlanStep, Report, RequestState, Task,
    TaskId, TaskKind,
};
use crate::{Error, Result, now_ms};

/// Review rounds one plan goes through (the first, and one revision) before the orchestrator
/// asks the user or rescopes.
pub(crate) const PLAN_ROUNDS: u32 = 2;

pub(crate) use crate::tools::SMALL_PLAN_STEPS;

/// Whether this is the request's first non-risky plan within the small-plan step limit.
/// The caller separately checks session mode and whether a review is required.
pub(crate) fn small_plan(plan: &Plan, plans: &HashMap<CardId, Plan>) -> bool {
    !plan.risky
        && plan.steps.len() <= SMALL_PLAN_STEPS
        && plan.revises.is_none()
        && !plans.values().any(|p| p.request_id == plan.request_id)
}

impl SessionManager {
    /// The orchestrator's model: a plan's reviewers come from another vendor.
    fn orchestrator_author(&self, id: &ConversationId) -> Result<Author> {
        Ok(match self.core.conversation(id)?.setup {
            Some(Setup::Session { orchestrator, .. }) => Author {
                provider: orchestrator.provider,
                model: orchestrator.model,
            },
            _ => Author {
                provider: ProviderKind::Claude,
                model: None,
            },
        })
    }

    /// Opens review round `round` of a stored plan with `reviewers` new reviewers, beside
    /// `keep`: reviewers of the round that already gave their result (a round a restart cut
    /// off). `decides`: the review decides the plan (it goes `InReview`); otherwise the user
    /// does, and the review only informs them. Returns the new reviewers.
    pub(crate) async fn open_plan_gate(
        &self,
        plan: &Plan,
        round: u32,
        keep: Vec<GateMember>,
        reviewers: usize,
        decides: bool,
    ) -> Result<Vec<Task>> {
        let held = self.gates.lock().await;
        let board = self.core.board(&plan.conversation_id).await?;
        let previous = plan
            .revises
            .as_ref()
            .and_then(|id| board.plans.get(id))
            .cloned();
        let spec = review_spec(plan, previous.as_ref(), round);
        let author = self.orchestrator_author(&plan.conversation_id)?;
        let owner = GateOwner::Plan {
            plan_id: plan.id.clone(),
        };
        // New reviewers avoid the models of the ones kept, as of each other.
        let mut checking: Vec<Author> = keep
            .iter()
            .filter_map(|member| board.tasks.get(&member.task_id))
            .map(|task| Author {
                provider: task.route.choice.provider,
                model: task.route.choice.model.clone(),
            })
            .collect();
        let kept = keep.len();
        let mut members: Vec<GateMember> = keep;
        let mut started: Vec<Task> = Vec::new();
        let opened: Result<()> = async {
            for index in kept..kept + reviewers {
                let review = self
                    .create_task(
                        &plan.conversation_id,
                        if index == 0 {
                            format!("Review plan: {}", plan.title)
                        } else {
                            format!("Second review of plan: {}", plan.title)
                        },
                        TaskKind::Review,
                        spec.clone(),
                        None,
                        Some(author.clone()),
                        checking.clone(),
                        Some(GateLink {
                            owner: owner.clone(),
                            round,
                            role: GateRole::Review,
                        }),
                        None,
                        Vec::new(),
                        None,
                        None,
                        Vec::new(),
                    )
                    .await?;
                checking.push(Author {
                    provider: review.route.choice.provider,
                    model: review.route.choice.model.clone(),
                });
                members.push(GateMember {
                    task_id: review.id.clone(),
                    role: GateRole::Review,
                    result: None,
                    avoid: Vec::new(),
                });
                started.push(review);
            }
            Ok(())
        }
        .await;
        // The plan as it is now: the user may have decided it meanwhile.
        let stored = match opened {
            Ok(()) => match self.core.board(&plan.conversation_id).await {
                Ok(board) => board
                    .plans
                    .get(&plan.id)
                    .cloned()
                    .ok_or_else(|| Error::NotFound(format!("plan {}", plan.id))),
                Err(err) => Err(err),
            },
            Err(err) => Err(err),
        };
        let stored = match stored {
            Ok(mut stored) if is_open(&stored.state) => {
                // The kept reviewers' findings stay, under the ids they were given.
                let findings = stored
                    .gate
                    .as_ref()
                    .filter(|gate| gate.round == round)
                    .map(|gate| {
                        gate.findings
                            .iter()
                            .filter(|finding| members.iter().any(|m| m.task_id == finding.by))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                stored.gate = Some(Gate {
                    verification_scope: Default::default(),
                    rebased: false,
                    round,
                    commit: None,
                    members,
                    outcome: None,
                    relanding: false,
                    retry: false,
                    overridden: false,
                    findings,
                });
                let first = stored
                    .gate
                    .as_ref()
                    .and_then(|gate| gate.members.first())
                    .map(|member| member.task_id.clone());
                if decides && let Some(task_id) = first {
                    stored.state = PlanState::InReview { task_id };
                }
                self.store_plan(&stored).await
            }
            Ok(_) => Err(Error::Invalid("the plan was decided meanwhile".into())),
            Err(err) => Err(err),
        };
        drop(held);
        if let Err(err) = stored {
            // Nothing half-started keeps running. Stopped outside the lock: a stopped
            // reviewer's missing result is recorded under it.
            for member in started {
                let _ = Box::pin(self.stop_task(member.id)).await;
            }
            return Err(err);
        }
        Ok(started)
    }

    /// A plan's reviewer reported, or (`failed`, with why) failed or was stopped first.
    pub(crate) async fn plan_member_done(
        &self,
        member: &Task,
        plan_id: &CardId,
        link: &GateLink,
        failed: Option<&str>,
    ) {
        let result = match (failed, &member.report) {
            (None, Some(report)) => review_result(report),
            (None, None) => return,
            (Some(reason), _) => GateResult::NoResult {
                reason: format!(
                    "The reviewer (task-{}) gave no result: {reason}",
                    member.number
                ),
            },
        };
        let envelope = {
            // A plan this sends back for revision can't be replaced meanwhile by a proposal
            // that read it still in review (see `propose_plan`).
            let _plans = self.plans.lock().await;
            let _held = self.gates.lock().await;
            let Ok(board) = self.core.board(&member.conversation_id).await else {
                return;
            };
            let Some(mut plan) = board.plans.get(plan_id).cloned() else {
                return;
            };
            let Some(mut gate) = plan.gate.clone() else {
                return;
            };
            // A result of an older or closed round, or for a plan already decided.
            if gate.round != link.round || gate.outcome.is_some() || !is_open(&plan.state) {
                return;
            }
            let Some(slot) = gate
                .members
                .iter_mut()
                .find(|known| known.task_id == member.id)
                .filter(|slot| slot.result.is_none())
            else {
                return;
            };
            slot.result = Some(result);
            // Its findings count at once: the user's card shows them while the others review,
            // and a decision made before the round ends carries them.
            let slot = slot.clone();
            record_findings(&mut gate.findings, &slot);
            let decided = gate
                .members
                .iter()
                .all(|m| m.result.is_some())
                .then(|| outcome_of(&gate.members));
            let mut reviewers = Vec::new();
            for known in &gate.members {
                if let Ok(task) = self
                    .task_by_id(&member.conversation_id, &known.task_id)
                    .await
                {
                    reviewers.push(task);
                }
            }
            gate.outcome = decided.clone();
            plan.gate = Some(gate.clone());
            // Under "Ask for approval" the user decides: the review only shows on the card.
            let decides = matches!(plan.state, PlanState::InReview { .. });
            // What it decided on the user's behalf, and why ("Decided for you").
            let mut decision: Option<(String, String)> = None;
            let envelope = match decided {
                None => None,
                Some(GateOutcome::Passed) => {
                    plan.review_notes = reviewers
                        .iter()
                        .filter_map(|t| t.report.as_ref())
                        .flat_map(review_notes)
                        .collect();
                    decides.then(|| {
                        plan.state = PlanState::Approved {
                            by: PlanApprover::Review,
                        };
                        plan.decided_at_ms = Some(now_ms());
                        decision = Some((
                            format!("Approved the plan \u{201c}{}\u{201d}", plan.title),
                            match plan.review_notes.len() {
                                0 => "An independent review from another vendor approved it.".to_owned(),
                                notes => format!(
                                    "An independent review from another vendor approved it, with {notes} note{}.",
                                    if notes == 1 { "" } else { "s" }
                                ),
                            },
                        ));
                        approved_text(&plan, &reviewers)
                    })
                }
                Some(GateOutcome::Failed) if decides => {
                    let listed = findings_list(&gate.findings, &reviewers);
                    let found = format!(
                        "{} review finding{}",
                        gate.findings.len(),
                        if gate.findings.len() == 1 { "" } else { "s" }
                    );
                    if gate.round < PLAN_ROUNDS {
                        decision = Some((
                            format!("Sent the plan \u{201c}{}\u{201d} back: {found}", plan.title),
                            "The findings are on the plan card.".to_owned(),
                        ));
                        plan.state = PlanState::Revising;
                        Some(revise_text(&plan, &gate, &reviewers))
                    } else {
                        decision = Some((
                            format!("Did not approve the plan \u{201c}{}\u{201d}", plan.title),
                            format!(
                                "{found} left after {PLAN_ROUNDS} review rounds. The orchestrator asks you or makes it smaller."
                            ),
                        ));
                        plan.state = PlanState::Rejected {
                            message: Some(format!(
                                "Not approved after {PLAN_ROUNDS} review rounds."
                            )),
                        };
                        plan.decided_at_ms = Some(now_ms());
                        Some(format!(
                            "[plan review] The revised plan \"{}\" still has problems after the last review round ({} of {PLAN_ROUNDS}), so it is not approved:\n{listed}\n[/plan review] Don't revise it again on your own: ask the user how to proceed (ask_user), or rescope the work into a smaller plan.",
                            plan.title, gate.round
                        ))
                    }
                }
                Some(GateOutcome::NoResult | GateOutcome::Unverified) if decides => {
                    let reasons = gate
                        .members
                        .iter()
                        .filter_map(|m| match &m.result {
                            Some(GateResult::NoResult { reason }) => Some(format!("- {reason}")),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    decision = Some((
                        format!("Did not approve the plan \u{201c}{}\u{201d}", plan.title),
                        "Its review couldn't run.".to_owned(),
                    ));
                    plan.state = PlanState::Rejected {
                        message: Some(format!("The review could not run.\n{reasons}")),
                    };
                    plan.decided_at_ms = Some(now_ms());
                    // What a reviewer that did finish found still matters to the next plan.
                    let found = if gate.findings.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\nA reviewer that finished found these problems; fix them in the plan you propose:\n{}",
                            findings_list(&gate.findings, &reviewers)
                        )
                    };
                    Some(format!(
                        "[decision] The independent review of the plan \"{}\" could not run, so it is not approved:\n{reasons}{found}\nPropose it again.",
                        plan.title
                    ))
                }
                Some(_) => None,
            };
            if let Err(err) = self.store_plan(&plan).await {
                tracing::warn!(plan = %plan.id, error = %err, "could not record a plan review");
                return;
            }
            envelope.map(|text| (plan, text, decision))
        };
        if let Some((plan, text, decision)) = envelope {
            if let Some((what, why)) = decision {
                self.decided_for_plan(&plan, what, why).await;
            }
            self.deliver_for(
                &plan.conversation_id,
                Envelope {
                    kind: InjectionKind::Decision,
                    label: "plan review".into(),
                    task_id: Some(member.id.clone()),
                    text,
                },
                plan.request_id.clone(),
            )
            .await;
            // An overnight run's Phase 0 plan goes on to its judge (or ends Phase 0).
            self.planning_plan_decided(&plan).await;
        }
    }

    /// What asks the orchestrator again for the revision of `request`'s plan that waits for
    /// one, and that plan: its review's findings, and the way on.
    pub(crate) fn revision_reminder(
        &self,
        board: &crate::board::Board,
        request: &str,
    ) -> Option<(CardId, String)> {
        let plan = board.plans.values().find(|plan| {
            plan.request_id.as_deref() == Some(request) && plan.state == PlanState::Revising
        })?;
        let gate = plan.gate.as_ref()?;
        let reviewers: Vec<Task> = gate
            .members
            .iter()
            .filter_map(|m| board.tasks.get(&m.task_id).cloned())
            .collect();
        Some((
            plan.id.clone(),
            format!(
                "{}\nYour last turn ended without the revision. Revise the plan now; if only the user can settle a finding, ask them (ask_user).",
                revise_text(plan, gate, &reviewers)
            ),
        ))
    }

    /// Changes a plan as it is stored now, under the gate lock so a review result can't race
    /// the change. A review round still open on a plan that is no longer open is closed, and
    /// its reviewers stop.
    pub(crate) async fn change_plan(
        &self,
        conversation_id: &ConversationId,
        plan_id: &CardId,
        change: impl FnOnce(&mut Plan) -> Result<()>,
    ) -> Result<Plan> {
        let (plan, moot) = self
            .change_plan_only(conversation_id, plan_id, change)
            .await?;
        for member in moot {
            // Boxed: stopping a reviewer records its missing result, which reaches this plan.
            let _ = Box::pin(self.stop_task(member)).await;
        }
        Ok(plan)
    }

    /// [`Self::change_plan`] without stopping anything: returns the changed plan and the
    /// reviewers of its closed round to stop, for a caller holding a lock their stopping
    /// needs.
    pub(crate) async fn change_plan_only(
        &self,
        conversation_id: &ConversationId,
        plan_id: &CardId,
        change: impl FnOnce(&mut Plan) -> Result<()>,
    ) -> Result<(Plan, Vec<TaskId>)> {
        let _held = self.gates.lock().await;
        let board = self.core.board(conversation_id).await?;
        let before = board
            .plans
            .get(plan_id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("plan {plan_id}")))?;
        let mut plan = before.clone();
        change(&mut plan)?;
        let mut moot = Vec::new();
        if !is_open(&plan.state)
            && let Some(gate) = plan.gate.as_mut()
            && gate.outcome.is_none()
        {
            gate.outcome = Some(GateOutcome::Superseded);
            moot = gate
                .members
                .iter()
                .filter(|m| m.result.is_none())
                .map(|m| m.task_id.clone())
                .collect();
        }
        if plan != before {
            self.store_plan(&plan).await?;
        }
        Ok((plan, moot))
    }

    /// After a restart: a plan's review round whose reviewers were stopped runs again with
    /// the reviewers that already gave a result, so the plan doesn't wait for results that
    /// never come; a plan Brigadier decides whose review never started gets it; and a plan
    /// still waiting for its revision has the orchestrator told again.
    pub(crate) async fn rerun_plan_reviews(&self, conversation_id: &ConversationId) {
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        let brigadier_decides = self.brigadier_decides_plans(conversation_id);
        for plan in board.plans.values() {
            let (round, keep, reviewers, decides) =
                match rerun_of(plan, &board.plans, brigadier_decides) {
                    Some(Rerun::Review {
                        round,
                        keep,
                        start,
                        decides,
                    }) => (round, keep, start, decides),
                    Some(Rerun::Revise) => {
                        // Unless the user stopped the request, or it failed.
                        let over = plan
                            .request_id
                            .as_ref()
                            .and_then(|request| board.requests.get(request))
                            .is_some_and(|request| {
                                matches!(
                                    request.state,
                                    RequestState::Stopped | RequestState::Failed { .. }
                                )
                            });
                        if let (false, Some(gate)) = (over, &plan.gate) {
                            let reviewers: Vec<Task> = gate
                                .members
                                .iter()
                                .filter_map(|m| board.tasks.get(&m.task_id).cloned())
                                .collect();
                            self.deliver_for(
                                conversation_id,
                                Envelope {
                                    kind: InjectionKind::Decision,
                                    label: "plan review".into(),
                                    task_id: None,
                                    text: format!(
                                        "{}\nBrigadier restarted before the revision arrived.",
                                        revise_text(plan, gate, &reviewers)
                                    ),
                                },
                                plan.request_id.clone(),
                            )
                            .await;
                        }
                        continue;
                    }
                    None => continue,
                };
            if let Err(err) = self
                .open_plan_gate(plan, round, keep, reviewers, decides)
                .await
            {
                tracing::warn!(plan = %plan.id, error = %err, "could not review a plan again");
                if decides {
                    let reason = err.to_string();
                    let _ = self
                        .change_plan(conversation_id, &plan.id, |p| {
                            p.state = PlanState::Rejected {
                                message: Some(format!("The review could not run: {reason}")),
                            };
                            p.decided_at_ms = Some(now_ms());
                            Ok(())
                        })
                        .await;
                    self.deliver_for(
                        conversation_id,
                        Envelope {
                            kind: InjectionKind::Decision,
                            label: "plan review".into(),
                            task_id: None,
                            text: format!(
                                "[decision] Brigadier restarted before the review of the plan \"{}\" finished, and its review could not run again ({reason}), so it is not approved. Propose it again.",
                                plan.title
                            ),
                        },
                        plan.request_id.clone(),
                    )
                    .await;
                }
            }
        }
    }

    /// Whether Brigadier decides the conversation's plans on the user's behalf (Approve for me
    /// and Full access, outside plan mode), as `propose_plan` reads it.
    pub(crate) fn brigadier_decides_plans(&self, id: &ConversationId) -> bool {
        match self.core.conversation(id).map(|c| c.setup) {
            Ok(Some(Setup::Session { .. })) => {
                !self.plan_mode(id) && self.permission(id) != PermissionLevel::AskForApproval
            }
            Ok(_) => true,
            Err(_) => false,
        }
    }

    /// Who a plan's reviewer must not be, for a hand-off to another model: the orchestrator,
    /// and the models of the round's other reviewers.
    pub(crate) async fn plan_gate_avoid(
        &self,
        member: &Task,
        plan_id: &CardId,
        round: u32,
    ) -> (Option<Author>, Vec<Author>) {
        let author = self.orchestrator_author(&member.conversation_id).ok();
        let mut others = Vec::new();
        if let Ok(board) = self.core.board(&member.conversation_id).await
            && let Some(gate) = board
                .plans
                .get(plan_id)
                .and_then(|plan| plan.gate.as_ref())
                .filter(|gate| gate.round == round)
        {
            for other in gate.members.iter().filter(|m| m.task_id != member.id) {
                if let Some(other) = board.tasks.get(&other.task_id) {
                    others.push(Author {
                        provider: other.route.choice.provider,
                        model: other.route.choice.model.clone(),
                    });
                }
            }
        }
        (author, others)
    }
}

/// Whether a plan waits for a decision a review can inform.
fn is_open(state: &PlanState) -> bool {
    matches!(state, PlanState::Proposed | PlanState::InReview { .. })
}

/// How many reviewers check a new plan; none approves it at once (under "Approve for me").
/// `revising`: it revises a plan whose review asked for changes, so it is always reviewed
/// again. `after_approved`: it follows an approved plan of the same request, and whether its
/// steps differ from that plan's. `after_rejected`: a plan of the same request ran out of
/// review rounds, so whatever follows it is reviewed.
pub(crate) fn plan_reviewers(
    steps: usize,
    risky: bool,
    revising: bool,
    after_approved: Option<bool>,
    after_rejected: bool,
) -> usize {
    if risky {
        2
    } else if revising || after_rejected {
        1
    } else {
        match after_approved {
            Some(changed) => usize::from(changed),
            None => usize::from(steps >= 2),
        }
    }
}

/// How many reviewers check `plan` (stored or about to be), given the conversation's
/// `plans`.
pub(crate) fn reviewers_for(plan: &Plan, plans: &HashMap<CardId, Plan>) -> usize {
    let earlier = || {
        plans.values().filter(|p| {
            p.id != plan.id
                && p.request_id == plan.request_id
                && p.created_at_ms <= plan.created_at_ms
        })
    };
    let approved = earlier()
        .filter(|p| matches!(p.state, PlanState::Approved { .. }))
        .max_by_key(|p| p.created_at_ms)
        .filter(|_| plan.revises.is_none());
    let after_rejected = earlier().any(rounds_ran_out);
    let reviewers = plan_reviewers(
        plan.steps.len(),
        plan.risky,
        plan.revises.is_some(),
        approved.map(|before| steps_differ(&before.steps, &plan.steps)),
        after_rejected,
    );
    // A revision reruns the whole panel, even if the orchestrator drops the risky flag.
    reviewers.max(
        plan.revises
            .as_ref()
            .and_then(|id| plans.get(id))
            .and_then(|previous| previous.gate.as_ref())
            .map_or(0, |gate| gate.members.len()),
    )
}

/// The review round of `plan`: the one after its predecessor's for a revision, else the first.
pub(crate) fn review_round(plan: &Plan, plans: &HashMap<CardId, Plan>) -> u32 {
    plan.revises
        .as_ref()
        .and_then(|id| plans.get(id))
        .and_then(|previous| previous.gate.as_ref())
        .map_or(1, |gate| gate.round + 1)
}

/// A plan's steps as compared between plans: each step's title and detail with runs of
/// whitespace made single spaces, in order. Case is kept: paths are case-sensitive.
fn step_keys(steps: &[PlanStep]) -> Vec<String> {
    steps
        .iter()
        .map(|step| {
            format!("{} {}", step.title, step.detail.as_deref().unwrap_or(""))
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Whether two plans' steps differ materially: a step added, removed, changed, moved or
/// repeated.
pub(crate) fn steps_differ(before: &[PlanStep], after: &[PlanStep]) -> bool {
    step_keys(before) != step_keys(after)
}

/// Whether a plan ran out of review rounds: its last round still asked for changes, so it was
/// not approved.
fn rounds_ran_out(plan: &Plan) -> bool {
    matches!(plan.state, PlanState::Rejected { .. })
        && plan.gate.as_ref().is_some_and(|gate| {
            gate.round >= PLAN_ROUNDS && gate.outcome == Some(GateOutcome::Failed)
        })
}

/// The plan of `request` with the same steps as `steps` that ran out of review rounds, or
/// that such a plan revised: proposing it again would only start its review over.
pub(crate) fn repeats_rejected<'a>(
    plans: &'a HashMap<CardId, Plan>,
    request: Option<&str>,
    steps: &[PlanStep],
) -> Option<&'a Plan> {
    let keys = step_keys(steps);
    for rejected in plans
        .values()
        .filter(|p| p.request_id.as_deref() == request && rounds_ran_out(p))
    {
        let mut plan = Some(rejected);
        // A revision chain is short; the bound only guards against a cycle in stored data.
        for _ in 0..plans.len() {
            let Some(of) = plan else {
                break;
            };
            if step_keys(&of.steps) == keys {
                return Some(rejected);
            }
            plan = of.revises.as_ref().and_then(|id| plans.get(id));
        }
    }
    None
}

/// Why `request`'s write tasks wait, when Brigadier decides plans: a plan of it is still in
/// its review, or its review asked for changes and the revision hasn't come.
pub(crate) fn review_blocks_writes<'a>(
    plans: impl IntoIterator<Item = &'a Plan>,
    request: Option<&str>,
) -> Option<String> {
    let plan = plans.into_iter().find(|plan| {
        plan.request_id.as_deref() == request
            && matches!(
                plan.state,
                PlanState::Proposed | PlanState::InReview { .. } | PlanState::Revising
            )
    })?;
    Some(if plan.state == PlanState::Revising {
        format!(
            "The review of the plan \"{}\" asked for changes: revise the plan first (propose_plan with revises: \"{}\" and one response per finding), then start implement or merge tasks once it is approved.",
            plan.title, plan.id
        )
    } else {
        format!(
            "The plan \"{}\" is in its independent review: wait for the plan review (its outcome arrives as a message) before starting implement or merge tasks.",
            plan.title
        )
    })
}

/// What a restart leaves to redo of a plan's review.
#[derive(Debug, PartialEq)]
pub(crate) enum Rerun {
    /// Run review round `round` again: the reviewers in `keep` already gave their result,
    /// `start` new ones replace those the restart stopped.
    Review {
        round: u32,
        keep: Vec<GateMember>,
        start: usize,
        decides: bool,
    },
    /// The plan waits for its revision: tell the orchestrator again.
    Revise,
}

/// What a restart leaves to redo of `plan`'s review. `brigadier_decides`: Brigadier decides
/// the conversation's plans.
pub(crate) fn rerun_of(
    plan: &Plan,
    plans: &HashMap<CardId, Plan>,
    brigadier_decides: bool,
) -> Option<Rerun> {
    let started = plan.gate.as_ref().filter(|gate| !gate.members.is_empty());
    match (started, &plan.state) {
        (_, PlanState::Revising) => Some(Rerun::Revise),
        (Some(gate), PlanState::Proposed | PlanState::InReview { .. })
            if gate.outcome.is_none() =>
        {
            let keep: Vec<GateMember> = gate
                .members
                .iter()
                .filter(|member| member.result.is_some())
                .cloned()
                .collect();
            let start = gate.members.len() - keep.len();
            (start > 0).then_some(Rerun::Review {
                round: gate.round,
                keep,
                start,
                decides: matches!(plan.state, PlanState::InReview { .. }),
            })
        }
        // Reviewed before plans had a gate.
        (None, PlanState::InReview { .. }) => Some(Rerun::Review {
            round: 1,
            keep: Vec::new(),
            start: if plan.risky { 2 } else { 1 },
            decides: true,
        }),
        // Brigadier stopped between recording the plan and starting its review.
        (None, PlanState::Proposed) if brigadier_decides && plan.review_skip_reason.is_none() => {
            let start = reviewers_for(plan, plans);
            (start > 0).then(|| Rerun::Review {
                round: review_round(plan, plans),
                keep: Vec::new(),
                start,
                decides: true,
            })
        }
        _ => None,
    }
}

/// Adds a reviewer's findings to its round's, numbered on from the last (F1, F2, …), so an
/// id shown or answered never changes as later results arrive.
pub(crate) fn record_findings(findings: &mut Vec<Finding>, member: &GateMember) {
    let Some(GateResult::Failed { findings: found }) = &member.result else {
        return;
    };
    if findings.iter().any(|known| known.by == member.task_id) {
        return;
    }
    let next = findings
        .iter()
        .filter_map(|finding| finding.id.get(1..)?.parse::<usize>().ok())
        .max()
        .unwrap_or(0);
    for (index, text) in found
        .iter()
        .map(|text| text.trim())
        .filter(|text| !text.is_empty())
        .enumerate()
    {
        findings.push(Finding {
            id: format!("F{}", next + index + 1),
            text: text.to_owned(),
            by: member.task_id.clone(),
        });
    }
}

/// What tells the orchestrator to revise a plan whose review asked for changes.
fn revise_text(plan: &Plan, gate: &Gate, reviewers: &[Task]) -> String {
    format!(
        "[plan review] The independent review (round {} of {PLAN_ROUNDS}) asked for changes to the plan \"{}\" (id {}), so it is not approved:\n{}\n[/plan review] Revise it: call propose_plan with the revised steps, revises: \"{}\", and responses with one line per finding: \"F1 accepted: what you changed\" or \"F2 declined: why\". The revision is reviewed again; don't start write tasks before it is approved.",
        gate.round,
        plan.title,
        plan.id,
        findings_list(&gate.findings, reviewers),
        plan.id
    )
}

/// The revision's answer to each finding, from lines like "F1 accepted: what changed" or
/// "F2 declined: why". Every finding needs exactly one answer, and a decline its reason.
pub(crate) fn parse_responses(
    lines: &[String],
    findings: &[Finding],
) -> std::result::Result<Vec<FindingResponse>, String> {
    const FORM: &str = "\"F1 accepted: what you changed\" or \"F2 declined: why\"";
    let mut responses: Vec<FindingResponse> = Vec::new();
    for line in lines.iter().flat_map(|text| text.lines()) {
        let line = line.trim().trim_start_matches(['-', '*', ' ']);
        if line.is_empty() {
            continue;
        }
        let (id, rest) = line
            .split_once(|c: char| c.is_whitespace() || c == ':')
            .ok_or_else(|| format!("\"{line}\" is not a response: write {FORM}"))?;
        let id = id.trim().to_uppercase();
        let finding = findings
            .iter()
            .find(|finding| finding.id == id)
            .ok_or_else(|| {
                format!(
                    "\"{line}\" answers no finding of the review (they are {})",
                    ids(findings)
                )
            })?;
        let rest = rest.trim_start_matches([':', ' ', '\t']);
        let lower = rest.to_lowercase();
        let (accepted, word) = ["accepted", "accept", "declined", "decline"]
            .iter()
            .find(|word| lower.starts_with(*word))
            .map(|word| (word.starts_with("accept"), word.len()))
            .ok_or_else(|| format!("\"{line}\" neither accepts nor declines {id}: write {FORM}"))?;
        let note = rest[word..]
            .trim_start_matches([':', '-', ' ', '\t', '—', '–'])
            .trim()
            .to_owned();
        if !accepted && note.is_empty() {
            return Err(format!("{id} is declined without a reason: say why"));
        }
        if responses.iter().any(|known| known.id == id) {
            return Err(format!("{id} is answered twice"));
        }
        responses.push(FindingResponse {
            id,
            finding: finding.text.clone(),
            accepted,
            note,
        });
    }
    let unanswered: Vec<&str> = findings
        .iter()
        .filter(|finding| !responses.iter().any(|r| r.id == finding.id))
        .map(|finding| finding.id.as_str())
        .collect();
    if !unanswered.is_empty() {
        return Err(format!(
            "every finding of the review needs a response; unanswered: {}. Add one line each: {FORM}",
            unanswered.join(", ")
        ));
    }
    responses.sort_by_key(|r| r.id[1..].parse::<u32>().unwrap_or(u32::MAX));
    Ok(responses)
}

fn ids(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "none".into();
    }
    findings
        .iter()
        .map(|finding| finding.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a reviewer that approved noted for carrying the plan out.
fn review_notes(report: &Report) -> Vec<String> {
    report
        .open_questions
        .iter()
        .chain(&report.risks)
        .map(|note| note.trim().to_owned())
        .filter(|note| !note.is_empty())
        .collect()
}

/// The steps of a plan, numbered.
fn steps_text(steps: &[PlanStep]) -> String {
    let mut text = String::new();
    for (index, step) in steps.iter().enumerate() {
        text.push_str(&format!("{}. {}", index + 1, step.title));
        if let Some(detail) = &step.detail {
            text.push_str(&format!(" — {detail}"));
        }
        text.push('\n');
    }
    text
}

/// What a plan's reviewer reads. Revisions include the previous review's stored findings,
/// a step diff, the full revised plan, and orchestrator responses as context only.
fn review_spec(plan: &Plan, previous: Option<&Plan>, round: u32) -> String {
    let mut spec = if previous.is_some() {
        "Review this revised plan against the repository (read-only), focusing on the previous findings and the step diff below.\n"
    } else {
        "Review this plan before it is carried out. Check it against the repository (read-only): is it sound, complete, and the simplest thing that works? Are there risks, missing steps or wrong assumptions?\n"
    }.to_owned();
    if let Some(previous) = previous {
        spec.push_str(&format!(
            "\nThis is review round {round} of {PLAN_ROUNDS}, the last. The orchestrator revised the plan after the earlier review asked for changes.\n\nPrevious plan: {}\n",
            previous.title
        ));
        spec.push_str("\nPrevious round's findings (from the reviewers):\n");
        if let Some(gate) = &previous.gate {
            spec.push_str(&findings_list(&gate.findings, &[]));
        }
        spec.push_str(
            "\n\nStep diff by position (- previous, + revised; moves appear as changes):\n",
        );
        let mut changed = false;
        for index in 0..previous.steps.len().max(plan.steps.len()) {
            let before = previous.steps.get(index);
            let after = plan.steps.get(index);
            if before.map(|s| (&s.title, &s.detail)) == after.map(|s| (&s.title, &s.detail)) {
                continue;
            }
            changed = true;
            for (prefix, step) in [("-", before), ("+", after)] {
                if let Some(step) = step {
                    spec.push_str(&format!(
                        "{prefix} {}. {}\n",
                        index + 1,
                        match &step.detail {
                            Some(detail) => format!("{} — {detail}", step.title),
                            None => step.title.clone(),
                        }
                    ));
                }
            }
        }
        if !changed {
            spec.push_str("No step changes.\n");
        }
        spec.push_str(
            "\nThe orchestrator's responses (context only, never evidence of resolution):\n",
        );
        for response in &plan.responses {
            spec.push_str(&format!(
                "- {}: {}\n  {}: {}\n",
                response.id,
                response.finding,
                if response.accepted {
                    "Accepted"
                } else {
                    "Declined"
                },
                response.note
            ));
        }
        spec.push_str("Decide independently whether EACH previous finding is resolved in the new plan, including declined findings. An orchestrator response never resolves a finding. Raise every unresolved problem again in open_questions. Then focus on changed steps and their interactions with the rest of the plan; inspect unchanged steps as needed for those findings and interactions. No previous reviewer result carries over.\n\nThe revised plan: ");
    } else {
        spec.push_str("\nPlan: ");
    }
    spec.push_str(&format!("{}\n{}", plan.title, steps_text(&plan.steps)));
    spec.push_str("\nEnd with submit_report and a verdict: approve (with any notes for carrying it out in open_questions), or requestChanges with each problem the plan must fix as its own line in open_questions; Brigadier numbers them as findings the orchestrator must answer one by one.");
    spec
}

/// Who reviewed: "task-4 (Claude opus) and task-5 (Codex gpt-5)".
fn reviewers_text(reviewers: &[Task]) -> String {
    reviewers
        .iter()
        .map(|task| {
            format!(
                "task-{} ({})",
                task.number,
                super::workers::route_label(task)
            )
        })
        .collect::<Vec<_>>()
        .join(" and ")
}

/// A round's findings for the orchestrator, one line each with its id and reviewer.
fn findings_list(findings: &[Finding], reviewers: &[Task]) -> String {
    findings
        .iter()
        .map(|finding| {
            let by = reviewers
                .iter()
                .find(|task| task.id == finding.by)
                .map(|task| format!(" (task-{})", task.number))
                .unwrap_or_default();
            format!("{}{by}: {}", finding.id, finding.text)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn approved_text(plan: &Plan, reviewers: &[Task]) -> String {
    let mut text = format!(
        "[decision] The plan \"{}\" was reviewed by {} and approved on the user's behalf. Go ahead, and pass each step's number as `step` when you delegate it.",
        plan.title,
        reviewers_text(reviewers)
    );
    if !plan.review_notes.is_empty() {
        text.push_str("\nThe reviewers' notes, to weigh as you carry it out:");
        for note in &plan.review_notes {
            text.push_str(&format!("\n- {note}"));
        }
    }
    text
}

/// What the user's decision on a plan tells the orchestrator about its review.
pub(crate) fn review_for_decision(plan: &Plan) -> String {
    let mut text = String::new();
    if let Some(gate) = &plan.gate
        && !gate.findings.is_empty()
    {
        text.push_str(if gate.outcome == Some(GateOutcome::Superseded) {
            "\nThe independent review was still running when the user decided; its findings so far, which the user saw:"
        } else {
            "\nThe independent review's findings, which the user saw:"
        });
        for finding in &gate.findings {
            text.push_str(&format!("\n- {}: {}", finding.id, finding.text));
        }
    }
    if !plan.review_notes.is_empty() {
        text.push_str("\nThe independent review approved it, with these notes:");
        for note in &plan.review_notes {
            text.push_str(&format!("\n- {note}"));
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(title: &str, detail: Option<&str>) -> PlanStep {
        PlanStep {
            title: title.into(),
            detail: detail.map(Into::into),
            task_id: None,
        }
    }

    fn finding(id: &str, text: &str) -> Finding {
        Finding {
            id: id.into(),
            text: text.into(),
            by: TaskId("r".into()),
        }
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    fn plan(id: &str, request: &str, state: PlanState, steps: &[&str]) -> Plan {
        Plan {
            id: CardId(id.into()),
            conversation_id: ConversationId("c".into()),
            request_id: Some(request.into()),
            position: 0,
            title: format!("Plan {id}"),
            steps: steps.iter().map(|title| step(title, None)).collect(),
            risky: false,
            state,
            gate: None,
            review_skip_reason: None,
            revises: None,
            responses: Vec::new(),
            review_notes: Vec::new(),
            created_at_ms: 0,
            decided_at_ms: None,
        }
    }

    fn gate(round: u32, outcome: Option<GateOutcome>, members: Vec<GateMember>) -> Gate {
        Gate {
            verification_scope: Default::default(),
            rebased: false,
            round,
            commit: None,
            members,
            outcome,
            relanding: false,
            retry: false,
            overridden: false,
            findings: Vec::new(),
        }
    }

    fn member(id: &str, result: Option<GateResult>) -> GateMember {
        GateMember {
            task_id: TaskId(id.into()),
            role: GateRole::Review,
            result,
            avoid: Vec::new(),
        }
    }

    fn rejected() -> PlanState {
        PlanState::Rejected { message: None }
    }

    /// Plans by id.
    fn board(plans: Vec<Plan>) -> HashMap<CardId, Plan> {
        plans
            .into_iter()
            .map(|plan| (plan.id.clone(), plan))
            .collect()
    }

    /// p1 asked for changes, p2 revised it and still had problems after the last round.
    fn ran_out() -> HashMap<CardId, Plan> {
        let mut first = plan(
            "p1",
            "r",
            PlanState::Superseded,
            &["Add the API", "Wire the UI"],
        );
        first.gate = Some(gate(1, Some(GateOutcome::Failed), Vec::new()));
        let mut second = plan("p2", "r", rejected(), &["Add the API", "Wire it"]);
        second.revises = Some(first.id.clone());
        second.gate = Some(gate(PLAN_ROUNDS, Some(GateOutcome::Failed), Vec::new()));
        board(vec![first, second])
    }

    #[test]
    fn revision_spec_uses_stored_findings_and_a_step_diff() {
        let mut previous = plan(
            "p1",
            "r",
            PlanState::Revising,
            &["Keep", "Deploy", "Remove"],
        );
        previous.gate = Some(gate(
            1,
            Some(GateOutcome::Failed),
            vec![
                member("a", Some(GateResult::Passed)),
                member("b", Some(GateResult::Passed)),
            ],
        ));
        previous.gate.as_mut().unwrap().findings = vec![finding("F1", "No rollback")];
        let mut revision = plan("p2", "r", PlanState::Proposed, &["Keep", "Deploy safely"]);
        revision.steps[1].detail = Some("Use a transaction".into());
        revision.revises = Some(previous.id.clone());
        revision.responses = vec![FindingResponse {
            id: "F1".into(),
            finding: "orchestrator copy".into(),
            accepted: false,
            note: "unnecessary".into(),
        }];
        let spec = review_spec(&revision, Some(&previous), 2);
        assert!(spec.contains("F1: No rollback"));
        assert!(spec.contains("- 2. Deploy\n+ 2. Deploy safely — Use a transaction"));
        assert!(spec.contains("- 3. Remove"));
        assert!(!spec.contains("- 1. Keep"));
        assert!(spec.contains("context only, never evidence of resolution"));
        assert!(spec.contains("including declined findings"));
        assert!(spec.contains("changed steps and their interactions"));
        revision.steps = previous.steps.clone();
        let unchanged = review_spec(&revision, Some(&previous), 2);
        assert!(unchanged.contains("No step changes."));
        assert!(unchanged.contains("F1: No rollback"));
        assert!(unchanged.contains("Raise every unresolved problem again"));
        assert_eq!(reviewers_for(&revision, &board(vec![previous])), 2);
    }

    #[test]
    fn who_gets_reviewed() {
        // A single step goes at once; two or more get one reviewer; risky gets two.
        assert_eq!(plan_reviewers(1, false, false, None, false), 0);
        assert_eq!(plan_reviewers(2, false, false, None, false), 1);
        assert_eq!(plan_reviewers(1, true, false, None, false), 2);
        assert_eq!(plan_reviewers(5, true, true, None, false), 2);
        // A revision after a failed round is always reviewed again, even of one step.
        assert_eq!(plan_reviewers(1, false, true, None, false), 1);
        // After an approved plan, only a revision whose steps differ.
        assert_eq!(plan_reviewers(3, false, false, Some(false), false), 0);
        assert_eq!(plan_reviewers(1, false, false, Some(true), false), 1);
        // After a plan that ran out of review rounds, every plan, even of one step.
        assert_eq!(plan_reviewers(1, false, false, None, true), 1);
        assert_eq!(plan_reviewers(1, false, false, Some(false), true), 1);
    }

    #[test]
    fn a_plan_after_one_that_ran_out_of_rounds_is_reviewed() {
        let plans = ran_out();
        let mut next = plan("p3", "r", PlanState::Proposed, &["Just the API"]);
        next.created_at_ms = 1;
        assert_eq!(reviewers_for(&next, &plans), 1);
        // Another request's plan is not held to it.
        next.request_id = Some("other".into());
        assert_eq!(reviewers_for(&next, &plans), 0);
        // Nor after a round that only asked for changes, or one that could not run.
        let mut once = plan("p1", "r", rejected(), &["A", "B"]);
        once.gate = Some(gate(1, Some(GateOutcome::Failed), Vec::new()));
        let mut no_result = plan("p2", "r", rejected(), &["A", "B"]);
        no_result.gate = Some(gate(PLAN_ROUNDS, Some(GateOutcome::NoResult), Vec::new()));
        let mut next = plan("p3", "r", PlanState::Proposed, &["A"]);
        next.created_at_ms = 1;
        assert_eq!(reviewers_for(&next, &board(vec![once, no_result])), 0);
    }

    #[test]
    fn the_same_steps_after_running_out_of_rounds_are_refused() {
        let plans = ran_out();
        // The last plan's steps, spaced differently, and the plan it revised.
        assert_eq!(
            repeats_rejected(
                &plans,
                Some("r"),
                &[step("Add  the API", None), step("Wire it", None)]
            )
            .map(|p| p.id.0.as_str()),
            Some("p2")
        );
        assert!(
            repeats_rejected(
                &plans,
                Some("r"),
                &[step("Add the API", None), step("Wire the UI", None)]
            )
            .is_some()
        );
        // Other steps, or another request.
        assert!(repeats_rejected(&plans, Some("r"), &[step("Add the API", None)]).is_none());
        assert!(
            repeats_rejected(
                &plans,
                Some("x"),
                &[step("Add the API", None), step("Wire it", None)]
            )
            .is_none()
        );
    }

    #[test]
    fn removed_moved_repeated_or_changed_steps_are_material() {
        let before = [
            step("Add authorization", Some("in auth.rs")),
            step("Expose the endpoint", None),
            step("Test the endpoint", None),
        ];
        // Only spacing changes nothing material.
        assert!(!steps_differ(
            &before,
            &[
                step("Add  authorization", Some(" in auth.rs")),
                step("Expose the endpoint", None),
                step("Test the\nendpoint", None),
            ]
        ));
        // A prerequisite removed.
        assert!(steps_differ(&before, &before[1..]));
        // Reordered.
        assert!(steps_differ(
            &before,
            &[before[1].clone(), before[0].clone(), before[2].clone()]
        ));
        // A step repeated.
        assert!(steps_differ(
            &before,
            &[
                before[0].clone(),
                before[1].clone(),
                before[2].clone(),
                before[2].clone()
            ]
        ));
        // A path's case changed, a detail changed, a step added.
        assert!(steps_differ(
            &before,
            &[
                step("Add authorization", Some("in Auth.rs")),
                before[1].clone(),
                before[2].clone()
            ]
        ));
        assert!(steps_differ(
            &before,
            &[
                step("Add authorization", Some("in api.rs")),
                before[1].clone(),
                before[2].clone()
            ]
        ));
        assert!(steps_differ(&before[..2], &before));
    }

    #[test]
    fn writes_wait_for_a_plan_in_review_or_being_revised() {
        let open = |state| vec![plan("p", "r", state, &["A", "B"])];
        for state in [
            PlanState::Proposed,
            PlanState::InReview {
                task_id: TaskId("t".into()),
            },
        ] {
            let why = review_blocks_writes(&open(state), Some("r")).expect("held back");
            assert!(why.contains("wait for the plan review"), "{why}");
        }
        let why = review_blocks_writes(&open(PlanState::Revising), Some("r")).expect("held back");
        assert!(why.contains("revise the plan first"), "{why}");
        // A decided plan, or another request's, holds nothing back.
        assert!(review_blocks_writes(&open(rejected()), Some("r")).is_none());
        assert!(review_blocks_writes(&open(PlanState::Revising), Some("other")).is_none());
    }

    #[test]
    fn a_restart_keeps_the_results_a_round_already_has() {
        let failed = GateResult::Failed {
            findings: vec!["No rollback".into()],
        };
        let mut reviewing = plan(
            "p",
            "r",
            PlanState::InReview {
                task_id: TaskId("a".into()),
            },
            &["A", "B"],
        );
        reviewing.gate = Some(gate(
            1,
            None,
            vec![member("a", Some(failed.clone())), member("b", None)],
        ));
        assert_eq!(
            rerun_of(&reviewing, &HashMap::new(), true),
            Some(Rerun::Review {
                round: 1,
                keep: vec![member("a", Some(failed))],
                start: 1,
                decides: true,
            })
        );
        // A closed round has nothing to redo.
        reviewing.gate = Some(gate(1, Some(GateOutcome::Failed), vec![member("a", None)]));
        assert_eq!(rerun_of(&reviewing, &HashMap::new(), true), None);
    }

    #[test]
    fn a_restart_starts_a_review_that_never_started() {
        let proposed = plan("p", "r", PlanState::Proposed, &["A", "B"]);
        assert_eq!(
            rerun_of(&proposed, &HashMap::new(), true),
            Some(Rerun::Review {
                round: 1,
                keep: Vec::new(),
                start: 1,
                decides: true,
            })
        );
        // A gate with no reviewers counts as none.
        let mut empty = proposed.clone();
        empty.gate = Some(gate(1, None, Vec::new()));
        assert!(matches!(
            rerun_of(&empty, &HashMap::new(), true),
            Some(Rerun::Review { start: 1, .. })
        ));
        // The user decides it, or it needs no review.
        assert_eq!(rerun_of(&proposed, &HashMap::new(), false), None);
        let single = plan("p", "r", PlanState::Proposed, &["A"]);
        assert_eq!(rerun_of(&single, &HashMap::new(), true), None);
        // A revision, in the round after its predecessor's.
        let mut previous = plan("p0", "r", PlanState::Superseded, &["A", "B"]);
        previous.gate = Some(gate(1, Some(GateOutcome::Failed), Vec::new()));
        let mut revision = plan("p", "r", PlanState::Proposed, &["A"]);
        revision.revises = Some(previous.id.clone());
        assert_eq!(
            rerun_of(&revision, &board(vec![previous]), true),
            Some(Rerun::Review {
                round: 2,
                keep: Vec::new(),
                start: 1,
                decides: true,
            })
        );
        // A plan waiting for its revision is asked for again.
        let revising = plan("p", "r", PlanState::Revising, &["A", "B"]);
        assert_eq!(
            rerun_of(&revising, &HashMap::new(), true),
            Some(Rerun::Revise)
        );
    }

    #[test]
    fn findings_are_numbered_as_results_arrive() {
        let mut findings = Vec::new();
        // The second reviewer reports first; its ids stay when the first one's arrive.
        record_findings(
            &mut findings,
            &member(
                "b",
                Some(GateResult::Failed {
                    findings: vec!["Step 2 misses the migration".into(), "  ".into()],
                }),
            ),
        );
        record_findings(&mut findings, &member("c", Some(GateResult::Passed)));
        record_findings(
            &mut findings,
            &member(
                "a",
                Some(GateResult::Failed {
                    findings: vec!["No rollback step".into(), "Too broad".into()],
                }),
            ),
        );
        // Recorded once per reviewer.
        record_findings(
            &mut findings,
            &member(
                "a",
                Some(GateResult::Failed {
                    findings: vec!["No rollback step".into()],
                }),
            ),
        );
        assert_eq!(
            findings,
            vec![
                Finding {
                    id: "F1".into(),
                    text: "Step 2 misses the migration".into(),
                    by: TaskId("b".into())
                },
                Finding {
                    id: "F2".into(),
                    text: "No rollback step".into(),
                    by: TaskId("a".into())
                },
                Finding {
                    id: "F3".into(),
                    text: "Too broad".into(),
                    by: TaskId("a".into())
                },
            ]
        );
    }

    #[test]
    fn responses_answer_each_finding() {
        let findings = [finding("F1", "No rollback"), finding("F2", "Too broad")];
        let responses = parse_responses(
            &lines(&[
                "- f2 declined: the scope is what the user asked for",
                "F1 accepted: added step 4, a rollback",
            ]),
            &findings,
        )
        .expect("parsed");
        assert_eq!(
            responses,
            vec![
                FindingResponse {
                    id: "F1".into(),
                    finding: "No rollback".into(),
                    accepted: true,
                    note: "added step 4, a rollback".into(),
                },
                FindingResponse {
                    id: "F2".into(),
                    finding: "Too broad".into(),
                    accepted: false,
                    note: "the scope is what the user asked for".into(),
                },
            ]
        );
        // Several lines in one string, and "F1: accepted" read the same.
        let joined = parse_responses(
            &lines(&["F1: accepted\nF2 decline - not needed"]),
            &findings,
        )
        .expect("parsed");
        assert!(joined[0].accepted && !joined[1].accepted);
        assert_eq!(joined[1].note, "not needed");
    }

    #[test]
    fn an_unanswered_or_bad_response_is_refused() {
        let findings = [finding("F1", "No rollback"), finding("F2", "Too broad")];
        let err = parse_responses(&lines(&["F1 accepted: done"]), &findings).unwrap_err();
        assert!(err.contains("unanswered: F2"), "{err}");
        let err = parse_responses(&lines(&["F1 accepted", "F2 declined"]), &findings).unwrap_err();
        assert!(err.contains("without a reason"), "{err}");
        let err = parse_responses(&lines(&["F3 accepted: x"]), &findings).unwrap_err();
        assert!(err.contains("answers no finding"), "{err}");
        let err = parse_responses(&lines(&["F1 maybe later"]), &findings).unwrap_err();
        assert!(err.contains("neither accepts nor declines"), "{err}");
        let err = parse_responses(
            &lines(&["F1 accepted", "F1 declined: no", "F2 accepted"]),
            &findings,
        )
        .unwrap_err();
        assert!(err.contains("twice"), "{err}");
    }
}
