//! Keeps the computer awake, screen on: while agents work, always, or never (the `keepAwake`
//! setting), and optionally with the lid closed (`keepAwakeLidClosed`). An overnight run holds
//! both until it ends, whatever the settings say, and leaves them as they were saved.
//!
//! - macOS: `caffeinate -d -i -s -w <pid>` holds the display, idle and system sleep assertions
//!   and ends with the daemon. No assertion stops a closed lid from sleeping the computer, so
//!   the lid sets `pmset disablesleep 1` instead, through a sudoers rule that allows only
//!   `pmset disablesleep 0` and `1`: Brigadier's own, installed once behind an administrator
//!   prompt, or one another keep-awake app installed. That setting outlives the process (and a
//!   reboot), so a marker file records it before it is set, a guard process restores it if the
//!   daemon dies, and startup restores what a crash left behind.
//! - Linux: `systemd-inhibit` blocks idle and sleep, and the lid switch when asked. The screen
//!   follows the desktop's own settings.
//! - Windows: `SetThreadExecutionState` from a thread of its own; the lid follows the power
//!   plan.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use brigadier_core::manager::SessionManager;
use brigadier_core::{Core, KeepAwake};
use brigadier_ipc::protocol::{KeepAwakeStatus, LidClosed};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// How often the setting and the agents' work are checked again.
const CHECK_EVERY: Duration = Duration::from_secs(10);

pub struct Awake {
    core: Arc<Core>,
    sessions: Arc<SessionManager>,
    data_dir: PathBuf,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    blocker: Option<Blocker>,
    #[cfg(target_os = "macos")]
    lid: Option<lid::Held>,
    /// Whether the sudoers rule lets the lid option work without a password; unknown until
    /// first needed.
    #[cfg(target_os = "macos")]
    authorized: Option<bool>,
    /// Held for an overnight run rather than for the settings.
    for_run: bool,
    error: Option<String>,
    /// Shut down: nothing keeps the computer awake any more, whatever the settings say.
    stopped: bool,
}

