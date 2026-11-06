//! A project's icon, found among the files of its first repository on this machine; nothing goes
//! over the network. Repositories are untrusted: a file counts only when it really lies inside the
//! repository (a symlink out of it doesn't), is a regular file under a size cap, and its first
//! bytes say it is an image. The app draws the answer through `<img>`, so an SVG's scripts don't
//! run.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::site_icons::image_kind;

/// An icon larger than this isn't read; the search goes on to the next file.
const ICON_BYTES: u64 = 512 * 1024;
/// Square marks first, a possibly wide logo last.
const NAMES: [&str; 3] = ["favicon", "icon", "logo"];
const FOLDERS: [&str; 6] = ["", "public", "static", "app", "src/app", "assets"];
const EXTENSIONS: [&str; 7] = ["svg", "png", "ico", "webp", "jpg", "jpeg", "gif"];

/// The first icon in `repo` as a `data:` URL: every folder for each name, in the order above.
/// `None` when there is none, or the repository can't be read.
pub fn find_icon(repo: &Path) -> Option<String> {
    let root = repo.canonicalize().ok()?;
    NAMES.iter().find_map(|name| {
        FOLDERS.iter().find_map(|folder| {
            EXTENSIONS.iter().find_map(|extension| {
                read_icon(
                    &root,
                    &root.join(folder).join(format!("{name}.{extension}")),
                )
            })
        })
    })
}

/// The image at `path` as a `data:` URL, when it passes the checks above.
fn read_icon(root: &Path, path: &Path) -> Option<String> {
    let real = path.canonicalize().ok()?;
    if !real.starts_with(root) {
        return None;
    }
    // Looked at before opening: opening a named pipe would wait for a writer.
    let meta = std::fs::metadata(&real).ok()?;
    if !meta.is_file() || meta.len() > ICON_BYTES {
        return None;
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&real)
        .ok()?
        .take(ICON_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > ICON_BYTES {
        return None;
    }
    let mime = image_kind(&bytes, "")?;
    Some(format!("data:{mime};base64,{}", BASE64.encode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n";
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A new empty folder, gone when dropped.
    struct Repo(std::path::PathBuf);

    impl Repo {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repo() -> Repo {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "brigadier-project-icons-{}-{next}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Repo(path)
    }

    #[test]
    fn a_repository_with_no_icon_has_none() {
        let dir = repo();
        write(dir.path(), "README.md", b"# hi");
        assert_eq!(find_icon(dir.path()), None);
        assert_eq!(find_icon(&dir.path().join("gone")), None);
    }

    #[test]
    fn a_favicon_anywhere_comes_before_a_logo_at_the_root() {
        let dir = repo();
        write(dir.path(), "logo.png", PNG);
        assert!(
            find_icon(dir.path())
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        write(dir.path(), "src/app/favicon.svg", SVG);
        assert!(
            find_icon(dir.path())
                .unwrap()
                .starts_with("data:image/svg+xml;base64,")
        );
    }

    #[test]
    fn what_is_no_image_or_too_large_is_passed_over() {
        let dir = repo();
        write(dir.path(), "favicon.png", b"<!doctype html>");
        write(
            dir.path(),
            "public/favicon.png",
            &vec![0x89; ICON_BYTES as usize + 1],
        );
        std::fs::create_dir_all(dir.path().join("favicon.ico")).unwrap();
        assert_eq!(find_icon(dir.path()), None);
        write(dir.path(), "assets/icon.ico", &[0, 0, 1, 0, 1]);
        assert!(
            find_icon(dir.path())
                .unwrap()
                .starts_with("data:image/x-icon;base64,")
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_stays_inside_the_repository() {
        let outside = repo();
        write(outside.path(), "secret.png", PNG);
        let dir = repo();
        std::os::unix::fs::symlink(
            outside.path().join("secret.png"),
            dir.path().join("favicon.png"),
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("public")).unwrap();
        write(outside.path(), "favicon.svg", SVG);
        assert_eq!(find_icon(dir.path()), None);
        // One pointing within it is a file of the repository like any other.
        write(dir.path(), "brand/mark.png", PNG);
        std::os::unix::fs::symlink(
            dir.path().join("brand/mark.png"),
            dir.path().join("icon.png"),
        )
        .unwrap();
        assert!(find_icon(dir.path()).is_some());
    }
}
