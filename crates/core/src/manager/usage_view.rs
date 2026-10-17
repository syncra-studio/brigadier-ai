//! The Usage page's read and the Inspector's routing preview (PLAN.md §6 Phase 5).

use std::collections::{HashMap, HashSet};

use brigadier_providers::ProviderKind;
use brigadier_router::{Area, Heat, TaskCategory};

use super::SessionManager;
use super::routing::{Ask, Trial};
use crate::accounts::AccountRef;
use crate::model::{
    AccountActivity, AccountTokens, AccountUsage, ConversationId, ConversationTokens, ModelChoice,
    ProjectId, ProviderUsage, RoutePreview, RoutePreviewOutcome, RoutingActivity, TokenCount,
    UsageActivity, UsageView, WindowHistory,
};
use crate::routing::TurnUsage;
use crate::work::AttemptEnd;
use crate::{Result, now_ms};

/// Hand-offs this recent are listed.
const ACTIVITY_MS: i64 = 7 * 24 * 60 * 60 * 1000;
/// Conversations listed per account, most tokens first.
const TOP_CONVERSATIONS: usize = 5;
/// The longest range the Usage page's Activity shows, in days (what the history keeps).
const ACTIVITY_DAYS_MAX: u32 = 31;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

impl SessionManager {
    /// Everything the Usage page shows. `project`: whose outcomes to show what routing learned
    /// from (every project's when absent).
    pub async fn usage_view(&self, project: Option<ProjectId>) -> Result<UsageView> {
        let now = now_ms();
        let inputs = self.routing_inputs(project.as_ref(), now).await;
        let learned = match (&project, self.runtime.routing_store()) {
            (Some(_), _) => inputs.learned,
            (None, Some(store)) => {
                let outcomes = store.outcomes(None).await.unwrap_or_default();
                brigadier_router::learn(&outcomes, &inputs.models, &inputs.registry)
            }
            (None, None) => Vec::new(),
        };
        let working = self.working_accounts().await;
        let providers = inputs
            .providers
            .iter()
            .map(|state| ProviderUsage {
                provider: state.provider,
                accounts: self
                    .runtime
                    .account_candidates(state.provider)
                    .into_iter()
                    .map(|candidate| AccountUsage {
                        working: working.contains(&candidate.account),
                        quota: self.runtime.account_usage(&candidate.account, now),
                        read_at_ms: self.runtime.monitor().read_at(&candidate.account),
                        account: candidate.account,
                    })
                    .collect(),
                balancing: state
                    .quota
                    .as_ref()
                    .and_then(|quota| balancing(state.provider, quota, &inputs.providers)),
            })
            .collect();
        Ok(UsageView {
            providers,
            activity: self.routing_activity(now).await,
            models: inputs.models,
            learned,
            project_id: project,
            registry: self.runtime.registry().info(),
        })
    }

