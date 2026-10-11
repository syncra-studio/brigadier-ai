//! Chromium-family browsers: Chrome, Edge, Brave, Arc, Dia, Vivaldi, Opera and unknown builds.
//!
//! Formats follow Chromium's sources: `Local State` lists profiles under `profile.info_cache`;
//! `Cookies` is a SQLite store whose values are encrypted with a key derived from the
//! browser's "Safe Storage" Keychain password (components/os_crypt/sync/os_crypt_mac.mm); and
//! since cookie store version 24 the plaintext starts with the SHA-256 of the cookie's domain
//! (net/extras/sqlite/sqlite_persistent_cookie_store.cc).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::sqlite::{columns, open_copy};
use crate::{
    Cookie, Cookies, Error, Family, Profile, SameSite, Visit, WEBKIT_EPOCH_OFFSET_SECS, app_path,
    existing_app, is_web_url, key_for, opens_web_links, probe,
};

/// PBKDF2 salt Chromium uses on macOS.
const SALT: &[u8] = b"saltysalt";
/// PBKDF2 iterations Chromium uses on macOS.
const ITERATIONS: u32 = 1003;
/// AES-CBC IV Chromium uses on macOS: sixteen spaces.
const IV: [u8; 16] = [b' '; 16];
/// Cookie store version from which plaintexts carry a SHA-256 of the domain.
const DOMAIN_HASH_VERSION: i64 = 24;

/// A Chromium browser whose folder, name and Keychain entry are known.
struct Known {
    /// Folder under `~/Library/Application Support` holding `Local State`.
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    service: &'static str,
    account: &'static str,
    bundle: &'static str,
}

/// Every channel of a product shares its Keychain entry: Chromium names it after the brand,
/// not the channel.
const KNOWN: &[Known] = &[
    known(
        "Google/Chrome",
        "Google Chrome",
        "chrome",
        "Chrome",
        "Google Chrome.app",
    ),
    known(
        "Google/Chrome Beta",
        "Google Chrome Beta",
        "chrome-beta",
        "Chrome",
        "Google Chrome Beta.app",
    ),
    known(
        "Google/Chrome Dev",
        "Google Chrome Dev",
        "chrome-dev",
        "Chrome",
        "Google Chrome Dev.app",
    ),
    known(
        "Google/Chrome Canary",
        "Google Chrome Canary",
        "chrome-canary",
        "Chrome",
        "Google Chrome Canary.app",
    ),
    known(
        "Chromium",
        "Chromium",
        "chromium",
        "Chromium",
        "Chromium.app",
    ),
    known(
        "net.imput.helium",
        "Helium",
        "helium",
        "Helium",
        "Helium.app",
    ),
    known("Arc/User Data", "Arc", "arc", "Arc", "Arc.app"),
    known("Dia/User Data", "Dia", "dia", "Dia", "Dia.app"),
    known(
        "BraveSoftware/Brave-Browser",
        "Brave",
        "brave",
        "Brave",
        "Brave Browser.app",
    ),
    known(
        "BraveSoftware/Brave-Browser-Beta",
        "Brave Beta",
        "brave-beta",
        "Brave",
        "Brave Browser Beta.app",
    ),
    known(
        "BraveSoftware/Brave-Browser-Nightly",
        "Brave Nightly",
        "brave-nightly",
        "Brave",
        "Brave Browser Nightly.app",
    ),
    known(
        "Microsoft Edge",
        "Microsoft Edge",
        "edge",
        "Microsoft Edge",
        "Microsoft Edge.app",
    ),
    known(
        "Microsoft Edge Beta",
        "Microsoft Edge Beta",
        "edge-beta",
        "Microsoft Edge",
        "Microsoft Edge Beta.app",
    ),
    known(
        "Microsoft Edge Dev",
        "Microsoft Edge Dev",
        "edge-dev",
        "Microsoft Edge",
        "Microsoft Edge Dev.app",
    ),
    known(
        "Microsoft Edge Canary",
        "Microsoft Edge Canary",
        "edge-canary",
        "Microsoft Edge",
        "Microsoft Edge Canary.app",
    ),
    known("Vivaldi", "Vivaldi", "vivaldi", "Vivaldi", "Vivaldi.app"),
    known(
        "com.operasoftware.Opera",
        "Opera",
        "opera",
        "Opera",
        "Opera.app",
    ),
    // A best guess: a Chromium-based browser whose layout isn't confirmed. The generic scan
    // below finds it too if it keeps its data elsewhere.
    known("Thor", "Thor", "thor", "Thor", "Thor.app"),
];

