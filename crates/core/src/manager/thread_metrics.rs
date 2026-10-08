//! The session thread's metrics, to tune its instructions by (THREAD-PLAN.md Q13). Nothing is
//! held back by them; the Inspector shows them.
//!
//! - **Self-edits:** the commits on the session's branch marked with the
//!   [`super::prompts::THREAD_TRAILER`], from where the thread first looked at the branch, with
//!   the lines they added and removed. Counted again whenever the thread's commit scan sees the
//!   branch move, and on request when it moved since; the last count is kept in
//!   `routing.sqlite`, so a branch merged and removed still shows its numbers.
//! - **Context growth:** the thread's `turn_usage` rows (step `thread`), per user request: the
//!   context its first and last model call for the request read.

use std::collections::HashMap;
use std::path::PathBuf;

use brigadier_git::Oid;

use super::{SessionManager, blocking, git_error};
use crate::board::Board;
use crate::model::{
    ConversationId, ConversationKind, Environment, ProviderTokens, RequestContext, RequestSummary,
    Setup, StepTokens, ThreadEdits, ThreadMetrics,
};
use crate::routing::{StepKind, StoredEdits, TurnUsage};
use crate::work::{OrchestratorStepKind, RequestState, UserRequest};
use crate::{Error, Result, now_ms};

