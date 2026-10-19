//! Fusion (Q77): a research question costly to get wrong goes to a Claude worker and a Codex
//! worker on the same brief, and the thread judges their answers itself.
//!
//! The thread decides when (`delegate_task`'s `fuse`); Brigadier only plumbs:
//!
//! - **Blind.** The thread knows the two only as answers A and B, assigned at random: their
//!   start reply, report envelopes, `read_report` and stored full reports name no vendor or
//!   model ([`super::workers::route_label`]).
//! - **Together.** The first report waits for the second, and both reach the thread in one
//!   message. The pairing, the labels and whether they were delivered are on the tasks, so a
//!   restart in between changes nothing.
//! - **Alone.** A partner that fails or is stopped without a report leaves the other one's
//!   report to go alone, with one plain line saying only one answer came back.
//! - **One vendor.** With only one vendor usable there is no pair: one worker starts, and
//!   nothing says fusion.

use brigadier_providers::ProviderKind;

use super::SessionManager;
use super::prompts;
use super::workers::TaskExtra;
use crate::model::ConversationId;
use crate::tools::DelegateTask;
use crate::work::{Fusion, InjectionKind, Task, TaskId, TaskKind};
use crate::{Error, Result};

/// How a fused pair's reports reach the thread, after the reports.
const FUSED_NOTE: &str = "[fused answers] These are two independent answers to the same brief, A and B. Before you pick, name where they disagree (facts, numbers, the recommendation) and check what decides it; then write one answer as your instructions say, with one line `Confidence: high, medium or low, and why` and one line `Disagreement: …` (or `Disagreement: none`), each a paragraph of its own. Link a file of either by the link its report gives, which opens that answer's own copy. Never say which model or vendor gave which.";

/// How a lone report of a pair reaches the thread.
const LONE_NOTE: &str = "[one answer] Only one of the two independent answers to this brief came back; write your answer from it as usual, with no Confidence or Disagreement line.";

/// The pair's two labels, in the order the vendors are assigned.
const LABELS: [&str; 2] = ["A", "B"];

/// What both workers of a fused pair get: the brief as `delegate_task` resolved it.
pub(crate) struct FusedBrief {
    pub spec: String,
    pub effort: String,
    pub request: Option<String>,
    pub attachments: Vec<crate::work::AttachmentRef>,
    pub areas: Option<Vec<brigadier_router::Area>>,
    pub floor: Option<brigadier_router::QualityTier>,
    pub needs: Vec<brigadier_router::Capability>,
}

/// What a fused task's report does when it comes in.
pub(crate) enum FusedReport {
    /// Its partner hasn't reported yet: it waits, delivering nothing.
    Hold,
    /// Both are in: this envelope carries both, and both count as delivered.
    Both { text: String, partner: TaskId },
    /// Its partner ended without a report: it goes alone.
    Alone { text: String },
}

impl SessionManager {
    /// Whether both vendors can take work now: a fused pair needs both.
    pub(crate) fn both_vendors_usable(&self) -> bool {
        [ProviderKind::Claude, ProviderKind::Codex]
            .into_iter()
            .all(|kind| self.provider_usable(kind))
    }

    /// `delegate_task` with `fuse`: the same research brief to a Claude and a Codex worker,
    /// known to the thread as A and B. `None` when only one vendor is usable: the caller starts
    /// one worker as usual.
    pub(crate) async fn delegate_fused(
        &self,
        id: &ConversationId,
        args: &DelegateTask,
        brief: FusedBrief,
    ) -> Result<Option<String>> {
        let FusedBrief {
            spec,
            effort,
            request,
            attachments,
            areas,
            floor,
            needs,
        } = brief;
        if args.kind != TaskKind::Research {
            return Err(Error::Invalid(
                "`fuse` is for research tasks: kind research".into(),
            ));
        }
        if args.provider.is_some() || args.model.is_some() {
            return Err(Error::Invalid(
                "a fused task runs on both vendors: leave `provider` and `model` out".into(),
            ));
        }
        if !self.both_vendors_usable() {
            return Ok(None);
        }
        let ids = [TaskId::generate(), TaskId::generate()];
        // Which vendor is A, at random, so the thread can't learn it from the order.
        let mut vendors = [ProviderKind::Claude, ProviderKind::Codex];
        if uuid::Uuid::new_v4().as_bytes()[0] & 1 == 1 {
            vendors.swap(0, 1);
        }
        let mut started: Vec<Task> = Vec::new();
        for index in 0..2 {
            let pin = brigadier_router::Pin {
                provider: Some(vendors[index]),
                model: None,
                effort: Some(effort.clone()),
                by_user: false,
            };
            let task = crate::heap(|| {
                self.create_task_as(
                    id,
                    args.title.clone(),
                    TaskKind::Research,
                    spec.clone(),
                    Some(pin),
                    None,
                    Vec::new(),
                    None,
                    None,
                    attachments.clone(),
                    areas.clone(),
                    floor,
                    needs.clone(),
                    TaskExtra {
                        id: Some(ids[index].clone()),
                        request: request.clone(),
                        fusion: Some(Fusion {
                            partner: ids[1 - index].clone(),
                            label: LABELS[index].to_owned(),
                            delivered: false,
                        }),
                        ..Default::default()
                    },
                )
            })
            .await;
            match task {
                Ok(task) => started.push(task),
                // The second couldn't start: the first goes on as a plain task.
                Err(err) if index == 1 => {
                    tracing::info!(error = %err, "the second of a fused pair didn't start");
                    let first = &started[0];
                    self.update_task(id, &first.id, |task| task.fusion = None)
                        .await?;
                    return Ok(Some(format!(
                        "Started task-{} (Research). Its report arrives later as a message; don't wait for it. If you have nothing to say now, reply with exactly {}.",
                        first.number,
                        prompts::QUIET
                    )));
                }
                Err(err) => return Err(err),
            }
        }
        Ok(Some(format!(
            "Started task-{} (answer A) and task-{} (answer B): the same brief, researched independently. Their reports arrive later together, as answers A and B; don't wait for them. If you have nothing to say now, reply with exactly {}.",
            started[0].number,
            started[1].number,
            prompts::QUIET
        )))
    }

