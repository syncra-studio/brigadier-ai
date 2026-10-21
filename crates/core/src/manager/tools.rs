//! The Brigadier MCP tools, answered. Orchestrator tools return at once; their outcomes
//! arrive later as envelopes. Every reply the orchestrator reads is logged as a context
//! injection.

use std::collections::HashSet;

use brigadier_providers::{FileSearch, ProviderKind, SearchKind};

use super::SessionManager;
use super::prompts;
use super::workers::route_label;
use crate::model::{ConversationId, DomainEvent, Environment, MessageRole, PermissionLevel, Setup};
use crate::tools::{NoteKind, OrchestratorCall, ToolReply, WorkerCall};
use crate::work::{
    ApprovalSubject, ArtifactRef, AttachmentRef, DecisionSource, InjectionKind, MockupPart,
    OrchestratorStep, OrchestratorStepKind, QuestionItem, QuestionKind, QuestionOption, Task,
    TaskId, TaskKind, WaitingSource, WorkerRole,
};
use crate::{Error, Result, now_ms};

/// Largest slice `read_artifact` returns.
const ARTIFACT_PAGE_MAX: u32 = 16_000;
/// The most bytes of a `stop_worker` reason the thread shows.
const STOP_REASON_MAX: usize = 300;

impl SessionManager {
    pub(crate) async fn orchestrator_call(
        &self,
        conversation_id: ConversationId,
        call: OrchestratorCall,
    ) -> ToolReply {
        let name = call.name();
        let is_artifact = matches!(call, OrchestratorCall::ReadArtifact(_));
        let image = match &call {
            OrchestratorCall::ReadArtifact(args) => {
                self.read_image_artifact(&conversation_id, &args.id).await
            }
            _ => None,
        };
        let reply = match image {
            Some(reply) => reply,
            None => match self.run_orchestrator_call(&conversation_id, call).await {
                Ok(text) => ToolReply::ok(text),
                Err(err) => ToolReply::error(err.to_string()),
            },
        };
        self.log_injection(
            &conversation_id,
            if is_artifact {
                InjectionKind::Artifact
            } else {
                InjectionKind::ToolResult
            },
            name.into(),
            None,
            reply.text.len(),
        )
        .await;
        reply
    }

