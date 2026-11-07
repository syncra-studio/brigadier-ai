//! Gecko-family browsers: Firefox and its forks (Zen, LibreWolf, Waterfox, Floorp).
//!
//! Profiles are listed in `profiles.ini` at the browser's root. Cookies live in
//! `cookies.sqlite` (`moz_cookies`, unencrypted); history and bookmarks in `places.sqlite`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::layout::join_rel;
use crate::sqlite::{columns, open_copy};
use crate::{
    Bookmark, BookmarkRoot, Bookmarks, Cookie, Cookies, Error, Family, Layout, Profile, SameSite,
    Visit, app_path, existing_app, is_web_url, key_for, probe,
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

/// A Gecko browser's roots and executables on Windows, under `%APPDATA%`.
struct WindowsKnown {
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    /// Under the Program Files or per-user folders in [`Layout::Windows`].
    apps: &'static [&'static str],
}

const WINDOWS_KNOWN: &[WindowsKnown] = &[
    WindowsKnown {
        folder: "Mozilla/Firefox",
        browser: "Firefox",
        key: "firefox",
        // Developer Edition and Nightly share Firefox's profiles.ini.
        apps: &[
            "Mozilla Firefox/firefox.exe",
            "Firefox Developer Edition/firefox.exe",
            "Firefox Nightly/firefox.exe",
        ],
    },
    WindowsKnown {
        folder: "zen",
        browser: "Zen",
        key: "zen",
        apps: &["Zen Browser/zen.exe"],
    },
    WindowsKnown {
        folder: "librewolf",
        browser: "LibreWolf",
        key: "librewolf",
        apps: &["LibreWolf/librewolf.exe"],
    },
    WindowsKnown {
        folder: "Waterfox",
        browser: "Waterfox",
        key: "waterfox",
        apps: &["Waterfox/waterfox.exe"],
    },
    WindowsKnown {
        folder: "Floorp",
        browser: "Floorp",
        key: "floorp",
        apps: &["Ablaze Floorp/floorp.exe", "Floorp/floorp.exe"],
    },
];

/// A Gecko browser's roots and launchers on Linux.
struct LinuxKnown {
    browser: &'static str,
    key: &'static str,
    /// Launchers on `$PATH`.
    bins: &'static [&'static str],
    /// Roots under the home, then under `$XDG_CONFIG_HOME`, in the order the browser prefers:
    /// Firefox keeps using `~/.mozilla` when it is there, and new installs use the XDG folder.
    homes: &'static [&'static str],
    configs: &'static [&'static str],
    /// The snap's name and its root under `~/snap/<name>`.
    snap: Option<(&'static str, &'static str)>,
    /// The Flatpak app id and its roots under `~/.var/app/<id>`.
    flatpak: Option<(&'static str, &'static [&'static str])>,
}

// Firefox's classic and Snap folders agree with bb's `sources.ts`. The XDG and Flatpak folders and
// the forks are the projects' usual defaults, recalled rather than checked against a real machine.
const LINUX_KNOWN: &[LinuxKnown] = &[
    LinuxKnown {
        browser: "Firefox",
        key: "firefox",
        bins: &["firefox", "firefox-esr", "firefox-developer-edition"],
        homes: &[".mozilla/firefox"],
        configs: &["mozilla/firefox"],
        snap: Some(("firefox", "common/.mozilla/firefox")),
        flatpak: Some((
            "org.mozilla.firefox",
            &[".mozilla/firefox", "config/mozilla/firefox"],
        )),
    },
    LinuxKnown {
        browser: "Zen",
        key: "zen",
        bins: &["zen", "zen-browser"],
        homes: &[".zen"],
        configs: &[],
        snap: None,
        flatpak: Some(("app.zen_browser.zen", &[".zen"])),
    },
    LinuxKnown {
        browser: "LibreWolf",
        key: "librewolf",
        bins: &["librewolf"],
        homes: &[".librewolf"],
        configs: &["librewolf/librewolf"],
        snap: None,
        flatpak: Some(("io.gitlab.librewolf-community", &[".librewolf"])),
    },
    LinuxKnown {
        browser: "Waterfox",
        key: "waterfox",
        bins: &["waterfox"],
        homes: &[".waterfox"],
        configs: &[],
        snap: None,
        flatpak: Some(("net.waterfox.waterfox", &[".waterfox"])),
    },
    LinuxKnown {
        browser: "Floorp",
        key: "floorp",
        bins: &["floorp"],
        homes: &[".floorp"],
        configs: &[],
        snap: None,
        flatpak: Some(("one.ablaze.floorp", &[".floorp"])),
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
            let mut key = key_for(&folder);
            if KNOWN.iter().any(|known| known.key == key) {
                key.push_str("-firefox");
            }
            profiles.extend(root_profiles(
                &support.join(&folder),
                &format!("{folder} (Firefox)"),
                &key,
                app,
            ));
        }
    }
    profiles
}

