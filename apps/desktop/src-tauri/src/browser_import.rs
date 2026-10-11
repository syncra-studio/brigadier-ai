//! Settings › Browser › Import: copies another browser's cookies and history into Brigadier's
//! browser once (macOS). `brigadier-browser-data` reads the other browser's files, off the main
//! thread and never writing them; cookies go into the browser's WebKit data store
//! (`browser_ui::set_cookies`), history into its own store as the user's ("Other").

use brigadier_browser_data::{Error, Family, KeychainError, Profile};
use brigadier_ipc::app::{
    CookieImport, HistoryImport, ImportFamily, ImportProfile, ImportProgress, ImportResult,
};
use brigadier_ipc::protocol::{ErrorCode, IpcError};
use tauri::ipc::Channel;

/// The most visits one import brings over, newest first.
const HISTORY_LIMIT: usize = 50_000;

fn refused(message: impl Into<String>) -> IpcError {
    IpcError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(std::path::PathBuf::from)
}

fn family(family: Family) -> ImportFamily {
    match family {
        Family::Chromium => ImportFamily::Chromium,
        Family::Gecko => ImportFamily::Gecko,
        Family::Safari => ImportFamily::Safari,
    }
}

/// Why a profile's `what` ("cookies", "history") couldn't be read, in the user's words.
fn reason(error: &Error, browser: &str, what: &str) -> String {
    match error {
        Error::Keychain(KeychainError::Denied) => format!(
            "macOS didn't let Brigadier use {browser}'s cookie key, so no cookies were copied."
        ),
        Error::Keychain(KeychainError::NotFound) => {
            format!("{browser}'s cookie key isn't in your Keychain, so its cookies can't be read.")
        }
        Error::PermissionDenied(_) => format!(
            "Brigadier needs Full Disk Access to read {browser}'s {what}: System Settings › \
             Privacy & Security › Full Disk Access."
        ),
        Error::Locked(_) => format!("{browser} is still using its {what}. Quit it and try again."),
        Error::NotFound(_) => format!("{browser} has no {what} in this profile."),
        _ => format!("{browser}'s {what} couldn't be read ({error})."),
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// The installed browsers' profiles, each saying whether its browser is open now.
#[tauri::command]
pub async fn browser_import_profiles() -> Vec<ImportProfile> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let Some(home) = home() else {
        return Vec::new();
    };
    tauri::async_runtime::spawn_blocking(move || {
        brigadier_browser_data::discover(&home)
            .into_iter()
            .map(|profile| ImportProfile {
                running: brigadier_browser_data::running(&profile),
                id: profile.id,
                browser: profile.browser,
                name: profile.name,
                family: family(profile.family),
                has_cookies: profile.has_cookies,
                has_history: profile.has_history,
                readable: profile.readable,
                is_default: profile.is_default,
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Copies the profile `id`'s cookies and/or history into Brigadier's browser, telling
/// `progress` how it goes. Refused while that browser is open (its files may be half written).
#[tauri::command]
pub async fn browser_import(
    app: tauri::AppHandle,
    id: String,
    cookies: bool,
    history: bool,
    progress: Channel<ImportProgress>,
) -> Result<ImportResult, IpcError> {
    crate::browser::check_enabled()?;
    let home = home().ok_or_else(|| refused("no home folder"))?;
    let _ = progress.send(ImportProgress::Reading);
    let read = tauri::async_runtime::spawn_blocking(move || read(&home, &id, cookies, history))
        .await
        .map_err(|err| refused(err.to_string()))??;
    let mut result = ImportResult::default();
    if let Some(read_cookies) = read.cookies {
        result.cookies = Some(match read_cookies {
            Ok(found) => add_cookies(&app, found, &progress).await?,
            Err(error) => CookieImport {
                error: Some(error),
                ..CookieImport::default()
            },
        });
    }
    if let Some(read_history) = read.history {
        let _ = progress.send(ImportProgress::History);
        result.history = Some(match read_history {
            Ok(visits) => {
                let added = crate::browser_store::with(|store| {
                    store.import_visits(
                        visits
                            .iter()
                            .map(|v| (v.url.as_str(), v.title.as_str(), v.visited_at_ms)),
                    )
                });
                match added {
                    Some(added) => HistoryImport {
                        imported: count(added),
                        error: None,
                    },
                    None => HistoryImport {
                        imported: 0,
                        error: Some("Brigadier's browsing history can't be changed now.".into()),
                    },
                }
            }
            Err(error) => HistoryImport {
                imported: 0,
                error: Some(error),
            },
        });
    }
    Ok(result)
}

/// What was read from a profile, each part with why it couldn't be when it couldn't.
struct Read {
    cookies: Option<Result<brigadier_browser_data::Cookies, String>>,
    history: Option<Result<Vec<brigadier_browser_data::Visit>, String>>,
}

fn read(home: &std::path::Path, id: &str, cookies: bool, history: bool) -> Result<Read, IpcError> {
    let profile: Profile = brigadier_browser_data::discover(home)
        .into_iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| refused("that browser profile is gone"))?;
    if brigadier_browser_data::running(&profile) {
        return Err(refused(format!(
            "Quit {} first, then import again.",
            profile.browser
        )));
    }
    let browser = profile.browser.clone();
    let cookies = cookies.then(|| {
        let password = match (&profile.family, &profile.keychain_service) {
            (Family::Chromium, Some(service)) => Some(chromium_password(
                service,
                profile.keychain_account.as_deref(),
            )),
            _ => None,
        };
        let password = match password.transpose() {
            Ok(password) => password,
            Err(error) => return Err(reason(&error, &browser, "cookies")),
        };
        let now = crate::browser_store::now_ms() / 1000;
        brigadier_browser_data::read_cookies(&profile, password.as_deref(), now)
            .map_err(|error| reason(&error, &browser, "cookies"))
    });
    let history = history.then(|| {
        brigadier_browser_data::read_history(&profile, HISTORY_LIMIT)
            .map_err(|error| reason(&error, &browser, "history"))
    });
    Ok(Read { cookies, history })
}

#[cfg(target_os = "macos")]
fn chromium_password(service: &str, account: Option<&str>) -> Result<Vec<u8>, Error> {
    brigadier_browser_data::chromium_password(service, account)
}

#[cfg(not(target_os = "macos"))]
fn chromium_password(_service: &str, _account: Option<&str>) -> Result<Vec<u8>, Error> {
    Err(Error::Keychain(KeychainError::NotFound))
}

/// Opens System Settings at Privacy & Security › Full Disk Access, which Safari's files need.
#[tauri::command]
pub fn browser_import_open_access(app: tauri::AppHandle) -> Result<(), IpcError> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(
            "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles",
            None::<&str>,
        )
        .map_err(|err| refused(format!("could not open System Settings: {err}")))
}

/// Adds the cookies read to the browser's WebKit store, and counts what it took.
#[cfg(target_os = "macos")]
async fn add_cookies(
    app: &tauri::AppHandle,
    found: brigadier_browser_data::Cookies,
    progress: &Channel<ImportProgress>,
) -> Result<CookieImport, IpcError> {
    let skipped = count(found.expired + found.partitioned + found.unreadable);
    let total = count(found.cookies.len());
    let _ = progress.send(ImportProgress::Cookies { done: 0, total });
    let channel = progress.clone();
    let cookies = found.cookies;
    let stored: Vec<bool> = crate::browser::on_store(app, move |identifier, done| {
        crate::browser_ui::set_cookies(
            identifier,
            cookies,
            move |added| {
                if added % 50 == 0 || count(added) == total {
                    let _ = channel.send(ImportProgress::Cookies {
                        done: count(added),
                        total,
                    });
                }
            },
            done,
        );
    })
    .await?;
    let imported = count(stored.iter().filter(|stored| **stored).count());
    Ok(CookieImport {
        imported,
        rejected: total.saturating_sub(imported),
        skipped,
        error: None,
    })
}

#[cfg(not(target_os = "macos"))]
async fn add_cookies(
    _app: &tauri::AppHandle,
    _found: brigadier_browser_data::Cookies,
    _progress: &Channel<ImportProgress>,
) -> Result<CookieImport, IpcError> {
    Err(refused("importing cookies works on macOS only"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn why_a_profile_cant_be_read_is_said_plainly() {
        let path = PathBuf::from("/x");
        assert_eq!(
            reason(&Error::Keychain(KeychainError::Denied), "Dia", "cookies"),
            "macOS didn't let Brigadier use Dia's cookie key, so no cookies were copied."
        );
        assert!(
            reason(&Error::PermissionDenied(path.clone()), "Safari", "history")
                .starts_with("Brigadier needs Full Disk Access to read Safari's history")
        );
        assert_eq!(
            reason(&Error::Locked(path), "Firefox", "cookies"),
            "Firefox is still using its cookies. Quit it and try again."
        );
    }

    #[test]
    fn a_missing_profile_is_refused() {
        let empty = std::env::temp_dir().join("brigadier-import-test-empty-home");
        let Err(err) = read(&empty, "firefox/none", true, true) else {
            panic!("a profile that isn't there can't be read");
        };
        assert_eq!(err.message, "that browser profile is gone");
    }
}
