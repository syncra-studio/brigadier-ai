//! Brigadier's side of the computer-use helper: what Settings and the menu bar show, what
//! their buttons do, and the watchdog that keeps the helper answering.
//!
//! The helper's socket only takes peers signed like the helper, so every request goes through
//! the helper's own binary (`brigadier-cu host …`, one JSON line out). Showing the status never
//! starts the helper: a running one is asked, otherwise what it last wrote is shown, marked as
//! not current. Only a button the user presses (Grant…, Check again, the capture check) may
//! start it.
//!
//! The watchdog looks every [`TICK`] while computer use is on and a helper process is
//! recorded. A helper that stops answering twice in a row (a stopped process, a wedged main
//! loop) is restarted; right after the Mac wakes (the wall clock jumped past the monotonic
//! one), once is enough. Agents' own shims also replace a dead helper on their next call, so
//! between the two nothing ever needs the user to turn computer use off and on.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;
use tokio::sync::broadcast;

use super::SessionManager;
use crate::{
    ComputerPermission, ComputerPlatform, ComputerSession, ComputerSetup, ComputerUseStatus,
    ConversationId, PermissionState, TaskId,
};

/// How often the watchdog looks.
const TICK: Duration = Duration::from_secs(2);
/// Unanswered looks in a row before a helper counts as hung.
const HUNG_STRIKES: u32 = 2;
/// A wall-clock jump this much past the monotonic clock means the Mac slept.
const WAKE_GAP: Duration = Duration::from_secs(30);
/// The longest a `host` request may take: a restart waits for the old helper to stop and the
/// new one to answer.
const HOST_TIMEOUT: Duration = Duration::from_secs(45);
/// Refreshing the helper (so it sees a new Screen Recording grant) at most this often.
const REFRESH_SPACING: Duration = Duration::from_secs(5);
/// Written once screen capture is confirmed: setup is finished.
pub(crate) const CAPTURE_CONFIRMED: &str = "capture-confirmed";
/// Touched by an agent's call that starts waiting for setup: the setup window opens.
const SETUP_NEEDED: &str = "setup-needed";
/// The helper's mark that computer use is off: its shims refuse calls and start nothing.
const OFF_MARKER: &str = "off";

/// What the host tells Brigadier's windows.
#[derive(Debug, Clone)]
pub enum ComputerNews {
    Status(ComputerUseStatus),
    /// An agent needs computer use and setup isn't finished.
    SetupNeeded,
}

/// The helper as Brigadier tends it.
pub(crate) struct ComputerHost {
    state: std::sync::Mutex<HostState>,
    news: broadcast::Sender<ComputerNews>,
    /// Held while the helper is refreshed; concurrent Check agains wait and share it.
    refresh: tokio::sync::Mutex<Option<Instant>>,
    /// Held while the helper is turned on or off, so turns follow one another.
    turning: tokio::sync::Mutex<()>,
}

impl Default for ComputerHost {
    fn default() -> Self {
        Self {
            state: std::sync::Mutex::new(HostState::default()),
            news: broadcast::channel(16).0,
            refresh: tokio::sync::Mutex::new(None),
            turning: tokio::sync::Mutex::new(()),
        }
    }
}

