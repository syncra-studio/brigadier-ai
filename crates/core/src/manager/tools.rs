//! The Brigadier MCP tools, answered. Orchestrator tools return at once; their outcomes
//! arrive later as envelopes. Every reply the orchestrator reads is logged as a context
//! injection.

use brigadier_providers::{FileSearch, ProviderKind, SearchKind};

use super::SessionManager;
use super::prompts;
use super::workers::route_label;
use crate::model::{ConversationId, DomainEvent, Environment, PermissionLevel, Setup};
use crate::tools::{NoteKind, OrchestratorCall, ToolReply, WorkerCall};
use crate::work::{
    ApprovalSubject, ArtifactRef, AttachmentRef, DecisionSource, InjectionKind, OrchestratorStep,
    OrchestratorStepKind, QuestionItem, QuestionKind, QuestionOption, Task, TaskId, TaskKind,
    WaitingSource, WorkerRole,
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
                    self.check_overnight_proposal(id).await?;
                }
                if args.kind == TaskKind::Merge {
                    self.check_plan_mode(id).await?;
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
                    (Some(reference), _) => Some(self.find_task(id, reference).await?),
                    (None, TaskKind::Review) => {
                        return Err(Error::Invalid(
                            "a review task needs `subject`: the task whose change it reviews"
                                .into(),
                        ));
                    }
                    // With no task, Brigadier merges the session's base into the session
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
                    let request = self.request_for(id, None).await;
                    self.phase_step(id, request, phase).await?;
                }
                let effort = match args.effort.trim().to_lowercase().as_str() {
                    effort @ ("low" | "medium" | "high") => effort.to_owned(),
                    other => {
                        return Err(Error::Invalid(format!(
                            "unknown effort \"{other}\": use \"low\", \"medium\" or \"high\""
                        )));
                    }
                };
                let pin = pin(args.provider.as_deref(), args.model, Some(effort))?;
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
                let attachments = self.find_attachments(id, &args.attachments).await?;
                let avoid = subject
                    .as_ref()
                    .filter(|_| args.kind == TaskKind::Review)
                    .map(|s| brigadier_router::Author {
                        provider: s.route.choice.provider,
                        model: s.route.choice.model.clone(),
                    });
                let task = self
                    .create_task_as(
                        id,
                        args.title,
                        args.kind,
                        args.spec,
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
                            ..Default::default()
                        },
                    )
                    .await?;
                // A lead of a phase takes it over (a new lead redoes it).
                if let Some(phase) = args.phase
                    && role == Some(WorkerRole::Lead)
                {
                    self.assign_phase(&task, phase).await?;
                }
                self.orchestrator_step(
                    id,
                    OrchestratorStepKind::Created {
                        task_id: task.id.clone(),
                    },
                )
                .await;
                if let Some(wait) = &task.quota_wait {
                    return Ok(format!(
                        "Created task-{} ({:?}), but no model it may use can take it now: {}. It \
                         starts on its own when one can (after a reset, or when the user changes \
                         their routing); its report arrives later as a message. The user sees \
                         it waiting, so don't announce it: if nothing else is needed now, reply \
                         with exactly {} and nothing else.",
                        task.number,
                        task.kind,
                        wait.reason,
                        prompts::QUIET
                    ));
                }
                Ok(format!(
                    "Started task-{} ({:?}) on {}: {}. Its report arrives later as a message; don't wait for it. \
                     The user sees the worker live, so don't announce it: if nothing else is \
                     needed now, reply with exactly {} and nothing else.",
                    task.number,
                    task.kind,
                    route_label(&task),
                    task.route.reason,
                    prompts::QUIET
                ))
            }
            OrchestratorCall::MessageWorker(args) => {
                let mut task = self.find_task(id, &args.task).await?;
                // A later request's turn hands the worker its question: the report that
                // answers it, and the reply after it, belong to the request that asked.
                if !task.state.is_final()
                    && let Some(later) = self.later_request_for(id, &task).await
                {
                    task = self
                        .update_task(id, &task.id, |task| task.request_id = Some(later))
                        .await?;
                    self.settle_requests(id).await;
                }
                let (reply, answered) = self
                    .message_worker(id, &task, args.text.clone(), "the orchestrator")
                    .await?;
                let text = super::brains::cut(args.text.trim(), crate::work::STEP_TEXT_MAX);
                self.update_task(id, &task.id, |task| messaged(task, args.text, answered))
                    .await?;
                self.orchestrator_step(
                    id,
                    OrchestratorStepKind::Messaged {
                        task_id: task.id,
                        text: Some(text),
                    },
                )
                .await;
                Ok(reply)
            }
            OrchestratorCall::AnswerWorker(args) => {
                let task = self.find_task(id, &args.task).await?;
                let reply = self
                    .answer_worker(&task, args.answer.clone(), args.why)
                    .await?;
                self.update_task(id, &task.id, |task| messaged(task, args.answer, true))
                    .await?;
                Ok(reply)
            }
            OrchestratorCall::RouteFollowUp(args) => {
                self.route_follow_up(id, &args.follow_up, args.joins).await
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
                let task = self.find_task(id, &args.task).await?;
                self.stop_worker(id, &task, reason).await?;
                Ok(format!("Stopped task-{}.", task.number))
            }
            OrchestratorCall::AskUser(args) => {
                let items = ask_round(args.questions)?;
                let task_id = match &args.task {
                    Some(reference) => Some(self.find_task(id, reference).await?.id),
                    None => None,
                };
                let asked = items.len();
                self.open_round(id, task_id, QuestionKind::Orchestrator, items)
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
                let (task, here) = self.find_report(id, &args.task).await?;
                let report = task.report.as_ref().ok_or_else(|| {
                    Error::Invalid(format!("task-{} has not reported yet", task.number))
                })?;
                let mut text = prompts::report_envelope(&task, report, &route_label(&task));
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
                self.orchestrator_step(id, step).await;
                Ok(text)
            }
            OrchestratorCall::ReadArtifact(args) => {
                // A trimmed command output of this conversation's (`out-<id>`), or a report's
                // artifact of this project.
                let output = self.output_blob(id, args.id.trim()).await?;
                if output.is_none() {
                    self.check_artifact(id, &args.id).await?;
                }
                let limit = args
                    .limit
                    .unwrap_or(ARTIFACT_PAGE_MAX)
                    .min(ARTIFACT_PAGE_MAX);
                let offset = args.offset.unwrap_or(0);
                let blob = output
                    .as_ref()
                    .map_or_else(|| args.id.clone(), |(blob, _)| blob.clone());
                let (bytes, total) = self.core.read_blob_range(blob, offset, limit).await?;
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
                        None => self.artifact_name(id, &args.id).await,
                    };
                    self.orchestrator_step(id, OrchestratorStepKind::ReadArtifact { name })
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
            OrchestratorCall::QueryBrain(args) => {
                self.query_brain_tool(
                    id,
                    args.query,
                    args.history.unwrap_or(false),
                    args.page,
                    false,
                )
                .await
            }
            OrchestratorCall::Remember(args) => self.remember_tool(id, args).await,
            OrchestratorCall::SearchTranscript(args) => self.search_transcript_tool(id, args).await,
            OrchestratorCall::PlanPhases(args) => self.plan_phases(id, args).await,
            OrchestratorCall::ProposePlan(args) => self.propose_plan(id, args).await,
            OrchestratorCall::ApproveOutline(args) => self.approve_outline(id, args).await,
            OrchestratorCall::StartVerifier(args) => {
                // A verifier changes code: in plan mode nothing does.
                self.check_plan_mode(id).await?;
                self.verify_task(id, args).await
            }
            OrchestratorCall::RequestApproval(args) => {
                self.open_approval(
                    id,
                    None,
                    ApprovalSubject::Action {
                        action: args.action,
                        details: args.details,
                        live: false,
                    },
                )
                .await?;
                Ok("Asked the user. The decision arrives later as a message.".into())
            }
            OrchestratorCall::LandPhase(args) => {
                self.check_plan_mode(id).await?;
                let task = self.find_task(id, &args.task).await?;
                self.land_phase(id, task).await
            }
            OrchestratorCall::FinishSession(args) => {
                self.check_plan_mode(id).await?;
                self.finish_session(id, &args.user_words, args.message)
                    .await
            }
            OrchestratorCall::ProposeMerge(args) => {
                self.check_plan_mode(id).await?;
                self.propose_merge(id, args.note).await
            }
            OrchestratorCall::NoteForUser(args) => self.note_for_user(id, args).await,
            OrchestratorCall::SuggestFullAccess(args) => {
                self.suggest_full_access(id, &args.reason).await
            }
            OrchestratorCall::SettleStep(args) => self.settle_step(id, args).await,
            OrchestratorCall::EndRun(args) => self.end_run_now(id, args).await,
            OrchestratorCall::ProposeOvernight(args) => self.interpret_overnight(id, args).await,
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
                let (text, hits) = self.code_search_tool(id, args).await?;
                self.looked_in_index(id, FileSearch { hits, ..search })
                    .await;
                Ok(text)
            }
            OrchestratorCall::CodeRefs(args) => {
                let symbol = args.symbol.clone();
                let (text, hits) = self.code_refs_tool(id, args).await?;
                let search = FileSearch {
                    kind: SearchKind::Content,
                    pattern: Some(symbol),
                    scope: None,
                    glob: None,
                    hits,
                };
                self.looked_in_index(id, search).await;
                Ok(text)
            }
            OrchestratorCall::ProjectMap => self.project_map_tool(id).await,
            OrchestratorCall::ReviewPlan(args) => self.review_thread_plan(id, args).await,
            OrchestratorCall::Run(args) => {
                self.run_tool(
                    id,
                    &args.command,
                    args.workdir.as_deref(),
                    args.timeout_secs,
                    false,
                )
                .await
            }
            // Codex let it through: under Approve for me its auto-reviewer approved it, under
            // Ask for approval the user did.
            OrchestratorCall::RunUnsandboxed(args) => {
                self.run_tool(
                    id,
                    &args.command,
                    args.workdir.as_deref(),
                    args.timeout_secs,
                    true,
                )
                .await
            }
            OrchestratorCall::RunCheck(args) => self.run_check_tool(id, None, args).await,
            OrchestratorCall::StartPreview(args) => self.start_preview(id, args).await,
            OrchestratorCall::StopPreview(args) => {
                self.stop_preview_tool(id, args.id.as_deref()).await
            }
            OrchestratorCall::PreviewLog(args) => {
                self.preview_log(id, args.id.as_deref(), args.tail_lines)
                    .await
            }
            OrchestratorCall::ListTasks => {
                let tasks = self.core.tasks(id).await?;
                if tasks.is_empty() {
                    return Ok("No tasks yet.".into());
                }
                Ok(tasks
                    .iter()
                    .map(|task| {
                        format!(
                            "task-{} {:?} {:?} on {}: {}{}",
                            task.number,
                            task.kind,
                            task.state,
                            route_label(task),
                            task.title,
                            task.blocked_reason
                                .as_ref()
                                .map(|r| format!(" ({r})"))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
        }
    }

    /// Files a row for the thread under the request the orchestrator serves.
    pub(crate) async fn orchestrator_step(&self, id: &ConversationId, kind: OrchestratorStepKind) {
        let request = self.request_for(id, None).await;
        self.orchestrator_step_in(id, request, kind).await;
    }

    /// Files a row for the thread under `request`.
    pub(crate) async fn orchestrator_step_in(
        &self,
        id: &ConversationId,
        request: Option<String>,
        kind: OrchestratorStepKind,
    ) {
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
                self.worker_question(&conversation_id, &task_id, args.question)
                    .await
            }
            WorkerCall::SubmitOutline(args) => {
                self.submit_outline(&conversation_id, &task_id, args.outline)
                    .await
            }
            WorkerCall::ReviewCode => self.review_code(&conversation_id, &task_id).await,
            WorkerCall::QueryBrain(args) => {
                self.query_brain_tool(
                    &conversation_id,
                    args.query,
                    args.history.unwrap_or(false),
                    args.page,
                    true,
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
                if self.computer_permission_asked(id, what).await {
                    return Ok("It is already listed under Waiting on you: Brigadier asked for \
                               computer use's permissions itself and closes that item once they're in."
                        .into());
                }
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
                "The user doesn't trust this project's folder, so this session asks first whatever its level: say in your answer what needs more access; trusting the folder is in the project's settings.".into(),
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

    /// The user's attachments in this conversation, by id.
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
}
