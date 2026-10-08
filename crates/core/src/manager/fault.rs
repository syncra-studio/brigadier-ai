//! Development builds only: a forced usage limit, to exercise fallback without spending real
//! quota. Nothing here is compiled into a release build.
//!
//! A limit is armed for one task's worker or one conversation's model. After the session's
//! next `after_tool_calls` finished tool calls, commands and file edits (0: its next event of
//! any kind), it fires:
//! 1. the quota monitor holds the provider at a limit until the chosen reset, so no read can
//!    lift it early;
//! 2. the session's transcript gets a notice saying a limit was injected;
//! 3. the CLI's running turn is interrupted, so its in-flight tool activity stops;
//! 4. when that turn ends, the session reports the limit (a rate-limit notification and a
//!    usage-limit error) just before the turn's end, as a CLI at its limit would.
//!
//! Nothing else is made up: the turn's own end is the CLI's, and everything after the error is
//! the real fallback path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_providers::{
    ErrorKind, ItemStatus, LimitHit, LimitKind, NoticeLevel, ProviderError, ProviderEvent,
    ProviderKind,
};

use super::SessionManager;
use super::conversation::Cli;
use crate::model::ConversationId;
use crate::work::TaskId;
use crate::{Error, Result, now_ms};

/// Whose session a fault is armed for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FaultKey {
    Task(TaskId),
    Conversation(ConversationId),
}

#[derive(Debug, Clone)]
struct Armed {
    provider: ProviderKind,
    limit: LimitHit,
    calls_left: u32,
    fired: bool,
}

#[derive(Default)]
pub(crate) struct Faults {
    armed: Mutex<HashMap<FaultKey, Armed>>,
    /// `BRIGADIER_FAULT` was applied (it arms one worker per daemon run).
    env_used: std::sync::atomic::AtomicBool,
}

/// `BRIGADIER_FAULT=<claude|codex>-limit[:tool-calls=N][:window=ID][:reset=MINUTES]`, for
/// scripted runs: arms a limit for the first worker the daemon starts on that provider.
fn env_fault() -> Option<(ProviderKind, String, u32, u32)> {
    let spec = std::env::var("BRIGADIER_FAULT").ok()?;
    let mut parts = spec.split(':');
    let provider = match parts.next()? {
        "claude-limit" => ProviderKind::Claude,
        "codex-limit" => ProviderKind::Codex,
        _ => return None,
    };
    let (mut window, mut reset, mut calls) = (
        match provider {
            ProviderKind::Claude => "five_hour".to_owned(),
            ProviderKind::Codex => "primary".to_owned(),
        },
        60,
        3,
    );
    for part in parts {
        match part.split_once('=')? {
            ("tool-calls", value) => calls = value.parse().ok()?,
            ("window", value) => value.clone_into(&mut window),
            ("reset", value) => reset = value.parse().ok()?,
            _ => return None,
        }
    }
    Some((provider, window, reset, calls))
}

