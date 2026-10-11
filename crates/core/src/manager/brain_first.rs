//! Brain first (SESSION-FIXES-PLAN.md Q6.6): each worker's Brain use is counted on its task
//! (queries, and its tool calls, so a long task that never asked shows), and a worker whose
//! first calls don't ask the Brain is told to, once. The brief's Brain section is
//! `context_pack.rs`'s; what it writes back is `brains.rs`'s `record_report`.

use std::sync::Arc;

use brigadier_providers::{NoticeLevel, ProviderEvent, TurnInput};

use super::SessionManager;
use super::conversation::Cli;
use super::workers::TaskLive;
use crate::model::ConversationId;
use crate::work::{TaskId, TaskKind};

/// A worker that hasn't asked the Brain by its this-many-th call is told to.
pub(crate) const STEER_AFTER: u32 = 10;

/// What a worker that didn't ask the Brain is told, mid-turn.
pub(crate) const BRAIN_FIRST: &str = "Ask the Brain first: query_brain with what you are looking for (a module, a decision, how something works here) before you search or read more code. It holds what earlier workers in this project found, so you don't have to find it again.";

impl SessionManager {
    /// A worker finished a tool call or command: counted, and at its [`STEER_AFTER`]th call
    /// with no Brain query yet, it is steered to ask the Brain.
    pub(crate) async fn note_worker_call(&self, live: &Arc<TaskLive>, cli: &Arc<Cli>) {
        let (pending, undecided) = live.count_call().await;
        if !undecided {
            return;
        }
        let Ok(task) = self.task_by_id(&live.conversation_id, &live.id).await else {
            return;
        };
        if task.brain.calls + pending < STEER_AFTER {
            return;
        }
        live.brain_steer_decided().await;
        // Its first calls were before a restart: too late to say "first".
        let steer = task.brain.calls < STEER_AFTER
            && task.brain.queries == 0
            && !matches!(task.kind, TaskKind::Review | TaskKind::Operate)
            && self
                .core
                .conversation(&task.conversation_id)
                .is_ok_and(|conversation| conversation.project_id.is_some());
        if !steer {
            return;
        }
        self.record_worker_event(
            &live.id,
            ProviderEvent::Notice {
                level: NoticeLevel::Info,
                message: format!(
                    "No Brain query in its first {STEER_AFTER} calls: asked the worker to ask the Brain first."
                ),
            },
        )
        .await;
        // Not from inside the worker's event pump: a steer may wait on the CLI.
        let cli = cli.clone();
        self.spawn(async move {
            if let Err(err) = cli.session.steer(TurnInput::text(BRAIN_FIRST)).await {
                tracing::warn!(error = %err, "could not ask the worker to ask the Brain first");
            }
        });
    }

    /// Adds the worker's counted calls to its task, at the end of each turn.
    pub(crate) async fn save_worker_calls(&self, live: &Arc<TaskLive>) {
        let calls = live.take_calls().await;
        if calls > 0 {
            self.update_task(&live.conversation_id, &live.id, |task| {
                task.brain.calls += calls;
            })
            .await
            .ok();
        }
    }

    /// A worker's `query_brain`, counted on its task.
    pub(crate) async fn note_brain_query(
        &self,
        conversation_id: &ConversationId,
        task_id: &TaskId,
    ) {
        self.update_task(conversation_id, task_id, |task| task.brain.queries += 1)
            .await
            .ok();
    }
}