/// The profiles of the known browsers on Windows or Linux. Each browser's install uses the first
/// of its roots that has a profiles.ini. Unknown forks are left out, as on these OSes nothing says
/// whether an app is a browser.
pub(crate) fn discover_other(layout: &Layout) -> Vec<Profile> {
    let first_root = |roots: Vec<PathBuf>| {
        roots
            .into_iter()
            .find(|root| root.join("profiles.ini").is_file())
    };
    let mut profiles = Vec::new();
    match layout {
        Layout::Windows {
            roaming_app_data,
            app_dirs,
            ..
        } => {
            let app_dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
            for known in WINDOWS_KNOWN {
                if let Some(root) = first_root(vec![join_rel(roaming_app_data, known.folder)]) {
                    let app = app_path(&app_dirs, known.apps);
                    profiles.extend(root_profiles(&root, known.browser, known.key, app));
                }
            }
        }
        Layout::Linux {
            home,
            config_home,
            path_dirs,
            snap_bin,
            flatpak_bin,
            ..
        } => {
            let app = |dirs: &[PathBuf], names: &[&str]| {
                let dirs: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
                app_path(&dirs, names)
            };
            for known in LINUX_KNOWN {
                let native = known
                    .homes
                    .iter()
                    .map(|root| join_rel(home, root))
                    .chain(known.configs.iter().map(|root| join_rel(config_home, root)))
                    .collect();
                if let Some(root) = first_root(native) {
                    let app = app(path_dirs, known.bins);
                    profiles.extend(root_profiles(&root, known.browser, known.key, app));
                }
                if let Some((snap, data)) = known.snap
                    && let Some(root) =
                        first_root(vec![join_rel(&home.join("snap").join(snap), data)])
                {
                    profiles.extend(root_profiles(
                        &root,
                        &format!("{} (Snap)", known.browser),
                        &format!("{}-snap", known.key),
                        app(snap_bin, &[snap]),
                    ));
                }
                if let Some((id, roots)) = known.flatpak {
                    let base = home.join(".var/app").join(id);
                    if let Some(root) =
                        first_root(roots.iter().map(|root| join_rel(&base, root)).collect())
                    {
                        profiles.extend(root_profiles(
                            &root,
                            &format!("{} (Flatpak)", known.browser),
                            &format!("{}-flatpak", known.key),
                            app(flatpak_bin, &[id]),
                        ));
                    }
                }
            }
        }
        Layout::Macos { .. } => {}
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
                local_state: None,
                linux_secret_application: None,
                has_cookies: true,
                has_history: true,
                has_bookmarks: true,
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
            local_state: None,
            linux_secret_application: None,
            has_cookies,
            has_history,
            // places.sqlite holds both.
            has_bookmarks: has_history,
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
            if let Ok(Some(host)) = row.get::<_, Option<String>>(2) {
                out.partitioned_domains.push(host);
            }
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
            source_insecure: false,
        });
    }
    Ok(out)
}

