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
use crate::model::{
    ConversationId, ConversationKind, Environment, RequestContext, Setup, ThreadEdits,
    ThreadMetrics,
};
use crate::routing::{StepKind, StoredEdits, TurnUsage};
use crate::work::UserRequest;
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
}
