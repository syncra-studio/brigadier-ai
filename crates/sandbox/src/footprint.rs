//! What a Brigadier app leaves around the system outside its data directory, found only by
//! exact names: the per-app folders the OS and the webview keep under its bundle identifier,
//! and the temp folders its Claude sessions made (named `brigadier-<12 hex>`, this user's,
//! private, and marked with the data directory that made them).

use std::path::PathBuf;

/// Where Claude sessions' own temp folders are (see the Claude adapter).
#[cfg(unix)]
pub const SESSION_TEMP_BASE: &str = "/tmp";

/// A per-app folder or file, with the folder it must stay inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppFolder {
    pub root: PathBuf,
    pub path: PathBuf,
}

/// Whether `identifier` is a Brigadier bundle identifier (`ai.brigadier.app`, or a development
/// one under `ai.brigadier.`), safe to join to a folder: letters, digits, dots and dashes.
pub fn is_brigadier_identifier(identifier: &str) -> bool {
    identifier
        .strip_prefix("ai.brigadier.")
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest.len() <= 100
                && rest.split('.').all(|part| {
                    !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                })
        })
}

/// Every place the OS and the webview keep for the app `identifier`, whether it is there or
/// not. Empty for an identifier that isn't Brigadier's.
pub fn app_folders(identifier: &str) -> Vec<AppFolder> {
    if !is_brigadier_identifier(identifier) {
        return Vec::new();
    }
    let mut folders = Vec::new();
    let mut add = |root: Option<PathBuf>, name: String| {
        if let Some(root) = root {
            folders.push(AppFolder {
                path: root.join(name),
                root,
            });
        }
    };
    #[cfg(target_os = "macos")]
    {
        let library = dirs::home_dir().map(|home| home.join("Library"));
        let under = |part: &str| library.as_ref().map(|library| library.join(part));
        add(under("Application Support"), identifier.to_owned());
        add(under("Caches"), identifier.to_owned());
        add(under("WebKit"), identifier.to_owned());
        add(under("HTTPStorages"), identifier.to_owned());
        add(under("HTTPStorages"), format!("{identifier}.binarycookies"));
        add(under("Preferences"), format!("{identifier}.plist"));
        add(
            under("Saved Application State"),
            format!("{identifier}.savedState"),
        );
        add(under("Logs"), identifier.to_owned());
        add(
            darwin_user_dir("DARWIN_USER_CACHE_DIR"),
            identifier.to_owned(),
        );
        add(
            darwin_user_dir("DARWIN_USER_TEMP_DIR"),
            identifier.to_owned(),
        );
    }
    #[cfg(target_os = "linux")]
    {
        add(dirs::config_dir(), identifier.to_owned());
        add(dirs::data_dir(), identifier.to_owned());
        add(dirs::cache_dir(), identifier.to_owned());
    }
    #[cfg(windows)]
    {
        add(dirs::config_dir(), identifier.to_owned());
        add(dirs::data_local_dir(), identifier.to_owned());
    }
    folders
}

/// Where the system and the webview keep Brigadier apps' rebuildable caches: never an app's
/// persistent website data (local storage, IndexedDB, cookies) or its settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheRoots {
    pub dirs: Vec<CacheDir>,
}

/// A folder holding per-app caches, and how they are named in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDir {
    pub root: PathBuf,
    pub naming: CacheNaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheNaming {
    /// `<root>/<identifier>`, all of it a cache (macOS `~/Library/Caches`, the per-user cache
    /// folder; Linux `~/.cache`).
    Whole,
    /// `<root>/com.apple.WebKit.<process>+<identifier>`: the caches of the app's WebKit
    /// processes (macOS per-user cache folder).
    WebKitProcesses,
    /// `<root>/<identifier>/EBWebView/…`: only WebView2's cache folders in it (Windows
    /// `%LOCALAPPDATA%`).
    WebView2,
}

/// The WebKit processes that keep caches of their own per app.
const WEBKIT_PROCESSES: &[&str] = &[
    "com.apple.WebKit.GPU+",
    "com.apple.WebKit.Networking+",
    "com.apple.WebKit.WebContent+",
];
/// WebView2's rebuildable caches under an app's `EBWebView` folder.
const WEBVIEW2_CACHES: &[&str] = &[
    "Default/Cache",
    "Default/Code Cache",
    "Default/GPUCache",
    "GrShaderCache",
    "ShaderCache",
];

