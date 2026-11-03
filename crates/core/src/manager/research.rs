//! The daily ratings research (brief L1, L8, L10): once a day, at the time Settings › Routing
//! sets (04:00 by default; at the next start when missed), the model with the best research
//! rating that still has quota reads current model cards, release notes and benchmarks with
//! read-only web tools and answers one JSON object of rating patches, within 15 minutes.
//!
//! The registry checks and keeps the patches ([`crate::routing::RegistryHolder::finish_refresh`]):
//! each is sourced, names a concrete model of the user's catalogs (an inherited model, such as
//! `gpt-6.1-terra` from `gpt-5.6-terra`'s entry, as itself), keeps to the registry's bounds, and
//! no vendor raises its own models. The automatic picks follow the result; the user's own
//! orders in `models.md` never change, they get suggestions instead.

use std::time::Duration;

use brigadier_providers::{
    Access, ApprovalDecision, Origin, ProviderEvent, ProviderKind, Role, SessionSpec, Started,
    ToolSet, TurnInput, TurnStatus,
};
use brigadier_router::{MergedModel, Rating, TaskCategory};
use tokio_util::sync::CancellationToken;

use super::SessionManager;
use crate::routing::registry::RankingsRefreshState;
use crate::{Error, Result, RoutingSuggestion, now_ms};

/// The research session's time box.
const RESEARCH_TIME: Duration = Duration::from_secs(15 * 60);

const RESEARCH_ROLE: &str = "You research AI models for Brigadier, an app that routes coding work between AI models. Use web search to read official model cards, release notes and reputable benchmark results. Don't guess: leave out what you can't source. Change nothing on this computer. Your final message is exactly one JSON object and nothing else.";

impl SessionManager {
    /// Researches the ratings now, unless a research is already running: `check` (the
    /// registry's own update check) first, then the research. Returns when it is done; the
    /// result is the registry's rankings refresh.
    pub async fn research_ratings(
        &self,
        check: impl std::future::Future<Output = ()>,
    ) -> Result<()> {
        self.admit()?;
        let (id, cancel) = self.runtime.registry().admit_refresh();
        let Some(cancel) = cancel else {
            return Ok(());
        };
        let _turn = self.background_turn();
        tokio::select! {
            biased;
            () = cancel.cancelled() => return Ok(()),
            () = check => {}
        }
        let before = self.routing_suggestions().unwrap_or_default();
        self.research_rankings(&id, cancel).await;
        let refresh = self.runtime.registry().rankings_refresh();
        if refresh.job_id.as_deref() == Some(id.as_str())
            && refresh.state == RankingsRefreshState::Done
        {
            let after = self.routing_suggestions().unwrap_or_default();
            self.runtime
                .registry()
                .note_found(&id, newly_found(&before, &after));
        }
        self.runtime.rankings_changed().await;
        Ok(())
    }