    async fn run_orchestrator_call(
        &self,
        id: &ConversationId,
        call: OrchestratorCall,
    ) -> Result<String> {
        match call {
            OrchestratorCall::DelegateTask(args) => {
                // In plan mode a lead may outline the work (it stops at its outline); nothing
                // merges.
                if args.kind.writes() {
                    crate::heap(|| self.check_overnight_proposal(id)).await?;
                }
                if args.kind == TaskKind::Merge {
                    crate::heap(|| self.check_plan_mode(id)).await?;
                }
                let role = match (args.kind, args.role) {
                    (TaskKind::Implement, None) => Some(WorkerRole::Lead),
                    (TaskKind::Merge, None) => Some(WorkerRole::Merge),
                    (
                        TaskKind::Implement,
                        Some(role @ (WorkerRole::Lead | WorkerRole::Parallel | WorkerRole::Fix)),
                    ) => Some(role),
                    (TaskKind::Merge, Some(WorkerRole::Merge)) => Some(WorkerRole::Merge),
                    (_, None) => None,
                    (kind, Some(role)) => {
                        return Err(Error::Invalid(format!(
                            "a {kind:?} task can't be a {role:?}: leads, parallel workers and fixes are implement tasks; start a verifier with start_verifier, and every landing gets its review by itself"
                        )));
                    }
                };
                let subject = match (&args.subject, args.kind) {
                    // An operator works on apps, not on another task's change.
                    (Some(_), TaskKind::Operate) => {
                        return Err(Error::Invalid(
                            "an operate task takes no `subject`: name the app, dev build or \
                             files it works on in its spec"
                                .into(),
                        ));
                    }
                    (Some(reference), kind) => {
                        let subject = crate::heap(|| self.find_task(id, reference)).await?;
                        // A merge is finished by its own worker.
                        if kind == TaskKind::Merge && subject.kind == TaskKind::Merge {
                            return Err(Error::Invalid(super::landing::merge_of_merge(
                                subject.number,
                                subject
                                    .workspace
                                    .as_ref()
                                    .and_then(|w| w.target.as_deref())
                                    .unwrap_or_default(),
                            )));
                        }
                        Some(subject)
                    }
                    (None, TaskKind::Review) => {
                        return Err(Error::Invalid(
                            "a review task needs `subject`: the task whose change it reviews"
                                .into(),
                        ));
                    }
                    // With no task, the worker merges the session's base into the session
                    // branch: only a new worktree has a base, and a run's branch merges only
                    // its verified work.
                    (None, TaskKind::Merge) => {
                        if self.overnight.active.get(id).is_some() {
                            return Err(Error::Invalid(
                                "an overnight run is going: its branch takes no merge of the \
                                 base; a merge task needs `subject`, the task whose work \
                                 conflicts with the run's branch"
                                    .into(),
                            ));
                        }
                        if !matches!(
                            self.core.conversation(id)?.setup,
                            Some(Setup::Session {
                                environment: Environment::NewWorktree { .. },
                                ..
                            })
                        ) {
                            return Err(Error::Invalid(
                                "a merge task needs `subject`, the task whose work conflicts \
                                 with the session's branch: this session works on a local \
                                 checkout, so it has no base to merge in"
                                    .into(),
                            ));
                        }
                        None
                    }
                    (None, _) => None,
                };
                let given = |field: &Option<String>| {
                    field
                        .as_deref()
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                };
                let (target, end_state) = (given(&args.target), given(&args.end_state));
                let built = match given(&args.matches_mockup) {
                    Some(reference) => Some(self.find_task(id, &reference).await?),
                    None => None,
                };
                let (mockup, mockup_images) =
                    mockup_part(args.mockup, built.as_ref(), args.kind, role)?;
                if args.live_look && (args.kind != TaskKind::Implement || mockup.is_some()) {
                    return Err(Error::Invalid(
                        "`live_look` is for an implement task that only looks at the \
                         integrated tree's dev build: no mockup, no other kind"
                            .into(),
                    ));
                }
                if args.kind == TaskKind::Operate {
                    if target.is_none() || end_state.is_none() {
                        return Err(Error::Invalid(
                            "an operate task needs `target` (the app, dev build or URL it works \
                             in, and the window if known) and `end_state` (what must be true at \
                             the end, checkable on screen)"
                                .into(),
                        ));
                    }
                } else if args.target.is_some() || args.end_state.is_some() {
                    return Err(Error::Invalid(format!(
                        "`target` and `end_state` are for operate tasks; a {:?} task takes its \
                         goal from its spec",
                        args.kind
                    )));
                }
                if args.kind == TaskKind::Review
                    && subject
                        .as_ref()
                        .is_some_and(|s| s.candidate.is_none() && s.report.is_none())
                {
                    return Err(Error::Invalid(
                        "the subject task has nothing to review yet".into(),
                    ));
                }
                if let Some(phase) = args.phase {
                    let request = crate::heap(|| self.request_for(id, None)).await;
                    crate::heap(|| self.phase_step(id, request, phase)).await?;
                }
                let effort = match args.effort.trim().to_lowercase().as_str() {
                    effort @ ("low" | "medium" | "high") => effort.to_owned(),
                    other => {
                        return Err(Error::Invalid(format!(
                            "unknown effort \"{other}\": use \"low\", \"medium\" or \"high\""
                        )));
                    }
                };
                // The request it takes whole, bound by its identity: the task belongs to it.
                let (spec, request) = match args.include_request {
                    true => {
                        let request = crate::heap(|| self.request_for(id, None))
                            .await
                            .ok_or_else(|| {
                                Error::Invalid(
                                    "include_request: no request is running; write it into the spec"
                                        .into(),
                                )
                            })?;
                        let verbatim = crate::heap(|| self.request_verbatim(id, &request)).await?;
                        (format!("{}{verbatim}", args.spec), Some(request))
                    }
                    false => (args.spec.clone(), None),
                };
                let areas = task_areas(&args.areas)?;
                let floor = match args.quality.as_deref().map(str::trim) {
                    None | Some("" | "normal") => None,
                    Some("high") => Some(brigadier_router::QualityTier::Frontier),
                    Some(other) => {
                        return Err(Error::Invalid(format!(
                            "unknown quality \"{other}\": use \"high\" or leave it out"
                        )));
                    }
                };
                let needs = if args.image_generation {
                    vec![brigadier_router::Capability::ImageGeneration]
                } else {
                    Vec::new()
                };
                let mut attachments =
                    crate::heap(|| self.find_attachments(id, &args.attachments)).await?;
                for image in mockup_images {
                    // Same bytes under another name are still that screen's `mockup-N` file.
                    if !attachments
                        .iter()
                        .any(|given| given.id == image.id && given.name == image.name)
                    {
                        attachments.push(image);
                    }
                }
                // A research question costly to get wrong goes to both vendors, when both can
                // take it (Q77), with the same files, areas and quality floor.
                if args.fuse
                    && let Some(reply) = crate::heap(|| {
                        self.delegate_fused(
                            id,
                            &args,
                            super::fusion::FusedBrief {
                                spec: spec.clone(),
                                effort: effort.clone(),
                                request: request.clone(),
                                attachments: attachments.clone(),
                                areas: areas.clone(),
                                floor,
                                needs: needs.clone(),
                            },
                        )
                    })
                    .await?
                {
                    return Ok(reply);
                }
                // A fix of landed work resumes the worker that did it, on its model; one fix
                // at a time resumes a session (SESSION-FIXES-PLAN.md Q6.1).
                let held = subject.as_ref().filter(|subject| {
                    role == Some(WorkerRole::Fix)
                        && subject.landed.is_some()
                        && subject.review_hold.is_some()
                });
                // Held until the fix is on the board, so two at once can't both pass the check.
                let _starting = match held {
                    Some(_) => Some(self.fixes_starting.lock().await),
                    None => None,
                };
                if let Some(held) = held
                    && let Some(fix) = crate::heap(|| self.core.board(id))
                        .await?
                        .tasks
                        .values()
                        .find(|task| {
                            !task.state.is_final()
                                && task.role == Some(WorkerRole::Fix)
                                && task.subject.as_ref() == Some(&held.id)
                        })
                {
                    // One fix at a time resumes the session: a second is told where the first
                    // stands, not refused (#251).
                    return Ok(format!(
                        "task-{} fixes task-{}'s findings already ({:?}), so no new task started: send it more with message_worker.",
                        fix.number, held.number, fix.state
                    ));
                }
                let pin = match held {
                    Some(held) => Some(brigadier_router::Pin {
                        provider: Some(held.route.choice.provider),
                        model: held.route.choice.model.clone(),
                        effort: Some(effort),
                        by_user: false,
                    }),
                    None => pin(args.provider.as_deref(), args.model, Some(effort))?,
                };
                let avoid = subject
                    .as_ref()
                    .filter(|_| args.kind == TaskKind::Review)
                    .map(|s| brigadier_router::Author {
                        provider: s.route.choice.provider,
                        model: s.route.choice.model.clone(),
                    });
                let task = crate::heap(|| {
                    self.create_task_as(
                        id,
                        args.title,
                        args.kind,
                        spec,
                        pin,
                        avoid,
                        Vec::new(),
                        None,
                        subject,
                        attachments,
                        areas,
                        floor,
                        needs,
                        super::workers::TaskExtra {
                            role,
                            phase: args.phase,
                            target,
                            end_state,
                            mockup,
                            design: args.design,
                            live_look: args.live_look,
                            request,
                            ..Default::default()
                        },
                    )
                })
                .await?;
                // A lead of a phase takes it over (a new lead redoes it).
                if let Some(phase) = args.phase
                    && role == Some(WorkerRole::Lead)
                {
                    crate::heap(|| self.assign_phase(&task, phase)).await?;
                }
                crate::heap(|| {
                    self.orchestrator_step(
                        id,
                        OrchestratorStepKind::Created {
                            task_id: task.id.clone(),
                        },
                    )
                })
                .await;
                if let Some(wait) = &task.quota_wait {
                    return Ok(format!(
                        "Created task-{} ({:?}), but no model it may use can take it now: {}. It \
                         starts on its own when one can (after a reset, or when the user changes \
                         their routing); its report arrives later as a message. The user sees \
                         it waiting. If you have nothing to say now, reply with exactly {}.",
                        task.number,
                        task.kind,
                        wait.reason,
                        prompts::QUIET
                    ));
                }
                Ok(format!(
                    "Started task-{} ({:?}) on {}: {}. Its report arrives later as a message; don't wait for it. \
                     The user sees the worker live. If you have nothing to say now, reply with \
                     exactly {}.",
                    task.number,
                    task.kind,
                    route_label(&task),
                    task.route.reason,
                    prompts::QUIET
                ))
            }
            OrchestratorCall::MessageWorker(mut args) => {
                let mut task = crate::heap(|| self.find_task(id, &args.task)).await?;
                self.refuse_user_stopped(id, &task).await?;
                if !args.attachments.is_empty() {
                    let given =
                        crate::heap(|| self.find_attachments(id, &args.attachments)).await?;
                    args.text = self.with_files(&task, given, args.text).await?;
                }
                // A later request's turn hands the worker its question: the report that
                // answers it, and the reply after it, belong to the request that asked.
                if !task.state.is_final()
                    && let Some(later) = crate::heap(|| self.later_request_for(id, &task)).await
                {
                    task = crate::heap(|| {
                        self.update_task(id, &task.id, |task| task.request_id = Some(later))
                    })
                    .await?;
                    crate::heap(|| self.settle_requests(id)).await;
                }
                let (reply, answered) =
                    crate::heap(|| self.message_worker(id, &task, args.text.clone(), "the thread"))
                        .await?;
                let text = super::brains::cut(args.text.trim(), crate::work::STEP_TEXT_MAX);
                crate::heap(|| {
                    self.update_task(id, &task.id, |task| messaged(task, args.text, answered))
                })
                .await?;
                crate::heap(|| {
                    self.orchestrator_step(
                        id,
                        OrchestratorStepKind::Messaged {
                            task_id: task.id,
                            text: Some(text),
                        },
                    )
                })
                .await;
                Ok(reply)
            }
            OrchestratorCall::AnswerWorker(args) => {
                let task = crate::heap(|| self.find_task(id, &args.task)).await?;
                self.refuse_user_stopped(id, &task).await?;
                let reply =
                    crate::heap(|| self.answer_worker(&task, args.answer.clone(), args.why))
                        .await?;
                crate::heap(|| {
                    self.update_task(id, &task.id, |task| messaged(task, args.answer, true))
                })
                .await?;
                Ok(reply)
            }
            OrchestratorCall::RouteFollowUp(args) => {
                crate::heap(|| self.route_follow_up(id, &args)).await
            }
            OrchestratorCall::StopWorker(args) => {
                let reason = super::brains::one_line(&args.reason, STOP_REASON_MAX);
                if reason.is_empty() {
                    return Err(Error::Invalid(
                        "stop_worker needs a `reason`: one line saying why the worker stops \
                         (the user reads it)."
                            .into(),
                    ));
                }
                let task = crate::heap(|| self.find_task(id, &args.task)).await?;
                crate::heap(|| self.stop_worker(id, &task, reason)).await?;
                Ok(format!("Stopped task-{}.", task.number))
            }
            OrchestratorCall::RerunTask(args) => {
                crate::heap(|| self.rerun_by_thread(id, args)).await
            }
            OrchestratorCall::AskUser(args) => {
                let items = ask_round(args.questions)?;
                let task_id = match &args.task {
                    Some(reference) => {
                        Some(crate::heap(|| self.find_task(id, reference)).await?.id)
                    }
                    None => None,
                };
                let asked = items.len();
                let title = args
                    .title
                    .as_deref()
                    .map(|title| super::brains::one_line(title, 80))
                    .filter(|title| !title.is_empty());
                crate::heap(|| {
                    self.open_round(id, task_id, QuestionKind::Orchestrator, items, title)
                })
                .await?;
                Ok(format!(
                    "Asked the user {} on one card. Reply with exactly {} now. The answers arrive as an [answer] message in this request; carry on with anything that doesn't depend on them.",
                    if asked == 1 {
                        "1 question".to_owned()
                    } else {
                        format!("{asked} questions")
                    },
                    prompts::QUIET
                ))
            }
            OrchestratorCall::ReadReport(args) => {
                let (task, here) = crate::heap(|| self.find_report(id, &args.task)).await?;
                let report = task.report.as_ref().ok_or_else(|| {
                    Error::Invalid(format!("task-{} has not reported yet", task.number))
                })?;
                let mut text = prompts::report_text(&task, report, &route_label(&task));
                let step = if here {
                    OrchestratorStepKind::ReadReport { task_id: task.id }
                } else {
                    text = format!("[from another session of this project]\n{text}");
                    OrchestratorStepKind::ReadArtifact {
                        name: format!(
                            "the report \u{201c}{}\u{201d} from another session",
                            task.title
                        ),
                    }
                };
                crate::heap(|| self.orchestrator_step(id, step)).await;
                Ok(text)
            }
            OrchestratorCall::ReadArtifact(args) => {
                // A trimmed command output of this conversation's (`out-<id>`), or a report's
                // artifact of this project.
                let output = crate::heap(|| self.output_blob(id, args.id.trim())).await?;
                if output.is_none() {
                    crate::heap(|| self.check_artifact(id, &args.id)).await?;
                }
                let limit = args
                    .limit
                    .unwrap_or(ARTIFACT_PAGE_MAX)
                    .min(ARTIFACT_PAGE_MAX);
                let offset = args.offset.unwrap_or(0);
                let blob = output
                    .as_ref()
                    .map_or_else(|| args.id.clone(), |(blob, _)| blob.clone());
                let (bytes, total) =
                    crate::heap(|| self.core.read_blob_range(blob, offset, limit)).await?;
                let text = match std::str::from_utf8(&bytes) {
                    Ok(text) => text.to_owned(),
                    // A page may end inside a character.
                    Err(err) if err.error_len().is_none() => {
                        String::from_utf8_lossy(&bytes[..err.valid_up_to()]).into_owned()
                    }
                    Err(_) => return Ok(format!("{} is not text ({total} bytes).", args.id)),
                };
                let end = offset + text.len() as u64;
                // Paging on through the same artifact is one read.
                if offset == 0 {
                    let name = match output {
                        Some((_, name)) => name,
                        None => crate::heap(|| self.artifact_name(id, &args.id)).await,
                    };
                    crate::heap(|| {
                        self.orchestrator_step(id, OrchestratorStepKind::ReadArtifact { name })
                    })
                    .await;
                }
                Ok(format!(
                    "[artifact {} bytes {offset}–{end} of {total}]\n{text}{}",
                    args.id,
                    if end < total {
                        format!("\n[more: read_artifact with offset {end}]")
                    } else {
                        String::new()
                    }
                ))
            }
            OrchestratorCall::StoreOutput(args) => {
                crate::heap(|| self.store_output_tool(id, args)).await
            }
            OrchestratorCall::QueryBrain(args) => {
                crate::heap(|| {
                    self.query_brain_tool(
                        id,
                        None,
                        args.query,
                        args.history.unwrap_or(false),
                        args.page,
                    )
                })
                .await
            }
            OrchestratorCall::Remember(args) => crate::heap(|| self.remember_tool(id, args)).await,
            OrchestratorCall::SearchTranscript(args) => {
                crate::heap(|| self.search_transcript_tool(id, args)).await
            }
            OrchestratorCall::PlanPhases(args) => crate::heap(|| self.plan_phases(id, args)).await,
            OrchestratorCall::ProposePlan(args) => {
                crate::heap(|| self.propose_plan(id, args)).await
            }
            OrchestratorCall::ApproveOutline(args) => {
                crate::heap(|| self.approve_outline(id, args)).await
            }
            OrchestratorCall::StartVerifier(args) => {
                // A verifier changes code: in plan mode nothing does.
                crate::heap(|| self.check_plan_mode(id)).await?;
                crate::heap(|| self.verify_task(id, args)).await
            }
            OrchestratorCall::RequestReview(args) => {
                crate::heap(|| self.request_review(id, args)).await
            }
            OrchestratorCall::AttachProof(args) => {
                crate::heap(|| self.attach_proof(id, args)).await
            }
            OrchestratorCall::RequestApproval(args) => {
                crate::heap(|| {
                    self.open_approval(
                        id,
                        None,
                        ApprovalSubject::Action {
                            action: args.action,
                            details: args.details,
                        },
                    )
                })
                .await?;
                Ok("Asked the user. The decision arrives later as a message.".into())
            }
            OrchestratorCall::LandPhase(args) => {
                crate::heap(|| self.check_plan_mode(id)).await?;
                let task = crate::heap(|| self.find_task(id, &args.task)).await?;
                crate::heap(|| self.land_phase(id, task)).await
            }
            OrchestratorCall::FinishSession(args) => {
                crate::heap(|| self.check_plan_mode(id)).await?;
                let consent = args.consent.as_deref().unwrap_or(&args.user_words);
                crate::heap(|| self.finish_session_when(id, consent, args.message, args.when)).await
            }
            OrchestratorCall::StopMerge(args) => {
                crate::heap(|| self.stop_merge(id, &args.why)).await
            }
            OrchestratorCall::NoteForUser(args) => {
                crate::heap(|| self.note_for_user(id, args)).await
            }
            OrchestratorCall::RecordFindings(args) => {
                crate::heap(|| self.record_findings(id, args)).await
            }
            OrchestratorCall::CiStatus(args) => crate::heap(|| self.ci_status(id, args)).await,
            OrchestratorCall::WatchCi(args) => crate::heap(|| self.watch_ci(id, args)).await,
            OrchestratorCall::SuggestFullAccess(args) => {
                crate::heap(|| self.suggest_full_access(id, &args.reason)).await
            }
            OrchestratorCall::SuggestSetting(args) => {
                crate::heap(|| self.suggest_setting(id, args)).await
            }
            OrchestratorCall::OfferFeedback(args) => {
                crate::heap(|| self.offer_feedback(id, args)).await
            }
            OrchestratorCall::OfferSession(args) => {
                crate::heap(|| self.offer_session(id, args)).await
            }
            OrchestratorCall::SetHeadline(args) => self.set_headline(id, args).await,
            OrchestratorCall::SettleStep(args) => crate::heap(|| self.settle_step(id, args)).await,
            OrchestratorCall::EndRun(args) => crate::heap(|| self.end_run_now(id, args)).await,
            OrchestratorCall::ProposeOvernight(args) => {
                crate::heap(|| self.interpret_overnight(id, args)).await
            }
            // What the thread finds in the index counts as its own search (`super::reads`).
            OrchestratorCall::CodeSearch(args) => {
                let search = FileSearch {
                    kind: match args.kind.as_deref() {
                        Some("file") => SearchKind::Files,
                        _ => SearchKind::Content,
                    },
                    pattern: Some(args.query.clone()),
                    scope: args.path.clone(),
                    glob: args
                        .language
                        .as_ref()
                        .map(|language| format!("language:{language}")),
                    hits: Vec::new(),
                };
                let (text, hits) = crate::heap(|| self.code_search_tool(id, args)).await?;
                crate::heap(|| self.looked_in_index(id, FileSearch { hits, ..search })).await;
                Ok(text)
            }
            OrchestratorCall::CodeRefs(args) => {
                let symbol = args.symbol.clone();
                let (text, hits) = crate::heap(|| self.code_refs_tool(id, args)).await?;
                let search = FileSearch {
                    kind: SearchKind::Content,
                    pattern: Some(symbol),
                    scope: None,
                    glob: None,
                    hits,
                };
                crate::heap(|| self.looked_in_index(id, search)).await;
                Ok(text)
            }
            OrchestratorCall::ProjectMap => crate::heap(|| self.project_map_tool(id)).await,
            OrchestratorCall::CleanUp => crate::heap(|| self.clean_up(id)).await,
            OrchestratorCall::RunChecks(args) => {
                crate::heap(|| self.run_checks_tool(id, args.rerun)).await
            }
            OrchestratorCall::ReviewPlan(args) => {
                crate::heap(|| self.review_thread_plan(id, args)).await
            }
            OrchestratorCall::ReadPage(args) => crate::heap(|| self.read_page(id, args)).await,
            OrchestratorCall::WritePage(args) => {
                crate::heap(|| self.check_plan_mode(id)).await?;
                crate::heap(|| self.write_page(id, args)).await
            }
            OrchestratorCall::SaveReport(args) => crate::heap(|| self.save_report(args)).await,
            OrchestratorCall::Run(args) => {
                crate::heap(|| {
                    self.run_tool(
                        id,
                        &args.command,
                        args.workdir.as_deref(),
                        args.timeout_secs,
                        args.name.as_deref(),
                        false,
                    )
                })
                .await
            }
            // Codex let it through: under Approve for me its auto-reviewer approved it, under
            // Ask for approval the user did.
            OrchestratorCall::RunUnsandboxed(args) => {
                crate::heap(|| {
                    self.run_tool(
                        id,
                        &args.command,
                        args.workdir.as_deref(),
                        args.timeout_secs,
                        args.name.as_deref(),
                        true,
                    )
                })
                .await
            }
            OrchestratorCall::RunCheck(args) => {
                crate::heap(|| self.thread_run_check(id, args)).await
            }
            OrchestratorCall::StartPreview(args) => {
                crate::heap(|| self.start_preview(id, args)).await
            }
            OrchestratorCall::StopPreview(args) => {
                crate::heap(|| self.stop_preview_tool(id, args.id.as_deref(), None)).await
            }
            OrchestratorCall::PreviewLog(args) => {
                crate::heap(|| {
                    self.preview_log(id, args.id.as_deref(), args.tail_lines, args.wait_secs)
                })
                .await
            }
            OrchestratorCall::ListSessions(args) => {
                crate::heap(|| self.list_sessions(id, args.query.as_deref(), args.project)).await
            }
            OrchestratorCall::ReadSession(args) => {
                crate::heap(|| self.read_session(&args.id)).await
            }
            OrchestratorCall::MessageSession(args) => {
                crate::heap(|| self.message_session(id, args)).await
            }
            OrchestratorCall::WaitForReplies(args) => {
                crate::heap(|| self.wait_for_replies(id, args)).await
            }
            OrchestratorCall::SettleWait(args) => {
                crate::heap(|| self.settle_wait(id, &args.message_id)).await
            }
            OrchestratorCall::ListTasks => {
                let tasks = crate::heap(|| self.core.tasks(id)).await?;
                let standing = match self.core.board(id).await {
                    Ok(board) => {
                        super::integrated_checks::standing_lines(&board, crate::now_ms()).join("\n")
                    }
                    Err(_) => String::new(),
                };
                if tasks.is_empty() {
                    return Ok(format!("No tasks yet.\n{standing}").trim_end().to_owned());
                }
                let board = self.core.board(id).await.ok();
                let listed = tasks
                    .iter()
                    .map(|task| {
                        let by_user = board
                            .as_ref()
                            .is_some_and(|board| self.user_stopped(board, &task.id));
                        format!(
                            "task-{} {:?} {:?}{} on {}: {}{}",
                            task.number,
                            task.kind,
                            task.state,
                            if by_user {
                                " (stopped by the user)"
                            } else {
                                ""
                            },
                            route_label(task),
                            task.title,
                            task.blocked_reason
                                .as_ref()
                                .map(|r| format!(" ({r})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(format!("{listed}\n{standing}").trim_end().to_owned())
            }
        }
    }

    /// Refuses a call that would message, steer or approve a task the user stopped.
    pub(crate) async fn refuse_user_stopped(&self, id: &ConversationId, task: &Task) -> Result<()> {
        let stopped = match self.core.board(id).await {
            Ok(board) => self.user_stopped(&board, &task.id),
            Err(_) => false,
        };
        if stopped {
            return Err(Error::Invalid(super::workers::user_stopped_refusal(task)));
        }
        Ok(())
    }

    /// Files a row for the thread under the request the orchestrator serves.
    pub(crate) async fn orchestrator_step(&self, id: &ConversationId, kind: OrchestratorStepKind) {
        let request = self.request_for(id, None).await;
        self.orchestrator_step_in(id, request, kind).await;
    }

    /// Says a moment of the conversation (a usage limit, a switch of model, provider or
    /// account) as a divider in the thread, under the request it happened in (`task`'s own).
    pub(crate) async fn moment(&self, id: &ConversationId, task: Option<&TaskId>, text: String) {
        let request = self.request_for(id, task).await;
        self.orchestrator_step_in(id, request, OrchestratorStepKind::Moment { text })
            .await;
    }

    /// Files a row for the thread under `request`.
    pub(crate) async fn orchestrator_step_in(
        &self,
        id: &ConversationId,
        request: Option<String>,
        kind: OrchestratorStepKind,
    ) {
        // A step that ends a held merge is stored under the lock a tip update holds from its
        // read to its write, so a landing told meanwhile can't hold the ended merge again.
        let ends = matches!(
            kind,
            OrchestratorStepKind::Merged { .. }
                | OrchestratorStepKind::MergeCancelled { .. }
                | OrchestratorStepKind::MergeStopped { .. }
        );
        let conv = self.conv(id).ok().filter(|_| ends);
        let _moves = match &conv {
            Some(conv) => Some(conv.held_moves.lock().await),
            None => None,
        };
        let step = OrchestratorStep {
            request_id: request,
            kind,
            at_ms: now_ms(),
            position: 0,
        };
        if let Err(err) = self
            .core
            .record_conversation(id, vec![DomainEvent::OrchestratorStepped { step }])
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not store an orchestrator step");
        }
    }

    /// The task `reference` names for `read_report`: one of this session's (by number or id),
    /// else, by its id, one of another session of the same project (as a Brain answer names
    /// it). Whether it is this session's comes with it.
    async fn find_report(&self, id: &ConversationId, reference: &str) -> Result<(Task, bool)> {
        match self.find_task(id, reference).await {
            Ok(task) => return Ok((task, true)),
            Err(Error::NotFound(_)) => {}
            Err(err) => return Err(err),
        }
        let wanted = TaskId(reference.trim().to_owned());
        for other in self.project_conversations(id) {
            if let Ok(board) = self.core.board(&other).await
                && let Some(task) = board.tasks.get(&wanted)
            {
                return Ok((task.clone(), false));
            }
        }
        Err(Error::Invalid(format!(
            "{reference} is not a task of this session or of another session of this project. Task numbers (task-3) are this session's; a report from another session is read by the task id its Brain answer names (\"from report <id>\")."
        )))
    }

    /// `read_artifact` reads what a report of this project stored (its artifacts and
    /// outputs, a kept patch), from this session or another of the project; nothing else in
    /// the store.
    async fn check_artifact(&self, id: &ConversationId, artifact: &str) -> Result<ArtifactRef> {
        let named = |task: &Task| {
            task.report
                .iter()
                .flat_map(|report| &report.artifacts)
                .chain(&task.outputs)
                .chain(task.kept.iter().filter_map(|kept| match kept {
                    crate::work::KeptWork::Diff { artifact, .. } => Some(artifact),
                    _ => None,
                }))
                .find(|known| known.id == artifact)
                .cloned()
        };
        for conversation in std::iter::once(id.clone()).chain(self.project_conversations(id)) {
            if let Ok(board) = self.core.board(&conversation).await
                && let Some(known) = board.tasks.values().find_map(named)
            {
                return Ok(known);
            }
        }
        Err(Error::Invalid(format!(
            "{artifact} is not an artifact of a report in this project. Use an id a report, read_report or query_brain gave you."
        )))
    }

    /// `read_artifact` on a report's image (an operate worker's last screenshot): the image
    /// itself, which the model reads; `None` for anything else.
    async fn read_image_artifact(&self, id: &ConversationId, artifact: &str) -> Option<ToolReply> {
        let known = self.check_artifact(id, artifact.trim()).await.ok()?;
        if !known.mime.starts_with("image/") {
            return None;
        }
        let reply = match self
            .core
            .read_blob_range(known.id.clone(), 0, u32::MAX)
            .await
        {
            Ok((bytes, total)) => ToolReply::ok(format!(
                "[artifact {} · {} · {total} bytes]",
                known.id, known.title
            ))
            .with_image(known.mime.clone(), bytes),
            Err(err) => ToolReply::error(err.to_string()),
        };
        self.orchestrator_step(id, OrchestratorStepKind::ReadArtifact { name: known.title })
            .await;
        Some(reply)
    }

    /// The project's other conversations (sessions of the same project), newest first.
    fn project_conversations(&self, id: &ConversationId) -> Vec<ConversationId> {
        let Some(project) = self.core.conversation(id).ok().and_then(|c| c.project_id) else {
            return Vec::new();
        };
        let mut others: Vec<_> = self
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter(|c| c.id != *id && c.project_id.as_ref() == Some(&project))
            .collect();
        others.sort_by_key(|c| std::cmp::Reverse(c.updated_at_ms));
        others.into_iter().map(|c| c.id).collect()
    }

    /// An artifact's title from the report that lists it, else "an artifact".
    async fn artifact_name(&self, id: &ConversationId, artifact: &str) -> String {
        let Ok(board) = self.core.board(id).await else {
            return "an artifact".into();
        };
        board
            .tasks
            .values()
            .filter_map(|task| task.report.as_ref())
            .flat_map(|report| &report.artifacts)
            .chain(board.tasks.values().flat_map(|task| &task.outputs))
            .find(|known| known.id == artifact)
            .map_or_else(|| "an artifact".into(), |known| known.title.clone())
    }

    pub(crate) async fn worker_call(
        &self,
        conversation_id: ConversationId,
        task_id: TaskId,
        call: WorkerCall,
    ) -> ToolReply {
        let result = match call {
            WorkerCall::AskOrchestrator(args) => {
                self.worker_question(&conversation_id, &task_id, args).await
            }
            WorkerCall::SubmitOutline(args) => {
                self.submit_outline(&conversation_id, &task_id, args.outline)
                    .await
            }
            WorkerCall::RequestReview => {
                self.worker_request_review(&conversation_id, &task_id).await
            }
            // The app shows the note from the call itself, in the worker's live events.
            WorkerCall::NoteProgress(args) => {
                if args.note.trim().is_empty() {
                    Err(Error::Invalid(
                        "Give the note: a few words on where you are.".into(),
                    ))
                } else {
                    Ok("Noted: it shows under your name until your next note.".into())
                }
            }
            WorkerCall::QueryBrain(args) => {
                self.note_brain_query(&conversation_id, &task_id).await;
                // A claim's source is re-read where the worker works.
                let worktree = self
                    .task_by_id(&conversation_id, &task_id)
                    .await
                    .ok()
                    .and_then(|task| task.workspace)
                    .and_then(|workspace| workspace.worktree)
                    .map(std::path::PathBuf::from);
                self.query_brain_tool(
                    &conversation_id,
                    worktree.as_deref(),
                    args.query,
                    args.history.unwrap_or(false),
                    args.page,
                )
                .await
            }
            WorkerCall::SubmitReport(args) => {
                self.worker_report(&conversation_id, &task_id, args).await
            }
            WorkerCall::CodeSearch(args) => self
                .code_search_tool(&conversation_id, args)
                .await
                .map(|(text, _)| text),
            WorkerCall::CodeRefs(args) => self
                .code_refs_tool(&conversation_id, args)
                .await
                .map(|(text, _)| text),
            WorkerCall::ProjectMap => self.project_map_tool(&conversation_id).await,
            WorkerCall::RunCheck(args) => {
                self.run_check_tool(&conversation_id, Some(&task_id), args)
                    .await
            }
            WorkerCall::RunChecks(args) => {
                crate::heap(|| self.worker_run_checks(&conversation_id, &task_id, args.rerun)).await
            }
            WorkerCall::StartPreview(args) => {
                crate::heap(|| self.start_worker_preview(&conversation_id, &task_id, args)).await
            }
            WorkerCall::StopPreview(args) => {
                crate::heap(|| {
                    self.stop_preview_tool(&conversation_id, args.id.as_deref(), Some(&task_id))
                })
                .await
            }
            WorkerCall::PreviewLog(args) => {
                crate::heap(|| {
                    self.worker_preview_log(
                        &conversation_id,
                        &task_id,
                        args.id.as_deref(),
                        args.tail_lines,
                        args.wait_secs,
                    )
                })
                .await
            }
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// In plan mode nothing changes until the user turns it off: landing, merging and
    /// finishing are refused, whatever the permission level.
    async fn check_plan_mode(&self, id: &ConversationId) -> Result<()> {
        self.check_overnight_proposal(id).await?;
        if self.plan_mode(id) {
            return Err(Error::Invalid(
                "Plan mode is on: change nothing yet. Scouts and research may look around, and a lead may write its outline (it stops there). Merge tasks, landing and finish_session work again once the user turns plan mode off.".into(),
            ));
        }
        Ok(())
    }

    /// `note_for_user`: a judgement call for "Decided for you", or, in an overnight run,
    /// something only the user can do for its "Waiting on you", under the request the
    /// orchestrator serves (refused outside a run).
    pub(crate) async fn note_for_user(
        &self,
        id: &ConversationId,
        args: crate::tools::NoteForUser,
    ) -> Result<String> {
        let what = args.what.trim();
        if what.is_empty() {
            return Err(Error::Invalid("`what` is empty".into()));
        }
        let request = self.request_for(id, None).await;
        match args.kind {
            NoteKind::Decided => {
                self.record_decision(
                    id,
                    request,
                    DecisionSource::Orchestrator,
                    crate::work::DecisionKind::Routine,
                    what.to_owned(),
                    args.why.unwrap_or_default(),
                )
                .await;
                Ok("Noted under Decided for you.".into())
            }
            NoteKind::Waiting => {
                let added = self
                    .wait_on_user(id, request, WaitingSource::Orchestrator, what)
                    .await?;
                Ok(if added {
                    "Listed under the run's Waiting on you; carry on with anything that doesn't depend on it."
                } else {
                    "It is already listed under the run's Waiting on you."
                }
                .into())
            }
        }
    }

    /// `set_headline`: the session's headline and its one detail line, stored and sent as an
    /// event for the Job card. The user's next message clears it.
    pub(crate) async fn set_headline(
        &self,
        id: &ConversationId,
        args: crate::tools::SetHeadline,
    ) -> Result<String> {
        let text = one_line(&args.headline, HEADLINE_MAX);
        if text.is_empty() {
            return Err(Error::Invalid("`headline` is empty".into()));
        }
        let detail = args
            .detail
            .map(|detail| one_line(&detail, HEADLINE_MAX * 2))
            .filter(|detail| !detail.is_empty());
        self.core
            .record_conversation(
                id,
                vec![DomainEvent::HeadlineSet {
                    conversation_id: id.clone(),
                    headline: Some(crate::work::SessionHeadline {
                        text,
                        detail,
                        at_ms: crate::now_ms(),
                    }),
                }],
            )
            .await?;
        Ok("Headline set.".into())
    }

    /// `suggest_full_access`: a notice in the thread, under the request the orchestrator serves,
    /// with a button that switches the session to Full access. Once per request, and only where
    /// the switch would change something: a session below Full access, in a trusted folder,
    /// with the user there to press it.
    pub(crate) async fn suggest_full_access(
        &self,
        id: &ConversationId,
        reason: &str,
    ) -> Result<String> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(Error::Invalid("`reason` is empty".into()));
        }
        if !matches!(
            self.core.conversation(id)?.setup,
            Some(crate::model::Setup::Session { .. })
        ) {
            return Err(Error::Invalid(
                "only a session has a permission level".into(),
            ));
        }
        if self.repo_trust(id) == Some(false) {
            return Err(Error::Invalid(
                "The user hasn't trusted this project's folder, so this session asks first whatever its level: say in your answer what needs more access; trusting the folder is in the project's settings.".into(),
            ));
        }
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going and nobody can press the button: list it with note_for_user, kind waiting.".into(),
            ));
        }
        if self.permission(id) == PermissionLevel::FullAccess {
            return Err(Error::Invalid(
                "This session already has Full access: the sandbox isn't what stopped it.".into(),
            ));
        }
        let request = self.request_for(id, None).await;
        let conv = self.conv(id)?;
        let _notice = conv.full_access_notice.lock().await;
        let board = self.core.board(id).await?;
        if board.orchestrator_steps.iter().any(|step| {
            step.request_id == request
                && matches!(step.kind, OrchestratorStepKind::FullAccessSuggested { .. })
        }) {
            return Err(Error::Invalid(
                "The notice is already shown for this request: don't show it again.".into(),
            ));
        }
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::FullAccessSuggested {
                reason: reason.to_owned(),
            },
        )
        .await;
        Ok("Showed the user a notice with a button that switches this session to Full access. Say in a line what waits on it, and carry on with what doesn't need it; if they switch, a [settings] note tells you.".into())
    }

