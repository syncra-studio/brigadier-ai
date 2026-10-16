//! Development builds only: a forced usage limit or provider error, to exercise fallback and the
//! continuity battery's seams (`tools/ab/v2`) without spending real quota. Nothing here is
//! compiled into a release build.
//!
//! A limit (or an error: a context window too small, a failed sign-in, credits run out) is
//! armed for one task's worker or one conversation's model. After the session's
//! next `after_tool_calls` finished tool calls, commands and file edits (0: its next event of
//! any kind), it fires:
//! 1. the quota monitor holds the provider at a limit until the chosen reset, so no read can
//!    lift it early;
//! 2. the session's transcript gets a notice saying a limit was injected;
//! 3. the CLI's running turn is interrupted, so its in-flight tool activity stops;
//! 4. when that turn ends, the session reports the limit (a rate-limit notification and a
//!    usage-limit error) just before the turn's end, as a CLI at its limit would. An injected
//!    error skips step 1 and reports its error instead.
//!
//! A planted `learned` line ([`SessionManager::debug_plant_learned`]) is added to the next
//! report a worker of the conversation submits: the battery's false claim, which must never
//! reach a later brief as fact.
//!
//! Nothing else is made up: the turn's own end is the CLI's, and everything after the error is
//! the real fallback path.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_providers::{
    ErrorKind, ItemStatus, LimitHit, LimitKind, NoticeLevel, ProviderError, ProviderEvent,
    ProviderKind, TurnStatus,
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

/// An error a development build can make a provider report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedError {
    /// "Prompt is too long": the context no longer fits the model's window.
    ContextWindow,
    /// The CLI's login failed.
    Auth,
    /// The account's credits ran out.
    Billing,
}

impl InjectedError {
    fn error(self, provider: ProviderKind) -> ProviderError {
        let (kind, message) = match self {
            Self::ContextWindow => (ErrorKind::ContextWindow, "Prompt is too long"),
            Self::Auth => (
                ErrorKind::Auth,
                "Invalid authentication credentials. Please sign in again.",
            ),
            Self::Billing => (ErrorKind::Billing, "Credit balance is too low"),
        };
        ProviderError {
            kind,
            message: format!(
                "{message} ({} error injected by a development fault).",
                provider.label()
            ),
            will_retry: false,
            limit: None,
            code: Some("injected".into()),
        }
    }
}

/// What an armed fault makes the session report.
#[derive(Debug, Clone)]
enum Injected {
    Limit(LimitHit),
    Error(InjectedError),
}

#[derive(Debug, Clone)]
struct Armed {
    provider: ProviderKind,
    what: Injected,
    calls_left: u32,
    fired: bool,
}

#[derive(Default)]
pub(crate) struct Faults {
    armed: Mutex<HashMap<FaultKey, Armed>>,
    /// `learned` lines for the next report of a worker of the conversation.
    planted: Mutex<HashMap<ConversationId, Vec<String>>>,
    /// `BRIGADIER_FAULT` was applied (it arms one worker per daemon run).
    env_used: std::sync::atomic::AtomicBool,
    /// Storing a kept output fails, as on a full disk (BUILD-PLAN D2.1).
    kept_writes_fail: std::sync::atomic::AtomicBool,
}

impl Faults {
    pub(crate) fn kept_writes_fail(&self) -> bool {
        self.kept_writes_fail
            .load(std::sync::atomic::Ordering::Relaxed)
    }
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
        self.running_on(&key, provider).await?;
        let resets_at_ms = now_ms() + i64::from(reset_in_minutes.max(1)) * 60_000;
        tracing::info!(?key, %provider, window, resets_at_ms, after_tool_calls, "armed an injected usage limit");
        self.arm(
            key,
            Armed {
                provider,
                what: Injected::Limit(LimitHit {
                    window: Some(window),
                    resets_at_ms: Some(resets_at_ms),
                    kind: LimitKind::UsageWindow,
                }),
                calls_left: after_tool_calls,
                fired: false,
            },
        );
        Ok(())
    }

    /// Arms a provider error for a task's worker or a conversation's model: after
    /// `after_tool_calls` finished tool calls its running turn is interrupted and ends with
    /// `error`, as a CLI that hit it would end it (see the module docs).
    pub async fn debug_inject_error(
        &self,
        key: FaultKey,
        provider: ProviderKind,
        error: InjectedError,
        after_tool_calls: u32,
    ) -> Result<()> {
        self.running_on(&key, provider).await?;
        tracing::info!(?key, %provider, ?error, after_tool_calls, "armed an injected provider error");
        self.arm(
            key,
            Armed {
                provider,
                what: Injected::Error(error),
                calls_left: after_tool_calls,
                fired: false,
            },
        );
        Ok(())
    }

    /// Makes storing a kept output fail (`on`), as on a full disk, or work again.
    pub fn debug_fail_kept_writes(&self, on: bool) {
        self.faults
            .kept_writes_fail
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// Adds `text` to the `learned` items of the next report a worker of `conversation`
    /// submits.
    pub fn debug_plant_learned(&self, conversation: ConversationId, text: String) {
        tracing::info!(%conversation, "planted a learned line for the next worker report");
        self.faults
            .planted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(conversation)
            .or_default()
            .push(text);
    }

    /// The `learned` lines planted for a report of a worker of `conversation`, taken.
    pub(crate) fn planted_learned(&self, conversation: &ConversationId) -> Vec<String> {
        self.faults
            .planted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(conversation)
            .unwrap_or_default()
    }

    /// Fails unless a CLI session on `provider` runs for `key`.
    async fn running_on(&self, key: &FaultKey, provider: ProviderKind) -> Result<()> {
        let running = match key {
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
            None => Err(Error::Invalid(
                "nothing is running there: a fault is injected into a running CLI session".into(),
            )),
            Some(running) if running != provider => Err(Error::Invalid(format!(
                "it runs on {}, not {}",
                running.label(),
                provider.label()
            ))),
            Some(_) => Ok(()),
        }
    }

    fn arm(&self, key: FaultKey, armed: Armed) {
        self.faults
            .armed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, armed);
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
            if let Injected::Limit(limit) = &fault.what {
                self.runtime.debug_limit(cli.account.clone(), limit.clone());
            }
            let session = cli.session.clone();
            self.spawn(async move {
                if let Err(err) = session.interrupt().await {
                    tracing::warn!(error = %err, "could not interrupt the session for an injected limit");
                }
            });
            let message = match &fault.what {
                Injected::Limit(limit) => format!(
                    "Development fault injection: {}'s {} window is treated as used up until \
                     the chosen reset; the running turn is being interrupted.",
                    fault.provider.label(),
                    limit.window.as_deref().unwrap_or("usage")
                ),
                Injected::Error(error) => format!(
                    "Development fault injection: {} reports {error:?}; the running turn is \
                     being interrupted.",
                    fault.provider.label()
                ),
            };
            let notice = ProviderEvent::Notice {
                level: NoticeLevel::Warning,
                message,
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
        match fault.what {
            Injected::Limit(limit) => {
                before.extend(self.limit_report(fault.provider, limit));
                before.push(event);
            }
            Injected::Error(error) => {
                before.push(ProviderEvent::Error {
                    error: error.error(fault.provider),
                });
                // A CLI that hit the error ends its turn failed, not stopped.
                before.push(match event {
                    ProviderEvent::TurnCompleted {
                        turn_id,
                        duration_ms,
                        usage,
                        ..
                    } => ProviderEvent::TurnCompleted {
                        turn_id,
                        status: TurnStatus::Failed,
                        duration_ms,
                        usage,
                    },
                    other => other,
                });
            }
        }
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