/// How many cookies a profile's store holds outside containers, private windows and
/// partitions, the ones an import can bring.
pub(crate) fn count_cookies(dir: &Path) -> Result<usize, Error> {
    let conn = open_copy(&dir.join("cookies.sqlite"))?;
    let cols = columns(&conn, "moz_cookies")?;
    let mut sql = "SELECT COUNT(*) FROM moz_cookies WHERE 1".to_owned();
    if cols.contains("originAttributes") {
        sql.push_str(" AND originAttributes = ''");
    }
    if cols.contains("isPartitionedAttributeSet") {
        sql.push_str(" AND isPartitionedAttributeSet = 0");
    }
    let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
    Ok(usize::try_from(count).unwrap_or(0))
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

/// A `moz_bookmarks` row.
struct BookmarkRow {
    id: i64,
    kind: i64,
    title: String,
    url: Option<String>,
    added_micros: Option<i64>,
}

/// The `moz_bookmarks` type of a bookmark, and of a folder. Separators are the third.
const TYPE_BOOKMARK: i64 = 1;
const TYPE_FOLDER: i64 = 2;
/// The guids of the folders holding what the user sees. The tags folder is left out on purpose:
/// its rows are labels on bookmarks that are listed elsewhere.
const BOOKMARK_ROOTS: [(&str, BookmarkRoot); 4] = [
    ("toolbar_____", BookmarkRoot::Bar),
    ("menu________", BookmarkRoot::Menu),
    ("unfiled_____", BookmarkRoot::Other),
    ("mobile______", BookmarkRoot::Mobile),
];

pub(crate) fn read_bookmarks(dir: &Path) -> Result<Bookmarks, Error> {
    let conn = open_copy(&dir.join("places.sqlite"))?;
    let mut children: HashMap<i64, Vec<BookmarkRow>> = HashMap::new();
    let mut stmt = conn.prepare(
        "SELECT b.id, b.parent, b.type, COALESCE(NULLIF(b.title, ''), p.title, ''), p.url,
                b.dateAdded
         FROM moz_bookmarks b LEFT JOIN moz_places p ON p.id = b.fk
         ORDER BY b.parent, b.position, b.id",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        children.entry(row.get(1)?).or_default().push(BookmarkRow {
            id: row.get(0)?,
            kind: row.get(2)?,
            title: row.get(3)?,
            url: row.get(4)?,
            added_micros: row.get(5)?,
        });
    }
    let mut out = Bookmarks::default();
    for (guid, root) in BOOKMARK_ROOTS {
        let id: Option<i64> = conn
            .query_row(
                "SELECT id FROM moz_bookmarks WHERE guid = ?1",
                [guid],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = id {
            let mut seen = HashSet::from([id]);
            walk_bookmarks(id, &children, root, &mut Vec::new(), &mut seen, &mut out);
        }
    }
    Ok(out)
}

/// Collects the bookmarks under a folder. `seen` stops a damaged database from looping.
fn walk_bookmarks(
    parent: i64,
    children: &HashMap<i64, Vec<BookmarkRow>>,
    root: BookmarkRoot,
    folders: &mut Vec<String>,
    seen: &mut HashSet<i64>,
    out: &mut Bookmarks,
) {
    for row in children.get(&parent).into_iter().flatten() {
        match row.kind {
            TYPE_FOLDER if seen.insert(row.id) => {
                folders.push(row.title.clone());
                walk_bookmarks(row.id, children, root, folders, seen, out);
                folders.pop();
            }
            TYPE_BOOKMARK => match row.url.as_deref().filter(|url| is_web_url(url)) {
                Some(url) => out.bookmarks.push(Bookmark {
                    url: url.to_owned(),
                    title: row.title.clone(),
                    root,
                    folders: folders.clone(),
                    // PRTime: microseconds since the Unix epoch.
                    added_at_ms: row
                        .added_micros
                        .filter(|micros| *micros > 0)
                        .map(|micros| micros.div_euclid(1000)),
                }),
                None => out.skipped += 1,
            },
            _ => {}
        }
    }
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
