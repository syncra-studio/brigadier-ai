//! Settings › Import Browser Data, the part that needs no window toolkit: which browsers are here
//! and where each stands, and reading one of a browser's profiles for main to write into a
//! Brigadier profile. `brigadier-browser-data` does the reading and never writes a source's files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use brigadier_browser_data::{
    Bookmark, ChromiumKeys, Cookie, Error, Family, Profile, SecretError, Source, Visit,
};
use brigadier_ipc::app::{ImportFailure, ImportFamily, ImportSourceProfile, ImportState};
use serde::{Deserialize, Serialize};

/// The most visits one profile's import brings over, newest first.
const HISTORY_LIMIT: usize = 50_000;
/// How long a browser's key stays for the rest of its profiles: one question per browser.
const KEY_KEPT: Duration = Duration::from_secs(120);
/// Linux has no password import, by the user's decision: it holds for every browser there.
const PASSWORDS_HERE: bool = !cfg!(target_os = "linux");

/// One browser as the page lists it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listed {
    pub key: String,
    pub name: String,
    pub family: ImportFamily,
    pub app_path: PathBuf,
    pub state: ImportState,
    /// Its profiles with something to bring, the default first.
    pub profiles: Vec<ImportSourceProfile>,
    /// The names of those with nothing to bring, which get no Brigadier profile.
    pub empty_profiles: Vec<String>,
    /// Whether this build may read its cookies.
    pub sign_ins: bool,
    /// Whether this build may read its passwords.
    pub passwords: bool,
}