const fn known(
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    account: &'static str,
    bundle: &'static str,
) -> Known {
    let service = match account.as_bytes() {
        b"Chrome" => "Chrome Safe Storage",
        b"Chromium" => "Chromium Safe Storage",
        b"Arc" => "Arc Safe Storage",
        b"Dia" => "Dia Safe Storage",
        b"Brave" => "Brave Safe Storage",
        b"Microsoft Edge" => "Microsoft Edge Safe Storage",
        b"Vivaldi" => "Vivaldi Safe Storage",
        b"Opera" => "Opera Safe Storage",
        b"Thor" => "Thor Safe Storage",
        // Helium names its entry differently from the other Chromium builds.
        b"Helium" => "Helium Storage Key",
        _ => panic!("add the Keychain service for this account"),
    };
    Known {
        folder,
        browser,
        key,
        service,
        account,
        bundle,
    }
}

/// Folders never treated as a browser even if they look like one.
const SKIP: &[&str] = &["Google/Chrome for Testing"];

/// One browser install: its data root and how to label its profiles.
struct Root {
    dir: PathBuf,
    browser: String,
    key: String,
    service: String,
    account: String,
    app_path: Option<PathBuf>,
}

pub(crate) fn discover(home: &Path, app_dirs: &[&Path]) -> Vec<Profile> {
    let support = home.join("Library/Application Support");
    let mut handled: HashSet<String> = SKIP
        .iter()
        .map(|folder| support.join(folder).to_string_lossy().to_lowercase())
        .collect();
    let mut profiles = Vec::new();

    for known in KNOWN {
        let dir = support.join(known.folder);
        // APFS is usually case-insensitive, so compare lowercased paths when deduplicating.
        if !handled.insert(dir.to_string_lossy().to_lowercase()) || !dir.is_dir() {
            continue;
        }
        let root = Root {
            dir,
            browser: known.browser.to_owned(),
            key: known.key.to_owned(),
            service: known.service.to_owned(),
            account: known.account.to_owned(),
            app_path: app_path(app_dirs, &[known.bundle]),
        };
        profiles.extend(root_profiles(&root));
    }

    // Unknown Chromium builds: `<X>` or `<X>/User Data` with a `Local State` and a profile
    // that has a History file, whose `<X>.app` is installed and opens web links. Apps built on
    // Chromium (Electron) keep a `Local State`, `Cookies` and sometimes a History too, but
    // don't open web links; neither does a folder whose app is gone.
    let Ok(entries) = std::fs::read_dir(&support) else {
        return profiles;
    };
    let mut folders: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    folders.sort();
    for folder in folders {
        for dir in [
            support.join(&folder),
            support.join(&folder).join("User Data"),
        ] {
            if handled.contains(&dir.to_string_lossy().to_lowercase())
                || !dir.join("Local State").is_file()
            {
                continue;
            }
            let Some(app) =
                existing_app(app_dirs, &format!("{folder}.app")).filter(|app| opens_web_links(app))
            else {
                continue;
            };
            // Named for its folder and engine, and keyed apart from a known browser of the
            // same name.
            let mut key = key_for(&folder);
            if KNOWN.iter().any(|known| known.key == key) {
                key.push_str("-chromium");
            }
            let root = Root {
                dir: dir.clone(),
                browser: format!("{folder} (Chromium)"),
                key,
                service: format!("{folder} Safe Storage"),
                account: folder.clone(),
                app_path: Some(app),
            };
            let found = root_profiles(&root);
            if found.iter().any(|profile| profile.has_history) {
                handled.insert(dir.to_string_lossy().to_lowercase());
                profiles.extend(found);
            }
        }
    }
    profiles
}

/// The profiles under one browser root that have cookies or history.
fn root_profiles(root: &Root) -> Vec<Profile> {
    let state = std::fs::read(root.dir.join("Local State"))
        .ok()
        .and_then(|bytes| {
            serde_json::from_slice::<serde_json::Value>(&bytes)
                .inspect_err(|err| {
                    tracing::debug!(dir = %root.dir.display(), %err, "unreadable Local State");
                })
                .ok()
        });
    let mut names = BTreeMap::new();
    let mut last_used = None;
    if let Some(profile) = state.as_ref().and_then(|state| state.get("profile")) {
        if let Some(cache) = profile
            .get("info_cache")
            .and_then(|cache| cache.as_object())
        {
            for (dir, info) in cache {
                let name = info
                    .get("name")
                    .and_then(|name| name.as_str())
                    .unwrap_or(dir);
                names.insert(dir.clone(), name.to_owned());
            }
        }
        last_used = profile
            .get("last_used")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
    }

    // Profile folders: everything Local State names, plus `Default` and `Profile N` in case it
    // is missing or stale.
    let mut dirs: Vec<String> = names.keys().cloned().collect();
    dirs.push("Default".to_owned());
    if let Ok(entries) = std::fs::read_dir(&root.dir) {
        for entry in entries.flatten() {
            if let Ok(name) = entry.file_name().into_string()
                && name
                    .strip_prefix("Profile ")
                    .is_some_and(|n| n.parse::<u32>().is_ok())
            {
                dirs.push(name);
            }
        }
    }
    dirs.sort();
    dirs.dedup();

    let mut profiles = Vec::new();
    for dir_name in &dirs {
        let dir = root.dir.join(dir_name);
        let is_default = match &last_used {
            Some(last) => last == dir_name,
            None => dir_name == "Default",
        };
        let name = names
            .get(dir_name)
            .cloned()
            .unwrap_or_else(|| dir_name.clone());
        if let Some(profile) = profile(root, dir, dir_name, name, is_default) {
            profiles.push(profile);
        }
    }
    // Opera keeps its single profile in the root itself.
    if profiles.is_empty()
        && let Some(profile) = profile(
            root,
            root.dir.clone(),
            "Default",
            "Default".to_owned(),
            true,
        )
    {
        profiles.push(profile);
    }
    profiles
}

