//! Diagnostics the Inspector shows, and the §4 performance budgets they are checked against.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::protocol::DaemonInfo;

/// Distribution of a latency sample set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct LatencySummary {
    pub samples: u32,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

impl LatencySummary {
    /// Summarizes `samples` (sorted in place).
    pub fn from_samples(samples: &mut [f64]) -> Self {
        if samples.is_empty() {
            return Self::default();
        }
        samples.sort_by(f64::total_cmp);
        let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
        Self {
            samples: samples.len() as u32,
            p50_ms: at(0.50),
            p95_ms: at(0.95),
            max_ms: samples[samples.len() - 1],
        }
    }
}

/// Poll and scheduling statistics from instrumenting every daemon task (tokio-metrics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskPollMetrics {
    /// A poll longer than this blocked its runtime worker (a stall).
    pub slow_poll_threshold_ms: f64,
    pub polls: u64,
    pub slow_polls: u64,
    pub slow_poll_total_ms: f64,
    pub mean_poll_us: f64,
    /// Wake-to-poll delay above this counts as a long scheduling delay.
    pub long_delay_threshold_ms: f64,
    pub scheduled: u64,
    pub long_delays: u64,
    pub mean_scheduling_delay_us: f64,
}

/// Tokio runtime gauges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeMetrics {
    pub workers: u32,
    pub alive_tasks: u32,
    pub global_queue_depth: u32,
}

/// Event store counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct StoreMetrics {
    pub last_seq: i64,
    pub queued_writes: u32,
    pub committed_batches: u64,
    pub committed_events: u64,
    pub last_batch_commands: u64,
    pub last_commit_ms: f64,
    pub wal_bytes: u64,
    pub checkpoints: u64,
}

/// The last full code-index scan of any project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct IndexRun {
    pub root: String,
    pub files: u64,
    /// Files it parsed (the rest were unchanged).
    pub parsed: u64,
    pub duration_ms: u64,
    pub at_ms: i64,
}

/// Project Brain counters (PLAN.md §4: Brain query and static index budgets).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainMetrics {
    /// `query_brain` tool calls, end to end in the daemon, over the recent window.
    pub query: LatencySummary,
    /// The embedding model is loaded.
    pub embedder_loaded: bool,
    /// The last full scan with the most files, since the daemon started.
    pub largest_index_run: Option<Box<IndexRun>>,
}

/// The user's Undos and Reapplies since the daemon started (a request's changes or picked
/// pending changes), and how many of them failed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct UndoMetrics {
    pub tried: u64,
    pub failed: u64,
}

/// A daemon metrics sample.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DaemonMetrics {
    pub at_ms: i64,
    pub uptime_ms: i64,
    pub rss_bytes: u64,
    pub cpu_percent: f32,
    /// How late a 50 ms heartbeat timer fires: a proxy for runtime stalls. Sampled only while
    /// metrics are streaming.
    pub scheduler_delay: LatencySummary,
    pub tasks: TaskPollMetrics,
    pub runtime: RuntimeMetrics,
    pub store: StoreMetrics,
    pub connections: u32,
    pub brain: BrainMetrics,
    #[serde(default)]
    pub undo: UndoMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum ProcessRole {
    Daemon,
    App,
    Worker,
}

/// A process Brigadier owns or is connected to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub role: ProcessRole,
    pub name: String,
    pub rss_bytes: u64,
    pub cpu_percent: f32,
    pub started_at_ms: Option<f64>,
}

/// Everything the Inspector loads when it opens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub daemon: DaemonInfo,
    pub metrics: DaemonMetrics,
    pub processes: Vec<ProcessInfo>,
    pub budgets: Vec<Budget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BudgetId {
    CoreIdleRss,
    CoreRssTwentyWorkers,
    ColdStart,
    FirstColdStart,
    IngestToPaint,
    WorkerCardsFrameRate,
    FrameGaps,
    RuntimeStalls,
    SchedulerDelay,
    BrainQuery,
    StaticIndex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BudgetUnit {
    Ms,
    Mb,
    Count,
}

/// A §4 performance budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    pub id: BudgetId,
    pub metric: String,
    /// Human-readable target from docs/PLAN.md §4.
    pub target: String,
    /// Upper bound (exclusive) for measured values, when the budget is numeric.
    pub limit: Option<f64>,
    pub unit: BudgetUnit,
    /// Timing budgets get a tolerance multiplier on shared CI runners; size budgets never do.
    pub timing: bool,
    /// Set when the feature this budget covers arrives in a later phase; not measured yet.
    pub phase: Option<u8>,
}

/// The §4 table. Budgets for features that do not exist yet carry the phase that adds them and
/// are reported as "n/a", never as passing.
pub fn budgets() -> Vec<Budget> {
    let budget = |id, metric: &str, target: &str, limit, unit, timing, phase| Budget {
        id,
        metric: metric.into(),
        target: target.into(),
        limit,
        unit,
        timing,
        phase,
    };
    use BudgetId::*;
    use BudgetUnit::*;
    vec![
        budget(
            CoreIdleRss,
            "Core idle RSS",
            "< 60 MB",
            Some(60.0),
            Mb,
            false,
            None,
        ),
        budget(
            CoreRssTwentyWorkers,
            "Core RSS with 20 active workers",
            "< 300 MB",
            Some(300.0),
            Mb,
            false,
            Some(3),
        ),
        budget(
            ColdStart,
            "App cold start to interactive",
            "< 1 s",
            Some(1000.0),
            Ms,
            true,
            None,
        ),
        budget(
            FirstColdStart,
            "First launch to interactive",
            "< 3 s",
            Some(3000.0),
            Ms,
            true,
            None,
        ),
        budget(
            IngestToPaint,
            "Event ingest to UI paint (p95)",
            "< 50 ms",
            Some(50.0),
            Ms,
            true,
            None,
        ),
        budget(
            WorkerCardsFrameRate,
            "UI with 20 streaming worker cards",
            "60 fps, no long tasks > 50 ms",
            None,
            Count,
            true,
            Some(3),
        ),
        budget(
            FrameGaps,
            "Frame gaps (proxy for long tasks)",
            "< 50 ms",
            Some(50.0),
            Ms,
            true,
            None,
        ),
        budget(
            RuntimeStalls,
            "Task polls > 10 ms (runtime stalls)",
            "none",
            Some(1.0),
            Count,
            true,
            None,
        ),
        budget(
            SchedulerDelay,
            "Runtime scheduler delay (proxy)",
            "< 10 ms",
            Some(10.0),
            Ms,
            true,
            None,
        ),
        budget(
            BrainQuery,
            "Brain query (p95)",
            "< 50 ms",
            Some(50.0),
            Ms,
            true,
            None,
        ),
        budget(
            StaticIndex,
            "Static index of a 100k-file repo",
            "< 60 s",
            Some(60_000.0),
            Ms,
            true,
            None,
        ),
    ]
}
