//! Nightly updates. Only a build that carries an updater configuration looks for them: the
//! signed nightly, whose workflow (.github/workflows/nightly.yml) adds it. Debug builds, local
//! release builds and smoke launches never do.
//!
//! A minute after launch and then every four hours it reads the nightly feed. A newer build is
//! only offered: the window shows an update pill and nothing is downloaded. A click on it
//! downloads and checks the build, and the pill turns into "Restart to update". The update is
//! installed when the app quits after that (by that pill or any other way), once the daemon has
//! stopped. Without the click, quitting installs nothing. When the daemon doesn't stop it isn't
//! installed: the next launch offers it again.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// The first check after launch, then how often.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(4 * 60 * 60);

/// Sent to the window with the update as it now shows ([`Shown`]).
pub const EVENT: &str = "app-update";

/// Whether this launch looks for updates.
pub fn enabled(debug: bool, configured: bool, smoke: bool) -> bool {
    !debug && configured && !smoke
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

/// This build's own commit and date, where the build env carried them: the commit from
/// `GITHUB_SHA` (every Actions job has it), the date from the nightly workflow's own stamp on
/// `version` ("…-nightly.202610161908" -> "2026-10-16"). `None` for a build neither set (a
/// local build, a release tag with no `GITHUB_SHA` at hand).
pub fn build(config: &tauri::Config) -> Option<brigadier_ipc::app::BuildInfo> {
    let sha = option_env!("GITHUB_SHA").map(|sha| sha[..sha.len().min(9)].to_owned());
    let date = version(config)
        .rsplit_once("-nightly.")
        .and_then(|(_, stamp)| {
            (stamp.len() >= 8 && stamp.as_bytes().iter().all(u8::is_ascii_digit))
                .then(|| format!("{}-{}-{}", &stamp[0..4], &stamp[4..6], &stamp[6..8]))
        });
    (sha.is_some() || date.is_some()).then_some(brigadier_ipc::app::BuildInfo { sha, date })
}

/// The feed this build checks, in words ("Nightly", "Release"), where it checks at all.
pub fn channel(config: &tauri::Config) -> Option<&'static str> {
    configured(config).then(|| {
        if version(config).contains("-nightly.") {
            "Nightly"
        } else {
            "Release"
        }
    })
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
    let version = update.version.clone();
    let handle = app.clone();
    let download: Download = Arc::new(move || {
        let (app, offered) = (handle.clone(), update.clone());
        Box::pin(async move {
            // The feed read again: a nightly published since the offer deletes the offered
            // build's archive, so its address would no longer answer.
            let update = match app.updater() {
                Ok(updater) => updater.check().await.ok().flatten().unwrap_or(offered),
                Err(_) => offered,
            };
            tracing::info!(version = %update.version, "downloading a nightly update");
            let bytes = update
                .download(|_, _| {}, || {})
                .await
                .map_err(|err| err.to_string())?;
            Ok(Box::new(move || update.install(&bytes).map_err(|err| err.to_string())) as Install)
        })
    });
    if let Some(shown) = UPDATES.offer(version.clone(), download) {
        tracing::info!(%version, "a nightly update is available");
        emit(app, &shown);
    }
    Ok(())
}

fn emit(app: &AppHandle, shown: &Shown) {
    let _ = app.emit(EVENT, shown);
}

/// The update as the window shows it, at the sidebar's foot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shown {
    pub state: ShownState,
    pub version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ShownState {
    /// Found, not downloaded: the user hasn't asked for it.
    Available,
    /// Asked for: downloading and checking it.
    Updating,
    /// Asked for, and its download failed; asking again tries again.
    Failed,
    /// Downloaded and checked: it goes in when the app quits.
    Ready,
}

/// Puts a downloaded update in place.
type Install = Box<dyn FnOnce() -> Result<(), String> + Send>;
/// Downloads an update and checks its signature; again on each call.
type Download =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Install, String>> + Send>> + Send + Sync>;

enum Stage {
    Nothing,
    /// Found; `failed` once a download the user asked for failed.
    Offered {
        version: String,
        download: Download,
        failed: bool,
    },
    Downloading {
        version: String,
        download: Download,
    },
    /// Downloaded, waiting for the app to quit.
    Ready {
        version: String,
        install: Install,
    },
}

