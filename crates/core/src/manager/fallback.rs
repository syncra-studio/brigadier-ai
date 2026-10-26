//! What a CLI's error means for its work, and the waits for quota. A worker's usage limit
//! pauses it and its session hears it (`brigadier wait`); Brigadier moves no work to another
//! model (G18). Conversations waiting for a model look again whenever a provider's state is
//! recorded.

use std::time::Duration;

use brigadier_providers::{ErrorKind, LimitHit, LimitKind, ProviderError};

use super::SessionManager;
use crate::model::ModelChoice;
use crate::now_ms;
use crate::work::{Attempt, AttemptEnd, Task};

/// Work waiting for quota with no known reset looks again this often.
const WAIT_RETRY: Duration = Duration::from_secs(30 * 60);
/// A reset is past this long before the work looks again.
const AFTER_RESET: Duration = Duration::from_secs(45);
/// Up to this much more, at random, so the work waiting for one reset doesn't all go at once.
const RESET_SPREAD_MS: u64 = 60_000;
/// Provider checks this close together make one retry of the work waiting for quota.
const PROVIDER_CHECK_SETTLE: Duration = Duration::from_secs(2);

/// What a worker's error means for its task.
pub(crate) enum ErrorVerdict {
    /// Its turn ends early: its session hears why when the turn is over.
    HandOff(AttemptEnd),
    /// A transient error (overloaded, server, network).
    Transient,
    /// The task's error.
    Keep,
}

/// Classifies an error a worker's CLI gave up on.
pub(crate) fn verdict(error: &ProviderError) -> ErrorVerdict {
    if error.will_retry {
        return ErrorVerdict::Keep;
    }
    match error.kind {
        ErrorKind::UsageLimit => ErrorVerdict::HandOff(AttemptEnd::Limit {
            limit: error.limit.clone().unwrap_or(LimitHit {
                window: None,
                resets_at_ms: None,
                kind: LimitKind::UsageWindow,
            }),
        }),
        ErrorKind::Billing => ErrorVerdict::HandOff(AttemptEnd::Limit {
            limit: LimitHit {
                window: None,
                resets_at_ms: None,
                kind: LimitKind::Credits,
            },
        }),
        ErrorKind::Auth | ErrorKind::ContextWindow => ErrorVerdict::HandOff(AttemptEnd::Error {
            kind: error.kind,
            message: error.message.clone(),
        }),
        ErrorKind::Overloaded | ErrorKind::Server | ErrorKind::Network => ErrorVerdict::Transient,
        _ => ErrorVerdict::Keep,
    }
}

impl SessionManager {
    /// Whenever a provider's state is recorded (a login, a limit, a fresh usage read), work
    /// waiting for quota looks again. Checks that come close together count once.
    pub(super) fn retry_waiting_on_provider_checks(&self) {
        let mut checks = self.runtime.provider_checks();
        let manager = self.me.clone();
        self.spawn(async move {
            while checks.changed().await.is_ok() {
                tokio::time::sleep(PROVIDER_CHECK_SETTLE).await;
                checks.borrow_and_update();
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.retry_waiting_work().await;
            }
        });
    }

    /// Everything that waits for quota looks again, now: after the user changed their rules,
    /// or a provider's state changed (a login, a fresh usage read).
    pub async fn retry_waiting_work(&self) {
        self.retry_waiting_conversations().await;
    }
}

/// Persist the scheduler's promise. Repeated failures back off instead of silently
/// abandoning a subscription window's automatic recovery.
pub(super) fn recovery(
    reset: Option<i64>,
    kind: Option<LimitKind>,
    tries: u32,
) -> crate::work::QuotaRecovery {
    use crate::work::{QuotaRecovery, QuotaRecoveryMode as Mode};
    if kind.is_some_and(|kind| kind != LimitKind::UsageWindow) {
        return QuotaRecovery {
            mode: Mode::ManualOnly,
            next_check_at_ms: None,
        };
    }
    let future = reset.filter(|at| *at > now_ms());
    let delay = if future.is_none() && tries >= 4 {
        WAIT_RETRY
    } else {
        retry_delay(future)
    };
    QuotaRecovery {
        mode: if future.is_some() {
            Mode::AutomaticAt
        } else {
            Mode::AutomaticProbe
        },
        next_check_at_ms: Some(now_ms() + delay.as_millis() as i64),
    }
}

/// How long work waiting for quota sleeps before it looks again: until just after the reset
/// that could free it (spread at random over the minute after it), and at most [`WAIT_RETRY`].
pub(super) fn retry_delay(resets_at_ms: Option<i64>) -> Duration {
    match resets_at_ms {
        Some(at) => {
            let spread = getrandom::u64().unwrap_or(0) % RESET_SPREAD_MS;
            Duration::from_millis(u64::try_from(at - now_ms()).unwrap_or(0) + spread) + AFTER_RESET
        }
        None => WAIT_RETRY,
    }
    .min(WAIT_RETRY)
}

/// Closes the running attempt of `task` (or its first, never recorded one) with `end`. A task
/// that waited from the start and never began has none.
pub(crate) fn end_attempt(task: &mut Task, end: Option<AttemptEnd>) {
    let now = now_ms();
    if task.attempts.is_empty() && task.workspace.is_some() {
        task.attempts.push(Attempt {
            route: task.route.clone(),
            started_at_ms: task.created_at_ms,
            ended_at_ms: None,
            end: None,
        });
    }
    if let Some(last) = task.attempts.last_mut()
        && last.ended_at_ms.is_none()
    {
        last.ended_at_ms = Some(now);
        last.end = end;
    }
}

pub(crate) fn model_label(choice: &ModelChoice) -> String {
    match &choice.model {
        Some(model) => format!("{} {model}", choice.provider.label()),
        None => format!("{}'s default model", choice.provider.label()),
    }
}

/// Why an attempt ended, in a few words ("5-hour limit reached").
pub(crate) fn end_reason(end: &AttemptEnd) -> String {
    match end {
        AttemptEnd::Limit { limit } => match limit.kind {
            LimitKind::UsageWindow => match limit.window.as_deref() {
                Some(window) => format!("{} limit reached", window_name(window)),
                None => "usage limit reached".into(),
            },
            LimitKind::SpendControl => "stopped by a spend control".into(),
            LimitKind::Credits => "out of credits".into(),
        },
        AttemptEnd::Error { kind, .. } => match kind {
            ErrorKind::ContextWindow => "context window full".into(),
            ErrorKind::Auth => "logged out".into(),
            ErrorKind::Process => "its CLI exited".into(),
            ErrorKind::Stalled => "went silent".into(),
            _ => "kept failing".into(),
        },
    }
}

/// A window id as people say it.
fn window_name(id: &str) -> &str {
    match id {
        "five_hour" => "5-hour",
        "seven_day" => "weekly",
        "primary" => "primary",
        "secondary" => "secondary",
        other => other,
    }
}
