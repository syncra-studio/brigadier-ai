use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use brigadier_ipc::metrics::{
    BrainMetrics, DaemonMetrics, IndexRun, LatencySummary, ProcessInfo, ProcessRole,
    RuntimeMetrics, StoreMetrics, TaskPollMetrics, UndoMetrics,
};
use brigadier_ipc::protocol::ClientInfo;
use brigadier_sandbox::Platform;
use brigadier_store::Store;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::sync::{Notify, watch};

use brigadier_core::manager::SessionManager;

use crate::supervisor::{Supervisor, slow_poll_threshold};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT: Duration = Duration::from_millis(50);
/// Heartbeat samples kept for the scheduler-delay summary (10 s at 50 ms).
const HEARTBEAT_WINDOW: usize = 200;
/// The name the desktop app connects under (its bridge).
const APP_CLIENT: &str = "Brigadier";

struct ProcessSample {
    pid: u32,
    rss_bytes: u64,
    cpu_percent: f32,
    name: String,
    started_at_ms: Option<f64>,
}

/// Collects daemon metrics. Sampling (and the heartbeat) only runs while at least one client
/// streams metrics, so an idle daemon does not wake up for them.
pub struct Metrics {
    started: Instant,
    supervisor: Supervisor,
    store: Store,
    platform: Arc<dyn Platform>,
    streaming: AtomicUsize,
    streaming_changed: Notify,
    latest: watch::Sender<DaemonMetrics>,
    connections: AtomicU32,
    clients: Mutex<HashMap<u64, ClientInfo>>,
    /// The process of the app that connected last, kept once it disconnects.
    last_app: Mutex<Option<u32>>,
    heartbeat: Mutex<VecDeque<f64>>,
    system: Arc<Mutex<System>>,
    /// Where the Brain counters come from.
    sessions: Weak<SessionManager>,
    undo: UndoCounts,
}

/// The user's Undos and Reapplies since the daemon started, and how many failed.
#[derive(Default)]
struct UndoCounts {
    tried: AtomicU64,
    failed: AtomicU64,
}

