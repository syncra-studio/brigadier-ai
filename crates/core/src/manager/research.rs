//! Live discovery (PLAN.md §6 Phase 5): a model a CLI lists that the registry doesn't know is
//! researched once, on spare quota, from its release notes, model card and benchmarks. The
//! finding is a dated [`ResearchNote`] in `routing.sqlite`; the router places the model with it
//! (within [`brigadier_router::RESEARCH_REACH`] of unrated, never above
//! [`brigadier_router::RESEARCH_MAX_TIER`]) until the registry curates it, and the model's
//! trial tasks and outcomes do the rest.
//!
//! **Who pays.** Only spare quota, as for Brain enrichment: the setting is on, the provider has
//! been idle for ten minutes and has a window that resets within the hour with plenty left
//! (or, with no such window, every window is under half used). It runs on that provider's
//! cheapest model at low effort, with web search and nothing else, read-only, time-boxed.
//! One research runs at a time, and a model whose research failed waits a day.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::Duration;

use brigadier_providers::{
    Access, ApprovalDecision, Origin as SessionOrigin, ProviderEvent, ProviderKind, Role,
    SessionSpec, Started, ToolSet, TurnInput, TurnStatus,
};
use brigadier_router::{MergedModel, QualityTier, ResearchNote, TaskCategory};
use serde::Deserialize;
use tokio_util::{sync::CancellationToken, task::TaskTracker};

use super::SessionManager;
use super::usage::TokenOwner;
use crate::routing::TokenMeter;
use crate::{Error, Result, now_ms};

/// A research session's time box.
const RESEARCH_TIME: Duration = Duration::from_secs(10 * 60);
/// A model whose research failed is tried again after this long.
const RETRY_AFTER_MS: i64 = 24 * 60 * 60 * 1000;
/// With no window resetting soon, a provider is spare when every window is under this.
const QUIET_MAX_USED: f64 = 50.0;
/// The finding is cut to this many sources and this long a summary.
const MAX_SOURCES: usize = 8;
const MAX_SUMMARY_CHARS: usize = 600;
/// A context window outside this range (in tokens) is left unknown, as the registry's are.
const CONTEXT_WINDOWS: std::ops::RangeInclusive<i64> = 8_000..=20_000_000;

const RESEARCH_ROLE: &str = "You research AI models for Brigadier, an app that routes coding tasks between AI models. Use web search to read the model's official release notes or model card and reputable benchmark results. Don't guess: leave out what you can't find. Change nothing on this computer. Your final message is exactly one JSON object and nothing else.";

/// Research in progress and past failures.
#[derive(Default)]
pub(crate) struct Research {
    running: Mutex<Option<(ProviderKind, String)>>,
    pub(super) stop: CancellationToken,
    pub(super) jobs: TaskTracker,
    failed: Mutex<HashMap<(ProviderKind, String), i64>>,
}

/// The research session's answer.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    summary: String,
    tier: QualityTier,
    #[serde(default)]
    strengths: BTreeMap<String, f64>,
    #[serde(default)]
    context_window: Option<i64>,
    #[serde(default)]
    knowledge_cutoff: Option<String>,
    #[serde(default)]
    sources: Vec<String>,
}

impl SessionManager {
    /// Each minute: researches one unknown model when a provider has quota to spare.
    pub(super) async fn research_tick(&self) {
        // Without the store a finding couldn't be kept, and the model would be researched again.
        if !self.core.settings().enrich_brain
            || self.runtime.routing_store().is_none()
            || self.admit().is_err()
            || self.brains.jobs.running()
            || self
                .research
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_some()
        {
            return;
        }
        let now = now_ms();
        let Some(provider) = ProviderKind::ALL
            .into_iter()
            .find(|kind| self.spare_for_research(*kind, now))
        else {
            return;
        };
        let failed = self
            .research
            .failed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let Some((model_provider, model)) = self
            .unresearched_models()
            .await
            .into_iter()
            .find(|key| failed.get(key).is_none_or(|at| now - at >= RETRY_AFTER_MS))
        else {
            return;
        };
        *self
            .research
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some((model_provider, model.clone()));
        let manager = self.arc();
        let turn = self.background_turn();
        self.spawn(self.research.jobs.track_future(async move {
            let _turn = turn;
            tracing::info!(model = %model, runs_on = %provider, "researching a new model on spare quota");
            let researched = manager.research_model(provider, model_provider, &model).await;
            let key = (model_provider, model.clone());
            let stored = match (researched, manager.runtime.routing_store()) {
                (Ok(note), Some(store)) => store.put_research(note).await,
                (Ok(_), None) => Err(Error::Invalid("the routing store is gone".into())),
                (Err(err), _) => Err(err),
            };
            // A finding that isn't kept counts as a failure: the model waits a day.
            if let Err(err) = stored {
                tracing::info!(model = %model, error = %err, "model research failed");
                manager
                    .research
                    .failed
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .insert(key, now_ms());
            }
            *manager
                .research
                .running
                .lock()
                .unwrap_or_else(|p| p.into_inner()) = None;
        }));
    }

