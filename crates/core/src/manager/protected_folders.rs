//! The folders macOS asks the user about before an app may read them. A scan the user didn't
//! ask for (the first run's suggestions, Add project's list) never looks into them: a folder
//! the user chose themselves (a project's) still counts, and so does anything inside it.
//!
//! From Apple Platform Security, "Controlling app access to files in macOS": apps need the
//! user's consent before reading Documents, Downloads, Desktop, iCloud Drive, network volumes
//! and removable volumes; access to the whole disk needs Full Disk Access, which guards the
//! other apps' data (Mail, Messages, Safari…) kept in `~/Library`, where macOS 14 and later
//! also ask before an app reads another app's container.

use std::collections::VecDeque;
use std::path::{Component, Path, PathBuf};

/// Below the home folder: Desktop, Documents, Downloads; `~/Library` holds iCloud Drive
/// (`Library/Mobile Documents`) and the other apps' data.
const IN_HOME: &[&str] = &["Desktop", "Documents", "Downloads", "Library"];
/// Every volume but the startup disk is mounted here: removable and network volumes.
const VOLUMES: &str = "/Volumes";

/// The guarded folders of one home folder, less the ones the user chose.
#[derive(Debug, Clone, Default)]
pub(super) struct ProtectedFolders {
    roots: Vec<PathBuf>,
    chosen: Vec<PathBuf>,
}

impl ProtectedFolders {
    /// This computer's: macOS's guarded folders there, none elsewhere.
    pub(super) fn here(home: Option<&Path>, chosen: impl IntoIterator<Item = PathBuf>) -> Self {
        if cfg!(target_os = "macos") {
            Self::macos(home, chosen)
        } else {
            Self::default()
        }
    }

    pub(super) fn macos(home: Option<&Path>, chosen: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut roots = vec![PathBuf::from(VOLUMES)];
        if let Some(home) = home {
            let home = lexical(home);
            roots.extend(IN_HOME.iter().map(|name| home.join(name)));
        }
        let mut protected = Self {
            roots,
            // Resolving these is allowed: the user explicitly chose them. Keep both names
            // so a project chosen through a link also permits its resolved folder.
            chosen: chosen
                .into_iter()
                .flat_map(|path| {
                    let resolved = path.canonicalize().ok();
                    std::iter::once(lexical(&path)).chain(resolved)
                })
                .collect(),
        };
        // The home folder's ancestors can be links (/var on macOS, for example). Resolve
        // only the unguarded home itself, then guard the same children under both names.
        if let Some(home) = home.and_then(|home| protected.canonicalize(home)) {
            protected
                .roots
                .extend(IN_HOME.iter().map(|name| home.join(name)));
        }
        protected
    }

    /// Check each link hop before resolving a scan folder. Never stat through a link into a
    /// guarded folder: even canonicalizing it can trigger the macOS permission prompt.
    pub(super) fn canonicalize(&self, path: &Path) -> Option<PathBuf> {
        self.canonicalize_with(path, |path| path.canonicalize())
    }

    fn canonicalize_with(
        &self,
        path: &Path,
        canonicalize: impl FnOnce(&Path) -> std::io::Result<PathBuf>,
    ) -> Option<PathBuf> {
        if self.covers(path) {
            return None;
        }
        if self.roots.is_empty() {
            return canonicalize(path).ok();
        }
        let mut pending: VecDeque<_> = path
            .components()
            .map(|part| part.as_os_str().to_owned())
            .collect();
        let mut resolved = PathBuf::new();
        let mut links = 0;
        while let Some(part) = pending.pop_front() {
            match Path::new(&part).components().next()? {
                Component::Prefix(_) | Component::RootDir => resolved.push(part),
                Component::CurDir => {}
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::Normal(_) => {
                    let next = resolved.join(part);
                    let full = lexical(
                        &pending
                            .iter()
                            .fold(next.clone(), |path, part| path.join(part)),
                    );
                    // A chosen folder may require traversing a guarded parent. Only that
                    // route is allowed, never a sibling or a link escaping the choice.
                    let chosen_parent = self
                        .chosen
                        .iter()
                        .any(|chosen| within(&full, chosen) && within(chosen, &next));
                    if self.covers(&full) || (self.covers(&next) && !chosen_parent) {
                        return None;
                    }
                    // lstat/readlink inspect the link itself, without accessing its target.
                    let metadata = std::fs::symlink_metadata(&next).ok()?;
                    if metadata.file_type().is_symlink() {
                        links += 1;
                        if links > 40 {
                            return None;
                        }
                        let target = std::fs::read_link(&next).ok()?;
                        if target.is_absolute() {
                            resolved.clear();
                        }
                        for part in target.components().rev() {
                            pending.push_front(part.as_os_str().to_owned());
                        }
                    } else {
                        if !pending.is_empty() && !metadata.is_dir() {
                            return None;
                        }
                        resolved = next;
                    }
                }
            }
        }
        let path = canonicalize(&resolved).ok()?;
        (!self.covers(&path)).then_some(path)
    }

    /// Whether the user chose this repository or a parent project folder.
    pub(super) fn is_chosen(&self, path: &Path) -> bool {
        self.chosen.iter().any(|chosen| within(path, chosen))
    }

    /// Whether `path` is in a guarded folder the user didn't choose, told from the path alone:
    /// nothing on disk is read, not even to follow a link. A relative path is, as there is no
    /// telling where it is.
    pub(super) fn covers(&self, path: &Path) -> bool {
        if self.roots.is_empty() {
            return false;
        }
        if !path.is_absolute() {
            return true;
        }
        let path = lexical(path);
        self.roots.iter().any(|root| within(&path, root))
            && !self.chosen.iter().any(|chosen| within(&path, chosen))
    }
}

