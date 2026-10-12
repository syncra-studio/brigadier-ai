//! What keeps the helper the right one, and off when Brigadier says so.
//!
//! - **Build id.** The desktop build writes the helper binary's digest to
//!   `Contents/Resources/build-id` in the bundle. A daemon remembers the id
//!   it started with; a shim compares it with the id of the bundle it would
//!   launch (not with its own, so an older shim still running after an update
//!   never fights a newer one) and replaces a daemon from another build.
//! - **Off marker.** `<home>/off` exists while computer use is turned off in
//!   Brigadier. Shims refuse calls and never start the daemon while it exists,
//!   and the daemon refuses driving calls, so turning it off holds even for
//!   agents that were already running.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The bundle-relative file holding a staged helper's build id.
const BUILD_ID_FILE: &str = "Contents/Resources/build-id";
/// The marker Brigadier writes while computer use is turned off.
const OFF_MARKER: &str = "off";

/// What an agent is told while computer use is off.
pub const OFF_MESSAGE: &str = "Computer use is turned off in Brigadier's settings, so these \
     tools can't see or control apps right now. Don't try again unless the user turns it back on.";

/// The build id of the staged bundle at `app`, if it has one.
pub fn bundle_build_id(app: &Path) -> Option<String> {
    let id = std::fs::read_to_string(app.join(BUILD_ID_FILE)).ok()?;
    let id = id.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// The bundle around this executable (`<app>/Contents/MacOS/<bin>`).
fn own_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let app = exe.parent()?.parent()?.parent()?;
    (app.extension()? == "app").then(|| app.to_path_buf())
}

/// The build id this process started with: read once, so replacing the
/// bundle on disk later doesn't change what a running daemon reports.
pub fn own_build_id() -> Option<&'static str> {
    static ID: OnceLock<Option<String>> = OnceLock::new();
    ID.get_or_init(|| own_bundle().and_then(|app| bundle_build_id(&app)))
        .as_deref()
}