    /// Models the CLIs list that neither the registry nor research has placed yet (and the
    /// user hasn't made unavailable).
    async fn unresearched_models(&self) -> Vec<(ProviderKind, String)> {
        let settings = self.core.settings();
        self.routing_inputs(None, now_ms())
            .await
            .models
            .into_iter()
            .filter(|model| {
                model.status == brigadier_router::ModelStatus::Unknown
                    && model.rating_provenance
                        != brigadier_router::RatingProvenance::ResearchedOverlay
                    && !model.excluded
                    && crate::routing::availability::model_available(
                        &settings,
                        model.provider,
                        &model.id,
                    )
            })
            .map(|model| (model.provider, model.id))
            .collect()
    }

    /// Whether `kind` may spend quota on research now (see the module docs).
    fn spare_for_research(&self, kind: ProviderKind, now: i64) -> bool {
        if self.provider_idle_ms(kind, now) < super::brain_jobs::ENRICH_IDLE_MS
            || !self.provider_usable(kind)
            || self.cheapest(kind).is_err()
        {
            return false;
        }
        let Some(quota) = self
            .runtime
            .overview(kind)
            .and_then(|overview| overview.quota)
        else {
            return false;
        };
        super::brain_jobs::spare_window(&quota, now).is_some()
            || (quota.limit.is_none()
                && !quota.windows.is_empty()
                && quota
                    .windows
                    .iter()
                    .all(|window| window.used_percent < QUIET_MAX_USED))
    }

    /// Researches `model` (listed by `of`'s CLI) in a session on `runs_on`.
    async fn research_model(
        &self,
        runs_on: ProviderKind,
        of: ProviderKind,
        model: &str,
    ) -> Result<ResearchNote> {
        let vendor = match of {
            ProviderKind::Claude => "Anthropic",
            ProviderKind::Codex => "OpenAI",
        };
        let prompt = format!(
            "Research the model `{model}` from {vendor}, as its {cli} CLI lists it. Reply with \
             one JSON object: {{\"summary\": \"two or three sentences: what it is, when it was \
             released, what it is good at\", \"tier\": \"frontier\" | \"strong\" | \"standard\" \
             | \"light\" (frontier: the vendor's best; strong: a strong coding model; standard: \
             an everyday model; light: small and fast), \"strengths\": {{\"scout\", \
             \"research\", \"implement\", \"review\", \"merge\", \"verify\", \"chat\", \
             \"orchestrate\": 0 to 10 each}}, \"contextWindow\": tokens or null, \
             \"knowledgeCutoff\": \"YYYY-MM\" or null, \"sources\": [the URLs you read]}}.",
            cli = of.label()
        );
        let (cheap, effort) = self.cheapest(runs_on)?;
        let reply = self
            .run_web_research(WebResearch {
                provider: runs_on,
                model: cheap,
                effort: effort.or_else(|| Some("low".into())),
                prompt,
                cancel: self.research.stop.clone(),
                time: RESEARCH_TIME,
            })
            .await?;
        note_from(of, model, &reply)
    }