impl UndoCounts {
    fn count(&self, ok: bool) {
        self.tried.fetch_add(1, Ordering::Relaxed);
        if !ok {
            self.failed.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn get(&self) -> UndoMetrics {
        UndoMetrics {
            tried: self.tried.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

impl Metrics {
    pub fn start(
        supervisor: Supervisor,
        store: Store,
        platform: Arc<dyn Platform>,
        sessions: &Arc<SessionManager>,
    ) -> Arc<Self> {
        let metrics = Arc::new(Self {
            started: Instant::now(),
            supervisor: supervisor.clone(),
            store,
            platform,
            streaming: AtomicUsize::new(0),
            streaming_changed: Notify::new(),
            latest: watch::channel(DaemonMetrics::default()).0,
            connections: AtomicU32::new(0),
            clients: Mutex::new(HashMap::new()),
            last_app: Mutex::new(None),
            heartbeat: Mutex::new(VecDeque::with_capacity(HEARTBEAT_WINDOW)),
            system: Arc::new(Mutex::new(System::new())),
            sessions: Arc::downgrade(sessions),
            undo: UndoCounts::default(),
        });

        let sampler = metrics.clone();
        supervisor.spawn_critical("metrics sampler", async move {
            loop {
                sampler.wait_for_streaming().await;
                let sample = sampler.sample().await;
                sampler.latest.send_replace(sample);
                tokio::time::sleep(SAMPLE_INTERVAL).await;
            }
        });

        let heart = metrics.clone();
        supervisor.spawn_critical("metrics heartbeat", async move {
            loop {
                heart.wait_for_streaming().await;
                let deadline = tokio::time::Instant::now() + HEARTBEAT;
                tokio::time::sleep_until(deadline).await;
                let late = tokio::time::Instant::now().saturating_duration_since(deadline);
                let mut samples = heart.heartbeat.lock().unwrap_or_else(|p| p.into_inner());
                if samples.len() == HEARTBEAT_WINDOW {
                    samples.pop_front();
                }
                samples.push_back(late.as_secs_f64() * 1000.0);
            }
        });
        metrics
    }

    async fn wait_for_streaming(&self) {
        loop {
            let changed = self.streaming_changed.notified();
            if self.streaming.load(Ordering::Acquire) > 0 {
                return;
            }
            changed.await;
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<DaemonMetrics> {
        self.latest.subscribe()
    }

    pub fn set_streaming(&self, on: bool) {
        if on {
            self.streaming.fetch_add(1, Ordering::AcqRel);
        } else {
            self.streaming.fetch_sub(1, Ordering::AcqRel);
            if self.streaming.load(Ordering::Acquire) == 0 {
                // Stale samples would misreport the next session.
                self.heartbeat
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .clear();
            }
        }
        self.streaming_changed.notify_waiters();
    }

    /// How many app connections (token holders) are open.
    pub fn clients(&self) -> usize {
        self.clients.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// The process of the app that connected last, still connected or not.
    pub fn last_app(&self) -> Option<u32> {
        *self.last_app.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn connection_opened(&self, id: u64, client: ClientInfo) {
        if client.name == APP_CLIENT {
            *self.last_app.lock().unwrap_or_else(|p| p.into_inner()) = Some(client.pid);
        }
        self.connections.fetch_add(1, Ordering::Relaxed);
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, client);
    }

    pub fn connection_closed(&self, id: u64) {
        self.connections.fetch_sub(1, Ordering::Relaxed);
        self.clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&id);
    }

    /// One Undo or Reapply the user asked for ended, done or failed.
    pub fn undo_ended(&self, ok: bool) {
        self.undo.count(ok);
    }

    /// A fresh sample, taken now.
    /// The Brains' query timings, embedder and largest index run.
    fn brain(&self) -> BrainMetrics {
        let Some(sessions) = self.sessions.upgrade() else {
            return BrainMetrics::default();
        };
        let mut counters = sessions.brain_counters();
        BrainMetrics {
            query: LatencySummary::from_samples(&mut counters.query_ms),
            embedder_loaded: counters.embedder_loaded,
            largest_index_run: counters.largest_index_run.map(|run| {
                Box::new(IndexRun {
                    root: run.root,
                    files: run.files,
                    parsed: run.parsed,
                    duration_ms: run.duration_ms,
                    at_ms: run.at_ms,
                })
            }),
        }
    }

    pub async fn sample(&self) -> DaemonMetrics {
        let own_pid = std::process::id();
        let (rss_bytes, cpu_percent) = self
            .refresh(vec![own_pid])
            .await
            .into_iter()
            .next()
            .map(|sample| (sample.rss_bytes, sample.cpu_percent))
            .unwrap_or_default();

        let mut delays: Vec<f64> = self
            .heartbeat
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .copied()
            .collect();

        let tasks = self.supervisor.monitor().cumulative();
        let runtime = tokio::runtime::Handle::current().metrics();
        let store = self.store.stats();
        DaemonMetrics {
            at_ms: brigadier_core::now_ms(),
            uptime_ms: self.started.elapsed().as_millis() as i64,
            rss_bytes,
            cpu_percent,
            scheduler_delay: LatencySummary::from_samples(&mut delays),
            tasks: TaskPollMetrics {
                slow_poll_threshold_ms: slow_poll_threshold().as_secs_f64() * 1000.0,
                polls: tasks.total_poll_count,
                slow_polls: tasks.total_slow_poll_count,
                slow_poll_total_ms: tasks.total_slow_poll_duration.as_secs_f64() * 1000.0,
                mean_poll_us: tasks.mean_poll_duration().as_secs_f64() * 1e6,
                long_delay_threshold_ms: slow_poll_threshold().as_secs_f64() * 1000.0,
                scheduled: tasks.total_scheduled_count,
                long_delays: tasks.total_long_delay_count,
                mean_scheduling_delay_us: tasks.mean_scheduled_duration().as_secs_f64() * 1e6,
            },
            runtime: RuntimeMetrics {
                workers: runtime.num_workers() as u32,
                alive_tasks: runtime.num_alive_tasks() as u32,
                global_queue_depth: runtime.global_queue_depth() as u32,
            },
            store: StoreMetrics {
                last_seq: store.last_seq,
                queued_writes: store.queued_writes as u32,
                committed_batches: store.committed_batches,
                committed_events: store.committed_events,
                last_batch_commands: store.last_batch_commands,
                last_commit_ms: store.last_commit_us as f64 / 1000.0,
                wal_bytes: store.wal_bytes,
                checkpoints: store.checkpoints,
            },
            connections: self.connections.load(Ordering::Relaxed),
            brain: self.brain(),
            undo: self.undo.get(),
        }
    }

    /// The daemon and every connected client process.
    pub async fn processes(&self) -> Vec<ProcessInfo> {
        let own_pid = std::process::id();
        let clients: Vec<ClientInfo> = self
            .clients
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect();
        let mut pids = vec![own_pid];
        pids.extend(clients.iter().map(|client| client.pid));
        pids.dedup();

        self.refresh(pids)
            .await
            .into_iter()
            .map(|sample| {
                let client = clients.iter().find(|client| client.pid == sample.pid);
                ProcessInfo {
                    pid: sample.pid,
                    role: if sample.pid == own_pid {
                        ProcessRole::Daemon
                    } else {
                        ProcessRole::App
                    },
                    name: client
                        .map(|client| client.name.clone())
                        .unwrap_or(sample.name),
                    rss_bytes: sample.rss_bytes,
                    cpu_percent: sample.cpu_percent,
                    started_at_ms: sample.started_at_ms,
                }
            })
            .collect()
    }

    /// Samples live processes among `pids`. Runs on the blocking pool: sysinfo and the start
    /// time lookup call into the kernel or read /proc synchronously.
    async fn refresh(&self, pids: Vec<u32>) -> Vec<ProcessSample> {
        let system = self.system.clone();
        let platform = self.platform.clone();
        tokio::task::spawn_blocking(move || {
            let mut system = system.lock().unwrap_or_else(|p| p.into_inner());
            let sys_pids: Vec<Pid> = pids.iter().map(|pid| Pid::from_u32(*pid)).collect();
            system.refresh_processes_specifics(
                ProcessesToUpdate::Some(&sys_pids),
                true,
                ProcessRefreshKind::nothing().with_memory().with_cpu(),
            );
            pids.iter()
                .filter_map(|pid| {
                    let process = system.process(Pid::from_u32(*pid))?;
                    Some(ProcessSample {
                        pid: *pid,
                        rss_bytes: process.memory(),
                        cpu_percent: process.cpu_usage(),
                        name: process.name().to_string_lossy().into_owned(),
                        started_at_ms: platform.processes().start_time_ms(*pid).ok(),
                    })
                })
                .collect()
        })
        .await
        .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every Undo counts, and those that failed apart (#129).
    #[test]
    fn undo_failures_are_counted() {
        let undo = UndoCounts::default();
        undo.count(true);
        undo.count(false);
        undo.count(false);
        assert_eq!(
            undo.get(),
            UndoMetrics {
                tried: 3,
                failed: 2
            }
        );
    }
}
