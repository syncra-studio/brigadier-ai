//! Reads other browsers' cookies and history so Brigadier's built-in browser can import them.
//!
//! - **Discovery.** [`discover`] lists the profiles of the Chromium-family, Gecko-family and
//!   Safari browsers installed for one user, from the folders those browsers keep under
//!   `~/Library`. Known browsers get their display name, app path and Keychain entry; unknown
//!   Chromium and Gecko browsers are found by the shape of their folders.
//! - **Reading.** [`read_cookies`] and [`read_history`] turn a profile's databases into plain
//!   structs. Source databases are opened read-only and copied into memory with SQLite's
//!   online backup before any query runs, so a browser's own files are never written.
//! - **Nothing is written.** The app imports the results into WebKit itself; this crate only
//!   reads.
//!
//! Imports are expected to run with the source browser quit ([`running`] says when it is not),
//! because a running browser holds its databases open and may not have flushed them.

mod chromium;
mod gecko;
mod running;
mod safari;
mod sources;
mod sqlite;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use running::running;
pub use sources::{Source, browser_rank, sources, sources_in};

/// Seconds between 1601-01-01 (the Windows/WebKit epoch Chromium uses) and the Unix epoch.
pub const WEBKIT_EPOCH_OFFSET_SECS: i64 = 11_644_473_600;
/// Seconds between 2001-01-01 (Apple's Core Data epoch Safari uses) and the Unix epoch.
pub const APPLE_EPOCH_OFFSET_SECS: i64 = 978_307_200;

/// The engine family a browser belongs to; it decides the file formats a profile uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Family {
    /// Chrome, Edge, Brave, Arc, Dia, Vivaldi, Opera and other Chromium builds.
    Chromium,
    /// Firefox and its forks (Zen, LibreWolf, Waterfox, Floorp).
    Gecko,
    /// Safari.
    Safari,
}

/// One browser profile found on disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Stable id, `<browser key>/<profile dir name>`, e.g. `dia/Default` or `safari/default`, so
    /// the app can remember which profile a user picked across launches.
    pub id: String,
    /// The browser's display name, e.g. "Google Chrome".
    pub browser: String,
    /// The profile's display name, e.g. "Person 1" or "default-release".
    pub name: String,
    /// The engine family, which decides how the files are read.
    pub family: Family,
    /// The profile directory.
    pub dir: PathBuf,
    /// The browser's app bundle when known; [`running`] uses it to tell whether it is open.
    pub app_path: Option<PathBuf>,
    /// Chromium only: the Keychain service holding the cookie encryption password.
    pub keychain_service: Option<String>,
    /// Chromium only: the Keychain account of that password.
    pub keychain_account: Option<String>,
    /// Whether the profile has a cookie store.
    pub has_cookies: bool,
    /// Whether the profile has a history database.
    pub has_history: bool,
    /// False when the files exist but can't be opened, which for Safari means Brigadier lacks
    /// Full Disk Access.
    pub readable: bool,
    /// The profile the browser itself opens by default, so the app can preselect it.
    pub is_default: bool,
}

/// A cookie as a browser stored it, in the shape WebKit needs to set it again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookie {
    /// The cookie's name.
    pub name: String,
    /// The cookie's value, decrypted.
    pub value: String,
    /// `.example.com` for a domain cookie, `example.com` for a host-only one.
    pub domain: String,
    /// True when the cookie is sent only to the exact host that set it.
    pub host_only: bool,
    /// The cookie's path.
    pub path: String,
    /// Expiry in Unix seconds; `None` for a session cookie.
    pub expires_unix: Option<i64>,
    /// Sent only over HTTPS.
    pub secure: bool,
    /// Hidden from page scripts.
    pub http_only: bool,
    /// The SameSite attribute; `None` when the browser recorded it as unspecified.
    pub same_site: Option<SameSite>,
}

/// A cookie's SameSite attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SameSite {
    /// Sent on cross-site requests too.
    None,
    /// Sent on top-level cross-site navigations only.
    Lax,
    /// Never sent cross-site.
    Strict,
}

/// The cookies read from one profile, plus counts of the ones left out and why, so the app
/// can tell the user what didn't come across.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cookies {
    /// The cookies to import.
    pub cookies: Vec<Cookie>,
    /// Skipped because they had already expired.
    pub expired: usize,
    /// Skipped because they were partitioned (per top-level site, or per Firefox container),
    /// an isolation WebKit's cookie store can't keep.
    pub partitioned: usize,
    /// Skipped because they could not be decrypted, failed their integrity check or were
    /// malformed.
    pub unreadable: usize,
    /// The domains of the unreadable cookies whose domain could be read, one per cookie.
    pub unreadable_domains: Vec<String>,
    /// The domains of the partitioned cookies, one per cookie.
    pub partitioned_domains: Vec<String>,
}