/// The nightly update: found by a check, downloaded only once the user asks for it, installed
/// when the app quits after that.
pub struct Updates {
    stage: Mutex<Stage>,
    /// "Restart to update" was chosen: the app opens again after quitting.
    restart: AtomicBool,
}

static UPDATES: Updates = Updates::new();

impl Updates {
    const fn new() -> Self {
        Self {
            stage: Mutex::new(Stage::Nothing),
            restart: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Stage> {
        self.stage
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// What the window shows, if anything.
    pub fn shown(&self) -> Option<Shown> {
        let stage = self.lock();
        let (state, version) = match &*stage {
            Stage::Nothing => return None,
            Stage::Offered {
                version, failed, ..
            } => (
                if *failed {
                    ShownState::Failed
                } else {
                    ShownState::Available
                },
                version,
            ),
            Stage::Downloading { version, .. } => (ShownState::Updating, version),
            Stage::Ready { version, .. } => (ShownState::Ready, version),
        };
        Some(Shown {
            state,
            version: version.clone(),
        })
    }

    /// Offers what a check found, downloading nothing. A version already offered, or any once the
    /// user has asked for one, changes nothing (None); else what the window now shows.
    pub fn offer(&self, version: String, download: Download) -> Option<Shown> {
        {
            let mut stage = self.lock();
            match &*stage {
                Stage::Offered {
                    version: offered, ..
                } if *offered == version => return None,
                Stage::Downloading { .. } | Stage::Ready { .. } => return None,
                Stage::Nothing | Stage::Offered { .. } => {}
            }
            *stage = Stage::Offered {
                version,
                download,
                failed: false,
            };
        }
        self.shown()
    }

    /// The user asked for the offered update: downloads it, showing each step with `show`.
    /// Nothing offered (or already under way) does nothing.
    pub async fn take(&self, show: impl Fn(&Shown)) {
        let download = {
            let mut stage = self.lock();
            let Stage::Offered {
                version, download, ..
            } = &*stage
            else {
                return;
            };
            let download = download.clone();
            *stage = Stage::Downloading {
                version: version.clone(),
                download: download.clone(),
            };
            download
        };
        if let Some(shown) = self.shown() {
            show(&shown);
        }
        let result = download().await;
        {
            let mut stage = self.lock();
            let Stage::Downloading { version, download } = &*stage else {
                return;
            };
            let (version, download) = (version.clone(), download.clone());
            *stage = match result {
                Ok(install) => {
                    tracing::info!(%version, "a nightly update is ready");
                    Stage::Ready { version, install }
                }
                Err(error) => {
                    tracing::warn!(%version, %error, "could not download the nightly update");
                    Stage::Offered {
                        version,
                        download,
                        failed: true,
                    }
                }
            };
        }
        if let Some(shown) = self.shown() {
            show(&shown);
        }
    }

    /// At the app's last event: installs the update the user asked for when the daemon has
    /// stopped (once; a later call finds none), and says whether to open the app again. One
    /// that was only found, or is still downloading, isn't installed.
    pub fn finish(&self, daemon_stopped: bool) -> bool {
        let ready = {
            let mut stage = self.lock();
            match std::mem::replace(&mut *stage, Stage::Nothing) {
                Stage::Ready { version, install } => Some((version, install)),
                other => {
                    *stage = other;
                    None
                }
            }
        };
        if let Some((version, install)) = ready {
            if daemon_stopped {
                match install() {
                    Ok(()) => tracing::info!(%version, "installed the nightly update"),
                    Err(error) => {
                        tracing::warn!(%error, "could not install the nightly update");
                    }
                }
            } else {
                tracing::info!("the daemon keeps running; the update is offered again next launch");
            }
        }
        self.restart.load(Ordering::Acquire)
    }
}

/// Keeps a stand-in update the user already asked for, whose install only counts (for tests
/// of the quit paths).
#[cfg(test)]
pub fn wait_counting(version: &str, installs: Arc<std::sync::atomic::AtomicUsize>) {
    *UPDATES.lock() = Stage::Ready {
        version: version.to_owned(),
        install: Box::new(move || {
            installs.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }),
    };
}

/// The update as the window shows it, if any (for a window that opens later).
#[tauri::command]
pub fn app_update() -> Option<Shown> {
    UPDATES.shown()
}

/// Settings › About's "Check now": checks right away instead of waiting for the next scheduled
/// check, and answers with what the window now shows (an offer, or none for "up to date").
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<Option<Shown>, String> {
    check(&app).await.map_err(|err| err.to_string())?;
    Ok(UPDATES.shown())
}

/// The user clicked the update: downloads it in the background, the window following along.
#[tauri::command]
pub fn take_app_update(app: AppHandle) {
    tauri::async_runtime::spawn(async move { UPDATES.take(|shown| emit(&app, shown)).await });
}

/// Quits the orderly way, installs the waiting update and opens the app again.
#[tauri::command]
pub fn restart_to_update(app: AppHandle) {
    UPDATES.restart.store(true, Ordering::Release);
    crate::shell::quit(&app, 0);
}

/// At the app's last event: see [`Updates::finish`].
pub fn finish(daemon_stopped: bool) -> bool {
    UPDATES.finish(daemon_stopped)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[test]
    fn only_a_configured_release_build_looks_for_updates() {
        assert!(enabled(false, true, false));
        assert!(!enabled(true, true, false), "a debug build never updates");
        assert!(
            !enabled(false, false, false),
            "a build without the feed never updates"
        );
        assert!(!enabled(false, true, true), "a smoke launch never updates");
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
    fn the_build_is_the_nightlys_date_stamp_and_the_sha_the_build_env_carried() {
        let nightly: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "ai.brigadier.test",
            "version": "0.1.0-nightly.202610161908",
        }))
        .unwrap();
        // A local `cargo test` carries no `GITHUB_SHA`; a CI one does.
        let sha = option_env!("GITHUB_SHA").map(|sha| sha[..sha.len().min(9)].to_owned());
        let info = build(&nightly).expect("a nightly stamps a date");
        assert_eq!(info.sha, sha);
        assert_eq!(info.date.as_deref(), Some("2026-10-16"));
        let plain: tauri::Config =
            serde_json::from_value(serde_json::json!({ "identifier": "ai.brigadier.test" }))
                .unwrap();
        assert_eq!(
            build(&plain),
            sha.map(|sha| brigadier_ipc::app::BuildInfo {
                sha: Some(sha),
                date: None
            })
        );
    }

    #[test]
    fn the_channel_is_nightly_or_release_only_where_the_updater_is_configured() {
        let nightly: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "ai.brigadier.test",
            "version": "0.1.0-nightly.202610161908",
            "plugins": { "updater": {
                "pubkey": "key",
                "endpoints": ["https://github.com/syncra-studio/brigadier-ai/releases/download/nightly/latest.json"]
            } }
        }))
        .unwrap();
        assert_eq!(channel(&nightly), Some("Nightly"));
        let release: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "ai.brigadier.test",
            "version": "0.1.0",
            "plugins": { "updater": {
                "pubkey": "key",
                "endpoints": ["https://github.com/syncra-studio/brigadier-ai/releases/latest/download/latest.json"]
            } }
        }))
        .unwrap();
        assert_eq!(channel(&release), Some("Release"));
        let plain: tauri::Config =
            serde_json::from_value(serde_json::json!({ "identifier": "ai.brigadier.test" }))
                .unwrap();
        assert_eq!(channel(&plain), None);
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

    /// A stand-in download that counts its calls and installs that count theirs.
    fn counting(downloads: &Arc<AtomicUsize>, installs: &Arc<AtomicUsize>, fail: bool) -> Download {
        let (downloads, installs) = (downloads.clone(), installs.clone());
        Arc::new(move || {
            downloads.fetch_add(1, Ordering::AcqRel);
            let installs = installs.clone();
            Box::pin(async move {
                if fail {
                    return Err("offline".to_owned());
                }
                Ok(Box::new(move || {
                    installs.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                }) as Install)
            })
        })
    }

    fn shown(state: ShownState, version: &str) -> Option<Shown> {
        Some(Shown {
            state,
            version: version.to_owned(),
        })
    }

    const V1: &str = "0.1.0-nightly.202610190130";
    const V2: &str = "0.1.0-nightly.202610200130";

    /// A found update is only offered: nothing downloads, and quitting installs nothing (#387).
    #[test]
    fn a_found_update_waits_for_the_users_click() {
        let updates = Updates::new();
        let (downloads, installs) = (Arc::default(), Arc::default());
        assert_eq!(
            updates.offer(V1.into(), counting(&downloads, &installs, false)),
            shown(ShownState::Available, V1)
        );
        assert_eq!(
            updates.offer(V1.into(), counting(&downloads, &installs, false)),
            None,
            "the same version again changes nothing"
        );
        assert!(!updates.finish(true));
        assert_eq!(downloads.load(Ordering::Acquire), 0, "nothing downloaded");
        assert_eq!(
            installs.load(Ordering::Acquire),
            0,
            "nothing installed on quit"
        );
        assert_eq!(
            updates.shown(),
            shown(ShownState::Available, V1),
            "still offered"
        );
    }

    /// After the click it downloads, shows "updating" then ready, and goes in at the next quit,
    /// once (#387).
    #[test]
    fn a_clicked_update_downloads_and_installs_on_quit() {
        let updates = Updates::new();
        let (downloads, installs) = (Arc::default(), Arc::default());
        updates.offer(V1.into(), counting(&downloads, &installs, false));
        let seen = Mutex::new(Vec::new());
        tauri::async_runtime::block_on(
            updates.take(|shown| seen.lock().unwrap().push(shown.clone())),
        );
        assert_eq!(
            seen.into_inner().unwrap(),
            [
                shown(ShownState::Updating, V1).unwrap(),
                shown(ShownState::Ready, V1).unwrap()
            ]
        );
        assert_eq!(downloads.load(Ordering::Acquire), 1);
        assert_eq!(installs.load(Ordering::Acquire), 0, "not before the quit");
        assert_eq!(
            updates.offer(V2.into(), counting(&downloads, &installs, false)),
            None,
            "a newer build waits for the next launch"
        );
        assert!(!updates.finish(true));
        assert_eq!(installs.load(Ordering::Acquire), 1, "installed on quit");
        updates.finish(true);
        assert_eq!(installs.load(Ordering::Acquire), 1, "once");
    }

    /// A download that fails says so, and a click tries again.
    #[test]
    fn a_failed_download_is_tried_again_on_the_next_click() {
        let updates = Updates::new();
        let (downloads, installs) = (Arc::default(), Arc::default());
        updates.offer(V1.into(), counting(&downloads, &installs, true));
        tauri::async_runtime::block_on(updates.take(|_| {}));
        assert_eq!(updates.shown(), shown(ShownState::Failed, V1));
        updates.finish(true);
        assert_eq!(installs.load(Ordering::Acquire), 0);
        tauri::async_runtime::block_on(updates.take(|_| {}));
        assert_eq!(downloads.load(Ordering::Acquire), 2, "tried again");
    }

    /// A daemon that keeps going leaves the update uninstalled.
    #[test]
    fn a_running_daemon_keeps_the_update_out() {
        let updates = Updates::new();
        let (downloads, installs) = (Arc::default(), Arc::default());
        updates.offer(V1.into(), counting(&downloads, &installs, false));
        tauri::async_runtime::block_on(updates.take(|_| {}));
        updates.finish(false);
        assert_eq!(installs.load(Ordering::Acquire), 0);
    }

    /// A newer build found before the click replaces the offer.
    #[test]
    fn a_newer_build_replaces_an_offer_not_yet_taken() {
        let updates = Updates::new();
        let (downloads, installs) = (Arc::default(), Arc::default());
        updates.offer(V1.into(), counting(&downloads, &installs, false));
        assert_eq!(
            updates.offer(V2.into(), counting(&downloads, &installs, false)),
            shown(ShownState::Available, V2)
        );
    }
}
