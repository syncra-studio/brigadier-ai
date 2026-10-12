//! One coalescing cursor-feed writer; input producers never wait for disk I/O.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

#[derive(Clone, Default)]
pub(crate) struct CursorFeedSnapshot {
    pub session: Option<String>,
    pub visible: bool,
    pub x: f64,
    pub y: f64,
}

#[derive(Default)]
struct PendingWrite {
    current: Option<CursorFeedSnapshot>,
    remove: bool,
    revision: u64,
    completed: u64,
    stopped: bool,
}

/// Retains only the latest desired state, including ownership and visibility.
/// Lifecycle writes and release barriers share the same ordered disk worker.
pub(crate) struct CursorFeedWriter {
    shared: Arc<(Mutex<PendingWrite>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

impl CursorFeedWriter {
    pub(crate) fn new(
        mut write: impl FnMut(Option<CursorFeedSnapshot>) -> std::io::Result<()> + Send + 'static,
    ) -> Self {
        let shared = Arc::new((Mutex::new(PendingWrite::default()), Condvar::new()));
        let worker_shared = shared.clone();
        let worker = std::thread::spawn(move || {
            let (lock, changed) = &*worker_shared;
            loop {
                let (snapshot, revision) = {
                    let mut pending = lock.lock().unwrap();
                    while pending.revision == pending.completed && !pending.stopped {
                        pending = changed.wait(pending).unwrap();
                    }
                    if pending.revision == pending.completed && pending.stopped {
                        return;
                    }
                    let snapshot = if pending.remove {
                        None
                    } else {
                        pending.current.clone()
                    };
                    (snapshot, pending.revision)
                };
                // No producer/ownership lock is held across filesystem work.
                if let Err(error) = write(snapshot) {
                    eprintln!("[brigadier-cu] warning: failed to write cursor feed: {error}");
                }
                let mut pending = lock.lock().unwrap();
                pending.completed = revision;
                changed.notify_all();
            }
        });
        Self {
            shared,
            worker: Some(worker),
        }
    }

    fn submit(&self, wait: bool, edit: impl FnOnce(&mut PendingWrite) -> bool) {
        let (lock, changed) = &*self.shared;
        let mut pending = lock.lock().unwrap();
        if !edit(&mut pending) {
            return;
        }
        pending.revision += 1;
        let revision = pending.revision;
        changed.notify_all();
        while wait && pending.completed < revision {
            pending = changed.wait(pending).unwrap();
        }
    }

    pub(crate) fn update(&self, session: Option<&str>, x: f64, y: f64, wait: bool) {
        self.submit(wait, |pending| {
            pending.current = Some(CursorFeedSnapshot {
                session: session.map(str::to_owned),
                visible: true,
                x,
                y,
            });
            pending.remove = false;
            true
        });
    }

    pub(crate) fn visibility(&self, owner: Option<&str>, visible: bool, wait: bool) {
        self.submit(wait, |pending| {
            if let Some(owner) = owner {
                if pending
                    .current
                    .as_ref()
                    .and_then(|state| state.session.as_deref())
                    != Some(owner)
                {
                    return false;
                }
            }
            pending
                .current
                .get_or_insert_with(CursorFeedSnapshot::default)
                .visible = visible;
            pending.remove = false;
            true
        });
    }

    pub(crate) fn remove(&self) {
        self.submit(true, |pending| {
            pending.remove = true;
            true
        });
    }

    pub(crate) fn flush(&self) {
        let (lock, changed) = &*self.shared;
        let mut pending = lock.lock().unwrap();
        let revision = pending.revision;
        while pending.completed < revision {
            pending = changed.wait(pending).unwrap();
        }
    }
}

impl Drop for CursorFeedWriter {
    fn drop(&mut self) {
        let (lock, changed) = &*self.shared;
        lock.lock().unwrap().stopped = true;
        changed.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn slow_disk_coalesces_motion_without_blocking_the_producer() {
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = writes.clone();
        let writer = Arc::new(CursorFeedWriter::new(move |state| {
            if recorded.lock().unwrap().is_empty() {
                started_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            }
            recorded.lock().unwrap().push(state);
            Ok(())
        }));
        writer.update(Some("drag"), 0.0, 0.0, false);
        started_rx.recv().unwrap();
        let producer_writer = writer.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            for x in 1..=10_000 {
                producer_writer.update(Some("drag"), x as f64, 42.0, false);
            }
            done_tx.send(()).unwrap();
        });
        let producer_completed = done_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .is_ok();
        // These updates completed while the disk worker was blocked. Only
        // the final position survives; there is no growing sample queue.
        resume_tx.send(()).unwrap();
        producer.join().unwrap();
        writer.flush();
        assert!(
            producer_completed,
            "input updates waited for the blocked disk sink"
        );
        let writes = writes.lock().unwrap();
        assert_eq!(writes.len(), 2);
        let last = writes.last().unwrap().as_ref().unwrap();
        assert_eq!((last.x, last.y), (10_000.0, 42.0));
    }

    #[test]
    fn lifecycle_edits_reconcile_with_the_pending_owner_before_flush() {
        let writes = Arc::new(Mutex::new(Vec::new()));
        let recorded = writes.clone();
        let writer = CursorFeedWriter::new(move |state| {
            recorded.lock().unwrap().push(state);
            Ok(())
        });
        writer.update(Some("first"), 10.0, 20.0, false);
        writer.update(Some("second"), 30.0, 40.0, false);
        writer.visibility(Some("first"), false, true);
        writer.flush();
        assert!(
            writes
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .as_ref()
                .unwrap()
                .visible
        );
        writer.visibility(Some("second"), false, true);
        {
            let writes = writes.lock().unwrap();
            let last = writes.last().unwrap().as_ref().unwrap();
            assert!(!last.visible);
            assert_eq!((last.x, last.y), (30.0, 40.0));
        }
        writer.remove();
        assert!(writes.lock().unwrap().last().unwrap().is_none());
    }
}