/// One visit to a page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    /// The page's URL (http or https).
    pub url: String,
    /// The page's title, empty when the browser recorded none.
    pub title: String,
    /// When the visit happened, in Unix milliseconds.
    pub visited_at_ms: i64,
}

/// Why the Keychain refused to hand over a browser's password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeychainError {
    /// The user declined the prompt, or access is not allowed right now.
    Denied,
    /// No such Keychain item.
    NotFound,
    /// Any other Keychain failure, with its message.
    Other(String),
}

impl std::fmt::Display for KeychainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Denied => f.write_str("access denied"),
            Self::NotFound => f.write_str("item not found"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// What can go wrong reading a profile.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file is missing.
    #[error("not found: {}", .0.display())]
    NotFound(PathBuf),
    /// The OS refused to open the file (EPERM/EACCES); for Safari this means Brigadier needs
    /// Full Disk Access.
    #[error("permission denied: {}", .0.display())]
    PermissionDenied(PathBuf),
    /// The database is busy or locked, usually because the browser is still running.
    #[error("database is locked: {}", .0.display())]
    Locked(PathBuf),
    /// Any other SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Any other I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// The Keychain refused or had no such password.
    #[error("keychain: {0}")]
    Keychain(KeychainError),
    /// The file is not in the shape this crate understands.
    #[error("unexpected format: {0}")]
    Format(String),
}

/// Where macOS keeps apps: the shared and the user's Applications folders, then the system's
/// (Safari lives in the last).
pub fn app_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications"),
        home.join("Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Cryptexes/App/System/Applications"),
    ]
}