    async fn run_web_research(&self, request: WebResearch) -> Result<String> {
        let WebResearch {
            provider: runs_on,
            model: cheap,
            effort,
            prompt,
            cancel,
            time,
        } = request;
        let id = uuid::Uuid::now_v7().to_string();
        let owner = format!("research:{id}");
        let scratch = self.owned_dir("scratch", &format!("research-{id}"));
        if let Err(error) = self.prepare_owned_dir(&owner, &scratch).await {
            let _ = self.runtime.ledger().dispose(&owner).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(error);
        }
        let spec = SessionSpec {
            cwd: scratch.clone(),
            model: Some(cheap.clone()),
            effort,
            fast: false,
            origin: SessionOrigin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(RESEARCH_ROLE.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::Web,
            add_dirs: Vec::new(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: true,
            allowed_models: None,
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
        };
        let account = self.runtime.launch_account(runs_on, spec.model.as_deref());
        let meter = TokenMeter::default().on_account(account.account.clone());
        let ran = run_web_session(
            self.runtime.start_hosted(&owner, &account, spec),
            prompt,
            &cancel,
            &self.research.stop,
            time,
            |event| async {
                match event {
                    ProviderEvent::RateLimits { quota } => {
                        self.runtime
                            .note_quota_snapshot(&account, quota.clone())
                            .await;
                    }
                    ProviderEvent::Usage { total, last } => {
                        self.note_tokens(
                            &meter,
                            runs_on,
                            Some(&cheap),
                            TokenOwner::Upkeep,
                            &total,
                            last.as_ref(),
                        )
                        .await;
                    }
                    _ => {}
                }
            },
        )
        .await;
        let _ = self.runtime.ledger().dispose(&owner).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        ran
    }
}

/// The research session's reply as a note, within bounds: a known tier, strengths 0–10 for
/// categories the router has, a plausible context window, a short summary and a few web
/// sources.
fn note_from(provider: ProviderKind, model: &str, reply: &str) -> Result<ResearchNote> {
    let start = reply.find('{');
    let end = reply.rfind('}');
    let (Some(start), Some(end)) = (start, end) else {
        return Err(Error::Invalid("the research gave no JSON".into()));
    };
    let finding: Finding = serde_json::from_str(&reply[start..=end])
        .map_err(|err| Error::Invalid(format!("the research's JSON didn't read: {err}")))?;
    let strengths = TaskCategory::ALL
        .into_iter()
        .filter_map(|category| {
            let name = serde_json::to_value(category).ok()?.as_str()?.to_owned();
            let value = finding.strengths.get(&name)?;
            value
                .is_finite()
                .then(|| (category, value.clamp(0.0, 10.0)))
        })
        .collect();
    let summary: String = finding
        .summary
        .trim()
        .chars()
        .take(MAX_SUMMARY_CHARS)
        .collect();
    if summary.is_empty() {
        return Err(Error::Invalid("the research found nothing".into()));
    }
    Ok(ResearchNote {
        provider,
        model: model.to_owned(),
        at_ms: now_ms(),
        summary,
        tier: finding.tier,
        strengths,
        context_window: finding
            .context_window
            .filter(|tokens| CONTEXT_WINDOWS.contains(tokens)),
        knowledge_cutoff: finding
            .knowledge_cutoff
            .filter(|cutoff| cutoff.len() <= 10 && !cutoff.trim().is_empty()),
        sources: finding
            .sources
            .into_iter()
            .filter(|source| source.starts_with("https://") && source.len() <= 500)
            .take(MAX_SOURCES)
            .collect(),
    })
}

struct WebResearch {
    provider: ProviderKind,
    model: String,
    effort: Option<String>,
    prompt: String,
    cancel: CancellationToken,
    time: Duration,
}

impl SessionManager {
    /// The caller sends the start signal only after flushing the job id to the client.
    pub fn refresh_rankings(
        &self,
        check: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> Result<(String, Option<tokio::sync::oneshot::Sender<()>>)> {
        self.admit()?;
        let (id, cancel) = self.runtime.registry().admit_refresh();
        let mut start = None;
        if let Some(cancel) = cancel {
            let (tx, acknowledged) = tokio::sync::oneshot::channel();
            start = Some(tx);
            let manager = self.arc();
            let job = id.clone();
            let turn = self.background_turn();
            self.spawn(self.research.jobs.track_future(async move {
                let _turn = turn;
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return,
                    () = manager.research.stop.cancelled() => return,
                    checked = after_acknowledgement(acknowledged, check) => {
                        if let Err(error) = checked {
                            let revision = manager.runtime.registry().current().revision;
                            manager.runtime.registry().finish_refresh(&job, revision, &[], Err(error));
                            manager.runtime.rankings_changed().await;
                            return;
                        }
                    }
                }
                manager.research_rankings(&job, cancel).await;
                manager.runtime.rankings_changed().await;
            }));
        }
        Ok((id, start))
    }

    async fn research_rankings(&self, id: &str, cancel: CancellationToken) {
        let now = now_ms();
        let inputs = self.routing_inputs(None, now).await;
        let settings = self.core.settings();
        let producer = pick_researcher(
            &inputs.models,
            &inputs.providers,
            &inputs.registry,
            now,
            |model| {
                self.provider_usable(model.provider)
                    && crate::routing::availability::model_available(
                        &settings,
                        model.provider,
                        &model.id,
                    )
            },
        );
        let Some(producer) = producer else {
            self.runtime.registry().finish_refresh(
                id,
                inputs.registry.revision,
                &[],
                Err(
                    "None of your models can do the research right now: turn one on, or wait until its limit resets."
                        .into(),
                ),
            );
            return;
        };
        let catalogs: Vec<_> = ProviderKind::ALL
            .into_iter()
            .map(|provider| {
                let models = self
                    .runtime
                    .overview(provider)
                    .and_then(|o| o.models)
                    .map(|c| c.models)
                    .unwrap_or_default();
                (provider, models)
            })
            .collect();
        let refs: Vec<_> = catalogs
            .iter()
            .map(|(provider, models)| (*provider, models.as_slice()))
            .collect();
        let notes: Vec<_> = inputs
            .models
            .iter()
            .filter_map(|model| model.research.clone())
            .collect();
        let Some((revision, models)) = self.runtime.registry().capture_refresh(
            id,
            &refs,
            (producer.provider, &producer.id),
            &notes,
        ) else {
            return;
        };
        let allowlist: Vec<_> = models
            .iter()
            .map(|model| {
                serde_json::json!({
                    "provider": model.provider, "catalogId": model.id,
                    "model": model.rating_identity(), "efforts": model.efforts,
                    "tier": model.tier, "strengths": model.strengths,
                    "areaStrengths": model.area_strengths, "defaultEffort": model.default_effort,
                })
            })
            .collect();
        let prompt = format!(
            r#"Research current official model cards, release notes and reputable benchmarks for
EVERY model in this catalog, as of {}. Do not guess. Omit models you cannot source.
Catalog and prior ratings: {}.
Reply with exactly one JSON object:
{{"patches":[{{"provider":"claude or codex","model":"the supplied concrete model identity",
"sources":["https URLs you actually read"],"tier":"light|standard|strong|frontier",
"strengths":{{"scout":5}},"areaStrengths":{{"backend":0}},
"defaultEffort":{{"research":"medium"}}}}]}}.
Each field is optional except provider, model and sources. Strength categories: scout,
research, implement, review, merge, verify, chat, orchestrate (0 to 10). Areas: frontend,
backend, infra, docs, tests (-2 to 2). Default efforts must be in the model's supplied efforts
list and never above high. Return one patch per concrete identity; equivalent aliases share
it. Do not change any other facts."#,
            jiff::Timestamp::now(),
            serde_json::to_string(&allowlist).unwrap_or_default()
        );
        let effort = ["high", "medium", "low"]
            .into_iter()
            .find(|effort| producer.efforts.iter().any(|value| value == effort))
            .map(str::to_owned);
        let reply = self
            .run_web_research(WebResearch {
                provider: producer.provider,
                model: producer.id.clone(),
                effort,
                prompt,
                cancel,
                time: Duration::from_secs(15 * 60),
            })
            .await
            .map_err(|error| error.to_string());
        self.runtime
            .registry()
            .finish_refresh(id, revision, &models, reply);
    }
}

/// The strongest model that may research now: not excluded (Fable never is picked), allowed by
/// `allowed`, and with no used-up window of its own (a weekly Opus cap, say) even while its
/// provider has quota left. Ties go to the better research score.
fn pick_researcher<'a>(
    models: &'a [MergedModel],
    providers: &[brigadier_router::ProviderState],
    registry: &brigadier_router::Registry,
    now: i64,
    allowed: impl Fn(&MergedModel) -> bool,
) -> Option<&'a MergedModel> {
    models
        .iter()
        .filter(|model| {
            !model.excluded
                && allowed(model)
                && brigadier_router::available(model, providers, registry, now)
        })
        .max_by(|a, b| {
            a.tier.cmp(&b.tier).then_with(|| {
                a.strengths
                    .get(&TaskCategory::Research)
                    .unwrap_or(&5.0)
                    .total_cmp(b.strengths.get(&TaskCategory::Research).unwrap_or(&5.0))
            })
        })
}

