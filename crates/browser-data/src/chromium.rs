//! Chromium-family browsers: Chrome, Edge, Brave, Arc, Dia, Vivaldi, Opera and unknown builds.
//!
//! Formats follow Chromium's sources: `Local State` lists profiles under `profile.info_cache`;
//! `Cookies` is a SQLite store whose values are encrypted with a key that depends on the OS (see
//! the `keys` module and components/os_crypt/sync); and since cookie store version 24 the
//! plaintext starts with the SHA-256 of the cookie's domain
//! (net/extras/sqlite/sqlite_persistent_cookie_store.cc). Bookmarks are a JSON file
//! (components/bookmarks/browser/bookmark_codec.cc).

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockModeDecrypt, KeyIvInit};
use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use rusqlite::Connection;
use sha2::{Digest, Sha256};

use crate::layout::join_rel;
use crate::sqlite::{columns, io_error, open_copy};
use crate::{
    Bookmark, BookmarkRoot, Bookmarks, ChromiumCookies, ChromiumKeys, Cookie, Error, Family,
    Layout, PartitionedCookie, Profile, SameSite, Visit, WEBKIT_EPOCH_OFFSET_SECS, app_path,
    existing_app, is_web_url, key_for, opens_web_links, probe,
};

/// AES-CBC IV Chromium uses on macOS and Linux: sixteen spaces.
const IV: [u8; 16] = [b' '; 16];
/// AES-GCM nonce length Chromium uses on Windows.
const GCM_NONCE: usize = 12;
/// A profile's bookmarks, then the ones Chromium keeps apart for a signed-in account.
const BOOKMARK_FILES: [&str; 2] = ["Bookmarks", "AccountBookmarks"];
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

/// Where a Windows browser keeps its data.
#[derive(Clone, Copy)]
enum WindowsBase {
    Local,
    Roaming,
}

/// A Chromium browser's data folder and executables on Windows.
struct WindowsKnown {
    base: WindowsBase,
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    /// Under the Program Files or per-user folders in [`Layout::Windows`].
    apps: &'static [&'static str],
}

const fn windows(
    base: WindowsBase,
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    apps: &'static [&'static str],
) -> WindowsKnown {
    WindowsKnown {
        base,
        folder,
        browser,
        key,
        apps,
    }
}

// Keys match macOS's, so a browser is one source on any OS. Paths are the vendors' usual defaults,
// recalled rather than checked against a real Windows machine.
const WINDOWS_KNOWN: &[WindowsKnown] = {
    use WindowsBase::{Local, Roaming};
    &[
        windows(
            Local,
            "Google/Chrome/User Data",
            "Google Chrome",
            "chrome",
            &["Google/Chrome/Application/chrome.exe"],
        ),
        windows(
            Local,
            "Google/Chrome Beta/User Data",
            "Google Chrome Beta",
            "chrome-beta",
            &["Google/Chrome Beta/Application/chrome.exe"],
        ),
        windows(
            Local,
            "Google/Chrome Dev/User Data",
            "Google Chrome Dev",
            "chrome-dev",
            &["Google/Chrome Dev/Application/chrome.exe"],
        ),
        windows(
            Local,
            "Google/Chrome SxS/User Data",
            "Google Chrome Canary",
            "chrome-canary",
            &["Google/Chrome SxS/Application/chrome.exe"],
        ),
        windows(
            Local,
            "Chromium/User Data",
            "Chromium",
            "chromium",
            &["Chromium/Application/chrome.exe"],
        ),
        windows(
            Local,
            "Microsoft/Edge/User Data",
            "Microsoft Edge",
            "edge",
            &["Microsoft/Edge/Application/msedge.exe"],
        ),
        windows(
            Local,
            "Microsoft/Edge Beta/User Data",
            "Microsoft Edge Beta",
            "edge-beta",
            &["Microsoft/Edge Beta/Application/msedge.exe"],
        ),
        windows(
            Local,
            "Microsoft/Edge Dev/User Data",
            "Microsoft Edge Dev",
            "edge-dev",
            &["Microsoft/Edge Dev/Application/msedge.exe"],
        ),
        windows(
            Local,
            "Microsoft/Edge SxS/User Data",
            "Microsoft Edge Canary",
            "edge-canary",
            &["Microsoft/Edge SxS/Application/msedge.exe"],
        ),
        windows(
            Local,
            "BraveSoftware/Brave-Browser/User Data",
            "Brave",
            "brave",
            &["BraveSoftware/Brave-Browser/Application/brave.exe"],
        ),
        windows(
            Local,
            "BraveSoftware/Brave-Browser-Beta/User Data",
            "Brave Beta",
            "brave-beta",
            &["BraveSoftware/Brave-Browser-Beta/Application/brave.exe"],
        ),
        windows(
            Local,
            "BraveSoftware/Brave-Browser-Nightly/User Data",
            "Brave Nightly",
            "brave-nightly",
            &["BraveSoftware/Brave-Browser-Nightly/Application/brave.exe"],
        ),
        windows(
            Local,
            "Vivaldi/User Data",
            "Vivaldi",
            "vivaldi",
            &["Vivaldi/Application/vivaldi.exe"],
        ),
        // Opera keeps its data under Roaming, with its single profile often in the root.
        windows(
            Roaming,
            "Opera Software/Opera Stable",
            "Opera",
            "opera",
            &["Opera/launcher.exe", "Opera/opera.exe"],
        ),
        windows(
            Roaming,
            "Opera Software/Opera GX Stable",
            "Opera GX",
            "opera-gx",
            &["Opera GX/launcher.exe", "Opera GX/opera.exe"],
        ),
    ]
};

