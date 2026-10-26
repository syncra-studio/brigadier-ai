//! A quota pause belongs to one worker. Its durable wait retains the task and workspace;
//! neither a lead handoff nor another worker's account change cancels it.
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use brigadier_providers::{LimitHit, LimitKind, Origin, ProviderKind, TurnInput};
use serde::{Deserialize, Serialize};

use super::TaskLive;
use crate::manager::conversation::Cli;
use crate::manager::seam::{Phase, SeamEnd, read_seam, write_seam};
use crate::manager::session_dir::write_atomic;
use crate::manager::{SessionManager, blocking, fallback};
use crate::model::{Lifecycle, ModelChoice};
use crate::work::{
    Attempt, AttemptEnd, OrchestratorStepKind, ProviderTransitionCause, QuotaRecoveryMode,
    QuotaWait, Task, TaskState,
};
use crate::{Error, Result, now_ms};

#[derive(Serialize, Deserialize)]
struct Transition {
    id: String,
    from: ModelChoice,
    to: ModelChoice,
    cause: ProviderTransitionCause,
    wait_tries: u32,
}

impl SessionManager {
    pub(in crate::manager) async fn pause_worker_for_quota(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        limit: LimitHit,
    ) {
        if live.state.lock().await.quota_cancelled {
            return;
        }
        let account = live
            .cli()
            .await
            .map(|cli| cli.account.clone())
            .unwrap_or_else(|| self.runtime.account_for(&task.route.choice));
        let known = self
            .task_by_id(&task.conversation_id, &task.id)
            .await
            .ok()
            .and_then(|task| task.quota_wait);
        let prior_limits = task
            .attempts
            .iter()
            .rev()
            .skip(1)
            .take_while(|attempt| matches!(attempt.end, Some(AttemptEnd::Limit { .. })))
            .count() as u32;
        let tries = known
            .as_ref()
            .map_or(prior_limits, |wait| wait.tries.saturating_add(1));
        let recovery = fallback::recovery(limit.resets_at_ms, Some(limit.kind), tries);
        let wait = QuotaWait {
            recovery: Some(recovery),
            reason: format!("{} hit its usage limit", task.route.choice.provider.label()),
            resets_at_ms: limit.resets_at_ms,
            rule: None,
            ranking: None,
            since_ms: known.as_ref().map_or_else(now_ms, |wait| wait.since_ms),
            messages: Vec::new(),
            model: Some(task.route.choice.clone()),
            request: task.request_id.clone(),
            first_reset_ms: known
                .as_ref()
                .and_then(|wait| wait.first_reset_ms)
                .or(limit.resets_at_ms),
            account: Some(account),
            limit_kind: Some(limit.kind),
            tries,
            landed: Vec::new(),
        };
        if let Err(err) = self
            .update_task(&task.conversation_id, &task.id, |task| {
                fallback::end_attempt(
                    task,
                    Some(AttemptEnd::Limit {
                        limit: limit.clone(),
                    }),
                );
                task.state = TaskState::Paused;
                task.blocked_reason = Some(wait.reason.clone());
                task.error = None;
                task.quota_wait = Some(Box::new(wait.clone()));
            })
            .await
        {
            tracing::error!(task = %task.id, error = %err, "could not keep worker quota wait");
            return;
        }
        let mut event = crate::manager::plain_workers::event(
            &task.id,
            brigadier_cli::protocol::EventKind::UsageLimit,
        );
        event.message = Some("a usage limit".into());
        event.resets_at_ms = limit.resets_at_ms;
        self.worker_event(&task.conversation_id, &task.id, None, event);
        // Never close the CLI from its own event pump: its close waits for that pump.
        let (manager, task) = (self.arc(), task.clone());
        self.spawn(async move {
            manager.retry_worker_quota(task).await;
        });
    }

