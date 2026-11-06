//! Windows and Linux, over fixtures. Layouts are plain paths and the ciphers are plain Rust, so all
//! of this runs on every OS; only calling DPAPI itself or a live keyring needs the real thing (see
//! `src/keys.rs` and `src/secret_service.rs`).

mod common;

use std::path::{Path, PathBuf};

use brigadier_browser_data::{
    ChromiumKeys, Error, Family, Layout, Profile, SecretError, discover_layout, read_bookmarks,
    read_cookies_with_keys, read_history, sources_layout,
};
use common::*;

fn local_state(dir: &Path, json: &str) {
    mkdirs(dir);
    std::fs::write(dir.join("Local State"), json).unwrap();
}

fn ids(profiles: &[Profile]) -> Vec<&str> {
    profiles.iter().map(|p| p.id.as_str()).collect()
}

/// `root/<path>/...` for a `/`-separated path, in native separators.
fn at(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_owned();
    path.extend(relative.split('/'));
    path
}

fn firefox_root(root: &Path) {
    touch(&at(root, "Profiles/abc.default-release/cookies.sqlite"));
    touch(&at(root, "Profiles/abc.default-release/places.sqlite"));
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\n\
         Default=1\n",
    )
    .unwrap();
}

struct WindowsHome {
    local: tempfile::TempDir,
    roaming: tempfile::TempDir,
    program_files: tempfile::TempDir,
}

impl WindowsHome {
    fn new() -> Self {
        Self {
            local: tempfile::tempdir().unwrap(),
            roaming: tempfile::tempdir().unwrap(),
            program_files: tempfile::tempdir().unwrap(),
        }
    }

    fn layout(&self) -> Layout {
        Layout::Windows {
            local_app_data: self.local.path().to_owned(),
            roaming_app_data: self.roaming.path().to_owned(),
            app_dirs: vec![
                self.program_files.path().to_owned(),
                self.local.path().to_owned(),
                self.local.path().join("Programs"),
            ],
        }
    }
}

#[test]
fn windows_finds_every_listed_browser_where_it_keeps_its_data() {
    let home = WindowsHome::new();
    let (local, roaming, program_files) = (
        home.local.path(),
        home.roaming.path(),
        home.program_files.path(),
    );

    // Chrome, installed for everyone, with two profiles.
    let chrome = at(local, "Google/Chrome/User Data");
    local_state(
        &chrome,
        r#"{"profile":{"info_cache":{"Default":{"name":"Me"},"Profile 1":{"name":"Work"}},
            "last_used":"Profile 1"}}"#,
    );
    touch(&at(&chrome, "Default/Network/Cookies"));
    touch(&at(&chrome, "Default/History"));
    touch(&at(&chrome, "Default/Bookmarks"));
    touch(&at(&chrome, "Profile 1/History"));
    touch(&at(program_files, "Google/Chrome/Application/chrome.exe"));

    // Edge, Brave and Vivaldi. Vivaldi installs per user.
    for (folder, exe) in [
        ("Microsoft/Edge", "Microsoft/Edge/Application/msedge.exe"),
        (
            "BraveSoftware/Brave-Browser",
            "BraveSoftware/Brave-Browser/Application/brave.exe",
        ),
    ] {
        let dir = at(local, &format!("{folder}/User Data"));
        local_state(&dir, "{}");
        touch(&at(&dir, "Default/Network/Cookies"));
        touch(&at(program_files, exe));
    }
    let vivaldi = at(local, "Vivaldi/User Data");
    local_state(&vivaldi, "{}");
    touch(&at(&vivaldi, "Default/History"));
    touch(&at(local, "Vivaldi/Application/vivaldi.exe"));

    // Opera, under Roaming with its profile in the root, installed per user.
    let opera = at(roaming, "Opera Software/Opera Stable");
    local_state(&opera, "{}");
    touch(&at(&opera, "Network/Cookies"));
    touch(&at(local, "Programs/Opera/launcher.exe"));

    // Firefox, under Roaming.
    firefox_root(&at(roaming, "Mozilla/Firefox"));
    touch(&at(program_files, "Mozilla Firefox/firefox.exe"));

    // Chrome Beta's folder remains but its app is gone.
    let beta = at(local, "Google/Chrome Beta/User Data");
    local_state(&beta, "{}");
    touch(&at(&beta, "Default/History"));

    let layout = home.layout();
    let profiles = discover_layout(&layout);
    assert_eq!(
        ids(&profiles),
        [
            "brave/Default",
            "firefox/abc.default-release",
            "chrome/Default",
            "chrome/Profile 1",
            "chrome-beta/Default",
            "edge/Default",
            "opera/Default",
            "vivaldi/Default",
        ]
    );
    let get = |id: &str| profiles.iter().find(|p| p.id == id).unwrap();

    let default = get("chrome/Default");
    assert_eq!(default.family, Family::Chromium);
    assert_eq!(default.name, "Me");
    assert_eq!(default.dir, chrome.join("Default"));
    assert_eq!(
        default.local_state.as_deref(),
        Some(chrome.join("Local State").as_path())
    );
    assert_eq!(default.keychain_service, None);
    assert_eq!(default.linux_secret_application, None);
    assert!(default.has_cookies && default.has_history && default.has_bookmarks);
    assert!(!default.is_default && get("chrome/Profile 1").is_default);
    assert_eq!(
        default.app_path.as_deref(),
        Some(at(program_files, "Google/Chrome/Application/chrome.exe").as_path())
    );

    let opera_profile = get("opera/Default");
    assert_eq!(opera_profile.dir, opera);
    assert_eq!(
        opera_profile.local_state.as_deref(),
        Some(opera.join("Local State").as_path())
    );

    let firefox = get("firefox/abc.default-release");
    assert_eq!(firefox.family, Family::Gecko);
    assert_eq!(
        firefox.dir,
        at(roaming, "Mozilla/Firefox/Profiles/abc.default-release")
    );
    assert!(firefox.has_bookmarks && firefox.is_default);

    // Installed browsers only, in Settings' order, the last-used profile first; Safari is nowhere.
    let sources = sources_layout(&layout);
    assert_eq!(
        sources.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
        ["chrome", "edge", "brave", "vivaldi", "opera", "firefox"]
    );
    assert_eq!(sources[0].profiles[0].name, "Work");
    assert_eq!(
        sources[4].app_path,
        at(local, "Programs/Opera/launcher.exe")
    );
}

