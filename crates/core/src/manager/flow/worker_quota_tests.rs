//! Worker quota recovery keeps the worker identity/workspace, honors routing and owns its
//! own cancellation. The fake sends Error without RateLimits, matching real CLI failures.
use super::cli_tests::{brigadier, start};
use super::{Flow, Options, Reply};
use crate::work::{
    OrchestratorStepKind, ProviderTransitionCause, QuotaRecoveryMode, Task, TaskState,
};
use brigadier_cli::protocol::{Agent, CliRequest};
use brigadier_providers::{LimitHit, LimitKind, ProviderKind};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<(ProviderKind, Option<String>, String)>>>;

async fn flow(
    name: &str,
    pinned: bool,
    reset_ms: i64,
    failures: usize,
    account: bool,
    restriction: Option<&str>,
) -> (Flow, Seen) {
    let seen: Seen = Arc::default();
    let logged = seen.clone();
    let flow = Flow::start(
        name,
        Options::default(),
        Arc::new(move |turn| {
            let logged = logged.clone();
            Box::pin(async move {
                if turn.worker().is_none() {
                    if turn.input.contains("Start quota worker.") {
                        let mut request =
                            start("Keep the task identity and finish the existing work.");
                        request.title = Some("Quota fixture".into());
                        if pinned {
                            request.agent = Some(Agent::Claude);
                        }
                        brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                    }
                    return Reply::text("Heard.");
                }
                let count = {
                    let mut seen = logged.lock().unwrap();
                    seen.push((turn.provider, turn.account.clone(), turn.native_id.clone()));
                    seen.len()
                };
                if count <= failures {
                    return Reply {
                        limit: Some(LimitHit {
                            window: Some("five_hour".into()),
                            resets_at_ms: Some(crate::now_ms() + reset_ms),
                            kind: LimitKind::UsageWindow,
                        }),
                        ..Reply::default()
                    };
                }
                Reply::text("Finished the existing task.")
            })
        }),
    )
    .await;
    if account {
        flow.add_accounts(&[(ProviderKind::Claude, "acct-free")], true)
            .await;
    }
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    if restriction == Some("send-failed") {
        flow.behavior
            .refuse_sends
            .lock()
            .unwrap()
            .push(ProviderKind::Codex);
    }
    if restriction == Some("disabled") {
        settings.disabled_providers.push(ProviderKind::Codex);
    }
    if restriction == Some("rules") {
        std::fs::write(
            flow.manager.models_rules_path(),
            "# My rules\n\n- allow codex: gpt-7\n",
        )
        .unwrap();
    }
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Start quota worker.").await;
    flow.until("worker created", |board| !board.tasks.is_empty())
        .await;
    (flow, seen)
}

async fn worker(flow: &Flow) -> Task {
    Flow::task(&flow.board().await, 1).clone()
}

#[tokio::test]
async fn an_error_only_limit_updates_the_account_and_moves_the_same_worker() {
    let (flow, seen) = flow("worker-quota-account", false, 60_000, 1, true, None).await;
    flow.until("worker recovered", |board| {
        Flow::task(board, 1).quota_wait.is_none()
            && Flow::task(board, 1).state == TaskState::Reported
            && board.orchestrator_steps.iter().any(|step| {
                matches!(
                    &step.kind,
                    OrchestratorStepKind::ProviderTransition {
                        task_id: Some(_),
                        cause: ProviderTransitionCause::AutomaticFallback,
                        ..
                    }
                )
            })
    })
    .await;
    let task = worker(&flow).await;
    let turns = seen.lock().unwrap().clone();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].0, turns[1].0);
    assert_ne!(turns[0].1, turns[1].1);
    assert_eq!(
        turns[0].2, turns[1].2,
        "same-provider account changes resume native history"
    );
    let exhausted = crate::accounts::AccountRef::new(turns[0].0, turns[0].1.clone());
    assert!(
        flow.manager
            .runtime
            .monitor()
            .current_for(&exhausted, crate::now_ms())
            .unwrap()
            .limit
            .is_some()
    );
    assert!(task.quota_wait.is_none());
    assert!(task.workspace.is_some());
    flow.stop().await;
}

