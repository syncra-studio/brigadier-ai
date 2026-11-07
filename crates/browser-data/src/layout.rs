//! Where each OS keeps browsers' data and apps.
//!
//! A [`Layout`] is plain paths, so discovery for any OS can run over a fixture on any other.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// An operating system this crate knows how to read browsers on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

impl Os {
    /// The OS this build runs on. Other Unixes keep browsers where Linux does.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Macos
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// The folders discovery looks in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Layout {
    Macos {
        home: PathBuf,
        /// Where app bundles live.
        app_dirs: Vec<PathBuf>,
    },
    Windows {
        /// `%LOCALAPPDATA%`.
        local_app_data: PathBuf,
        /// `%APPDATA%`.
        roaming_app_data: PathBuf,
        /// Where browsers install under: the Program Files folders and the per-user ones.
        app_dirs: Vec<PathBuf>,
    },
    Linux {
        home: PathBuf,
        /// `$XDG_CONFIG_HOME`, else `~/.config`.
        config_home: PathBuf,
        /// `$PATH`.
        path_dirs: Vec<PathBuf>,
        /// Where Snap puts its launchers.
        snap_bin: Vec<PathBuf>,
        /// Where Flatpak puts its launchers, one named after each app id.
        flatpak_bin: Vec<PathBuf>,
        /// Where app icons live: icon themes, pixmaps, and Snap's own packages.
        icon_dirs: Vec<PathBuf>,
    },
}

impl Layout {
    /// This machine's layout, for the user whose home is `home`.
    pub fn current(home: &Path) -> Self {
        Self::for_os(Os::current(), home, &|name| std::env::var_os(name))
    }

    /// The layout of `os`, reading its environment through `env`.
    pub fn for_os(os: Os, home: &Path, env: &dyn Fn(&str) -> Option<OsString>) -> Self {
        // An empty variable counts as unset, as a relative one does for XDG.
        let var = |name: &str| {
            env(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        match os {
            Os::Macos => Self::Macos {
                home: home.to_owned(),
                app_dirs: crate::app_dirs(home),
            },
            Os::Windows => {
                let local_app_data =
                    var("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"));
                let roaming_app_data =
                    var("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"));
                let mut app_dirs: Vec<PathBuf> =
                    ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
                        .iter()
                        .filter_map(|name| var(name))
                        .collect();
                app_dirs.dedup();
                app_dirs.push(local_app_data.clone());
                app_dirs.push(local_app_data.join("Programs"));
                Self::Windows {
                    local_app_data,
                    roaming_app_data,
                    app_dirs,
                }
            }
            Os::Linux => Self::Linux {
                home: home.to_owned(),
                config_home: var("XDG_CONFIG_HOME")
                    .filter(|dir| dir.is_absolute())
                    .unwrap_or_else(|| home.join(".config")),
                path_dirs: env("PATH")
                    .map(|path| std::env::split_paths(&path).collect())
                    .unwrap_or_default(),
                snap_bin: vec![
                    PathBuf::from("/snap/bin"),
                    PathBuf::from("/var/lib/snapd/snap/bin"),
                ],
                flatpak_bin: vec![
                    PathBuf::from("/var/lib/flatpak/exports/bin"),
                    home.join(".local/share/flatpak/exports/bin"),
                ],
                icon_dirs: vec![
                    home.join(".local/share/icons"),
                    PathBuf::from("/usr/share/icons"),
                    PathBuf::from("/usr/share/pixmaps"),
                    PathBuf::from("/var/lib/flatpak/exports/share/icons"),
                    home.join(".local/share/flatpak/exports/share/icons"),
                    PathBuf::from("/snap"),
                ],
            },
        }
    }
}

/// `base` joined with a `/`-separated path, in the separators of the OS it runs on.
pub(crate) fn join_rel(base: &Path, relative: &str) -> PathBuf {
    let mut path = base.to_owned();
    path.extend(relative.split('/'));
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_come_from_the_environment_or_the_home() {
        let home = Path::new("home");
        let env = |name: &str| match name {
            "LOCALAPPDATA" => Some(OsString::from("local")),
            "APPDATA" => Some(OsString::from("roaming")),
            "ProgramFiles" | "ProgramW6432" => Some(OsString::from("pf")),
            "ProgramFiles(x86)" => Some(OsString::from("pf86")),
            _ => None,
        };
        assert_eq!(
            Layout::for_os(Os::Windows, home, &env),
            Layout::Windows {
                local_app_data: PathBuf::from("local"),
                roaming_app_data: PathBuf::from("roaming"),
                app_dirs: vec![
                    PathBuf::from("pf"),
                    PathBuf::from("pf86"),
                    PathBuf::from("local"),
                    join_rel(Path::new("local"), "Programs"),
                ],
            }
        );
        let Layout::Windows {
            local_app_data,
            roaming_app_data,
            ..
        } = Layout::for_os(Os::Windows, home, &|_| Some(OsString::new()))
        else {
            panic!("not the Windows layout");
        };
        assert_eq!(local_app_data, join_rel(home, "AppData/Local"));
        assert_eq!(roaming_app_data, join_rel(home, "AppData/Roaming"));
    }

    #[test]
    fn linux_config_home_must_be_absolute() {
        let home = tempfile::tempdir().unwrap();
        let config = |value: &str| {
            let value = OsString::from(value);
            let env = move |name: &str| (name == "XDG_CONFIG_HOME").then(|| value.clone());
            match Layout::for_os(Os::Linux, home.path(), &env) {
                Layout::Linux { config_home, .. } => config_home,
                other => panic!("not the Linux layout: {other:?}"),
            }
        };
        assert_eq!(config("relative/dir"), home.path().join(".config"));
        let absolute = home.path().join("xdg");
        assert_eq!(config(absolute.to_str().unwrap()), absolute);
    }

    #[test]
    fn current_matches_the_build() {
        let layout = Layout::current(Path::new("/home"));
        #[cfg(target_os = "macos")]
        assert!(matches!(layout, Layout::Macos { .. }));
        #[cfg(windows)]
        assert!(matches!(layout, Layout::Windows { .. }));
        #[cfg(target_os = "linux")]
        assert!(matches!(layout, Layout::Linux { .. }));
    }
}
