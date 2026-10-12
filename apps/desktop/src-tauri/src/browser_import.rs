//! Settings › Import Browser Data: lists the browsers installed on this Mac and copies the
//! cookies (and, if asked, the history) of one or more of a browser's profiles into Brigadier's
//! browser (macOS). `brigadier-browser-data` reads the other browser's files, off the main
//! thread and never writing them; cookies go into the browser's one WebKit data store
//! (`browser_ui::set_cookies`), history into its own store as the user's ("Other"). What each
//! import brought is kept per browser in that store, for the browser's row.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_browser_data::{Cookie, Cookies, Error, Family, KeychainError, Profile, Source};
use brigadier_ipc::app::{
    ImportFailure, ImportFamily, ImportOutcome, ImportProgress, ImportRecord, ImportSource,
    ImportSourceProfile, ImportState,
};
use brigadier_ipc::protocol::{ErrorCode, IpcError};
use tauri::ipc::Channel;

/// The most visits one profile's import brings over, newest first.
const HISTORY_LIMIT: usize = 50_000;
/// The most skipped domains a record keeps; the row names three.
const SKIPPED_DOMAINS_KEPT: usize = 20;

fn refused(message: impl Into<String>) -> IpcError {
    IpcError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
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

/// The Keychain entry a Chromium browser locks its cookies with.
fn keychain_entry(source: &Source) -> Option<(&str, Option<&str>)> {
    let profile = source.profiles.first()?;
    (source.family == Family::Chromium).then_some(())?;
    Some((
        profile.keychain_service.as_deref()?,
        profile.keychain_account.as_deref(),
    ))
}

/// Where a browser stands, from what can be learnt without asking the user anything.
fn state(source: &Source, keychain_denied: bool) -> ImportState {
    if running(source) {
        return ImportState::Running;
    }
    if source.profiles.iter().any(|profile| !profile.readable) {
        return ImportState::NeedsFullDiskAccess;
    }
    if let Some((service, account)) = keychain_entry(source) {
        match password_exists(service, account) {
            Ok(true) => {}
            Ok(false) => return ImportState::NoKeychainKey,
            Err(error) => {
                tracing::debug!(%error, browser = %source.name, "Keychain lookup failed");
                return ImportState::Unavailable;
            }
        }
    }
    if keychain_denied {
        return ImportState::NeedsKeychainAccess;
    }
    ImportState::Ready
}

#[cfg(target_os = "macos")]
fn password_exists(service: &str, account: Option<&str>) -> Result<bool, Error> {
    brigadier_browser_data::chromium_password_exists(service, account)
}

#[cfg(not(target_os = "macos"))]
fn password_exists(_service: &str, _account: Option<&str>) -> Result<bool, Error> {
    Ok(false)
}

#[cfg(target_os = "macos")]
fn chromium_password(service: &str, account: Option<&str>) -> Result<Vec<u8>, Error> {
    brigadier_browser_data::chromium_password(service, account)
}

#[cfg(not(target_os = "macos"))]
fn chromium_password(_service: &str, _account: Option<&str>) -> Result<Vec<u8>, Error> {
    Err(Error::Keychain(KeychainError::NotFound))
}

/// A ready browser's profiles that keep cookies, with their counts. A profile macOS won't let
/// Brigadier count turns the browser into one that needs Full Disk Access.
fn counted(source: &Source) -> Result<Vec<ImportSourceProfile>, ImportState> {
    let mut profiles = Vec::new();
    for profile in source.profiles.iter().filter(|profile| profile.has_cookies) {
        let cookies = match brigadier_browser_data::count_cookies(profile) {
            Ok(n) => Some(count(n)),
            Err(Error::PermissionDenied(_)) => return Err(ImportState::NeedsFullDiskAccess),
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
    Ok(profiles)
}

/// The installed browsers, in Settings' order, each with where it stands, its profiles' cookie
/// counts, its icon and what the last import from it brought.
#[tauri::command]
pub async fn browser_import_sources(app: tauri::AppHandle) -> Vec<ImportSource> {
    if !cfg!(target_os = "macos") {
        return Vec::new();
    }
    let Some(home) = home() else {
        return Vec::new();
    };
    let listed = tauri::async_runtime::spawn_blocking(move || {
        let records = crate::browser_store::with(|store| store.imports()).unwrap_or_default();
        brigadier_browser_data::sources(&home)
            .into_iter()
            .map(|source| {
                let (record, denied) = records.get(&source.key).cloned().unwrap_or_default();
                let mut state = state(&source, denied);
                let mut profiles = Vec::new();
                // A refused Keychain key is asked for again by importing, so it needs the
                // profiles too.
                if matches!(state, ImportState::Ready | ImportState::NeedsKeychainAccess) {
                    match counted(&source) {
                        Ok(found) => profiles = found,
                        Err(blocked) => state = blocked,
                    }
                }
                let item = ImportSource {
                    key: source.key,
                    name: source.name,
                    family: family(source.family),
                    icon: None,
                    state,
                    profiles,
                    record,
                };
                (item, source.app_path)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    let mut sources = Vec::with_capacity(listed.len());
    for (mut source, app_path) in listed {
        source.icon = icon(&app, app_path).await;
        sources.push(source);
    }
    sources
}

/// App icons already read, by bundle path.
static ICONS: Mutex<Option<HashMap<PathBuf, Option<String>>>> = Mutex::new(None);

/// A browser's app icon as a `data:image/png` URL, read once per run.
async fn icon(app: &tauri::AppHandle, bundle: PathBuf) -> Option<String> {
    let cached = ICONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_default()
        .get(&bundle)
        .cloned();
    if let Some(icon) = cached {
        return icon;
    }
    let (sent, answer) = tokio::sync::oneshot::channel();
    let path = bundle.clone();
    let asked = app.run_on_main_thread(move || {
        let _ = sent.send(app_icon_png(&path).map(|png| data_url(&png)));
    });
    let icon = match asked {
        Ok(()) => answer.await.ok().flatten(),
        Err(_) => None,
    };
    ICONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_or_insert_default()
        .insert(bundle, icon.clone());
    icon
}

/// The icon Finder shows for the app at `bundle`, as a PNG about 64 pixels wide (so it stays
/// sharp at 32 points on a Retina screen). Main thread only. One AppKit call needs `unsafe`
/// (the workspace denies it) only for its untyped properties dictionary, passed empty.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn app_icon_png(bundle: &Path) -> Option<Vec<u8>> {
    use objc2::rc::Retained;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSDictionary, NSString};

    const WANTED: isize = 64;
    let path = NSString::from_str(&bundle.to_string_lossy());
    let image = NSWorkspace::sharedWorkspace().iconForFile(&path);
    let tiff = image.TIFFRepresentation()?;
    let reps = NSBitmapImageRep::imageRepsWithData(&tiff);
    // The smallest at least as wide as wanted, else the widest there is.
    let rep = reps
        .iter()
        .filter_map(|rep| rep.downcast::<NSBitmapImageRep>().ok())
        .min_by_key(|rep| {
            let wide = rep.pixelsWide();
            if wide >= WANTED {
                (0, wide)
            } else {
                (1, -wide)
            }
        })?;
    let rep: Retained<NSBitmapImageRep> = rep;
    // SAFETY: an empty properties dictionary is of the type the method takes.
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(png.to_vec())
}

#[cfg(not(target_os = "macos"))]
fn app_icon_png(_bundle: &Path) -> Option<Vec<u8>> {
    None
}

/// `png` as a `data:` URL.
fn data_url(png: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(22 + png.len().div_ceil(3) * 4);
    out.push_str("data:image/png;base64,");
    for chunk in png.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        for (index, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if index <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> shift) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A refusal before anything was copied.
fn failed(reason: ImportFailure, message: impl Into<String>) -> ImportOutcome {
    ImportOutcome::Failed {
        reason,
        message: message.into(),
    }
}

/// Why `browser`'s cookies couldn't be read, as the page's reason and in plain words.
fn cookie_failure(error: &Error, browser: &str) -> (ImportFailure, String) {
    match error {
        Error::Keychain(KeychainError::Denied) => (
            ImportFailure::NeedsKeychainAccess,
            format!("macOS didn't let Brigadier use {browser}'s cookie key."),
        ),
        Error::Keychain(KeychainError::NotFound) => (
            ImportFailure::NoKeychainKey,
            format!("{browser}'s cookie key isn't in your Keychain."),
        ),
        Error::PermissionDenied(_) => (
            ImportFailure::NeedsFullDiskAccess,
            format!("Brigadier needs Full Disk Access to read {browser}'s cookies."),
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

/// The cookies of several profiles as one import: a later profile's cookie wins over an
/// earlier one with the same name, domain and path, as a later import would.
#[derive(Debug, Default)]
struct Merged {
    cookies: Vec<Cookie>,
    /// Left out before writing: partitioned or unreadable. Expired cookies aren't counted:
    /// they are gone from the other browser too.
    skipped: usize,
    /// The domains of the skipped ones, one per cookie.
    domains: Vec<String>,
}

fn merge(reads: Vec<Cookies>) -> Merged {
    let mut merged = Merged::default();
    let mut at: HashMap<(String, String, String), usize> = HashMap::new();
    for read in reads {
        merged.skipped += read.partitioned + read.unreadable;
        merged.domains.extend(read.partitioned_domains);
        merged.domains.extend(read.unreadable_domains);
        for cookie in read.cookies {
            let key = (
                cookie.name.clone(),
                cookie.domain.clone(),
                cookie.path.clone(),
            );
            match at.get(&key) {
                Some(&index) => merged.cookies[index] = cookie,
                None => {
                    at.insert(key, merged.cookies.len());
                    merged.cookies.push(cookie);
                }
            }
        }
    }
    merged
}

/// The distinct domains among `domains` (without a leading dot), the first
/// [`SKIPPED_DOMAINS_KEPT`] of them and how many there were.
fn skipped_domains<'a>(domains: impl IntoIterator<Item = &'a str>) -> (Vec<String>, u32) {
    let mut seen = HashSet::new();
    let mut kept = Vec::new();
    for domain in domains {
        let domain = domain.trim_start_matches('.').to_ascii_lowercase();
        if domain.is_empty() || !seen.insert(domain.clone()) {
            continue;
        }
        if kept.len() < SKIPPED_DOMAINS_KEPT {
            kept.push(domain);
        }
    }
    (kept, count(seen.len()))
}

/// What was read for an import, ready to write.
struct Read {
    key: String,
    profiles: Vec<String>,
    merged: Merged,
    history: Option<Result<Vec<brigadier_browser_data::Visit>, String>>,
}

/// Reads the chosen profiles of the browser `key`, or says why they can't be.
fn read(home: &Path, key: &str, ids: &[String], history: bool) -> Result<Read, ImportOutcome> {
    let gone = || {
        failed(
            ImportFailure::Gone,
            "That browser or profile is no longer here.",
        )
    };
    let source = brigadier_browser_data::sources(home)
        .into_iter()
        .find(|source| source.key == key)
        .ok_or_else(gone)?;
    let chosen: Vec<&Profile> = ids
        .iter()
        .map(|id| source.profiles.iter().find(|profile| &profile.id == id))
        .collect::<Option<_>>()
        .ok_or_else(gone)?;
    if chosen.is_empty() {
        return Err(gone());
    }
    let browser = &source.name;
    if running(&source) {
        return Err(failed(
            ImportFailure::Running,
            format!("Quit {browser} first, then import again."),
        ));
    }
    let password = match keychain_entry(&source) {
        Some((service, account)) => match chromium_password(service, account) {
            Ok(password) => Some(password),
            Err(error) => {
                if matches!(error, Error::Keychain(KeychainError::Denied)) {
                    crate::browser_store::with(|store| store.import_denied(&source.key));
                }
                let (reason, message) = cookie_failure(&error, browser);
                return Err(failed(reason, message));
            }
        },
        None => None,
    };
    let now = crate::browser_store::now_ms() / 1000;
    let mut reads = Vec::new();
    for profile in &chosen {
        match brigadier_browser_data::read_cookies(profile, password.as_deref(), now) {
            Ok(read) => reads.push(read),
            // A profile without a cookie file just brings none.
            Err(Error::NotFound(_)) => {}
            Err(error) => {
                let (reason, message) = cookie_failure(&error, browser);
                return Err(failed(reason, message));
            }
        }
    }
    let history = history.then(|| {
        let mut visits = Vec::new();
        for profile in chosen.iter().filter(|profile| profile.has_history) {
            match brigadier_browser_data::read_history(profile, HISTORY_LIMIT) {
                Ok(found) => visits.extend(found),
                Err(Error::PermissionDenied(_)) => {
                    return Err(format!(
                        "Brigadier needs Full Disk Access to read {browser}'s history."
                    ));
                }
                Err(error) => {
                    return Err(format!("{browser}'s history couldn't be read ({error})."));
                }
            }
        }
        Ok(visits)
    });
    Ok(Read {
        key: source.key.clone(),
        profiles: chosen.iter().map(|profile| profile.name.clone()).collect(),
        merged: merge(reads),
        history,
    })
}

/// Copies the cookies of the profiles `ids` of the browser `key` into Brigadier's browser, and
/// their history when `history` is set, telling `progress` how it goes. Refused while that
/// browser is open (its files may be half written). What it brought is kept for the browser's
/// row.
#[tauri::command]
pub async fn browser_import(
    app: tauri::AppHandle,
    key: String,
    ids: Vec<String>,
    history: bool,
    progress: Channel<ImportProgress>,
) -> Result<ImportOutcome, IpcError> {
    crate::browser::check_enabled()?;
    let home = home().ok_or_else(|| refused("no home folder"))?;
    let _ = progress.send(ImportProgress::Reading);
    let read = tauri::async_runtime::spawn_blocking(move || read(&home, &key, &ids, history))
        .await
        .map_err(|err| refused(err.to_string()))?;
    let read = match read {
        Ok(read) => read,
        Err(outcome) => return Ok(outcome),
    };
    let Merged {
        cookies,
        skipped,
        mut domains,
    } = read.merged;
    let total = cookies.len();
    let stored = add_cookies(&app, cookies.clone(), &progress).await?;
    let mut imported = 0;
    for (cookie, stored) in cookies
        .iter()
        .zip(stored.iter().chain(std::iter::repeat(&false)))
    {
        if *stored {
            imported += 1;
        } else {
            domains.push(cookie.domain.clone());
        }
    }
    let (skipped_domains, skipped_domain_count) =
        skipped_domains(domains.iter().map(String::as_str));
    let mut history_error = None;
    let mut history_added = 0;
    if let Some(found) = read.history {
        let _ = progress.send(ImportProgress::History);
        match found {
            Ok(visits) => {
                let added = crate::browser_store::with(|store| {
                    store.import_visits(
                        visits
                            .iter()
                            .map(|v| (v.url.as_str(), v.title.as_str(), v.visited_at_ms)),
                    )
                });
                match added {
                    Some(added) => history_added = added,
                    None => {
                        history_error =
                            Some("Brigadier's browsing history can't be changed now.".to_owned());
                    }
                }
            }
            Err(error) => history_error = Some(error),
        }
    }
    let record = ImportRecord {
        at_ms: crate::browser_store::now_ms(),
        profiles: read.profiles,
        imported: count(imported),
        skipped: count(skipped + (total - imported)),
        skipped_domains,
        skipped_domain_count,
        history: count(history_added),
    };
    crate::browser_store::with(|store| store.save_import(&read.key, &record));
    Ok(ImportOutcome::Done {
        record,
        history_error,
    })
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

/// Adds `cookies` to the browser's WebKit store and says, per cookie, whether the store took
/// it: each set call's completion is waited for and the store read back, which is also when
/// WebKit has them for good (it has no separate flush).
#[cfg(target_os = "macos")]
async fn add_cookies(
    app: &tauri::AppHandle,
    cookies: Vec<Cookie>,
    progress: &Channel<ImportProgress>,
) -> Result<Vec<bool>, IpcError> {
    let total = count(cookies.len());
    let _ = progress.send(ImportProgress::Cookies { done: 0, total });
    let channel = progress.clone();
    crate::browser::on_store(app, move |identifier, done| {
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
    .await
}

#[cfg(not(target_os = "macos"))]
async fn add_cookies(
    _app: &tauri::AppHandle,
    _cookies: Vec<Cookie>,
    _progress: &Channel<ImportProgress>,
) -> Result<Vec<bool>, IpcError> {
    Err(refused("importing cookies works on macOS only"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, domain: &str, value: &str) -> Cookie {
        Cookie {
            name: name.into(),
            value: value.into(),
            domain: domain.into(),
            host_only: !domain.starts_with('.'),
            path: "/".into(),
            expires_unix: None,
            secure: true,
            http_only: false,
            same_site: None,
        }
    }

    #[test]
    fn several_profiles_merge_into_one_import_the_later_one_winning() {
        let work = Cookies {
            cookies: vec![
                cookie("sid", ".a.com", "work"),
                cookie("only-work", "b.com", "1"),
            ],
            expired: 2,
            partitioned: 1,
            unreadable: 1,
            unreadable_domains: vec![".x.com".into()],
            partitioned_domains: vec!["p.com".into()],
        };
        let personal = Cookies {
            cookies: vec![
                cookie("sid", ".a.com", "personal"),
                cookie("only-personal", "c.com", "2"),
            ],
            unreadable: 1,
            unreadable_domains: vec!["y.com".into()],
            ..Cookies::default()
        };
        let merged = merge(vec![work, personal]);
        assert_eq!(
            merged
                .cookies
                .iter()
                .map(|c| (c.name.as_str(), c.value.as_str()))
                .collect::<Vec<_>>(),
            [
                ("sid", "personal"),
                ("only-work", "1"),
                ("only-personal", "2")
            ]
        );
        assert_eq!(merged.skipped, 3);
        assert_eq!(merged.domains, ["p.com", ".x.com", "y.com"]);
    }

    #[test]
    fn skipped_domains_are_distinct_and_capped_with_their_full_count() {
        let many: Vec<String> = (0..30)
            .flat_map(|n| [format!(".site{n}.com"), format!("SITE{n}.com")])
            .collect();
        let (kept, total) = skipped_domains(many.iter().map(String::as_str));
        assert_eq!(total, 30);
        assert_eq!(kept.len(), 20);
        assert_eq!(kept[0], "site0.com");
        assert_eq!(kept[19], "site19.com");
        let (kept, total) = skipped_domains(["", ".a.com", "a.com"]);
        assert_eq!((kept, total), (vec!["a.com".to_owned()], 1));
    }

    #[test]
    fn why_cookies_cant_be_read_decides_what_the_page_shows() {
        let path = PathBuf::from("/x");
        assert_eq!(
            cookie_failure(&Error::Keychain(KeychainError::Denied), "Dia").0,
            ImportFailure::NeedsKeychainAccess
        );
        assert_eq!(
            cookie_failure(&Error::Keychain(KeychainError::NotFound), "Dia").0,
            ImportFailure::NoKeychainKey
        );
        assert_eq!(
            cookie_failure(&Error::PermissionDenied(path.clone()), "Safari"),
            (
                ImportFailure::NeedsFullDiskAccess,
                "Brigadier needs Full Disk Access to read Safari's cookies.".to_owned()
            )
        );
        assert_eq!(
            cookie_failure(&Error::Locked(path), "Firefox").0,
            ImportFailure::Running
        );
    }

    #[test]
    fn a_browser_that_isnt_here_is_gone() {
        let empty = std::env::temp_dir().join("brigadier-import-test-empty-home");
        let Err(ImportOutcome::Failed { reason, .. }) =
            read(&empty, "firefox", &["firefox/none".to_owned()], true)
        else {
            panic!("a browser that isn't there can't be read");
        };
        assert_eq!(reason, ImportFailure::Gone);
    }

    #[test]
    fn png_data_urls_are_base64() {
        assert_eq!(data_url(b""), "data:image/png;base64,");
        assert_eq!(data_url(b"f"), "data:image/png;base64,Zg==");
        assert_eq!(data_url(b"fo"), "data:image/png;base64,Zm8=");
        assert_eq!(data_url(b"foobar"), "data:image/png;base64,Zm9vYmFy");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_app_icon_reads_as_a_png() {
        let png = app_icon_png(Path::new("/System/Applications/Calculator.app"))
            .expect("Calculator has an icon");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }
}
