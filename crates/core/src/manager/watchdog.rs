//! How long a thread turn may stay still before the watchdog says so (`conversation/watchdog.rs`),
//! and what counts as progress. Commands and tool calls are never interrupted for taking time.

pub(crate) mod progress;

use std::time::Duration;

/// The silence before a report, in seconds; every other threshold follows from it.
const UNIT_SECS: u64 = 3 * 60;

/// How long things may stay still before the watchdog acts, in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Timing {
    /// A thread turn silent this long is reported to the user.
    pub silent: i64,
    /// Command timing metadata, never a reason to act on a running call.
    pub command: i64,
    pub server: i64,
    pub build: i64,
    /// What a hand-off or a result being recorded may take before a task without a running
    /// CLI, or a member that ended without its result, counts as stuck.
    pub grace: i64,
    /// How often the watchdog looks.
    pub tick: Duration,
}

impl Timing {
    /// Three minutes of silence before a report, with a separate process-exit grace.
    /// A debug build takes `BRIGADIER_STALL_SECS` instead of the three minutes.
    pub(crate) fn current() -> Self {
        if cfg!(debug_assertions)
            && let Some(secs) = std::env::var("BRIGADIER_STALL_SECS")
                .ok()
                .and_then(|value| value.trim().parse::<u64>().ok())
                .filter(|secs| *secs > 0)
        {
            return Self::scaled(secs);
        }
        Self::scaled(UNIT_SECS)
    }

    pub(crate) fn scaled(unit_secs: u64) -> Self {
        let unit = i64::try_from(unit_secs.min(24 * 60 * 60)).unwrap_or(0) * 1_000;
        let tick = u64::try_from(unit / 4).unwrap_or(0).clamp(5_000, 60_000);
        Self {
            silent: unit,
            command: unit * 5 / 3,
            server: unit * 2 / 3,
            build: unit * 10,
            grace: (unit * 2 / 3).max(1_000),
            tick: Duration::from_millis(tick),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    #[test]
    fn the_thresholds_scale_with_one_unit() {
        let t = Timing::scaled(UNIT_SECS);
        assert_eq!((t.silent, t.command), (3 * MIN, 5 * MIN));
        assert_eq!(t.tick, Duration::from_secs(45));
        let fast = Timing::scaled(60);
        assert_eq!((fast.silent, fast.command), (MIN, 100_000));
        assert_eq!(fast.tick, Duration::from_secs(15));
        assert_eq!(Timing::scaled(1).tick, Duration::from_secs(5));
    }
}