impl SessionManager {
    /// A session thread's metrics (the Inspector).
    pub async fn thread_metrics(&self, id: &ConversationId) -> Result<ThreadMetrics> {
        let conversation = self.core.conversation(id)?;
        if conversation.kind != ConversationKind::Session {
            return Err(Error::Invalid("only a session has a thread".into()));
        }
        let edits = self.count_thread_edits(id).await;
        let rows = match self.runtime.routing_store() {
            Some(store) => store
                .step_turns(id.0.clone(), StepKind::Thread)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let board = self.core.board(id).await?;
        let requests = request_contexts(&rows, &board.requests);
        let all = match self.runtime.routing_store() {
            Some(store) => store
                .conversation_turns(id.0.clone())
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let summaries = request_summaries(&all, &board);
        let live = self.convs_lock().get(id).cloned();
        let live_context = match live {
            Some(conv) => conv.context_used().await,
            None => None,
        };
        Ok(ThreadMetrics {
            conversation_id: id.clone(),
            edits,
            requests,
            context_tokens: live_context.or_else(|| rows.last().map(context_of_row)),
            summaries,
        })
    }

    /// The thread's own commits on the session's branch: counted again when the branch moved
    /// since they were last counted, else as kept.
    pub(crate) async fn count_thread_edits(&self, id: &ConversationId) -> Option<ThreadEdits> {
        let store = self.runtime.routing_store()?.clone();
        let stored = store.thread_edits(id.0.clone()).await.ok().flatten();
        let kept = |stored: Option<StoredEdits>| stored.map(|edits| shown(edits, true));
        let Some((repo, branch)) = self.session_branch(id) else {
            return kept(stored);
        };
        let start = self
            .core
            .board(id)
            .await
            .ok()
            .and_then(|board| board.thread_starts.get(&branch).cloned());
        let Some(start) = start else {
            return kept(stored);
        };
        let (key, value) = super::prompts::THREAD_TRAILER
            .split_once(": ")
            .unwrap_or_default();
        let known = stored.clone();
        let (git, name) = (self.git.clone(), branch.clone());
        let counted = blocking(move || {
            let repo = git.open(&repo).map_err(git_error)?;
            let Some(tip) = repo.branch_tip(&name).map_err(git_error)? else {
                return Ok(None);
            };
            if let Some(known) = known.filter(|known| known.branch == name && known.tip == tip.0) {
                return Ok(Some((known, false)));
            }
            let start = Oid(start);
            // A rewritten history counts from where the two still meet.
            let base = if repo.ancestor(&start, &tip).unwrap_or(false) {
                start
            } else {
                repo.merge_base(&start, &tip).map_err(git_error)?
            };
            let stat = repo
                .trailer_stat(&base, &tip, key, value)
                .map_err(git_error)?;
            Ok(Some((
                StoredEdits {
                    branch: name,
                    base: base.0,
                    tip: tip.0,
                    commits: stat.commits,
                    added: i64::try_from(stat.added).unwrap_or(i64::MAX),
                    removed: i64::try_from(stat.removed).unwrap_or(i64::MAX),
                    at_ms: now_ms(),
                },
                true,
            )))
        })
        .await;
        match counted {
            Ok(Some((edits, fresh))) => {
                if fresh && let Err(err) = store.put_thread_edits(id.0.clone(), edits.clone()).await
                {
                    tracing::warn!(conversation = %id, error = %err, "could not keep the thread's edit count");
                }
                Some(shown(edits, false))
            }
            // The branch is gone: what was counted last stands.
            Ok(None) => kept(stored),
            Err(err) => {
                tracing::debug!(conversation = %id, error = %err, "could not count the thread's edits");
                kept(stored)
            }
        }
    }

    /// The session's repository and its own branch (an overnight run's is merged into it).
    fn session_branch(&self, id: &ConversationId) -> Option<(PathBuf, String)> {
        let conversation = self.core.conversation(id).ok()?;
        let Some(Setup::Session {
            repo, environment, ..
        }) = conversation.setup
        else {
            return None;
        };
        let branch = match environment {
            Environment::LocalCheckout { branch } | Environment::NewWorktree { branch, .. } => {
                branch
            }
        };
        Some((PathBuf::from(repo), branch))
    }
}

fn shown(edits: StoredEdits, kept: bool) -> ThreadEdits {
    ThreadEdits {
        branch: edits.branch,
        commits: edits.commits,
        added: edits.added,
        removed: edits.removed,
        at_ms: edits.at_ms,
        kept,
    }
}

/// The context a thread row's latest model call read: as the CLI said it apart, else its
/// input, cached input and cache writes (one Codex call; rows stored before it was kept).
fn context_of_row(row: &TurnUsage) -> i64 {
    row.context
        .unwrap_or(row.input + row.cached_input + row.cache_write)
}

/// The thread's context per request, from its rows (oldest first): the first and last row
/// for each request, in the order the requests first used the thread. Rows of no request
/// (stored before requests were kept) are left out.
pub(crate) fn request_contexts(
    rows: &[TurnUsage],
    requests: &HashMap<String, UserRequest>,
) -> Vec<RequestContext> {
    let mut order: Vec<String> = Vec::new();
    let mut spans: HashMap<&str, (u32, i64, i64)> = HashMap::new();
    for row in rows {
        let Some(request) = row.request_id.as_deref() else {
            continue;
        };
        let context = context_of_row(row);
        spans
            .entry(request)
            .and_modify(|(calls, _, last)| {
                *calls += 1;
                *last = context;
            })
            .or_insert_with(|| {
                order.push(request.to_owned());
                (1, context, context)
            });
    }
    order
        .into_iter()
        .filter_map(|request_id| {
            let (calls, first, last) = *spans.get(request_id.as_str())?;
            let known = requests.get(&request_id);
            Some(RequestContext {
                preview: known.map(|r| r.preview.clone()).unwrap_or_default(),
                started_at_ms: known.map_or(0, |r| r.started_at_ms),
                request_id,
                calls,
                first_tokens: first,
                last_tokens: last,
                growth_tokens: last - first,
            })
        })
        .collect()
}

/// Each request's time and tokens (THREAD-PLAN.md phase 4), oldest first, from all of the
/// conversation's turn rows (`rows`, oldest first) and its board.
pub(crate) fn request_summaries(rows: &[TurnUsage], board: &Board) -> Vec<RequestSummary> {
    let mut requests: Vec<&UserRequest> = board.requests.values().collect();
    requests.sort_by_key(|request| request.started_at_ms);
    requests
        .into_iter()
        .map(|request| {
            let id = request.id.as_str();
            let start = request.started_at_ms;
            let after = |at: i64| (at - start).max(0);
            let ours: Vec<&TurnUsage> = rows
                .iter()
                .filter(|row| row.request_id.as_deref() == Some(id))
                .collect();
            let steps: Vec<_> = board
                .orchestrator_steps
                .iter()
                .filter(|step| step.request_id.as_deref() == Some(id))
                .collect();
            let first_thread_call = ours
                .iter()
                .filter(|row| row.step == Some(StepKind::Thread))
                .map(|row| row.at_ms)
                .min();
            let first_step = steps.iter().map(|step| step.at_ms).min();
            let first_event_ms = [first_step, first_thread_call].into_iter().flatten().min();
            let working = matches!(request.state, RequestState::Working);
            let answer_ms = request.ended_at_ms.filter(|_| !working);
            let landed_ms = steps
                .iter()
                .filter(|step| matches!(step.kind, OrchestratorStepKind::Landed { .. }))
                .map(|step| step.at_ms)
                .max();
            let settled_ms = answer_ms.map(|answer| {
                ours.iter()
                    .map(|row| row.at_ms)
                    .chain(steps.iter().map(|step| step.at_ms))
                    .fold(answer, i64::max)
            });
            RequestSummary {
                request_id: request.id.clone(),
                preview: request.preview.clone(),
                started_at_ms: start,
                first_event_ms: first_event_ms.map(after),
                answer_ms: answer_ms.map(after),
                landed_ms: landed_ms.map(after),
                settled_ms: settled_ms.map(after),
                providers: provider_tokens(&ours),
                steps: step_tokens(&ours),
            }
        })
        .collect()
}

fn provider_tokens(rows: &[&TurnUsage]) -> Vec<ProviderTokens> {
    let mut by: Vec<ProviderTokens> = Vec::new();
    for row in rows {
        let at = match by.iter().position(|known| known.provider == row.provider) {
            Some(at) => at,
            None => {
                by.push(ProviderTokens {
                    provider: row.provider,
                    input: 0,
                    cached_input: 0,
                    cache_write: 0,
                    output: 0,
                    raw: 0,
                    raw_without_cache_reads: 0,
                    cost_usd: None,
                });
                by.len() - 1
            }
        };
        let tokens = &mut by[at];
        tokens.input += row.input;
        tokens.cached_input += row.cached_input;
        tokens.cache_write += row.cache_write;
        tokens.output += row.output;
        tokens.raw += row.total();
        tokens.raw_without_cache_reads += row.total() - row.cached_input;
        if let Some(cost) = row.cost_usd {
            tokens.cost_usd = Some(tokens.cost_usd.unwrap_or(0.0) + cost);
        }
    }
    by.sort_by_key(|tokens| std::cmp::Reverse(tokens.raw));
    by
}

fn step_tokens(rows: &[&TurnUsage]) -> Vec<StepTokens> {
    let mut by: Vec<StepTokens> = Vec::new();
    for row in rows {
        let step = row.step.map_or("unknown", StepKind::as_str);
        match by.iter_mut().find(|known| known.step == step) {
            Some(known) => {
                known.raw += row.total();
                known.calls += 1;
            }
            None => by.push(StepTokens {
                step: step.to_owned(),
                raw: row.total(),
                calls: 1,
            }),
        }
    }
    by.sort_by_key(|tokens| std::cmp::Reverse(tokens.raw));
    by
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::ProviderKind;

    fn row(request: Option<&str>, context: Option<i64>, input: i64, cached: i64) -> TurnUsage {
        TurnUsage {
            at_ms: 0,
            provider: ProviderKind::Codex,
            model: "m".into(),
            conversation_id: Some("c".into()),
            project_id: None,
            task_id: None,
            input,
            cached_input: cached,
            cache_write: 0,
            output: 50,
            step: Some(StepKind::Thread),
            duration_ms: None,
            request_id: request.map(str::to_owned),
            context,
            child_thread: None,
            cost_usd: None,
            account: None,
        }
    }

    #[test]
    fn the_threads_context_grows_per_request_from_its_first_call_to_its_last() {
        let request: UserRequest = serde_json::from_value(serde_json::json!({
            "id": "r1",
            "conversationId": "c",
            "preview": "Fix the login",
            "state": { "type": "done" },
            "startedAtMs": 7,
            "endedAtMs": null,
        }))
        .expect("a request");
        let requests = HashMap::from([("r1".to_owned(), request)]);
        let rows = [
            // Stored before requests were kept: left out.
            row(None, None, 1, 1),
            // r1: Codex calls (no context said apart) from 10k to 14k.
            row(Some("r1"), None, 2_000, 8_000),
            row(Some("r1"), None, 1_000, 11_000),
            // r2 (gone since) interleaves: a Claude turn, whose context came apart.
            row(Some("r2"), Some(20_000), 9_000, 90_000),
            row(Some("r1"), None, 500, 13_500),
        ];
        let contexts = request_contexts(&rows, &requests);
        assert_eq!(
            contexts,
            [
                RequestContext {
                    request_id: "r1".into(),
                    preview: "Fix the login".into(),
                    started_at_ms: 7,
                    calls: 3,
                    first_tokens: 10_000,
                    last_tokens: 14_000,
                    growth_tokens: 4_000,
                },
                RequestContext {
                    request_id: "r2".into(),
                    preview: String::new(),
                    started_at_ms: 0,
                    calls: 1,
                    first_tokens: 20_000,
                    last_tokens: 20_000,
                    growth_tokens: 0,
                },
            ]
        );
    }

    #[test]
    fn a_requests_summary_counts_everything_it_started_per_provider_and_step() {
        use crate::work::{OrchestratorStep, TaskId};
        let request: UserRequest = serde_json::from_value(serde_json::json!({
            "id": "r1",
            "conversationId": "c",
            "preview": "Fix the login",
            "state": { "type": "done" },
            "startedAtMs": 1_000,
            "endedAtMs": 61_000,
        }))
        .expect("a request");
        let working: UserRequest = serde_json::from_value(serde_json::json!({
            "id": "r2",
            "conversationId": "c",
            "preview": "And the logout",
            "state": { "type": "working" },
            "startedAtMs": 70_000,
            "endedAtMs": null,
        }))
        .expect("a request");
        let mut board = Board::default();
        board.requests.insert("r1".into(), request);
        board.requests.insert("r2".into(), working);
        let step = |at_ms, kind| OrchestratorStep {
            request_id: Some("r1".into()),
            kind,
            at_ms,
            position: 0,
        };
        board.orchestrator_steps = vec![
            step(
                3_500,
                OrchestratorStepKind::Created {
                    task_id: TaskId("t1".into()),
                },
            ),
            step(
                50_000,
                OrchestratorStepKind::Landed {
                    task_ids: vec![TaskId("t1".into())],
                    commits: 1,
                    branch: "main".into(),
                    head: "abc".into(),
                },
            ),
        ];
        let at = |mut row: TurnUsage, at_ms, step, provider, cost| {
            row.at_ms = at_ms;
            row.step = Some(step);
            row.provider = provider;
            row.cost_usd = cost;
            row
        };
        let rows = [
            at(
                row(Some("r1"), None, 100, 1_000),
                3_000,
                StepKind::Thread,
                ProviderKind::Claude,
                Some(0.25),
            ),
            at(
                row(Some("r1"), None, 200, 2_000),
                40_000,
                StepKind::Worker,
                ProviderKind::Codex,
                None,
            ),
            at(
                row(Some("r1"), None, 10, 100),
                20_000,
                StepKind::Thread,
                ProviderKind::Claude,
                Some(0.5),
            ),
            // A review that finished after the answer.
            at(
                row(Some("r1"), None, 5, 0),
                75_000,
                StepKind::Review,
                ProviderKind::Codex,
                None,
            ),
            at(
                row(Some("r2"), None, 1, 1),
                71_000,
                StepKind::Thread,
                ProviderKind::Claude,
                None,
            ),
        ];
        let summaries = request_summaries(&rows, &board);
        assert_eq!(summaries.len(), 2);
        let done = &summaries[0];
        assert_eq!(done.request_id, "r1");
        assert_eq!(done.first_event_ms, Some(2_000));
        assert_eq!(done.answer_ms, Some(60_000));
        assert_eq!(done.landed_ms, Some(49_000));
        assert_eq!(done.settled_ms, Some(74_000));
        assert_eq!(
            done.providers,
            [
                ProviderTokens {
                    provider: ProviderKind::Codex,
                    input: 205,
                    cached_input: 2_000,
                    cache_write: 0,
                    output: 100,
                    raw: 2_305,
                    raw_without_cache_reads: 305,
                    cost_usd: None,
                },
                ProviderTokens {
                    provider: ProviderKind::Claude,
                    input: 110,
                    cached_input: 1_100,
                    cache_write: 0,
                    output: 100,
                    raw: 1_310,
                    raw_without_cache_reads: 210,
                    cost_usd: Some(0.75),
                },
            ]
        );
        assert_eq!(
            done.steps,
            [
                StepTokens {
                    step: "worker".into(),
                    raw: 2_250,
                    calls: 1
                },
                StepTokens {
                    step: "thread".into(),
                    raw: 1_310,
                    calls: 2
                },
                StepTokens {
                    step: "review".into(),
                    raw: 55,
                    calls: 1
                },
            ]
        );
        let still = &summaries[1];
        assert_eq!(still.first_event_ms, Some(1_000));
        assert_eq!(
            (still.answer_ms, still.settled_ms, still.landed_ms),
            (None, None, None)
        );
    }
}
