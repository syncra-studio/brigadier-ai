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
use brigadier_router::{QualityTier, ResearchNote, TaskCategory};
use serde::Deserialize;

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

const RESEARCH_ROLE: &str = "You research one AI model for Brigadier, an app that routes coding tasks between AI models. Use web search to read the model's official release notes or model card and reputable benchmark results. Don't guess: leave out what you can't find. Change nothing on this computer. Your final message is exactly one JSON object and nothing else.";

/// Research in progress and past failures.
#[derive(Default)]
pub(crate) struct Research {
    running: Mutex<Option<(ProviderKind, String)>>,
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
        self.spawn(async move {
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
        });
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
        let id = uuid::Uuid::now_v7().to_string();
        let owner = format!("research:{id}");
        let scratch = self.owned_dir("scratch", &format!("research-{id}"));
        self.prepare_owned_dir(&owner, &scratch).await?;
        let (cheap, effort) = self.cheapest(runs_on)?;
        let spec = SessionSpec {
            cwd: scratch.clone(),
            model: Some(cheap.clone()),
            effort: effort.or_else(|| Some("low".into())),
            fast: false,
            origin: SessionOrigin::New,
            access: Access::ReadOnly,
            append_system_prompt: Some(RESEARCH_ROLE.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::Web,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            path_prepend: Vec::new(),
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: true,
            allowed_models: None,
            unattended: false,
        };
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
        let ran = async {
            let Started {
                session,
                mut events,
            } = self.runtime.start_hosted(&owner, runs_on, spec).await?;
            let meter = TokenMeter::default();
            let turn = async {
                session
                    .send(TurnInput::text(prompt))
                    .await
                    .map_err(|err| Error::Provider(format!("the research didn't start: {err}")))?;
                let mut last = String::new();
                while let Some(event) = events.recv().await {
                    match event {
                        ProviderEvent::RateLimits { quota } => {
                            self.runtime.note_quota_snapshot(quota).await;
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
                        ProviderEvent::Message {
                            role: Role::Assistant,
                            text,
                            ..
                        } => last = text,
                        ProviderEvent::ApprovalRequested { request } => {
                            let _ = session
                                .answer(
                                    request.id,
                                    ApprovalDecision::Deny {
                                        message: "Declined: research only reads the web.".into(),
                                    },
                                )
                                .await;
                        }
                        ProviderEvent::TurnCompleted { status, .. } => {
                            return match status {
                                TurnStatus::Completed => Ok(last),
                                _ => {
                                    Err(Error::Provider("the research turn did not finish".into()))
                                }
                            };
                        }
                        ProviderEvent::Exited { .. } => {
                            return Err(Error::Provider("the research CLI exited".into()));
                        }
                        _ => {}
                    }
                }
                Err(Error::Provider("the research CLI went away".into()))
            };
            let reply = tokio::time::timeout(RESEARCH_TIME, turn)
                .await
                .unwrap_or_else(|_| Err(Error::Provider("the research ran out of time".into())));
            session.close().await;
            reply
        }
        .await;
        let _ = self.runtime.ledger().dispose(&owner).await;
        let _ = tokio::fs::remove_dir_all(&scratch).await;
        note_from(of, model, &ran?)
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
