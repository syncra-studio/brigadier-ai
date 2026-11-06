//! Browser icons on Windows and Linux, which keep them as image files. macOS asks the system.

use std::path::{Path, PathBuf};

use crate::Layout;

/// Icon sizes worth showing at 32 points on a sharp screen, best first.
const SIZES: [&str; 6] = ["64x64", "128x128", "96x96", "72x72", "256x256", "48x48"];

/// The PNG file of the app at `app`, on `layout`. On macOS, `None`: its icons are in the bundle in
/// a format the system reads.
pub fn app_icon_file(layout: &Layout, app: &Path) -> Option<PathBuf> {
    let found = match layout {
        Layout::Macos { .. } => None,
        Layout::Windows { .. } => windows_icon(app),
        Layout::Linux { icon_dirs, .. } => linux_icon(icon_dirs, app),
    };
    found.filter(|path| is_png(path))
}

fn is_png(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; 8];
    std::fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && magic == *b"\x89PNG\r\n\x1a\n"
}

/// Chromium browsers put tile images in `<app folder>/<version>/VisualElements`, Firefox in
/// `<app folder>/browser/VisualElements`. Windows' own icons sit inside the `.exe`, which would
/// take GDI to get at; this finds an image without it. Browsers with neither show no icon.
fn windows_icon(app: &Path) -> Option<PathBuf> {
    let folder = app.parent()?;
    let mut folders = vec![folder.join("browser")];
    let mut versions: Vec<PathBuf> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("VisualElements").is_dir())
        .collect();
    versions.sort();
    versions.reverse();
    folders.splice(0..0, versions);
    for folder in folders {
        let elements = folder.join("VisualElements");
        for name in [
            "Logo.png",
            "VisualElements_150.png",
            "SmallLogo.png",
            "VisualElements_70.png",
        ] {
            let path = elements.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// A launcher is named after its icon in the freedesktop icon theme, or after its Flatpak app id.
/// Some Snap launchers aren't; their packages hold it in `meta/gui`.
fn linux_icon(icon_dirs: &[PathBuf], app: &Path) -> Option<PathBuf> {
    let name = app.file_name()?.to_str()?;
    // `google-chrome-stable` and the like install their icon under the plain name.
    let names = [name, name.strip_suffix("-stable").unwrap_or(name)];
    for dir in icon_dirs {
        for name in names {
            for size in SIZES {
                let path = dir
                    .join("hicolor")
                    .join(size)
                    .join("apps")
                    .join(format!("{name}.png"));
                if path.is_file() {
                    return Some(path);
                }
            }
            let path = dir.join(format!("{name}.png"));
            if path.is_file() {
                return Some(path);
            }
            let path = dir.join(name).join("current/meta/gui/icon.png");
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}