impl Awake {
    pub fn new(core: Arc<Core>, sessions: Arc<SessionManager>, data_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            core,
            sessions,
            data_dir,
            state: Mutex::new(State::default()),
        })
    }

    /// Restores sleep if a crashed daemon left it disabled.
    pub async fn recover(&self) {
        #[cfg(target_os = "macos")]
        lid::recover(&self.data_dir).await;
        #[cfg(not(target_os = "macos"))]
        let _ = &self.data_dir;
    }

    /// Applies the settings every few seconds until `stop`.
    pub async fn run(self: Arc<Self>, stop: CancellationToken) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(CHECK_EVERY);
        loop {
            tokio::select! {
                () = stop.cancelled() => return Ok(()),
                _ = tick.tick() => {
                    let mut state = self.state.lock().await;
                    self.hold(&mut state).await;
                }
            }
        }
    }

    /// Starts or stops keeping awake to match the settings now, and says how it stands.
    pub async fn apply(&self) -> KeepAwakeStatus {
        let mut state = self.state.lock().await;
        self.hold(&mut state).await;
        status(&mut state).await
    }

    async fn hold(&self, state: &mut State) {
        let settings = self.core.settings();
        // An overnight run keeps the computer awake whatever the setting (PLAN.md §10.10):
        // the user left it to work; the setting stays as they saved it.
        let run = self.sessions.overnight_active();
        let wanted = run
            || match settings.keep_awake {
                KeepAwake::Off => false,
                KeepAwake::Always => true,
                KeepAwake::Agents => self.sessions.agents_working().await,
            };
        let wanted = wanted && !state.stopped;
        let lid_wanted = lid_wanted(wanted, run, settings.keep_awake_lid_closed);
        state.for_run = wanted && run;
        state.error = None;

        let running = state.blocker.as_mut().is_some_and(Blocker::running);
        let current = state.blocker.as_ref().map(|blocker| blocker.lid);
        if !wanted || !running || current != Some(blocker_lid(lid_wanted)) {
            state.blocker = None;
        }
        if wanted && state.blocker.is_none() {
            match Blocker::start(blocker_lid(lid_wanted)) {
                Ok(blocker) => state.blocker = Some(blocker),
                Err(err) => {
                    tracing::warn!(error = %err, "could not keep the computer awake");
                    state.error = Some(format!("Could not keep the computer awake: {err}"));
                }
            }
        }

        #[cfg(target_os = "macos")]
        self.apply_lid(state, lid_wanted, !run).await;
    }

    /// Lets the lid option work without asking again, behind one administrator prompt.
    pub async fn set_up_lid_closed(&self) -> KeepAwakeStatus {
        #[cfg(target_os = "macos")]
        {
            let result = lid::set_up().await;
            let mut state = self.state.lock().await;
            state.authorized = None;
            if let Err(err) = result {
                drop(state);
                let mut status = self.apply().await;
                status.error = Some(err);
                return status;
            }
        }
        self.apply().await
    }

    /// Stops keeping awake and restores sleep for good, before the daemon exits (or while
    /// Brigadier is uninstalled).
    pub async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        state.stopped = true;
        state.blocker = None;
        // A run still under way restarts with the next daemon (the supervisor's), which holds
        // the lid again: a shut lid mustn't sleep the computer in between.
        #[cfg(target_os = "macos")]
        if let Some(held) = state.lid.take()
            && let Err(err) =
                lid::restore(&self.data_dir, held, !self.sessions.overnight_active()).await
        {
            tracing::warn!(error = %err, "could not restore sleep");
        }
    }

    /// `sleep_closed`: once sleep is back on, sleep now if the lid is already shut, as closing
    /// it would have (not while a run is under way, unless the battery is low).
    #[cfg(target_os = "macos")]
    async fn apply_lid(&self, state: &mut State, wanted: bool, sleep_closed: bool) {
        let low = if wanted || state.lid.is_some() {
            battery::read().await.and_then(|battery| battery.low())
        } else {
            None
        };
        if let Some(percent) = low.filter(|_| wanted) {
            state.error = Some(format!(
                "Closing the lid sleeps the computer again: the battery is at {percent}%."
            ));
        }
        let wanted = wanted && low.is_none();
        if !wanted {
            if let Some(held) = state.lid.take()
                && let Err(err) =
                    lid::restore(&self.data_dir, held, sleep_closed || low.is_some()).await
            {
                tracing::warn!(error = %err, "could not restore sleep");
                state.error = Some(format!("Could not restore sleep: {err}"));
            }
            return;
        }
        // Borrowed from another app that has since let sleep back on: take it over.
        if state.lid.as_ref().is_some_and(|held| !held.owned())
            && lid::sleep_disabled().await == Some(false)
        {
            state.lid = None;
        }
        if state.lid.is_some() {
            return;
        }
        if state.authorized == Some(false) {
            return;
        }
        match lid::disable_sleep(&self.data_dir).await {
            Ok(held) => {
                state.lid = Some(held);
                state.authorized = Some(true);
            }
            Err(err) => {
                tracing::info!(error = %err, "could not disable sleep for the lid");
                state.authorized = Some(false);
            }
        }
    }
}

/// Whether to keep going with the lid closed: whenever the computer is kept awake for a run,
/// else as the lid setting says.
fn lid_wanted(awake: bool, run: bool, setting: bool) -> bool {
    awake && (run || setting)
}

/// Whether the blocker itself holds the lid (Linux's inhibitor does; elsewhere it doesn't).
fn blocker_lid(lid_wanted: bool) -> bool {
    cfg!(target_os = "linux") && lid_wanted
}