struct LinuxHome {
    home: tempfile::TempDir,
    config: tempfile::TempDir,
    bin: tempfile::TempDir,
    snap_bin: tempfile::TempDir,
    flatpak_bin: tempfile::TempDir,
}

impl LinuxHome {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
            config: tempfile::tempdir().unwrap(),
            bin: tempfile::tempdir().unwrap(),
            snap_bin: tempfile::tempdir().unwrap(),
            flatpak_bin: tempfile::tempdir().unwrap(),
        }
    }

    fn layout(&self) -> Layout {
        Layout::Linux {
            home: self.home.path().to_owned(),
            config_home: self.config.path().to_owned(),
            path_dirs: vec![self.bin.path().to_owned()],
            snap_bin: vec![self.snap_bin.path().to_owned()],
            flatpak_bin: vec![self.flatpak_bin.path().to_owned()],
            icon_dirs: vec![self.home.path().join("icons")],
        }
    }
}

#[test]
fn linux_finds_native_snap_and_flatpak_installs() {
    let fixture = LinuxHome::new();
    let home = fixture.home.path();
    let config = fixture.config.path();
    let chromium_profile = |dir: &Path| {
        local_state(dir, "{}");
        touch(&at(dir, "Default/Cookies"));
        touch(&at(dir, "Default/History"));
    };

    // Native: Chrome under the XDG config home, and Brave's nested folder.
    chromium_profile(&config.join("google-chrome"));
    touch(&fixture.bin.path().join("google-chrome"));
    chromium_profile(&at(config, "BraveSoftware/Brave-Browser"));
    touch(&fixture.bin.path().join("brave-browser"));
    // Edge's folder remains with no launcher.
    chromium_profile(&config.join("microsoft-edge"));

    // Snap: Chromium, and Brave's revision-linked folder.
    chromium_profile(&at(home, "snap/chromium/common/chromium"));
    touch(&fixture.snap_bin.path().join("chromium"));
    chromium_profile(&at(
        home,
        "snap/brave/current/.config/BraveSoftware/Brave-Browser",
    ));
    touch(&fixture.snap_bin.path().join("brave"));

    // Flatpak: Chrome.
    chromium_profile(&at(home, ".var/app/com.google.Chrome/config/google-chrome"));
    touch(&fixture.flatpak_bin.path().join("com.google.Chrome"));

    // Firefox's variants: classic, Snap and Flatpak.
    firefox_root(&at(home, ".mozilla/firefox"));
    touch(&fixture.bin.path().join("firefox"));
    firefox_root(&at(home, "snap/firefox/common/.mozilla/firefox"));
    touch(&fixture.snap_bin.path().join("firefox"));
    firefox_root(&at(home, ".var/app/org.mozilla.firefox/.mozilla/firefox"));
    touch(&fixture.flatpak_bin.path().join("org.mozilla.firefox"));

    let layout = fixture.layout();
    let profiles = discover_layout(&layout);
    let get = |id: &str| profiles.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        ids(&profiles),
        [
            "brave/Default",
            "brave-snap/Default",
            "chromium-snap/Default",
            "firefox/abc.default-release",
            "firefox-flatpak/abc.default-release",
            "firefox-snap/abc.default-release",
            "chrome/Default",
            "chrome-flatpak/Default",
            "edge/Default",
        ]
    );

    let chrome = get("chrome/Default");
    assert_eq!(chrome.dir, at(config, "google-chrome/Default"));
    assert_eq!(chrome.linux_secret_application.as_deref(), Some("chrome"));
    assert_eq!(chrome.keychain_service, None);
    assert_eq!(
        get("edge/Default").linux_secret_application.as_deref(),
        Some("msedge")
    );
    let flatpak = get("chrome-flatpak/Default");
    assert_eq!(flatpak.browser, "Google Chrome (Flatpak)");
    assert_eq!(flatpak.linux_secret_application.as_deref(), Some("chrome"));
    assert_eq!(
        flatpak.app_path.as_deref(),
        Some(
            fixture
                .flatpak_bin
                .path()
                .join("com.google.Chrome")
                .as_path()
        )
    );
    assert_eq!(
        get("firefox-snap/abc.default-release").browser,
        "Firefox (Snap)"
    );

    let sources = sources_layout(&layout);
    assert_eq!(
        sources.iter().map(|s| s.key.as_str()).collect::<Vec<_>>(),
        [
            "chrome",
            "chrome-flatpak",
            "chromium-snap",
            "brave",
            "brave-snap",
            "firefox",
            "firefox-flatpak",
            "firefox-snap",
        ]
    );
}

