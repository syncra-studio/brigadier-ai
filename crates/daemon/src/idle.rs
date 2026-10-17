//! The daemon quits on its own once nobody uses it: no app has been connected for
//! [`IDLE_EXIT`] and nothing runs (no turn or worker, Brain job, index scan, download,
//! terminal or dictation, and keeping awake isn't set to "Always"). The daemon keeps running
//! when the window closes so that unattended work goes on (PLAN.md §3), but once that work is
//! done and the app is gone (quit, or crashed), nothing would ever end it. An app that went
//! without quitting it (killed, or crashed) isn't waited for: the daemon quits as soon as
//! nothing runs. The app launches it again when it starts.

use std::sync::Arc;
use std::time::{Duration, Instant};

use brigadier_core::KeepAwake;
use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// How long the daemon waits with no app connected before it quits, once nothing runs.
const IDLE_EXIT: Duration = Duration::from_secs(30 * 60);
/// How often it looks.
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// What runs now, said plainly; empty when nothing does.
pub async fn running(daemon: &Daemon) -> Vec<String> {
    let mut running = Vec::new();
    if daemon.sessions.overnight_active() {
        running.push("an overnight run".to_owned());
    } else if daemon.sessions.agents_working().await {
        running.push("a turn or a worker".to_owned());
    }
    running.extend(daemon.sessions.brain_work());
    match daemon.terminals.count() {
        0 => {}
        1 => running.push("a terminal".to_owned()),
        n => running.push(format!("{n} terminals")),
    }
    running.extend(daemon.dictation.work());
    if daemon.core.settings().keep_awake == KeepAwake::Always {
        running.push("keeping the computer awake (Always)".to_owned());
    }
    running
}

/// Asks the daemon to quit once no app has been connected for [`IDLE_EXIT`] and nothing runs.
pub async fn exit_when_idle(daemon: Arc<Daemon>, stop: CancellationToken) -> anyhow::Result<()> {
    let wait = idle_exit();
    let every = CHECK_EVERY.min(wait);
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut alone_since: Option<Instant> = None;
    loop {
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            _ = tick.tick() => {}
        }
        if daemon.metrics.clients() > 0 {
            alone_since = None;
            continue;
        }
        // Uninstalled, and the app went without asking this daemon to quit (it crashed): the
        // finisher waits for this daemon before it removes the rest.
        if daemon.uninstall.started() {
            let _ = daemon.quit.try_send("uninstalled, and no app is connected");
            return Ok(());
        }
        let since = *alone_since.get_or_insert_with(Instant::now);
        let platform = daemon.runtime.platform();
        let gone = app_gone(daemon.metrics.last_app(), |pid| {
            platform.processes().is_alive(pid)
        });
        if !due(since.elapsed(), wait, gone) || !running(&daemon).await.is_empty() {
            continue;
        }
        let _ = daemon.quit.try_send(if gone {
            "the app is gone and nothing is running"
        } else {
            "no app connected for a while and nothing running"
        });
        return Ok(());
    }
}

/// Whether the app that last used the daemon has gone without asking it to quit (killed, or
/// crashed): no app comes back until one is started anew.
fn app_gone(last_app: Option<u32>, alive: impl Fn(u32) -> bool) -> bool {
    last_app.is_some_and(|pid| !alive(pid))
}

/// Whether the daemon, with no app connected for `alone` and nothing running, quits now: after
/// `wait`, or at once when the app is gone, rather than outliving it as an orphan.
fn due(alone: Duration, wait: Duration, app_gone: bool) -> bool {
    app_gone || alone >= wait
}

/// [`IDLE_EXIT`], or in a debug build `BRIGADIER_IDLE_EXIT_SECS` (to verify it without
/// waiting half an hour).
fn idle_exit() -> Duration {
    #[cfg(debug_assertions)]
    if let Some(secs) = std::env::var("BRIGADIER_IDLE_EXIT_SECS")
        .ok()
        .and_then(|secs| secs.parse::<u64>().ok())
    {
        return Duration::from_secs(secs.max(1));
    }
    IDLE_EXIT
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{app_gone, due};

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    /// An app killed by its process id leaves no orphan daemon: with nothing running, the
    /// daemon quits at once instead of waiting half an hour for an app (#99).
    #[cfg(unix)]
    #[test]
    fn a_daemon_quits_at_once_when_its_app_was_killed() {
        let wait = Duration::from_secs(30 * 60);
        let mut app = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = app.id();
        assert!(!app_gone(Some(pid), alive));
        assert!(!due(Duration::ZERO, wait, app_gone(Some(pid), alive)));
        app.kill().unwrap();
        app.wait().unwrap();
        assert!(app_gone(Some(pid), alive));
        assert!(due(Duration::ZERO, wait, app_gone(Some(pid), alive)));
    }

    /// With no app ever connected (started on its own), or once the wait is over, it goes as
    /// before.
    #[test]
    fn without_an_app_the_daemon_waits_as_before() {
        let wait = Duration::from_secs(30 * 60);
        assert!(!app_gone(None, |_| false));
        assert!(!due(Duration::from_secs(60), wait, false));
        assert!(due(wait, wait, false));
    }
}