    async fn research_rankings(&self, id: &str, cancel: CancellationToken) {
        let now = now_ms();
        let inputs = self.routing_inputs(now).await;
        let settings = self.core.settings();
        let producer = pick_researcher(
            &inputs.models,
            &inputs.providers,
            &inputs.registry,
            now,
            |model| {
                self.provider_ready(model.provider)
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
        let catalogs = research_catalogs(
            &settings,
            brigadier_providers::adapters::kinds().map(|provider| {
                let models = self
                    .runtime
                    .overview(provider)
                    .and_then(|o| o.models)
                    .map(|c| c.models)
                    .unwrap_or_default();
                (provider, models)
            }),
        );
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
        let effort = ["high", "medium", "low"]
            .into_iter()
            .find(|effort| producer.efforts.iter().any(|value| value == effort))
            .map(str::to_owned);
        let account = self
            .runtime
            .launch_account(producer.provider, Some(&producer.id));
        let reply = self
            .run_web_research(
                &account,
                producer.id.clone(),
                effort,
                research_prompt(&models),
                &cancel,
            )
            .await
            .map_err(|error| error.to_string());
        self.runtime
            .registry()
            .finish_refresh(id, revision, &models, reply);
    }

    /// One read-only web research turn on `account`, bounded by [`RESEARCH_TIME`].
    async fn run_web_research(
        &self,
        account: &crate::accounts::AccountRef,
        model: String,
        effort: Option<String>,
        prompt: String,
        cancel: &CancellationToken,
    ) -> Result<String> {
        let run = uuid::Uuid::now_v7().to_string();
        let owner = format!("research:{run}");
        let scratch = self.owned_dir("scratch", &format!("research-{run}"));
        if let Err(error) = self.prepare_owned_dir(&owner, &scratch, true).await {
            let _ = self.runtime.ledger().dispose(&owner).await;
            let _ = tokio::fs::remove_dir_all(&scratch).await;
            return Err(error);
        }
        let spec = SessionSpec {
            cwd: scratch.clone(),
            model: Some(model),
            effort,
            fast: false,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(RESEARCH_ROLE.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::Web,
            skill_plugins: Vec::new(),
            switched_off: Default::default(),
            add_dirs: Vec::new(),
            project: None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: Some(scratch.clone()),
            keeps_detached: false,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            naked_worker: false,
            native_compaction: None,
            temp_tag: None,
        };
        let ran = run_web_session(
            self.runtime.start_hosted(&owner, account, spec),
            prompt,
            cancel,
            RESEARCH_TIME,
            |event| async {
                match event {
                    ProviderEvent::RateLimits { quota } => {
                        self.runtime.note_quota_snapshot(account, quota).await;
                    }
                    ProviderEvent::Error { error } if !error.will_retry => {
                        if let Some(limit) = error.limit {
                            self.runtime.note_limit(account, limit).await;
                        }
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

/// The kinds of work with a suggestion after the research that had none for that model before.
fn newly_found(before: &[RoutingSuggestion], after: &[RoutingSuggestion]) -> Vec<String> {
    let mut works: Vec<String> = Vec::new();
    for found in after {
        let known = before.iter().any(|old| {
            old.work == found.work
                && old.row.agent == found.row.agent
                && old.row.model == found.row.model
        });
        if !known && !works.contains(&found.work) {
            works.push(found.work.clone());
        }
    }
    works
}

/// What the research is asked: every model of the catalogs with its ratings now, and the one
/// JSON object to answer with.
fn research_prompt(models: &[MergedModel]) -> String {
    let catalog: Vec<_> = models
        .iter()
        .map(|model| {
            serde_json::json!({
                "provider": model.provider, "catalogId": model.id,
                "model": model.rating_identity(), "efforts": model.efforts,
                "rated": model.status, "inheritsFrom": model.registry_key
                    .as_ref()
                    .filter(|_| model.status == brigadier_router::ModelStatus::Inherited),
                "tier": model.tier, "strengths": model.strengths,
                "areaStrengths": model.area_strengths, "defaultEffort": model.default_effort,
            })
        })
        .collect();
    let providers: Vec<&str> = brigadier_providers::adapters::kinds()
        .map(ProviderKind::id)
        .collect();
    let categories: Vec<String> = TaskCategory::ALL
        .into_iter()
        .filter_map(|category| {
            serde_json::to_value(category)
                .ok()?
                .as_str()
                .map(str::to_owned)
        })
        .collect();
    format!(
        r#"Research current official model cards, release notes and reputable benchmarks for
EVERY model in this catalog, as of {now}. Do not guess. Omit models you cannot source.
Catalog and current ratings: {catalog}.
A model marked "rated":"inherited" has the ratings of an older model of its family ("inheritsFrom")
copied in: research it as itself and rate it on its own.
Reply with exactly one JSON object:
{{"patches":[{{"provider":"{providers}","model":"the supplied concrete model identity",
"sources":["https URLs you actually read"],"tier":"light|standard|strong|frontier",
"strengths":{{"operate":7}},"areaStrengths":{{"backend":0}},
"defaultEffort":{{"operate":"medium"}}}}]}}.
Each field is optional except provider, model and sources. Strength categories ({categories}),
0 to 10 each: how well suited the model is, weighing quality against cost. "operate" is seeing
and using apps on screen (computer use); "design" is mockups and images; "gitText" is short git
text such as a commit message; "orchestrate" is planning and delegating work. Areas: frontend,
backend, infra, docs, tests (-2 to 2). Default efforts must be in the model's supplied efforts
list and never above high. Return one patch per concrete identity; equivalent aliases share it.
Do not change any other facts."#,
        now = jiff::Timestamp::now(),
        catalog = serde_json::to_string(&catalog).unwrap_or_default(),
        providers = providers.join(" or "),
        categories = categories.join(", "),
    )
}

/// The models a research rates: what each agent lists, less the models and agents the user
/// switched off.
fn research_catalogs(
    settings: &crate::model::Settings,
    catalogs: impl IntoIterator<Item = (ProviderKind, Vec<brigadier_providers::ModelInfo>)>,
) -> Vec<(ProviderKind, Vec<brigadier_providers::ModelInfo>)> {
    catalogs
        .into_iter()
        .map(|(provider, mut models)| {
            models.retain(|model| {
                crate::routing::availability::model_available(settings, provider, &model.id)
            });
            (provider, models)
        })
        .collect()
}

/// The researcher (brief L10): the model with the best research rating that may run now, not
/// excluded, allowed by `allowed`, and with no used-up window of its own (a weekly Opus cap,
/// say) even while its provider has quota left. Ties go to the higher tier.
fn pick_researcher<'a>(
    models: &'a [MergedModel],
    providers: &[brigadier_router::ProviderState],
    registry: &brigadier_router::Registry,
    now: i64,
    allowed: impl Fn(&MergedModel) -> bool,
) -> Option<&'a MergedModel> {
    let rating = |model: &MergedModel| {
        brigadier_router::picks::score(model, Rating::Categories(&[TaskCategory::Research]))
    };
    models
        .iter()
        .filter(|model| {
            rating(model).is_some()
                && allowed(model)
                && brigadier_router::available(model, providers, registry, now)
        })
        .max_by(|a, b| {
            rating(a)
                .unwrap_or(0.0)
                .total_cmp(&rating(b).unwrap_or(0.0))
                .then_with(|| a.tier.cmp(&b.tier))
                // The first listed of equals.
                .then(std::cmp::Ordering::Greater)
        })
}

/// One bounded, read-only research turn. Closing is outside the cancellation race, so a reset
/// and shutdown still reap the CLI.
async fn run_web_session<F, Observe, Observed>(
    start: F,
    prompt: String,
    cancel: &CancellationToken,
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
        reply = tokio::time::timeout_at(deadline, turn) =>
            reply.unwrap_or_else(|_| Err(Error::Provider("the research ran out of time".into()))),
    };
    session.close().await;
    reply
}

/// When the daily research is next due: the latest scheduled time (`hours`:`minutes`, local)
/// not after `now`, if the research hasn't run since. `None`: not due.
pub fn research_due(now_ms: i64, hours: u8, minutes: u8, last_ran_ms: Option<i64>) -> Option<i64> {
    let zone = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::from_millisecond(now_ms)
        .ok()?
        .to_zoned(zone);
    let today = now
        .date()
        .at(i8::try_from(hours).ok()?, i8::try_from(minutes).ok()?, 0, 0)
        .to_zoned(now.time_zone().clone())
        .ok()?;
    let scheduled = if today.timestamp() <= now.timestamp() {
        today
    } else {
        today.yesterday().ok()?
    };
    let at = scheduled.timestamp().as_millisecond();
    last_ran_ms.is_none_or(|ran| ran < at).then_some(at)
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
    async fn web_runner_closes_on_success_failure_timeout_and_cancel() {
        for mode in ["success", "failure", "timeout", "cancel"] {
            let session = Arc::new(FakeSession {
                closed: AtomicBool::new(false),
                fail_send: mode == "failure",
            });
            let (tx, events) = tokio::sync::mpsc::channel(4);
            let cancel = CancellationToken::new();
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
            let start = async {
                if mode == "cancel" {
                    cancel.cancel();
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
        let result = run_web_session(
            async { panic!("cancelled startup was polled") },
            String::new(),
            &cancel,
            Duration::from_millis(1),
            |_| async {},
        )
        .await;
        assert!(result.is_err());
        let result = run_web_session(
            std::future::pending(),
            String::new(),
            &CancellationToken::new(),
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

    fn info(id: &str) -> brigadier_providers::ModelInfo {
        brigadier_providers::ModelInfo {
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
        }
    }

    #[test]
    fn a_research_leaves_out_the_models_you_switched_off() {
        let mut settings = crate::model::Settings::default();
        settings.hidden_models.push(crate::model::ModelRef {
            provider: ProviderKind::Claude,
            id: "sonnet".into(),
        });
        let catalogs = research_catalogs(
            &settings,
            [
                (ProviderKind::Claude, vec![info("opus"), info("sonnet")]),
                (ProviderKind::Codex, vec![info("gpt-6.1-sol")]),
            ],
        );
        let ids = |provider| {
            catalogs
                .iter()
                .find(|(kind, _)| *kind == provider)
                .map(|(_, models)| models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>())
                .unwrap()
        };
        assert_eq!(ids(ProviderKind::Claude), ["opus"]);
        assert_eq!(ids(ProviderKind::Codex), ["gpt-6.1-sol"]);
    }

    /// The best research rating with quota left researches; a model whose own window is used
    /// up, or one switched off, gives way to the next.
    #[test]
    fn the_researcher_is_the_best_research_rating_with_quota() {
        let registry = brigadier_router::Registry::bundled();
        let catalog = [info("claude-opus-5-5"), info("claude-sonnet-5-5")];
        let models =
            brigadier_router::merge(&registry, &[(ProviderKind::Claude, &catalog)], &[], &[]);
        let state = |used_percent: f64| brigadier_router::ProviderState {
            provider: ProviderKind::Claude,
            logged_in: true,
            quota: Some(brigadier_router::ProviderQuota {
                provider: ProviderKind::Claude,
                windows: vec![brigadier_router::WindowState {
                    window: brigadier_providers::QuotaWindow {
                        id: "seven_day_sonnet".into(),
                        label: "Weekly (Sonnet)".into(),
                        used_percent,
                        resets_at_ms: Some(86_400_000),
                        window_minutes: None,
                        bucket: None,
                        model: Some("sonnet".into()),
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
        // Sonnet 5.5 rates 8 for research, Opus 5.5 7.5.
        assert_eq!(pick(&[state(10.0)]).as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(pick(&[state(100.0)]).as_deref(), Some("claude-opus-5-5"));
        assert_eq!(
            pick_researcher(&models, &[state(10.0)], &registry, 0, |model| model.id
                != "claude-sonnet-5-5")
            .map(|model| model.id.as_str()),
            Some("claude-opus-5-5")
        );
    }

    #[test]
    fn the_research_is_due_once_after_each_scheduled_time() {
        let zone = jiff::tz::TimeZone::system();
        let at = |hour: i8, minute: i8| {
            jiff::civil::date(2026, 11, 3)
                .at(hour, minute, 0, 0)
                .to_zoned(zone.clone())
                .unwrap()
                .timestamp()
                .as_millisecond()
        };
        let four = at(4, 0);
        // Before 04:00 today, yesterday's run is what's due; after it, today's.
        assert!(research_due(at(3, 0), 4, 0, None).is_some_and(|due| due < four));
        assert_eq!(research_due(at(9, 0), 4, 0, None), Some(four));
        assert_eq!(research_due(at(4, 0), 4, 0, Some(at(3, 59))), Some(four));
        // Ran since: not due again until tomorrow's.
        assert_eq!(research_due(at(9, 0), 4, 0, Some(at(4, 1))), None);
        assert_eq!(research_due(at(23, 59), 4, 0, Some(four)), None);
        // Asleep or closed at 04:00: due at the next start, once.
        assert_eq!(
            research_due(at(10, 30), 4, 0, Some(at(3, 0) - 86_400_000)),
            Some(four)
        );
    }

    #[test]
    fn only_a_suggestion_the_research_brought_counts_as_found() {
        let suggestion = |work: &str, model: &str| RoutingSuggestion {
            work: work.into(),
            row: crate::GuidanceRow {
                work: work.into(),
                agent: ProviderKind::Codex,
                model: model.into(),
                effort: None,
                reason: String::new(),
            },
            score: 9.5,
            current: "opus".into(),
            current_score: 9.0,
        };
        let before = [suggestion("Independent review", "gpt-6-astra")];
        let after = [
            suggestion("Independent review", "gpt-6-astra"),
            suggestion("Computer use", "gpt-6.1-terra"),
        ];
        assert_eq!(newly_found(&before, &after), ["Computer use"]);
        assert!(newly_found(&after, &after).is_empty());
    }
}
