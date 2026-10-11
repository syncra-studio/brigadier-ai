//! Gecko-family browsers: Firefox and its forks (Zen, LibreWolf, Waterfox, Floorp).
//!
//! Profiles are listed in `profiles.ini` at the browser's root. Cookies live in
//! `cookies.sqlite` (`moz_cookies`, unencrypted) and history in `places.sqlite`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::sqlite::{columns, open_copy};
use crate::{
    Cookie, Cookies, Error, Family, Profile, SameSite, Visit, app_path, existing_app, is_web_url,
    key_for, probe,
};

/// A Gecko browser whose folder and app are known.
struct Known {
    /// Folder under `~/Library/Application Support` holding `profiles.ini`.
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    /// App bundles sharing this root, most common first.
    bundles: &'static [&'static str],
}

const KNOWN: &[Known] = &[
    Known {
        folder: "Firefox",
        browser: "Firefox",
        key: "firefox",
        // Developer Edition and Nightly share Firefox's profiles.ini.
        bundles: &[
            "Firefox.app",
            "Firefox Developer Edition.app",
            "Firefox Nightly.app",
        ],
    },
    // Zen keeps its root in a lowercase `zen` folder on macOS.
    Known {
        folder: "zen",
        browser: "Zen",
        key: "zen",
        bundles: &["Zen.app", "Zen Browser.app"],
    },
    Known {
        folder: "librewolf",
        browser: "LibreWolf",
        key: "librewolf",
        bundles: &["LibreWolf.app"],
    },
    Known {
        folder: "Waterfox",
        browser: "Waterfox",
        key: "waterfox",
        bundles: &["Waterfox.app"],
    },
    Known {
        folder: "Floorp",
        browser: "Floorp",
        key: "floorp",
        bundles: &["Floorp.app"],
    },
];

/// Firefox's cookie schema version whose migration multiplied `expiry` by 1000, turning it
/// from seconds into milliseconds.
///
/// Recalled from Firefox's netwerk/cookie/CookiePersistentStorage.cpp (the 15 → 16 migration,
/// shipped around Firefox 141), not verified against the source here, so treat it as likely
/// rather than certain. [`expiry_secs`] therefore decides by magnitude first and uses this
/// only for the narrow range where magnitude can't tell.
const EXPIRY_MS_SCHEMA: i64 = 16;
/// Any expiry at or above this is milliseconds: as seconds it would be past the year 33000,
/// and as milliseconds it is after 2001.
const SURELY_MS: i64 = 1_000_000_000_000;
/// Any expiry below this is seconds: as milliseconds it would be before 1973.
const SURELY_SECS: i64 = 100_000_000_000;

pub(crate) fn discover(home: &Path, app_dirs: &[&Path]) -> Vec<Profile> {
    let support = home.join("Library/Application Support");
    let mut handled = HashSet::new();
    let mut profiles = Vec::new();
    for known in KNOWN {
        let root = support.join(known.folder);
        // APFS is usually case-insensitive, so compare lowercased names when deduplicating.
        if !handled.insert(known.folder.to_lowercase()) || !root.join("profiles.ini").is_file() {
            continue;
        }
        let app = app_path(app_dirs, known.bundles);
        profiles.extend(root_profiles(&root, known.browser, known.key, app));
    }
    // Unknown forks: any folder with a profiles.ini.
    if let Ok(entries) = std::fs::read_dir(&support) {
        let mut folders: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !handled.contains(&name.to_lowercase()))
            .filter(|name| support.join(name).join("profiles.ini").is_file())
            .collect();
        folders.sort();
        for folder in folders {
            let app = existing_app(app_dirs, &format!("{folder}.app"));
            profiles.extend(root_profiles(
                &support.join(&folder),
                &folder,
                &key_for(&folder),
                app,
            ));
        }
    }
    profiles
}

/// One `[ProfileN]` section of profiles.ini.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct IniProfile {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) relative: bool,
    pub(crate) default: bool,
}

