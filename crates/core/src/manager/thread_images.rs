//! Images are opened only through the item of the CLI's thread that named them in this
//! conversation, and a proof (a screenshot or recording the thread attached, `attach_proof`)
//! through its step.

use std::path::{Path, PathBuf};

use brigadier_providers::{ItemStatus, ProviderEvent};

use super::{SessionManager, blocking};
use crate::model::ConversationId;
use crate::work::{OrchestratorStepKind, ThreadItemKind};
use crate::{Error, Result};

/// The file name, without its extension, of the copy kept of generated image `item_id`.
fn kept_name(item_id: &str) -> String {
    item_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn image_io(err: std::io::Error) -> Error {
    Error::Invalid(err.to_string())
}

/// The biggest image the thread keeps: it is sent to the app whole, as base64 in one IPC
/// frame of at most 16 MiB, and base64 takes 4 bytes for every 3.
const MAX_IMAGE: u64 = 11 * 1024 * 1024;

fn mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "bmp" => Some("image/bmp"),
        "tif" | "tiff" => Some("image/tiff"),
        "heic" => Some("image/heic"),
        "mp4" | "m4v" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "log" | "txt" => Some("text/plain"),
        _ => None,
    }
}

/// Where a browser_screenshot was taken, kept beside the picture so `attach_proof` knows it.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ShotNote {
    pub(super) url: String,
    pub(super) taken_at_ms: i64,
}

/// The file beside a browser_screenshot's picture that holds its [`ShotNote`].
pub(super) fn shot_note_path(picture: &Path) -> PathBuf {
    let mut name = picture.as_os_str().to_owned();
    name.push(".where");
    PathBuf::from(name)
}

impl SessionManager {
    fn thread_images_dir(&self, id: &ConversationId) -> PathBuf {
        self.data_dir.join("thread-images").join(&id.0)
    }

    /// Retains a generated file before logging the event, so its recorded path outlasts the CLI.
    pub(super) async fn retain_thread_image(
        &self,
        id: &ConversationId,
        mut event: ProviderEvent,
    ) -> ProviderEvent {
        if let ProviderEvent::Image {
            item_id,
            status: ItemStatus::Completed,
            path: Some(path),
            ..
        } = &mut event
        {
            let source = PathBuf::from(&*path);
            let ext = source
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or("png");
            let target = self
                .thread_images_dir(id)
                .join(format!("{}.{ext}", kept_name(item_id)));
            let retained = blocking(move || {
                if std::fs::metadata(&source).map_err(image_io)?.len() > MAX_IMAGE {
                    return Err(Error::Invalid("This image is over 11 MB".into()));
                }
                std::fs::create_dir_all(target.parent().expect("image folder"))
                    .map_err(image_io)?;
                std::fs::copy(source, &target).map_err(image_io)?;
                Ok(target)
            })
            .await;
            match retained {
                Ok(target) => *path = target.to_string_lossy().into_owned(),
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not retain a generated image")
                }
            }
        }
        event
    }

    /// The copy kept of generated image `item` ([`Self::retain_thread_image`]), if there is one.
    fn kept_image(&self, id: &ConversationId, item: &str) -> Option<PathBuf> {
        let name = kept_name(item);
        std::fs::read_dir(self.thread_images_dir(id))
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name.as_str()))
    }

    pub(super) async fn image_row(
        &self,
        id: &ConversationId,
        event: &ProviderEvent,
        turn: Option<i64>,
    ) {
        let ProviderEvent::Image {
            item_id,
            status,
            prompt,
            ..
        } = event
        else {
            return;
        };
        self.orchestrator_step(
            id,
            OrchestratorStepKind::Tool {
                item_id: item_id.clone(),
                name: "imagegen".into(),
                detail: prompt.clone(),
                status: *status,
                stopped: false,
                through_position: 0,
                ended_at_ms: (*status != ItemStatus::InProgress).then(crate::now_ms),
                exit: None,
                item: Some(ThreadItemKind::Image),
                files: None,
                output_bytes: None,
                full_output: None,
                turn,
            },
        )
        .await;
    }

    /// Returns the image an item of this conversation's CLI thread names (its own image, or a
    /// `view_image` or `Read` of one), or a proof the thread attached, bounded before and
    /// during read.
    pub async fn read_thread_image(
        &self,
        id: &ConversationId,
        item: &str,
    ) -> Result<(String, String)> {
        let board = self.core.board(id).await?;
        let proof = board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Proof { id, file, .. } if id == item => Some(file.clone()),
                _ => None,
            });
        if let Some(file) = proof {
            return read_bounded(PathBuf::from(file)).await;
        }
        // An image the CLI looked at or made, at the path its item names.
        let view = self.thread_view(id)?;
        let entry = view
            .entries
            .iter()
            .find(|entry| brigadier_providers::session_file::item_id(&entry.event) == Some(item))
            .ok_or_else(|| {
                Error::Invalid("This item is not an image of this conversation".into())
            })?;
        let path = match &entry.event {
            // The copy Brigadier kept outlasts the CLI's own file.
            ProviderEvent::Image { path, .. } => self
                .kept_image(id, item)
                .map(|kept| kept.to_string_lossy().into_owned())
                .or_else(|| path.clone()),
            ProviderEvent::ToolCall { name, input, .. } => {
                super::thread_items::image_path(name, input.as_deref())
            }
            _ => None,
        }
        .ok_or_else(|| Error::Invalid("This image is no longer there".into()))?;
        let path = PathBuf::from(path);
        let path = if path.is_absolute() {
            path
        } else {
            let base = self
                .changes_checkout(id)
                .map(|(path, _)| path)
                .unwrap_or_else(|| self.owned_dir("orch", &id.0));
            base.join(path)
        };
        read_bounded(path).await
    }

    pub(super) async fn delete_thread_images(&self, id: &ConversationId) -> Result<()> {
        let path = self.thread_images_dir(id);
        blocking(move || match std::fs::remove_dir_all(path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(image_io(err)),
        })
        .await
    }
}

/// The file at `path` as base64 with its type, bounded before and during the read.
async fn read_bounded(path: PathBuf) -> Result<(String, String)> {
    blocking(move || {
        use std::io::Read;
        let mut file = std::fs::File::open(&path)
            .map_err(|_| Error::Invalid("This image is no longer there".into()))?;
        if file.metadata().map_err(image_io)?.len() > MAX_IMAGE {
            return Err(Error::Invalid("This image is over 11 MB".into()));
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_IMAGE + 1)
            .read_to_end(&mut bytes)
            .map_err(image_io)?;
        if bytes.len() as u64 > MAX_IMAGE {
            return Err(Error::Invalid("This image is over 11 MB".into()));
        }
        let mime = mime(&path)
            .ok_or_else(|| Error::Invalid("This file is not a supported image".into()))?;
        Ok((brigadier_providers::image_base64(&bytes), mime.to_owned()))
    })
    .await
}