#[tokio::test]
async fn eligible_cross_provider_recovery_is_scoped_to_the_worker() {
    let (flow, seen) = flow("worker-quota-provider", false, 60_000, 1, false, None).await;
    flow.until("worker on Codex", |board| Flow::task(board, 1).quota_wait.is_none()
        && Flow::task(board, 1).state == TaskState::Reported
        && board.orchestrator_steps.iter().any(|step| matches!(&step.kind,
        OrchestratorStepKind::ProviderTransition { task_id: Some(_), to, account, account_label, .. }
        if to.provider == ProviderKind::Codex && account.provider == ProviderKind::Codex && !account_label.is_empty()))).await;
    let task = worker(&flow).await;
    let turns = seen.lock().unwrap().clone();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1].0, ProviderKind::Codex);
    assert_ne!(
        turns[0].2, turns[1].2,
        "another provider starts fresh context"
    );
    assert_eq!(task.route.choice.provider, ProviderKind::Codex);
    assert!(task.quota_wait.is_none());
    let board = flow.board().await;
    let transitions: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::ProviderTransition { task_id, .. } => Some(task_id),
            _ => None,
        })
        .collect();
    assert_eq!(transitions, [&Some(task.id)]);
    flow.stop().await;
}

#[tokio::test]
async fn a_hard_provider_pin_waits_durably_and_stop_cancels_recovery() {
    let (mut flow, seen) = flow("worker-quota-pin", true, 60_000, usize::MAX, false, None).await;
    flow.until("worker quota wait", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    let task = worker(&flow).await;
    let wait = task.quota_wait.as_ref().unwrap();
    assert_eq!(
        wait.recovery.as_ref().unwrap().mode,
        QuotaRecoveryMode::AutomaticAt
    );
    assert_eq!(task.state, TaskState::Paused);
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .all(|turn| turn.0 == ProviderKind::Claude)
    );
    flow.crash().await;
    flow.until("restored worker wait", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    let restored = worker(&flow).await;
    assert_eq!(task.workspace, restored.workspace);
    flow.manager.stop_task(task.id.clone()).await.unwrap();
    let before = seen.lock().unwrap().len();
    flow.manager.retry_waiting_workers().await;
    assert_eq!(seen.lock().unwrap().len(), before);
    let stopped = worker(&flow).await;
    assert!(stopped.state.is_final());
    assert!(stopped.quota_wait.is_none());
    flow.stop().await;
}

#[tokio::test]
async fn a_worker_reset_resumes_native_history_and_confirms_once() {
    let (flow, seen) = flow("worker-quota-reset", true, 60_000, 1, false, None).await;
    flow.until("worker quota wait", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    let task = worker(&flow).await;
    // Observe the durable wait before resetting its quota. A 300 ms reset could pass
    // while the fixture was still starting, so there was no wait left to observe.
    let account = task.quota_wait.as_ref().unwrap().account.as_ref().unwrap();
    let now = crate::now_ms();
    let monitor = flow.manager.runtime.monitor();
    let mut quota = monitor.current_for(account, now).unwrap();
    quota.limit.as_mut().unwrap().resets_at_ms = Some(now - 1);
    for window in &mut quota.windows {
        window.resets_at_ms = Some(now - 1);
    }
    quota.observed_at_ms = now;
    monitor.note(account, &quota, now);
    flow.manager.retry_waiting_workers().await;
    flow.until("reset confirmed", |board| {
        Flow::task(board, 1).quota_wait.is_none()
            && Flow::task(board, 1).state == TaskState::Reported
            && board.orchestrator_steps.iter().any(|step| {
                matches!(
                    &step.kind,
                    OrchestratorStepKind::ProviderTransition {
                        task_id: Some(_),
                        cause: ProviderTransitionCause::QuotaReset,
                        ..
                    }
                )
            })
    })
    .await;
    let turns = seen.lock().unwrap().clone();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0].2, turns[1].2);
    let resumed = worker(&flow).await;
    assert_eq!(task.workspace, resumed.workspace);
    assert!(resumed.quota_wait.is_none());
    flow.manager.retry_waiting_workers().await;
    assert_eq!(seen.lock().unwrap().len(), 2);
    flow.stop().await;
}

