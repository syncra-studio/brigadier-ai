//! Nightly updates. Only a build that carries an updater configuration looks for them: the
//! signed nightly, whose workflow (.github/workflows/nightly.yml) adds it. Debug builds, local
//! release builds, smoke launches and the overnight notification host never do.
//!
//! A minute after launch and then every four hours it reads the nightly feed, downloads a newer
//! build in the background and keeps it. The window then shows "Restart to update". The update
//! is installed when the app quits (by that line or any other way), once the daemon has stopped.
//! When an overnight run keeps the daemon going it isn't installed: the next launch downloads it
//! again and installs it at a later quit.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// The first check after launch, then how often.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// Sent to the window with the waiting update's version.
pub const READY_EVENT: &str = "app-update-ready";

/// A downloaded, verified update waiting for the app to quit.
struct Ready {
    update: Update,
    bytes: Vec<u8>,
}

static READY: Mutex<Option<Ready>> = Mutex::new(None);
/// "Restart to update" was chosen: the app opens again after quitting.
static RESTART: AtomicBool = AtomicBool::new(false);

/// Whether this launch looks for updates.
pub fn enabled(debug: bool, configured: bool, smoke: bool, host: bool) -> bool {
    !debug && configured && !smoke && !host
}

/// Whether the build carries the updater's configuration (its feed and public key).
pub fn configured(config: &tauri::Config) -> bool {
    config.plugins.0.contains_key("updater")
}

/// The version this build carries, as About shows it: a nightly's full version
/// ("0.1.0-nightly.202610161908", its workflow sets it), else the crate's.
pub fn version(config: &tauri::Config) -> String {
    config
        .version
        .clone()
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// Whether a version the feed offers is worth downloading, given the one already waiting.
fn wanted(waiting: Option<&str>, offered: &str) -> bool {
    waiting != Some(offered)
}

/// Checks now and then, for as long as the app runs.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            if let Err(err) = check(&app).await {
                // Offline or a feed that can't be read: nothing to offer, try again later.
                tracing::info!(error = %err, "no nightly update this time");
            }
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

async fn check(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    let Some(update) = app.updater()?.check().await? else {
        return Ok(());
    };
    if !wanted(waiting().as_deref(), &update.version) {
        return Ok(());
    }
    tracing::info!(version = %update.version, "downloading a nightly update");
    let bytes = update.download(|_, _| {}, || {}).await?;
    let version = update.version.clone();
    *lock() = Some(Ready { update, bytes });
    tracing::info!(%version, "a nightly update is ready");
    let _ = app.emit(READY_EVENT, &version);
    Ok(())
}

fn lock() -> std::sync::MutexGuard<'static, Option<Ready>> {
    READY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn waiting() -> Option<String> {
    lock().as_ref().map(|ready| ready.update.version.clone())
}

/// The version of the update waiting for a restart, if any (for a window that opens later).
#[tauri::command]
pub fn app_update_ready() -> Option<String> {
    waiting()
}

/// Quits the orderly way, installs the waiting update and opens the app again.
#[tauri::command]
pub fn restart_to_update(app: AppHandle) {
    RESTART.store(true, Ordering::Release);
    crate::shell::quit(&app, 0);
}

/// After the app's last event: installs the waiting update when the daemon has stopped, and
/// says whether to open the app again.
pub fn finish(daemon_stopped: bool) -> bool {
    if let Some(ready) = lock().take() {
        if daemon_stopped {
            match ready.update.install(&ready.bytes) {
                Ok(()) => {
                    tracing::info!(version = %ready.update.version, "installed the nightly update")
                }
                Err(err) => tracing::warn!(error = %err, "could not install the nightly update"),
            }
        } else {
            tracing::info!("the daemon keeps running; the next launch fetches the update again");
        }
    }
    RESTART.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_configured_release_build_looks_for_updates() {
        assert!(enabled(false, true, false, false));
        assert!(
            !enabled(true, true, false, false),
            "a debug build never updates"
        );
        assert!(
            !enabled(false, false, false, false),
            "a build without the feed never updates"
        );
        assert!(
            !enabled(false, true, true, false),
            "a smoke launch never updates"
        );
        assert!(
            !enabled(false, true, false, true),
            "the notification host never updates"
        );
    }

    #[test]
    fn the_configuration_is_what_the_nightly_adds() {
        let plain: tauri::Config =
            serde_json::from_value(serde_json::json!({ "identifier": "ai.brigadier.test" }))
                .unwrap();
        assert!(!configured(&plain));
        let nightly: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "ai.brigadier.test",
            "plugins": { "updater": {
                "pubkey": "key",
                "endpoints": ["https://github.com/syncra-studio/brigadier-ai/releases/download/nightly/latest.json"]
            } }
        }))
        .unwrap();
        assert!(configured(&nightly));
    }

    /// About names the nightly that is installed, not only its release (#159).
    #[test]
    fn the_version_is_the_nightlys_full_one() {
        let nightly: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "ai.brigadier.test",
            "version": "0.1.0-nightly.202610161908",
        }))
        .unwrap();
        assert_eq!(version(&nightly), "0.1.0-nightly.202610161908");
        let plain: tauri::Config =
            serde_json::from_value(serde_json::json!({ "identifier": "ai.brigadier.test" }))
                .unwrap();
        assert_eq!(version(&plain), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn local_and_dev_builds_carry_no_feed() {
        for config in [
            include_str!("../tauri.conf.json"),
            include_str!("../tauri.dev.conf.json"),
            include_str!("../tauri.macos.conf.json"),
        ] {
            let config: serde_json::Value = serde_json::from_str(config).unwrap();
            assert!(config.pointer("/plugins/updater").is_none());
        }
    }

    #[test]
    fn a_waiting_version_is_not_downloaded_again() {
        assert!(wanted(None, "0.1.0-nightly.202610170130"));
        assert!(!wanted(
            Some("0.1.0-nightly.202610170130"),
            "0.1.0-nightly.202610170130"
        ));
        assert!(wanted(
            Some("0.1.0-nightly.202610170130"),
            "0.1.0-nightly.202610180130"
        ));
    }
}
