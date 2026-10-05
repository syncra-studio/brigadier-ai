//! Whether Brigadier is quiet enough for maintenance (compacting the database): nothing works,
//! and what may have run between two looks shows in a generation that only goes up.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tokio::sync::Notify;

use super::SessionManager;

/// Model turns in the background that no conversation shows as busy: a checkpoint's fork,
/// model research, a commit message.
#[derive(Default)]
pub(super) struct Activity {
    running: AtomicUsize,
    /// Goes up when one starts and when one ends.
    generation: AtomicU64,
    /// A delete finished in the background: it may have left space to give back.
    freed: Notify,
    /// The database was found too large to compact on its own (said once).
    too_large: AtomicBool,
}

/// A background model turn, counted until it is dropped.
pub(super) struct BackgroundTurn(Arc<Activity>);

impl Drop for BackgroundTurn {
    fn drop(&mut self) {
        self.0.running.fetch_sub(1, Ordering::SeqCst);
        self.0.generation.fetch_add(1, Ordering::SeqCst);
    }
}

impl SessionManager {
    /// Counts a background model turn until the guard is dropped.
    pub(super) fn background_turn(&self) -> BackgroundTurn {
        self.activity.generation.fetch_add(1, Ordering::SeqCst);
        self.activity.running.fetch_add(1, Ordering::SeqCst);
        BackgroundTurn(self.activity.clone())
    }

    /// What works now that maintenance must wait for, said plainly; `None` when nothing does.
    /// Turns, workers, landings, overnight runs and Brain jobs also write when they start and
    /// when they end, which the store counts ([`brigadier_store::Store::writes_admitted`]).
    pub async fn busy_for_maintenance(&self) -> Option<&'static str> {
        if self.overnight_active() {
            return Some("an overnight run");
        }
        if self.agents_working().await {
            return Some("a turn or a worker");
        }
        if !self.brain_work().is_empty() {
            return Some("Brain work");
        }
        if !self.research.jobs.is_empty() {
            return Some("model research");
        }
        if self.activity.running.load(Ordering::SeqCst) > 0 {
            return Some("a background model turn");
        }
        if self.cleanups_running() {
            return Some("an archive or delete cleanup");
        }
        if self.runtime.raw_sessions_open() {
            return Some("an Inspector session");
        }
        None
    }

    /// Goes up whenever anything is written or a background model turn starts or ends: a
    /// quiet period is one it stays the same through.
    pub fn maintenance_generation(&self) -> u64 {
        self.core.store().writes_admitted() + self.activity.generation.load(Ordering::SeqCst)
    }

    /// Resolves after a delete has finished in the background.
    pub async fn space_freed(&self) {
        self.activity.freed.notified().await;
    }

    pub(super) fn note_space_freed(&self) {
        self.activity.freed.notify_one();
    }

    /// Whether this is the first time the database is found too large to compact on its own.
    pub(super) fn first_too_large(&self) -> bool {
        !self.activity.too_large.swap(true, Ordering::SeqCst)
    }
}
