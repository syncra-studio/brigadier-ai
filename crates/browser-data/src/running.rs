//! Whether a profile's browser is open, so the app can ask the user to quit it first.

use std::path::Path;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::{Os, Profile};

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

/// On Linux the launcher on `$PATH` is often a script or a symlink, so a Chromium browser's
/// process is recognised by its executable's name. Channels share a name, which at worst asks the
/// user to quit a browser they didn't need to.
const LINUX_PROCESS_NAMES: &[(&str, &[&str])] = &[
    ("Google Chrome", &["chrome"]),
    ("Google Chrome Beta", &["chrome"]),
    ("Google Chrome Dev", &["chrome"]),
    ("Chromium", &["chromium", "chrome"]),
    ("Microsoft Edge", &["msedge"]),
    ("Microsoft Edge Beta", &["msedge"]),
    ("Microsoft Edge Dev", &["msedge"]),
    ("Brave", &["brave"]),
    ("Brave Beta", &["brave"]),
    ("Brave Nightly", &["brave"]),
    ("Vivaldi", &["vivaldi-bin", "vivaldi"]),
    ("Opera", &["opera"]),
    ("Firefox", &["firefox-bin", "firefox-esr"]),
];

/// On Windows, Opera's launcher is not the process that runs: that lives in a folder per version.
/// A running Chromium browser also holds its files shut, which reads as [`crate::Error::Locked`].
const WINDOWS_PROCESS_NAMES: &[(&str, &[&str])] =
    &[("Opera", &["opera"]), ("Opera GX", &["opera"])];

/// Whether the browser that owns `profile` is running: any process inside its app bundle
/// (wherever that bundle was moved), running its executable, or with one of its known executable
/// names.
pub fn running(profile: &Profile) -> bool {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    system.processes().values().any(|process| {
        matches(
            Os::current(),
            profile,
            process.exe(),
            &process.name().to_string_lossy(),
        )
    })
}

/// Whether one process, by its executable path and name, belongs to `profile`'s browser.
fn matches(os: Os, profile: &Profile, exe: Option<&Path>, name: &str) -> bool {
    if let (Some(exe), Some(app)) = (exe, profile.app_path.as_deref()) {
        if exe.starts_with(app) {
            return true;
        }
        // Windows paths don't care about case, and the two here come from different places.
        if os == Os::Windows
            && exe
                .to_string_lossy()
                .eq_ignore_ascii_case(&app.to_string_lossy())
        {
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
    let lowered;
    let name = if os == Os::Windows {
        lowered = name.to_ascii_lowercase();
        lowered.strip_suffix(".exe").unwrap_or(&lowered)
    } else {
        name
    };
    // A Linux browser's Snap and Flatpak installs run under the same names.
    let browser = profile
        .browser
        .strip_suffix(" (Snap)")
        .or_else(|| profile.browser.strip_suffix(" (Flatpak)"))
        .unwrap_or(&profile.browser);
    let for_os: &[(&str, &[&str])] = match os {
        Os::Macos => &[],
        Os::Windows => WINDOWS_PROCESS_NAMES,
        Os::Linux => LINUX_PROCESS_NAMES,
    };
    PROCESS_NAMES
        .iter()
        .chain(for_os)
        .filter(|(known, _)| *known == browser)
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
            local_state: None,
            linux_secret_application: None,
            has_cookies: true,
            has_history: true,
            has_bookmarks: true,
            readable: true,
            is_default: true,
        }
    }

    #[test]
    fn matches_by_bundle_path_moved_bundle_and_name() {
        let dia = profile("Dia", Some("/Applications/Dia.app"));
        let exe = Path::new("/Applications/Dia.app/Contents/MacOS/Dia");
        assert!(matches(Os::Macos, &dia, Some(exe), "Dia"));
        let moved = Path::new("/Users/me/Applications/Dia.app/Contents/MacOS/Dia");
        assert!(matches(Os::Macos, &dia, Some(moved), "Dia"));
        let other = Path::new("/Applications/Diary.app/Contents/MacOS/Dia");
        assert!(!matches(Os::Macos, &dia, Some(other), "Dia"));
        assert!(!matches(Os::Macos, &dia, None, "Dia"));

        let firefox = profile("Firefox", Some("/Applications/Firefox.app"));
        let dev = Path::new("/Applications/Firefox Developer Edition.app/Contents/MacOS/firefox");
        assert!(matches(Os::Macos, &firefox, Some(dev), "firefox"));
        assert!(!matches(Os::Macos, &firefox, None, "zen"));
    }

    #[test]
    fn windows_matches_by_executable_ignoring_case_and_by_name() {
        let chrome = profile(
            "Google Chrome",
            Some(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
        );
        let exe = Path::new(r"C:\PROGRAM FILES\Google\Chrome\Application\chrome.exe");
        assert!(matches(Os::Windows, &chrome, Some(exe), "chrome.exe"));
        let other = Path::new(r"C:\Program Files\Other\chrome.exe");
        assert!(!matches(Os::Windows, &chrome, Some(other), "chrome.exe"));

        let firefox = profile("Firefox", None);
        assert!(matches(Os::Windows, &firefox, None, "firefox.exe"));
        assert!(matches(Os::Windows, &firefox, None, "Firefox.EXE"));
        let opera = profile("Opera", None);
        assert!(matches(Os::Windows, &opera, None, "opera.exe"));
        assert!(!matches(Os::Macos, &opera, None, "opera"));
    }

    #[test]
    fn linux_matches_scripted_launchers_and_packaged_installs_by_name() {
        let chrome = profile("Google Chrome", Some("/usr/bin/google-chrome"));
        let real = Path::new("/opt/google/chrome/chrome");
        assert!(matches(Os::Linux, &chrome, Some(real), "chrome"));
        assert!(!matches(Os::Linux, &chrome, Some(real), "chromium"));
        // Other OSes keep their exact behaviour: no process is a browser by that name alone.
        assert!(!matches(Os::Macos, &chrome, Some(real), "chrome"));

        let snap = profile("Firefox (Snap)", Some("/snap/bin/firefox"));
        let real = Path::new("/snap/firefox/123/usr/lib/firefox/firefox");
        assert!(matches(Os::Linux, &snap, Some(real), "firefox"));
        let flatpak = profile("Brave (Flatpak)", Some("/x/com.brave.Browser"));
        assert!(matches(Os::Linux, &flatpak, None, "brave"));
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
