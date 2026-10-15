//! Rebirth when the prompt cache has expired (PLAN.md §7). An orchestrator's first turn after
//! its CLI's cache lifetime would resume by sending the whole history again at the cache-write
//! price. Instead, while the conversation idles and the cache is still warm, a fork writes a
//! checkpoint: the handoff note a rebirth needs. The checkpoint's fork reads the cache, which
//! keeps it warm for another lifetime. If the next turn comes after the cache has expired and
//! the checkpoint covers everything since the last model request, that turn is reborn from a
//! briefing with it (trigger `CacheExpired`).
//!
//! Without a current checkpoint the turn resumes as before, so nothing said since is lost. A
//! Codex orchestrator always resumes: how long Codex keeps its cache through the app-server is
//! not measured ([`cache_lifetime`]).

use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{ProviderEvent, ProviderKind};
use brigadier_store::StreamPage;

use super::conversation::ConvLive;
use super::rebirth::{BriefingPlan, HandoffPurpose};
use super::{SessionManager, prompts};
use crate::knowledge::{RebirthTrigger, cache_lifetime, cold_rebirth_min_tokens};
use crate::model::{ConversationKind, DomainEvent, Setup, streams};
use crate::now_ms;
use crate::work::OrchestratorEntry;

/// The checkpoint is written once the conversation has idled this share of the cache
/// lifetime: of the idle gaps that reached 50 minutes in measured sessions, 83% went past the
/// hour.
const CHECKPOINT_AT: f64 = 5.0 / 6.0;
/// A checkpoint's fork must start this long before the cache expires, to read it warm (a
/// 24th of a short debug lifetime).
const CHECKPOINT_MARGIN: Duration = Duration::from_secs(60);
/// Orchestrator log entries looked at to recover the last model request.
const RECOVER_SCAN: u32 = 500;

/// The CLI session an orchestrator last made a model request on, and when.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CacheMark {
    pub native_id: String,
    pub provider: ProviderKind,
    /// When its last model request finished.
    pub at_ms: i64,
    /// Its context then, and its model's window.
    pub context: i64,
    pub window: Option<i64>,
}

impl CacheMark {
    /// Idle long enough for a checkpoint, and still early enough to read the cache warm.
    fn checkpoint_due(&self, lifetime: Duration, now: i64) -> bool {
        let idle = now - self.at_ms;
        let from = (lifetime.as_millis() as f64 * CHECKPOINT_AT) as i64;
        let margin = CHECKPOINT_MARGIN.min(lifetime / 24);
        let until = lifetime.saturating_sub(margin).as_millis() as i64;
        idle >= from && idle < until
    }

    /// The cache has expired: `lifetime` has passed since the thread's last request. A
    /// checkpoint's fork doesn't extend it (BUILD-PLAN B5, research A11).
    fn expired(&self, lifetime: Duration, now: i64) -> bool {
        now - self.at_ms >= lifetime.as_millis() as i64
    }
}

/// How often idle orchestrators are checked for a checkpoint: a twelfth of the cache
/// lifetime, from 5 seconds (a debug build's short lifetime) to a minute.
pub(super) fn checkpoint_interval() -> Duration {
    cache_lifetime(ProviderKind::Claude)
        .map_or(Duration::from_secs(60), |lifetime| lifetime / 12)
        .clamp(Duration::from_secs(5), Duration::from_secs(60))
}

