//! Reads other browsers' cookies, history and bookmarks so Brigadier's built-in browser can import
//! them, on macOS, Windows and Linux.
//!
//! - **Discovery.** [`discover`] lists the profiles of the Chromium-family, Gecko-family and
//!   Safari browsers installed for one user, from the folders those browsers keep on this OS
//!   (see [`Layout`]). Known browsers get their display name, app path and where their cookie key
//!   is kept; on macOS, unknown Chromium and Gecko browsers are found by the shape of their
//!   folders.
//! - **Reading.** [`read_cookies`], [`read_history`] and [`read_bookmarks`] turn a profile's files
//!   into plain structs. Source databases are opened read-only and copied into memory with
//!   SQLite's online backup before any query runs, so a browser's own files are never written.
//! - **Keys.** [`chromium_keys`] fetches what opens Chromium's encrypted cookies on this OS, and
//!   [`read_cookies_with_keys`] uses it. Chrome's `v20` App-Bound cookies are never opened: see
//!   the `keys` module.
//! - **The source is never written.** The app imports the results itself; the one thing this
//!   crate writes is a copy of a source's site storage into a folder the app names.
//!
//! Imports are expected to run with the source browser quit ([`running`] says when it is not),
//! because a running browser holds its databases open and may not have flushed them.

mod chromium;
mod gecko;
mod icons;
mod keys;
mod layout;
mod passwords;
mod running;
mod safari;
// Compiled on any Unix for its tests, which run the client against a fake service.
#[cfg(any(target_os = "linux", all(test, unix)))]
mod secret_service;
mod site_storage;
mod sources;
mod sqlite;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub use site_storage::{SITE_STORAGE_FOLDERS, copy_site_storage};
pub use icons::app_icon_file;
pub use keys::{ChromiumKeys, chromium_keys};
pub use layout::{Layout, Os};
pub use passwords::{Credential, Password, Passwords, read_passwords};
pub use running::running;
pub use sources::{Source, browser_rank, sources, sources_in, sources_layout};

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
    /// The browser's app (a bundle on macOS, an executable or launcher elsewhere) when known;
    /// [`running`] uses it to tell whether it is open.
    pub app_path: Option<PathBuf>,
    /// Chromium on macOS only: the Keychain service holding the cookie encryption password.
    pub keychain_service: Option<String>,
    /// Chromium on macOS only: the Keychain account of that password.
    pub keychain_account: Option<String>,
    /// Chromium only: the browser's `Local State`, which on Windows holds its sealed master key.
    pub local_state: Option<PathBuf>,
    /// Chromium on Linux only: the `application` attribute of its keyring password.
    pub linux_secret_application: Option<String>,
    /// Whether the profile has a cookie store.
    pub has_cookies: bool,
    /// Whether the profile has a history database.
    pub has_history: bool,
    /// Whether the profile has bookmarks to read.
    pub has_bookmarks: bool,
    /// False when the cookie or history files exist but can't be opened, which for Safari means
    /// Brigadier lacks Full Disk Access.
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
    /// The browser recorded that plain http set it, which a store binding cookies to their scheme
    /// has to know; false when it recorded https or nothing.
    #[serde(default)]
    pub source_insecure: bool,
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
    /// Chromium's partitioned cookies that could be opened, which a Chromium target can keep as
    /// they were. They are counted in `partitioned` too.
    pub partitioned_cookies: Vec<PartitionedCookie>,
}

/// A cookie kept per top-level site (CHIPS), with what makes up its partition in Chromium.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionedCookie {
    pub cookie: Cookie,
    /// The top-level site it belongs to.
    pub top_frame_site_key: String,
    /// The other half of Chromium's partition key.
    pub has_cross_site_ancestor: bool,
}

/// A Chromium profile's cookies when read with [`read_cookies_with_keys`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromiumCookies {
    /// The cookies. App-Bound ones are among its unreadable ones, and in `unreadable_domains`.
    pub read: Cookies,
    /// The domains of the `v20` App-Bound cookies, one per cookie: Chrome on Windows locks these
    /// to itself, so the user has to sign in to these sites again.
    pub app_bound_domains: Vec<String>,
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

/// Which part of a browser's bookmarks a bookmark is under. The engines name these differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BookmarkRoot {
    /// The bar under the address field (Chromium's bookmarks bar, Firefox's toolbar, Safari's
    /// Favorites).
    Bar,
    /// The menu (Firefox and Safari).
    Menu,
    /// Other bookmarks, including Safari's reading list and anything at its top level.
    Other,
    /// Bookmarks synced from a phone.
    Mobile,
}

/// One bookmark, in the order the browser shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    /// The page's URL (http or https).
    pub url: String,
    /// The bookmark's title, empty when it has none.
    pub title: String,
    /// The part of the browser's bookmarks it is under.
    pub root: BookmarkRoot,
    /// The folders from that root down to the bookmark, outermost first.
    pub folders: Vec<String>,
    /// When it was added, in Unix milliseconds, when the browser recorded it.
    pub added_at_ms: Option<i64>,
}

