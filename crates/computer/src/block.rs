//! The hard block list (§5): targets no worker may observe or act on, enforced in the engine,
//! not in a prompt.

use std::collections::HashSet;

/// What the block list knows about a target.
#[derive(Debug, Clone, Default)]
pub struct TargetFacts<'a> {
    pub pid: i32,
    pub bundle_id: Option<&'a str>,
    pub app_name: &'a str,
    /// The app bundle's path on disk.
    pub bundle_path: Option<&'a str>,
    /// The window's title, when the target is a window.
    pub window_title: Option<&'a str>,
    pub window: Option<u32>,
}

const PASSWORD_MANAGERS: &[&str] = &[
    "com.1password.1password",
    "com.agilebits.onepassword7",
    "com.bitwarden.desktop",
    "com.dashlane.dashlanephonefinal",
    "org.keepassxc.keepassxc",
    "com.lastpass.lastpassmacdesktop",
    "com.apple.Passwords",
];

const SYSTEM_SECURITY: &[&str] = &[
    "com.apple.keychainaccess",
    "com.apple.SecurityAgent",
    "com.apple.LocalAuthentication.UIAgent",
    "com.apple.coreautha",
];

const TERMINALS: &[&str] = &[
    "com.apple.Terminal",
    "com.googlecode.iterm2",
    "com.cmuxterm.app",
    "com.mitchellh.ghostty",
    "dev.warp.Warp-Stable",
    "org.alacritty",
    "net.kovidgoyal.kitty",
    "com.github.wez.wezterm",
];

/// System Settings panes that are off limits, matched on the window title.
const SETTINGS_PANES: &[&str] = &[
    "Privacy & Security",
    "Users & Groups",
    "Passwords",
    "Login Items",
];
const SETTINGS_BUNDLE: &str = "com.apple.systempreferences";

/// Why a terminal is refused; a terminal the session launches itself is allowed.
pub const TERMINAL_NOT_LAUNCHED: &str = "a terminal the session didn't launch";

/// The installed app; dev builds the session launched are allowed (§5).
const INSTALLED_BRIGADIER: &str = "/Applications/Brigadier.app";

#[derive(Debug, Clone, Default)]
pub struct BlockList {
    /// The Brigadier instance hosting this session.
    pub host_pid: Option<i32>,
    pub host_bundle_path: Option<String>,
    /// Terminal windows this session launched, which it may drive.
    pub launched_windows: HashSet<u32>,
    pub launched_pids: HashSet<i32>,
}

impl BlockList {
    /// `Some(reason)` when the target is blocked.
    pub fn check(&self, t: &TargetFacts<'_>) -> Option<&'static str> {
        let bundle = t.bundle_id.unwrap_or("");
        if PASSWORD_MANAGERS.contains(&bundle) || t.app_name.eq_ignore_ascii_case("1Password") {
            return Some("password manager");
        }
        if SYSTEM_SECURITY.contains(&bundle) {
            return Some("keychain or system authentication");
        }
        if bundle == SETTINGS_BUNDLE
            && let Some(title) = t.window_title
            && SETTINGS_PANES.iter().any(|p| title.contains(p))
        {
            return Some("a System Settings security pane");
        }
        if TERMINALS.contains(&bundle) {
            let launched = self.launched_pids.contains(&t.pid)
                && t.window.is_none_or(|w| self.launched_windows.contains(&w));
            if !launched {
                return Some(TERMINAL_NOT_LAUNCHED);
            }
        }
        if self.host_pid == Some(t.pid) {
            return Some("the Brigadier instance running this session");
        }
        if let Some(path) = t.bundle_path {
            let path = path.trim_end_matches('/');
            if path == INSTALLED_BRIGADIER || self.host_bundle_path.as_deref() == Some(path) {
                return Some("the installed Brigadier app");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(bundle: &'a str, name: &'a str) -> TargetFacts<'a> {
        TargetFacts {
            pid: 10,
            bundle_id: Some(bundle),
            app_name: name,
            ..Default::default()
        }
    }

    #[test]
    fn password_managers_and_keychain_are_blocked() {
        let b = BlockList::default();
        assert!(
            b.check(&facts("com.1password.1password", "1Password"))
                .is_some()
        );
        assert!(
            b.check(&facts("com.apple.keychainaccess", "Keychain Access"))
                .is_some()
        );
        assert!(b.check(&facts("com.apple.TextEdit", "TextEdit")).is_none());
    }

    #[test]
    fn only_the_security_panes_of_settings_are_blocked() {
        let b = BlockList::default();
        let mut f = facts(SETTINGS_BUNDLE, "System Settings");
        f.window_title = Some("Privacy & Security");
        assert!(b.check(&f).is_some());
        f.window_title = Some("Appearance");
        assert!(b.check(&f).is_none());
    }

    #[test]
    fn terminals_are_blocked_unless_the_session_launched_that_window() {
        let mut b = BlockList::default();
        let mut f = facts("com.apple.Terminal", "Terminal");
        f.window = Some(5);
        assert!(b.check(&f).is_some());
        b.launched_pids.insert(10);
        b.launched_windows.insert(5);
        assert!(b.check(&f).is_none());
        f.window = Some(6);
        assert!(
            b.check(&f).is_some(),
            "another window of the same terminal stays blocked"
        );
    }

    #[test]
    fn the_host_and_the_installed_app_are_blocked_but_dev_builds_are_not() {
        let b = BlockList {
            host_pid: Some(77),
            ..Default::default()
        };
        let mut f = facts("ai.brigadier.dev", "Brigadier Dev");
        f.bundle_path = Some("/tmp/dev/Brigadier.app");
        assert!(b.check(&f).is_none());
        f.pid = 77;
        assert!(b.check(&f).is_some());
        f.pid = 78;
        f.bundle_path = Some("/Applications/Brigadier.app/");
        assert!(b.check(&f).is_some());
    }
}