fn profile(
    root: &Root,
    dir: PathBuf,
    dir_name: &str,
    name: String,
    is_default: bool,
) -> Option<Profile> {
    let (has_cookies, cookies_readable) = probe(&cookies_path(&dir));
    let (has_history, history_readable) = probe(&dir.join("History"));
    if !has_cookies && !has_history {
        return None;
    }
    Some(Profile {
        id: format!("{}/{dir_name}", root.key),
        browser: root.browser.clone(),
        name,
        family: Family::Chromium,
        app_path: root.app_path.clone(),
        keychain_service: Some(root.service.clone()),
        keychain_account: Some(root.account.clone()),
        has_cookies,
        has_history,
        readable: (!has_cookies || cookies_readable) && (!has_history || history_readable),
        is_default,
        dir,
    })
}

/// Newer Chromium keeps cookies in `Network/Cookies`; older builds in the profile root.
fn cookies_path(dir: &Path) -> PathBuf {
    let network = dir.join("Network/Cookies");
    if network.exists() {
        network
    } else {
        dir.join("Cookies")
    }
}

/// The AES-128 key Chromium derives from the Keychain password.
pub(crate) fn derive_key(password: &[u8]) -> [u8; 16] {
    pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(password, SALT, ITERATIONS)
}

/// Decrypts a `v10`/`v11` value, or `None` when it isn't one or the padding is wrong (which is
/// what a wrong password looks like).
fn decrypt(key: &[u8; 16], encrypted: &[u8]) -> Option<Vec<u8>> {
    let ciphertext = encrypted
        .strip_prefix(b"v10")
        .or_else(|| encrypted.strip_prefix(b"v11"))?;
    let decryptor = cbc::Decryptor::<aes::Aes128>::new(key.into(), &IV.into());
    decryptor.decrypt_padded_vec::<Pkcs7>(ciphertext).ok()
}

/// The plaintext value, checking and stripping the domain hash when the store has one.
fn cookie_value(
    plain: String,
    encrypted: &[u8],
    host: &str,
    key: Option<&[u8; 16]>,
    version: i64,
) -> Option<String> {
    if !plain.is_empty() || encrypted.is_empty() {
        return Some(plain);
    }
    let mut bytes = decrypt(key?, encrypted)?;
    if version >= DOMAIN_HASH_VERSION {
        let hash = Sha256::digest(host.as_bytes());
        if bytes.len() < hash.len() || bytes[..hash.len()] != hash[..] {
            return None;
        }
        bytes.drain(..hash.len());
    }
    String::from_utf8(bytes).ok()
}