/// The bookmarks read from one profile.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmarks {
    /// The bookmarks to import.
    pub bookmarks: Vec<Bookmark>,
    /// Left out because the built-in browser can't open them, such as `javascript:` or `file:`.
    pub skipped: usize,
}

/// Why a browser's cookie key couldn't be fetched, on any OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretError {
    /// The store holding the key is locked, and the user can unlock it and try again (Linux).
    Locked,
    /// The store refused: the user declined (macOS), or the key is sealed for another user
    /// (Windows).
    Denied,
    /// The store has no key for this browser.
    NotFound,
    /// This machine has no store to ask (Linux with no keyring or no session bus).
    NoStore,
    /// Any other failure, with its message.
    Other(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Locked => f.write_str("the store holding it is locked"),
            Self::Denied => f.write_str("access denied"),
            Self::NotFound => f.write_str("key not found"),
            Self::NoStore => f.write_str("no key store on this machine"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

impl From<KeychainError> for SecretError {
    fn from(err: KeychainError) -> Self {
        match err {
            KeychainError::Denied => Self::Denied,
            KeychainError::NotFound => Self::NotFound,
            KeychainError::Other(message) => Self::Other(message),
        }
    }
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
    /// The OS refused to open the file (EPERM/EACCES); for Safari on macOS this means Brigadier
    /// needs Full Disk Access.
    #[error("permission denied: {}", .0.display())]
    PermissionDenied(PathBuf),
    /// The file is busy or locked, usually because the browser is still running (on Windows,
    /// Chromium holds its files shut to others while it runs).
    #[error("database is locked: {}", .0.display())]
    Locked(PathBuf),
    /// Any other SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// Any other I/O failure.
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// [`chromium_password`] only: the Keychain refused or had no such password.
    #[error("keychain: {0}")]
    Keychain(KeychainError),
    /// [`chromium_keys`] on any OS: the browser's cookie key couldn't be fetched.
    #[error("cookie key: {0}")]
    Secret(SecretError),
    /// The file is not in the shape this crate understands.
    #[error("unexpected format: {0}")]
    Format(String),
    /// This browser keeps this data where no other app can read it (Safari's passwords).
    #[error("not readable by other apps: {0}")]
    NotReadable(&'static str),
    /// A folder isn't one a Chromium target can use as a profile.
    #[error("not a usable profile folder: {}", .0.display())]
    BadTarget(PathBuf),
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

/// Lists the browser profiles found for the user whose home directory is `home`, on this OS,
/// sorted by browser then profile name.
pub fn discover(home: &Path) -> Vec<Profile> {
    discover_layout(&Layout::current(home))
}

/// [`discover`] over macOS's folders, whichever OS this runs on, with explicit folders to look
/// for app bundles in, so tests and callers with unusual layouts don't depend on what is
/// installed in `/Applications`.
pub fn discover_in(home: &Path, app_dirs: &[&Path]) -> Vec<Profile> {
    discover_layout(&Layout::Macos {
        home: home.to_owned(),
        app_dirs: app_dirs.iter().map(|dir| dir.to_path_buf()).collect(),
    })
}

/// [`discover`] over the folders of `layout`, whichever OS this runs on.
pub fn discover_layout(layout: &Layout) -> Vec<Profile> {
    let mut profiles = Vec::new();
    match layout {
        Layout::Macos { home, app_dirs } => {
            let app_dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
            profiles.extend(chromium::discover(home, &app_dirs));
            profiles.extend(gecko::discover(home, &app_dirs));
            profiles.extend(safari::discover(home, &app_dirs));
        }
        Layout::Windows { .. } | Layout::Linux { .. } => {
            profiles.extend(chromium::discover_other(layout));
            profiles.extend(gecko::discover_other(layout));
        }
    }
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
/// macOS Keychain password from [`chromium_password`]; without it, encrypted Chromium cookies
/// count as unreadable. `now_unix` decides what has expired. For any OS, use
/// [`read_cookies_with_keys`].
pub fn read_cookies(
    profile: &Profile,
    chromium_password: Option<&[u8]>,
    now_unix: i64,
) -> Result<Cookies, Error> {
    match profile.family {
        Family::Chromium => {
            let keys = chromium_password
                .map(ChromiumKeys::from_macos_password)
                .unwrap_or_default();
            chromium::read_cookies(&profile.dir, &keys, now_unix).map(|cookies| cookies.read)
        }
        Family::Gecko => gecko::read_cookies(&profile.dir, now_unix),
        Family::Safari => safari::read_cookies(profile, now_unix),
    }
}

/// Reads a profile's cookies on any OS, skipping expired and partitioned ones. `keys` comes from
/// [`chromium_keys`] and is ignored for other browser families, which don't encrypt cookies;
/// without the right one, encrypted Chromium cookies count as unreadable.
pub fn read_cookies_with_keys(
    profile: &Profile,
    keys: &ChromiumKeys,
    now_unix: i64,
) -> Result<ChromiumCookies, Error> {
    match profile.family {
        Family::Chromium => chromium::read_cookies(&profile.dir, keys, now_unix),
        Family::Gecko | Family::Safari => Ok(ChromiumCookies {
            read: read_cookies(profile, None, now_unix)?,
            app_bound_domains: Vec::new(),
        }),
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

/// Reads a profile's bookmarks in the order the browser shows them, http and https pages only.
pub fn read_bookmarks(profile: &Profile) -> Result<Bookmarks, Error> {
    match profile.family {
        Family::Chromium => chromium::read_bookmarks(&profile.dir),
        Family::Gecko => gecko::read_bookmarks(&profile.dir),
        Family::Safari => safari::read_bookmarks(&profile.dir),
    }
}

/// Fetches a Chromium browser's cookie encryption password ("<Browser> Safe Storage") from the
/// login Keychain. macOS asks the user to allow it; a refusal is [`KeychainError::Denied`].
#[cfg(target_os = "macos")]
pub fn chromium_password(service: &str, account: Option<&str>) -> Result<Vec<u8>, Error> {
    if service == OFF_LIMITS_KEY {
        return Err(Error::Keychain(KeychainError::NotFound));
    }
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
    if service == OFF_LIMITS_KEY {
        return Ok(false);
    }
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

/// The one secret this crate never asks for, even to see whether it exists: Brigadier's own engine
/// is Chromium too, and this item isn't another browser's to give. A Chromium-branded source
/// therefore brings no cookies or passwords where the item is what locks them (macOS and Linux).
pub const OFF_LIMITS_KEY: &str = "Chromium Safe Storage";

/// Whether `profile`'s cookies and passwords are locked with [`OFF_LIMITS_KEY`].
pub fn key_off_limits(profile: &Profile) -> bool {
    profile.keychain_service.as_deref() == Some(OFF_LIMITS_KEY)
        || (cfg!(target_os = "linux") && profile.linux_secret_application.as_deref() == Some("chromium"))
}

/// Whether a profile has nothing an import could bring: no cookies, visits, bookmarks or saved
/// passwords. Reads no secret. A store that can't be read counts as having something, so a problem
/// shows when importing rather than as an empty profile.
pub fn profile_is_empty(profile: &Profile) -> bool {
    let cookies = !profile.has_cookies || matches!(count_cookies(profile), Ok(0));
    let history = !profile.has_history || read_history(profile, 1).is_ok_and(|visits| visits.is_empty());
    let bookmarks = !profile.has_bookmarks
        || read_bookmarks(profile).is_ok_and(|found| found.bookmarks.is_empty());
    cookies && history && bookmarks && !passwords::has_saved(profile)
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
/// path still names the bundle [`running`] looks for. A bundle may be a `/`-separated path.
fn app_path(app_dirs: &[&Path], bundles: &[&str]) -> Option<PathBuf> {
    for bundle in bundles {
        for dir in app_dirs {
            let path = layout::join_rel(dir, bundle);
            if path.exists() {
                return Some(path);
            }
        }
    }
    let (dir, bundle) = (app_dirs.first()?, bundles.first()?);
    Some(layout::join_rel(dir, bundle))
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

    fn profile(dir: &std::path::Path, family: Family, service: Option<&str>) -> Profile {
        Profile {
            id: "made/up".to_owned(),
            browser: "Made up".to_owned(),
            name: "Made up".to_owned(),
            family,
            dir: dir.to_owned(),
            app_path: None,
            keychain_service: service.map(str::to_owned),
            keychain_account: None,
            local_state: None,
            linux_secret_application: None,
            has_cookies: false,
            has_history: false,
            has_bookmarks: false,
            readable: true,
            is_default: true,
        }
    }

    #[test]
    fn the_off_limits_key_is_named_and_never_read() {
        let dir = tempfile::tempdir().unwrap();
        assert!(key_off_limits(&profile(dir.path(), Family::Chromium, Some(OFF_LIMITS_KEY))));
        assert!(!key_off_limits(&profile(dir.path(), Family::Chromium, Some("Made Up Safe Storage"))));
        #[cfg(target_os = "macos")]
        {
            // Both answer without a Keychain call, so no prompt can follow.
            assert!(matches!(chromium_password_exists(OFF_LIMITS_KEY, None), Ok(false)));
            assert!(matches!(
                chromium_password(OFF_LIMITS_KEY, None),
                Err(Error::Keychain(KeychainError::NotFound))
            ));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn chromium_s_linux_keyring_entry_is_off_limits() {
        let dir = tempfile::tempdir().unwrap();
        let mut branded = profile(dir.path(), Family::Chromium, None);
        branded.linux_secret_application = Some("chromium".to_owned());
        assert!(key_off_limits(&branded));
    }

    #[test]
    fn a_profile_with_no_stores_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(profile_is_empty(&profile(dir.path(), Family::Chromium, None)));
    }

    #[test]
    fn a_store_that_cannot_be_read_is_not_called_empty() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Bookmarks"), "not json").unwrap();
        let mut broken = profile(dir.path(), Family::Chromium, None);
        broken.has_bookmarks = true;
        assert!(!profile_is_empty(&broken));
    }

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