fn family(family: Family) -> ImportFamily {
    match family {
        Family::Chromium => ImportFamily::Chromium,
        Family::Gecko => ImportFamily::Gecko,
        Family::Safari => ImportFamily::Safari,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Whether a browser is open. Safari's cookie file is read whole and its history through a
/// snapshot, so Safari can stay open.
fn running(source: &Source) -> bool {
    source.family != Family::Safari
        && source
            .profiles
            .first()
            .is_some_and(brigadier_browser_data::running)
}

/// Whether the browser's key can be asked for, without asking. Only macOS has a question to ask,
/// and Windows and Linux report their problems when reading.
#[cfg(target_os = "macos")]
fn key_state(source: &Source) -> Option<ImportState> {
    let profile = source.profiles.first()?;
    if source.family != Family::Chromium || brigadier_browser_data::key_off_limits(profile) {
        return None;
    }
    let service = profile.keychain_service.as_deref()?;
    match brigadier_browser_data::chromium_password_exists(
        service,
        profile.keychain_account.as_deref(),
    ) {
        Ok(true) => None,
        Ok(false) => Some(ImportState::NoKeychainKey),
        Err(error) => {
            tracing::debug!(%error, browser = %source.name, "Keychain lookup failed");
            Some(ImportState::Unavailable)
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn key_state(_source: &Source) -> Option<ImportState> {
    None
}

fn listed(source: Source) -> Listed {
    let mut state = if running(&source) {
        ImportState::Running
    } else if source.profiles.iter().any(|profile| !profile.readable) {
        ImportState::NeedsFullDiskAccess
    } else {
        key_state(&source).unwrap_or(ImportState::Ready)
    };
    let off_limits = source.profiles.iter().any(brigadier_browser_data::key_off_limits);
    let mut profiles = Vec::new();
    let mut empty_profiles = Vec::new();
    if matches!(state, ImportState::Ready) {
        for profile in &source.profiles {
            if brigadier_browser_data::profile_is_empty(profile) {
                empty_profiles.push(profile.name.clone());
                continue;
            }
            let cookies = match brigadier_browser_data::count_cookies(profile) {
                Ok(n) => Some(count(n)),
                Err(Error::PermissionDenied(_)) => {
                    state = ImportState::NeedsFullDiskAccess;
                    profiles.clear();
                    empty_profiles.clear();
                    break;
                }
                Err(error) => {
                    tracing::debug!(%error, profile = %profile.id, "cookies couldn't be counted");
                    None
                }
            };
            profiles.push(ImportSourceProfile {
                id: profile.id.clone(),
                name: profile.name.clone(),
                cookies,
                is_default: profile.is_default,
            });
        }
    }
    Listed {
        key: source.key,
        name: source.name,
        family: family(source.family),
        app_path: source.app_path,
        state,
        profiles,
        empty_profiles,
        sign_ins: !off_limits,
        passwords: PASSWORDS_HERE && !off_limits,
    }
}

/// The browsers installed for the user whose home folder is `home`, in Settings' order.
pub fn list(home: &Path) -> Vec<Listed> {
    brigadier_browser_data::sources(home)
        .into_iter()
        .map(listed)
        .collect()
}

/// What a tick in the import brings.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Parts {
    pub sign_ins: bool,
    pub history: bool,
    pub bookmarks: bool,
    pub passwords: bool,
}

/// One saved password on its way to the profile's vault.
#[derive(Debug, Serialize)]
pub struct Login {
    pub url: String,
    pub username: String,
    pub password: String,
}

/// One profile read for import, or why it can't be.
#[derive(Debug, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ReadOutcome {
    Read(Box<ReadProfile>),
    Failed {
        reason: ImportFailure,
        message: String,
    },
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadProfile {
    pub profile_name: String,
    /// The source's sign-ins were read, even if there are none: importing over a profile swaps them.
    pub sign_ins_read: bool,
    pub cookies: Vec<Cookie>,
    /// Domains whose cookies couldn't be opened or can't be kept, one per cookie.
    pub left_out_domains: Vec<String>,
    /// Those locked to the source browser on this device (Chrome's App-Bound Encryption).
    pub app_bound_domains: Vec<String>,
    /// A Chromium source's folder of site storage, when sign-ins were asked for.
    pub site_storage_from: Option<PathBuf>,
    pub history: Vec<Visit>,
    pub history_error: Option<String>,
    pub bookmarks: Vec<Bookmark>,
    pub bookmarks_error: Option<String>,
    /// The source's passwords were read, even if there are none: importing over a profile can say it kept its own.
    pub passwords_read: bool,
    pub logins: Vec<Login>,
    pub logins_left_out: u32,
    pub passwords_error: Option<String>,
}

fn failed(reason: ImportFailure, message: String) -> ReadOutcome {
    ReadOutcome::Failed { reason, message }
}

/// Why `browser`'s cookies couldn't be read, as the page's reason and in plain words.
fn cookie_failure(error: &Error, browser: &str) -> (ImportFailure, String) {
    let macos = cfg!(target_os = "macos");
    match error {
        Error::Secret(SecretError::Denied) if macos => (
            ImportFailure::NeedsKeychainAccess,
            format!("macOS didn't let Brigadier use {browser}'s cookie key."),
        ),
        Error::Secret(SecretError::NotFound) if macos => (
            ImportFailure::NoKeychainKey,
            format!("{browser}'s cookie key isn't in your Keychain."),
        ),
        Error::Secret(secret) => (
            ImportFailure::ReadFailed,
            match secret {
                SecretError::Locked => format!(
                    "Your keyring is locked, so {browser}'s cookies can't be opened. Unlock it and try again."
                ),
                SecretError::Denied => format!(
                    "{browser}'s cookie key belongs to another user, so Brigadier can't open it."
                ),
                SecretError::NotFound => format!("{browser} has no cookie key yet."),
                SecretError::NoStore => {
                    format!("This machine has no keyring holding {browser}'s cookie key.")
                }
                SecretError::Other(_) => format!("{browser}'s cookie key couldn't be read."),
            },
        ),
        Error::PermissionDenied(_) if macos => (
            ImportFailure::NeedsFullDiskAccess,
            format!("Brigadier needs Full Disk Access to read {browser}'s cookies."),
        ),
        Error::PermissionDenied(_) => (
            ImportFailure::ReadFailed,
            format!("Your system won't let Brigadier read {browser}'s cookies."),
        ),
        Error::Locked(_) => (
            ImportFailure::Running,
            format!("{browser} is still using its cookies."),
        ),
        _ => (
            ImportFailure::ReadFailed,
            format!("{browser}'s cookies couldn't be read ({error})."),
        ),
    }
}

fn password_failure(error: &Error, browser: &str) -> String {
    match error {
        Error::NotReadable(_) => {
            format!(
                "{browser} keeps its passwords in iCloud Keychain, which other apps can't read."
            )
        }
        Error::Secret(SecretError::Locked) => {
            format!("{browser}'s passwords are locked with a Primary Password.")
        }
        Error::PermissionDenied(_) => {
            format!("Your system won't let Brigadier read {browser}'s passwords.")
        }
        // Not the error's text: nothing about a password's store needs repeating.
        _ => format!("{browser}'s passwords couldn't be read."),
    }
}

/// A browser's key for its other profiles, by browser key. Gone when it's next looked at, late.
static KEYS: Mutex<Option<HashMap<String, (ChromiumKeys, Instant)>>> = Mutex::new(None);

fn keys_of(browser: &str, profile: &Profile) -> Result<ChromiumKeys, Error> {
    let mut kept = KEYS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let kept = kept.get_or_insert_default();
    kept.retain(|_, (_, at)| at.elapsed() < KEY_KEPT);
    if let Some((keys, _)) = kept.get(browser) {
        return Ok(keys.clone());
    }
    let keys = brigadier_browser_data::chromium_keys(profile)?;
    kept.insert(browser.to_owned(), (keys.clone(), Instant::now()));
    Ok(keys)
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_secs()).unwrap_or(0))
}

fn read_profile(source: &Source, profile: &Profile, what: &Parts) -> Result<ReadProfile, ReadOutcome> {
    let browser = &source.name;
    let mut out = ReadProfile {
        profile_name: profile.name.clone(),
        ..ReadProfile::default()
    };
    let chromium = profile.family == Family::Chromium;
    let off_limits = brigadier_browser_data::key_off_limits(profile);
    let wants_key = chromium && !off_limits && (what.sign_ins || (what.passwords && PASSWORDS_HERE));
    let keys = if wants_key {
        match keys_of(&source.key, profile) {
            Ok(keys) => keys,
            Err(error) => {
                let (reason, message) = cookie_failure(&error, browser);
                return Err(failed(reason, message));
            }
        }
    } else {
        ChromiumKeys::default()
    };
    if what.sign_ins && !off_limits {
        match brigadier_browser_data::read_cookies_with_keys(profile, &keys, now_unix()) {
            Ok(read) => {
                let mut left_out = read.read.unreadable_domains;
                // Electron can't set a partitioned cookie, and its copy without the partition
                // would be sent where the site never meant it to go.
                left_out.extend(read.read.partitioned_domains);
                out.left_out_domains = left_out;
                out.app_bound_domains = read.app_bound_domains;
                out.cookies = read.read.cookies;
            }
            // A profile without a cookie file just brings none.
            Err(Error::NotFound(_)) => {}
            Err(error) => {
                let (reason, message) = cookie_failure(&error, browser);
                return Err(failed(reason, message));
            }
        }
        out.sign_ins_read = true;
        if chromium {
            out.site_storage_from = Some(profile.dir.clone());
        }
    }
    if what.history && profile.has_history {
        match brigadier_browser_data::read_history(profile, HISTORY_LIMIT) {
            Ok(visits) => out.history = visits,
            Err(Error::PermissionDenied(_)) => {
                out.history_error = Some(format!(
                    "Brigadier needs permission to read {browser}'s history."
                ));
            }
            Err(error) => {
                out.history_error = Some(format!("{browser}'s history couldn't be read ({error})."));
            }
        }
    }
    if what.bookmarks && profile.has_bookmarks {
        match brigadier_browser_data::read_bookmarks(profile) {
            Ok(found) => out.bookmarks = found.bookmarks,
            Err(error) => {
                out.bookmarks_error = Some(format!("{browser}'s bookmarks couldn't be read ({error})."));
            }
        }
    }
    if what.passwords && PASSWORDS_HERE && !off_limits {
        match brigadier_browser_data::read_passwords(profile, &keys) {
            Ok(found) => {
                out.logins_left_out = count(found.unreadable);
                out.logins = found
                    .credentials
                    .into_iter()
                    .map(|credential| Login {
                        password: credential.password.reveal().to_owned(),
                        url: credential.origin_url,
                        username: credential.username,
                    })
                    .collect();
                out.passwords_read = true;
            }
            Err(Error::NotFound(_)) => out.passwords_read = true,
            Err(error) => out.passwords_error = Some(password_failure(&error, browser)),
        }
    }
    Ok(out)
}

/// Reads the profile `id` of the browser `key`. Refused while that browser is open: its files may
/// be half written.
pub fn read(home: &Path, key: &str, id: &str, what: &Parts) -> ReadOutcome {
    let gone = || {
        failed(
            ImportFailure::Gone,
            "That browser or profile is no longer here.".to_owned(),
        )
    };
    let Some(source) = brigadier_browser_data::sources(home)
        .into_iter()
        .find(|source| source.key == key)
    else {
        return gone();
    };
    let Some(profile) = source.profiles.iter().find(|profile| profile.id == id) else {
        return gone();
    };
    if running(&source) {
        return failed(
            ImportFailure::Running,
            format!("Quit {} first, then import again.", source.name),
        );
    }
    match read_profile(&source, profile, what) {
        Ok(read) => ReadOutcome::Read(Box::new(read)),
        Err(outcome) => outcome,
    }
}
