//! Whole-phase checks (PLAN.md §10.6, steps 5–8). Once a phase's lead says its work is done,
//! the run branch's tip is the phase's candidate, and one round checks it as a whole:
//!
//! - a **fresh verifier** (not one of the phase's models) proves every criterion, by its id,
//!   on a checkout of the candidate, and runs the project's checks;
//! - a **reviewer from another vendor** than the work's authors reads the whole diff from the
//!   phase's start (one per authoring vendor, so each vendor's work is checked by the other);
//! - then a **fresh judge**, routed as orchestration, reads the scope, the criteria, the
//!   user's Rules and words and both results, and gives one verdict per criterion.
//!
//! Code, not the judge alone, accepts the phase: only when the verifier passed and showed
//! every criterion met, every reviewer of another vendor approved, the judge approved with
//! every criterion met exactly once, and the candidate and the run's generation are still the
//! ones checked. Otherwise the lead gets the gaps to fix (at most two rounds), and after that
//! the phase settles as partial or blocked, with what only the user can do listed for them.

use std::collections::HashMap;

use brigadier_providers::ProviderKind;
use brigadier_router::Author;

use super::super::SessionManager;
use super::super::conversation::Envelope;
use super::super::gates::{
    FIX_ROUNDS, criterion_evidence, review_result, verify_result, without_marker,
};
use super::super::plan_gates::record_findings;
use super::super::workers::TaskExtra;
use super::policy::PLANNING_PHASE;
use crate::model::OvernightRunId;
use crate::now_ms;
use crate::overnight::{
    CriterionResult, CriterionStatus, OvernightPhase, OvernightRun, OvernightState, PhaseState,
    RunRole, RunTaskContext,
};
use crate::work::{
    Gate, GateLink, GateMember, GateOutcome, GateOwner, GateResult, GateRole, InjectionKind,
    Report, ReviewVerdict, Task, TaskKind,
};

/// Rounds of whole-phase checks one phase may go through (fixes, a candidate that moved)
/// before it settles as it is.
const MAX_ROUNDS: u32 = 6;

/// What a decided round comes to.
enum Verdict {
    Verified(Vec<CriterionResult>),
    /// Gaps a worker can fix: the lead gets them.
    Fix(String),
    /// Settled without being verified.
    Settle {
        state: PhaseState,
        criteria: Vec<CriterionResult>,
        gaps: Vec<String>,
        asks: Vec<String>,
    },
    /// The candidate moved while it was checked: check the branch's new tip.
    Recheck,
    /// The verifier couldn't conclude (why): a second verifier checks the same candidate, once.
    Retry(String),
}

impl SessionManager {
    /// Opens a round of whole-phase checks on `candidate`.
    pub(crate) async fn open_phase_gate(
        &self,
        run: &OvernightRun,
        phase_id: &str,
        candidate: String,
    ) {
        self.open_phase_round(run, phase_id, candidate, None).await;
    }

    /// A round of whole-phase checks; `retry` says why the last verifier couldn't conclude.
    async fn open_phase_round(
        &self,
        run: &OvernightRun,
        phase_id: &str,
        candidate: String,
        retry: Option<String>,
    ) {
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(phase) = board
            .runs
            .get(&run.id)
            .and_then(|now| now.phase(phase_id))
            .cloned()
        else {
            return;
        };
        let round = phase.gate.as_ref().map_or(1, |gate| gate.round + 1);
        // The phase's authors: its landed work and its lead.
        let mut authors: Vec<Author> = board
            .tasks
            .values()
            .filter(|task| {
                task.kind.writes()
                    && task.run.as_ref().is_some_and(|context| {
                        context.run_id == run.id && context.phase_id.as_deref() == Some(phase_id)
                    })
            })
            .map(|task| Author {
                provider: task.route.choice.provider,
                model: task.route.choice.model.clone(),
            })
            .collect();
        if let Some(lead) = &phase.lead {
            authors.push(Author {
                provider: lead.provider,
                model: lead.model.clone(),
            });
        }
        let mut vendors: Vec<ProviderKind> = authors.iter().map(|a| a.provider).collect();
        vendors.sort_by_key(|vendor| format!("{vendor:?}"));
        vendors.dedup();
        let owner = GateOwner::Phase {
            run_id: run.id.clone(),
            phase_id: phase_id.to_owned(),
        };
        let context = |role: RunRole| RunTaskContext {
            run_id: run.id.clone(),
            segment: run.segment,
            phase_id: Some(phase_id.to_owned()),
            generation: run.generation,
            role,
            rules_hash: super::policy::rules_hash(&run.rules),
            candidate: Some(candidate.clone()),
        };
        let mut members: Vec<GateMember> = Vec::new();
        let mut gaps: Vec<(crate::model::TaskId, String)> = Vec::new();
        // A check the round needs that could not start: without it the round can't verify.
        let mut missing = false;
        let verifier = self
            .create_task_as(
                &run.conversation_id,
                format!("Verify phase {}", phase.number),
                TaskKind::Verify,
                verify_spec(
                    run,
                    &phase,
                    &candidate,
                    retry.as_deref(),
                    self.permission(&run.conversation_id)
                        != crate::model::PermissionLevel::FullAccess,
                ),
                None,
                None,
                // A fresh model: none of the phase's own.
                authors.clone(),
                Some(GateLink {
                    owner: owner.clone(),
                    round,
                    role: GateRole::Verify,
                }),
                None,
                Vec::new(),
                None,
                Some(brigadier_router::QualityTier::Strong),
                Vec::new(),
                TaskExtra {
                    run: Some(context(RunRole::PhaseVerifier)),
                    category: None,
                    request: phase.request_id.clone(),
                },
            )
            .await;
        match verifier {
            Ok(task) => members.push(GateMember {
                task_id: task.id,
                role: GateRole::Verify,
                result: None,
                avoid: Vec::new(),
            }),
            Err(err) => {
                missing = true;
                tracing::warn!(run = %run.id, error = %err, "could not start a phase's verifier");
            }
        }
        let mut reviewing: Vec<Author> = Vec::new();
        for vendor in &vendors {
            let review = self
                .create_task_as(
                    &run.conversation_id,
                    if vendors.len() > 1 {
                        format!(
                            "Review phase {} ({} work)",
                            phase.number,
                            vendor_name(*vendor)
                        )
                    } else {
                        format!("Review phase {}", phase.number)
                    },
                    TaskKind::Review,
                    review_spec(run, &phase, &candidate, *vendor, vendors.len() > 1),
                    None,
                    Some(Author {
                        provider: *vendor,
                        model: None,
                    }),
                    reviewing.clone(),
                    Some(GateLink {
                        owner: owner.clone(),
                        round,
                        role: GateRole::Review,
                    }),
                    None,
                    Vec::new(),
                    None,
                    Some(brigadier_router::QualityTier::Strong),
                    Vec::new(),
                    TaskExtra {
                        run: Some(context(RunRole::PhaseReviewer)),
                        category: None,
                        request: phase.request_id.clone(),
                    },
                )
                .await;
            match review {
                Ok(task) => {
                    reviewing.push(Author {
                        provider: task.route.choice.provider,
                        model: task.route.choice.model.clone(),
                    });
                    // The same vendor reviewing its own work is no independent review.
                    let result = (task.route.choice.provider == *vendor).then(|| {
                        GateResult::NoResult {
                            reason: format!(
                                "No model of another vendor than {} could review its work, so the phase has no independent review of it.",
                                vendor_name(*vendor)
                            ),
                        }
                    });
                    if result.is_some() {
                        gaps.push((task.id.clone(), String::new()));
                    }
                    members.push(GateMember {
                        task_id: task.id,
                        role: GateRole::Review,
                        result,
                        avoid: vec![crate::model::ModelChoice {
                            provider: *vendor,
                            model: None,
                            effort: None,
                            fast: None,
                        }],
                    });
                }
                Err(err) => {
                    missing = true;
                    tracing::warn!(run = %run.id, error = %err, "could not start a phase's reviewer");
                }
            }
        }
        if missing {
            // The checks that did start can't make up for it: they stop, and the phase settles
            // unverified, saying its checks could not start.
            for member in members.drain(..) {
                let _ = Box::pin(self.stop_task(member.task_id)).await;
            }
            gaps.clear();
        }
        let started: Vec<crate::model::TaskId> = members
            .iter()
            .map(|member| member.task_id.clone())
            .collect();
        let candidate_now = candidate.clone();
        let recorded = self
            .change_run_if(run, |now| {
                let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                if phase.state != PhaseState::Checking {
                    return None;
                }
                let gate = Gate {
                    verification_scope: Default::default(),
                    rebased: false,
                    round,
                    commit: Some(candidate_now.clone()),
                    members: members.clone(),
                    outcome: None,
                    relanding: false,
                    retry: retry.is_some(),
                    overridden: false,
                    findings: Vec::new(),
                };
                // An empty round stays undecided: deciding the phase closes it.
                phase.gate = Some(gate);
                now.state = OvernightState::PhaseGate;
                Some(())
            })
            .await;
        match recorded {
            None => {
                for task in started {
                    let _ = Box::pin(self.stop_task(task)).await;
                }
            }
            Some(now) => {
                // A reviewer of the authors' own vendor is no use: it stops (its result is
                // recorded already).
                for (task, _) in gaps {
                    let _ = Box::pin(self.stop_task(task)).await;
                }
                if members.is_empty() {
                    Box::pin(self.decide_phase(&now, phase_id)).await;
                }
            }
        }
    }

