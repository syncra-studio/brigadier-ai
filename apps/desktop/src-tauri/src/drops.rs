//! Files dropped on the main window where the window takes them from the page (macOS: there
//! the page gets no drag of files, nor their paths). The page routes each drop by where it
//! landed (`FolderDropZone`); from here it learns whether Shift was held, which paths are
//! folders, and the bytes of the files it attaches, for the last drop's paths only.
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use brigadier_core::MAX_ATTACHMENT_BYTES;
use serde::Serialize;

/// The last drop on the window: its paths, and whether Shift was held as it landed.
struct Dropped {
    paths: Vec<PathBuf>,
    shift: bool,
}

static LAST_DROP: Mutex<Option<Dropped>> = Mutex::new(None);

/// One path of a drop.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DropEntry {
    path: String,
    folder: bool,
}

/// What the page reads of the last drop.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LastDrop {
    shift: bool,
    entries: Vec<DropEntry>,
}

/// Notes a drop as the window gets it, before the page hears of it.
pub fn record(paths: &[PathBuf]) {
    let dropped = Dropped {
        paths: paths.to_vec(),
        shift: shift_held(),
    };
    *LAST_DROP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(dropped);
}

/// Whether Shift is held right now.
fn shift_held() -> bool {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSEvent, NSEventModifierFlags};
        NSEvent::modifierFlags_class().contains(NSEventModifierFlags::Shift)
    }
    #[cfg(not(target_os = "macos"))]
    false
}

/// Whether Shift is held, while files are dragged over the window.
#[tauri::command]
pub fn drag_shift() -> bool {
    shift_held()
}

/// The last drop: whether Shift was held and which of its paths are folders.
#[tauri::command]
pub fn last_drop() -> Option<LastDrop> {
    let last = LAST_DROP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    last.as_ref().map(|dropped| LastDrop {
        shift: dropped.shift,
        entries: dropped
            .paths
            .iter()
            .map(|path| DropEntry {
                path: path.to_string_lossy().into_owned(),
                folder: path.is_dir(),
            })
            .collect(),
    })
}

/// The bytes of a file of the last drop, to attach.
#[tauri::command]
pub fn read_dropped(path: String) -> Result<tauri::ipc::Response, String> {
    let last = LAST_DROP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let paths = last.as_ref().map_or(&[][..], |dropped| &dropped.paths[..]);
    dropped_file(paths, Path::new(&path)).map(tauri::ipc::Response::new)
}

/// `path`'s bytes, when it is a file among `dropped` no larger than an attachment may be;
/// nothing else is read.
fn dropped_file(dropped: &[PathBuf], path: &Path) -> Result<Vec<u8>, String> {
    if !dropped.iter().any(|candidate| candidate == path) {
        return Err("Only a dropped file can be read".into());
    }
    if !path.is_file() {
        return Err(format!("{} isn't a file", path.display()));
    }
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let too_large = || format!("{name} is too large to attach (maximum 10 MB)");
    let cannot = |error: std::io::Error| format!("Cannot read {name}: {error}");
    let file = std::fs::File::open(path).map_err(cannot)?;
    if file.metadata().map_err(cannot)?.len() > MAX_ATTACHMENT_BYTES as u64 {
        return Err(too_large());
    }
    // Read no further than the limit, should the file have grown since.
    let mut bytes = Vec::new();
    file.take(MAX_ATTACHMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(cannot)?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(too_large());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::dropped_file;

    /// A dropped file is read; a path that wasn't dropped, a dropped folder, or a file larger
    /// than an attachment may be, is not.
    #[test]
    fn only_a_dropped_file_is_read() {
        let root = std::env::temp_dir().join(format!("brigadier-drops-{}", std::process::id()));
        let folder = root.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let (file, other) = (root.join("notes.txt"), root.join("other.txt"));
        std::fs::write(&file, "notes").unwrap();
        std::fs::write(&other, "secret").unwrap();
        let large = root.join("large.bin");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(super::MAX_ATTACHMENT_BYTES as u64 + 1)
            .unwrap();
        let dropped = [file.clone(), folder.clone(), large.clone()];
        let read = dropped_file(&dropped, &file);
        let undropped = dropped_file(&dropped, &other);
        let folder_read = dropped_file(&dropped, &folder);
        let large_read = dropped_file(&dropped, &large);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(read.unwrap(), b"notes");
        assert!(undropped.is_err());
        assert!(folder_read.is_err());
        assert_eq!(
            large_read.unwrap_err(),
            "large.bin is too large to attach (maximum 10 MB)"
        );
    }
}