pub(crate) fn read_cookies(
    dir: &Path,
    password: Option<&[u8]>,
    now_unix: i64,
) -> Result<Cookies, Error> {
    let conn = open_copy(&cookies_path(dir))?;
    let cols = columns(&conn, "cookies")?;
    if !cols.contains("host_key") || !cols.contains("name") {
        return Err(Error::Format("no Chromium cookies table".to_owned()));
    }
    let pick = |names: &[&str], fallback: &str| -> String {
        names
            .iter()
            .find(|name| cols.contains(**name))
            .map_or_else(|| fallback.to_owned(), |name| (*name).to_owned())
    };
    let sql = format!(
        "SELECT host_key, name, {value}, {encrypted}, {path}, {expires}, {secure}, {http_only},
                {same_site}, {persistent}, {top_frame} FROM cookies",
        value = pick(&["value"], "''"),
        encrypted = pick(&["encrypted_value"], "X''"),
        path = pick(&["path"], "'/'"),
        expires = pick(&["expires_utc"], "0"),
        secure = pick(&["is_secure", "secure"], "0"),
        http_only = pick(&["is_httponly", "httponly"], "0"),
        same_site = pick(&["samesite"], "-1"),
        persistent = pick(&["is_persistent", "has_expires"], "1"),
        top_frame = pick(&["top_frame_site_key"], "''"),
    );
    let version = meta_version(&conn);
    let key = password.map(derive_key);

    let mut out = Cookies::default();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let host: String = row.get(0)?;
        let expires_utc: i64 = row.get::<_, Option<i64>>(5)?.unwrap_or(0);
        let persistent: i64 = row.get::<_, Option<i64>>(9)?.unwrap_or(1);
        let expires_unix = (persistent != 0 && expires_utc != 0)
            .then(|| expires_utc.div_euclid(1_000_000) - WEBKIT_EPOCH_OFFSET_SECS);
        if expires_unix.is_some_and(|expires| expires < now_unix) {
            out.expired += 1;
            continue;
        }
        let top_frame: Option<String> = row.get(10)?;
        if top_frame.is_some_and(|key| !key.is_empty()) {
            out.partitioned += 1;
            continue;
        }
        let plain: Option<String> = row.get(2).ok().flatten();
        let encrypted: Option<Vec<u8>> = row.get(3)?;
        let Some(value) = cookie_value(
            plain.unwrap_or_default(),
            encrypted.as_deref().unwrap_or_default(),
            &host,
            key.as_ref(),
            version,
        ) else {
            out.unreadable += 1;
            out.unreadable_domains.push(host);
            continue;
        };
        let same_site = match row.get::<_, Option<i64>>(8)?.unwrap_or(-1) {
            0 => Some(SameSite::None),
            1 => Some(SameSite::Lax),
            2 => Some(SameSite::Strict),
            _ => None,
        };
        out.cookies.push(Cookie {
            name: row.get(1)?,
            value,
            host_only: !host.starts_with('.'),
            domain: host,
            path: row
                .get::<_, Option<String>>(4)?
                .unwrap_or_else(|| "/".to_owned()),
            expires_unix,
            secure: row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
            http_only: row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0,
            same_site,
        });
    }
    Ok(out)
}

/// How many cookies a profile's store holds, without decrypting any.
pub(crate) fn count_cookies(dir: &Path) -> Result<usize, Error> {
    let conn = open_copy(&cookies_path(dir))?;
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM cookies", [], |row| row.get(0))?;
    Ok(usize::try_from(count).unwrap_or(0))
}

/// The cookie store's schema version from its `meta` table, 0 when absent.
fn meta_version(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'version'",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .ok()
    .flatten()
    .unwrap_or(0)
}

pub(crate) fn read_history(dir: &Path, limit: usize) -> Result<Vec<Visit>, Error> {
    let conn = open_copy(&dir.join("History"))?;
    let mut stmt = conn.prepare(
        "SELECT u.url, COALESCE(u.title, ''), v.visit_time
         FROM visits v JOIN urls u ON u.id = v.url
         WHERE u.url LIKE 'http://%' OR u.url LIKE 'https://%'
         GROUP BY u.url, v.visit_time
         ORDER BY v.visit_time DESC, u.url
         LIMIT ?1",
    )?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt.query_map([limit], |row| {
        let micros: i64 = row.get(2)?;
        Ok(Visit {
            url: row.get(0)?,
            title: row.get(1)?,
            visited_at_ms: micros.div_euclid(1000) - WEBKIT_EPOCH_OFFSET_SECS * 1000,
        })
    })?;
    let mut visits = Vec::new();
    for visit in rows {
        let visit = visit?;
        if is_web_url(&visit.url) {
            visits.push(visit);
        }
    }
    Ok(visits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_browsers_have_their_keychain_entries() {
        let find = |key: &str| KNOWN.iter().find(|k| k.key == key).unwrap();
        assert_eq!(find("chrome").service, "Chrome Safe Storage");
        assert_eq!(find("chrome-canary").account, "Chrome");
        assert_eq!(find("edge").service, "Microsoft Edge Safe Storage");
        assert_eq!(find("brave").account, "Brave");
        assert_eq!(find("dia").service, "Dia Safe Storage");
        assert_eq!(find("arc").folder, "Arc/User Data");
    }

    #[test]
    fn non_v10_values_and_bad_padding_are_rejected() {
        let key = derive_key(b"x");
        assert!(decrypt(&key, b"plain").is_none());
        assert!(decrypt(&key, b"v10short").is_none());
        assert!(decrypt(&key, &[b"v10".as_slice(), &[0u8; 16]].concat()).is_none());
    }
}