    /// A member of a phase's checks (or Phase 0's judge) reported, or (`failed`) ended without
    /// a result.
    /// Boxed with a named type: a round's members are tasks whose results come back here.
    pub(crate) fn phase_member_done<'a>(
        &'a self,
        member: &'a Task,
        run_id: &'a OvernightRunId,
        phase_id: &'a str,
        link: &'a GateLink,
        failed: Option<&'a str>,
    ) -> brigadier_providers::BoxFuture<'a, ()> {
        Box::pin(self.phase_member_result(member, run_id, phase_id, link, failed))
    }

    async fn phase_member_result(
        &self,
        member: &Task,
        run_id: &OvernightRunId,
        phase_id: &str,
        link: &GateLink,
        failed: Option<&str>,
    ) {
        // Ended by the restart, not by its own result: the round starts again afterwards.
        if failed.is_some()
            && self
                .overnight
                .recovering
                .load(std::sync::atomic::Ordering::Acquire)
        {
            return;
        }
        if phase_id == PLANNING_PHASE {
            self.planning_judged(member, run_id, failed).await;
            return;
        }
        let result = match (failed, &member.report, link.role) {
            (Some(reason), _, role) => GateResult::NoResult {
                reason: format!(
                    "The phase's {} gave no result: {reason}",
                    match role {
                        GateRole::Verify => "verifier",
                        GateRole::Review => "reviewer",
                        GateRole::Judge => "judge",
                    },
                ),
            },
            (None, None, _) => return,
            (None, Some(report), GateRole::Verify) => match self.verifier_changes(member).await {
                Some(changed) => GateResult::NoResult {
                    reason: format!(
                        "The phase's verifier changed what it checked ({changed}), so its result was discarded."
                    ),
                },
                None => {
                    let criteria = self
                        .core
                        .board(&member.conversation_id)
                        .await
                        .ok()
                        .and_then(|board| {
                            board
                                .runs
                                .get(run_id)?
                                .phase(phase_id)
                                .map(|p| p.done_when.len())
                        })
                        .unwrap_or(0);
                    verify_result(report, criteria)
                }
            },
            (None, Some(report), GateRole::Review | GateRole::Judge) => review_result(report),
        };
        let Some(board) = self.core.board(&member.conversation_id).await.ok() else {
            return;
        };
        let Some(run) = board.runs.get(run_id).cloned() else {
            return;
        };
        let member_id = member.id.clone();
        let round = link.round;
        let mut ready = false;
        let mut decided = false;
        let recorded = self
            .change_run_if(&run, |now| {
                let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                let gate = phase.gate.as_mut()?;
                if gate.round != round || gate.outcome.is_some() {
                    return None;
                }
                let slot = gate
                    .members
                    .iter_mut()
                    .find(|known| known.task_id == member_id)
                    .filter(|slot| slot.result.is_none())?;
                slot.result = Some(result.clone());
                let slot = slot.clone();
                if slot.role == GateRole::Review {
                    record_findings(&mut gate.findings, &slot);
                }
                let judged = gate
                    .members
                    .iter()
                    .any(|m| m.role == GateRole::Judge && m.result.is_some());
                let checked = gate
                    .members
                    .iter()
                    .filter(|m| m.role != GateRole::Judge)
                    .all(|m| m.result.is_some());
                let judging = gate.members.iter().any(|m| m.role == GateRole::Judge);
                decided = judged;
                ready = checked && !judging;
                Some(())
            })
            .await;
        let Some(now) = recorded else {
            return;
        };
        if decided {
            self.decide_phase(&now, phase_id).await;
        } else if ready {
            self.start_judge(&now, phase_id).await;
        }
    }

    /// The verifier and reviewers have results: a fresh judge decides from them.
    async fn start_judge(&self, run: &OvernightRun, phase_id: &str) {
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(phase) = run.phase(phase_id).cloned() else {
            return;
        };
        let Some(gate) = phase.gate.clone() else {
            return;
        };
        let Some(candidate) = gate.commit.clone() else {
            return;
        };
        // Work landed on the branch while it was checked: judging that candidate is moot.
        if self.run_tip(run).await.is_ok_and(|tip| tip != candidate) {
            Box::pin(self.decide_phase(run, phase_id)).await;
            return;
        }
        let members: Vec<(&GateMember, Option<&Task>)> = gate
            .members
            .iter()
            .map(|member| (member, board.tasks.get(&member.task_id)))
            .collect();
        let spec = judge_spec(run, &phase, &candidate, &members, &gate, &board);
        let judge = self
            .create_task_as(
                &run.conversation_id,
                format!("Judge phase {}", phase.number),
                TaskKind::Review,
                spec,
                None,
                None,
                Vec::new(),
                Some(GateLink {
                    owner: GateOwner::Phase {
                        run_id: run.id.clone(),
                        phase_id: phase_id.to_owned(),
                    },
                    round: gate.round,
                    role: GateRole::Judge,
                }),
                None,
                Vec::new(),
                None,
                Some(brigadier_router::QualityTier::Strong),
                Vec::new(),
                TaskExtra {
                    run: Some(RunTaskContext {
                        run_id: run.id.clone(),
                        segment: run.segment,
                        phase_id: Some(phase_id.to_owned()),
                        generation: run.generation,
                        role: RunRole::Judge,
                        rules_hash: super::policy::rules_hash(&run.rules),
                        candidate: Some(candidate.clone()),
                    }),
                    category: Some(brigadier_router::TaskCategory::Orchestrate),
                    request: phase.request_id.clone(),
                },
            )
            .await;
        let judge = match judge {
            Ok(judge) => judge,
            Err(err) => {
                tracing::warn!(run = %run.id, error = %err, "could not start a phase's judge");
                // Without a judge the phase can't be verified: it settles on what it has.
                let recorded = self
                    .change_run_if(run, |now| {
                        let gate = now
                            .phases
                            .iter_mut()
                            .find(|p| p.id == phase_id)?
                            .gate
                            .as_mut()?;
                        gate.members.push(GateMember {
                            task_id: crate::model::TaskId("no-judge".into()),
                            role: GateRole::Judge,
                            result: Some(GateResult::NoResult {
                                reason: format!("No judge could start: {err}"),
                            }),
                            avoid: Vec::new(),
                        });
                        Some(())
                    })
                    .await;
                if let Some(now) = recorded {
                    self.decide_phase(&now, phase_id).await;
                }
                return;
            }
        };
        let judge_id = judge.id.clone();
        let round = gate.round;
        let recorded = self
            .change_run_if(run, |now| {
                let gate = now
                    .phases
                    .iter_mut()
                    .find(|p| p.id == phase_id)?
                    .gate
                    .as_mut()?;
                if gate.round != round || gate.outcome.is_some() {
                    return None;
                }
                gate.members.push(GateMember {
                    task_id: judge_id.clone(),
                    role: GateRole::Judge,
                    result: None,
                    avoid: Vec::new(),
                });
                Some(())
            })
            .await;
        if recorded.is_none() {
            let _ = Box::pin(self.stop_task(judge.id)).await;
        }
    }

    /// Every member of the round has its result: the phase is verified, sent back to its lead
    /// with what to fix, or settled as it is.
    async fn decide_phase(&self, run: &OvernightRun, phase_id: &str) {
        let Ok(board) = self.core.board(&run.conversation_id).await else {
            return;
        };
        let Some(phase) = run.phase(phase_id).cloned() else {
            return;
        };
        let Some(gate) = phase.gate.clone() else {
            return;
        };
        let reports: HashMap<crate::model::TaskId, &Task> = gate
            .members
            .iter()
            .filter_map(|member| Some((member.task_id.clone(), board.tasks.get(&member.task_id)?)))
            .collect();
        let tip = self.run_tip(run).await.ok();
        let winding_down = !matches!(
            run.state,
            OvernightState::Running | OvernightState::PhaseGate
        );
        let verdict = verdict_of(&phase, &gate, &reports, tip.as_deref(), winding_down);
        let request = phase.request_id.clone();
        let number = phase.number;
        match verdict {
            Verdict::Recheck => {
                let Some(tip) = tip else {
                    return;
                };
                let reopened = self
                    .change_run_if(run, |now| {
                        let gate = now
                            .phases
                            .iter_mut()
                            .find(|p| p.id == phase_id)?
                            .gate
                            .as_mut()?;
                        gate.outcome.is_none().then_some(())?;
                        gate.outcome = Some(GateOutcome::NoResult);
                        Some(())
                    })
                    .await;
                if let Some(now) = reopened {
                    Box::pin(self.open_phase_gate(&now, phase_id, tip)).await;
                }
            }
            Verdict::Retry(why) => {
                let Some(candidate) = gate.commit.clone() else {
                    return;
                };
                let reopened = self
                    .change_run_if(run, |now| {
                        let gate = now
                            .phases
                            .iter_mut()
                            .find(|p| p.id == phase_id)?
                            .gate
                            .as_mut()?;
                        gate.outcome.is_none().then_some(())?;
                        gate.outcome = Some(GateOutcome::Unverified);
                        Some(())
                    })
                    .await;
                if let Some(now) = reopened {
                    Box::pin(self.open_phase_round(&now, phase_id, candidate, Some(why))).await;
                }
            }
            Verdict::Verified(criteria) => {
                let candidate = gate.commit.clone();
                let Some(now) = self
                    .change_run_if(run, |now| {
                        let all_verified = now
                            .phases
                            .iter()
                            .filter(|p| p.id != phase_id && p.start_commit.is_some())
                            .all(|p| p.state == PhaseState::Verified);
                        let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                        let gate = phase.gate.as_mut()?;
                        gate.outcome.is_none().then_some(())?;
                        gate.outcome = Some(GateOutcome::Passed);
                        phase.state = PhaseState::Verified;
                        phase.verified_commit = candidate.clone();
                        phase.criteria = criteria.clone();
                        phase.gaps.clear();
                        phase.settled_at_ms = Some(now_ms());
                        // The merge point moves only while everything before it is verified.
                        if all_verified {
                            now.verified_commit = candidate.clone();
                        }
                        if now.state == OvernightState::PhaseGate {
                            now.state = OvernightState::Running;
                        }
                        Some(())
                    })
                    .await
                else {
                    return;
                };
                self.phase_outcome_decided(
                    &now,
                    phase_id,
                    request,
                    format!("Verified phase {number} \u{201c}{}\u{201d}", phase.name),
                    format!(
                        "All {} criteria met. A fresh verifier, another vendor's reviewer and a judge agreed.",
                        phase.done_when.len()
                    ),
                )
                .await;
                self.phase_verified_waiting(&now.conversation_id, &now.id, number)
                    .await;
                self.advance_soon(&now.conversation_id, &now.id);
            }
            Verdict::Fix(findings) => {
                let Some(now) = self
                    .change_run_if(run, |now| {
                        let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                        let gate = phase.gate.as_mut()?;
                        gate.outcome.is_none().then_some(())?;
                        gate.outcome = Some(GateOutcome::Failed);
                        phase.state = PhaseState::Running;
                        phase.fix_rounds += 1;
                        phase.nudges = 0;
                        if now.state == OvernightState::PhaseGate {
                            now.state = OvernightState::Running;
                        }
                        Some(())
                    })
                    .await
                else {
                    return;
                };
                let round = now.phase(phase_id).map_or(0, |p| p.fix_rounds);
                self.run_decided(
                    &now,
                    Some(phase_id),
                    request.clone(),
                    format!("Sent phase {number} back to its lead (fix {round} of {FIX_ROUNDS})"),
                    "The findings are on the phase's checks.".to_owned(),
                )
                .await;
                self.deliver_for(
                    &now.conversation_id,
                    Envelope {
                        kind: InjectionKind::Phase,
                        label: format!("phase {number} checks"),
                        task_id: None,
                        text: format!(
                            "[overnight · phase {number} checks] The whole-phase checks found gaps, so phase {number} is not verified yet (fix round {round} of {FIX_ROUNDS}):\n{findings}\nFix each one through tasks (accept them as usual: they land on the run branch after their own checks). Then call phase_done again, with one response per finding (\"F1 fixed: how\", \"F2 declined: why\"). Don't widen the phase beyond its scope."
                        ),
                    },
                    request,
                )
                .await;
            }
            Verdict::Settle {
                state,
                criteria,
                gaps,
                asks,
            } => {
                let Some(now) = self
                    .change_run_if(run, |now| {
                        let phase = now.phases.iter_mut().find(|p| p.id == phase_id)?;
                        let gate = phase.gate.as_mut()?;
                        gate.outcome.is_none().then_some(())?;
                        gate.outcome = Some(GateOutcome::Unverified);
                        phase.state = state;
                        phase.criteria = criteria.clone();
                        phase.gaps = gaps.clone();
                        phase.settled_at_ms = Some(now_ms());
                        if now.state == OvernightState::PhaseGate {
                            now.state = OvernightState::Running;
                        }
                        Some(())
                    })
                    .await
                else {
                    return;
                };
                for ask in &asks {
                    self.run_waits(
                        &now,
                        request.clone(),
                        None,
                        &format!("Phase {number}: {ask}"),
                    )
                    .await;
                }
                let met = criteria
                    .iter()
                    .filter(|c| c.status == CriterionStatus::Met)
                    .count();
                self.phase_outcome_decided(
                    &now,
                    phase_id,
                    request,
                    format!(
                        "Settled phase {number} \u{201c}{}\u{201d} as {}",
                        phase.name,
                        if state == PhaseState::Partial {
                            "partial"
                        } else {
                            "blocked"
                        }
                    ),
                    format!(
                        "{met} of {} criteria met. What's missing is in the phase's checks.",
                        phase.done_when.len()
                    ),
                )
                .await;
                self.advance_soon(&now.conversation_id, &now.id);
            }
        }
    }

    /// Who a phase check must not be, for a hand-off to another model: a reviewer, the
    /// vendor whose work it covers; any member, the models it started avoiding.
    pub(crate) async fn phase_gate_avoid(
        &self,
        member: &Task,
        run_id: &OvernightRunId,
        phase_id: &str,
    ) -> (Option<Author>, Vec<Author>) {
        let Ok(board) = self.core.board(&member.conversation_id).await else {
            return (None, Vec::new());
        };
        let slot = board
            .runs
            .get(run_id)
            .and_then(|run| run.phase(phase_id))
            .and_then(|phase| phase.gate.as_ref())
            .and_then(|gate| gate.members.iter().find(|m| m.task_id == member.id))
            .cloned();
        let avoid: Vec<Author> = slot
            .map(|slot| {
                slot.avoid
                    .iter()
                    .map(|choice| Author {
                        provider: choice.provider,
                        model: choice.model.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        (avoid.first().cloned(), avoid)
    }

    /// Phase 0's plan was approved by its review: a fresh judge checks it against the goal.
    pub(crate) async fn judge_planning(&self, run: &OvernightRun) {
        let Some(planning) = run.planning.clone() else {
            return;
        };
        let round = planning.rounds + 1;
        let judge = self
            .create_task_as(
                &run.conversation_id,
                "Judge the overnight plan".into(),
                TaskKind::Review,
                planning_judge_spec(run, &planning.proposed),
                None,
                None,
                Vec::new(),
                Some(GateLink {
                    owner: GateOwner::Phase {
                        run_id: run.id.clone(),
                        phase_id: PLANNING_PHASE.into(),
                    },
                    round,
                    role: GateRole::Judge,
                }),
                None,
                Vec::new(),
                None,
                Some(brigadier_router::QualityTier::Strong),
                Vec::new(),
                TaskExtra {
                    run: Some(RunTaskContext {
                        run_id: run.id.clone(),
                        segment: run.segment,
                        phase_id: Some(PLANNING_PHASE.into()),
                        generation: run.generation,
                        role: RunRole::Judge,
                        rules_hash: super::policy::rules_hash(&run.rules),
                        candidate: None,
                    }),
                    category: Some(brigadier_router::TaskCategory::Orchestrate),
                    request: Some(planning.request_id.clone()),
                },
            )
            .await;
        match judge {
            Ok(judge) => {
                let id = judge.id.clone();
                if self
                    .change_run_if(run, |now| {
                        let planning = now.planning.as_mut()?;
                        planning.judge = Some(id.clone());
                        planning.rounds = round;
                        Some(())
                    })
                    .await
                    .is_none()
                {
                    let _ = Box::pin(self.stop_task(judge.id)).await;
                }
            }
            Err(err) => {
                self.planning_blocked(run, vec![format!(
                    "Phase 0's plan could not be judged against the goal: {err}. Continue once a model is available."
                )])
                .await;
            }
        }
    }

    /// Phase 0's judge reported (or failed): its phases become the run's plan, go back to the
    /// lead with what to change, or Phase 0 ends blocked.
    async fn planning_judged(&self, member: &Task, run_id: &OvernightRunId, failed: Option<&str>) {
        let Ok(board) = self.core.board(&member.conversation_id).await else {
            return;
        };
        let Some(run) = board.runs.get(run_id).cloned() else {
            return;
        };
        let Some(planning) = run.planning.clone() else {
            return;
        };
        if planning.judge.as_ref() != Some(&member.id) || planning.state != PhaseState::Checking {
            return;
        }
        let report = member.report.as_ref().filter(|_| failed.is_none());
        let approved = report.is_some_and(|report| report.verdict == Some(ReviewVerdict::Approve));
        if approved {
            let phases: Vec<OvernightPhase> = planning
                .proposed
                .iter()
                .enumerate()
                .map(|(index, phase)| {
                    OvernightPhase::new(
                        phase.number.unwrap_or(index as u32 + 1),
                        &phase.name,
                        &phase.scope,
                        &phase.done_when,
                        &phase.depends_on,
                    )
                })
                .collect();
            let Some(now) = self
                .change_run_if(&run, |now| {
                    if now.state != OvernightState::Planning {
                        return None;
                    }
                    let planning = now.planning.as_mut()?;
                    planning.state = PhaseState::Verified;
                    planning.settled_at_ms = Some(now_ms());
                    now.phases = phases.clone();
                    let directives = now.directives.clone();
                    for phase in &mut now.phases {
                        let selected = directives
                            .only
                            .is_none_or(|range| (range.from..=range.to).contains(&phase.number))
                            && !directives.skip.contains(&phase.number);
                        if !selected {
                            phase.state = PhaseState::Skipped;
                        }
                    }
                    now.state = OvernightState::Running;
                    Some(())
                })
                .await
            else {
                return;
            };
            self.run_decided(
                &now,
                Some(PLANNING_PHASE),
                Some(planning.request_id.clone()),
                format!("Approved the overnight plan \u{201c}{}\u{201d}", now.name),
                format!(
                    "Another vendor reviewed its {} phases; a judge found they follow the goal.",
                    now.phases.len()
                ),
            )
            .await;
            self.advance_soon(&now.conversation_id, &now.id);
            return;
        }
        let gaps: Vec<String> = match report {
            Some(report) if !report.open_questions.is_empty() => report.open_questions.clone(),
            Some(report) => vec![report.summary.clone()],
            None => vec![format!(
                "The judge of the plan gave no result: {}",
                failed.unwrap_or("it ended without a report")
            )],
        };
        if planning.rounds >= FIX_ROUNDS || report.is_none() {
            self.planning_blocked(
                &run,
                vec![format!(
                    "Phase 0's plan doesn't follow the goal yet: {}",
                    gaps.join("; ")
                )],
            )
            .await;
            return;
        }
        let Some(now) = self
            .change_run_if(&run, |now| {
                let planning = now.planning.as_mut()?;
                planning.state = PhaseState::Running;
                planning.gaps = gaps.clone();
                planning.nudges = 0;
                Some(())
            })
            .await
        else {
            return;
        };
        self.deliver_for(
            &now.conversation_id,
            Envelope {
                kind: InjectionKind::Phase,
                label: "phase 0 judgement".into(),
                task_id: Some(member.id.clone()),
                text: format!(
                    "[overnight · phase 0 judgement] A fresh judge compared the approved phases with the user's goal and Rules and found:\n- {}\nPropose the corrected phases with propose_phases (without `revises`: it is a new plan, reviewed again).",
                    gaps.join("\n- ")
                ),
            },
            Some(planning.request_id),
        )
        .await;
    }
}

fn vendor_name(vendor: ProviderKind) -> &'static str {
    match vendor {
        ProviderKind::Claude => "Claude",
        ProviderKind::Codex => "Codex",
    }
}

fn short(commit: &str) -> &str {
    &commit[..commit.len().min(10)]
}

/// The phase as every checker reads it first.
fn phase_text(run: &OvernightRun, phase: &OvernightPhase, candidate: &str) -> String {
    let start = phase.start_commit.as_deref().unwrap_or("?");
    let mut text = format!(
        "Phase {} (\u{201c}{}\u{201d}) of the overnight run \u{201c}{}\u{201d}, which the user left to Brigadier. Your checkout is at the phase's candidate commit {} on the run branch; the phase started at {}, so `git diff {} {}` is the whole phase.\n\nThe run's goal, in the user's words:\n{}\n\nThe phase's scope, exactly as its plan says:\n{}\n\nDone when, by id:\n",
        phase.number,
        phase.name,
        run.name,
        short(candidate),
        short(start),
        start,
        candidate,
        run.goal,
        phase.scope
    );
    for criterion in &phase.done_when {
        text.push_str(&format!("- {}: {}\n", criterion.id, criterion.text));
    }
    text.push_str(&format!(
        "\nRules and settled decisions, verbatim:\n{}\n",
        if run.rules.trim().is_empty() {
            "(none given)"
        } else {
            run.rules.as_str()
        }
    ));
    // Answers the user gave on starting or continuing the run are theirs to give: a checker
    // that only saw the Rules took a supplied account ID for an invented one.
    text.push_str(&format!(
        "\nThe user's words for this run, verbatim (a value they give here is theirs, not invented):\n{}\n",
        run.words
    ));
    text
}

/// What the fresh verifier of a whole phase reads.
fn verify_spec(
    run: &OvernightRun,
    phase: &OvernightPhase,
    candidate: &str,
    retry: Option<&str>,
    sandboxed: bool,
) -> String {
    let retry = retry.map_or_else(String::new, |why| {
        format!(
            "\n\nAn earlier verifier of this same candidate could not conclude:\n{why}\nCheck again, and give each [pre-existing] gap as \"[pre-existing] check: evidence from the start commit\"."
        )
    });
    format!(
        "Verify a whole phase independently: you are its fresh verifier, and none of its own workers.\n\n{}
1. For each criterion above, by its id, produce your own evidence on this checkout: run the command and quote the decisive line, or read the code and say where. Workers' and the lead's claims are not evidence.
2. Run the project's checks the way the project runs them (README, package scripts, Makefile, CI config): typecheck, lint, build and the existing tests. {setup}
3. Check the phase as a whole: its commits must work together, not just one by one.
4. Change no tracked file and add no source file: build output goes only into ignored folders. A verification that changed the checkout is discarded.
5. {not_run} A check that fails or can't run the same way on the phase's start commit (unpack it: `mkdir <scratch>/start && git archive {} | tar -x -C <scratch>/start`) is a gap the project already had: name it under risks as \"[pre-existing] check: evidence from the start commit\". It never excuses an unmet criterion.
End with submit_report. done_when: exactly one line per criterion, starting with its status and id: \"[met] p1-c1: your evidence\", \"[not met] p1-c2: what fails\", or \"[not checked] p1-c3: the command you tried and its error\". checks: passed, failed, notRun or noChecks, as for any verification ([pre-existing] and [excluded] checks don't make it notRun). open_questions: each problem a worker must fix, and nothing else. needs_user: exactly what only the user can do (name the config key, environment variable, account or action) before a criterion can be met, or nothing. A problem you notice that no criterion or check of this phase covers goes under risks as a plain line, without a marker.{retry}",
        phase_text(run, phase, candidate),
        phase.start_commit.as_deref().unwrap_or("HEAD~1"),
        setup = super::super::gates::checks_setup(sandboxed),
        not_run = super::super::gates::not_run_step(sandboxed),
    )
}

/// What a reviewer of a whole phase reads.
fn review_spec(
    run: &OvernightRun,
    phase: &OvernightPhase,
    candidate: &str,
    covers: ProviderKind,
    several: bool,
) -> String {
    let covers = if several {
        format!(
            "\nThe phase has work by more than one vendor: yours to check independently is the work by {} models (look at every commit, but your verdict answers for theirs).",
            vendor_name(covers)
        )
    } else {
        String::new()
    };
    format!(
        "Review a whole phase: read every change from the phase's start to its candidate and decide whether the phase may count as done.\n\n{}{covers}
Check that it does what the scope and criteria ask, correctly and safely, that its commits work together (integration, not just each commit alone), that nothing outside the scope or against the Rules slipped in, and that there are no stray files, debug code, secrets or unverified claims. Judge the code, tests and files, not the reports.
End with submit_report and a verdict: approve, or requestChanges with each exact issue (file, behaviour, the criterion id it breaks) in open_questions.",
        phase_text(run, phase, candidate)
    )
}

/// What the fresh judge of a whole phase reads: the phase, then every result.
fn judge_spec(
    run: &OvernightRun,
    phase: &OvernightPhase,
    candidate: &str,
    members: &[(&GateMember, Option<&Task>)],
    gate: &Gate,
    board: &crate::board::Board,
) -> String {
    let mut text = format!(
        "Judge a whole phase in a fresh context: decide from the evidence whether it is done as its plan says. You change nothing.\n\n{}",
        phase_text(run, phase, candidate)
    );
    if let Some(summary) = &phase.summary {
        text.push_str(&format!(
            "\nThe lead's own summary (a claim, not evidence):\n{summary}\n"
        ));
    }
    if !phase.responses.is_empty() {
        text.push_str(&format!(
            "\nThe lead's answers to the previous round's findings:\n- {}\n",
            phase.responses.join("\n- ")
        ));
    }
    for (member, task) in members {
        let Some(task) = task else {
            continue;
        };
        let who = format!(
            "task-{}, {:?} {}",
            task.number,
            task.route.choice.provider,
            task.route.choice.model.as_deref().unwrap_or("")
        );
        match (&member.result, &task.report) {
            (Some(GateResult::NoResult { reason }), _) => {
                text.push_str(&format!(
                    "\n[{:?} {who}: no result] {reason}\n",
                    member.role
                ));
            }
            (_, Some(report)) => text.push_str(&format!(
                "\n[{} {who}]\n{}\n[/{}]\n",
                match member.role {
                    GateRole::Verify => "fresh verifier",
                    GateRole::Review => "reviewer",
                    GateRole::Judge => "judge",
                },
                report_text(report),
                match member.role {
                    GateRole::Verify => "fresh verifier",
                    GateRole::Review => "reviewer",
                    GateRole::Judge => "judge",
                },
            )),
            _ => {}
        }
    }
    let authors: Vec<&str> = gate
        .members
        .iter()
        .filter(|member| member.role == GateRole::Review)
        .filter_map(|member| member.avoid.first())
        .map(|choice| vendor_name(choice.provider))
        .collect();
    if !authors.is_empty() {
        text.push_str(&format!(
            "\nThe phase's work (its lead and workers) is by {} models; each reviewer above was picked from another vendor than the work it covers. Brigadier checks that independence in code: it is not yours to judge, and never a gap.\n",
            authors.join(" and ")
        ));
    }
    if !gate.findings.is_empty() {
        text.push_str("\nThe reviewers' findings, by id:\n");
        for finding in &gate.findings {
            text.push_str(&format!("- {}: {}\n", finding.id, finding.text));
        }
    }
    let waiting: Vec<String> = board
        .waiting
        .values()
        .map(|item| format!("- {}", item.what))
        .collect();
    if !waiting.is_empty() {
        text.push_str(&format!(
            "\nWaiting on the user (only they can do these):\n{}\n",
            waiting.join("\n")
        ));
    }
    let later: Vec<String> = run
        .phases
        .iter()
        .filter(|later| later.number > phase.number && later.depends_on.contains(&phase.number))
        .map(|later| format!("phase {} (\u{201c}{}\u{201d})", later.number, later.name))
        .collect();
    if !later.is_empty() {
        text.push_str(&format!(
            "\nLater phases that build on this one: {}. Say under risks what of this phase they would miss.\n",
            later.join(", ")
        ));
    }
    text.push_str(
        "\nDecide, and end with submit_report:
- done_when: exactly one line per criterion id above, every id once: \"[met] p1-c1: the evidence that shows it (the verifier's command and result, or your own check)\", \"[not met] p1-c2: what is missing\", \"[blocked] p1-c3: exactly what only the user can give\", or \"[not checked] p1-c4: why it couldn't be checked\". A claim without evidence is never [met].
- verdict: approve only when every criterion is [met] with real evidence, the review from another vendor approved the whole phase (or each of its findings is fixed, or declined for a sound reason), and nothing beyond the scope or against the Rules was done. Otherwise requestChanges.
- open_questions: each specific gap a worker can fix (file, behaviour, criterion id), and nothing else.
- needs_user: each thing only the user can do, exactly (the config key or environment variable, the account, the action).
- risks: anything else the morning report must say. A failure already on the phase's start commit never excuses an unmet criterion.",
    );
    text
}

/// A report in full, as the judge reads it.
fn report_text(report: &Report) -> String {
    let mut text = format!("Summary: {}", report.summary);
    let mut list = |name: &str, lines: &[String]| {
        if !lines.is_empty() {
            text.push_str(&format!("\n{name}:\n- {}", lines.join("\n- ")));
        }
    };
    list("Done when", &report.done_when);
    list("Verification", &report.verification);
    list("Open questions", &report.open_questions);
    list("Needs the user", &report.needs_user);
    list("Risks", &report.risks);
    if let Some(verdict) = report.verdict {
        text.push_str(&format!("\nVerdict: {verdict:?}"));
    }
    if let Some(checks) = report.checks {
        text.push_str(&format!("\nChecks: {checks:?}"));
    }
    text
}

/// What Phase 0's judge reads.
fn planning_judge_spec(run: &OvernightRun, proposed: &[crate::overnight::ProposedPhase]) -> String {
    let mut text = format!(
        "Judge an overnight run's plan in a fresh context. The user gave a goal without a plan and left; Phase 0 wrote these phases and another vendor's review approved them. Decide whether they follow the goal: every part of the goal is covered, nothing beyond it or against the Rules is invented, each \"done when\" criterion can really be checked, and the dependencies make sense. You change nothing; you may read the repository.\n\nThe goal, in the user's words:\n{}\n\nRules and settled decisions, verbatim:\n{}\n\nThe proposed phases:\n",
        run.words,
        if run.rules.trim().is_empty() {
            "(none given)"
        } else {
            run.rules.as_str()
        }
    );
    for (index, phase) in proposed.iter().enumerate() {
        text.push_str(&format!(
            "\nPhase {} · {}\nScope: {}\nDone when:\n- {}\n{}",
            index + 1,
            phase.name,
            phase.scope,
            phase.done_when.join("\n- "),
            if phase.depends_on.is_empty() {
                String::new()
            } else {
                format!(
                    "Builds on: {}\n",
                    phase
                        .depends_on
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        ));
    }
    text.push_str("\nEnd with submit_report: verdict approve when the plan follows the goal, else requestChanges with each specific problem (an invented part, a missing part, an uncheckable criterion) in open_questions.");
    text
}

/// A "done when" line's status, its criterion id and whether evidence follows.
fn phase_line(line: &str, ids: &[&str]) -> Option<(String, CriterionStatus, bool)> {
    let bare = without_marker(line);
    let lower = bare.to_lowercase();
    let status = if lower.starts_with("[met]") {
        CriterionStatus::Met
    } else if lower.starts_with("[not met]") {
        CriterionStatus::NotMet
    } else if lower.starts_with("[blocked]") {
        CriterionStatus::Blocked
    } else if lower.starts_with("[not checked]") || lower.starts_with("[not run]") {
        CriterionStatus::NotRun
    } else {
        return None;
    };
    let rest = &lower[lower.find(']').map_or(0, |at| at + 1)..];
    // The criterion is the id named first ("[met] p1-c1: …", "[met] Criterion p1-c2 …").
    let id = ids
        .iter()
        .filter_map(|id| {
            let id_lower = id.to_lowercase();
            let mut from = 0;
            while let Some(at) = rest[from..].find(&id_lower) {
                let start = from + at;
                let end = start + id_lower.len();
                let before = rest[..start].chars().next_back();
                let after = rest[end..].chars().next();
                if !before.is_some_and(|c| c.is_ascii_alphanumeric())
                    && !after.is_some_and(|c| c.is_ascii_alphanumeric())
                {
                    return Some((start, *id));
                }
                from = end;
            }
            None
        })
        .min_by_key(|(at, _)| *at)
        .map(|(_, id)| id)?;
    Some(((*id).to_owned(), status, criterion_evidence(line).is_some()))
}

/// Each criterion's lines in a report: its statuses, each with whether evidence came with it.
fn criteria_in(
    report: &Report,
    ids: &[&str],
) -> HashMap<String, Vec<(CriterionStatus, bool, String)>> {
    let mut found: HashMap<String, Vec<(CriterionStatus, bool, String)>> = HashMap::new();
    for line in &report.done_when {
        if let Some((id, status, evidence)) = phase_line(line, ids) {
            found
                .entry(id)
                .or_default()
                .push((status, evidence, line.clone()));
        }
    }
    found
}

/// What a decided round comes to. Code accepts the phase only on the whole evidence.
fn verdict_of(
    phase: &OvernightPhase,
    gate: &Gate,
    tasks: &HashMap<crate::model::TaskId, &Task>,
    tip: Option<&str>,
    winding_down: bool,
) -> Verdict {
    let ids: Vec<&str> = phase.done_when.iter().map(|c| c.id.as_str()).collect();
    let candidate = gate.commit.clone();
    // A candidate that moved under its checks: what they said is about another tree.
    let stale =
        matches!((tip, candidate.as_deref()), (Some(tip), Some(candidate)) if tip != candidate);
    if stale && !winding_down && gate.round < MAX_ROUNDS {
        return Verdict::Recheck;
    }
    let report_of = |role: GateRole| -> Vec<(&GateMember, Option<&Report>)> {
        gate.members
            .iter()
            .filter(|m| m.role == role)
            .map(|m| (m, tasks.get(&m.task_id).and_then(|t| t.report.as_ref())))
            .collect()
    };
    let verifiers = report_of(GateRole::Verify);
    let reviewers = report_of(GateRole::Review);
    let judges = report_of(GateRole::Judge);
    let verifier = verifiers.first().and_then(|(_, report)| *report);
    let judge = judges.first().and_then(|(_, report)| *report);
    let verifier_passed = verifiers
        .first()
        .is_some_and(|(m, _)| m.result == Some(GateResult::Passed));
    let reviews_passed = !reviewers.is_empty()
        && reviewers
            .iter()
            .all(|(m, _)| m.result == Some(GateResult::Passed));
    let judge_approved = judge.is_some_and(|r| r.verdict == Some(ReviewVerdict::Approve));
    let verifier_lines = verifier.map(|r| criteria_in(r, &ids)).unwrap_or_default();
    let judge_lines = judge.map(|r| criteria_in(r, &ids)).unwrap_or_default();
    // What the verifier and the judge say only the user can do.
    let user_asks: Vec<&String> = [verifier, judge]
        .into_iter()
        .flatten()
        .flat_map(|report| report.needs_user.iter())
        .filter(|line| is_ask(line))
        .collect();
    let shown_met = |lines: &HashMap<String, Vec<(CriterionStatus, bool, String)>>, id: &str| {
        lines.get(id).is_some_and(|found| {
            found.len() == 1 && found[0].0 == CriterionStatus::Met && found[0].1
        })
    };
    let criteria: Vec<CriterionResult> = phase
        .done_when
        .iter()
        .map(|criterion| {
            let pick = |lines: &HashMap<String, Vec<(CriterionStatus, bool, String)>>| {
                lines.get(&criterion.id).and_then(|found| found.first()).cloned()
            };
            let (status, evidence, by) = match (pick(&judge_lines), pick(&verifier_lines)) {
                (Some((status, _, line)), _) => (status, line, judges.first().map(|(m, _)| m.task_id.clone())),
                (None, Some((status, _, line))) => {
                    (status, line, verifiers.first().map(|(m, _)| m.task_id.clone()))
                }
                (None, None) => (CriterionStatus::NotRun, "No checker reported on it.".to_owned(), None),
            };
            // Met only when both the verifier's evidence and the judge say so, once each.
            let (status, evidence) = if status == CriterionStatus::Met
                && !(shown_met(&judge_lines, &criterion.id)
                    && shown_met(&verifier_lines, &criterion.id))
            {
                let said = |lines: &HashMap<String, Vec<(CriterionStatus, bool, String)>>| {
                    lines.get(&criterion.id).map_or_else(
                        || "nothing".to_owned(),
                        |found| {
                            found
                                .iter()
                                .map(|(_, _, line)| line.clone())
                                .collect::<Vec<_>>()
                                .join(" / ")
                        },
                    )
                };
                (
                    CriterionStatus::NotRun,
                    format!(
                        "not shown met once with evidence by both the fresh verifier ({}) and the judge ({})",
                        said(&verifier_lines),
                        said(&judge_lines)
                    ),
                )
            } else if status == CriterionStatus::NotMet
                && user_asks.iter().any(|ask| names(ask, &criterion.id))
            {
                // Unmet because it waits on the user, as a checker says: no fix round helps.
                (CriterionStatus::Blocked, evidence)
            } else {
                (status, evidence)
            };
            CriterionResult {
                id: criterion.id.clone(),
                status,
                evidence,
                candidate: candidate.clone(),
                by,
            }
        })
        .collect();
    let all_met = !criteria.is_empty() && criteria.iter().all(|c| c.status == CriterionStatus::Met);
    if verifier_passed && reviews_passed && judge_approved && all_met && !winding_down && !stale {
        return Verdict::Verified(criteria);
    }
    // What a worker could fix: the checks' own findings, without those about criteria only
    // the user can complete.
    let blocked: Vec<&str> = criteria
        .iter()
        .filter(|c| c.status == CriterionStatus::Blocked)
        .map(|c| c.id.as_str())
        .collect();
    let fixable_line = |line: &str| {
        let named: Vec<&&str> = ids.iter().filter(|id| names(line, id)).collect();
        is_ask(line) && (named.is_empty() || !named.iter().all(|id| blocked.contains(id)))
    };
    let mut findings: Vec<String> = gate
        .findings
        .iter()
        .filter(|finding| fixable_line(&finding.text))
        .map(|finding| format!("{}: {}", finding.id, finding.text))
        .collect();
    if let Some(GateResult::Failed { findings: failed }) =
        verifiers.first().and_then(|(m, _)| m.result.clone())
    {
        findings.extend(
            failed
                .into_iter()
                .filter(|line| fixable_line(line))
                .map(|line| format!("verifier: {line}")),
        );
    }
    if let Some(judge) = judge {
        findings.extend(
            judge
                .open_questions
                .iter()
                .filter(|line| fixable_line(line))
                .map(|line| format!("judge: {line}")),
        );
    }
    let independent = reviewers.iter().all(|(m, _)| {
        !matches!(&m.result, Some(GateResult::NoResult { reason }) if reason.contains("another vendor"))
    });
    // What is left only the user can give: no fix round can change that.
    let only_user = criteria
        .iter()
        .filter(|c| c.status != CriterionStatus::Met)
        .all(|c| c.status == CriterionStatus::Blocked)
        && criteria
            .iter()
            .any(|c| c.status == CriterionStatus::Blocked);
    let fixable = !findings.is_empty() && independent && !winding_down && !only_user;
    if fixable && phase.fix_rounds < FIX_ROUNDS && gate.round < MAX_ROUNDS {
        return Verdict::Fix(
            findings
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    // A verifier that couldn't conclude gets a second one on the same candidate, once.
    if let Some(GateResult::Unverified { reason }) =
        verifiers.first().and_then(|(m, _)| m.result.clone())
        && !gate.retry
        && !winding_down
        && !stale
        && gate.round < MAX_ROUNDS
    {
        return Verdict::Retry(reason);
    }
    let mut asks: Vec<String> = Vec::new();
    for report in [verifier, judge].into_iter().flatten() {
        for line in &report.needs_user {
            if is_ask(line) && !asks.contains(line) {
                asks.push(line.clone());
            }
        }
    }
    let mut gaps: Vec<String> = criteria
        .iter()
        .filter(|c| c.status != CriterionStatus::Met)
        .map(|c| format!("{}: {}", c.id, super::report::evidence_text(&c.evidence)))
        .collect();
    if !independent {
        gaps.push("No reviewer of another vendor than the phase's authors could check it.".into());
    }
    for (member, _) in reviewers
        .iter()
        .chain(verifiers.iter())
        .chain(judges.iter())
    {
        match &member.result {
            Some(GateResult::NoResult { reason }) if !reason.contains("another vendor") => {
                gaps.push(reason.clone());
            }
            Some(GateResult::Unverified { reason }) => gaps.push(reason.clone()),
            _ => {}
        }
    }
    if winding_down {
        gaps.push("The run was ending before the phase's checks could pass.".into());
    }
    if gate.members.is_empty() {
        gaps.push("The phase's whole-phase checks could not start.".into());
    }
    if stale {
        gaps.push(
            "The run branch moved after its last round of checks, so its newest work was never checked."
                .into(),
        );
    }
    let met = criteria.iter().any(|c| c.status == CriterionStatus::Met);
    Verdict::Settle {
        state: if met {
            PhaseState::Partial
        } else {
            PhaseState::Blocked
        },
        criteria,
        gaps,
        asks,
    }
}

/// Whether `line` names criterion `id` ("p2-c2", not "p2-c20").
fn names(line: &str, id: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.match_indices(id).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + id.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric())
    })
}

/// Whether a `needs_user` line asks for something ("None for phase 1" asks nothing).
fn is_ask(line: &str) -> bool {
    let text = line
        .trim()
        .trim_start_matches(['-', '*', ' '])
        .to_lowercase();
    !(text.is_empty()
        || ["none", "n/a", "nothing", "no user action", "not needed"]
            .iter()
            .any(|nothing| text.starts_with(nothing)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_names_a_criterion_only_by_its_whole_id() {
        // The live A/B run's judge: "[not met]", yet its needs_user named the criterion.
        let ask = "Set [account] account_id in ledger.ini, then rerun whoami to establish p2-c2.";
        assert!(names(ask, "p2-c2"));
        assert!(names("P2-C2: blocked on the owner", "p2-c2"));
        assert!(!names(ask, "p2-c1"));
        assert!(!names("see p2-c20", "p2-c2"));
        assert!(!names("xp2-c2", "p2-c2"));
    }
}