    pub(in crate::manager) async fn cancel_worker_quota(
        &self,
        live: &TaskLive,
        task: &Task,
    ) -> Result<()> {
        let _transitioning = live.quota_transition.lock().await;
        {
            let mut state = live.state.lock().await;
            state.quota_cancelled = true;
            state.quota_timer += 1;
        }
        let current = self.task_by_id(&task.conversation_id, &task.id).await?;
        if let Some(wait) = current.quota_wait {
            let path = self.worker_record(task).join("quota-cancelled.json");
            blocking(move || write_atomic(&path, wait.since_ms.to_string().as_bytes())).await?;
            self.update_task(&task.conversation_id, &task.id, |task| {
                task.quota_wait = None;
            })
            .await?;
        }
        let path = self.worker_record(task).join("quota-transition.json");
        blocking(move || match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Invalid(format!(
                "could not cancel worker transition: {err}"
            ))),
        })
        .await
    }

    async fn worker_quota_choices(&self, task: &Task) -> Vec<ModelChoice> {
        let settings = self.core.settings();
        let policy = task
            .plain
            .as_ref()
            .and_then(|plain| plain.quota_policy.as_ref());
        let mut own = task.route.choice.clone();
        own.account = None; // Actual account history is not a hard account pin.
        let mut choices = vec![own];
        if settings.switch_providers
            && let Some(policy) = policy
        {
            let rows = match policy.work.as_deref() {
                Some(work) => self
                    .work_rows(work)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|row| ModelChoice {
                        provider: match row.agent {
                            brigadier_cli::protocol::Agent::Claude => ProviderKind::Claude,
                            brigadier_cli::protocol::Agent::Codex => ProviderKind::Codex,
                        },
                        model: (!row.model.eq_ignore_ascii_case("default"))
                            .then(|| row.model.clone()),
                        effort: row.effort,
                        fast: None,
                        account: None,
                    })
                    .collect(),
                None => self.fallback_rows(),
            };
            choices.extend(rows);
        }
        let mut allowed = Vec::new();
        for choice in choices {
            if allowed.contains(&choice) {
                continue;
            }
            if choice.provider != task.route.choice.provider
                && !crate::routing::availability::provider_on(&settings, choice.provider)
            {
                continue;
            }
            if [task.pin.as_ref(), policy.map(|policy| &policy.pin)]
                .into_iter()
                .flatten()
                .any(|pin| {
                    pin.provider
                        .is_some_and(|provider| provider != choice.provider)
                        || pin
                            .model
                            .as_ref()
                            .is_some_and(|model| choice.model.as_ref() != Some(model))
                        || pin
                            .effort
                            .as_ref()
                            .is_some_and(|effort| choice.effort.as_ref() != Some(effort))
                })
            {
                continue;
            }
            if policy.is_some_and(|policy| policy.avoid_providers.contains(&choice.provider)) {
                continue;
            }
            if self
                .model_refusal(
                    choice.provider,
                    choice.model.as_deref(),
                    choice.effort.as_deref(),
                    policy.and_then(|p| p.work.as_deref()),
                )
                .map_or(true, |rule| rule.is_some())
            {
                continue;
            }
            if choice.provider != task.route.choice.provider
                && let Some(plain) = &task.plain
            {
                let Some(workspace) = &task.workspace else {
                    continue;
                };
                let cwd = workspace.worktree.as_deref().unwrap_or(&workspace.scratch);
                if self
                    .worker_grants_named(
                        &task.conversation_id,
                        choice.provider,
                        std::path::Path::new(cwd),
                        &plain.mcps,
                    )
                    .await
                    .is_err()
                {
                    continue;
                }
            }
            allowed.push(choice);
        }
        allowed
    }

    pub(in crate::manager) fn retry_worker_quota<'a>(
        &'a self,
        original: Task,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let live = self.task_live(&original);
            let _settled = live.settle.lock().await;
            let _rerouting = live.reroute.lock().await;
            let Ok(task) = self
                .task_by_id(&original.conversation_id, &original.id)
                .await
            else {
                return;
            };
            let Some(wait) = task.quota_wait.as_ref() else {
                return;
            };
            if task.state.is_final() || live.state.lock().await.quota_cancelled || live.busy().await
            {
                return;
            }
            if self
                .core
                .conversation(&task.conversation_id)
                .map_or(true, |conv| conv.lifecycle != Lifecycle::Active)
            {
                return;
            }
            let cancelled_at = self.worker_record(&task).join("quota-cancelled.json");
            if std::fs::read_to_string(cancelled_at)
                .ok()
                .and_then(|text| text.parse::<i64>().ok())
                == Some(wait.since_ms)
            {
                return;
            }
            let choices = self.worker_quota_choices(&task).await;
            let mut target = None;
            for choice in &choices {
                let account = crate::accounts::ready(
                    &self.core.settings(),
                    choice.provider,
                    choice.model.as_deref(),
                    &self.runtime.account_candidates(choice.provider),
                    &[],
                    now_ms(),
                );
                if let Some(account) = account {
                    // Credits/spend controls require an explicit action on the same account.
                    if wait
                        .limit_kind
                        .is_some_and(|kind| kind != LimitKind::UsageWindow)
                        && wait.account.as_ref() == Some(&account)
                    {
                        continue;
                    }
                    let mut choice = choice.clone();
                    choice.account = Some(account.choice_id());
                    target = Some(choice);
                    break;
                }
            }
            let Some(target) = target else {
                let reset = choices
                    .iter()
                    .flat_map(|choice| {
                        self.runtime
                            .account_candidates(choice.provider)
                            .into_iter()
                            .filter(|candidate| candidate.logged_in)
                            .filter_map(move |candidate| {
                                candidate.quota.as_ref().and_then(|quota| {
                                    crate::accounts::ready_again_at(quota, choice.model.as_deref())
                                })
                            })
                    })
                    .filter(|at| *at > now_ms())
                    .min();
                let recovery = fallback::recovery(reset, wait.limit_kind, wait.tries);
                let recovery = wait
                    .recovery
                    .clone()
                    .filter(|known| {
                        wait.resets_at_ms == reset
                            && known.mode == recovery.mode
                            && known.next_check_at_ms.is_some_and(|at| at > now_ms())
                    })
                    .unwrap_or(recovery);
                let _ = self
                    .update_task(&task.conversation_id, &task.id, |task| {
                        if let Some(wait) = &mut task.quota_wait {
                            wait.resets_at_ms = reset;
                            wait.recovery = Some(recovery);
                        }
                    })
                    .await;
                if let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await {
                    self.arm_worker_quota_retry(&task).await;
                }
                return;
            };
            let mut from = wait
                .model
                .clone()
                .unwrap_or_else(|| task.route.choice.clone());
            if let Some(account) = &wait.account {
                from.account = Some(account.choice_id());
            }
            let cause = if target.provider == from.provider
                && wait.account.as_ref().is_some_and(|account| {
                    target.account.as_deref() == Some(account.choice_id().as_str())
                }) {
                ProviderTransitionCause::QuotaReset
            } else {
                ProviderTransitionCause::AutomaticFallback
            };
            let intent = Transition {
                id: uuid::Uuid::new_v4().to_string(),
                from,
                to: target.clone(),
                cause,
                wait_tries: wait.tries,
            };
            let path = self.worker_record(&task).join("quota-transition.json");
            if blocking(move || {
                write_atomic(
                    &path,
                    &serde_json::to_vec(&intent).map_err(|err| Error::Invalid(err.to_string()))?,
                )
            })
            .await
            .is_err()
            {
                return;
            }
            let predecessor = live
                .cli()
                .await
                .map(|cli| cli.session.native_id())
                .or(task.native_session.clone());
            live.close_cli().await;
            live.allow_revival().await;
            if live.state.lock().await.quota_cancelled {
                return;
            }
            let changed_provider = target.provider != task.route.choice.provider;
            if changed_provider {
                let record = self.worker_record(&task);
                if blocking(move || {
                    let mut seam = read_seam(&record);
                    seam.phase = Phase::Closed { end: SeamEnd { predecessor, hot_tokens: None, nudged_at_ms: None, closed_at_ms: now_ms(), death: Some("its provider hit a usage limit; continue from the worker's records and check git before repeating work".into()), stopped: false, shown: Vec::new(), moved_to: None } };
                    write_seam(&record, &seam)
                }).await.is_err() { return; }
            }
            if self
                .update_task(&task.conversation_id, &task.id, |task| {
                    task.route.choice = target;
                    task.attempts.push(Attempt {
                        route: task.route.clone(),
                        started_at_ms: now_ms(),
                        ended_at_ms: None,
                        end: None,
                    });
                })
                .await
                .is_err()
            {
                return;
            }
            let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return;
            };
            let input = "[Quota is available again. Check git status and git log first: earlier commands may have completed. Continue the existing task without repeating completed work.]";
            let launched = if changed_provider || self.worker_seam_closed(&task).await {
                self.worker_successor(&live, &task, vec![input.into()])
                    .await
            } else if let Some(native_id) = self.last_worker_native_id(&task).await {
                self.launch_worker(
                    &live,
                    &task,
                    Origin::Resume { native_id },
                    TurnInput::text(input),
                )
                .await
            } else {
                Err(Error::Invalid("the worker has no saved CLI session".into()))
            };
            if let Err(err) = launched {
                live.close_cli().await;
                live.allow_revival().await;
                let recovery =
                    fallback::recovery(None, wait.limit_kind, wait.tries.saturating_add(1));
                let _ = self
                    .update_task(&task.conversation_id, &task.id, |task| {
                        task.state = TaskState::Paused;
                        task.error = Some(err.to_string());
                        if let Some(wait) = &mut task.quota_wait {
                            wait.recovery = Some(recovery);
                            wait.tries += 1;
                        }
                    })
                    .await;
                if let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await {
                    self.arm_worker_quota_retry(&task).await;
                }
            }
        })
    }

    pub(in crate::manager) async fn arm_worker_quota_retry(&self, task: &Task) {
        let Some(wait) = task.quota_wait.as_ref() else {
            return;
        };
        let recovery = wait
            .recovery
            .clone()
            .unwrap_or_else(|| fallback::recovery(wait.resets_at_ms, wait.limit_kind, wait.tries));
        if recovery.mode == QuotaRecoveryMode::ManualOnly {
            return;
        }
        let live = self.task_live(task);
        let timer = {
            let mut state = live.state.lock().await;
            state.quota_timer += 1;
            state.quota_timer
        };
        let delay = Duration::from_millis(
            (recovery.next_check_at_ms.unwrap_or_else(now_ms) - now_ms()).max(0) as u64,
        );
        let (manager, task) = (self.arc(), task.clone());
        self.spawn(async move {
            tokio::time::sleep(delay).await;
            if live.state.lock().await.quota_timer == timer {
                manager.retry_worker_quota(task).await;
            }
        });
    }

    pub(in crate::manager) async fn retry_waiting_workers(&self) {
        for conversation in self.core.catalog().conversations {
            if conversation.lifecycle != Lifecycle::Active {
                continue;
            }
            if let Ok(tasks) = self.core.tasks(&conversation.id).await {
                for task in tasks
                    .into_iter()
                    .filter(|task| task.quota_wait.is_some() && !task.state.is_final())
                {
                    self.retry_worker_quota(task).await;
                }
            }
        }
    }

    pub(in crate::manager) async fn confirm_worker_quota_transition(
        &self,
        live: &TaskLive,
        cli: &Cli,
    ) {
        let _transitioning = live.quota_transition.lock().await;
        {
            let state = live.state.lock().await;
            if state.stopping
                || state.quota_cancelled
                || !state.quota_send_accepted
                || !state.quota_turn_started
                || !state
                    .cli
                    .as_ref()
                    .is_some_and(|current| std::ptr::eq(current.as_ref(), cli))
            {
                return;
            }
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        let path = self.worker_record(&task).join("quota-transition.json");
        let Some(intent) = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Transition>(&bytes).ok())
        else {
            return;
        };
        if intent.to.provider != cli.provider
            || intent.to.model != cli.model.model
            || intent.to.account.as_deref() != Some(cli.account.choice_id().as_str())
        {
            return;
        }
        let already = self.core.board(&task.conversation_id).await.is_ok_and(|board| board.orchestrator_steps.iter().any(|step|
            matches!(&step.kind, OrchestratorStepKind::ProviderTransition { id, .. } if id == &intent.id)));
        if !already {
            let mut to = cli.model.clone();
            to.account = Some(cli.account.choice_id());
            self.orchestrator_step_in(
                &task.conversation_id,
                task.request_id.clone(),
                OrchestratorStepKind::ProviderTransition {
                    task_id: Some(task.id.clone()),
                    id: intent.id,
                    cause: intent.cause,
                    from: intent.from,
                    to,
                    account: cli.account.clone(),
                    account_label: self.runtime.account_label(&cli.account),
                    started_at_ms: now_ms(),
                },
            )
            .await;
        }
        if self
            .update_task(&task.conversation_id, &task.id, |task| {
                // A fast response can hit the next limit before send() returns. Its new
                // wait is a different attempt and must not be cleared by the old launch.
                if task.state == TaskState::Running
                    && task
                        .quota_wait
                        .as_ref()
                        .is_some_and(|wait| wait.tries == intent.wait_tries)
                {
                    task.quota_wait = None;
                    task.blocked_reason = None;
                }
            })
            .await
            .is_ok()
        {
            if self
                .task_by_id(&task.conversation_id, &task.id)
                .await
                .is_ok_and(|task| task.quota_wait.is_none())
            {
                live.state.lock().await.quota_timer += 1;
            }
            let _ = std::fs::remove_file(path);
        }
    }
}