async fn status(state: &mut State) -> KeepAwakeStatus {
    #[cfg(target_os = "macos")]
    let battery = battery::read().await;
    #[cfg(target_os = "macos")]
    let lid_closed = if state.lid.is_some() {
        LidClosed::Active
    } else {
        let authorized = match state.authorized {
            Some(known) => known,
            None => {
                let known = lid::authorized().await;
                state.authorized = Some(known);
                known
            }
        };
        if !authorized {
            LidClosed::NeedsSetup
        } else if battery.as_ref().and_then(battery::Battery::low).is_some() {
            LidClosed::LowBattery
        } else {
            LidClosed::Ready
        }
    };
    #[cfg(target_os = "linux")]
    let lid_closed = if state.blocker.as_ref().is_some_and(|blocker| blocker.lid) {
        LidClosed::Active
    } else {
        LidClosed::Ready
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let lid_closed = LidClosed::Unsupported;
    KeepAwakeStatus {
        active: state.blocker.is_some(),
        lid_closed,
        for_run: state.for_run && state.blocker.is_some(),
        screen_on: cfg!(any(target_os = "macos", windows)),
        #[cfg(target_os = "macos")]
        on_battery: battery.is_some_and(|battery| battery.on_battery),
        #[cfg(not(target_os = "macos"))]
        on_battery: false,
        error: state.error.clone(),
    }
}

/// The screen stays on (`-d`), and neither idling (`-i`) nor anything else on power (`-s`)
/// sleeps the computer.
#[cfg(target_os = "macos")]
const CAFFEINATE: [&str; 3] = ["-d", "-i", "-s"];

/// Holds the computer awake until dropped.
struct Blocker {
    lid: bool,
    #[cfg(unix)]
    child: tokio::process::Child,
    #[cfg(windows)]
    _release: std::sync::mpsc::Sender<()>,
}

impl Blocker {
    #[cfg(target_os = "macos")]
    fn start(lid: bool) -> std::io::Result<Self> {
        let child = tokio::process::Command::new("/usr/bin/caffeinate")
            .args(CAFFEINATE)
            .args(["-w", &std::process::id().to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self { lid, child })
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    fn start(lid: bool) -> std::io::Result<Self> {
        let what = if lid {
            "idle:sleep:handle-lid-switch"
        } else {
            "idle:sleep"
        };
        let child = tokio::process::Command::new("systemd-inhibit")
            .args([
                &format!("--what={what}"),
                "--who=Brigadier",
                "--why=Agents are working",
                "--mode=block",
                "sleep",
                "infinity",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        Ok(Self { lid, child })
    }

    #[cfg(windows)]
    fn start(lid: bool) -> std::io::Result<Self> {
        let (release, released) = std::sync::mpsc::channel::<()>();
        std::thread::Builder::new()
            .name("keep-awake".into())
            .spawn(move || {
                windows::hold();
                // Returns once the sender is dropped.
                let _ = released.recv();
                windows::release();
            })?;
        Ok(Self {
            lid,
            _release: release,
        })
    }

    /// Whether it still holds (a helper process can exit on its own).
    fn running(&mut self) -> bool {
        #[cfg(unix)]
        return matches!(self.child.try_wait(), Ok(None));
        #[cfg(windows)]
        return true;
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    pub fn hold() {
        // SAFETY: takes flags only; applies to the calling thread until changed again.
        unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
        };
    }

    pub fn release() {
        // SAFETY: as above.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

#[cfg(target_os = "macos")]
mod battery {
    /// Below this charge, on battery, the lid sleeps the computer again.
    const MIN_PERCENT: u32 = 10;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Battery {
        pub on_battery: bool,
        pub percent: Option<u32>,
    }

    impl Battery {
        /// The charge, when running on battery at or below the minimum.
        pub fn low(&self) -> Option<u32> {
            self.percent
                .filter(|percent| self.on_battery && *percent <= MIN_PERCENT)
        }
    }

    /// How the computer is powered now (`pmset -g batt`).
    pub async fn read() -> Option<Battery> {
        let output = tokio::process::Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .await
            .ok()?;
        Some(parse(&String::from_utf8_lossy(&output.stdout)))
    }

    pub(super) fn parse(report: &str) -> Battery {
        Battery {
            on_battery: report.contains("'Battery Power'"),
            percent: report
                .split(|c: char| c.is_whitespace() || c == ';')
                .find_map(|word| word.strip_suffix('%')?.parse::<u32>().ok()),
        }
    }
}

/// Whether the keep-awake-with-the-lid-closed sudoers rule is installed.
pub fn lid_rule_installed() -> bool {
    #[cfg(target_os = "macos")]
    return lid::rule_installed();
    #[cfg(not(target_os = "macos"))]
    false
}

/// Removes the keep-awake-with-the-lid-closed sudoers rule behind one administrator prompt, if
/// it is installed. What happened, said plainly.
pub async fn remove_lid_rule() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    return lid::remove_rule().await;
    #[cfg(not(target_os = "macos"))]
    Ok("There is no such rule on this system.".into())
}

#[cfg(target_os = "macos")]
mod lid {
    use std::path::{Path, PathBuf};
    use std::process::Stdio;

    const SUDO: &str = "/usr/bin/sudo";
    const PMSET: &str = "/usr/bin/pmset";
    /// Sudo skips files in sudoers.d whose name has a dot, so the rule is checked under
    /// `RULE.new` before it takes effect.
    const RULE: &str = "/etc/sudoers.d/brigadier-lid-closed";

    /// Sleep disabled by this daemon; holds its pid.
    fn marker(data_dir: &Path) -> PathBuf {
        data_dir.join("sleep-disabled")
    }

    /// Sleep disabled, and the guard that restores it if the daemon dies. Not `owned` when
    /// something else (another keep-awake app) had disabled it already: then it is left as is.
    pub struct Held {
        guard: Option<std::process::Child>,
        owned: bool,
    }

    impl Held {
        pub fn owned(&self) -> bool {
            self.owned
        }
    }

    async fn pmset_disablesleep(on: bool) -> Result<(), String> {
        if dry_run().is_some() {
            let value = if on { "1" } else { "0" };
            tracing::info!(command = %format!("{SUDO} -n {PMSET} disablesleep {value}"), "dry run: would change sleep");
            return Ok(());
        }
        let output = tokio::process::Command::new(SUDO)
            .args(["-n", PMSET, "disablesleep", if on { "1" } else { "0" }])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|err| err.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }

    /// Whether sleep is disabled system-wide now (`SleepDisabled` in `pmset -g`). A dry run
    /// reads it as off, so the hold it takes is its own.
    pub async fn sleep_disabled() -> Option<bool> {
        if dry_run().is_some() {
            return Some(false);
        }
        let output = tokio::process::Command::new(PMSET)
            .arg("-g")
            .output()
            .await
            .ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| {
                let mut words = line.split_whitespace();
                (words.next()? == "SleepDisabled").then(|| words.next() == Some("1"))
            })
    }

    /// Whether sleep can be turned off and on again without a password: reads what sudo
    /// allows, changing nothing. Any rule that allows it will do (Brigadier's, or another
    /// keep-awake app's).
    pub async fn authorized() -> bool {
        if dry_run().is_some() {
            return rule_installed();
        }
        let Ok(output) = tokio::process::Command::new(SUDO)
            .args(["-n", "-l"])
            .stdin(Stdio::null())
            .output()
            .await
        else {
            return false;
        };
        let listing = String::from_utf8_lossy(&output.stdout);
        output.status.success()
            && ["1", "0"].into_iter().all(|value| {
                super::sudo::allows_without_password(
                    &listing,
                    &format!("{PMSET} disablesleep {value}"),
                )
            })
    }

    /// Installs the sudoers rule behind an administrator prompt.
    pub async fn set_up() -> Result<(), String> {
        let uid = nix::unistd::getuid();
        let rule =
            format!("#{uid} ALL=(root) NOPASSWD: {PMSET} disablesleep 1, {PMSET} disablesleep 0");
        let script = format!(
            "mkdir -p /etc/sudoers.d && echo '{rule}' > {RULE}.new && chmod 0440 {RULE}.new \
             && /usr/sbin/visudo -c -q -f {RULE}.new && mv -f {RULE}.new {RULE} \
             || {{ rm -f {RULE}.new; exit 1; }}"
        );
        let apple_script = format!(
            "do shell script \"{}\" with administrator privileges with prompt \
             \"Brigadier wants to keep working with the lid closed. It asks once; after that \
             it can only turn sleep off and on again.\"",
            applescript_escape(&script)
        );
        let output = tokio::process::Command::new("/usr/bin/osascript")
            .args(["-e", &apple_script])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|err| err.to_string())?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.contains("-128") {
            "The administrator password was not given.".to_owned()
        } else {
            format!("Setting up failed: {}", stderr.trim())
        })
    }

    /// A development build started with `BRIGADIER_LID_RULE_DRY_RUN=<file>` looks at that
    /// stand-in instead of the rule and only says what it would run, so uninstalling and a
    /// run's lid hold can be tried without touching the rule an installed Brigadier uses or
    /// this computer's sleep setting: the stand-in alone says whether the rule is there, and
    /// sleep is never changed, marked, guarded or slept.
    fn dry_run() -> Option<PathBuf> {
        if !cfg!(debug_assertions) {
            return None;
        }
        std::env::var_os("BRIGADIER_LID_RULE_DRY_RUN")
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    }

    fn rule_path() -> PathBuf {
        dry_run().unwrap_or_else(|| PathBuf::from(RULE))
    }

    pub fn rule_installed() -> bool {
        std::fs::symlink_metadata(rule_path()).is_ok()
    }

    /// Removes the sudoers rule behind an administrator prompt.
    pub async fn remove_rule() -> Result<String, String> {
        if !rule_installed() {
            return Ok("It wasn't installed.".into());
        }
        if let Some(stand_in) = dry_run() {
            let script = format!("/bin/rm -f {}", stand_in.display());
            tracing::info!(command = %script, "dry run: would remove the lid-closed sudoers rule as administrator");
            return Ok(format!("Dry run: would run “{script}” as administrator."));
        }
        let script = format!("/bin/rm -f {RULE}");
        let apple_script = format!(
            "do shell script \"{}\" with administrator privileges with prompt \
             \"Brigadier is being uninstalled and removes the rule that let it keep your Mac \
             awake with the lid closed.\"",
            applescript_escape(&script)
        );
        let output = tokio::process::Command::new("/usr/bin/osascript")
            .args(["-e", &apple_script])
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|err| err.to_string())?;
        if output.status.success() && !rule_installed() {
            return Ok("Removed.".into());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.contains("-128") {
            format!("The administrator password was not given; remove it with: sudo rm {RULE}")
        } else {
            format!(
                "It stays ({}); remove it with: sudo rm {RULE}",
                stderr.trim()
            )
        })
    }

    fn applescript_escape(text: &str) -> String {
        text.replace('\\', "\\\\").replace('"', "\\\"")
    }

    /// Disables sleep system-wide, recording it first so that it is undone even after a crash.
    pub async fn disable_sleep(data_dir: &Path) -> Result<Held, String> {
        if sleep_disabled().await == Some(true) {
            tracing::info!("sleep is already disabled by something else; leaving it to it");
            return Ok(Held {
                guard: None,
                owned: false,
            });
        }
        if dry_run().is_some() {
            // Nothing changes, so there is nothing for a guard or a later daemon to undo.
            pmset_disablesleep(true).await?;
            return Ok(Held {
                guard: None,
                owned: true,
            });
        }
        let marker = marker(data_dir);
        let pid = std::process::id().to_string();
        std::fs::write(&marker, &pid).map_err(|err| err.to_string())?;
        if let Err(err) = pmset_disablesleep(true).await {
            let _ = std::fs::remove_file(&marker);
            return Err(err);
        }
        tracing::info!("sleep disabled for the closed lid");
        Ok(Held {
            guard: spawn_guard(&pid, &marker),
            owned: true,
        })
    }

    /// A process of its own that restores sleep once the daemon is gone, unless a newer
    /// daemon took the marker over.
    fn spawn_guard(pid: &str, marker: &Path) -> Option<std::process::Child> {
        use std::os::unix::process::CommandExt;
        let script = r#"while kill -0 "$1" 2>/dev/null; do sleep 5; done
if [ "$(cat "$2" 2>/dev/null)" = "$1" ]; then /usr/bin/sudo -n /usr/bin/pmset disablesleep 0 && rm -f "$2"; fi"#;
        std::process::Command::new("/bin/sh")
            .args(["-c", script, "brigadier-sleep-guard", pid])
            .arg(marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .inspect_err(|err| tracing::warn!(error = %err, "could not start the sleep guard"))
            .ok()
    }

    /// Enables sleep again. With `sleep_closed`, also sleeps now if the lid is already closed
    /// without an external display: restoring the setting alone doesn't, as macOS looks at
    /// the lid only when it opens or closes.
    pub async fn restore(
        data_dir: &Path,
        mut held: Held,
        sleep_closed: bool,
    ) -> Result<(), String> {
        if !held.owned {
            return Ok(());
        }
        if let Some(mut guard) = held.guard.take() {
            let _ = guard.kill();
            let _ = guard.wait();
        }
        pmset_disablesleep(false).await?;
        let _ = std::fs::remove_file(marker(data_dir));
        tracing::info!("sleep restored");
        if sleep_closed {
            sleep_if_lid_closed().await;
        }
        Ok(())
    }

    /// Restores what a daemon that died with sleep disabled left behind.
    pub async fn recover(data_dir: &Path) {
        let marker = marker(data_dir);
        if !marker.exists() {
            return;
        }
        match pmset_disablesleep(false).await {
            Ok(()) => {
                let _ = std::fs::remove_file(&marker);
                tracing::info!("restored sleep left disabled by an earlier run");
            }
            Err(err) => tracing::warn!(error = %err, "could not restore sleep left disabled"),
        }
    }

    /// Sleeps the computer if its lid is shut and that would sleep it (no external display).
    /// powerd takes a moment to apply sleep coming back on and refuses until then, so a
    /// refusal, or a lid that doesn't read as sleeping yet, is tried again for a few seconds.
    async fn sleep_if_lid_closed() {
        if dry_run().is_some() {
            return;
        }
        for _ in 0..10 {
            let Some((closed, sleeps)) = lid_state().await else {
                return;
            };
            if !closed {
                return;
            }
            if sleeps && iokit::sleep_system() {
                tracing::info!("the lid is closed: sleeping now");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        tracing::info!("the lid is closed but the computer didn't sleep");
    }

    /// Whether the lid is closed, and whether that sleeps the computer now.
    async fn lid_state() -> Option<(bool, bool)> {
        let output = tokio::process::Command::new("/usr/sbin/ioreg")
            .args(["-r", "-k", "AppleClamshellState", "-d", "1"])
            .output()
            .await
            .ok()?;
        Some(super::parse_lid(&String::from_utf8_lossy(&output.stdout)))
    }

    #[allow(unsafe_code)]
    mod iokit {
        #[link(name = "IOKit", kind = "framework")]
        unsafe extern "C" {
            fn IOPMFindPowerManagement(main_port: u32) -> u32;
            fn IOPMSleepSystem(connection: u32) -> i32;
            fn IOServiceClose(connection: u32) -> i32;
        }

        /// Asks the computer to sleep now; whether it agreed.
        pub fn sleep_system() -> bool {
            // SAFETY: plain mach port values; the connection is closed before returning.
            unsafe {
                let connection = IOPMFindPowerManagement(0);
                if connection == 0 {
                    return false;
                }
                let result = IOPMSleepSystem(connection);
                IOServiceClose(connection);
                result == 0
            }
        }
    }
}

/// From `ioreg -r -k AppleClamshellState`: whether the lid is closed, and whether that sleeps
/// the computer now.
#[cfg(any(target_os = "macos", test))]
fn parse_lid(report: &str) -> (bool, bool) {
    (
        report.contains("\"AppleClamshellState\" = Yes"),
        report.contains("\"AppleClamshellCausesSleep\" = Yes"),
    )
}

/// Reading `sudo -l`.
#[cfg(any(target_os = "macos", test))]
mod sudo {
    /// Whether the listing lets `command` run as root without a password. Entries are
    /// `(runas) [TAG: …] command, command`, wrapped onto lines indented further; a tag holds
    /// for the commands after it in the entry. `ALL` or the bare program allow any arguments.
    pub fn allows_without_password(listing: &str, command: &str) -> bool {
        let program = command.split_whitespace().next().unwrap_or_default();
        let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
        let Some((_, rules)) = listing.split_once("may run the following commands") else {
            return false;
        };
        // Joins each entry's wrapped lines: an entry starts 4 spaces in, its rest 8.
        let mut entries: Vec<String> = Vec::new();
        for line in rules.lines().skip(1) {
            if line.starts_with("        ") {
                if let Some(entry) = entries.last_mut() {
                    entry.push(' ');
                    entry.push_str(line.trim());
                }
            } else if line.starts_with("    ") {
                entries.push(line.trim().to_owned());
            }
        }
        entries.iter().any(|entry| {
            let Some(rest) = entry.strip_prefix('(') else {
                return false;
            };
            let Some((runas, commands)) = rest.split_once(')') else {
                return false;
            };
            let users = runas.split(':').next().unwrap_or_default();
            if !users
                .split(',')
                .any(|user| matches!(user.trim(), "root" | "ALL"))
            {
                return false;
            }
            let mut no_password = false;
            commands.split(", ").any(|spec| {
                let mut words = spec.split_whitespace().peekable();
                while let Some(tag) = words.next_if(|word| {
                    word.ends_with(':')
                        && word[..word.len() - 1]
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c == '_')
                }) {
                    match tag {
                        "NOPASSWD:" => no_password = true,
                        "PASSWD:" => no_password = false,
                        _ => {}
                    }
                }
                let spec = words.collect::<Vec<_>>().join(" ");
                no_password && (spec == "ALL" || spec == program || spec == command)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn low_battery_only_on_battery_power() {
        use super::battery::parse;
        let on_battery = "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t8%; discharging; 0:20 remaining present: true";
        assert_eq!(parse(on_battery).low(), Some(8));
        assert!(parse(on_battery).on_battery);
        let charged =
            "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=1)\t54%; discharging;";
        assert_eq!(parse(charged).low(), None);
        let plugged = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=1)\t5%; charging;";
        assert_eq!(parse(plugged).low(), None);
        assert!(!parse(plugged).on_battery);
        // A desktop Mac: no battery at all.
        assert_eq!(parse("Now drawing from 'AC Power'\n").low(), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn keeps_the_screen_on_and_the_computer_awake() {
        assert_eq!(CAFFEINATE, ["-d", "-i", "-s"]);
    }

    #[test]
    fn a_run_keeps_going_with_the_lid_closed_whatever_the_setting() {
        assert!(lid_wanted(true, true, false));
        assert!(lid_wanted(true, false, true));
        assert!(!lid_wanted(true, false, false));
        // Nothing keeps the computer awake (shut down, say): neither does the lid.
        assert!(!lid_wanted(false, true, true));
    }

    #[test]
    fn reads_the_lid() {
        let closed = r#"| |   "AppleClamshellState" = Yes
| |   "AppleClamshellCausesSleep" = Yes"#;
        assert_eq!(parse_lid(closed), (true, true));
        let external = r#""AppleClamshellState" = Yes
"AppleClamshellCausesSleep" = No"#;
        assert_eq!(parse_lid(external), (true, false));
        assert_eq!(parse_lid(r#""AppleClamshellState" = No"#), (false, false));
    }

    /// `sudo -n -l` on a Mac with another keep-awake app's rule; the listing wraps at 80
    /// columns, mid-command.
    const LISTING: &str = "Matching Defaults entries for someone on host:
    env_reset, env_keep+=BLOCKSIZE, env_keep+=\"COLORFGBG COLORTERM\",
    lecture_file=/etc/sudo_lecture, !log_allowed

User someone may run the following commands on host:
    (ALL) ALL
    (root) NOPASSWD: /usr/bin/pmset disablesleep 1, /usr/bin/pmset disablesleep
        0
";

    #[test]
    fn finds_a_password_free_rule_in_sudo_listing() {
        use super::sudo::allows_without_password as allows;
        assert!(allows(LISTING, "/usr/bin/pmset disablesleep 1"));
        assert!(allows(LISTING, "/usr/bin/pmset disablesleep 0"));
        // Allowed only with the password.
        assert!(!allows(LISTING, "/usr/bin/pmset sleepnow"));
        let admin_only = "User someone may run the following commands on host:\n    (ALL) ALL\n";
        assert!(!allows(admin_only, "/usr/bin/pmset disablesleep 1"));
        let everything =
            "User someone may run the following commands on host:\n    (ALL : ALL) NOPASSWD: ALL\n";
        assert!(allows(everything, "/usr/bin/pmset disablesleep 1"));
        let program = "User someone may run the following commands on host:\n    (root) SETENV: NOPASSWD: /usr/bin/pmset\n";
        assert!(allows(program, "/usr/bin/pmset disablesleep 0"));
        let other_user =
            "User someone may run the following commands on host:\n    (nobody) NOPASSWD: ALL\n";
        assert!(!allows(other_user, "/usr/bin/pmset disablesleep 1"));
        let back_to_password = "User someone may run the following commands on host:\n    (root) NOPASSWD: /bin/ls, PASSWD: /usr/bin/pmset disablesleep 1\n";
        assert!(!allows(back_to_password, "/usr/bin/pmset disablesleep 1"));
        assert!(!allows("", "/usr/bin/pmset disablesleep 1"));
    }
}