/// Parses profiles.ini into its profiles and the install default (the `Default=` of an
/// `[Install…]` section, which newer Firefox uses instead of `Default=1`).
pub(crate) fn parse_profiles_ini(text: &str) -> (Vec<IniProfile>, Option<String>) {
    let mut profiles = Vec::new();
    let mut install_default = None;
    let mut section = String::new();
    let mut current: Option<IniProfile> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            profiles.extend(current.take().filter(|p| !p.path.is_empty()));
            section = name.to_owned();
            if section.starts_with("Profile") {
                current = Some(IniProfile {
                    relative: true,
                    ..IniProfile::default()
                });
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if let Some(profile) = current.as_mut() {
            match key {
                "Name" => profile.name = value.to_owned(),
                "Path" => profile.path = value.to_owned(),
                "IsRelative" => profile.relative = value == "1",
                "Default" => profile.default = value == "1",
                _ => {}
            }
        } else if section.starts_with("Install") && key == "Default" && install_default.is_none() {
            install_default = Some(value.to_owned());
        }
    }
    profiles.extend(current.take().filter(|p| !p.path.is_empty()));
    (profiles, install_default)
}

fn root_profiles(root: &Path, browser: &str, key: &str, app: Option<PathBuf>) -> Vec<Profile> {
    let text = match std::fs::read_to_string(root.join("profiles.ini")) {
        Ok(text) => text,
        // macOS may keep a browser's folder from other apps (Firefox's, on macOS 27): the file
        // is there but can't be read. The browser is still listed, unreadable, so the import
        // asks for Full Disk Access instead of leaving it out.
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            return vec![Profile {
                id: format!("{key}/locked"),
                browser: browser.to_owned(),
                name: "Default".to_owned(),
                family: Family::Gecko,
                app_path: app,
                keychain_service: None,
                keychain_account: None,
                has_cookies: true,
                has_history: true,
                readable: false,
                is_default: true,
                dir: root.to_owned(),
            }];
        }
        Err(_) => return Vec::new(),
    };
    let (entries, install_default) = parse_profiles_ini(&text);
    let mut profiles = Vec::new();
    for entry in entries {
        let dir = if entry.relative {
            root.join(&entry.path)
        } else {
            PathBuf::from(&entry.path)
        };
        let (has_cookies, cookies_readable) = probe(&dir.join("cookies.sqlite"));
        let (has_history, history_readable) = probe(&dir.join("places.sqlite"));
        if !has_cookies && !has_history {
            continue;
        }
        let dir_name = dir.file_name().map_or_else(
            || entry.path.clone(),
            |name| name.to_string_lossy().into_owned(),
        );
        let is_default = match &install_default {
            Some(path) => *path == entry.path,
            None => entry.default,
        };
        profiles.push(Profile {
            id: format!("{key}/{dir_name}"),
            browser: browser.to_owned(),
            name: if entry.name.is_empty() {
                dir_name.clone()
            } else {
                entry.name
            },
            family: Family::Gecko,
            app_path: app.clone(),
            keychain_service: None,
            keychain_account: None,
            has_cookies,
            has_history,
            readable: (!has_cookies || cookies_readable) && (!has_history || history_readable),
            is_default,
            dir,
        });
    }
    profiles
}

/// A `moz_cookies.expiry` in Unix seconds. Magnitude decides wherever it can (see
/// [`SURELY_MS`] and [`SURELY_SECS`]); the schema version only settles values in between.
pub(crate) fn expiry_secs(raw: i64, schema: i64) -> i64 {
    let millis = raw >= SURELY_MS || (raw >= SURELY_SECS && schema >= EXPIRY_MS_SCHEMA);
    if millis { raw.div_euclid(1000) } else { raw }
}