#[test]
fn linux_firefox_prefers_the_classic_folder_then_the_xdg_one() {
    let fixture = LinuxHome::new();
    let xdg = at(fixture.config.path(), "mozilla/firefox");
    firefox_root(&xdg);
    let profiles = discover_layout(&fixture.layout());
    assert_eq!(ids(&profiles), ["firefox/abc.default-release"]);
    assert!(profiles[0].dir.starts_with(&xdg));

    let classic = at(fixture.home.path(), ".mozilla/firefox");
    firefox_root(&classic);
    let profiles = discover_layout(&fixture.layout());
    assert_eq!(ids(&profiles), ["firefox/abc.default-release"]);
    assert!(profiles[0].dir.starts_with(&classic));
}

#[test]
fn no_browsers_list_nothing_on_windows_or_linux() {
    assert!(discover_layout(&WindowsHome::new().layout()).is_empty());
    assert!(discover_layout(&LinuxHome::new().layout()).is_empty());
}

const WINDOWS_KEY: [u8; 32] = [42; 32];

#[test]
fn windows_cookies_open_with_the_master_key_and_v20_stays_closed_with_its_domains_named() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("Network").join("Cookies");
    let conn = chromium_cookies_db(&db, 24);
    let host = ".site.example";
    insert_chromium(
        &conn,
        &ChromiumRow {
            host,
            name: "sid",
            encrypted: chromium_encrypt_windows(&WINDOWS_KEY, "secret", Some(host)),
            secure: true,
            samesite: 1,
            ..ChromiumRow::default()
        },
    );
    // App-Bound cookies: a valid GCM payload under the master key, so opening it would succeed
    // if the reader ever tried.
    for host in [".google.example", ".bank.example"] {
        let mut sealed = chromium_encrypt_windows(&WINDOWS_KEY, "locked", Some(host));
        sealed[..3].copy_from_slice(b"v20");
        insert_chromium(
            &conn,
            &ChromiumRow {
                host,
                name: "bound",
                encrypted: sealed,
                ..ChromiumRow::default()
            },
        );
    }
    // A wrong key, a broken tag, and a future prefix.
    let bad_key = chromium_encrypt_windows(&[1; 32], "x", Some(".k.example"));
    let mut bad_tag = chromium_encrypt_windows(&WINDOWS_KEY, "x", Some(".t.example"));
    *bad_tag.last_mut().unwrap() ^= 1;
    let mut future = chromium_encrypt_windows(&WINDOWS_KEY, "x", Some(".f.example"));
    future[..3].copy_from_slice(b"v99");
    for (host, encrypted) in [
        (".k.example", bad_key),
        (".t.example", bad_tag),
        (".f.example", future),
    ] {
        insert_chromium(
            &conn,
            &ChromiumRow {
                host,
                encrypted,
                ..ChromiumRow::default()
            },
        );
    }
    drop(conn);

    let chrome = profile("chrome/Default", Family::Chromium, dir.path().to_owned());
    let keys = ChromiumKeys::from_windows_key(WINDOWS_KEY);
    let read = read_cookies_with_keys(&chrome, &keys, NOW).unwrap();

    assert_eq!(read.read.cookies.len(), 1);
    let cookie = &read.read.cookies[0];
    assert_eq!(
        (cookie.name.as_str(), cookie.value.as_str()),
        ("sid", "secret")
    );
    assert!(cookie.secure);
    assert_eq!(read.app_bound_domains, [".google.example", ".bank.example"]);
    assert_eq!(read.read.unreadable, 5);
    assert_eq!(
        read.read.unreadable_domains,
        [
            ".google.example",
            ".bank.example",
            ".k.example",
            ".t.example",
            ".f.example"
        ]
    );

    // Without any key, plain reading still names the App-Bound domains.
    let read = read_cookies_with_keys(&chrome, &ChromiumKeys::default(), NOW).unwrap();
    assert!(read.read.cookies.is_empty());
    assert_eq!(read.app_bound_domains.len(), 2);
}

