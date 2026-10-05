//! Closing a conversation (archive, delete) without making anyone wait for its cleanup.
//!
//! - **Fence**: once a conversation starts closing, nothing new of it starts: no worker, no
//!   CLI session, no landing, no overnight segment, no commit. Work that already passed the
//!   fence holds a [`WorkGuard`]; the cleanup waits for those to finish ([`SessionManager::drain`])
//!   before it removes anything, so nothing is created under it. The fence opens again when
//!   the conversation is restored.
//! - **Cleanup jobs**: an archive is acknowledged once the fence and the archived state are
//!   stored; what it stops and removes goes on in the background. The jobs are tracked apart,
//!   so a quit waits for them (bounded); a cut-off one is finished at the next launch, from the
//!   conversation's durable `cleanup_pending` mark.
//! - **Order**: archive, restore and delete of one conversation run in the order they were
//!   asked for ([`SessionManager::in_order`]), without holding up anything else.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::sync::{Notify, watch};
use tokio_util::task::TaskTracker;

use super::SessionManager;
use crate::model::ConversationId;
use crate::{Error, Result};

/// How long a cleanup waits for work that passed the fence before it goes on anyway.
const DRAIN_WAIT: Duration = Duration::from_secs(120);
/// How long a quit waits for cleanups under way; what is left is finished at the next launch.
const QUIT_WAIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct Closing {
    fences: Arc<Fences>,
    /// Cleanups under way, tracked apart from other background work so a quit waits for them.
    jobs: TaskTracker,
    /// The cleanup under way for each conversation: `true` once it has finished.
    running: Mutex<HashMap<ConversationId, watch::Receiver<bool>>>,
    /// The last lifecycle change asked for, per conversation: `true` once it has finished.
    order: Mutex<HashMap<ConversationId, watch::Receiver<bool>>>,
    /// Held for the git and disk part of a cleanup, so cleanups under way at once don't take
    /// the same repository's locks at the same time.
    pub(super) lane: tokio::sync::Mutex<()>,
}

#[derive(Default)]
struct Fences {
    state: Mutex<HashMap<ConversationId, Fence>>,
    changed: Notify,
}

#[derive(Default)]
struct Fence {
    closed: bool,
    /// Work that passed the fence and has not finished.
    inflight: usize,
}

impl Fences {
    fn lock(&self) -> MutexGuard<'_, HashMap<ConversationId, Fence>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Work of a conversation that passed its fence; the conversation's cleanup waits for it.
pub(crate) struct WorkGuard {
    fences: Arc<Fences>,
    id: ConversationId,
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        {
            let mut state = self.fences.lock();
            if let Some(fence) = state.get_mut(&self.id) {
                fence.inflight = fence.inflight.saturating_sub(1);
                if !fence.closed && fence.inflight == 0 {
                    state.remove(&self.id);
                }
            }
        }
        self.fences.changed.notify_waiters();
    }
}

/// A lifecycle change's place in its conversations' order: [`Self::ready`] waits for the
/// changes asked for before it; dropping it lets the next one go.
pub struct Turn {
    before: Vec<watch::Receiver<bool>>,
    done: watch::Sender<bool>,
}

impl Turn {
    pub async fn ready(&mut self) {
        for before in &mut self.before {
            // A sender dropped without finishing (a panic) lets the next one go too.
            let _ = before.wait_for(|done| *done).await;
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.done.send_replace(true);
    }
}

/// Why something of a closing conversation doesn't start.
pub(super) fn closing_error() -> Error {
    Error::Invalid("This session is being archived or deleted: nothing new starts in it.".into())
}

impl SessionManager {
    /// Lets work of conversation `id` start, unless it is closing. Hold the guard until what
    /// it starts is in place (a CLI session registered, a worktree recorded, a landing done).
    pub(crate) fn enter(&self, id: &ConversationId) -> Result<WorkGuard> {
        let fences = &self.closing.fences;
        let mut state = fences.lock();
        let fence = state.entry(id.clone()).or_default();
        if fence.closed {
            return Err(closing_error());
        }
        fence.inflight += 1;
        Ok(WorkGuard {
            fences: fences.clone(),
            id: id.clone(),
        })
    }

    /// Whether conversation `id` is closing.
    pub(crate) fn is_closing(&self, id: &ConversationId) -> bool {
        self.closing
            .fences
            .lock()
            .get(id)
            .is_some_and(|fence| fence.closed)
    }

    /// Nothing new of conversation `id` starts from now on.
    pub(super) fn close_fence(&self, id: &ConversationId) {
        self.closing
            .fences
            .lock()
            .entry(id.clone())
            .or_default()
            .closed = true;
    }

    /// Conversation `id` works again (restored, or its closing failed).
    pub(super) fn open_fence(&self, id: &ConversationId) {
        let mut state = self.closing.fences.lock();
        if let Some(fence) = state.get_mut(id) {
            fence.closed = false;
            if fence.inflight == 0 {
                state.remove(id);
            }
        }
    }

    /// Waits until no work of conversation `id` that passed its fence is still going (at most
    /// [`DRAIN_WAIT`]). Whether it all finished.
    pub(super) async fn drain(&self, id: &ConversationId) -> bool {
        let fences = &self.closing.fences;
        let settled = async {
            loop {
                let changed = fences.changed.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                if fences
                    .lock()
                    .get(id)
                    .is_none_or(|fence| fence.inflight == 0)
                {
                    return;
                }
                changed.await;
            }
        };
        tokio::time::timeout(DRAIN_WAIT, settled).await.is_ok()
    }

    /// Takes the next place in the lifecycle order of each conversation in `ids` (call it in
    /// the order the changes were asked for; it doesn't wait).
    pub fn in_order(&self, ids: &[ConversationId]) -> Turn {
        let (done, finished) = watch::channel(false);
        let mut order = self.closing.order.lock().unwrap_or_else(|p| p.into_inner());
        // Places of finished changes are dropped, so the map stays small.
        order.retain(|_, before| !*before.borrow());
        let before = ids
            .iter()
            .filter_map(|id| order.insert(id.clone(), finished.clone()))
            .collect();
        Turn { before, done }
    }

    /// Runs `cleanup` for conversation `id` in the background, tracked as a cleanup.
    pub(super) fn start_cleanup(
        &self,
        id: ConversationId,
        cleanup: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let (done, finished) = watch::channel(false);
        let mut running = self
            .closing
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        running.retain(|_, job| !*job.borrow());
        running.insert(id, finished);
        drop(running);
        let job = async move {
            cleanup.await;
            done.send_replace(true);
        };
        (self.spawner)(Box::pin(self.closing.jobs.track_future(job)));
    }

    /// Waits for conversation `id`'s cleanup under way, if any.
    pub(super) async fn cleanup_finished(&self, id: &ConversationId) {
        let running = self
            .closing
            .running
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(id)
            .cloned();
        if let Some(mut running) = running {
            let _ = running.wait_for(|done| *done).await;
        }
    }

    /// A quit: waits for the cleanups under way, at most [`QUIT_WAIT`]. One cut off is
    /// finished at the next launch.
    pub(super) async fn finish_cleanups_for_quit(&self) {
        self.closing.jobs.close();
        if tokio::time::timeout(QUIT_WAIT, self.closing.jobs.wait())
            .await
            .is_err()
        {
            tracing::warn!(
                left = self.closing.jobs.len(),
                "cleanups still under way at quit; the next launch finishes them"
            );
        }
    }
}