pub(crate) fn read_cookies(dir: &Path, now_unix: i64) -> Result<Cookies, Error> {
    let conn = open_copy(&dir.join("cookies.sqlite"))?;
    let cols = columns(&conn, "moz_cookies")?;
    if !cols.contains("host") || !cols.contains("name") {
        return Err(Error::Format("no Firefox cookies table".to_owned()));
    }
    let pick = |name: &str, fallback: &str| -> String {
        if cols.contains(name) {
            name.to_owned()
        } else {
            fallback.to_owned()
        }
    };
    let sql = format!(
        "SELECT name, {value}, host, {path}, {expiry}, {secure}, {http_only}, {same_site},
                {origin}, {partitioned} FROM moz_cookies",
        value = pick("value", "''"),
        path = pick("path", "'/'"),
        expiry = pick("expiry", "0"),
        secure = pick("isSecure", "0"),
        http_only = pick("isHttpOnly", "0"),
        same_site = pick("sameSite", "-1"),
        origin = pick("originAttributes", "''"),
        partitioned = pick("isPartitionedAttributeSet", "0"),
    );
    let schema: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    let mut out = Cookies::default();
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let expires = expiry_secs(row.get::<_, Option<i64>>(4)?.unwrap_or(0), schema);
        if expires < now_unix {
            out.expired += 1;
            continue;
        }
        // Containers, private windows and partitioned (CHIPS) cookies carry origin attributes;
        // WebKit's store has no place to keep that isolation.
        let origin: Option<String> = row.get(8)?;
        let partitioned_attr = row.get::<_, Option<i64>>(9)?.unwrap_or(0) != 0;
        if partitioned_attr || origin.is_some_and(|origin| !origin.is_empty()) {
            out.partitioned += 1;
            continue;
        }
        let (Ok(Some(name)), Ok(Some(value)), Ok(Some(host))) = (
            row.get::<_, Option<String>>(0),
            row.get::<_, Option<String>>(1),
            row.get::<_, Option<String>>(2),
        ) else {
            out.unreadable += 1;
            continue;
        };
        let same_site = match row.get::<_, Option<i64>>(7)?.unwrap_or(-1) {
            0 => Some(SameSite::None),
            1 => Some(SameSite::Lax),
            2 => Some(SameSite::Strict),
            _ => None,
        };
        out.cookies.push(Cookie {
            name,
            value,
            host_only: !host.starts_with('.'),
            domain: host,
            path: row
                .get::<_, Option<String>>(3)?
                .unwrap_or_else(|| "/".to_owned()),
            // Firefox writes only persistent cookies to disk, so every one has an expiry.
            expires_unix: Some(expires),
            secure: row.get::<_, Option<i64>>(5)?.unwrap_or(0) != 0,
            http_only: row.get::<_, Option<i64>>(6)?.unwrap_or(0) != 0,
            same_site,
        });
    }
    Ok(out)
}

pub(crate) fn read_history(dir: &Path, limit: usize) -> Result<Vec<Visit>, Error> {
    let conn = open_copy(&dir.join("places.sqlite"))?;
    let mut stmt = conn.prepare(
        "SELECT p.url, COALESCE(p.title, ''), v.visit_date
         FROM moz_historyvisits v JOIN moz_places p ON p.id = v.place_id
         WHERE p.url LIKE 'http://%' OR p.url LIKE 'https://%'
         GROUP BY p.url, v.visit_date
         ORDER BY v.visit_date DESC, p.url
         LIMIT ?1",
    )?;
    let limit = i64::try_from(limit).unwrap_or(i64::MAX);
    let rows = stmt.query_map([limit], |row| {
        // PRTime: microseconds since the Unix epoch.
        let micros: i64 = row.get(2)?;
        Ok(Visit {
            url: row.get(0)?,
            title: row.get(1)?,
            visited_at_ms: micros.div_euclid(1000),
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
    fn expiry_units() {
        let secs = 1_800_000_000;
        assert_eq!(expiry_secs(secs, 12), secs);
        assert_eq!(expiry_secs(secs * 1000, 16), secs);
        // A milliseconds value is recognised even if the schema number is off by one.
        assert_eq!(expiry_secs(secs * 1000, 15), secs);
        // So is a seconds value in a newer schema.
        assert_eq!(expiry_secs(secs, 17), secs);
        // Year-9999 seconds in an old schema stay seconds.
        assert_eq!(expiry_secs(253_402_300_799, 14), 253_402_300_799);
    }

    #[test]
    fn profiles_ini() {
        let (profiles, install) = parse_profiles_ini(
            "[Install4F96D1932A9F858E]\nDefault=Profiles/b.default-release\nLocked=1\n\n\
             [Profile1]\nName=default\nIsRelative=1\nPath=Profiles/a.default\nDefault=1\n\n\
             [Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/b.default-release\n\n\
             [General]\nStartWithLastProfile=1\nVersion=2\n",
        );
        assert_eq!(install.as_deref(), Some("Profiles/b.default-release"));
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "default");
        assert!(profiles[0].default && profiles[0].relative);
        assert_eq!(profiles[1].path, "Profiles/b.default-release");
    }
}
