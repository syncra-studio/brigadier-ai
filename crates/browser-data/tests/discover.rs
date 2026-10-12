//! Profile discovery over a fake home folder.

mod common;

use std::path::Path;

use brigadier_browser_data::{Family, Profile, discover_in, sources_in};
use common::*;

/// An app bundle in `apps` whose Info.plist registers `schemes`.
fn app(apps: &Path, name: &str, schemes: &[&str]) {
    let contents = apps.join(format!("{name}.app/Contents"));
    mkdirs(&contents);
    let schemes: String = schemes
        .iter()
        .map(|s| format!("<string>{s}</string>"))
        .collect();
    std::fs::write(
        contents.join("Info.plist"),
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict>\
             <key>CFBundleURLTypes</key><array><dict><key>CFBundleURLSchemes</key>\
             <array>{schemes}</array></dict></array></dict></plist>\n"
        ),
    )
    .unwrap();
}

fn local_state(dir: &Path, json: &str) {
    mkdirs(dir);
    std::fs::write(dir.join("Local State"), json).unwrap();
}

/// Builds a home with known and unknown Chromium and Gecko browsers, Safari with two Safari 17
/// profiles, and folders that must be ignored.
fn fixture_home(home: &Path, elsewhere: &Path) {
    let support = home.join("Library/Application Support");

    // Google Chrome: two named profiles (one with the newer Network/Cookies), one empty folder.
    let chrome = support.join("Google/Chrome");
    local_state(
        &chrome,
        r#"{"profile":{"info_cache":{"Default":{"name":"Person 1"},"Profile 1":{"name":"Work"},
            "Profile 2":{"name":"Empty"}},"last_used":"Profile 1"}}"#,
    );
    touch(&chrome.join("Default/Cookies"));
    touch(&chrome.join("Default/History"));
    touch(&chrome.join("Profile 1/Network/Cookies"));
    mkdirs(&chrome.join("Profile 2"));

    // Dia: no info_cache, so the folder name is the profile name.
    let dia = support.join("Dia/User Data");
    local_state(&dia, "{}");
    touch(&dia.join("Default/History"));

    // Opera keeps its profile in the root.
    let opera = support.join("com.operasoftware.Opera");
    local_state(&opera, "{}");
    touch(&opera.join("History"));
    touch(&opera.join("Cookies"));

    // An unknown Chromium browser, found by shape.
    let comet = support.join("Comet");
    local_state(
        &comet,
        r#"{"profile":{"info_cache":{"Default":{"name":"Me"}}}}"#,
    );
    touch(&comet.join("Default/History"));
    touch(&comet.join("Default/Cookies"));

    // Helium, known, in its bundle-id folder.
    let helium = support.join("net.imput.helium");
    local_state(&helium, "{}");
    touch(&helium.join("Default/Cookies"));

    // Look-alikes that must be skipped.
    let testing = support.join("Google/Chrome for Testing");
    local_state(&testing, "{}");
    touch(&testing.join("Default/History"));
    let electron = support.join("Slackish");
    local_state(&electron, "{}");
    touch(&electron.join("Cookies"));
    touch(&electron.join("Network/Cookies"));
    touch(&electron.join("Default/Cookies"));
    // An Electron app that keeps a History too, and a Chromium folder whose app is gone.
    let chatty = support.join("Chatty");
    local_state(&chatty, "{}");
    touch(&chatty.join("Default/History"));
    touch(&chatty.join("Default/Cookies"));
    let gone = support.join("Gone Browser");
    local_state(&gone, "{}");
    touch(&gone.join("Default/History"));

    // Firefox: a relative profile, an absolute one (the install default) and one never used.
    let firefox = support.join("Firefox");
    touch(&firefox.join("Profiles/abcd.default-release/places.sqlite"));
    touch(&firefox.join("Profiles/abcd.default-release/cookies.sqlite"));
    touch(&elsewhere.join("efgh.work/cookies.sqlite"));
    mkdirs(&firefox.join("Profiles/zzzz.unused"));
    std::fs::write(
        firefox.join("profiles.ini"),
        format!(
            "[Profile2]\nName=unused\nIsRelative=1\nPath=Profiles/zzzz.unused\n\n\
             [Profile1]\nName=Work\nIsRelative=0\nPath={}\n\n\
             [Profile0]\nName=default-release\nIsRelative=1\n\
             Path=Profiles/abcd.default-release\nDefault=1\n\n\
             [Install4F96D1932A9F858E]\nDefault={}\nLocked=1\n\n\
             [General]\nStartWithLastProfile=1\nVersion=2\n",
            elsewhere.join("efgh.work").display(),
            elsewhere.join("efgh.work").display(),
        ),
    )
    .unwrap();

    // Zen, in its lowercase folder, with the older Default=1 style.
    let zen = support.join("zen");
    touch(&zen.join("Profiles/q1.Default (release)/places.sqlite"));
    std::fs::write(
        zen.join("profiles.ini"),
        "[Profile0]\nName=Default (release)\nIsRelative=1\nPath=Profiles/q1.Default (release)\n\
         Default=1\n",
    )
    .unwrap();

    // An unknown Gecko fork.
    let mercury = support.join("Mercury");
    touch(&mercury.join("Profiles/m.default/cookies.sqlite"));
    std::fs::write(
        mercury.join("profiles.ini"),
        "[Profile0]\nName=default\nIsRelative=1\nPath=Profiles/m.default\n",
    )
    .unwrap();

    // Safari: the default profile plus two Safari 17 profiles, one named in SafariTabs.db.
    touch(&home.join("Library/Safari/History.db"));
    let container = home.join("Library/Containers/com.apple.Safari/Data/Library");
    touch(&container.join("Cookies/Cookies.binarycookies"));
    touch(&container.join("Safari/Profiles/AAAA-1111/History.db"));
    touch(&container.join("WebKit/WebsiteDataStore/AAAA-1111/Cookies/Cookies.binarycookies"));
    touch(&container.join("Safari/Profiles/BBBB-2222/History.db"));
    let tabs = rusqlite::Connection::open(container.join("Safari/SafariTabs.db")).unwrap();
    tabs.execute_batch(
        "CREATE TABLE bookmarks (id INTEGER PRIMARY KEY, title TEXT, external_uuid TEXT);
         INSERT INTO bookmarks (title, external_uuid) VALUES ('Work', 'aaaa-1111'), ('x', NULL);",
    )
    .unwrap();
}