impl CacheRoots {
    /// This user's real cache folders.
    pub fn system() -> Self {
        let mut dirs = Vec::new();
        #[cfg(target_os = "macos")]
        {
            if let Some(home) = dirs::home_dir() {
                dirs.push(CacheDir {
                    root: home.join("Library").join("Caches"),
                    naming: CacheNaming::Whole,
                });
            }
            if let Some(user) = darwin_user_dir("DARWIN_USER_CACHE_DIR") {
                dirs.push(CacheDir {
                    root: user.clone(),
                    naming: CacheNaming::Whole,
                });
                dirs.push(CacheDir {
                    root: user,
                    naming: CacheNaming::WebKitProcesses,
                });
            }
        }
        #[cfg(target_os = "linux")]
        if let Some(cache) = dirs::cache_dir() {
            dirs.push(CacheDir {
                root: cache,
                naming: CacheNaming::Whole,
            });
        }
        #[cfg(windows)]
        if let Some(local) = dirs::data_local_dir() {
            dirs.push(CacheDir {
                root: local,
                naming: CacheNaming::WebView2,
            });
        }
        Self { dirs }
    }

    /// The same places under `home` instead of this user's home: a stand-in for tests and
    /// isolated runs, so they only ever see what they seeded.
    pub fn under(home: &std::path::Path) -> Self {
        let dir = |root: PathBuf, naming| CacheDir { root, naming };
        let dirs = if cfg!(target_os = "macos") {
            let user = home.join("darwin-user-cache");
            vec![
                dir(home.join("Library").join("Caches"), CacheNaming::Whole),
                dir(user.clone(), CacheNaming::Whole),
                dir(user, CacheNaming::WebKitProcesses),
            ]
        } else if cfg!(windows) {
            vec![dir(
                home.join("AppData").join("Local"),
                CacheNaming::WebView2,
            )]
        } else {
            vec![dir(home.join(".cache"), CacheNaming::Whole)]
        };
        Self { dirs }
    }
}

/// A Brigadier app's rebuildable cache folder, with the folder it must stay inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityCache {
    pub identifier: String,
    pub root: PathBuf,
    pub path: PathBuf,
}

/// Every Brigadier app's cache folder under `roots`, by exact names only: the identifier must
/// be one of Brigadier's ([`is_brigadier_identifier`]). Links are never taken for folders.
pub fn identity_caches(roots: &CacheRoots) -> Vec<IdentityCache> {
    let is_dir =
        |path: &std::path::Path| std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir());
    let mut found = Vec::new();
    for CacheDir { root, naming } in &roots.dirs {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let identifier = match naming {
                CacheNaming::Whole | CacheNaming::WebView2 => Some(name.as_str()),
                CacheNaming::WebKitProcesses => WEBKIT_PROCESSES
                    .iter()
                    .find_map(|prefix| name.strip_prefix(prefix)),
            };
            let Some(identifier) = identifier.filter(|id| is_brigadier_identifier(id)) else {
                continue;
            };
            let paths = match naming {
                CacheNaming::WebView2 => WEBVIEW2_CACHES
                    .iter()
                    .map(|part| {
                        part.split('/')
                            .fold(root.join(&name).join("EBWebView"), |path, part| {
                                path.join(part)
                            })
                    })
                    .collect(),
                _ => vec![root.join(&name)],
            };
            for path in paths.into_iter().filter(|path| is_dir(path)) {
                found.push(IdentityCache {
                    identifier: identifier.to_owned(),
                    root: root.clone(),
                    path,
                });
            }
        }
    }
    found
}

/// This user's per-user folder the system names `key` (`getconf DARWIN_USER_CACHE_DIR`).
#[cfg(target_os = "macos")]
fn darwin_user_dir(key: &str) -> Option<PathBuf> {
    let output = std::process::Command::new("/usr/bin/getconf")
        .arg(key)
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let dir = String::from_utf8(output.stdout).ok()?;
    let dir = PathBuf::from(dir.trim());
    (dir.starts_with("/var/folders") || dir.starts_with("/private/var/folders")).then_some(dir)
}

/// A session temp folder: its path and what its owner marker says (`None`: an older Brigadier
/// made it without one).
#[derive(Debug, Clone)]
pub struct SessionTemp {
    pub path: PathBuf,
    pub marker: Option<String>,
}

impl SessionTemp {
    /// Made by the data directory whose instance id is `instance`.
    pub fn made_by(&self, instance: &str) -> bool {
        self.marker
            .as_deref()
            .is_some_and(|marker| marker.lines().next() == Some(instance))
    }
}

/// Session temp folders in [`SESSION_TEMP_BASE`]: `brigadier-<12 lowercase hex>` folders (not
/// links) of this user, readable by nobody else.
#[cfg(unix)]
pub fn session_temp_folders() -> Vec<SessionTemp> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let uid = nix::unistd::getuid().as_raw();
    let base = std::path::Path::new(SESSION_TEMP_BASE);
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_session_dir = name.strip_prefix("brigadier-").is_some_and(|id| {
            id.len() == 12
                && id
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        });
        if !is_session_dir {
            continue;
        }
        let path = base.join(&name);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_dir() || meta.uid() != uid || meta.permissions().mode() & 0o777 != 0o700 {
            continue;
        }
        found.push(SessionTemp {
            marker: crate::removal::read_owner_marker(&path),
            path,
        });
    }
    found
}

#[cfg(not(unix))]
pub fn session_temp_folders() -> Vec<SessionTemp> {
    Vec::new()
}