#[test]
fn windows_cookies_before_the_domain_hash_store_version() {
    let dir = tempfile::tempdir().unwrap();
    let conn = chromium_cookies_db(&dir.path().join("Cookies"), 23);
    insert_chromium(
        &conn,
        &ChromiumRow {
            name: "old",
            encrypted: chromium_encrypt_windows(&WINDOWS_KEY, "value", None),
            ..ChromiumRow::default()
        },
    );
    drop(conn);
    let keys = ChromiumKeys::from_windows_key(WINDOWS_KEY);
    let read = read_cookies_with_keys(
        &profile("edge/Default", Family::Chromium, dir.path().to_owned()),
        &keys,
        NOW,
    )
    .unwrap();
    assert_eq!(read.read.cookies[0].value, "value");
}

fn linux_store(dir: &Path) -> Profile {
    let conn = chromium_cookies_db(&dir.join("Cookies"), 24);
    let rows: [(&str, Vec<u8>); 4] = [
        (
            ".v10.example",
            chromium_encrypt_linux("v10", b"peanuts", "from-peanuts", Some(".v10.example")),
        ),
        (
            ".v11.example",
            chromium_encrypt_linux(
                "v11",
                b"keyring-secret",
                "from-keyring",
                Some(".v11.example"),
            ),
        ),
        (
            ".empty10.example",
            chromium_encrypt_linux("v10", b"", "from-empty-10", Some(".empty10.example")),
        ),
        (
            ".empty11.example",
            chromium_encrypt_linux("v11", b"", "from-empty-11", Some(".empty11.example")),
        ),
    ];
    for (host, encrypted) in rows {
        insert_chromium(
            &conn,
            &ChromiumRow {
                host,
                encrypted,
                ..ChromiumRow::default()
            },
        );
    }
    drop(conn);
    profile("chrome/Default", Family::Chromium, dir.to_owned())
}

fn values(read: &brigadier_browser_data::Cookies) -> Vec<&str> {
    read.cookies.iter().map(|c| c.value.as_str()).collect()
}

#[test]
fn linux_cookies_open_with_the_fixed_key_the_keyring_and_the_empty_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = linux_store(dir.path());
    let keys = ChromiumKeys::from_linux_secret(Ok(b"keyring-secret")).unwrap();
    let read = read_cookies_with_keys(&chrome, &keys, NOW).unwrap();
    assert_eq!(
        values(&read.read),
        [
            "from-peanuts",
            "from-keyring",
            "from-empty-10",
            "from-empty-11"
        ]
    );
    assert_eq!(read.read.unreadable, 0);
    assert!(read.app_bound_domains.is_empty());
}

#[test]
fn linux_with_no_keyring_reads_what_it_can_and_says_why_the_rest_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let chrome = linux_store(dir.path());
    let keys = ChromiumKeys::from_linux_secret(Err(SecretError::NoStore)).unwrap();
    assert_eq!(keys.v11_failure(), Some(&SecretError::NoStore));
    let read = read_cookies_with_keys(&chrome, &keys, NOW).unwrap();
    assert_eq!(
        values(&read.read),
        ["from-peanuts", "from-empty-10", "from-empty-11"]
    );
    assert_eq!(read.read.unreadable, 1);
    assert_eq!(read.read.unreadable_domains, [".v11.example"]);
}

