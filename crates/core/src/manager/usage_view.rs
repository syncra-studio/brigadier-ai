//! The Usage page's read.

use std::collections::{HashMap, HashSet};

use brigadier_providers::ProviderKind;

use super::SessionManager;
use crate::accounts::AccountRef;
use crate::model::{
    AccountActivity, AccountTokens, AccountUsage, ConversationId, ConversationTokens, ModelChoice,
    ProviderUsage, RoutingActivity, TokenCount, UsageActivity, UsageView, WindowHistory,
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
    /// Everything the Usage page shows.
    pub async fn usage_view(&self) -> Result<UsageView> {
        let now = now_ms();
        let inputs = self.routing_inputs(now).await;
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
            })
            .collect();
        Ok(UsageView {
            providers,
            activity: self.routing_activity(now).await,
            models: inputs.models,
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
                        project_id: project
                            .map(|project| crate::model::ProjectId(project.to_owned())),
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
