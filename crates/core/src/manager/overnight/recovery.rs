//! Resuming runs after a restart (PLAN.md §10.10). Run ownership is replayed before the
//! generic recovery (which ends interrupted tasks). Afterwards each active run picks up where
//! it was: the thread hears that Brigadier restarted (and of the tasks the restart ended, as
//! usual) and carries on with the run's plan, a run past its deadline winds down at once, and
//! an ending that was cut off finishes. The time Brigadier wasn't running is recorded on the
//! run, with the Mac's own sleep record when it has one, for the report.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::super::SessionManager;
use crate::now_ms;
use crate::overnight::{OvernightRun, OvernightState, RunGap};

/// A heartbeat older than this many clock ticks means Brigadier wasn't running in between.
const MISSED_BEATS: i64 = 3;

impl SessionManager {
    fn heartbeat_path(&self) -> PathBuf {
        self.data_dir.join("overnight-heartbeat")
    }

    /// Records that Brigadier runs now, while any run is active.
    pub(crate) async fn beat(&self) {
        if self.overnight.active.all().is_empty() {
            return;
        }
        let _ = tokio::fs::write(self.heartbeat_path(), now_ms().to_string()).await;
    }

    /// After a restart, once the generic recovery is done: every active run carries on.
    pub(crate) async fn resume_runs(&self) {
        self.overnight.recovering.store(false, Ordering::Release);
        let now = now_ms();
        let last = tokio::fs::read_to_string(self.heartbeat_path())
            .await
            .ok()
            .and_then(|text| text.trim().parse::<i64>().ok());
        let missed = MISSED_BEATS * super::wind_down::CLOCK.as_millis() as i64;
        for (conversation_id, active) in self.overnight.active.all() {
            let Ok(board) = self.core.board(&conversation_id).await else {
                continue;
            };
            let Some(run) = board.runs.get(&active.id).cloned() else {
                continue;
            };
            // The run's own part of the time Brigadier was down: a beat from before it started
            // (an earlier run's) counts from its start.
            let gap = match (last, run.started_at_ms) {
                (Some(last), Some(started)) => Some((last.max(started), now)),
                _ => None,
            };
            let run = match gap.filter(|(from, to)| to - from > missed) {
                Some((from, to)) => {
                    let gap = RunGap {
                        from_ms: from,
                        to_ms: to,
                        cause: sleep_cause(from, to).await,
                    };
                    self.change_run_if(&run, |now| {
                        now.gaps.push(gap.clone());
                        Some(())
                    })
                    .await
                    .unwrap_or(run)
                }
                _ => run,
            };
            self.resume_run(run).await;
        }
        self.beat().await;
    }

    async fn resume_run(&self, run: OvernightRun) {
        tracing::info!(run = %run.id, state = ?run.state, "resuming an overnight run after a restart");
        // Past its wind-down instant while Brigadier was away: end now (the report says late).
        if run.wind_down_at_ms.is_some_and(|at| now_ms() >= at)
            && !matches!(
                run.state,
                OvernightState::WindingDown | OvernightState::Reporting
            )
        {
            let _ = self.deadline_reached(&run.conversation_id, &run.id).await;
            return;
        }
        match run.state {
            OvernightState::WindingDown | OvernightState::Reporting => self.wind_down_soon(&run),
            // The thread carries on with the plan; what the restart ended reaches it as usual.
            OvernightState::Running => {
                self.tell_thread(
                    &run,
                    "run restarted",
                    "[run] Brigadier restarted during the overnight run. list_tasks shows what still runs; carry on with the run's plan.".into(),
                )
                .await;
            }
            OvernightState::Preparing
            | OvernightState::WaitingQuota
            | OvernightState::Proposed
            | OvernightState::Superseded
            | OvernightState::Finished => {}
        }
    }
}

/// Why Brigadier wasn't running from `from` to `to`: the Mac's sleep record when it shows a
/// sleep in that time, else plain words.
async fn sleep_cause(from: i64, to: i64) -> String {
    let slept = mac_sleep(from, to).await;
    match slept {
        Some((asleep, awake)) => {
            format!("The Mac slept {}–{}", clock_time(asleep), clock_time(awake))
        }
        None => format!(
            "Brigadier was unavailable {}–{}",
            clock_time(from),
            clock_time(to)
        ),
    }
}

fn clock_time(at_ms: i64) -> String {
    jiff::Timestamp::from_millisecond(at_ms)
        .map(|at| {
            at.to_zoned(jiff::tz::TimeZone::system())
                .strftime("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|_| "?".into())
}

/// The first sleep and the last wake the power log records inside the gap, if any.
#[cfg(target_os = "macos")]
async fn mac_sleep(from: i64, to: i64) -> Option<(i64, i64)> {
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new("/usr/bin/pmset")
            .args(["-g", "log"])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .ok()?
    .ok()?;
    parse_power_log(&String::from_utf8_lossy(&output.stdout), from, to)
}

#[cfg(not(target_os = "macos"))]
async fn mac_sleep(_from: i64, _to: i64) -> Option<(i64, i64)> {
    let _ = Duration::ZERO;
    None
}

/// `2026-09-29 10:43:37 +0300 Sleep   Entering Sleep state …` lines: the first Sleep at or after
/// `from` (less a minute) and the last Wake before `to` (plus a minute).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_power_log(log: &str, from: i64, to: i64) -> Option<(i64, i64)> {
    let slack = 60_000;
    let mut asleep = None;
    let mut awake = None;
    for line in log.lines() {
        let mut fields = line.split_whitespace();
        let (Some(date), Some(time), Some(offset), Some(kind)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if kind != "Sleep" && kind != "Wake" && kind != "DarkWake" {
            continue;
        }
        let Ok(at) = format!("{date} {time} {offset}")
            .parse::<jiff::Zoned>()
            .or_else(|_| {
                jiff::fmt::strtime::parse("%Y-%m-%d %H:%M:%S %z", format!("{date} {time} {offset}"))
                    .and_then(|parsed| parsed.to_zoned())
            })
        else {
            continue;
        };
        let at = at.timestamp().as_millisecond();
        if at < from - slack || at > to + slack {
            continue;
        }
        match kind {
            "Sleep" if asleep.is_none() => asleep = Some(at),
            "Wake" | "DarkWake" if asleep.is_some() => awake = Some(at),
            _ => {}
        }
    }
    Some((asleep?, awake.unwrap_or(to)))
}