/// No repository request is polled until the IPC response has been flushed.
async fn after_acknowledgement(
    acknowledged: tokio::sync::oneshot::Receiver<()>,
    check: impl std::future::Future<Output = ()>,
) -> std::result::Result<(), String> {
    acknowledged
        .await
        .map_err(|_| "the client disconnected before research started".to_owned())?;
    check.await;
    Ok(())
}

/// Both research jobs use the same bounded, read-only turn protocol. Closing is outside the
/// cancellation race, so reset and shutdown still reap the CLI.
async fn run_web_session<F, Observe, Observed>(
    start: F,
    prompt: String,
    cancel: &CancellationToken,
    stop: &CancellationToken,
    time: Duration,
    mut observe: Observe,
) -> Result<String>
where
    F: std::future::Future<Output = Result<Started>>,
    Observe: FnMut(ProviderEvent) -> Observed,
    Observed: std::future::Future<Output = ()>,
{
    let deadline = tokio::time::Instant::now() + time;
    let Started {
        session,
        mut events,
    } = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(Error::Invalid("research cancelled".into())),
        () = stop.cancelled() => return Err(Error::Invalid("research cancelled".into())),
        started = tokio::time::timeout_at(deadline, start) =>
            started.map_err(|_| Error::Provider("the research startup ran out of time".into()))??,
    };
    let turn = async {
        session
            .send(TurnInput::text(prompt))
            .await
            .map_err(|err| Error::Provider(format!("the research didn't start: {err}")))?;
        let mut last = String::new();
        while let Some(event) = events.recv().await {
            match event {
                ProviderEvent::Message {
                    role: Role::Assistant,
                    text,
                    ..
                } => last = text,
                ProviderEvent::ApprovalRequested { request } => {
                    session
                        .answer(
                            request.id,
                            ApprovalDecision::Deny {
                                message: "Declined: research only reads the web.".into(),
                            },
                        )
                        .await
                        .map_err(|err| Error::Provider(err.to_string()))?;
                }
                ProviderEvent::TurnCompleted { status, .. } => {
                    return match status {
                        TurnStatus::Completed => Ok(last),
                        _ => Err(Error::Provider("the research turn did not finish".into())),
                    };
                }
                ProviderEvent::Exited { .. } => {
                    return Err(Error::Provider("the research CLI exited".into()));
                }
                event => observe(event).await,
            }
        }
        Err(Error::Provider("the research CLI went away".into()))
    };
    let reply = tokio::select! {
        biased;
        () = cancel.cancelled() => Err(Error::Invalid("research cancelled".into())),
        () = stop.cancelled() => Err(Error::Invalid("research cancelled".into())),
        reply = tokio::time::timeout_at(deadline, turn) =>
            reply.unwrap_or_else(|_| Err(Error::Provider("the research ran out of time".into()))),
    };
    session.close().await;
    reply
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::{BoxFuture, ProviderSession};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct FakeSession {
        closed: AtomicBool,
        fail_send: bool,
    }
    impl ProviderSession for FakeSession {
        fn native_id(&self) -> String {
            "fake".into()
        }
        fn send(&self, _: TurnInput) -> BoxFuture<'_, brigadier_providers::Result<()>> {
            Box::pin(async {
                if self.fail_send {
                    Err(brigadier_providers::Error::Invalid("fake failure".into()))
                } else {
                    Ok(())
                }
            })
        }
        fn steer(&self, input: TurnInput) -> BoxFuture<'_, brigadier_providers::Result<()>> {
            self.send(input)
        }
        fn interrupt(&self) -> BoxFuture<'_, brigadier_providers::Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn answer(
            &self,
            _: String,
            decision: ApprovalDecision,
        ) -> BoxFuture<'_, brigadier_providers::Result<()>> {
            assert!(matches!(decision, ApprovalDecision::Deny { .. }));
            Box::pin(async { Ok(()) })
        }
        fn close(&self) -> BoxFuture<'_, ()> {
            Box::pin(async {
                self.closed.store(true, Ordering::SeqCst);
            })
        }
        fn is_running(&self) -> bool {
            !self.closed.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn web_runner_closes_on_success_failure_timeout_reset_and_shutdown() {
        for mode in ["success", "failure", "timeout", "reset", "shutdown"] {
            let session = Arc::new(FakeSession {
                closed: AtomicBool::new(false),
                fail_send: mode == "failure",
            });
            let (tx, events) = tokio::sync::mpsc::channel(4);
            let cancel = CancellationToken::new();
            let stop = CancellationToken::new();
            if mode == "success" {
                tx.send(ProviderEvent::Message {
                    item_id: "a".into(),
                    role: Role::Assistant,
                    text: "{}".into(),
                })
                .await
                .unwrap();
                tx.send(ProviderEvent::TurnCompleted {
                    turn_id: None,
                    status: TurnStatus::Completed,
                    duration_ms: None,
                    usage: None,
                })
                .await
                .unwrap();
            }
            let trigger = if mode == "reset" {
                cancel.clone()
            } else {
                stop.clone()
            };
            let should_cancel = matches!(mode, "reset" | "shutdown");
            let start = async {
                if should_cancel {
                    trigger.cancel();
                }
                Ok(Started {
                    session: session.clone(),
                    events,
                })
            };
            let result = run_web_session(
                start,
                "research".into(),
                &cancel,
                &stop,
                Duration::from_millis(10),
                |_| async {},
            )
            .await;
            assert_eq!(result.is_ok(), mode == "success", "{mode}: {result:?}");
            assert!(session.closed.load(Ordering::SeqCst), "{mode}");
            drop(tx);
        }
    }

    #[tokio::test]
    async fn cancelled_start_is_not_polled_and_startup_is_bounded() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let stop = CancellationToken::new();
        let result = run_web_session(
            async { panic!("cancelled startup was polled") },
            String::new(),
            &cancel,
            &stop,
            Duration::from_millis(1),
            |_| async {},
        )
        .await;
        assert!(result.is_err());
        let result = run_web_session(
            std::future::pending(),
            String::new(),
            &CancellationToken::new(),
            &stop,
            Duration::from_millis(1),
            |_| async {},
        )
        .await;
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("startup ran out of time")
        );
    }
    #[tokio::test]
    async fn refresh_response_is_flushed_before_the_fake_fetch_starts() {
        let fetched = Arc::new(AtomicBool::new(false));
        let (response, acknowledged) = tokio::sync::oneshot::channel();
        let count = fetched.clone();
        let job = tokio::spawn(after_acknowledgement(acknowledged, async move {
            count.store(true, Ordering::SeqCst);
        }));
        tokio::task::yield_now().await;
        assert!(!fetched.load(Ordering::SeqCst));
        response.send(()).unwrap();
        job.await.unwrap().unwrap();
        assert!(fetched.load(Ordering::SeqCst));

        let (response, acknowledged) = tokio::sync::oneshot::channel();
        drop(response);
        assert!(
            after_acknowledgement(acknowledged, async {
                panic!("disconnected client fetched")
            })
            .await
            .is_err()
        );
    }

    #[test]
    fn researcher_skips_a_model_whose_own_window_is_used_up() {
        let registry = brigadier_router::Registry::bundled();
        let info = |id: &str| brigadier_providers::ModelInfo {
            id: id.to_owned(),
            display_name: id.to_owned(),
            description: String::new(),
            resolved: None,
            efforts: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("medium".into()),
            is_default: false,
            input_modalities: vec!["text".into()],
            fast: None,
            legacy: false,
        };
        let catalog = [info("opus"), info("sonnet")];
        let models =
            brigadier_router::merge(&registry, &[(ProviderKind::Claude, &catalog)], &[], &[]);
        let state = |used_percent: f64| brigadier_router::ProviderState {
            provider: ProviderKind::Claude,
            logged_in: true,
            quota: Some(brigadier_router::ProviderQuota {
                provider: ProviderKind::Claude,
                windows: vec![brigadier_router::WindowState {
                    window: brigadier_providers::QuotaWindow {
                        id: "seven_day_opus".into(),
                        label: "Weekly (Opus)".into(),
                        used_percent,
                        resets_at_ms: Some(86_400_000),
                        window_minutes: None,
                        bucket: None,
                        model: Some("opus".into()),
                    },
                    forecast: None,
                    heat: brigadier_router::Heat::Cool,
                }],
                limit: None,
                heat: brigadier_router::Heat::Cool,
                observed_at_ms: None,
            }),
        };
        let pick = |providers: &[brigadier_router::ProviderState]| {
            pick_researcher(&models, providers, &registry, 0, |_| true)
                .map(|model| model.id.clone())
        };
        assert_eq!(pick(&[state(10.0)]).as_deref(), Some("opus"));
        assert_eq!(pick(&[state(100.0)]).as_deref(), Some("sonnet"));
        assert_eq!(
            pick_researcher(&models, &[state(10.0)], &registry, 0, |model| model.id
                != "opus")
            .map(|model| model.id.as_str()),
            Some("sonnet")
        );
    }
}
