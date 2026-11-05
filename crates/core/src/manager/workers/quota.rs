//! A quota pause belongs to one worker. Its durable wait retains the task and workspace;
//! neither a lead handoff nor another worker's account change cancels it.
use brigadier_providers::KindFacts as _;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};

use brigadier_providers::{LimitHit, LimitKind, Origin, TurnInput, TurnStatus};
use serde::{Deserialize, Serialize};

use super::TaskLive;
use crate::accounts::AccountRef;
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

#[derive(Clone, Serialize, Deserialize)]
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

    /// Its CLI gave up on its model (at capacity, overloaded, a server or network error): the
    /// worker goes on at once with the next model its run hasn't seen fail, with the usual
    /// "Switched" row. With none left, its turn ends as a failed one does and says so.
    pub(in crate::manager) fn move_off_failed_model(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        failure: AttemptEnd,
    ) {
        let (manager, live, task) = (self.arc(), live.clone(), task.clone());
        // Never close the CLI from its own event pump: its close waits for that pump.
        self.spawn(async move {
            manager.go_on_past_failed_model(&live, &task, failure).await;
        });
    }

    async fn go_on_past_failed_model(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        failure: AttemptEnd,
    ) {
        let mut failure = failure;
        loop {
            let settled = live.settle.lock().await;
            let rerouting = live.reroute.lock().await;
            let Ok(task) = self.task_by_id(&task.conversation_id, &task.id).await else {
                return;
            };
            let tried = {
                let mut state = live.state.lock().await;
                // A stop, a pause or its session's message took it meanwhile.
                if task.state.is_final()
                    || super::explicitly_paused(&task)
                    || state.stopping
                    || state.busy
                {
                    return;
                }
                state
                    .run_origin
                    .get_or_insert_with(|| task.route.choice.clone());
                let failed = self.tried_model(&task.route.choice);
                if !state.failed_models.contains(&failed) {
                    state.failed_models.push(failed);
                }
                state.failed_models.clone()
            };
            let Some(target) = self.next_worker_model(&task, &tried).await else {
                drop((rerouting, settled));
                return self
                    .nothing_takes_worker(live, &task, failure, &tried)
                    .await;
            };
            let mut from = task.route.choice.clone();
            from.account = Some(match live.cli().await {
                Some(cli) => cli.account.choice_id(),
                None => self.runtime.account_for(&from).choice_id(),
            });
            let intent = Transition {
                id: uuid::Uuid::new_v4().to_string(),
                from,
                to: target,
                cause: ProviderTransitionCause::AutomaticFallback,
                wait_tries: 0,
            };
            let closing = failure.clone();
            let closed = self
                .update_task(&task.conversation_id, &task.id, |task| {
                    fallback::end_attempt(task, Some(closing));
                    task.error = None;
                })
                .await;
            if let Err(err) = closed {
                tracing::warn!(task = %task.id, error = %err, "could not record a failed model");
            }
            let why = "its model could not serve it (at capacity or failing); continue from the \
                       worker's records and check git before repeating work";
            let moved = self.move_worker(live, &task, &intent, why, false).await;
            drop((rerouting, settled));
            let task = match moved {
                Ok(Some(task)) => task,
                Ok(None) => return,
                Err(err) => {
                    tracing::warn!(task = %task.id, error = %err, "could not move a worker off its failed model");
                    return self
                        .nothing_takes_worker(live, &task, failure, &tried)
                        .await;
                }
            };
            // Not `message_worker`: its quota cancel would take the intent its launch confirms.
            let Err(err) = self
                .send_worker_text(&task.conversation_id, &task, super::RESUME_WORDS.into())
                .await
            else {
                return;
            };
            // That one could not start either: the next goes on.
            tracing::info!(task = %task.id, error = %err, "the model a worker moved to could not start");
            failure = AttemptEnd::Error {
                kind: brigadier_providers::ErrorKind::Process,
                message: err.to_string(),
            };
        }
    }

    /// The first model the worker may move to that its run hasn't seen fail, on an account
    /// that can take it.
    async fn next_worker_model(
        &self,
        task: &Task,
        tried: &[fallback::TriedModel],
    ) -> Option<ModelChoice> {
        let session = self.last_worker_native_id(task).await;
        for mut choice in self.worker_move_choices(task).await {
            if tried.contains(&self.tried_model(&choice)) {
                continue;
            }
            let model = self
                .runs_as(choice.provider, choice.model.as_deref(), None)
                .0;
            let account = crate::accounts::ready(
                &self.core.settings(),
                choice.provider,
                model.as_deref(),
                &self
                    .runtime
                    .resume_candidates(choice.provider, session.as_deref()),
                &[],
                now_ms(),
            );
            if let Some(account) = account {
                choice.account = Some(account.choice_id());
                return Some(choice);
            }
        }
        None
    }

    /// Nothing can take the worker: its run is over, it goes back to the model the run started
    /// on (its next message tries that first), and its turn ends as a failed one does, saying
    /// so.
    async fn nothing_takes_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        failure: AttemptEnd,
        tried: &[fallback::TriedModel],
    ) {
        let AttemptEnd::Error { kind, message } = failure.clone() else {
            return;
        };
        let origin = live.state.lock().await.run_origin.clone();
        live.end_model_run().await;
        // A stop or a pause took it meanwhile: its end is theirs.
        let Some((task, back_on)) = self.back_to_run_origin(live, task, failure, origin).await
        else {
            return;
        };
        live.set_cutoff(AttemptEnd::Error {
            kind,
            message: fallback::nothing_takes_it(&message, tried, back_on.as_ref()),
        })
        .await;
        self.plain_turn_ended(live, &task, TurnStatus::Failed).await;
    }

    /// The worker's route goes back to `origin`, with its row in its session's thread now, as
    /// nothing launches until its next message. `None`: a stop or a pause took it meanwhile.
    /// The model is `None` when it stayed: on `origin` already, or the move could not be
    /// recorded.
    async fn back_to_run_origin(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        failure: AttemptEnd,
        origin: Option<ModelChoice>,
    ) -> Option<(Task, Option<ModelChoice>)> {
        let _settled = live.settle.lock().await;
        let _rerouting = live.reroute.lock().await;
        let task = self
            .task_by_id(&task.conversation_id, &task.id)
            .await
            .ok()?;
        if task.state.is_final() || super::explicitly_paused(&task) {
            return None;
        }
        let origin = origin.filter(|origin| !fallback::same_model(origin, &task.route.choice));
        let Some(origin) = origin else {
            return Some((task, None));
        };
        let account = self.runtime.account_for(&origin);
        let mut from = task.route.choice.clone();
        from.account = Some(match live.cli().await {
            Some(cli) => cli.account.choice_id(),
            None => self.runtime.account_for(&from).choice_id(),
        });
        let intent = Transition {
            id: uuid::Uuid::new_v4().to_string(),
            from,
            to: origin.clone(),
            cause: ProviderTransitionCause::AutomaticFallback,
            wait_tries: 0,
        };
        let closed = self
            .update_task(&task.conversation_id, &task.id, |task| {
                fallback::end_attempt(task, Some(failure));
            })
            .await;
        if let Err(err) = closed {
            tracing::warn!(task = %task.id, error = %err, "could not record a failed model");
        }
        let why = "nothing else could serve it, so it went back to the model it started on; \
                   continue from the worker's records and check git before repeating work";
        let task = match self.move_worker(live, &task, &intent, why, false).await {
            Ok(Some(task)) => task,
            Ok(None) => return None,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not move a worker back to its model");
                return Some((task, None));
            }
        };
        self.file_worker_move_now(&task, intent, origin.clone(), &account)
            .await;
        Some((task, Some(origin)))
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

    /// The user's message gives the worker a turn, so that turn replaces its quota wait (#749):
    /// the wait, to [`Self::put_back_quota_wait`] if the message doesn't get through. A turn
    /// still running (ending at its limit, or a relaunch's) leaves the wait to its end.
    pub(in crate::manager) async fn message_takes_quota_wait(
        &self,
        live: &TaskLive,
        task: &Task,
    ) -> Result<Option<Box<QuotaWait>>> {
        let Some(wait) = task.quota_wait.clone() else {
            return Ok(None);
        };
        if live.busy().await {
            return Ok(None);
        }
        self.cancel_worker_quota(live, task).await?;
        live.state.lock().await.quota_cancelled = false;
        Ok(Some(wait))
    }

    /// The message that took `wait` didn't reach the worker: it goes on waiting, as before.
    pub(in crate::manager) async fn put_back_quota_wait(&self, task: &Task, wait: Box<QuotaWait>) {
        let path = self.worker_record(task).join("quota-cancelled.json");
        let _ = blocking(move || match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(Error::Invalid(err.to_string())),
        })
        .await;
        let restored = self
            .update_task_if(
                &task.conversation_id,
                &task.id,
                |task| !task.state.is_final() && task.quota_wait.is_none(),
                |task| task.quota_wait = Some(wait),
            )
            .await;
        match restored {
            Ok(Some(task)) => self.arm_worker_quota_retry(&task).await,
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not keep a worker's quota wait");
            }
        }
    }

    /// The choices of a `Work`'s routing rows, in its order.
    fn work_choices(&self, work: &str) -> Vec<ModelChoice> {
        self.work_rows(work)
            .unwrap_or_default()
            .into_iter()
            .map(|row| ModelChoice {
                provider: row.agent,
                model: (!row.model.eq_ignore_ascii_case("default"))
                    .then(|| self.listed_model(row.agent, &row.model)),
                effort: row.effort,
                fast: None,
                account: None,
            })
            .collect()
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
                Some(work) => self.work_choices(work),
                None => self.fallback_rows(),
            };
            choices.extend(rows);
        }
        self.allowed_worker_choices(task, choices).await
    }

    /// Where a worker can move off its model: its own model (another account of it), the next
    /// rows of its Work, then the Fallback rows. Another agent's rows need "another agent when
    /// all are out" on; its pins and its rules hold either way. A legacy worker, with no policy
    /// kept, stays on its own.
    async fn worker_move_choices(&self, task: &Task) -> Vec<ModelChoice> {
        let settings = self.core.settings();
        let mut own = task.route.choice.clone();
        own.account = None;
        let mut rows = Vec::new();
        if let Some(policy) = task
            .plain
            .as_ref()
            .and_then(|plain| plain.quota_policy.as_ref())
        {
            if let Some(work) = policy.work.as_deref() {
                rows = self.after_row_of(self.work_choices(work), &own);
            }
            rows.extend(self.after_row_of(self.fallback_rows(), &own));
            rows.retain(|row| row.provider == own.provider || settings.switch_providers);
        }
        let mut choices = vec![own];
        choices.extend(rows);
        self.allowed_worker_choices(task, choices).await
    }

    /// What of `choices` this worker may run on: its agents on, its pins, its rules, its grants.
    async fn allowed_worker_choices(
        &self,
        task: &Task,
        choices: Vec<ModelChoice>,
    ) -> Vec<ModelChoice> {
        let settings = self.core.settings();
        let policy = task
            .plain
            .as_ref()
            .and_then(|plain| plain.quota_policy.as_ref());
        let mut allowed = Vec::new();
        for choice in choices {
            let work = policy.and_then(|p| p.work.as_deref());
            let moving = choice.provider != task.route.choice.provider;
            let Ok(choice) = self.ruled_quota_choice(choice, work, moving) else {
                continue;
            };
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
                        || pin.effort.as_ref().is_some_and(|effort| {
                            let mut pinned = choice.clone();
                            pinned.effort = Some(effort.clone());
                            self.ruled_quota_choice(pinned, work, moving)
                                .map_or(true, |pinned| pinned.effort != choice.effort)
                        })
                })
            {
                continue;
            }
            if policy.is_some_and(|policy| policy.avoid_providers.contains(&choice.provider)) {
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
            // A session asleep still has its wait: it wakes below if the worker can go on (#750).
            let Some(lifecycle) = self
                .core
                .conversation(&task.conversation_id)
                .ok()
                .map(|conv| conv.lifecycle)
                .filter(|lifecycle| *lifecycle != Lifecycle::Archived)
            else {
                return;
            };
            let cancelled_at = self.worker_record(&task).join("quota-cancelled.json");
            if std::fs::read_to_string(cancelled_at)
                .ok()
                .and_then(|text| text.parse::<i64>().ok())
                == Some(wait.since_ms)
            {
                return;
            }
            let choices = self.worker_quota_choices(&task).await;
            let session = self.last_worker_native_id(&task).await;
            let mut target = None;
            for choice in &choices {
                // A `default` row's quota is its default model's (#678).
                let model = self
                    .runs_as(choice.provider, choice.model.as_deref(), None)
                    .0;
                let account = crate::accounts::ready(
                    &self.core.settings(),
                    choice.provider,
                    model.as_deref(),
                    &self
                        .runtime
                        .resume_candidates(choice.provider, session.as_deref()),
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
                        let model = self
                            .runs_as(choice.provider, choice.model.as_deref(), None)
                            .0;
                        self.runtime
                            .account_candidates(choice.provider)
                            .into_iter()
                            .filter(|candidate| candidate.logged_in)
                            .filter_map(move |candidate| {
                                candidate.quota.as_ref().and_then(|quota| {
                                    crate::accounts::ready_again_at(quota, model.as_deref())
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
            if lifecycle == Lifecycle::Hibernated {
                self.wake_for_worker(&task).await;
            }
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
            let why = "its provider hit a usage limit; continue from the worker's records and check git before repeating work";
            let changed_provider = intent.to.provider != task.route.choice.provider;
            let Ok(Some(task)) = self.move_worker(&live, &task, &intent, why, false).await else {
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

    /// Its worker goes on, so the session that slept while it waited is awake again.
    async fn wake_for_worker(&self, task: &Task) {
        let id = &task.conversation_id;
        match self.core.set_lifecycle(id.clone(), Lifecycle::Active).await {
            Ok(_) => {
                self.set_run(id, crate::work::RunState::Idle, None).await;
            }
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "could not wake the session of a worker going on");
            }
        }
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
            if conversation.lifecycle == Lifecycle::Archived {
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
            self.file_worker_transition(&task, intent.clone(), cli.model.clone(), &cli.account)
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

    /// Nothing launches now: its move's row goes in now, and no launch confirms it again.
    async fn file_worker_move_now(
        &self,
        task: &Task,
        intent: Transition,
        to: ModelChoice,
        account: &AccountRef,
    ) {
        let path = self.worker_record(task).join("quota-transition.json");
        let _ = blocking(move || {
            std::fs::remove_file(path).or_else(|err| match err.kind() {
                std::io::ErrorKind::NotFound => Ok(()),
                _ => Err(Error::Invalid(err.to_string())),
            })
        })
        .await;
        self.file_worker_transition(task, intent, to, account).await;
    }

    /// The one switch row of a worker's move (goal 1), in its session's thread.
    async fn file_worker_transition(
        &self,
        task: &Task,
        intent: Transition,
        mut to: ModelChoice,
        account: &AccountRef,
    ) {
        to.account = Some(account.choice_id());
        self.orchestrator_step_in(
            &task.conversation_id,
            task.request_id.clone(),
            OrchestratorStepKind::ProviderTransition {
                task_id: Some(task.id.clone()),
                id: intent.id,
                cause: intent.cause,
                from: intent.from,
                to,
                account: account.clone(),
                account_label: self.runtime.account_label(account),
                started_at_ms: now_ms(),
            },
        )
        .await;
    }

    /// Goal 1's move of a worker to `intent.to`: the intent its next launch confirms as one
    /// switch row, its CLI closed, its seam closed when the CLI changes (`why` tells its
    /// successor), its route on the target. `pins`: its session chose it, so the pins it was
    /// started with follow it. `None` when a stop took it meanwhile.
    async fn move_worker(
        &self,
        live: &Arc<TaskLive>,
        task: &Task,
        intent: &Transition,
        why: &str,
        pins: bool,
    ) -> Result<Option<Task>> {
        let path = self.worker_record(task).join("quota-transition.json");
        let bytes = serde_json::to_vec(intent).map_err(|err| Error::Invalid(err.to_string()))?;
        blocking(move || write_atomic(&path, &bytes)).await?;
        let predecessor = live
            .cli()
            .await
            .map(|cli| cli.session.native_id())
            .or(task.native_session.clone());
        self.note_worker_cut(&task.id);
        live.close_cli().await;
        live.allow_revival().await;
        if live.state.lock().await.quota_cancelled {
            return Ok(None);
        }
        let target = intent.to.clone();
        if target.provider != task.route.choice.provider {
            let (record, why) = (self.worker_record(task), why.to_owned());
            blocking(move || {
                let mut seam = read_seam(&record);
                seam.phase = Phase::Closed {
                    end: SeamEnd {
                        predecessor,
                        hot_tokens: None,
                        nudged_at_ms: None,
                        closed_at_ms: now_ms(),
                        death: Some(why),
                        stopped: false,
                        shown: Vec::new(),
                        moved_to: None,
                    },
                };
                write_seam(&record, &seam)
            })
            .await?;
        }
        let follow = |pin: &mut brigadier_router::Pin| {
            if pin.provider.is_some() {
                pin.provider = Some(target.provider);
            }
            if pin.model.is_some() {
                pin.model.clone_from(&target.model);
            }
            if pin.effort.is_some() {
                pin.effort.clone_from(&target.effort);
            }
        };
        let task = self
            .update_task(&task.conversation_id, &task.id, |task| {
                if pins {
                    task.pin.as_mut().map(follow);
                    if let Some(policy) = task.plain.as_mut().and_then(|p| p.quota_policy.as_mut())
                    {
                        follow(&mut policy.pin);
                        policy.avoid_providers.retain(|p| *p != target.provider);
                    }
                }
                fallback::end_attempt(task, None);
                task.route.choice = target.clone();
                task.attempts.push(Attempt {
                    route: task.route.clone(),
                    started_at_ms: now_ms(),
                    ended_at_ms: None,
                    end: None,
                });
            })
            .await?;
        Ok(Some(task))
    }

    /// Its session moves a worker that didn't switch on its own (`brigadier switch`): to
    /// `named`, else by the D82 order to the first choice another account can take, through
    /// goal 1's same move and its one "Switched to" row. One that was working or waiting for
    /// quota goes on there with "Continue where you stopped."; a paused one stays paused and an
    /// idle one idle (D7, D118), going on there once resumed or messaged.
    pub(in crate::manager) async fn switch_worker(
        &self,
        task: &Task,
        named: Option<ModelChoice>,
    ) -> Result<(ModelChoice, AccountRef)> {
        let live = self.task_live(task);
        let _managed = live.manage.lock().await;
        let was_cancelled = live.cancel_quota().await;
        let settled = live.settle.lock().await;
        let rerouting = live.reroute.lock().await;
        let task = self.task_by_id(&task.conversation_id, &task.id).await?;
        if task.state.is_final() {
            return Err(Error::Invalid("the worker has ended".into()));
        }
        if let Some(why) = super::unsettled(&live, &task).await {
            return Err(self.left_as_it_was(&live, &task, was_cancelled, why).await);
        }
        let wait = task.quota_wait.clone();
        let goes_on = !super::explicitly_paused(&task)
            && (wait.is_some()
                || matches!(
                    task.state,
                    TaskState::Running | TaskState::Starting | TaskState::Blocked
                ));
        let current = match live.cli().await {
            Some(cli) => cli.account.clone(),
            None => wait
                .as_ref()
                .and_then(|wait| wait.account.clone())
                .unwrap_or_else(|| self.runtime.account_for(&task.route.choice)),
        };
        let now = self.runs_as(
            task.route.choice.provider,
            task.route.choice.model.as_deref(),
            task.route.choice.effort.as_deref(),
        );
        let choices = match &named {
            Some(choice) => vec![choice.clone()],
            None => self.worker_move_choices(&task).await,
        };
        let session = self.last_worker_native_id(&task).await;
        let mut target = None;
        for choice in choices {
            let runs = self.runs_as(
                choice.provider,
                choice.model.as_deref(),
                choice.effort.as_deref(),
            );
            // Where it runs now is no switch: another account of it is.
            let here = choice.provider == current.provider && runs == now;
            let avoid = if here {
                vec![current.clone()]
            } else {
                Vec::new()
            };
            if let Some(account) = crate::accounts::ready(
                &self.core.settings(),
                choice.provider,
                runs.0.as_deref(),
                &self
                    .runtime
                    .resume_candidates(choice.provider, session.as_deref()),
                &avoid,
                now_ms(),
            ) {
                target = Some((choice, account));
                break;
            }
        }
        let Some((mut target, account)) = target else {
            return Err(self
                .left_as_it_was(
                    &live,
                    &task,
                    was_cancelled,
                    match &named {
                        Some(choice) => format!(
                            "no account of {} can take it now",
                            brigadier_providers::KindFacts::label(choice.provider)
                        ),
                        None => "no other account or routing row can take it now; name one with \
                         --agent, --model or --effort"
                            .into(),
                    },
                )
                .await);
        };
        target.account = Some(account.choice_id());
        self.cancel_worker_quota(&live, &task).await?;
        live.state.lock().await.quota_cancelled = false;
        // Its session chose where it runs: a new run starts there.
        live.end_model_run().await;
        let mut from = wait
            .as_ref()
            .and_then(|wait| wait.model.clone())
            .unwrap_or_else(|| task.route.choice.clone());
        from.account = Some(current.choice_id());
        let intent = Transition {
            id: uuid::Uuid::new_v4().to_string(),
            from,
            to: target.clone(),
            cause: ProviderTransitionCause::ManualSwitch,
            wait_tries: wait.as_ref().map_or(0, |wait| wait.tries),
        };
        let why = "its session moved it to another CLI; continue from the worker's records and check git before repeating work";
        let Some(task) = self.move_worker(&live, &task, &intent, why, true).await? else {
            return Err(Error::Invalid("the worker was stopped meanwhile".into()));
        };
        if !goes_on {
            self.file_worker_move_now(&task, intent, target.clone(), &account)
                .await;
            return Ok((target, account));
        }
        drop((rerouting, settled));
        // Not `message_worker`: its quota cancel would take the intent its launch confirms.
        if let Err(err) = self
            .send_worker_text(&task.conversation_id, &task, super::RESUME_WORDS.into())
            .await
        {
            // It waits for its session's next message there.
            let _ = self
                .update_task_if(
                    &task.conversation_id,
                    &task.id,
                    |task| !task.state.is_final(),
                    |task| {
                        task.state = TaskState::Reported;
                        task.error = Some(err.to_string());
                    },
                )
                .await;
            return Err(err);
        }
        Ok((target, account))
    }
}

#[cfg(test)]
mod rules_tests {
    use crate::manager::flow::{Flow, Options, Reply};
    use std::sync::Arc;

    #[tokio::test]
    async fn quota_moves_adjust_effort_and_obey_fallback_rules() {
        let flow = Flow::start(
            "quota-model-rules",
            Options::default(),
            Arc::new(|turn| {
                Box::pin(async move {
                    if turn.input.contains("Start a worker.") {
                        turn.start_worker("worker", "Reply once.").await;
                    }
                    Reply::text("Done.")
                })
            }),
        )
        .await;
        flow.say("Start a worker.").await;
        let board = flow.settled().await;
        let mut task = Flow::task(&board, 1).clone();
        task.route.choice.model = Some("opus".into());
        task.route.choice.effort = Some("high".into());
        // An effort that was pinned at start still yields to a newly set cap on a move.
        task.plain
            .as_mut()
            .unwrap()
            .quota_policy
            .as_mut()
            .unwrap()
            .pin
            .effort = Some("high".into());
        std::fs::write(
            flow.manager.models_rules_path(),
            "- effort-ceiling: medium\n",
        )
        .unwrap();
        let choices = flow.manager.worker_quota_choices(&task).await;
        assert!(!choices.is_empty(), "the cap refused the pinned worker");
        assert!(
            choices
                .iter()
                .all(|choice| choice.effort.as_deref() == Some("medium")),
            "effort cap bypassed: {choices:?}"
        );

        task.plain
            .as_mut()
            .unwrap()
            .quota_policy
            .as_mut()
            .unwrap()
            .pin
            .effort = None;
        std::fs::write(
            flow.manager.models_rules_path(),
            "- forbid for Fallback: *\n",
        )
        .unwrap();
        let choices = flow.manager.worker_quota_choices(&task).await;
        assert!(!choices.is_empty());
        assert!(
            choices
                .iter()
                .all(|choice| choice.provider == task.route.choice.provider),
            "fallback rule bypassed: {choices:?}"
        );
        // A named work used to bypass Fallback entirely.
        task.plain
            .as_mut()
            .unwrap()
            .quota_policy
            .as_mut()
            .unwrap()
            .work = Some("Docs".into());
        std::fs::write(flow.manager.models_rules_path(), "| Docs | claude | opus | high | Own |\n| Docs | codex | default | high | Other |\n- forbid for Fallback: *\n").unwrap();
        let choices = flow.manager.worker_quota_choices(&task).await;
        assert!(!choices.is_empty());
        assert!(
            choices
                .iter()
                .all(|choice| choice.provider == task.route.choice.provider),
            "named work bypassed Fallback: {choices:?}"
        );
        std::fs::write(flow.manager.models_rules_path(), "| Docs | claude | opus | high | Own |\n| Docs | codex | default | high | Other |\n- effort-ceiling for Docs: medium\n- effort-floor for Fallback: high\n").unwrap();
        let choices = flow.manager.worker_quota_choices(&task).await;
        assert!(!choices.is_empty());
        assert!(
            choices
                .iter()
                .all(|choice| choice.provider == task.route.choice.provider
                    && choice.effort.as_deref() == Some("medium")),
            "conflicting move bounds ignored: {choices:?}"
        );
        // Floors also adjust an already-running worker on its own agent.
        task.route.choice.effort = Some("medium".into());
        std::fs::write(
            flow.manager.models_rules_path(),
            "- effort-floor for Docs: high\n",
        )
        .unwrap();
        let choices = flow.manager.worker_quota_choices(&task).await;
        assert_eq!(choices[0].effort.as_deref(), Some("high"));
        flow.stop().await;
    }
}