/// Lists the browser profiles found under `home` (the user's home directory), sorted by
/// browser then profile name. Apps are looked up in [`app_dirs`].
pub fn discover(home: &Path) -> Vec<Profile> {
    let dirs = app_dirs(home);
    discover_in(home, &dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

/// [`discover`] with explicit folders to look for app bundles in, so tests and callers with
/// unusual layouts don't depend on what is installed in `/Applications`.
pub fn discover_in(home: &Path, app_dirs: &[&Path]) -> Vec<Profile> {
    let mut profiles = Vec::new();
    profiles.extend(chromium::discover(home, app_dirs));
    profiles.extend(gecko::discover(home, app_dirs));
    profiles.extend(safari::discover(home, app_dirs));
    profiles.sort_by(|a, b| {
        (a.browser.to_lowercase(), a.name.to_lowercase(), &a.id).cmp(&(
            b.browser.to_lowercase(),
            b.name.to_lowercase(),
            &b.id,
        ))
    });
    profiles
}

/// Reads a profile's cookies, skipping expired and partitioned ones. `chromium_password` is the
/// Keychain password from [`chromium_password`]; without it, encrypted Chromium cookies count as
/// unreadable. `now_unix` decides what has expired.
pub fn read_cookies(
    profile: &Profile,
    chromium_password: Option<&[u8]>,
    now_unix: i64,
) -> Result<Cookies, Error> {
    match profile.family {
        Family::Chromium => chromium::read_cookies(&profile.dir, chromium_password, now_unix),
        Family::Gecko => gecko::read_cookies(&profile.dir, now_unix),
        Family::Safari => safari::read_cookies(profile, now_unix),
    }
}

/// How many cookies a profile keeps, read without decrypting them (and so without asking the
/// Keychain). A running browser's store is read as it stands, unflushed changes included.
pub fn count_cookies(profile: &Profile) -> Result<usize, Error> {
    match profile.family {
        Family::Chromium => chromium::count_cookies(&profile.dir),
        Family::Gecko => gecko::count_cookies(&profile.dir),
        Family::Safari => safari::count_cookies(profile),
    }
}

/// Reads up to `limit` visits from a profile's history, newest first, one entry per visit,
/// http and https pages only.
pub fn read_history(profile: &Profile, limit: usize) -> Result<Vec<Visit>, Error> {
    match profile.family {
        Family::Chromium => chromium::read_history(&profile.dir, limit),
        Family::Gecko => gecko::read_history(&profile.dir, limit),
        Family::Safari => safari::read_history(&profile.dir, limit),
    }
}

/// Fetches a Chromium browser's cookie encryption password ("<Browser> Safe Storage") from the
/// login Keychain. macOS asks the user to allow it; a refusal is [`KeychainError::Denied`].
#[cfg(target_os = "macos")]
pub fn chromium_password(service: &str, account: Option<&str>) -> Result<Vec<u8>, Error> {
    use security_framework::item::{ItemClass, ItemSearchOptions, SearchResult};
    use security_framework::passwords;

    // Security framework status codes (SecBase.h).
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    const ERR_SEC_USER_CANCELED: i32 = -128;
    const ERR_SEC_AUTH_FAILED: i32 = -25293;
    const ERR_SEC_INTERACTION_NOT_ALLOWED: i32 = -25308;

    let map = |err: security_framework::base::Error| {
        Error::Keychain(match err.code() {
            ERR_SEC_ITEM_NOT_FOUND => KeychainError::NotFound,
            ERR_SEC_USER_CANCELED | ERR_SEC_AUTH_FAILED | ERR_SEC_INTERACTION_NOT_ALLOWED => {
                KeychainError::Denied
            }
            _ => KeychainError::Other(err.to_string()),
        })
    };
    if let Some(account) = account {
        return passwords::get_generic_password(service, account).map_err(map);
    }
    let results = ItemSearchOptions::new()
        .class(ItemClass::generic_password())
        .service(service)
        .load_data(true)
        .limit(1i64)
        .search()
        .map_err(map)?;
    results
        .into_iter()
        .find_map(|result| match result {
            SearchResult::Data(data) => Some(data),
            _ => None,
        })
        .ok_or(Error::Keychain(KeychainError::NotFound))
}

/// Whether the login Keychain holds a Chromium browser's cookie encryption password, looked up
/// by its attributes only, so macOS never asks the user. `Ok(false)` when it isn't there.
#[cfg(target_os = "macos")]
pub fn chromium_password_exists(service: &str, account: Option<&str>) -> Result<bool, Error> {
    use security_framework::item::{ItemClass, ItemSearchOptions};

    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    let mut search = ItemSearchOptions::new();
    search
        .class(ItemClass::generic_password())
        .service(service)
        .load_attributes(true)
        .limit(1i64);
    if let Some(account) = account {
        search.account(account);
    }
    match search.search() {
        Ok(found) => Ok(!found.is_empty()),
        Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
        Err(err) => Err(Error::Keychain(KeychainError::Other(err.to_string()))),
    }
}

/// Whether a URL is one the built-in browser can open again.
fn is_web_url(url: &str) -> bool {
    let lower = url
        .get(..8)
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// Whether a file exists, and whether it can be opened. A file whose metadata the OS refuses
/// (as macOS does for Safari's folders without Full Disk Access) counts as existing but
/// unreadable.
fn probe(path: &Path) -> (bool, bool) {
    match std::fs::File::open(path) {
        Ok(file) => (file.metadata().map(|m| m.is_file()).unwrap_or(false), true),
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => (true, false),
        Err(_) => match std::fs::metadata(path) {
            Ok(meta) => (meta.is_file(), false),
            Err(err) => (err.kind() == std::io::ErrorKind::PermissionDenied, false),
        },
    }
}

/// The first of `bundles` found in `app_dirs`, else the first in the first folder, since the
/// path still names the bundle [`running`] looks for.
fn app_path(app_dirs: &[&Path], bundles: &[&str]) -> Option<PathBuf> {
    for bundle in bundles {
        for dir in app_dirs {
            let path = dir.join(bundle);
            if path.exists() {
                return Some(path);
            }
        }
    }
    let (dir, bundle) = (app_dirs.first()?, bundles.first()?);
    Some(dir.join(bundle))
}

/// Whether the app bundle at `app` says it opens web links (`http`/`https` among its
/// `CFBundleURLSchemes`), as every browser does and apps built on Chromium (Electron) don't.
fn opens_web_links(app: &Path) -> bool {
    let Ok(info) = plist::Value::from_file(app.join("Contents/Info.plist")) else {
        return false;
    };
    let types = info
        .as_dictionary()
        .and_then(|info| info.get("CFBundleURLTypes"))
        .and_then(plist::Value::as_array);
    types.into_iter().flatten().any(|kind| {
        kind.as_dictionary()
            .and_then(|kind| kind.get("CFBundleURLSchemes"))
            .and_then(plist::Value::as_array)
            .is_some_and(|schemes| {
                schemes
                    .iter()
                    .any(|scheme| matches!(scheme.as_string(), Some("http" | "https")))
            })
    })
}

/// The app bundle in `app_dirs` named `bundle`, only if it exists.
fn existing_app(app_dirs: &[&Path], bundle: &str) -> Option<PathBuf> {
    app_dirs
        .iter()
        .map(|dir| dir.join(bundle))
        .find(|path| path.exists())
}

/// Lowercase ASCII key for a browser id, e.g. "Google Chrome Beta" → "google-chrome-beta".
fn key_for(name: &str) -> String {
    let mut key = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            key.push(c.to_ascii_lowercase());
        } else if !key.ends_with('-') && !key.is_empty() {
            key.push('-');
        }
    }
    key.trim_end_matches('-').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_urls() {
        assert!(is_web_url("https://example.com"));
        assert!(is_web_url("HTTP://example.com"));
        assert!(!is_web_url("file:///etc/hosts"));
        assert!(!is_web_url("chrome://settings"));
        assert!(!is_web_url("http"));
    }

    #[test]
    fn keys() {
        assert_eq!(key_for("Google Chrome Beta"), "google-chrome-beta");
        assert_eq!(
            key_for("com.operasoftware.Opera"),
            "com-operasoftware-opera"
        );
        assert_eq!(key_for("  Zen  "), "zen");
    }
}