    /// What `task`'s report does now that it came in (`envelope`: its own report's text).
    /// Called under [`SessionManager::fusing`], before its report is recorded.
    pub(crate) async fn fused_report(&self, task: &Task, envelope: &str) -> FusedReport {
        let Some(fusion) = &task.fusion else {
            return FusedReport::Alone {
                text: envelope.to_owned(),
            };
        };
        let Ok(partner) = self
            .task_by_id(&task.conversation_id, &fusion.partner)
            .await
        else {
            return FusedReport::Alone {
                text: lone(envelope),
            };
        };
        match (&partner.report, &partner.fusion) {
            (Some(report), Some(theirs)) if !theirs.delivered => {
                let other = prompts::report_envelope(
                    &partner,
                    report,
                    &super::workers::route_label(&partner),
                );
                let (a, b) = if fusion.label == LABELS[0] {
                    (envelope.to_owned(), other)
                } else {
                    (other, envelope.to_owned())
                };
                FusedReport::Both {
                    text: format!("{a}\n\n{b}\n{FUSED_NOTE}"),
                    partner: partner.id.clone(),
                }
            }
            _ if partner.state.is_final() => FusedReport::Alone {
                text: lone(envelope),
            },
            _ => FusedReport::Hold,
        }
    }

    /// Delivers the held report of each pair in `id` whose partner ended without one: it goes
    /// alone. Whether any went.
    ///
    /// Boxed with its `Send` said, as it settles requests, which may start it again.
    pub(crate) fn deliver_lone_fused<'a>(
        &'a self,
        id: &'a ConversationId,
    ) -> brigadier_providers::BoxFuture<'a, bool> {
        Box::pin(self.deliver_lone_fused_now(id))
    }

    async fn deliver_lone_fused_now(&self, id: &ConversationId) -> bool {
        let _fusing = self.fusing.lock().await;
        let Ok(board) = self.core.board(id).await else {
            return false;
        };
        let lone_ones: Vec<Task> = board
            .tasks
            .values()
            .filter(|task| {
                let Some(fusion) = &task.fusion else {
                    return false;
                };
                !fusion.delivered
                    && task.report.is_some()
                    && board
                        .tasks
                        .get(&fusion.partner)
                        .is_none_or(|partner| partner.state.is_final() && partner.report.is_none())
            })
            .cloned()
            .collect();
        let mut delivered = false;
        for task in lone_ones {
            let Some(report) = &task.report else {
                continue;
            };
            let text = lone(&prompts::report_envelope(
                &task,
                report,
                &super::workers::route_label(&task),
            ));
            if self
                .update_task(id, &task.id, |task| {
                    if let Some(fusion) = &mut task.fusion {
                        fusion.delivered = true;
                    }
                })
                .await
                .is_err()
            {
                continue;
            }
            let envelope = super::conversation::Envelope {
                kind: InjectionKind::Report,
                label: format!("report task-{}", task.number),
                task_id: Some(task.id.clone()),
                text,
            };
            let request = task.request_id.clone();
            if let Some(conv) = self.queue_envelope(id, envelope, request).await {
                self.kick(&conv);
            }
            delivered = true;
        }
        delivered
    }
}

/// A lone report's envelope.
fn lone(envelope: &str) -> String {
    format!("{envelope}\n{LONE_NOTE}")
}

/// How the thread knows a fused task: by its answer's letter alone.
pub(crate) fn fused_label(task: &Task) -> Option<String> {
    task.fusion
        .as_ref()
        .map(|fusion| format!("answer {}", fusion.label))
}