/// `path` with `.` and `..` worked out on the path itself.
fn lexical(path: &Path) -> PathBuf {
    let mut clean = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

/// Whether `path` is `root` or inside it, letters compared as the Mac's disk does (`~/documents`
/// is `~/Documents`).
fn within(path: &Path, root: &Path) -> bool {
    let mut parts = path.components();
    root.components().all(|want| {
        parts.next().is_some_and(|part| {
            part.as_os_str()
                .to_string_lossy()
                .eq_ignore_ascii_case(&want.as_os_str().to_string_lossy())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn links_into_guarded_folders_are_rejected_before_canonicalizing() {
        let home = std::env::temp_dir().join(format!(
            "20261018-1945-issues-run-2-w39-protected-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(home.join("Documents/code")).unwrap();
        let home = home.canonicalize().unwrap();
        std::os::unix::fs::symlink("Documents/code", home.join("code")).unwrap();
        std::os::unix::fs::symlink("code", home.join("chain")).unwrap();
        std::fs::create_dir_all(home.join("plain")).unwrap();
        std::os::unix::fs::symlink("../Documents/code", home.join("plain/relative")).unwrap();
        std::os::unix::fs::symlink(&home, home.join("home-alias")).unwrap();
        let protected = ProtectedFolders::macos(Some(&home), []);
        let mut canonicalized = Vec::new();
        for path in ["code", "chain", "plain/relative", "home-alias/code"] {
            assert!(
                protected
                    .canonicalize_with(&home.join(path), |path| {
                        canonicalized.push(path.to_owned());
                        path.canonicalize()
                    })
                    .is_none()
            );
        }
        let alias = home.join("home-alias");
        let alias_protected = ProtectedFolders::macos(Some(&alias), []);
        assert!(
            alias_protected
                .canonicalize_with(&alias.join("code"), |path| {
                    canonicalized.push(path.to_owned());
                    path.canonicalize()
                })
                .is_none()
        );
        std::os::unix::fs::symlink("cycle", home.join("cycle")).unwrap();
        assert!(
            protected
                .canonicalize_with(&home.join("cycle"), |path| {
                    canonicalized.push(path.to_owned());
                    path.canonicalize()
                })
                .is_none()
        );
        std::os::unix::fs::symlink("../other", home.join("Documents/code/escape")).unwrap();
        let picked = ProtectedFolders::macos(Some(&home), [home.join("Documents/code")]);
        assert!(
            picked
                .canonicalize_with(&home.join("code/escape"), |path| {
                    canonicalized.push(path.to_owned());
                    path.canonicalize()
                })
                .is_none()
        );
        // Parent components must be applied after links, as the filesystem does.
        assert_eq!(
            picked.canonicalize(&home.join("code/../code")),
            Some(home.join("Documents/code")),
        );
        std::fs::remove_dir_all(&home).unwrap();
        assert!(canonicalized.is_empty(), "resolved {canonicalized:?}");
    }

    #[cfg(unix)]
    #[test]
    fn git_links_are_rejected_before_target_metadata() {
        let home = std::env::temp_dir().join(format!(
            "20261018-1945-issues-run-2-w41-git-links-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(home.join("Documents/git")).unwrap();
        std::fs::create_dir_all(home.join("code/app")).unwrap();
        let home = home.canonicalize().unwrap();
        std::os::unix::fs::symlink("../../Documents/git", home.join("code/app/.git")).unwrap();
        std::os::unix::fs::symlink("app/.git", home.join("code/hop")).unwrap();
        let protected = ProtectedFolders::macos(Some(&home), []);
        let mut reads = Vec::new();
        for path in ["code/app/.git", "code/hop", "code/app/.git/HEAD"] {
            assert!(
                protected
                    .canonicalize_with(&home.join(path), |path| {
                        reads.push(path.to_owned());
                        path.canonicalize()
                    })
                    .is_none()
            );
        }
        std::fs::remove_dir_all(&home).unwrap();
        assert!(reads.is_empty(), "must not stat through .git: {reads:?}");
    }

    #[cfg(unix)]
    #[test]
    fn the_guarded_folders_and_all_inside_them_are_covered_but_the_ones_the_user_chose() {
        let home = Path::new("/Users/someone");
        let chosen = home.join("Documents/picked");
        let protected = ProtectedFolders::macos(Some(home), [chosen.clone()]);
        for path in [
            "/Users/someone/Documents",
            "/Users/someone/Documents/app",
            "/Users/someone/documents/app",
            "/Users/someone/Desktop/notes",
            "/Users/someone/Downloads/repo",
            "/Users/someone/Library/Mobile Documents/com~apple~CloudDocs/app",
            "/Users/someone/Library/Containers/com.example.app/Data",
            "/Users/someone/code/../Documents/app",
            "/Users/someone/Documents/picked/../other",
            "/Volumes/USB/app",
            "relative/app",
        ] {
            assert!(protected.covers(Path::new(path)), "{path} is guarded");
        }
        for path in [
            "/Users/someone",
            "/Users/someone/code/app",
            "/Users/someone/Documentsy",
            "/Users/someone/Documents/picked",
            "/Users/someone/Documents/picked/sub",
            "/Users/other/Documents/app",
            "/Volumesy",
        ] {
            assert!(!protected.covers(Path::new(path)), "{path} is not guarded");
        }
        assert!(!ProtectedFolders::default().covers(Path::new("/Users/someone/Documents")));
    }
}
