//! Save grants belong to the native shell. The frontend supplies text and an opaque ID,
//! never a destination. Grants expire when the app quits; restoring a document asks again.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
pub struct Documents(tokio::sync::Mutex<HashMap<String, SaveGrant>>);

struct SaveGrant {
    path: PathBuf,
    last: Option<Vec<u8>>,
}

impl SaveGrant {
    fn check(&self) -> Result<(), String> {
        let current = match std::fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(format!(
                    "Cannot check the saved file: {error}. Your draft is unchanged."
                ));
            }
        };
        if current != self.last {
            return Err("The file changed outside Brigadier. Your draft was not saved. Save again to choose a new destination or confirm replacement in the save dialog.".into());
        }
        Ok(())
    }
}

/// A sibling temporary file keeps a failed write from truncating the user's original.
fn atomic_write(path: &Path, text: &str) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("missing folder"))?;
    let (temporary, mut file) = loop {
        let candidate = parent.join(format!(
            ".brigadier-save-{}-{}",
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
        if let Ok(metadata) = std::fs::metadata(path) {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[tauri::command]
pub async fn save_document(
    app: tauri::AppHandle,
    documents: State<'_, Documents>,
    id: String,
    text: String,
    directory: Option<String>,
    name: String,
) -> Result<Option<String>, String> {
    // Serialize dialogs and writes, including repeated Cmd+S while a dialog is open.
    let mut grants = documents.0.lock().await;
    let path = if let Some(grant) = grants.get(&id) {
        if let Err(error) = grant.check() {
            grants.remove(&id);
            return Err(error);
        }
        grant.path.clone()
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app.dialog().file().set_title("Save file");
        // These are dialog hints only. No shell expansion and no file write uses them.
        if let Some(directory) = directory {
            dialog = dialog.set_directory(directory);
        }
        let name = Path::new(&name)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("Untitled");
        dialog = dialog.set_file_name(name);
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
        // Follow a chosen link once; the grant then names its target, so atomic replacement
        // preserves the link. New files still use the exact parent chosen in the dialog.
        let path = match path.canonicalize() {
            Ok(target) => target,
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && std::fs::symlink_metadata(&path).is_err() =>
            {
                path
            }
            Err(error) => return Err(format!("Cannot resolve the selected file: {error}")),
        };
        grants.insert(
            id.clone(),
            SaveGrant {
                path: path.clone(),
                last: std::fs::read(&path).ok(),
            },
        );
        path
    };
    let destination = path.clone();
    let written = text.as_bytes().to_vec();
    tauri::async_runtime::spawn_blocking(move || atomic_write(&destination, &text))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    grants.insert(
        id,
        SaveGrant {
            path: path.clone(),
            last: Some(written),
        },
    );
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn forget_document(documents: State<'_, Documents>, id: String) {
    documents.0.lock().await.remove(&id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_replacement_and_failed_rename_preserve_files() {
        let root = std::env::temp_dir().join(format!("document-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("notes.txt");
        std::fs::write(&path, "original").unwrap();
        atomic_write(&path, "edited").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited");
        let directory = root.join("directory");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("keep"), "original").unwrap();
        assert!(atomic_write(&directory, "cannot replace a directory").is_err());
        assert_eq!(
            std::fs::read_to_string(directory.join("keep")).unwrap(),
            "original"
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn grant_rejects_external_changes_and_deletion() {
        let path = std::env::temp_dir().join(format!("document-grant-{}.txt", std::process::id()));
        std::fs::write(&path, "saved").unwrap();
        let grant = SaveGrant {
            path: path.clone(),
            last: Some(b"saved".to_vec()),
        };
        assert!(grant.check().is_ok());
        std::fs::write(&path, "other").unwrap();
        assert!(grant.check().unwrap_err().contains("changed outside"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "other");
        std::fs::remove_file(&path).unwrap();
        assert!(grant.check().is_err());
    }
}