/// A Chromium browser's folders and launchers on Linux.
struct LinuxKnown {
    /// Under `$XDG_CONFIG_HOME`, and under `config` in its Flatpak's data.
    folder: &'static str,
    browser: &'static str,
    key: &'static str,
    /// The `application` attribute of its keyring password, shared across channels.
    secret: &'static str,
    /// Launchers on `$PATH`.
    bins: &'static [&'static str],
    /// The snap's name and its data folder under `~/snap/<name>`.
    snap: Option<(&'static str, &'static str)>,
    /// The Flatpak app id.
    flatpak: Option<&'static str>,
}

// The folders and `secret` names of the stable channels agree with bb's `sources.ts`. The other
// channels, Snap and Flatpak paths are the vendors' usual defaults, recalled rather than checked
// against a real machine.
const LINUX_KNOWN: &[LinuxKnown] = &[
    LinuxKnown {
        folder: "google-chrome",
        browser: "Google Chrome",
        key: "chrome",
        secret: "chrome",
        bins: &["google-chrome", "google-chrome-stable"],
        snap: None,
        flatpak: Some("com.google.Chrome"),
    },
    LinuxKnown {
        folder: "google-chrome-beta",
        browser: "Google Chrome Beta",
        key: "chrome-beta",
        secret: "chrome",
        bins: &["google-chrome-beta"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "google-chrome-unstable",
        browser: "Google Chrome Dev",
        key: "chrome-dev",
        secret: "chrome",
        bins: &["google-chrome-unstable"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "chromium",
        browser: "Chromium",
        key: "chromium",
        secret: "chromium",
        bins: &["chromium", "chromium-browser"],
        snap: Some(("chromium", "common/chromium")),
        flatpak: Some("org.chromium.Chromium"),
    },
    LinuxKnown {
        folder: "microsoft-edge",
        browser: "Microsoft Edge",
        key: "edge",
        secret: "msedge",
        bins: &["microsoft-edge", "microsoft-edge-stable"],
        snap: None,
        flatpak: Some("com.microsoft.Edge"),
    },
    LinuxKnown {
        folder: "microsoft-edge-beta",
        browser: "Microsoft Edge Beta",
        key: "edge-beta",
        secret: "msedge",
        bins: &["microsoft-edge-beta"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "microsoft-edge-dev",
        browser: "Microsoft Edge Dev",
        key: "edge-dev",
        secret: "msedge",
        bins: &["microsoft-edge-dev"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "BraveSoftware/Brave-Browser",
        browser: "Brave",
        key: "brave",
        secret: "brave",
        bins: &["brave-browser", "brave", "brave-browser-stable"],
        snap: Some(("brave", "current/.config/BraveSoftware/Brave-Browser")),
        flatpak: Some("com.brave.Browser"),
    },
    LinuxKnown {
        folder: "BraveSoftware/Brave-Browser-Beta",
        browser: "Brave Beta",
        key: "brave-beta",
        secret: "brave",
        bins: &["brave-browser-beta"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "BraveSoftware/Brave-Browser-Nightly",
        browser: "Brave Nightly",
        key: "brave-nightly",
        secret: "brave",
        bins: &["brave-browser-nightly"],
        snap: None,
        flatpak: None,
    },
    LinuxKnown {
        folder: "vivaldi",
        browser: "Vivaldi",
        key: "vivaldi",
        secret: "vivaldi",
        bins: &["vivaldi", "vivaldi-stable"],
        snap: None,
        flatpak: Some("com.vivaldi.Vivaldi"),
    },
    LinuxKnown {
        folder: "opera",
        browser: "Opera",
        key: "opera",
        secret: "opera",
        bins: &["opera"],
        snap: Some(("opera", "current/.config/opera")),
        flatpak: Some("com.opera.Opera"),
    },
];

/// Folders never treated as a browser even if they look like one.
const SKIP: &[&str] = &["Google/Chrome for Testing"];

/// One browser install: its data root and how to label its profiles.
struct Root {
    dir: PathBuf,
    browser: String,
    key: String,
    service: Option<String>,
    account: Option<String>,
    linux_secret_application: Option<String>,
    app_path: Option<PathBuf>,
}

/// The profiles of the known browsers on Windows or Linux. Unlike macOS, whose bundles say whether
/// they open web links, these OSes give no cheap way to tell an unknown browser from an Electron
/// app, so only known browsers are listed.
pub(crate) fn discover_other(layout: &Layout) -> Vec<Profile> {
    let mut profiles = Vec::new();
    let mut add =
        |dir: PathBuf, browser: String, key: String, secret: Option<&str>, app: Option<PathBuf>| {
            if dir.is_dir() {
                profiles.extend(root_profiles(&Root {
                    dir,
                    browser,
                    key,
                    service: None,
                    account: None,
                    linux_secret_application: secret.map(str::to_owned),
                    app_path: app,
                }));
            }
        };
    match layout {
        Layout::Windows {
            local_app_data,
            roaming_app_data,
            app_dirs,
        } => {
            let app_dirs: Vec<&Path> = app_dirs.iter().map(PathBuf::as_path).collect();
            for known in WINDOWS_KNOWN {
                let base = match known.base {
                    WindowsBase::Local => local_app_data,
                    WindowsBase::Roaming => roaming_app_data,
                };
                add(
                    join_rel(base, known.folder),
                    known.browser.to_owned(),
                    known.key.to_owned(),
                    None,
                    app_path(&app_dirs, known.apps),
                );
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
                let secret = Some(known.secret);
                add(
                    join_rel(config_home, known.folder),
                    known.browser.to_owned(),
                    known.key.to_owned(),
                    secret,
                    app(path_dirs, known.bins),
                );
                if let Some((snap, data)) = known.snap {
                    add(
                        join_rel(&home.join("snap").join(snap), data),
                        format!("{} (Snap)", known.browser),
                        format!("{}-snap", known.key),
                        secret,
                        app(snap_bin, &[snap]),
                    );
                }
                if let Some(id) = known.flatpak {
                    let config = home.join(".var/app").join(id).join("config");
                    add(
                        join_rel(&config, known.folder),
                        format!("{} (Flatpak)", known.browser),
                        format!("{}-flatpak", known.key),
                        secret,
                        app(flatpak_bin, &[id]),
                    );
                }
            }
        }
        Layout::Macos { .. } => {}
    }
    profiles
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
            service: Some(known.service.to_owned()),
            account: Some(known.account.to_owned()),
            linux_secret_application: None,
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
                service: Some(format!("{folder} Safe Storage")),
                account: Some(folder.clone()),
                linux_secret_application: None,
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
        keychain_service: root.service.clone(),
        keychain_account: root.account.clone(),
        local_state: Some(root.dir.join("Local State")),
        linux_secret_application: root.linux_secret_application.clone(),
        has_cookies,
        has_history,
        has_bookmarks: BOOKMARK_FILES.iter().any(|file| probe(&dir.join(file)).0),
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

/// Decrypts an AES-128-CBC payload, or `None` when the padding is wrong (which is what a wrong
/// password looks like).
fn decrypt_cbc(key: &[u8; 16], ciphertext: &[u8]) -> Option<Vec<u8>> {
    let decryptor = cbc::Decryptor::<aes::Aes128>::new(key.into(), &IV.into());
    decryptor.decrypt_padded_vec::<Pkcs7>(ciphertext).ok()
}

/// Decrypts an AES-256-GCM payload (nonce, then ciphertext and tag), or `None` when it fails its
/// integrity check.
fn decrypt_gcm(key: &[u8; 32], payload: &[u8]) -> Option<Vec<u8>> {
    let (nonce, sealed) = payload.split_at_checked(GCM_NONCE)?;
    Aes256Gcm::new(key.into())
        .decrypt(Nonce::from_slice(nonce), sealed)
        .ok()
}

/// The value inside a decrypted plaintext, checking and stripping the domain hash when the store
/// has one.
fn plaintext_value(mut bytes: Vec<u8>, host: &str, version: i64) -> Option<String> {
    if version >= DOMAIN_HASH_VERSION {
        let hash = Sha256::digest(host.as_bytes());
        if bytes.len() < hash.len() || bytes[..hash.len()] != hash[..] {
            return None;
        }
        bytes.drain(..hash.len());
    }
    String::from_utf8(bytes).ok()
}

/// What a cookie's stored value turned out to be.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Opened {
    Value(String),
    /// Chrome's `v20`: locked to the browser, so never tried.
    AppBound,
    Unreadable,
}

/// Opens a stored `vNN` value with `keys`, passing each decrypted plaintext through `decode`,
/// which also judges whether the key was right.
pub(crate) fn open_sealed(
    encrypted: &[u8],
    keys: &ChromiumKeys,
    decode: &dyn Fn(Vec<u8>) -> Option<String>,
) -> Opened {
    let Some((prefix, payload)) = encrypted.split_at_checked(3) else {
        return Opened::Unreadable;
    };
    let cbc = |primary: Option<[u8; 16]>| {
        // Each key is tried through `decode` too: bad padding isn't the only sign of a wrong key.
        // The empty password is what Chromium on Linux seals with when no keyring answered, which
        // also covers a keyring that has gone since.
        [primary, keys.cbc_empty]
            .iter()
            .flatten()
            .find_map(|key| decode(decrypt_cbc(key, payload)?))
    };
    let value = match prefix {
        b"v20" => return Opened::AppBound,
        b"v10" => match &keys.gcm_v10 {
            Some(key) => decrypt_gcm(key, payload).and_then(decode),
            None => cbc(keys.cbc_v10),
        },
        b"v11" => cbc(keys.cbc_v11),
        _ => None,
    };
    value.map_or(Opened::Unreadable, Opened::Value)
}

/// The cookie's value, decrypted if it needs it.
fn cookie_value(
    plain: String,
    encrypted: &[u8],
    host: &str,
    keys: &ChromiumKeys,
    version: i64,
) -> Opened {
    if !plain.is_empty() || encrypted.is_empty() {
        return Opened::Value(plain);
    }
    open_sealed(encrypted, keys, &|bytes| {
        plaintext_value(bytes, host, version)
    })
}

pub(crate) fn read_cookies(
    dir: &Path,
    keys: &ChromiumKeys,
    now_unix: i64,
) -> Result<ChromiumCookies, Error> {
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
                {same_site}, {persistent}, {top_frame}, {cross_site}, {source_scheme} FROM cookies",
        value = pick(&["value"], "''"),
        encrypted = pick(&["encrypted_value"], "X''"),
        path = pick(&["path"], "'/'"),
        expires = pick(&["expires_utc"], "0"),
        secure = pick(&["is_secure", "secure"], "0"),
        http_only = pick(&["is_httponly", "httponly"], "0"),
        same_site = pick(&["samesite"], "-1"),
        persistent = pick(&["is_persistent", "has_expires"], "1"),
        top_frame = pick(&["top_frame_site_key"], "''"),
        source_scheme = pick(&["source_scheme"], "0"),
        cross_site = pick(&["has_cross_site_ancestor"], "1"),
    );
    let version = meta_version(&conn);

    let mut all = ChromiumCookies::default();
    let out = &mut all.read;
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
        let top_frame = row
            .get::<_, Option<String>>(10)?
            .filter(|key| !key.is_empty());
        if top_frame.is_some() {
            out.partitioned += 1;
            out.partitioned_domains.push(host.clone());
        }
        let plain: Option<String> = row.get(2).ok().flatten();
        let encrypted: Option<Vec<u8>> = row.get(3)?;
        let value = match cookie_value(
            plain.unwrap_or_default(),
            encrypted.as_deref().unwrap_or_default(),
            &host,
            keys,
            version,
        ) {
            Opened::Value(value) => value,
            // A partitioned cookie is already counted as that.
            _ if top_frame.is_some() => continue,
            Opened::AppBound => {
                out.unreadable += 1;
                out.unreadable_domains.push(host.clone());
                all.app_bound_domains.push(host);
                continue;
            }
            Opened::Unreadable => {
                out.unreadable += 1;
                out.unreadable_domains.push(host);
                continue;
            }
        };
        let same_site = match row.get::<_, Option<i64>>(8)?.unwrap_or(-1) {
            0 => Some(SameSite::None),
            1 => Some(SameSite::Lax),
            2 => Some(SameSite::Strict),
            _ => None,
        };
        let cookie = Cookie {
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
            // net::CookieSourceScheme: 0 unset, 1 non-secure, 2 secure.
            source_insecure: row.get::<_, Option<i64>>(12)?.unwrap_or(0) == 1,
        };
        match top_frame {
            Some(top_frame_site_key) => out.partitioned_cookies.push(PartitionedCookie {
                cookie,
                top_frame_site_key,
                has_cross_site_ancestor: row.get::<_, Option<i64>>(11)?.unwrap_or(1) != 0,
            }),
            None => out.cookies.push(cookie),
        }
    }
    Ok(all)
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

/// A profile's bookmarks: its own file, then any kept apart for a signed-in account. The second
/// file's name is recalled from Chromium's sources, not checked; it is read only when it is there.
pub(crate) fn read_bookmarks(dir: &Path) -> Result<Bookmarks, Error> {
    let mut out = Bookmarks::default();
    let mut found = false;
    for file in BOOKMARK_FILES {
        let path = dir.join(file);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(io_error(&path, err)),
        };
        found = true;
        let tree: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|err| Error::Format(format!("{file}: {err}")))?;
        let roots = tree
            .get("roots")
            .ok_or_else(|| Error::Format(format!("{file}: no roots")))?;
        for (name, root) in [
            ("bookmark_bar", BookmarkRoot::Bar),
            ("other", BookmarkRoot::Other),
            ("synced", BookmarkRoot::Mobile),
        ] {
            if let Some(children) = roots.get(name).and_then(|node| node.get("children")) {
                walk_bookmarks(children, root, &mut Vec::new(), &mut out);
            }
        }
    }
    if !found {
        return Err(Error::NotFound(dir.join(BOOKMARK_FILES[0])));
    }
    Ok(out)
}

/// Collects the bookmarks under a JSON array of nodes. serde_json refuses documents nested past
/// its recursion limit, which bounds the depth here.
fn walk_bookmarks(
    children: &serde_json::Value,
    root: BookmarkRoot,
    folders: &mut Vec<String>,
    out: &mut Bookmarks,
) {
    for node in children.as_array().into_iter().flatten() {
        let name = node.get("name").and_then(|name| name.as_str());
        match node.get("type").and_then(|kind| kind.as_str()) {
            Some("folder") => {
                folders.push(name.unwrap_or_default().to_owned());
                if let Some(children) = node.get("children") {
                    walk_bookmarks(children, root, folders, out);
                }
                folders.pop();
            }
            Some("url") => {
                let url = node.get("url").and_then(|url| url.as_str());
                let Some(url) = url.filter(|url| is_web_url(url)) else {
                    out.skipped += 1;
                    continue;
                };
                out.bookmarks.push(Bookmark {
                    url: url.to_owned(),
                    title: name.unwrap_or_default().to_owned(),
                    root,
                    folders: folders.clone(),
                    added_at_ms: added_at_ms(node),
                });
            }
            _ => {}
        }
    }
}

/// A node's `date_added`, a string of microseconds since 1601; `"0"` means unrecorded.
fn added_at_ms(node: &serde_json::Value) -> Option<i64> {
    let micros: i64 = node.get("date_added")?.as_str()?.parse().ok()?;
    (micros > 0).then(|| micros.div_euclid(1000) - WEBKIT_EPOCH_OFFSET_SECS * 1000)
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
        let keys = ChromiumKeys::from_macos_password(b"x");
        let open = |encrypted: &[u8]| cookie_value(String::new(), encrypted, "h", &keys, 0);
        assert_eq!(open(b"plain"), Opened::Unreadable);
        assert_eq!(open(b"v1"), Opened::Unreadable);
        assert_eq!(open(b"v10short"), Opened::Unreadable);
        assert_eq!(
            open(&[b"v10".as_slice(), &[0u8; 16]].concat()),
            Opened::Unreadable
        );
    }

    #[test]
    fn v20_is_app_bound_with_any_keys_and_no_key_is_tried() {
        // Even a payload that would open with these keys isn't tried.
        let keys = ChromiumKeys::from_windows_key([1; 32]);
        for keys in [keys, ChromiumKeys::default()] {
            assert_eq!(
                cookie_value(String::new(), b"v20anything", "h", &keys, 24),
                Opened::AppBound
            );
        }
    }

    #[test]
    fn gcm_rejects_short_and_bad_payloads() {
        let key = [1; 32];
        assert!(decrypt_gcm(&key, b"short").is_none());
        assert!(decrypt_gcm(&key, &[0u8; 40]).is_none());
    }
}
