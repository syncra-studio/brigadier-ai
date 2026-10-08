//! Brigadier's own token use, per turn, for the Usage page and the Inspector.

use std::time::{Duration, UNIX_EPOCH};

use brigadier_providers::{ProviderKind, TokenUsage};

use super::SessionManager;
use super::conversation::Cli;
use crate::model::{ConversationId, ConversationKind, ProjectId};
use crate::now_ms;
use crate::routing::{StepKind, TokenMeter, TurnUsage};
use crate::work::TaskId;

/// Whose turn a token report belongs to.
pub(crate) enum TokenOwner<'a> {
    /// A conversation's model (a session's thread, or a Chat).
    Conversation(&'a ConversationId),
    /// The fork of a session's thread that writes its rebirth handoff.
    Handoff(&'a ConversationId),
    /// A task's worker, or a review (counted under `review:<id>`).
    Task(&'a ConversationId, &'a TaskId),
    /// A Brain job of a project.
    Project(&'a ProjectId),
    /// Brigadier's own upkeep for no conversation or project (researching a new model).
    Upkeep,
}

/// Rollouts written this long before the last look for child threads are looked at again: a
/// file's time and the clock's may differ a little. Metering a child twice counts nothing twice.
const CHILD_LOOK_SLACK: Duration = Duration::from_secs(10);

impl SessionManager {
    /// Records what the turn just reported used, from `meter`'s view of the CLI session's
    /// running totals. Awaited, so an outcome recorded next counts it.
    pub(crate) async fn note_tokens(
        &self,
        meter: &TokenMeter,
        provider: ProviderKind,
        model: Option<&str>,
        owner: TokenOwner<'_>,
        total: &TokenUsage,
        last: Option<&TokenUsage>,
    ) {
        self.note_use(meter, provider, model, owner, total, last, None)
            .await;
    }

    /// The same, with the context the turn's latest model call read when the CLI said it
    /// apart (Claude reports a turn's use summed over its calls).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn note_use(
        &self,
        meter: &TokenMeter,
        provider: ProviderKind,
        model: Option<&str>,
        owner: TokenOwner<'_>,
        total: &TokenUsage,
        last: Option<&TokenUsage>,
        context: Option<i64>,
    ) {
        let Some(used) = meter.delta(total, last) else {
            return;
        };
        let at_ms = now_ms();
        let duration_ms = meter.took(at_ms);
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let mut turn = self.turn_for(&owner, provider, model, at_ms).await;
        turn.account = meter.account().map(str::to_owned);
        turn.input = used.input_tokens;
        turn.cached_input = used.cached_input_tokens;
        turn.cache_write = used.cache_write_tokens;
        turn.output = used.output_tokens;
        turn.cost_usd = used.cost_usd;
        turn.duration_ms = duration_ms;
        turn.context = last.map(context_of).or(context);
        if let Err(err) = store.add_turn(turn).await {
            tracing::warn!(error = %err, "could not record a turn's token use");
        }
    }

    /// A row for `owner`'s use at `at_ms`, with no tokens yet: who, which step, which request.
    async fn turn_for(
        &self,
        owner: &TokenOwner<'_>,
        provider: ProviderKind,
        model: Option<&str>,
        at_ms: i64,
    ) -> TurnUsage {
        let conversation = |id: &ConversationId| self.core.conversation(id).ok();
        let (conversation_id, project_id, task_id, step, request_id) = match *owner {
            TokenOwner::Conversation(id) => {
                let conversation = conversation(id);
                let step = match conversation.as_ref().map(|c| c.kind) {
                    Some(ConversationKind::Chat) => StepKind::Chat,
                    _ => StepKind::Thread,
                };
                (
                    Some(id.0.clone()),
                    conversation.and_then(|c| c.project_id).map(|p| p.0),
                    None,
                    step,
                    self.thread_request(id).await,
                )
            }
            TokenOwner::Handoff(id) => (
                Some(id.0.clone()),
                conversation(id).and_then(|c| c.project_id).map(|p| p.0),
                None,
                StepKind::Handoff,
                None,
            ),
            TokenOwner::Task(id, task) => {
                let escalation = task.0.starts_with(super::escalation::ESCALATION_OWNER);
                let review = task.0.starts_with("review:") || escalation;
                (
                    Some(id.0.clone()),
                    conversation(id).and_then(|c| c.project_id).map(|p| p.0),
                    Some(task.0.clone()),
                    if escalation {
                        StepKind::Escalation
                    } else if review {
                        StepKind::Review
                    } else {
                        StepKind::Worker
                    },
                    // The task's own, read from its board: a worker's events must not wait
                    // on the conversation's state, which a thread turn may hold meanwhile.
                    if review {
                        None
                    } else {
                        self.task_by_id(id, task)
                            .await
                            .ok()
                            .and_then(|task| task.request_id)
                    },
                )
            }
            TokenOwner::Project(project) => {
                (None, Some(project.0.clone()), None, StepKind::Brain, None)
            }
            TokenOwner::Upkeep => (None, None, None, StepKind::Upkeep, None),
        };
        TurnUsage {
            at_ms,
            provider,
            model: model.unwrap_or("default").to_owned(),
            conversation_id,
            project_id,
            task_id,
            input: 0,
            cached_input: 0,
            cache_write: 0,
            output: 0,
            step: Some(step),
            duration_ms: None,
            request_id,
            context: None,
            child_thread: None,
            cost_usd: None,
            account: None,
        }
    }

    /// The request a conversation's running turn serves, without making its live state.
    async fn thread_request(&self, id: &ConversationId) -> Option<String> {
        let conv = self.convs_lock().get(id).cloned()?;
        conv.running_request().await
    }

    /// Meters the Codex child threads of `cli`'s thread (a worker's or a session thread's) at
    /// its turn's end. Under Approve for me Codex's auto-review (the "guardian") runs in a
    /// child thread whose use the parent's totals leave out; only its rollout holds it. Each
    /// child's use is recorded once, as [`StepKind::Guardian`] rows marked with the child's
    /// id, whichever turn's end sees it grow.
    pub(crate) async fn meter_child_threads(&self, cli: &Cli, owner: TokenOwner<'_>) {
        if cli.provider != ProviderKind::Codex {
            return;
        }
        // Scripted CLIs have no rollouts; the user's own are never read in tests.
        #[cfg(test)]
        if self.runtime.faked() {
            return;
        }
        let Some(sessions) = brigadier_review::codex_sessions(self.runtime.cli_env()) else {
            return;
        };
        let Some(store) = self.runtime.routing_store().cloned() else {
            return;
        };
        let thread = cli.session.native_id();
        if thread.is_empty() {
            return;
        }
        let since = cli
            .meter
            .children_looked(now_ms())
            .and_then(|at| u64::try_from(at).ok())
            .map(|at| UNIX_EPOCH + Duration::from_millis(at))
            .and_then(|at| at.checked_sub(CHILD_LOOK_SLACK));
        let children = tokio::task::spawn_blocking(move || {
            brigadier_review::child_threads_since(&sessions, &thread, since)
        })
        .await
        .unwrap_or_default();
        let mut row = self
            .turn_for(&owner, cli.provider, cli.model.model.as_deref(), now_ms())
            .await;
        row.account = cli.meter.account().map(str::to_owned);
        meter_children(&store, row, children).await;
    }
}

/// Records what each of `children` used since last metered, as rows like `row`.
pub(crate) async fn meter_children(
    store: &std::sync::Arc<crate::routing::RoutingStore>,
    row: TurnUsage,
    children: Vec<brigadier_review::ChildThread>,
) {
    for child in children {
        let Some(usage) = child.usage else {
            continue;
        };
        let mut turn = row.clone();
        turn.step = Some(StepKind::Guardian);
        // Its own model isn't in its rollout's first line; the parent's stands in.
        turn.input = usage.input_tokens;
        turn.cached_input = usage.cached_input_tokens;
        turn.cache_write = usage.cache_write_tokens;
        turn.output = usage.output_tokens;
        turn.duration_ms = None;
        turn.context = None;
        if let Err(err) = store.meter_child_thread(child.id, turn).await {
            tracing::warn!(error = %err, "could not record a Codex child thread's token use");
        }
    }
}

/// The context a model call read: its input, cached input and cache writes.
pub(crate) fn context_of(usage: &TokenUsage) -> i64 {
    usage.input_tokens + usage.cached_input_tokens + usage.cache_write_tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::RoutingStore;

    fn token_count(input: i64, cached: i64, output: i64) -> String {
        format!(
            r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":0}}}}}}}}"#
        )
    }

    #[tokio::test]
    async fn a_workers_auto_review_is_metered_once_across_its_turn_ends() {
        let dir = std::env::temp_dir().join(format!("brigadier-guardian-{}", uuid::Uuid::new_v4()));
        let day = dir.join("sessions/2026/10/07");
        std::fs::create_dir_all(&day).unwrap();
        let store = RoutingStore::open(&dir.join("routing.sqlite")).unwrap();
        let parent = "01a1162f-4845";
        std::fs::write(
            day.join(format!("rollout-2026-10-07T14-40-00-{parent}.jsonl")),
            format!(r#"{{"type":"session_meta","payload":{{"id":"{parent}"}}}}"#),
        )
        .unwrap();
        let guardian = day.join("rollout-2026-10-07T14-48-07-01a11631-3bbe.jsonl");
        let mut lines = vec![
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"01a11631-3bbe","parent_thread_id":"{parent}","thread_source":"guardian_review"}}}}"#
            ),
            token_count(30_000, 20_000, 300),
        ];
        std::fs::write(&guardian, lines.join("\n")).unwrap();
        let row = TurnUsage {
            at_ms: 1,
            provider: ProviderKind::Codex,
            model: "gpt".into(),
            conversation_id: Some("c1".into()),
            project_id: None,
            task_id: Some("t1".into()),
            input: 0,
            cached_input: 0,
            cache_write: 0,
            output: 0,
            step: Some(StepKind::Worker),
            duration_ms: Some(5),
            request_id: Some("r1".into()),
            context: Some(9),
            child_thread: None,
            cost_usd: None,
            account: None,
        };
        let sessions = dir.join("sessions");
        let turn_end = || async {
            let children = brigadier_review::child_threads(&sessions, parent);
            meter_children(&store, row.clone(), children).await;
        };
        // The first turn end meters what it used so far; the second, with nothing new, adds
        // nothing; the third, after it reviewed again, only what is new.
        turn_end().await;
        turn_end().await;
        lines.push(token_count(117_603, 90_880, 641));
        std::fs::write(&guardian, lines.join("\n")).unwrap();
        turn_end().await;
        let turns = store.turns_since(ProviderKind::Codex, 0).await.unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let shown: Vec<_> = turns
            .iter()
            .map(|turn| {
                (
                    turn.step,
                    turn.child_thread.as_deref(),
                    turn.task_id.as_deref(),
                    turn.input,
                    turn.cached_input,
                    turn.output,
                    turn.context,
                )
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    Some(StepKind::Guardian),
                    Some("01a11631-3bbe"),
                    Some("t1"),
                    10_000,
                    20_000,
                    300,
                    None
                ),
                (
                    Some(StepKind::Guardian),
                    Some("01a11631-3bbe"),
                    Some("t1"),
                    16_723,
                    70_880,
                    341,
                    None
                ),
            ]
        );
        // Together, its whole use: 117,603 input (90,880 of it cached) and 641 output.
        let total: i64 = turns.iter().map(TurnUsage::total).sum();
        assert_eq!(total, 117_603 + 641);
    }
}
