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

    /// Where conversation `id`'s kept images are: its own folder, then, for a fork, those of the
    /// sessions the thread it copied came from.
    fn kept_image_dirs(&self, id: &ConversationId) -> Vec<PathBuf> {
        let origin = |id: &ConversationId| {
            let conversation = self.core.conversation(id).ok()?;
            conversation.forked_from.map(|from| from.conversation_id)
        };
        let mut dirs = vec![self.thread_images_dir(id)];
        let mut source = origin(id);
        while let Some(from) = source {
            dirs.push(self.thread_images_dir(&from));
            source = origin(&from);
        }
        dirs
    }

    /// Capture image bytes before recording the event, while a worker's temp folder exists.
    pub(super) async fn retain_thread_image(
        &self,
        id: &ConversationId,
        event: ProviderEvent,
    ) -> ProviderEvent {
        let paths = match &event {
            ProviderEvent::Image {
                status: ItemStatus::Completed,
                path: Some(path),
                ..
            } => vec![path.clone()],
            ProviderEvent::ToolCall { name, input, .. } => {
                super::thread_items::image_path(name, input.as_deref())
                    .into_iter()
                    .collect()
            }
            ProviderEvent::Message {
                role: brigadier_providers::Role::Assistant,
                text,
                ..
            } => answer_image_paths(text),
            _ => Vec::new(),
        };
        if !paths.is_empty() {
            let paths: Vec<_> = paths
                .iter()
                .map(|path| self.image_source(id, path))
                .collect();
            let directory = self.thread_images_dir(id);
            let result = blocking(move || {
                for path in paths {
                    // Invalid/missing references do not interrupt the answer or expose bytes.
                    let _ = crate::thread_image_files::retain_image(&directory, &path);
                }
                Ok(())
            })
            .await;
            if let Err(err) = result {
                tracing::warn!(conversation = %id, error = %err, "could not retain thread images");
            }
        }
        event
    }

    /// Where an item's path points: a relative one is under the conversation's checkout.
    fn image_source(&self, id: &ConversationId, path: &str) -> PathBuf {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            return path;
        }
        self.changes_checkout(id)
            .map(|(path, _)| path)
            .unwrap_or_else(|| self.owned_dir("orch", &id.0))
            .join(path)
    }

    /// Only absolute, content-validated images can cross the agent-path trust boundary.
    pub async fn read_local_image(
        &self,
        id: &ConversationId,
        path: &str,
    ) -> Result<(String, String)> {
        self.core.conversation(id)?;
        let source = PathBuf::from(path);
        if !source.is_absolute() {
            return Err(Error::Invalid("An image needs an absolute path".into()));
        }
        let retained: Vec<_> = self
            .kept_image_dirs(id)
            .iter()
            .map(|dir| crate::thread_image_files::retained_path(dir, &source))
            .collect();
        blocking(move || {
            let path = retained
                .into_iter()
                .find(|path| path.is_file())
                .unwrap_or(source);
            let (bytes, mime) = crate::thread_image_files::read_image(&path)?;
            Ok((brigadier_providers::image_base64(&bytes), mime.to_owned()))
        })
        .await
    }

    /// The copy kept of generated image `item` ([`Self::retain_thread_image`]), if there is one.
    fn kept_image(&self, id: &ConversationId, item: &str) -> Option<PathBuf> {
        let name = kept_name(item);
        self.kept_image_dirs(id).iter().find_map(|dir| {
            std::fs::read_dir(dir)
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .find(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(name.as_str()))
        })
    }

    /// Item `item` of one of conversation `id`'s workers' threads, as its CLI's files keep it.
    async fn worker_item(&self, id: &ConversationId, item: &str) -> Result<Option<ProviderEvent>> {
        for task in self.core.tasks(id).await? {
            let record = self.session_dir(id).worker(&task).join("record");
            for session in super::session_dir::read_lineage(&record) {
                let Some(file) = session.file else { continue };
                let entries = self.session_files.read(session.provider, Path::new(&file));
                let found = entries.iter().find(|entry| {
                    brigadier_providers::session_file::item_id(&entry.event) == Some(item)
                });
                if let Some(entry) = found {
                    return Ok(Some(entry.event.clone()));
                }
            }
        }
        Ok(None)
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

    /// Returns the image an item of this conversation's CLI thread, or of one of its workers',
    /// names (its own image, or a `view_image` or `Read` of one), or a proof the thread attached,
    /// bounded before and during read.
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
            if mime(Path::new(&file)).is_some_and(|mime| mime.starts_with("image/")) {
                return self.read_local_image(id, &file).await;
            }
            return read_bounded(PathBuf::from(file)).await;
        }
        // An image the CLI looked at or made, at the path its item names.
        let view = self.thread_view(id)?;
        let own = view
            .entries
            .iter()
            .find(|entry| brigadier_providers::session_file::item_id(&entry.event) == Some(item))
            .map(|entry| entry.event.clone());
        let event = match own {
            Some(event) => Some(event),
            None => self.worker_item(id, item).await?,
        }
        .ok_or_else(|| Error::Invalid("This item is not an image of this conversation".into()))?;
        let path = match &event {
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
        self.read_local_image(id, &self.image_source(id, &path).to_string_lossy())
            .await
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

/// Markdown image/link destinations, including angle-wrapped paths with spaces and reference
/// definitions. Backtick paths are also common in answers describing a generated image.
fn answer_image_paths(text: &str) -> Vec<String> {
    static PATHS: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let paths = PATHS.get_or_init(|| {
        regex::Regex::new(r#"(?:\]\(\s*|\]:\s*|`)(?:<([^>]+)>|([^\s)`]+))"#)
            .expect("image paths regex")
    });
    paths
        .captures_iter(text)
        .filter_map(|capture| {
            let path = capture.get(1).or_else(|| capture.get(2))?.as_str();
            let path = path.strip_prefix("file://").unwrap_or(path);
            // Markdown encodes spaces and non-ASCII characters in destinations.
            let mut bytes = Vec::new();
            let mut input = path.as_bytes().iter().copied();
            while let Some(byte) = input.next() {
                if byte == b'%' {
                    let a = (input.next()? as char).to_digit(16)?;
                    let b = (input.next()? as char).to_digit(16)?;
                    bytes.push((a * 16 + b) as u8);
                } else {
                    bytes.push(byte);
                }
            }
            let path = String::from_utf8(bytes).ok()?;
            Path::new(&path).is_absolute().then_some(path)
        })
        .collect()
}