#[test]
fn linux_with_a_locked_keyring_stops_so_the_user_can_unlock_it() {
    assert!(matches!(
        ChromiumKeys::from_linux_secret(Err(SecretError::Locked)),
        Err(Error::Secret(SecretError::Locked))
    ));
}

#[test]
fn secret_errors_read_the_same_on_any_os() {
    let text = Error::Secret(SecretError::NoStore).to_string();
    assert_eq!(text, "cookie key: no key store on this machine");
    assert!(!text.to_lowercase().contains("keychain"));
    let denied = Error::Secret(SecretError::Denied).to_string();
    assert!(!denied.to_lowercase().contains("keychain"));
}

#[test]
fn a_discovered_windows_profile_reads_through_the_public_api() {
    let home = WindowsHome::new();
    let chrome = at(home.local.path(), "Google/Chrome/User Data");
    local_state(&chrome, "{}");
    let default = chrome.join("Default");
    let conn = chromium_cookies_db(&at(&default, "Network/Cookies"), 24);
    insert_chromium(
        &conn,
        &ChromiumRow {
            encrypted: chromium_encrypt_windows(&WINDOWS_KEY, "v", Some(".example.com")),
            ..ChromiumRow::default()
        },
    );
    drop(conn);
    let history = chromium_history_db(&default.join("History"));
    history
        .execute_batch(&format!(
            "INSERT INTO urls (id, url, title, last_visit_time) VALUES (1, 'https://a.example/', 'A', 0);
             INSERT INTO visits (url, visit_time) VALUES (1, {});",
            chromium_time(NOW)
        ))
        .unwrap();
    drop(history);
    std::fs::write(
        default.join("Bookmarks"),
        r#"{"roots":{"bookmark_bar":{"type":"folder","children":[
            {"type":"url","name":"A","url":"https://a.example/"}]}}}"#,
    )
    .unwrap();

    let profiles = discover_layout(&home.layout());
    let [profile] = profiles.as_slice() else {
        panic!("one profile expected, found {}", profiles.len());
    };
    let keys = ChromiumKeys::from_windows_key(WINDOWS_KEY);
    assert_eq!(
        read_cookies_with_keys(profile, &keys, NOW)
            .unwrap()
            .read
            .cookies[0]
            .value,
        "v"
    );
    assert_eq!(
        read_history(profile, 10).unwrap()[0].url,
        "https://a.example/"
    );
    assert_eq!(read_bookmarks(profile).unwrap().bookmarks[0].title, "A");
}

#[test]
fn a_discovered_linux_firefox_reads_its_bookmarks_and_cookies() {
    let fixture = LinuxHome::new();
    let root = at(fixture.home.path(), "snap/firefox/common/.mozilla/firefox");
    let dir = at(&root, "Profiles/abc.default-release");
    let places = firefox_places_db(&dir.join("places.sqlite"));
    firefox_bookmark_roots(&places);
    places
        .execute_batch(
            "INSERT INTO moz_places (id, url) VALUES (1, 'https://a.example/');
             INSERT INTO moz_bookmarks (id, type, fk, parent, position, title, guid) VALUES
               (10, 1, 1, 3, 0, 'A', 'x');",
        )
        .unwrap();
    drop(places);
    let cookies = firefox_cookies_db(&dir.join("cookies.sqlite"), 16, true);
    cookies
        .execute(
            "INSERT INTO moz_cookies (name, value, host, path, expiry) VALUES ('n', 'v', '.a.example', '/', ?1)",
            [(NOW + 86_400) * 1000],
        )
        .unwrap();
    drop(cookies);
    std::fs::write(
        root.join("profiles.ini"),
        "[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/abc.default-release\n",
    )
    .unwrap();

    let profiles = discover_layout(&fixture.layout());
    let [profile] = profiles.as_slice() else {
        panic!("one profile expected, found {}", profiles.len());
    };
    assert_eq!(profile.id, "firefox-snap/abc.default-release");
    assert_eq!(read_bookmarks(profile).unwrap().bookmarks[0].title, "A");
    let read = read_cookies_with_keys(profile, &ChromiumKeys::default(), NOW).unwrap();
    assert_eq!(read.read.cookies[0].value, "v");
}