    /// The Usage page's Activity over the last `days`: each account's windows' samples, and
    /// Brigadier's own tokens on it, by model and by conversation.
    pub async fn usage_activity(&self, days: u32) -> UsageActivity {
        let now = now_ms();
        let since_ms = now - i64::from(days.clamp(1, ACTIVITY_DAYS_MAX)) * DAY_MS;
        let mut accounts = Vec::new();
        for provider in ProviderKind::ALL {
            let turns = match self.runtime.routing_store() {
                Some(store) => store
                    .turns_since(provider, since_ms)
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            for candidate in self.runtime.account_candidates(provider) {
                let account = candidate.account;
                let mut windows = candidate
                    .quota
                    .map(|quota| quota.windows)
                    .unwrap_or_default();
                windows.sort_by_key(|window| window.window_minutes.unwrap_or(i64::MAX));
                let history = windows
                    .into_iter()
                    .map(|window| WindowHistory {
                        samples: self
                            .runtime
                            .monitor()
                            .history(&account, &window.id, since_ms),
                        window_id: window.id,
                        label: window.label,
                    })
                    .collect();
                let own: Vec<&TurnUsage> = turns
                    .iter()
                    .filter(|turn| turn.account == account.account)
                    .collect();
                accounts.push(AccountActivity {
                    tokens: self.account_tokens(&own),
                    history,
                    account,
                });
            }
        }
        UsageActivity { since_ms, accounts }
    }

    /// The accounts a chat's turn or a worker runs on now.
    async fn working_accounts(&self) -> HashSet<AccountRef> {
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        let tasks: Vec<_> = self.tasks_lock().values().cloned().collect();
        let mut working = HashSet::new();
        for conv in convs {
            if let Some(cli) = conv.working_cli().await {
                working.insert(cli.account.clone());
            }
        }
        for task in tasks {
            if let Some(cli) = task.cli().await {
                working.insert(cli.account.clone());
            }
        }
        working
    }

    /// Brigadier's tokens in `turns`, by model and by conversation, most first.
    fn account_tokens(&self, turns: &[&TurnUsage]) -> AccountTokens {
        let mut by_model: HashMap<&str, TokenCount> = HashMap::new();
        let mut by_conversation: HashMap<&str, (Option<&str>, i64)> = HashMap::new();
        for turn in turns {
            let tokens = turn.input + turn.cached_input + turn.output;
            let count = by_model.entry(&turn.model).or_insert_with(|| TokenCount {
                model: turn.model.clone(),
                tokens: 0,
                output_tokens: 0,
                turns: 0,
            });
            count.tokens += tokens;
            count.output_tokens += turn.output;
            count.turns += 1;
            if let Some(conversation) = &turn.conversation_id {
                let entry = by_conversation
                    .entry(conversation)
                    .or_insert((turn.project_id.as_deref(), 0));
                entry.1 += tokens;
            }
        }
        let mut by_model: Vec<TokenCount> = by_model.into_values().collect();
        by_model.sort_by_key(|count| std::cmp::Reverse(count.tokens));
        let mut by_conversation: Vec<(&str, (Option<&str>, i64))> =
            by_conversation.into_iter().collect();
        by_conversation.sort_by_key(|(_, (_, tokens))| std::cmp::Reverse(*tokens));
        AccountTokens {
            by_model,
            by_conversation: by_conversation
                .into_iter()
                .take(TOP_CONVERSATIONS)
                .map(|(id, (project, tokens))| {
                    let id = ConversationId(id.to_owned());
                    ConversationTokens {
                        title: self
                            .core
                            .conversation(&id)
                            .map_or_else(|_| "A removed conversation".into(), |c| c.title),
                        conversation_id: id,
                        project_id: project.map(|project| ProjectId(project.to_owned())),
                        tokens,
                    }
                })
                .collect(),
        }
    }

    /// Hand-offs of the last week, tasks waiting for quota, and conversations on a stand-in
    /// model, newest first.
    async fn routing_activity(&self, now: i64) -> Vec<RoutingActivity> {
        let mut activity = Vec::new();
        for conversation in self.core.catalog().conversations {
            if let Some(fallback) = &conversation.fallback {
                activity.push(RoutingActivity::Fallback {
                    conversation_id: conversation.id.clone(),
                    title: conversation.title.clone(),
                    fallback: fallback.clone(),
                });
            }
            if conversation.setup.is_none() {
                continue;
            }
            let Ok(tasks) = self.core.tasks(&conversation.id).await else {
                continue;
            };
            for task in tasks {
                if let Some(wait) = task.quota_wait.as_ref().filter(|_| !task.state.is_final()) {
                    activity.push(RoutingActivity::Waiting {
                        conversation_id: conversation.id.clone(),
                        task_id: task.id.clone(),
                        title: task.title.clone(),
                        reason: wait.reason.clone(),
                        resets_at_ms: wait.resets_at_ms,
                        since_ms: wait.since_ms,
                    });
                }
                for pair in task.attempts.windows(2) {
                    let (before, after) = (&pair[0], &pair[1]);
                    let (Some(end), Some(at_ms)) = (&before.end, before.ended_at_ms) else {
                        continue;
                    };
                    if now - at_ms > ACTIVITY_MS {
                        continue;
                    }
                    activity.push(RoutingActivity::Handoff {
                        conversation_id: conversation.id.clone(),
                        task_id: task.id.clone(),
                        title: task.title.clone(),
                        from: before.route.choice.clone(),
                        to: after.route.choice.clone(),
                        cause: cause(&before.route.choice, end),
                        at_ms,
                    });
                }
            }
        }
        activity.sort_by_key(|item| std::cmp::Reverse(activity_at(item)));
        activity
    }

    /// What routing would choose for each task category now, for the next task in `project`
    /// touching `areas`: the same question a submitted task asks, the next trial slot
    /// included; a review as if `author`'s vendor wrote the change. Starts nothing and takes
    /// no trial slot.
    pub async fn preview_routes(
        &self,
        project: Option<ProjectId>,
        areas: Vec<Area>,
        author: Option<ProviderKind>,
    ) -> Vec<RoutePreview> {
        let rankings = self.core.settings().routing_rankings;
        let mut previews = Vec::new();
        for category in TaskCategory::ALL {
            let (preview, trial_slot) = self
                .preview(&Ask {
                    category,
                    areas: &areas,
                    floor: brigadier_router::default_floor(category),
                    // An operator looks at the screen, whatever it is sent.
                    needs: brigadier_router::Needs {
                        image_input: category == TaskCategory::Operate,
                        ..brigadier_router::Needs::default()
                    },
                    pin: None,
                    hold_pin: false,
                    // A review of a change goes to another vendor than its author's.
                    avoid: author
                        .filter(|_| category == TaskCategory::Review)
                        .map(|provider| brigadier_router::Author {
                            provider,
                            model: None,
                        }),
                    distinct_from: Vec::new(),
                    exclude: &[],
                    project_id: project.as_ref(),
                    trial: Trial::Peek,
                })
                .await;
            let ranking_id = brigadier_router::ranking_for(
                &rankings,
                category,
                &areas,
                project.as_ref().map(|id| id.0.as_str()),
            )
            .map(|ranking| ranking.id.clone());
            let outcome = match preview.decision {
                brigadier_router::Decision::Run(routed) => {
                    let route = super::routing::route_from(routed);
                    RoutePreviewOutcome::Chosen {
                        choice: route.choice,
                        reason: route.reason,
                        explanation: route.explanation,
                    }
                }
                brigadier_router::Decision::Wait(waiting) => RoutePreviewOutcome::Wait {
                    reason: waiting.reason,
                    resets_at_ms: waiting.resets_at_ms,
                    rule: waiting.rule,
                    ranking: waiting.ranking,
                },
            };
            previews.push(RoutePreview {
                category,
                areas: areas.clone(),
                outcome,
                candidates: preview.candidates,
                ranking_id,
                places: preview.places,
                trial_slot,
            });
        }
        previews
    }
}

/// What balancing does about a hot provider, when another can take its work.
fn balancing(
    provider: ProviderKind,
    quota: &brigadier_router::ProviderQuota,
    all: &[brigadier_router::ProviderState],
) -> Option<String> {
    if !matches!(quota.heat, Heat::Hot | Heat::Limited) {
        return None;
    }
    let other = all.iter().find(|state| {
        state.provider != provider
            && state.logged_in
            && state
                .quota
                .as_ref()
                .is_none_or(|quota| matches!(quota.heat, Heat::Cool | Heat::Warm))
    })?;
    let hot = quota
        .windows
        .iter()
        .filter(|window| window.window.model.is_none())
        .max_by_key(|window| window.heat)
        .map_or("usage", |window| window.window.label.as_str());
    Some(match quota.heat {
        Heat::Limited => format!(
            "New work goes to {} until {}'s {hot} window resets",
            other.provider.label(),
            provider.label()
        ),
        _ => format!(
            "New work shifts to {} where it can while {}'s {hot} window runs hot",
            other.provider.label(),
            provider.label()
        ),
    })
}

/// Why a model handed its task on, in a few words.
fn cause(from: &ModelChoice, end: &AttemptEnd) -> String {
    format!(
        "{}: {}",
        super::fallback::model_label(from),
        super::fallback::end_reason(end)
    )
}

fn activity_at(item: &RoutingActivity) -> i64 {
    match item {
        RoutingActivity::Handoff { at_ms, .. } => *at_ms,
        RoutingActivity::Waiting { since_ms, .. } => *since_ms,
        RoutingActivity::Fallback { fallback, .. } => fallback.since_ms,
    }
}