/// The bundle a shim launches the daemon from.
pub fn launch_bundle() -> Option<PathBuf> {
    std::env::var_os("BRIGADIER_CU_DAEMON_APP")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Whether a daemon reporting `running` must be replaced by the bundle
/// whose id is `on_disk`. Unstaged builds (no id on either side) are never
/// replaced: there is nothing to compare.
pub fn needs_replacement(running: Option<&str>, on_disk: Option<&str>) -> bool {
    match (running, on_disk) {
        (Some(running), Some(on_disk)) => running != on_disk,
        (None, Some(_)) => true,
        _ => false,
    }
}

/// Whether computer use is turned off for the helper home `home`.
pub fn is_off(home: &Path) -> bool {
    home.join(OFF_MARKER).exists()
}

/// Turns computer use off (`true`) or back on for `home`.
pub fn set_off(home: &Path, off: bool) -> std::io::Result<()> {
    let marker = home.join(OFF_MARKER);
    if off {
        std::fs::create_dir_all(home)?;
        std::fs::write(marker, b"")
    } else {
        match std::fs::remove_file(marker) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }
}

/// A main loop silent this long counts as hung. Long enough for the slowest
/// legitimate main-thread work (a large accessibility walk), short enough
/// that a wedged daemon is replaced while the agent still waits.
pub const MAIN_LOOP_STALL: Duration = Duration::from_secs(30);

/// When the main loop last ran a heartbeat, in ms since the epoch; 0 until
/// the heartbeat starts.
static LAST_BEAT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// How long the main loop has been silent: zero before the heartbeat starts.
pub fn main_loop_silence() -> Duration {
    stall_since(LAST_BEAT_MS.load(Ordering::Acquire), now_ms())
}

fn stall_since(last_beat_ms: u64, now_ms: u64) -> Duration {
    if last_beat_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(now_ms.saturating_sub(last_beat_ms))
}

/// Has the main queue stamp [`LAST_BEAT_MS`] every second, so `health` can
/// tell a daemon whose AppKit thread is wedged from one that only answers on
/// its socket thread. Call right before the main thread enters its run loop.
#[cfg(target_os = "macos")]
pub fn start_main_loop_heartbeat() {
    use std::ffi::c_void;

    #[link(name = "System", kind = "dylib")]
    extern "C" {
        static _dispatch_main_q: u8;
        fn dispatch_async_f(
            queue: *const c_void,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }
    unsafe extern "C" fn beat(_: *mut c_void) {
        LAST_BEAT_MS.store(now_ms(), Ordering::Release);
    }
    LAST_BEAT_MS.store(now_ms(), Ordering::Release);
    let _ = std::thread::Builder::new()
        .name("cu-heartbeat".into())
        .spawn(|| loop {
            std::thread::sleep(Duration::from_secs(1));
            // SAFETY: queues a plain function with no context on the main
            // dispatch queue; it only writes an atomic.
            unsafe {
                dispatch_async_f(
                    &raw const _dispatch_main_q as *const c_void,
                    std::ptr::null_mut(),
                    beat,
                );
            }
        });
}

/// The file holding the permissions the daemon last saw, so Brigadier can
/// show them without starting a daemon.
pub const PERMISSIONS_FILE: &str = "permissions.json";

/// Records what the daemon just read about its permissions, when it changed.
pub fn record_permissions(accessibility: bool, screen_recording: bool) {
    static LAST: std::sync::Mutex<Option<(bool, bool)>> = std::sync::Mutex::new(None);
    let Ok(mut last) = LAST.lock() else { return };
    if *last == Some((accessibility, screen_recording)) {
        return;
    }
    let record = serde_json::json!({
        "accessibility": accessibility,
        "screen_recording": screen_recording,
        "pid": std::process::id(),
        "build_id": own_build_id(),
        "checked_at_ms": now_ms(),
    });
    let path = home().join(PERMISSIONS_FILE);
    let partial = path.with_extension(format!("json.{}", std::process::id()));
    if std::fs::write(&partial, record.to_string()).is_ok()
        && std::fs::rename(&partial, &path).is_ok()
    {
        *last = Some((accessibility, screen_recording));
    }
}

/// This helper's home directory.
pub fn home() -> PathBuf {
    pip_preview::home::home()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_daemon_from_another_build_is_replaced_and_unstaged_builds_are_left_alone() {
        assert!(!needs_replacement(Some("a1"), Some("a1")));
        assert!(needs_replacement(Some("a1"), Some("b2")));
        // A daemon from before build ids, with a staged bundle on disk.
        assert!(needs_replacement(None, Some("b2")));
        // Development: nothing staged to compare against.
        assert!(!needs_replacement(Some("a1"), None));
        assert!(!needs_replacement(None, None));
    }

    #[test]
    fn a_bundle_reports_the_build_id_written_into_it() {
        let dir = std::env::temp_dir().join(format!("cu-lifecycle-build-{}", std::process::id()));
        let app = dir.join("Helper.app");
        std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
        assert_eq!(bundle_build_id(&app), None);
        std::fs::write(app.join(BUILD_ID_FILE), "0.1.0-abc123\n").unwrap();
        assert_eq!(bundle_build_id(&app).as_deref(), Some("0.1.0-abc123"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_main_loop_counts_as_silent_only_after_its_heartbeat_starts() {
        assert_eq!(stall_since(0, 50_000), Duration::ZERO);
        assert_eq!(stall_since(10_000, 12_500), Duration::from_millis(2_500));
        // A clock step backwards is no stall.
        assert_eq!(stall_since(10_000, 9_000), Duration::ZERO);
    }

    #[test]
    fn the_off_marker_turns_computer_use_off_and_back_on() {
        let home = std::env::temp_dir().join(format!("cu-lifecycle-off-{}", std::process::id()));
        assert!(!is_off(&home));
        set_off(&home, true).unwrap();
        assert!(is_off(&home));
        set_off(&home, false).unwrap();
        assert!(!is_off(&home));
        // Turning on twice is fine.
        set_off(&home, false).unwrap();
        std::fs::remove_dir_all(&home).unwrap();
    }
}
