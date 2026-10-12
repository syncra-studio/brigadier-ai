//! Whether a profile's browser is open, so the app can ask the user to quit it first.

use std::path::Path;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::Profile;

/// Executable names that identify a browser regardless of where or under which bundle it is
/// installed; Firefox's Developer Edition and Nightly share Firefox's profiles.
const PROCESS_NAMES: &[(&str, &[&str])] = &[
    ("Firefox", &["firefox"]),
    ("Zen", &["zen"]),
    ("LibreWolf", &["librewolf"]),
    ("Waterfox", &["waterfox"]),
    ("Floorp", &["floorp"]),
    ("Safari", &["Safari"]),
];

/// Whether the browser that owns `profile` is running: any process inside its app bundle
/// (wherever that bundle was moved), or with one of its known executable names.
pub fn running(profile: &Profile) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    system
        .processes()
        .values()
        .any(|process| matches(profile, process.exe(), &process.name().to_string_lossy()))
}

/// Whether one process, by its executable path and name, belongs to `profile`'s browser.
fn matches(profile: &Profile, exe: Option<&Path>, name: &str) -> bool {
    if let (Some(exe), Some(app)) = (exe, profile.app_path.as_deref()) {
        if exe.starts_with(app) {
            return true;
        }
        if let Some(bundle) = app.file_name() {
            let inside = exe.ancestors().any(|dir| {
                dir.file_name() == Some(bundle) && exe.starts_with(dir.join("Contents/MacOS"))
            });
            if inside {
                return true;
            }
        }
    }
    PROCESS_NAMES
        .iter()
        .filter(|(browser, _)| *browser == profile.browser)
        .any(|(_, names)| names.contains(&name))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::Family;

    fn profile(browser: &str, app: Option<&str>) -> Profile {
        Profile {
            id: "x/Default".to_owned(),
            browser: browser.to_owned(),
            name: "Default".to_owned(),
            family: Family::Chromium,
            dir: PathBuf::from("/nowhere"),
            app_path: app.map(PathBuf::from),
            keychain_service: None,
            keychain_account: None,
            has_cookies: true,
            has_history: true,
            readable: true,
            is_default: true,
        }
    }

    #[test]
    fn matches_by_bundle_path_moved_bundle_and_name() {
        let dia = profile("Dia", Some("/Applications/Dia.app"));
        let exe = Path::new("/Applications/Dia.app/Contents/MacOS/Dia");
        assert!(matches(&dia, Some(exe), "Dia"));
        let moved = Path::new("/Users/me/Applications/Dia.app/Contents/MacOS/Dia");
        assert!(matches(&dia, Some(moved), "Dia"));
        let other = Path::new("/Applications/Diary.app/Contents/MacOS/Dia");
        assert!(!matches(&dia, Some(other), "Dia"));
        assert!(!matches(&dia, None, "Dia"));

        let firefox = profile("Firefox", Some("/Applications/Firefox.app"));
        let dev = Path::new("/Applications/Firefox Developer Edition.app/Contents/MacOS/firefox");
        assert!(matches(&firefox, Some(dev), "firefox"));
        assert!(!matches(&firefox, None, "zen"));
    }

    #[test]
    fn running_does_not_panic() {
        // The test runner itself is not a browser bundle.
        assert!(!running(&profile(
            "Nope",
            Some("/Applications/Brigadier-test-nope.app")
        )));
    }
}
