//! Launch smoke check (`brigadier-app --smoke`).
//!
//! The webview drives the run (it is the one that paints): it reports first interactive paint,
//! fires a synthetic event burst through the daemon's normal write path, times every probe from
//! daemon ingest to paint, and hands its measurements over. This module evaluates the §4
//! budgets for implemented features, prints a JSON report (also written to
//! `BRIGADIER_SMOKE_REPORT` when set), quits the daemon and exits non-zero on any failure.
//! Timing budgets are multiplied by `BRIGADIER_BUDGET_TOLERANCE` (CI only); size budgets never.

use std::time::Duration;

use brigadier_ipc::app::{BudgetCheck, CheckStatus, SmokeReport, UiMeasurements};
use brigadier_ipc::metrics::{BudgetId, Diagnostics, budgets};
use tauri::AppHandle;

use crate::shell;

pub const REPORT_ENV: &str = "BRIGADIER_SMOKE_REPORT";
/// The whole run must finish within this, or it fails.
pub const WATCHDOG: Duration = Duration::from_secs(120);
/// The repository size the static index budget is judged on (PLAN.md §4: 100k files).
const STATIC_INDEX_FILES: u64 = 100_000;

pub fn evaluate(
    platform: &str,
    tolerance: f64,
    cold_start_ms: f64,
    first_launch: bool,
    ui: &UiMeasurements,
    diagnostics: &Diagnostics,
) -> SmokeReport {
    let metrics = &diagnostics.metrics;
    let checks: Vec<BudgetCheck> = budgets()
        .into_iter()
        .map(|budget| {
            let limit = budget.limit.map(|limit| {
                if budget.timing {
                    limit * tolerance
                } else {
                    limit
                }
            });
            let mut check = BudgetCheck {
                id: budget.id,
                metric: budget.metric.clone(),
                target: budget.target.clone(),
                measured: None,
                limit,
                status: CheckStatus::NotApplicable,
                note: String::new(),
            };
            if let Some(phase) = budget.phase {
                check.note = format!("n/a — Phase {phase}");
                return check;
            }
            let (measured, note) = match budget.id {
                BudgetId::CoreIdleRss => (
                    Some(ui.idle_rss_bytes as f64 / (1024.0 * 1024.0)),
                    "daemon RSS after startup, before the burst".to_owned(),
                ),
                BudgetId::ColdStart | BudgetId::FirstColdStart => {
                    if first_launch != (budget.id == BudgetId::FirstColdStart) {
                        check.note = if first_launch {
                            "n/a — a first launch (new data folder) is judged on its own budget"
                        } else {
                            "n/a — not a first launch (the data folder was already set up)"
                        }
                        .into();
                        return check;
                    }
                    (
                        Some(cold_start_ms),
                        format!(
                            "process start to the app shown and usable, daemon launch and agent checks included; ms since process start: {}",
                            ui.startup
                        ),
                    )
                }
                BudgetId::IngestToPaint => (
                    (ui.probes_painted > 0).then_some(ui.ingest_to_paint.p95_ms),
                    format!(
                        "{} of {} probes painted, p50 {:.1} ms, max {:.1} ms",
                        ui.probes_painted,
                        ui.probes_expected,
                        ui.ingest_to_paint.p50_ms,
                        ui.ingest_to_paint.max_ms
                    ),
                ),
                BudgetId::FrameGaps => (
                    (ui.frame_gaps.samples > 0).then_some(ui.frame_gaps.max_ms),
                    format!(
                        "largest of {} frame gaps during the burst; {}",
                        ui.frame_gaps.samples, ui.stall_context
                    ),
                ),
                BudgetId::RuntimeStalls => (
                    Some(metrics.tasks.slow_polls as f64),
                    format!(
                        "task polls over {:.0} ms out of {}",
                        metrics.tasks.slow_poll_threshold_ms, metrics.tasks.polls
                    ),
                ),
                BudgetId::SchedulerDelay => (
                    (metrics.scheduler_delay.samples > 0).then_some(metrics.scheduler_delay.max_ms),
                    format!(
                        "worst heartbeat lateness over {} samples",
                        metrics.scheduler_delay.samples
                    ),
                ),
                BudgetId::BrainQuery if metrics.brain.query.samples > 0 => (
                    Some(metrics.brain.query.p95_ms),
                    format!(
                        "query_brain tool calls: {} samples, p50 {:.1} ms, max {:.1} ms",
                        metrics.brain.query.samples,
                        metrics.brain.query.p50_ms,
                        metrics.brain.query.max_ms
                    ),
                ),
                BudgetId::StaticIndex => match metrics.brain.largest_index_run.as_deref() {
                    Some(run) if run.files >= STATIC_INDEX_FILES => (
                        Some(run.duration_ms as f64),
                        format!("full scan of {} files ({} parsed)", run.files, run.parsed),
                    ),
                    run => {
                        check.note = match run {
                            Some(run) => format!(
                                "n/a — largest scan so far: {} files in {} ms (needs ≥ {STATIC_INDEX_FILES})",
                                run.files, run.duration_ms
                            ),
                            None => "n/a — no repository indexed in this run".into(),
                        };
                        return check;
                    }
                },
                BudgetId::BrainQuery => {
                    check.note = "n/a — no query_brain calls in this run".into();
                    return check;
                }
                _ => (None, "not measured".into()),
            };
            check.measured = measured;
            check.note = note;
            check.status = match (measured, limit) {
                (Some(value), Some(limit)) if value < limit => CheckStatus::Pass,
                _ => CheckStatus::Fail,
            };
            if budget.id == BudgetId::IngestToPaint && ui.probes_painted < ui.probes_expected {
                check.status = CheckStatus::Fail;
            }
            check
        })
        .collect();
    SmokeReport {
        passed: checks.iter().all(|check| check.status != CheckStatus::Fail),
        platform: platform.into(),
        budget_tolerance: tolerance,
        cold_start_ms,
        checks,
    }
}

/// Prints the report, writes it to `BRIGADIER_SMOKE_REPORT` if set, then quits.
pub fn finish(app: &AppHandle, report: &SmokeReport) {
    let json = serde_json::to_string_pretty(report)
        .unwrap_or_else(|err| format!("{{\"error\":\"{err}\"}}"));
    println!("{json}");
    if let Some(path) = std::env::var_os(REPORT_ENV)
        && let Err(err) = std::fs::write(&path, &json)
    {
        eprintln!("could not write the smoke report: {err}");
    }
    shell::quit(app, if report.passed { 0 } else { 1 });
}

/// Fails the run if it has not finished in time.
pub fn start_watchdog(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(WATCHDOG).await;
        eprintln!("smoke check timed out after {WATCHDOG:?}");
        if let Some(path) = std::env::var_os(REPORT_ENV) {
            let _ = std::fs::write(&path, "{\"passed\":false,\"error\":\"timed out\"}");
        }
        shell::quit(&app, 2);
        // If even the quit hangs, leave anyway.
        tokio::time::sleep(Duration::from_secs(10)).await;
        std::process::exit(2);
    });
}
