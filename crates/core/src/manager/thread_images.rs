//! Images are opened only through the image step that named them in this conversation.

use std::path::{Path, PathBuf};

use brigadier_providers::{ItemStatus, ProviderEvent};
use brigadier_store::StreamPage;

use super::{SessionManager, blocking};
use crate::model::{ConversationId, DomainEvent, streams};
use crate::work::{OrchestratorEntry, OrchestratorStepKind, ThreadItemKind};
use crate::{Error, Result};

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
        _ => None,
    }
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
            let name = item_id
                .as_bytes()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let target = self.thread_images_dir(id).join(format!("{name}.{ext}"));
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

    pub(super) async fn image_row(
        &self,
        id: &ConversationId,
        event: &ProviderEvent,
        log_seq: Option<i64>,
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
                log_seq,
                turn,
            },
        )
        .await;
    }

    /// Returns the image named by an image row of this conversation, bounded before and during read.
    pub async fn read_thread_image(
        &self,
        id: &ConversationId,
        item: &str,
    ) -> Result<(String, String)> {
        let board = self.core.board(id).await?;
        let row = board
            .orchestrator_steps
            .iter()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Tool {
                    item_id,
                    item: Some(ThreadItemKind::Image),
                    log_seq: Some(log_seq),
                    ..
                } if item_id == item => Some(*log_seq),
                _ => None,
            })
            .ok_or_else(|| {
                Error::Invalid("This item is not an image of this conversation".into())
            })?;
        let stored = self
            .core
            .store()
            .read_stream(
                streams::orchestrator(id),
                StreamPage {
                    before: Some(row + 1),
                    limit: 1,
                    kinds: vec!["orchestrator.logged".into()],
                },
            )
            .await?;
        let event = stored
            .iter()
            .find(|stored| stored.stream_seq == row)
            .and_then(|stored| crate::sessions::decode(stored).ok());
        let path = match event {
            Some(DomainEvent::OrchestratorLogged {
                entry:
                    OrchestratorEntry::Provider {
                        event:
                            ProviderEvent::Image {
                                item_id,
                                path: Some(path),
                                ..
                            },
                        ..
                    },
                ..
            }) if item_id == item => Some(path),
            Some(DomainEvent::OrchestratorLogged {
                entry:
                    OrchestratorEntry::Provider {
                        event:
                            ProviderEvent::ToolCall {
                                item_id,
                                name,
                                input,
                                ..
                            },
                        ..
                    },
                ..
            }) if item_id == item => super::thread_items::image_path(&name, input.as_deref()),
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