impl SessionManager {
    /// Checks idle orchestrators for a checkpoint on a timer.
    pub(super) fn start_checkpoint_timer(&self) {
        let manager = self.me.clone();
        self.spawn(async move {
            let mut tick = tokio::time::interval(checkpoint_interval());
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.checkpoint_idle().await;
            }
        });
    }

    /// Starts a checkpoint for every orchestrator that has idled long enough with a context
    /// large enough to be worth a rebirth, unless a current one exists.
    async fn checkpoint_idle(&self) {
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        let now = now_ms();
        for conv in convs {
            if conv.kind != ConversationKind::Session
                || conv.is_busy().await
                || conv.rebirth_pending().await
            {
                continue;
            }
            let Some(mark) = self.cache_mark_of(&conv).await else {
                continue;
            };
            let Some(lifetime) = cache_lifetime(mark.provider) else {
                continue;
            };
            if !mark.checkpoint_due(lifetime, now)
                || mark.context < cold_rebirth_min_tokens(mark.provider, mark.window)
            {
                continue;
            }
            if let Some(checkpoint) = conv.checkpoint().await
                && checkpoint.old_native_id.as_deref() == Some(mark.native_id.as_str())
                && checkpoint.started_at_ms >= mark.at_ms
            {
                continue;
            }
            let Ok(conversation) = self.core.conversation(&conv.id) else {
                continue;
            };
            let Some(Setup::Session { orchestrator, .. }) = &conversation.setup else {
                continue;
            };
            // The fork runs on the model the session runs on: another model has no cache to
            // read.
            let choice = match conversation.fallback.as_ref() {
                Some(fallback) => fallback.choice.clone(),
                None => orchestrator.clone(),
            };
            if choice.provider != mark.provider {
                continue;
            }
            tracing::info!(conversation = %conv.id, context = mark.context, idle_ms = now - mark.at_ms, "writing a checkpoint while the cache is warm");
            let checkpoint = self.prepare_rebirth(
                &conv.id,
                mark.provider,
                choice,
                Some(mark.native_id.clone()),
                (mark.context, mark.window),
                HandoffPurpose::Checkpoint,
            );
            conv.set_checkpoint(checkpoint).await;
        }
    }

    /// The rebirth a turn starting now gets because the cache has expired: when the
    /// orchestrator would resume the session of its last request, the context is worth it,
    /// and a checkpoint with a note covers everything since.
    pub(super) async fn cold_rebirth_plan(&self, conv: &Arc<ConvLive>) -> Option<BriefingPlan> {
        let mark = self.cache_mark_of(conv).await?;
        let lifetime = cache_lifetime(mark.provider)?;
        let now = now_ms();
        let checkpoint = conv.checkpoint().await.filter(|checkpoint| {
            checkpoint.ready()
                && checkpoint.old_native_id.as_deref() == Some(mark.native_id.as_str())
                && checkpoint.started_at_ms >= mark.at_ms
        });
        if !mark.expired(lifetime, now)
            || mark.context < cold_rebirth_min_tokens(mark.provider, mark.window)
        {
            return None;
        }
        let resumes = self.last_native_id(&conv.id, mark.provider).await;
        if resumes.as_deref() != Some(mark.native_id.as_str()) {
            return None;
        }
        let Some(checkpoint) = checkpoint else {
            tracing::info!(conversation = %conv.id, context = mark.context, "the cache has expired but no checkpoint covers the last turn; resuming");
            return None;
        };
        if checkpoint.note(Duration::ZERO).await.is_none() {
            tracing::info!(conversation = %conv.id, "the cache has expired but the checkpoint has no note; resuming");
            return None;
        }
        tracing::info!(conversation = %conv.id, context = mark.context, idle_ms = now - mark.at_ms, "the cache has expired: rebirth from the checkpoint");
        Some(BriefingPlan {
            trigger: RebirthTrigger::CacheExpired,
            at_tokens: mark.context,
            window: mark.window,
            prep: Some(checkpoint),
            swap_started_at_ms: now,
        })
    }

    /// Whether the conversation's last CLI session's cache has expired by now: a resume now is
    /// a cold one (BUILD-PLAN B0).
    pub(super) async fn cache_gone_cold(&self, conv: &Arc<ConvLive>) -> bool {
        let Some(mark) = self.cache_mark_of(conv).await else {
            return false;
        };
        cache_lifetime(mark.provider).is_some_and(|lifetime| mark.expired(lifetime, now_ms()))
    }

    /// The conversation's last model request: as seen live, or else recovered from the
    /// orchestrator log (after a restart).
    async fn cache_mark_of(&self, conv: &Arc<ConvLive>) -> Option<CacheMark> {
        if let Some(mark) = conv.cache_mark().await {
            return Some(mark);
        }
        let mark = self.recover_cache_mark(conv).await?;
        conv.set_cache_mark(mark.clone()).await;
        Some(mark)
    }

    /// The last model request of the conversation's newest CLI session, from its logged
    /// context sizes (each follows a model response).
    async fn recover_cache_mark(&self, conv: &ConvLive) -> Option<CacheMark> {
        let page = self
            .core
            .store()
            .read_stream(
                streams::orchestrator(&conv.id),
                StreamPage {
                    before: None,
                    kinds: vec!["orchestrator.logged".into()],
                    limit: RECOVER_SCAN,
                },
            )
            .await
            .ok()?;
        let mut last: Option<(i64, i64, Option<i64>)> = None;
        for stored in page {
            let Ok(DomainEvent::OrchestratorLogged {
                entry: OrchestratorEntry::Provider { provider, event },
                ..
            }) = serde_json::from_str::<DomainEvent>(stored.payload.get())
            else {
                continue;
            };
            match event {
                ProviderEvent::ContextSize {
                    used_tokens,
                    window_tokens,
                } => {
                    // Newest first: the first one seen is the last request.
                    last.get_or_insert((stored.at_ms, used_tokens, window_tokens));
                }
                ProviderEvent::Notice { message, .. } if message == prompts::SESSION_RESET => {
                    return None;
                }
                ProviderEvent::SessionStarted { native_id, .. } => {
                    let (at_ms, context, window) = last?;
                    return Some(CacheMark {
                        native_id,
                        provider,
                        at_ms,
                        context,
                        window,
                    });
                }
                _ => {}
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: i64 = 60_000;

    fn mark() -> CacheMark {
        CacheMark {
            native_id: "s".into(),
            provider: ProviderKind::Claude,
            at_ms: 0,
            context: 100_000,
            window: None,
        }
    }

    #[test]
    fn a_checkpoint_is_due_from_five_sixths_of_the_lifetime_until_a_minute_before_it_ends() {
        let hour = Duration::from_secs(3_600);
        let mark = mark();
        assert!(!mark.checkpoint_due(hour, 49 * MINUTE));
        assert!(mark.checkpoint_due(hour, 50 * MINUTE));
        assert!(mark.checkpoint_due(hour, 59 * MINUTE - 1));
        assert!(!mark.checkpoint_due(hour, 59 * MINUTE));
        // A short debug lifetime keeps a 24th of it as the margin.
        let short = Duration::from_secs(240);
        assert!(mark.checkpoint_due(short, 200_000));
        assert!(mark.checkpoint_due(short, 229_999));
        assert!(!mark.checkpoint_due(short, 230_000));
    }

    #[test]
    fn the_cache_expires_one_lifetime_after_the_threads_last_request() {
        let hour = Duration::from_secs(3_600);
        let mark = mark();
        assert!(!mark.expired(hour, 60 * MINUTE - 1));
        assert!(mark.expired(hour, 60 * MINUTE));
    }

    /// The checkpoint wait's test-only override (`BRIGADIER_CACHE_TTL_SECS`, read by a debug
    /// build alone) is off unless set: Claude's cache lives an hour.
    #[test]
    fn the_cache_lifetime_is_an_hour_unless_a_debug_build_is_told_otherwise() {
        if std::env::var_os("BRIGADIER_CACHE_TTL_SECS").is_none() {
            assert_eq!(
                cache_lifetime(ProviderKind::Claude),
                Some(Duration::from_secs(3_600))
            );
        }
        assert_eq!(cache_lifetime(ProviderKind::Codex), None);
    }
}
