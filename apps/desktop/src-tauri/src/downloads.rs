//! Save independent copies without changing the source file or document save grants.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

fn source_bytes(bytes: Option<Vec<u8>>, path: Option<String>) -> Result<Vec<u8>, String> {
    match (bytes, path) {
        (Some(bytes), None) => Ok(bytes),
        (None, Some(path)) => {
            let path = Path::new(&path);
            let metadata = path
                .metadata()
                .map_err(|error| format!("Cannot read file: {error}"))?;
            if !metadata.is_file() {
                return Err("Cannot copy a directory or a non-file source".into());
            }
            std::fs::read(path).map_err(|error| format!("Cannot read file: {error}"))
        }
        _ => Err("Provide exactly one of bytes or path".into()),
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Missing folder"))?;
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(
            ".brigadier-copy-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Reserve the destination exclusively so simultaneous downloads cannot replace each other.
fn save_unique(directory: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let name = Path::new(name);
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let extension = name
        .extension()
        .map(|value| format!(".{}", value.to_string_lossy()))
        .unwrap_or_default();
    for number in 0_u64.. {
        let candidate = directory.join(if number == 0 {
            name.as_os_str().to_owned()
        } else {
            format!("{stem} ({number}){extension}").into()
        });
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                drop(file);
                if let Err(error) = atomic_write(&candidate, bytes) {
                    let _ = std::fs::remove_file(&candidate);
                    return Err(error);
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!()
}

#[tauri::command]
pub async fn save_copy(
    app: tauri::AppHandle,
    name: String,
    bytes: Option<Vec<u8>>,
    path: Option<String>,
    ask: bool,
) -> Result<Option<String>, String> {
    let bytes = source_bytes(bytes, path)?;
    let name = Path::new(&name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Untitled")
        .to_owned();
    let downloads = app
        .path()
        .download_dir()
        .map_err(|error| format!("Cannot find Downloads: {error}"))?;
    let destination = if ask {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title("Download image")
            .set_directory(&downloads)
            .set_file_name(&name);
        if let Some(window) = app.get_webview_window(crate::shell::MAIN_WINDOW) {
            dialog = dialog.set_parent(&window);
        }
        dialog.save_file(move |path| {
            let _ = tx.send(path);
        });
        let Some(path) = rx
            .await
            .ok()
            .flatten()
            .and_then(|path| path.into_path().ok())
        else {
            return Ok(None);
        };
        Some(path)
    } else {
        None
    };
    let path = tauri::async_runtime::spawn_blocking(move || {
        if let Some(path) = destination {
            atomic_write(&path, &bytes)?;
            Ok(path)
        } else {
            std::fs::create_dir_all(&downloads)?;
            save_unique(&downloads, &name, &bytes)
        }
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error: std::io::Error| format!("Cannot save copy: {error}"))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn downloads_unique_names_and_atomic_cleanup() {
        let root = std::env::temp_dir().join(format!(
            "20261015-0226-rebuild-wow-w33lb-downloads-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        for (number, name) in ["image.png", "image (1).png", "image (2).png"]
            .iter()
            .enumerate()
        {
            let bytes = [number as u8, 0, 255];
            let path = save_unique(&root, "image.png", &bytes).unwrap();
            assert_eq!(path.file_name().unwrap(), *name);
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
        let directory = root.join("directory");
        std::fs::create_dir(&directory).unwrap();
        assert!(
            source_bytes(None, Some(directory.to_string_lossy().into_owned()))
                .unwrap_err()
                .contains("directory")
        );
        assert!(
            source_bytes(
                None,
                Some(root.join("missing").to_string_lossy().into_owned())
            )
            .is_err()
        );
        assert!(source_bytes(None, None).is_err());
        assert!(source_bytes(Some(vec![]), Some("unused".into())).is_err());
        assert!(atomic_write(&directory, b"cannot replace directory").is_err());
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            4,
            "no temporary file remains after failed rename"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