impl SessionManager {
    /// Arms a usage limit for a task's worker or a conversation's model (see the module docs).
    pub async fn debug_inject_limit(
        &self,
        key: FaultKey,
        provider: ProviderKind,
        window: String,
        reset_in_minutes: u32,
        after_tool_calls: u32,
    ) -> Result<()> {
        let running = match &key {
            FaultKey::Task(id) => {
                let live = self.tasks_lock().get(id).cloned();
                match live {
                    Some(live) => live.cli().await.map(|cli| cli.provider),
                    None => None,
                }
            }
            FaultKey::Conversation(id) => {
                let live = self.convs_lock().get(id).cloned();
                match live {
                    Some(live) => live.cli().await.map(|cli| cli.provider),
                    None => None,
                }
            }
        };
        match running {
            None => {
                return Err(Error::Invalid(
                    "nothing is running there: a limit is injected into a running CLI session"
                        .into(),
                ));
            }
            Some(running) if running != provider => {
                return Err(Error::Invalid(format!(
                    "it runs on {}, not {}",
                    running.label(),
                    provider.label()
                )));
            }
            Some(_) => {}
        }
        let resets_at_ms = now_ms() + i64::from(reset_in_minutes.max(1)) * 60_000;
        tracing::info!(?key, %provider, window, resets_at_ms, after_tool_calls, "armed an injected usage limit");
        self.faults
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                key,
                Armed {
                    provider,
                    limit: LimitHit {
                        window: Some(window),
                        resets_at_ms: Some(resets_at_ms),
                        kind: LimitKind::UsageWindow,
                    },
                    calls_left: after_tool_calls,
                    fired: false,
                },
            );
        Ok(())
    }

    /// Arms `BRIGADIER_FAULT` for a worker that just started on `provider`, if it asks for one
    /// and none was armed yet.
    pub(crate) async fn arm_env_fault(&self, task: &TaskId, provider: ProviderKind) {
        let Some((wanted, window, reset, calls)) = env_fault() else {
            return;
        };
        if wanted != provider
            || self
                .faults
                .env_used
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        if let Err(err) = self
            .debug_inject_limit(FaultKey::Task(task.clone()), provider, window, reset, calls)
            .await
        {
            tracing::warn!(error = %err, "could not arm BRIGADIER_FAULT");
        }
    }

    /// Passes a session's event on, with what an armed fault adds around it.
    pub(crate) fn fault_events(
        &self,
        key: FaultKey,
        cli: &Arc<Cli>,
        event: ProviderEvent,
    ) -> Vec<ProviderEvent> {
        let mut armed = self
            .faults
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(fault) = armed.get_mut(&key) else {
            return vec![event];
        };
        if fault.provider != cli.provider {
            // A different CLI took over since it was armed.
            armed.remove(&key);
            return vec![event];
        }
        let ends = matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
        );
        let mut before = Vec::new();
        if !fault.fired {
            // Shell commands and file edits come as their own events, not as tool calls.
            if matches!(
                &event,
                ProviderEvent::ToolCall {
                    status: ItemStatus::Completed | ItemStatus::Failed,
                    ..
                } | ProviderEvent::Command {
                    status: ItemStatus::Completed | ItemStatus::Failed,
                    ..
                } | ProviderEvent::FileChanges {
                    status: ItemStatus::Completed | ItemStatus::Failed,
                    ..
                }
            ) {
                fault.calls_left = fault.calls_left.saturating_sub(1);
            }
            if fault.calls_left > 0 {
                return vec![event];
            }
            fault.fired = true;
            self.runtime
                .debug_limit(cli.account.clone(), fault.limit.clone());
            let session = cli.session.clone();
            self.spawn(async move {
                if let Err(err) = session.interrupt().await {
                    tracing::warn!(error = %err, "could not interrupt the session for an injected limit");
                }
            });
            let notice = ProviderEvent::Notice {
                level: NoticeLevel::Warning,
                message: format!(
                    "Development fault injection: {}'s {} window is treated as used up until \
                     the chosen reset; the running turn is being interrupted.",
                    fault.provider.label(),
                    fault.limit.window.as_deref().unwrap_or("usage")
                ),
            };
            if !ends {
                return vec![event, notice];
            }
            before.push(notice);
        }
        if !ends {
            return vec![event];
        }
        // The interrupted turn ended: report the limit just before its end.
        let Some(fault) = armed.remove(&key) else {
            return vec![event];
        };
        drop(armed);
        before.extend(self.limit_report(fault.provider, fault.limit));
        before.push(event);
        before
    }

    /// What a CLI at its limit reports: its quota with the limit, and a usage-limit error.
    fn limit_report(&self, provider: ProviderKind, limit: LimitHit) -> Vec<ProviderEvent> {
        let mut events = Vec::new();
        if let Some(quota) = self.runtime.monitor().current(provider, now_ms()) {
            events.push(ProviderEvent::RateLimits { quota });
        }
        events.push(ProviderEvent::Error {
            error: ProviderError {
                kind: ErrorKind::UsageLimit,
                message: format!(
                    "{}'s {} usage limit was reached (injected by a development fault).",
                    provider.label(),
                    limit.window.as_deref().unwrap_or("usage")
                ),
                will_retry: false,
                limit: Some(limit),
                code: Some("injected".into()),
            },
        });
        events
    }
}