fn summary(profiles: &[Profile]) -> Vec<(&str, &str, &str)> {
    profiles
        .iter()
        .map(|p| (p.id.as_str(), p.browser.as_str(), p.name.as_str()))
        .collect()
}

#[test]
fn finds_exactly_the_fixture_profiles() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    mkdirs(&apps.path().join("Dia.app"));
    mkdirs(&apps.path().join("Safari.app"));
    app(apps.path(), "Comet", &["http", "https"]);
    app(apps.path(), "Chatty", &["chatty"]);
    fixture_home(home.path(), elsewhere.path());

    let profiles = discover_in(home.path(), &[apps.path()]);
    assert_eq!(
        summary(&profiles),
        [
            ("comet/Default", "Comet (Chromium)", "Me"),
            ("dia/Default", "Dia", "Default"),
            ("firefox/abcd.default-release", "Firefox", "default-release"),
            ("firefox/efgh.work", "Firefox", "Work"),
            ("chrome/Default", "Google Chrome", "Person 1"),
            ("chrome/Profile 1", "Google Chrome", "Work"),
            ("helium/Default", "Helium", "Default"),
            ("mercury/m.default", "Mercury (Firefox)", "default"),
            ("opera/Default", "Opera", "Default"),
            ("safari/default", "Safari", "Default"),
            ("safari/BBBB-2222", "Safari", "Profile 2"),
            ("safari/AAAA-1111", "Safari", "Work"),
            ("zen/q1.Default (release)", "Zen", "Default (release)"),
        ]
    );
    let get = |id: &str| profiles.iter().find(|p| p.id == id).unwrap();

    let dia = get("dia/Default");
    assert_eq!(dia.family, Family::Chromium);
    assert_eq!(dia.keychain_service.as_deref(), Some("Dia Safe Storage"));
    assert_eq!(dia.keychain_account.as_deref(), Some("Dia"));
    assert_eq!(
        dia.app_path.as_deref(),
        Some(apps.path().join("Dia.app").as_path())
    );
    assert!(dia.has_history && !dia.has_cookies && dia.readable && dia.is_default);
    assert_eq!(
        dia.dir,
        home.path()
            .join("Library/Application Support/Dia/User Data/Default")
    );

    let chrome = get("chrome/Default");
    assert_eq!(
        chrome.keychain_service.as_deref(),
        Some("Chrome Safe Storage")
    );
    assert_eq!(chrome.keychain_account.as_deref(), Some("Chrome"));
    // Not installed in the fixture, but the bundle name is still known for `running`.
    assert_eq!(
        chrome.app_path.as_deref(),
        Some(apps.path().join("Google Chrome.app").as_path())
    );
    assert!(!chrome.is_default);
    let work = get("chrome/Profile 1");
    assert!(work.has_cookies && !work.has_history && work.is_default);

    let comet = get("comet/Default");
    assert_eq!(
        comet.keychain_service.as_deref(),
        Some("Comet Safe Storage")
    );
    assert_eq!(comet.keychain_account.as_deref(), Some("Comet"));
    assert_eq!(
        comet.app_path.as_deref(),
        Some(apps.path().join("Comet.app").as_path())
    );
    let helium = get("helium/Default");
    assert_eq!(
        helium.keychain_service.as_deref(),
        Some("Helium Storage Key")
    );
    assert_eq!(helium.keychain_account.as_deref(), Some("Helium"));

    let opera = get("opera/Default");
    assert_eq!(
        opera.keychain_service.as_deref(),
        Some("Opera Safe Storage")
    );
    assert_eq!(
        opera.dir,
        home.path()
            .join("Library/Application Support/com.operasoftware.Opera")
    );

    let release = get("firefox/abcd.default-release");
    assert_eq!(release.family, Family::Gecko);
    assert_eq!(release.keychain_service, None);
    assert!(release.has_cookies && release.has_history && !release.is_default);
    let ff_work = get("firefox/efgh.work");
    assert_eq!(ff_work.dir, elsewhere.path().join("efgh.work"));
    assert!(ff_work.has_cookies && !ff_work.has_history && ff_work.is_default);
    assert!(get("zen/q1.Default (release)").is_default);

    let safari = get("safari/default");
    assert_eq!(safari.family, Family::Safari);
    assert!(safari.has_cookies && safari.has_history && safari.readable && safari.is_default);
    assert_eq!(
        safari.app_path.as_deref(),
        Some(apps.path().join("Safari.app").as_path())
    );
    assert!(get("safari/AAAA-1111").has_cookies);
    assert!(!get("safari/BBBB-2222").has_cookies);
}