struct HostState {
    strikes: u32,
    trouble: Option<Trouble>,
    last: Option<ComputerUseStatus>,
    /// The newest setup request answered (its file's change time); older ones, also from
    /// before Brigadier started, open nothing.
    setup_asked: SystemTime,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            strikes: 0,
            trouble: None,
            last: None,
            setup_asked: SystemTime::now(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Trouble {
    Restarting,
    Failed(String),
}

/// `brigadier-cu host status`.
#[derive(Debug, Default, Deserialize)]
struct Raw {
    #[serde(default)]
    state: String,
    #[serde(default)]
    permissions: Option<RawPermissions>,
    #[serde(default)]
    sessions: Vec<RawSession>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct RawPermissions {
    accessibility: bool,
    screen_recording: bool,
}

/// Windows and Linux: what the helper says about the desktop session, beside its
/// permissions (`permissions.platform` of `host status`).
#[derive(Debug, Default, Deserialize)]
struct RawPlatformStatus {
    #[serde(default)]
    permissions: Option<RawPlatformPermissions>,
}

#[derive(Debug, Default, Deserialize)]
struct RawPlatformPermissions {
    #[serde(default)]
    platform: Option<RawPlatform>,
}

#[derive(Debug, Default, Deserialize)]
struct RawPlatform {
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawSession {
    session: String,
    #[serde(default)]
    apps: Vec<RawApp>,
}

#[derive(Debug, Deserialize)]
struct RawApp {
    name: String,
    #[serde(default)]
    bundle_id: Option<String>,
    #[serde(default)]
    pid: Option<i64>,
}

fn state_of(granted: Option<bool>) -> PermissionState {
    match granted {
        Some(true) => PermissionState::Granted,
        Some(false) => PermissionState::NotGranted,
        None => PermissionState::Unknown,
    }
}

/// Where setup stands, from what is known.
fn setup_of(
    enabled: bool,
    trouble: Option<&Trouble>,
    permissions: Option<RawPermissions>,
    capture_confirmed: bool,
) -> ComputerSetup {
    if !enabled {
        return ComputerSetup::Off;
    }
    match trouble {
        Some(Trouble::Restarting) => return ComputerSetup::Restarting,
        Some(Trouble::Failed(message)) => {
            return ComputerSetup::Failed {
                message: message.clone(),
            };
        }
        None => {}
    }
    match permissions {
        // Unread (no helper since): the capture check passing is what finished setup.
        None if capture_confirmed => ComputerSetup::Ready,
        None => ComputerSetup::NotSetUp,
        Some(p) if !p.accessibility || !p.screen_recording => ComputerSetup::Missing,
        Some(_) if !capture_confirmed => ComputerSetup::ConfirmCapture,
        Some(_) => ComputerSetup::Ready,
    }
}

/// Whether process `pid` exists.
#[cfg(unix)]
fn alive(pid: i32) -> bool {
    pid > 1 && nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_ok()
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn alive(pid: i32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };
    let Ok(pid) = u32::try_from(pid) else {
        return false;
    };
    // SAFETY: the handle is checked and closed; a zero timeout only polls it.
    unsafe {
        let process = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if process.is_null() {
            return false;
        }
        let running = WaitForSingleObject(process, 0) == WAIT_TIMEOUT;
        CloseHandle(process);
        running
    }
}

#[cfg(not(any(unix, windows)))]
fn alive(_: i32) -> bool {
    false
}

/// Whether the Mac slept between two looks: the wall clock moved well past the monotonic one.
fn slept(wall: Duration, monotonic: Duration) -> bool {
    wall.saturating_sub(monotonic) >= WAKE_GAP
}

impl SessionManager {
    fn computer_home(&self) -> PathBuf {
        self.data_dir.join("computer")
    }

    /// The helper binary `host` requests run: the placed copy, else the one bundled.
    #[cfg(target_os = "macos")]
    fn computer_binary(&self) -> Option<PathBuf> {
        let placed = self.computer_home().join(super::computer_use::HELPER_APP);
        let app = if placed.exists() {
            placed
        } else {
            super::computer_use::bundled_helper(&self.config.daemon_exe)?
        };
        Some(
            app.join("Contents/MacOS")
                .join(super::computer_use::HELPER_BIN),
        )
    }

    /// Windows and Linux: the placed program, else the one beside brigadierd.
    #[cfg(not(target_os = "macos"))]
    fn computer_binary(&self) -> Option<PathBuf> {
        let placed = self
            .computer_home()
            .join("bin")
            .join(super::computer_use::HELPER_APP);
        if placed.exists() {
            return Some(placed);
        }
        super::computer_use::bundled_helper(&self.config.daemon_exe)
    }

    /// The placed helper: its bundle on macOS, its program elsewhere.
    fn computer_placed(&self) -> PathBuf {
        let home = self.computer_home();
        if cfg!(target_os = "macos") {
            home.join(super::computer_use::HELPER_APP)
        } else {
            home.join("bin").join(super::computer_use::HELPER_APP)
        }
    }

    /// Runs `brigadier-cu host <args>` and returns its JSON line.
    async fn computer_host(&self, args: &[&str]) -> Result<serde_json::Value, String> {
        let binary = self
            .computer_binary()
            .ok_or_else(|| "the computer-use helper is missing from this install".to_owned())?;
        let home = self.computer_home();
        let placed = self.computer_placed();
        let mut command = tokio::process::Command::new(binary);
        command
            .arg("host")
            .args(args)
            .env("BRIGADIER_CU_HOME", &home)
            .env(super::computer_use::EXTERNAL_SETUP_ENV, "1")
            .env_remove("BRIGADIER_CU_SOCKET_AUTH_TOKEN")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);
        if placed.exists() {
            command.env("BRIGADIER_CU_DAEMON_APP", &placed);
        }
        let output = tokio::time::timeout(HOST_TIMEOUT, command.output())
            .await
            .map_err(|_| "the computer-use helper didn't answer in time".to_owned())?
            .map_err(|e| format!("couldn't run the computer-use helper: {e}"))?;
        let line = String::from_utf8_lossy(&output.stdout);
        let value: serde_json::Value = serde_json::from_str(line.trim())
            .map_err(|_| "the computer-use helper gave no answer".to_owned())?;
        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(error.to_owned());
        }
        Ok(value)
    }

    /// A helper process is recorded and alive: in-process, nothing is run.
    fn computer_helper_recorded(&self) -> bool {
        self.computer_helper_pid().is_some_and(alive)
    }

    /// The permissions the helper last wrote down.
    fn computer_permissions_cached(&self) -> Option<RawPermissions> {
        let bytes = std::fs::read(
            self.computer_home()
                .join(super::computer_use::PERMISSIONS_FILE),
        )
        .ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn computer_capture_confirmed(&self) -> bool {
        self.computer_home().join(CAPTURE_CONFIRMED).exists()
    }

    /// The agent behind a helper session (`brigadier-<task or conversation id>`).
    async fn computer_owner(
        &self,
        session: &str,
    ) -> (Option<ConversationId>, Option<TaskId>, String) {
        let owner = session.strip_prefix("brigadier-").unwrap_or(session);
        let task = TaskId(owner.to_owned());
        if let Some(live) = self.existing_task_live(&task) {
            let title = self
                .task_by_id(&live.conversation_id, &task)
                .await
                .map(|task| task.title)
                .unwrap_or_else(|_| "A worker".to_owned());
            return (Some(live.conversation_id.clone()), Some(task), title);
        }
        let conversation = ConversationId(owner.to_owned());
        match self.core.conversation(&conversation) {
            Ok(found) => {
                let title = if found.title.trim().is_empty() {
                    "A session".to_owned()
                } else {
                    found.title
                };
                (Some(conversation), None, title)
            }
            Err(_) => (None, None, "An agent".to_owned()),
        }
    }

    /// Computer use as Settings shows it. Starts nothing.
    pub async fn computer_use_status(&self) -> ComputerUseStatus {
        let raw = if self.computer_helper_recorded() {
            self.computer_host(&["status"]).await.ok()
        } else {
            None
        };
        self.computer_status_from(raw).await
    }

    /// The status from `host status`'s answer, if any.
    async fn computer_status_from(&self, answer: Option<serde_json::Value>) -> ComputerUseStatus {
        let notes = answer
            .clone()
            .and_then(|value| serde_json::from_value::<RawPlatformStatus>(value).ok())
            .and_then(|status| status.permissions?.platform)
            .map(|platform| platform.notes)
            .unwrap_or_default();
        let raw = answer.and_then(|value| serde_json::from_value::<Raw>(value).ok());
        let enabled = self.core.settings().computer_use;
        let live = raw
            .as_ref()
            .filter(|raw| raw.state == "healthy")
            .and_then(|raw| raw.permissions);
        let current = live.is_some();
        let permissions = live.or_else(|| self.computer_permissions_cached());
        let trouble = self.computer_host_state().trouble.clone();
        let mut sessions = Vec::new();
        if let Some(raw) = raw.filter(|raw| raw.state == "healthy") {
            for session in raw.sessions {
                let (conversation_id, task_id, label) = self.computer_owner(&session.session).await;
                for app in session.apps {
                    sessions.push(ComputerSession {
                        session: session.session.clone(),
                        conversation_id: conversation_id.clone(),
                        task_id: task_id.clone(),
                        label: label.clone(),
                        bundle_id: app.bundle_id.unwrap_or_default(),
                        app: app.name,
                        pid: app.pid.and_then(|pid| i32::try_from(pid).ok()).unwrap_or(0),
                    });
                }
            }
        }
        ComputerUseStatus {
            available: true,
            platform: ComputerPlatform::current(),
            accessibility: state_of(permissions.map(|p| p.accessibility)),
            screen_recording: state_of(permissions.map(|p| p.screen_recording)),
            current,
            setup: setup_of(
                enabled,
                trouble.as_ref(),
                permissions,
                // Windows and Linux have no capture check to pass.
                !cfg!(target_os = "macos") || self.computer_capture_confirmed(),
            ),
            sessions,
            notes,
        }
    }

    fn computer_host_state(&self) -> std::sync::MutexGuard<'_, HostState> {
        self.computer_host
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Computer use's news: status changes and setup requests.
    pub fn subscribe_computer_use(&self) -> broadcast::Receiver<ComputerNews> {
        self.computer_host.news.subscribe()
    }

    /// The status the windows were last told, for a window that subscribes after it.
    pub fn last_computer_status(&self) -> Option<ComputerUseStatus> {
        self.computer_host_state().last.clone()
    }

    /// Tells the windows about `status` when it changed.
    fn publish_computer_status(&self, status: &ComputerUseStatus) {
        let mut state = self.computer_host_state();
        if state.last.as_ref() != Some(status) {
            state.last = Some(status.clone());
            drop(state);
            let _ = self
                .computer_host
                .news
                .send(ComputerNews::Status(status.clone()));
        }
    }

    async fn refresh_computer_status(&self) -> ComputerUseStatus {
        let status = self.computer_use_status().await;
        self.publish_computer_status(&status);
        status
    }

    /// Makes the helper ready to start: placed in the data folder (again, after an update).
    /// A copy placed earlier still serves when the bundled one can't be found.
    async fn place_computer_helper(&self) -> Result<(), String> {
        match self
            .computer_use
            .ready(&self.config.daemon_exe, &self.data_dir)
            .await
        {
            Ok(_) => Ok(()),
            Err(_) if self.computer_binary().is_some_and(|binary| binary.exists()) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Grant…: macOS asks the user, on behalf of the helper (which holds the grant).
    pub async fn grant_computer_permission(
        &self,
        permission: ComputerPermission,
    ) -> Result<(), String> {
        self.place_computer_helper().await?;
        self.computer_host(&["ensure"]).await?;
        let name = match permission {
            ComputerPermission::Accessibility => "accessibility",
            ComputerPermission::ScreenRecording => "screen_recording",
        };
        self.computer_host(&["request", "request_system_permission", "--name", name])
            .await?;
        self.refresh_computer_status().await;
        Ok(())
    }

    /// Check again: reads the permissions; refreshes the helper only when Screen Recording
    /// still reads as missing (macOS shows a process a new grant only after a restart), and at
    /// most every [`REFRESH_SPACING`]. Concurrent checks share one refresh.
    pub async fn check_computer_use(&self) -> Result<ComputerUseStatus, String> {
        self.place_computer_helper().await?;
        let mut last_refresh = self.computer_host.refresh.lock().await;
        self.computer_host(&["ensure"]).await?;
        let raw: Raw = serde_json::from_value(self.computer_host(&["status"]).await?)
            .map_err(|e| e.to_string())?;
        let needs_refresh = raw
            .permissions
            .is_some_and(|p| p.accessibility && !p.screen_recording)
            && last_refresh.is_none_or(|at| at.elapsed() >= REFRESH_SPACING);
        if needs_refresh {
            *last_refresh = Some(Instant::now());
            let pid_before = self.computer_helper_pid();
            self.computer_host(&["request", "permissions_refresh"])
                .await?;
            // The helper re-executes in place; wait for it to answer again.
            let until = Instant::now() + Duration::from_secs(10);
            tokio::time::sleep(Duration::from_millis(300)).await;
            while Instant::now() < until {
                if let Ok(probe) = self.computer_host(&["probe"]).await
                    && probe["state"] == "healthy"
                    && (pid_before.is_none() || probe["pid"].as_u64().is_some())
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
        drop(last_refresh);
        Ok(self.refresh_computer_status().await)
    }

    fn computer_helper_pid(&self) -> Option<i32> {
        std::fs::read_to_string(self.computer_home().join("cu.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// The capture check: one test picture. When it works, setup is finished.
    pub async fn confirm_computer_capture(
        &self,
    ) -> Result<(ComputerUseStatus, Option<String>), String> {
        self.place_computer_helper().await?;
        self.computer_host(&["ensure"]).await?;
        let verified = self
            .computer_host(&["request", "verify_screen_capture"])
            .await?;
        let problem = if verified["capturable"].as_bool() == Some(true) {
            let marker = self.computer_home().join(CAPTURE_CONFIRMED);
            std::fs::write(&marker, b"").map_err(|e| e.to_string())?;
            let _ = self
                .computer_host(&[
                    "request",
                    "set_external_permission_ready",
                    r#"{"ready":true}"#,
                ])
                .await;
            None
        } else {
            Some(
                verified["capture_error"]
                    .as_str()
                    .unwrap_or("screen capture didn't work")
                    .to_owned(),
            )
        };
        Ok((self.refresh_computer_status().await, problem))
    }

    /// Stop using <app>: the agent's further actions there are refused.
    pub async fn stop_computer_session(
        &self,
        session: &str,
        bundle_id: Option<&str>,
        app: &str,
    ) -> Result<(), String> {
        let args = serde_json::json!({"session": session, "bundle_id": bundle_id, "app": app});
        self.computer_host(&["request", "stop_app", &args.to_string()])
            .await?;
        self.refresh_computer_status().await;
        Ok(())
    }

    /// Computer use was turned on or off: the helper follows, and running agents too. It
    /// follows the setting as it is now, one turn at a time, so quick flips end as the last one.
    pub async fn computer_use_turned(&self) {
        let _turning = self.computer_host.turning.lock().await;
        let on = self.core.settings().computer_use;
        if let Err(error) = self.computer_host(&[if on { "on" } else { "off" }]).await {
            tracing::warn!(%error, on, "couldn't tell the computer-use helper");
        }
        self.refresh_computer_status().await;
    }

    /// Starts the watchdog. It lives as long as the manager.
    pub fn start_computer_watchdog(self: &std::sync::Arc<Self>) {
        // Tests look themselves, with a fake helper ([`Self::watch_computer_helper`]).
        if cfg!(test) {
            return;
        }
        let weak = std::sync::Arc::downgrade(self);
        tokio::spawn(async move {
            // The helper's off marker follows the setting, also when it changed while Brigadier
            // was closed.
            if let Some(manager) = weak.upgrade() {
                let on = manager.core.settings().computer_use;
                if manager.computer_home().join(OFF_MARKER).exists() == on {
                    manager.computer_use_turned().await;
                }
            }
            let mut wall = SystemTime::now();
            let mut monotonic = Instant::now();
            loop {
                tokio::time::sleep(TICK).await;
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                let woke = slept(wall.elapsed().unwrap_or_default(), monotonic.elapsed());
                wall = SystemTime::now();
                monotonic = Instant::now();
                manager.watch_computer_helper(woke).await;
            }
        });
    }

    pub(super) async fn watch_computer_helper(&self, woke: bool) {
        if !self.core.settings().computer_use {
            // Off holds: a helper without its off mark (a turn that failed) gets it again.
            if !self.computer_home().join(OFF_MARKER).exists() {
                self.computer_use_turned().await;
            }
            return;
        }
        self.notice_setup_request();
        if woke {
            tracing::info!("the Mac woke; checking the computer-use helper");
        }
        if !self.computer_helper_recorded() {
            self.computer_host_state().strikes = 0;
            let status = self.computer_status_from(None).await;
            self.publish_computer_status(&status);
            return;
        }
        let answer = self.computer_host(&["status"]).await.ok();
        let raw = answer
            .clone()
            .and_then(|value| serde_json::from_value::<Raw>(value).ok())
            .unwrap_or_default();
        // A recorded helper that answers nothing (no socket, a stopped process, a wedged
        // main loop) counts as hung.
        let hung = raw.state != "healthy";
        let restart = {
            let mut state = self.computer_host_state();
            if hung {
                state.strikes += 1;
            } else {
                state.strikes = 0;
                if raw.state == "healthy" && state.trouble.is_some() {
                    state.trouble = None;
                }
            }
            hung && (state.strikes >= HUNG_STRIKES || woke)
        };
        if restart {
            self.restart_computer_helper().await;
        } else {
            let status = self.computer_status_from(answer).await;
            self.publish_computer_status(&status);
        }
    }

    /// An agent started waiting for setup since the last look: the windows hear it, and the
    /// app opens the setup window.
    fn notice_setup_request(&self) {
        let Some(asked) = std::fs::metadata(self.computer_home().join(SETUP_NEEDED))
            .and_then(|meta| meta.modified())
            .ok()
        else {
            return;
        };
        let mut state = self.computer_host_state();
        if asked > state.setup_asked {
            state.setup_asked = asked;
            drop(state);
            let _ = self.computer_host.news.send(ComputerNews::SetupNeeded);
        }
    }

    /// Replaces a hung helper. Agents' next calls use the fresh one.
    async fn restart_computer_helper(&self) {
        tracing::warn!("the computer-use helper stopped answering; restarting it");
        self.computer_host_state().trouble = Some(Trouble::Restarting);
        let status = self.computer_status_from(None).await;
        self.publish_computer_status(&status);
        let outcome = self.computer_host(&["restart"]).await;
        {
            let mut state = self.computer_host_state();
            state.strikes = 0;
            state.trouble = match outcome {
                Ok(_) => None,
                Err(message) => {
                    tracing::warn!(%message, "the computer-use helper couldn't be restarted");
                    Some(Trouble::Failed(message))
                }
            };
        }
        self.refresh_computer_status().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn granted(accessibility: bool, screen_recording: bool) -> Option<RawPermissions> {
        Some(RawPermissions {
            accessibility,
            screen_recording,
        })
    }

    #[test]
    fn setup_follows_the_setting_the_permissions_and_the_capture_check() {
        assert_eq!(
            setup_of(false, None, granted(true, true), true),
            ComputerSetup::Off
        );
        assert_eq!(setup_of(true, None, None, false), ComputerSetup::NotSetUp);
        assert_eq!(
            setup_of(true, None, granted(true, false), false),
            ComputerSetup::Missing
        );
        assert_eq!(
            setup_of(true, None, granted(false, true), true),
            ComputerSetup::Missing
        );
        assert_eq!(
            setup_of(true, None, granted(true, true), false),
            ComputerSetup::ConfirmCapture
        );
        assert_eq!(
            setup_of(true, None, granted(true, true), true),
            ComputerSetup::Ready
        );
    }

    #[test]
    fn trouble_with_the_helper_shows_over_the_permissions() {
        assert_eq!(
            setup_of(true, Some(&Trouble::Restarting), granted(true, true), true),
            ComputerSetup::Restarting
        );
        assert_eq!(
            setup_of(
                true,
                Some(&Trouble::Failed("no".into())),
                granted(true, true),
                true
            ),
            ComputerSetup::Failed {
                message: "no".into()
            }
        );
        // Off wins: nothing is tended while it is off.
        assert_eq!(
            setup_of(false, Some(&Trouble::Restarting), None, false),
            ComputerSetup::Off
        );
    }

    #[test]
    fn a_wall_clock_jump_past_the_monotonic_clock_is_a_wake() {
        assert!(!slept(Duration::from_secs(2), Duration::from_secs(2)));
        assert!(!slept(Duration::from_secs(20), Duration::from_secs(2)));
        assert!(slept(Duration::from_secs(600), Duration::from_secs(2)));
        // A clock set back is no wake.
        assert!(!slept(Duration::ZERO, Duration::from_secs(2)));
    }

    #[test]
    fn permission_states_read_as_granted_not_granted_or_unknown() {
        assert_eq!(state_of(Some(true)), PermissionState::Granted);
        assert_eq!(state_of(Some(false)), PermissionState::NotGranted);
        assert_eq!(state_of(None), PermissionState::Unknown);
    }
}
