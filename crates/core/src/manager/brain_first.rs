//! Brain use (SESSION-FIXES-PLAN.md Q6.6): each worker's Brain use is counted on its task
//! (queries, and its tool calls, so a long task that never asked shows); whether to ask the
//! Brain is the worker's call (#250). The brief's Brain section is `context_pack.rs`'s; what it
//! writes back is `brains.rs`'s `record_report`.

use std::sync::Arc;

use super::SessionManager;
use super::workers::TaskLive;
use crate::model::ConversationId;
use crate::work::TaskId;

impl SessionManager {
    /// A worker finished a tool call or command: counted.
    pub(crate) async fn note_worker_call(&self, live: &Arc<TaskLive>) {
        live.count_call().await;
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