    /// `suggest_setting`: a card in the thread with one settings change and a button that makes
    /// it; nothing changes until the user presses it. Full access is `suggest_full_access`'s
    /// notice. Once per change and request, and only where the change would change something.
    pub(crate) async fn suggest_setting(
        &self,
        id: &ConversationId,
        args: crate::tools::SuggestSetting,
    ) -> Result<String> {
        use crate::tools::SuggestedSetting;
        use crate::work::SettingChange;
        let reason = args.reason.trim();
        if reason.is_empty() {
            return Err(Error::Invalid("`reason` is empty".into()));
        }
        let value = args
            .value
            .trim()
            .to_ascii_lowercase()
            .replace([' ', '-'], "_");
        let switch = || match value.as_str() {
            "on" | "true" => Ok(true),
            "off" | "false" => Ok(false),
            _ => Err(Error::Invalid(
                "`value` is \"on\" or \"off\" for this setting".into(),
            )),
        };
        let settings = self.core.settings();
        let (change, already) = match args.setting {
            SuggestedSetting::Permission => {
                let level = match value.as_str() {
                    "full_access" => return self.suggest_full_access(id, reason).await,
                    "ask_for_approval" => PermissionLevel::AskForApproval,
                    "approve_for_me" => PermissionLevel::ApproveForMe,
                    _ => {
                        return Err(Error::Invalid(
                            "`value` for permission is \"ask_for_approval\", \"approve_for_me\" or \"full_access\"".into(),
                        ));
                    }
                };
                if !matches!(
                    self.core.conversation(id)?.setup,
                    Some(crate::model::Setup::Session { .. })
                ) {
                    return Err(Error::Invalid(
                        "only a session has a permission level".into(),
                    ));
                }
                (
                    SettingChange::Permission { level },
                    self.permission(id) == level,
                )
            }
            SuggestedSetting::Browser => {
                let on = switch()?;
                (
                    SettingChange::Browser { on },
                    settings.browser_enabled == on,
                )
            }
            SuggestedSetting::MergeOnOwn => {
                let on = switch()?;
                (SettingChange::MergeOnOwn { on }, settings.auto_merge == on)
            }
        };
        if already {
            return Err(Error::Invalid(
                "The setting is that way already: nothing to suggest.".into(),
            ));
        }
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going and nobody can press the button: list it with note_for_user, kind waiting.".into(),
            ));
        }
        let request = self.request_for(id, None).await;
        let conv = self.conv(id)?;
        let _notice = conv.full_access_notice.lock().await;
        let board = self.core.board(id).await?;
        if board.orchestrator_steps.iter().any(|step| {
            step.request_id == request
                && matches!(&step.kind, OrchestratorStepKind::SettingSuggested { change: shown, .. } if *shown == change)
        }) {
            return Err(Error::Invalid(
                "The card is already shown for this request: don't show it again.".into(),
            ));
        }
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::SettingSuggested {
                change,
                reason: reason.to_owned(),
            },
        )
        .await;
        Ok("Showed the user a card with the change and a button that makes it; nothing changes unless they press it. Say in a line what waits on it, and carry on with what doesn't; if they change it, a [settings] note tells you.".into())
    }

    /// `offer_feedback`: a card in the thread with a report to Brigadier's developers, which
    /// the user sends or dismisses (the thread judges they are annoyed, or that they asked;
    /// this only plumbs). One offer per incident: none while the session's last offer is
    /// unanswered or after the user dismissed one unless they asked for this one, none twice in
    /// a request, and none in an overnight run. An offer
    /// from an attempt the user asked to redo, or from a message they edited away, counts for nothing.
    pub(crate) async fn offer_feedback(
        &self,
        id: &ConversationId,
        args: crate::tools::OfferFeedback,
    ) -> Result<String> {
        let (title, body) = (args.title.trim(), args.body.trim());
        if title.is_empty() || body.is_empty() {
            return Err(Error::Invalid(
                "`title` and `body` must both be written".into(),
            ));
        }
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going and nobody is there to send it: don't offer feedback now.".into(),
            ));
        }
        let request = self.request_for(id, None).await;
        let conv = self.conv(id)?;
        let _offer = conv.feedback_offer.lock().await;
        let board = self.core.board(id).await?;
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        let shown: HashSet<String> = messages
            .into_iter()
            .filter(|message| message.role == MessageRole::User)
            .map(|message| message.request_id.unwrap_or(message.id))
            .collect();
        let mut last = None;
        // An offer from a request's earlier attempt (before a Try again) or from a message the
        // user edited away is out of view: it counts for nothing, as the thread shows it.
        let current = |step: &&crate::work::OrchestratorStep| match &step.request_id {
            Some(request) => {
                shown.contains(request)
                    && board
                        .requests
                        .get(request)
                        .is_none_or(|request| step.at_ms >= request.started_at_ms)
            }
            None => true,
        };
        for step in board.orchestrator_steps.iter().filter(current) {
            match &step.kind {
                OrchestratorStepKind::FeedbackOffered { id, .. } => {
                    if step.request_id == request {
                        return Err(Error::Invalid(
                            "Feedback was already offered for this request: don't offer it again."
                                .into(),
                        ));
                    }
                    last = Some((id.clone(), None));
                }
                OrchestratorStepKind::FeedbackAnswered { id, sent } => {
                    if let Some((offered, answer)) = &mut last
                        && offered == id
                    {
                        *answer = Some(*sent);
                    }
                }
                _ => {}
            }
        }
        match last {
            _ if args.asked => {}
            Some((_, None)) => {
                return Err(Error::Invalid(
                    "The user hasn't answered the feedback already offered in this session: don't offer another.".into(),
                ));
            }
            Some((_, Some(false))) => {
                return Err(Error::Invalid(
                    "The user dismissed the feedback offered earlier in this session: don't offer it again.".into(),
                ));
            }
            _ => {}
        }
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::FeedbackOffered {
                id: uuid::Uuid::now_v7().to_string(),
                kind: args.kind,
                title: title.to_owned(),
                body: body.to_owned(),
            },
        )
        .await;
        Ok("The user sees the report as a card under your answer, to send or dismiss. Don't mention it again; carry on with your answer.".into())
    }

    /// `offer_session`: a card in the thread with one "Start as session" button for work
    /// outside the request (the thread judges what is out of scope; this only plumbs). Nothing
    /// starts until the user clicks it ([`Self::start_offered_session`]). The same task once a
    /// request, and none in an overnight run, where nobody is there to click.
    pub(crate) async fn offer_session(
        &self,
        id: &ConversationId,
        args: crate::tools::OfferSession,
    ) -> Result<String> {
        let task = args.task.trim();
        if task.is_empty() {
            return Err(Error::Invalid(
                "`task` must say what the session is to do".into(),
            ));
        }
        if self.overnight.active.get(id).is_some() {
            return Err(Error::Invalid(
                "An overnight run is going and nobody is there to start it: note it in the run's report instead.".into(),
            ));
        }
        let title = args
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map_or_else(|| offer_title(task), str::to_owned);
        let request = self.request_for(id, None).await;
        let board = self.core.board(id).await?;
        if board.orchestrator_steps.iter().any(|step| {
            step.request_id == request
                && matches!(&step.kind, OrchestratorStepKind::SessionOffered { task: offered, .. } if offered == task)
        }) {
            return Err(Error::Invalid(
                "This task was already offered for this request: don't offer it again.".into(),
            ));
        }
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::SessionOffered {
                id: uuid::Uuid::now_v7().to_string(),
                title,
                task: task.to_owned(),
            },
        )
        .await;
        Ok("The user sees it as a card with a Start as session button: one click starts it as a session of its own, on its own branch. Don't start it yourself or do it here; carry on with the request.".into())
    }

    /// The user's answer to the feedback offer `offer_id` in this conversation: sent (opened
    /// on GitHub) or dismissed. Once only.
    pub async fn answer_feedback_offer(
        &self,
        id: &ConversationId,
        offer_id: &str,
        sent: bool,
    ) -> Result<()> {
        let conv = self.conv(id)?;
        let _offer = conv.feedback_offer.lock().await;
        let board = self.core.board(id).await?;
        let offer = board.orchestrator_steps.iter().find(|step| {
            matches!(&step.kind, OrchestratorStepKind::FeedbackOffered { id, .. } if id == offer_id)
        });
        let Some(offer) = offer else {
            return Err(Error::NotFound(format!("feedback offer {offer_id}")));
        };
        if board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, OrchestratorStepKind::FeedbackAnswered { id, .. } if id == offer_id)
        }) {
            return Err(Error::Invalid("this feedback was already answered".into()));
        }
        let request = offer.request_id.clone();
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::FeedbackAnswered {
                id: offer_id.to_owned(),
                sent,
            },
        )
        .await;
        Ok(())
    }

    /// The user's attachments in this conversation, by id.
    /// A message to a worker with the user's attachments written into its scratch folder, like
    /// a new task's, and their paths named after the text: it reads them there.
    async fn with_files(
        &self,
        task: &Task,
        attachments: Vec<AttachmentRef>,
        text: String,
    ) -> Result<String> {
        let scratch = task
            .workspace
            .as_ref()
            .map(|workspace| workspace.scratch.clone())
            .filter(|scratch| !scratch.is_empty())
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "task-{} has no folder to put files in yet; send them once it has started",
                    task.number
                ))
            })?;
        let carrier = Task {
            attachments,
            ..task.clone()
        };
        let files = self
            .worker_files(&carrier, std::path::Path::new(&scratch))
            .await;
        if files.len() < carrier.attachments.len() {
            return Err(Error::Invalid(
                "an attachment couldn't be given to the worker; check its id".into(),
            ));
        }
        let paths: Vec<String> = files
            .iter()
            .map(|file| file.path.display().to_string())
            .collect();
        Ok(format!(
            "{text}\n\nFiles with this message (open each one): {}",
            paths.join(", ")
        ))
    }

    async fn find_attachments(
        &self,
        id: &ConversationId,
        ids: &[String],
    ) -> Result<Vec<AttachmentRef>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut found: Vec<Option<AttachmentRef>> = vec![None; ids.len()];
        let mut before = None;
        // Newest first, page by page, until every attachment is found or history ends.
        while found.iter().any(Option::is_none) {
            let page = self.core.list_messages(id.clone(), before, 500).await?;
            for attachment in page.messages.iter().flat_map(|m| m.attachments.iter()) {
                for (wanted, slot) in ids.iter().zip(found.iter_mut()) {
                    if slot.is_none() && &attachment.id == wanted {
                        *slot = Some(attachment.clone());
                    }
                }
            }
            match page.messages.first() {
                Some(oldest) if page.has_more => before = Some(oldest.seq),
                _ => break,
            }
        }
        ids.iter()
            .zip(found)
            .map(|(wanted, attachment)| {
                attachment.ok_or_else(|| {
                    Error::NotFound(format!("attachment {wanted} in this conversation"))
                })
            })
            .collect()
    }
}