#[tokio::test]
async fn a_disabled_or_refused_provider_never_takes_a_worker() {
    for restriction in ["disabled", "rules"] {
        let (flow, seen) = flow(
            &format!("worker-quota-{restriction}"),
            false,
            60_000,
            1,
            false,
            Some(restriction),
        )
        .await;
        flow.until("restricted worker waits", |board| {
            Flow::task(board, 1).quota_wait.is_some()
        })
        .await;
        flow.manager.retry_waiting_workers().await;
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(seen.lock().unwrap()[0].0, ProviderKind::Claude);
        assert!(worker(&flow).await.quota_wait.is_some());
        flow.stop().await;
    }
}

#[tokio::test]
async fn repeated_worker_limits_keep_automatic_recovery_and_count_attempts() {
    let (flow, seen) = flow("worker-quota-repeated", true, 300, 5, false, None).await;
    for attempt in 1..=5 {
        flow.until("worker waiting again", |board| {
            Flow::task(board, 1).state == TaskState::Paused
                && Flow::task(board, 1).quota_wait.is_some()
                && seen.lock().unwrap().len() == attempt
        })
        .await;
        let task = worker(&flow).await;
        let wait = task.quota_wait.as_ref().unwrap();
        assert_eq!(wait.tries, (attempt - 1) as u32);
        assert_ne!(
            wait.recovery.as_ref().unwrap().mode,
            QuotaRecoveryMode::ManualOnly
        );
        assert!(wait.recovery.as_ref().unwrap().next_check_at_ms.unwrap() > crate::now_ms());
        let reset = wait.resets_at_ms.unwrap();
        flow.until("next worker reset", |_| crate::now_ms() > reset)
            .await;
        flow.manager.retry_waiting_workers().await;
    }
    flow.until("worker finished after five resets", |board| {
        Flow::task(board, 1).quota_wait.is_none() && seen.lock().unwrap().len() == 6
    })
    .await;
    flow.stop().await;
}

#[tokio::test]
async fn cancellation_storage_failure_is_reported_and_cannot_rearm_a_worker() {
    let (mut flow, seen) = flow(
        "worker-quota-cancel-io",
        true,
        60_000,
        usize::MAX,
        false,
        None,
    )
    .await;
    flow.until("worker waiting", |board| {
        Flow::task(board, 1).quota_wait.is_some()
    })
    .await;
    let task = worker(&flow).await;
    let path = flow
        .manager
        .session_dir(&flow.conversation)
        .worker(&task)
        .join("record/quota-transition.json");
    std::fs::create_dir(&path).unwrap();
    let error = flow.manager.pause_task(task.id.clone()).await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("could not cancel worker transition")
    );
    assert!(
        worker(&flow).await.quota_wait.is_none(),
        "cancellation committed before the failed intent cleanup"
    );
    std::fs::remove_dir(path).unwrap();
    flow.crash().await;
    let before = seen.lock().unwrap().len();
    flow.manager.retry_waiting_workers().await;
    assert_eq!(seen.lock().unwrap().len(), before);
    assert!(worker(&flow).await.quota_wait.is_none());
    flow.stop().await;
}

#[tokio::test]
async fn a_failed_worker_send_has_no_completed_transition() {
    let (flow, seen) = flow(
        "worker-quota-refused-send",
        false,
        60_000,
        1,
        false,
        Some("send-failed"),
    )
    .await;
    flow.until("failed worker launch is paused", |board| {
        let task = Flow::task(board, 1);
        task.route.choice.provider == ProviderKind::Codex
            && task.state == TaskState::Paused
            && task.error.is_some()
    })
    .await;
    let board = flow.board().await;
    assert!(
        board
            .orchestrator_steps
            .iter()
            .all(|step| !matches!(step.kind, OrchestratorStepKind::ProviderTransition { .. }))
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    let task = worker(&flow).await;
    assert!(
        task.quota_wait
            .as_ref()
            .unwrap()
            .recovery
            .as_ref()
            .unwrap()
            .next_check_at_ms
            .unwrap()
            > crate::now_ms()
    );
    flow.stop().await;
}
