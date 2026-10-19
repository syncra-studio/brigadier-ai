//! The folders macOS asks the user about before an app may read them. A scan the user didn't
//! ask for (the first run's suggestions, Add project's list) never looks into them: a folder
//! the user chose themselves (a project's) still counts, and so does anything inside it.
//!
//! From Apple Platform Security, "Controlling app access to files in macOS": apps need the
//! user's consent before reading Documents, Downloads, Desktop, iCloud Drive, network volumes
//! and removable volumes; access to the whole disk needs Full Disk Access, which guards the
//! other apps' data (Mail, Messages, Safari…) kept in `~/Library`, where macOS 14 and later
//! also ask before an app reads another app's container.

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
        Self {
            roots,
            chosen: chosen.into_iter().map(|path| lexical(&path)).collect(),
        }
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