#[test]
fn safari_is_listed_when_only_the_app_exists() {
    let home = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    assert!(discover_in(home.path(), &[apps.path()]).is_empty());
    mkdirs(&apps.path().join("Safari.app"));
    let profiles = discover_in(home.path(), &[apps.path()]);
    assert_eq!(
        summary(&profiles),
        [("safari/default", "Safari", "Default")]
    );
    assert!(!profiles[0].has_cookies && !profiles[0].has_history && profiles[0].readable);
}

#[cfg(unix)]
#[test]
fn a_gecko_browser_whose_folder_macos_keeps_locked_is_listed_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    let ini = home
        .path()
        .join("Library/Application Support/Firefox/profiles.ini");
    mkdirs(ini.parent().unwrap());
    std::fs::write(
        &ini,
        "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abcd\n",
    )
    .unwrap();
    std::fs::set_permissions(&ini, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Running as root reads it anyway; nothing to check then.
    if std::fs::read(&ini).is_ok() {
        return;
    }
    let profiles = discover_in(home.path(), &[apps.path()]);
    std::fs::set_permissions(&ini, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        summary(&profiles),
        [("firefox/locked", "Firefox", "Default")]
    );
    let firefox = &profiles[0];
    assert_eq!(firefox.family, Family::Gecko);
    assert!(firefox.has_cookies && firefox.has_history && !firefox.readable && firefox.is_default);
}

#[test]
fn sources_are_the_installed_browsers_in_settings_order() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    // Installed: Dia, Safari, Comet, Firefox, Helium. Chrome, Opera, Zen and Mercury leave
    // folders behind without an app.
    for bundle in ["Dia.app", "Safari.app", "Firefox.app", "Helium.app"] {
        mkdirs(&apps.path().join(bundle));
    }
    app(apps.path(), "Comet", &["http", "https"]);
    fixture_home(home.path(), elsewhere.path());

    let sources = sources_in(home.path(), &[apps.path()]);
    let listed: Vec<(&str, &str, Vec<&str>)> = sources
        .iter()
        .map(|source| {
            (
                source.key.as_str(),
                source.name.as_str(),
                source.profiles.iter().map(|p| p.name.as_str()).collect(),
            )
        })
        .collect();
    assert_eq!(
        listed,
        [
            ("helium", "Helium", vec!["Default"]),
            ("dia", "Dia", vec!["Default"]),
            // The install default first.
            ("firefox", "Firefox", vec!["Work", "default-release"]),
            ("safari", "Safari", vec!["Default", "Profile 2", "Work"]),
            ("comet", "Comet (Chromium)", vec!["Me"]),
        ]
    );
    assert_eq!(sources[1].app_path, apps.path().join("Dia.app"));
    assert_eq!(sources[2].family, Family::Gecko);
}

#[test]
fn leftover_folders_without_an_app_list_nothing() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let apps = tempfile::tempdir().unwrap();
    fixture_home(home.path(), elsewhere.path());
    assert!(!discover_in(home.path(), &[apps.path()]).is_empty());
    assert!(sources_in(home.path(), &[apps.path()]).is_empty());
}
