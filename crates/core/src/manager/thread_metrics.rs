//! The session thread's metrics, to tune its instructions by (THREAD-PLAN.md Q13). Nothing is
//! held back by them; the Inspector shows them.
//!
//! - **Self-edits:** the thread's commits on the session's branch, from where it first looked
//!   at the branch, with the lines they added and removed: those marked with the
//!   [`super::prompts::THREAD_TRAILER`], and those the store records as its own without it
//!   (`ThreadCommitsSeen.authored_from`, the user's own checkout). On a local checkout its
//!   uncommitted changes count too: what its requests changed in the files not committed yet. Counted again whenever the thread's commit scan sees the
//!   branch move, and on request when it moved since; the last count is kept in
//!   `routing.sqlite`, so a branch merged and removed still shows its numbers.
//! - **Context growth:** the thread's `turn_usage` rows (step `thread`), per user request: the
//!   context its first and last model call for the request read.

use std::collections::HashMap;
use std::path::PathBuf;

use brigadier_git::Oid;

use super::{SessionManager, blocking, git_error};
use crate::board::{Authored, Board};
use crate::model::{
    CacheCauses, CauseTokens, ConversationId, ConversationKind, Environment, ProviderTokens,
    RequestContext, RequestEnd, RequestSummary, Setup, StepTokens, ThreadEdits, ThreadMetrics,
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
        self.count_thread_edits_again(id, false).await
    }

    /// The same, counted again anyway when `again` (the store just recorded commits as the
    /// thread's: a count at the same tip from before left them out).
    pub(crate) async fn count_thread_edits_again(
        &self,
        id: &ConversationId,
        again: bool,
    ) -> Option<ThreadEdits> {
        let store = self.runtime.routing_store()?.clone();
        let stored = store.thread_edits(id.0.clone()).await.ok().flatten();
        let kept = |stored: Option<StoredEdits>| stored.map(|edits| shown(edits, true));
        let Some((repo, branch)) = self.session_branch(id) else {
            return kept(stored);
        };
        let Ok(board) = self.core.board(id).await else {
            return kept(stored);
        };
        let Some(start) = board.thread_starts.get(&branch).cloned() else {
            return kept(stored);
        };
        // On the user's own checkout, commits it made without the trailer (elsewhere the
        // trailer says it exactly: a range the store records may hold others' commits too),
        // and the files over each request.
        let local = matches!(
            self.core.conversation(id).ok().and_then(|c| c.setup),
            Some(Setup::Session {
                environment: Environment::LocalCheckout { .. },
                ..
            })
        );
        let (mut authored, mut windows) = (Vec::new(), Vec::new());
        for known in &board.thread_authored {
            match known {
                Authored::Commits {
                    branch: on,
                    from,
                    tip,
                } if local && *on == branch => {
                    authored.push((Oid(from.clone()), Oid(tip.clone())));
                }
                Authored::Files {
                    branch: on,
                    start,
                    tip,
                    ..
                } if *on == branch => windows.push((Oid(start.clone()), Oid(tip.clone()))),
                _ => {}
            }
        }
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
            // Uncommitted changes don't move the tip: they are added to the kept count each time.
            let pending = uncommitted(&repo, &tip, &windows);
            // Commits the store records as its own are looked up each time: few, and a count
            // kept before they were recorded left them out.
            if let Some(known) = known.filter(|known| {
                !again && authored.is_empty() && known.branch == name && known.tip == tip.0
            }) {
                return Ok(Some((known, false, pending)));
            }
            let start = Oid(start);
            // A rewritten history counts from where the two still meet.
            let base = if repo.ancestor(&start, &tip).unwrap_or(false) {
                start
            } else {
                repo.merge_base(&start, &tip).map_err(git_error)?
            };
            let stat = repo
                .trailer_stat(&base, &tip, key, value, &authored)
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
                pending,
            )))
        })
        .await;
        match counted {
            Ok(Some((edits, fresh, (plus, minus)))) => {
                if fresh && let Err(err) = store.put_thread_edits(id.0.clone(), edits.clone()).await
                {
                    tracing::warn!(conversation = %id, error = %err, "could not keep the thread's edit count");
                }
                let mut edits = shown(edits, false);
                edits.added = edits.added.saturating_add(plus);
                edits.removed = edits.removed.saturating_add(minus);
                Some(edits)
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

/// Requests that changed files looked at for the thread's uncommitted changes, newest first.
const PENDING_REQUESTS: usize = 50;

/// The thread's uncommitted changes on a local checkout whose branch is at `tip`, as it last
/// left the files: in each path that still differs from `tip` in its newest snapshot, what its
/// requests changed there, each from its own start (so the user's changes before or after a
/// request, and paths only the user changed, stay out), back to the last request that left
/// nothing uncommitted. A path committed since a request ended is no longer its pending change.
/// `windows`: snapshots of the files when each request started and as last looked at, oldest
/// first (a snapshot is `HEAD` itself when nothing differs from it, else a child of it).
/// Nothing while another branch is checked out; a snapshot git has pruned since counts none.
fn uncommitted(repo: &brigadier_git::Repo, tip: &Oid, windows: &[(Oid, Oid)]) -> (i64, i64) {
    let Some((_, newest)) = windows.last() else {
        return (0, 0);
    };
    if newest == tip || repo.resolve("HEAD").ok().as_ref() != Some(tip) {
        return (0, 0);
    }
    let paths = |from: &Oid, to: &Oid| -> std::collections::HashSet<String> {
        repo.diff_stat(from, to)
            .map(|diff| diff.files.into_iter().map(|file| file.path).collect())
            .unwrap_or_default()
    };
    let mut pending = paths(tip, newest);
    // A snapshot that is a commit: nothing was uncommitted then.
    let clean = |snapshot: &Oid| snapshot == tip || repo.ancestor(snapshot, tip).unwrap_or(false);
    let head_of = |snapshot: &Oid| repo.resolve(&format!("{}^", snapshot.0)).ok();
    let mut since: Option<Oid> = None;
    let (mut plus, mut minus, mut looked) = (0, 0, 0);
    for (start, end) in windows.iter().rev() {
        // Left with nothing uncommitted: what came before was committed or undone.
        if clean(end) {
            break;
        }
        let started_clean = clean(start);
        if start != end {
            looked += 1;
            if looked > PENDING_REQUESTS {
                break;
            }
            let Some(head) = head_of(end) else {
                break;
            };
            if since.as_ref() != Some(&head) {
                for path in paths(&head, tip) {
                    pending.remove(&path);
                }
                since = Some(head.clone());
            }
            if pending.is_empty() {
                break;
            }
            let ours: std::collections::HashSet<String> =
                paths(start, end).intersection(&pending).cloned().collect();
            // A request that committed on the way: only what it changed after its commit.
            let start_head = if started_clean {
                Some(start.clone())
            } else {
                head_of(start)
            };
            let from = if start_head.as_ref() == Some(&head) {
                start
            } else {
                &head
            };
            if !ours.is_empty()
                && let Ok(diff) = repo.diff_stat(from, end)
            {
                for file in diff.files.iter().filter(|file| ours.contains(&file.path)) {
                    plus += i64::from(file.insertions);
                    minus += i64::from(file.deletions);
                }
            }
        }
        if started_clean {
            break;
        }
    }
    (plus, minus)
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
        let calls = model_calls(row);
        spans
            .entry(request)
            .and_modify(|(counted, _, last)| {
                *counted = counted.saturating_add(calls);
                *last = context;
            })
            .or_insert_with(|| {
                order.push(request.to_owned());
                (calls, context, context)
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
            // Time Brigadier was closed while a quit had cut it off doesn't count.
            let closed = |at: i64| -> i64 {
                request
                    .closed
                    .iter()
                    .map(|span| {
                        let to = span.to_ms.unwrap_or(at).min(at);
                        (to - span.from_ms.max(start)).max(0)
                    })
                    .sum()
            };
            let after = |at: i64| (at - start - closed(at)).max(0);
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
            let ended = match &request.state {
                RequestState::Stopped => Some(RequestEnd::Stopped),
                state if super::conversation::interrupted_by_quit(state) => {
                    Some(RequestEnd::Interrupted)
                }
                RequestState::Failed { .. } => Some(RequestEnd::Failed),
                _ => None,
            };
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
                ended,
                landed_ms: landed_ms.map(after),
                settled_ms: settled_ms.map(after),
                providers: provider_tokens(&ours),
                steps: step_tokens(&ours),
                cache: cache_causes(&ours),
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
                    cache_write_1h: 0,
                    cache_write_5m: 0,
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
        tokens.cache_write_1h += row.cache.write_1h;
        tokens.cache_write_5m += row.cache.write_5m;
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

/// Where the rows' cache writes came from: the estimated miss in all, each known cause's
/// sessions with what their first call wrote and missed, and the miss left unexplained.
pub(crate) fn cache_causes(rows: &[&TurnUsage]) -> CacheCauses {
    let mut causes: Vec<CauseTokens> = Vec::new();
    let mut summary = CacheCauses::default();
    for row in rows {
        let trace = &row.cache;
        summary.missed += trace.missed;
        summary.big_misses += u32::try_from(trace.missed_calls).unwrap_or(u32::MAX);
        summary.unestimated_calls += u32::try_from(trace.unestimated_calls).unwrap_or(u32::MAX);
        let Some(cause) = trace.cause else {
            continue;
        };
        let name = cause.as_str();
        match causes.iter_mut().find(|known| known.cause == name) {
            Some(known) => {
                known.sessions += 1;
                known.written += trace.cause_write;
                known.missed += trace.cause_missed;
            }
            None => causes.push(CauseTokens {
                cause: name.to_owned(),
                sessions: 1,
                written: trace.cause_write,
                missed: trace.cause_missed,
            }),
        }
    }
    let explained: i64 = causes.iter().map(|cause| cause.missed).sum();
    summary.unexplained = (summary.missed - explained).max(0);
    causes.sort_by_key(|cause| std::cmp::Reverse(cause.written + cause.missed));
    summary.causes = causes;
    summary
}

/// The model calls a row covers; one stored before they were counted counts as one.
fn model_calls(row: &TurnUsage) -> u32 {
    row.model_calls
        .map_or(1, |calls| u32::try_from(calls).unwrap_or(u32::MAX))
}

fn step_tokens(rows: &[&TurnUsage]) -> Vec<StepTokens> {
    let mut by: Vec<StepTokens> = Vec::new();
    for row in rows {
        let step = row.step.map_or("unknown", StepKind::as_str);
        let calls = model_calls(row);
        let turns = u32::from(row.opens_turn);
        match by.iter_mut().find(|known| known.step == step) {
            Some(known) => {
                known.raw += row.total();
                known.calls += calls;
                known.turns += turns;
            }
            None => by.push(StepTokens {
                step: step.to_owned(),
                raw: row.total(),
                calls,
                turns,
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
            model_calls: None,
            opens_turn: false,
            cache: Default::default(),
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
            // r2 (gone since) interleaves: a Claude turn of 5 model calls, whose context came
            // apart.
            TurnUsage {
                model_calls: Some(5),
                ..row(Some("r2"), Some(20_000), 9_000, 90_000)
            },
            row(Some("r1"), None, 500, 13_500),
        ];
        let contexts = request_contexts(&rows, &requests);
        // Its calls are counted as the request's summary counts its steps' calls.
        let of_requests: Vec<&TurnUsage> = rows.iter().filter(|r| r.request_id.is_some()).collect();
        assert_eq!(
            step_tokens(&of_requests)[0].calls,
            contexts.iter().map(|context| context.calls).sum::<u32>()
        );
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
                    calls: 5,
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
        // The calls behind each use and the turns' first uses, as the CLIs reported them.
        let counted = |mut row: TurnUsage, calls, opens_turn| {
            row.model_calls = Some(calls);
            row.opens_turn = opens_turn;
            row
        };
        let rows = [
            counted(
                at(
                    row(Some("r1"), None, 100, 1_000),
                    3_000,
                    StepKind::Thread,
                    ProviderKind::Claude,
                    Some(0.25),
                ),
                4,
                true,
            ),
            at(
                row(Some("r1"), None, 200, 2_000),
                40_000,
                StepKind::Worker,
                ProviderKind::Codex,
                None,
            ),
            counted(
                at(
                    row(Some("r1"), None, 10, 100),
                    20_000,
                    StepKind::Thread,
                    ProviderKind::Claude,
                    Some(0.5),
                ),
                3,
                true,
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
                    cache_write_1h: 0,
                    cache_write_5m: 0,
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
                    cache_write_1h: 0,
                    cache_write_5m: 0,
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
                    calls: 1,
                    turns: 0
                },
                StepTokens {
                    step: "thread".into(),
                    raw: 1_310,
                    calls: 7,
                    turns: 2
                },
                StepTokens {
                    step: "review".into(),
                    raw: 55,
                    calls: 1,
                    turns: 0
                },
            ]
        );
        // BUILD-PLAN A0: the report sums to the request's turn_usage rows, through settlement.
        let stored: i64 = rows
            .iter()
            .filter(|row| row.request_id.as_deref() == Some("r1"))
            .map(TurnUsage::total)
            .sum();
        let by_provider: i64 = done.providers.iter().map(|tokens| tokens.raw).sum();
        let by_step: i64 = done.steps.iter().map(|step| step.raw).sum();
        assert_eq!((by_provider, by_step), (stored, stored));
        let still = &summaries[1];
        assert_eq!(still.first_event_ms, Some(1_000));
        assert_eq!(
            (still.answer_ms, still.settled_ms, still.landed_ms),
            (None, None, None)
        );
        assert_eq!((done.ended, still.ended), (None, None));
    }

    /// A request that didn't answer says how it ended, so the Inspector never reads a stop, a
    /// failure or a quit mid-turn as an answer (UI audit H1, phase A verify).
    #[test]
    fn a_request_that_did_not_answer_says_how_it_ended() {
        let ended = |state: serde_json::Value| {
            let request: UserRequest = serde_json::from_value(serde_json::json!({
                "id": "r1",
                "conversationId": "c",
                "preview": "Add a test",
                "state": state,
                "startedAtMs": 1_000,
                "endedAtMs": 14_000,
            }))
            .expect("a request");
            let mut board = Board::default();
            board.requests.insert("r1".into(), request);
            let summary = &request_summaries(&[], &board)[0];
            assert_eq!(summary.answer_ms, Some(13_000));
            summary.ended
        };
        assert_eq!(ended(serde_json::json!({ "type": "done" })), None);
        assert_eq!(
            ended(serde_json::json!({ "type": "stopped" })),
            Some(RequestEnd::Stopped)
        );
        assert_eq!(
            ended(serde_json::json!({ "type": "failed", "error": "The CLI crashed." })),
            Some(RequestEnd::Failed)
        );
        assert_eq!(
            ended(serde_json::json!({
                "type": "failed",
                "error": super::super::conversation::INTERRUPTED_BY_QUIT,
            })),
            Some(RequestEnd::Interrupted)
        );
    }

    /// One Claude session's stream through the parser and the meter (BUILD-PLAN B0): each
    /// call's usage as `message_start` carries it, then the turn's result. Returns the row.
    fn replayed(
        meter: &crate::routing::TokenMeter,
        turns: &[&[(&str, i64, i64, i64)]],
        last: Option<&crate::routing::LastSession>,
        request: &str,
        step: StepKind,
    ) -> Vec<TurnUsage> {
        use brigadier_providers::ProviderEvent;
        use brigadier_providers::claude::parse::{Output, Parser};
        use serde_json::json;
        let usage = |input: i64, read: i64, write: i64| {
            json!({ "input_tokens": input, "cache_read_input_tokens": read,
                    "cache_creation_input_tokens": write, "output_tokens": 400,
                    "cache_creation": { "ephemeral_1h_input_tokens": write,
                                        "ephemeral_5m_input_tokens": 0 } })
        };
        let mut parser = Parser::live();
        let mut rows = Vec::new();
        for calls in turns {
            let mut sum = (0, 0, 0);
            for &(id, input, read, write) in *calls {
                parser.feed(
                    &json!({ "type": "stream_event", "parent_tool_use_id": null,
                             "event": { "type": "message_start",
                                        "message": { "id": id, "usage": usage(input, read, write) } } })
                    .to_string(),
                );
                sum = (sum.0 + input, sum.1 + read, sum.2 + write);
            }
            let outputs = parser.feed(
                &json!({ "type": "result", "subtype": "success", "is_error": false,
                         "usage": usage(sum.0, sum.1, sum.2) })
                .to_string(),
            );
            let (total, calls) = outputs
                .into_iter()
                .find_map(|output| match output {
                    Output::Event(ProviderEvent::Usage { total, calls, .. }) => {
                        Some((total, calls))
                    }
                    _ => None,
                })
                .expect("a usage report");
            let used = meter.delta(&total, None).expect("a use");
            let mut turn = row(
                Some(request),
                None,
                used.input_tokens,
                used.cached_input_tokens,
            );
            turn.provider = ProviderKind::Claude;
            turn.step = Some(step);
            turn.cache_write = used.cache_write_tokens;
            turn.output = used.output_tokens;
            turn.cache = meter.trace(&used, &calls, last);
            rows.push(turn);
        }
        rows
    }

    fn board_with(request: &str) -> Board {
        let request: UserRequest = serde_json::from_value(serde_json::json!({
            "id": request,
            "conversationId": "c",
            "preview": "Look at the cart",
            "state": { "type": "done" },
            "startedAtMs": 0,
            "endedAtMs": 1_000,
        }))
        .expect("a request");
        let mut board = Board::default();
        board.requests.insert(request.id.clone(), request);
        board
    }

    /// BUILD-PLAN B0's done-when: the note fork of 2026-10-14 03:46 (WEAK-SPOTS C11) replayed.
    /// The thread's last turn stood at 252,313; the fork's one call read 544 and wrote 216,616
    /// (the row stored then; its lifetime split wasn't recorded, so the fixture writes it for
    /// an hour). The report shows about 252K missed, under "note fork".
    #[test]
    fn the_c11_note_fork_shows_its_missed_prefix_under_its_cause() {
        use crate::routing::{LastSession, TokenMeter, WriteCause};
        // The thread's own session, as its rows recorded it: its last call's prompt.
        let thread = LastSession {
            last_prompt: Some(252_313),
            session_key: None,
        };
        let fork = TokenMeter::new(false).starting_from(WriteCause::NoteFork, None);
        let rows = replayed(
            &fork,
            &[&[("fork-1", 3, 544, 216_616)]],
            Some(&thread),
            "r1",
            StepKind::Handoff,
        );
        let summary = &request_summaries(&rows, &board_with("r1"))[0];
        assert_eq!(
            summary.cache.causes,
            [CauseTokens {
                cause: "note fork".into(),
                sessions: 1,
                written: 216_616,
                missed: 252_313 - 544,
            }]
        );
        assert_eq!(
            (
                summary.cache.missed,
                summary.cache.big_misses,
                summary.cache.unexplained
            ),
            (251_769, 1, 0)
        );
        let claude = &summary.providers[0];
        assert_eq!(
            (
                claude.cache_write,
                claude.cache_write_1h,
                claude.cache_write_5m
            ),
            (216_616, 216_616, 0)
        );
    }

    /// Several calls in a turn, each compared with the call before it (Codex's review of the
    /// B0 outline): a cold resume misses the session's last prompt on its first call, warm calls
    /// miss nothing, and a later call that misses part of the prefix is left unexplained.
    #[test]
    fn each_call_is_compared_with_the_one_before_and_a_cause_explains_only_its_first() {
        use crate::routing::{LastSession, TokenMeter, WriteCause};
        let last = LastSession {
            last_prompt: Some(80_000),
            session_key: None,
        };
        let meter = TokenMeter::new(false).starting(WriteCause::ColdResume);
        let rows = replayed(
            &meter,
            &[
                // Cold: the whole history written again; then warm calls.
                &[
                    ("a1", 5, 0, 80_500),
                    ("a2", 5, 80_505, 900),
                    ("a3", 5, 81_410, 700),
                ],
                // Next turn: 30K of the prefix gone (an edit high in the prompt, say).
                &[("a4", 5, 52_115, 30_000)],
            ],
            Some(&last),
            "r1",
            StepKind::Thread,
        );
        assert_eq!(rows[0].cache.cause, Some(WriteCause::ColdResume));
        assert_eq!(rows[1].cache.cause, None);
        assert_eq!(rows[0].cache.last_prompt, Some(82_115));
        assert_eq!(rows[1].cache.missed, 82_115 - 52_115);
        let cache = &request_summaries(&rows, &board_with("r1"))[0].cache;
        assert_eq!(
            cache.causes,
            [CauseTokens {
                cause: "cold resume".into(),
                sessions: 1,
                written: 80_500,
                missed: 80_000,
            }]
        );
        assert_eq!(
            (
                cache.missed,
                cache.big_misses,
                cache.unexplained,
                cache.unestimated_calls
            ),
            (110_000, 2, 30_000, 0)
        );
    }
}