/// The orchestrator's provider/model/effort override.
/// The most a headline may hold; its detail line holds twice that.
const HEADLINE_MAX: usize = 80;

/// `text` on one line, its whitespace collapsed, cut at `max` characters.
fn one_line(text: &str, max: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match joined.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", joined[..at].trim_end()),
        None => joined,
    }
}

fn pin(
    provider: Option<&str>,
    model: Option<String>,
    effort: Option<String>,
) -> Result<Option<brigadier_router::Pin>> {
    let provider = match provider.map(|p| p.trim().to_lowercase()) {
        None => None,
        Some(p) if p.is_empty() => None,
        Some(p) if p == "claude" => Some(ProviderKind::Claude),
        Some(p) if p == "codex" => Some(ProviderKind::Codex),
        Some(p) => {
            return Err(Error::Invalid(format!(
                "unknown provider {p}: use claude or codex"
            )));
        }
    };
    let model = model.filter(|m| !m.trim().is_empty());
    let effort = effort.filter(|e| !e.trim().is_empty());
    if provider.is_none() && model.is_none() && effort.is_none() {
        return Ok(None);
    }
    Ok(Some(brigadier_router::Pin {
        provider,
        model,
        effort,
        by_user: false,
    }))
}

/// `delegate_task`'s areas; none given: routing infers them from the spec.
fn task_areas(names: &[String]) -> Result<Option<Vec<brigadier_router::Area>>> {
    if names.is_empty() {
        return Ok(None);
    }
    names
        .iter()
        .map(|name| {
            serde_json::from_value(serde_json::Value::String(name.trim().to_lowercase())).map_err(
                |_| {
                    Error::Invalid(format!(
                        "unknown area \"{name}\": use frontend, backend, infra, docs or tests"
                    ))
                },
            )
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

/// What the orchestrator's message to a task's worker leaves on the task: the message (its
/// checks read it), and, unless it answered the worker's own question, the end of Brigadier
/// landing it on its own: a steer takes the task over.
fn messaged(task: &mut Task, text: String, answered: bool) {
    task.messages.push(text);
    if !answered {
        task.landing = None;
    }
}

/// A delegated task's part in a mockup first (SESSION-FIXES-PLAN.md Q10), from
/// `delegate_task`'s `mockup` and the task `matches_mockup` named (`built`), with the images it
/// is given: the real work gets the mockup's, as files to match.
fn mockup_part(
    builds: bool,
    built: Option<&Task>,
    kind: TaskKind,
    role: Option<WorkerRole>,
) -> Result<(Option<MockupPart>, Vec<AttachmentRef>)> {
    if !builds && built.is_none() {
        return Ok((None, Vec::new()));
    }
    if builds && built.is_some() {
        return Err(Error::Invalid(
            "a task builds a mockup (`mockup`) or matches one (`matches_mockup`), not both".into(),
        ));
    }
    if kind != TaskKind::Implement || role != Some(WorkerRole::Lead) {
        return Err(Error::Invalid(
            "a mockup, and the real work that matches one, is an implement task with one lead \
             (role lead)"
                .into(),
        ));
    }
    let Some(built) = built else {
        return Ok((Some(MockupPart::Builds), Vec::new()));
    };
    if built.mockup != Some(MockupPart::Builds) {
        return Err(Error::Invalid(format!(
            "task-{} didn't build a mockup: `matches_mockup` names the task delegated with \
             `mockup: true`",
            built.number
        )));
    }
    let images: Vec<AttachmentRef> = built
        .outputs
        .iter()
        .filter(|output| output.mime.starts_with("image/"))
        .map(|output| AttachmentRef {
            id: output.id.clone(),
            name: output
                .file_name
                .clone()
                .unwrap_or_else(|| output.title.clone()),
            mime: output.mime.clone(),
            bytes: output.bytes,
            pasted: false,
            inline: None,
        })
        .collect();
    if images.is_empty() {
        return Err(Error::Invalid(format!(
            "task-{} has no mockup images yet: the real work starts once its report brings them",
            built.number
        )));
    }
    Ok((
        Some(MockupPart::Matches {
            task: built.id.clone(),
        }),
        images,
    ))
}

/// The most questions one card asks, and the most options one question offers.
const ROUND_MAX: usize = 6;
const OPTIONS_MAX: usize = 4;

/// An `ask_user` round as the card's questions, or why it can't be asked as it is.
fn ask_round(questions: Vec<crate::tools::AskQuestion>) -> Result<Vec<QuestionItem>> {
    if questions.is_empty() || questions.len() > ROUND_MAX {
        return Err(Error::Invalid(format!(
            "ask_user takes 1 to {ROUND_MAX} questions in one round; ask the rest in the next round."
        )));
    }
    questions
        .into_iter()
        .map(|question| {
            let text = question.question.trim().to_owned();
            if text.is_empty() {
                return Err(Error::Invalid("a question of the round is empty".into()));
            }
            // A blank option goes; the recommendation keeps pointing at the option it named.
            let mut recommended = None;
            let mut options: Vec<QuestionOption> = Vec::new();
            for (index, option) in question.options.into_iter().enumerate() {
                let label = option.label.trim();
                if label.is_empty() {
                    continue;
                }
                if question.recommended == Some(index as u32) {
                    recommended = Some(options.len() as u32);
                }
                options.push(QuestionOption {
                    label: label.to_owned(),
                    description: option
                        .description
                        .map(|line| line.trim().to_owned())
                        .filter(|line| !line.is_empty()),
                });
            }
            if options.len() == 1 || options.len() > OPTIONS_MAX {
                return Err(Error::Invalid(format!(
                    "\"{text}\": give 2 to {OPTIONS_MAX} options (or none, for a free answer)."
                )));
            }
            if !options.is_empty() && recommended.is_none() {
                return Err(Error::Invalid(format!(
                    "\"{text}\": say which option you recommend in `recommended` (its 0-based index)."
                )));
            }
            Ok(QuestionItem {
                text,
                options,
                recommended,
            })
        })
        .collect()
}

/// A session offer's name when the thread gave none: the task's first sentence, cut to a few
/// words.
fn offer_title(task: &str) -> String {
    let first = task.split(['.', ':', '\n']).next().unwrap_or(task).trim();
    let words: Vec<&str> = first.split_whitespace().take(8).collect();
    let title = words.join(" ");
    if first.split_whitespace().count() > words.len() {
        format!("{title}…")
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answering_a_question_keeps_brigadiers_fix_loop_and_a_steer_ends_it() {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "blocked",
            "attachments": [],
            "landing": "Add the flag",
            "fixRounds": 1,
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        messaged(&mut task, "Yes, src/index.js may change.".into(), true);
        assert_eq!(task.landing.as_deref(), Some("Add the flag"));
        assert_eq!(
            task.messages,
            vec!["Yes, src/index.js may change.".to_owned()]
        );
        messaged(&mut task, "Stop and use the old API instead.".into(), false);
        assert_eq!(task.landing, None);
        assert_eq!(task.messages.len(), 2);
    }

    /// Q10: a mockup is an implement lead; the real work names a task that built one and has
    /// images, and gets those images (only those) as its files.
    #[test]
    fn the_real_work_gets_the_mockups_images_and_only_a_built_mockup_can_be_matched() {
        let implement = (TaskKind::Implement, Some(WorkerRole::Lead));
        let (none, images) = mockup_part(false, None, implement.0, implement.1).unwrap();
        assert_eq!((none, images.len()), (None, 0));
        let (builds, images) = mockup_part(true, None, implement.0, implement.1).unwrap();
        assert_eq!((builds, images.len()), (Some(MockupPart::Builds), 0));
        for (kind, role) in [
            (TaskKind::Scout, None),
            (TaskKind::Implement, Some(WorkerRole::Parallel)),
            (TaskKind::Implement, Some(WorkerRole::Fix)),
        ] {
            let refused = mockup_part(true, None, kind, role).unwrap_err().to_string();
            assert!(
                refused.contains("implement task with one lead"),
                "{refused}"
            );
        }

        let output = |name: &str, mime: &str| ArtifactRef {
            id: format!("hash-{name}"),
            title: name.to_owned(),
            kind: crate::work::ArtifactKind::File,
            mime: mime.to_owned(),
            bytes: 7,
            file_name: Some(name.to_owned()),
        };
        let mut built: Task = serde_json::from_value(serde_json::json!({
            "id": "t2",
            "conversationId": "c1",
            "number": 2,
            "position": 0,
            "title": "Density toggle mockup",
            "kind": "implement",
            "spec": "Build the mockup.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        let matches = |built: &Task| mockup_part(false, Some(built), implement.0, implement.1);
        let unbuilt = matches(&built).unwrap_err().to_string();
        assert!(
            unbuilt.contains("task-2 didn't build a mockup"),
            "{unbuilt}"
        );
        built.mockup = Some(MockupPart::Builds);
        let early = matches(&built).unwrap_err().to_string();
        assert!(early.contains("no mockup images yet"), "{early}");
        built.outputs = vec![
            output("mockup-1.png", "image/png"),
            output("notes.md", "text/markdown"),
            output("mockup-2.png", "image/png"),
        ];
        let (part, images) = matches(&built).unwrap();
        assert_eq!(
            part,
            Some(MockupPart::Matches {
                task: built.id.clone()
            })
        );
        let names: Vec<_> = images.iter().map(|image| image.name.as_str()).collect();
        assert_eq!(names, ["mockup-1.png", "mockup-2.png"]);
        assert_eq!(images[0].id, "hash-mockup-1.png");
        assert!(!images[0].pasted && images[0].inline.is_none());

        let both = mockup_part(true, Some(&built), implement.0, implement.1)
            .unwrap_err()
            .to_string();
        assert!(both.contains("not both"), "{both}");
    }

    #[test]
    fn a_blank_option_leaves_the_recommendation_on_the_option_it_named() {
        let round = |options: serde_json::Value, recommended: u32| {
            ask_round(vec![
                serde_json::from_value(serde_json::json!({
                    "question": "Which format?",
                    "options": options,
                    "recommended": recommended,
                }))
                .unwrap(),
            ])
        };
        let options =
            serde_json::json!([{ "label": " " }, { "label": "CSV" }, { "label": "JSON" }]);
        let [item] = &round(options.clone(), 1).unwrap()[..] else {
            panic!("one question")
        };
        assert_eq!(item.options.len(), 2);
        assert_eq!(item.recommended, Some(0));
        assert_eq!(item.options[0].label, "CSV");
        // Recommending the blank one recommends nothing: the thread is asked to say which.
        let blank = round(options, 0).unwrap_err().to_string();
        assert!(blank.contains("recommend"), "{blank}");
    }

    /// Every await of `run_orchestrator_call` goes through [`crate::heap`]: an unoptimized
    /// build keeps all of a function's awaited futures in its stack frame at once, and before
    /// that this dispatch's frame took 0.9 MB of a runtime thread's 2 MiB stack. Its future
    /// was 44 KB then and is 12 KB now (its locals); one past 16 KB means an engine future is
    /// awaited around it again.
    #[test]
    fn the_orchestrator_tools_dispatch_keeps_its_futures_on_the_heap() {
        fn future_size<R>(
            _: fn(&'static SessionManager, &'static ConversationId, OrchestratorCall) -> R,
        ) -> usize {
            std::mem::size_of::<R>()
        }
        let size = future_size(SessionManager::run_orchestrator_call);
        assert!(
            size <= 16 << 10,
            "run_orchestrator_call's future is {size} bytes"
        );
    }
}
