//! Images are opened only through the image step that named them in this conversation, and a
//! proof (a screenshot or recording the thread attached, `attach_proof`) through its step.

use std::path::{Path, PathBuf};

use brigadier_providers::{ItemStatus, ProviderEvent};
use brigadier_store::StreamPage;

use super::{SessionManager, blocking};
use crate::model::{ConversationId, DomainEvent, streams};
use crate::work::{OrchestratorEntry, OrchestratorStepKind, ProofKind, ThreadItemKind};
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
        "mp4" | "m4v" => Some("video/mp4"),
        "mov" => Some("video/quicktime"),
        "webm" => Some("video/webm"),
        "log" | "txt" => Some("text/plain"),
        _ => None,
    }
}

/// A text proof's limit: a log or test output past it is cut by the thread first.
const MAX_TEXT: u64 = 1024 * 1024;

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

/// A URL's scheme, host and port: two pages of one preview share it.
fn origin(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..url.len() - rest.len() + end])
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

    /// `attach_proof`: keeps a copy of the thread's screenshot or recording and shows it with
    /// the reply it is writing, the merge message.
    pub(crate) async fn attach_proof(
        &self,
        id: &ConversationId,
        args: crate::tools::AttachProof,
    ) -> Result<String> {
        let base = match self.effective_workspace(id).await {
            Ok(Some(workspace)) => workspace.path,
            _ => self.owned_dir("orch", &id.0),
        };
        // Resolved, links followed: Brigadier's private folder is never read, as in the sandbox.
        let source = super::reads::absolute(&base, args.path.trim());
        let run_dir = self.runtime.platform().paths().run_dir.clone();
        let private = run_dir.canonicalize().unwrap_or_else(|_| run_dir.clone());
        if source.starts_with(&private) || source.starts_with(&run_dir) {
            return Err(Error::Invalid(format!(
                "{} is in Brigadier's private folder: attach a file you made",
                source.display()
            )));
        }
        let name = source
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mime = mime(&source)
            .filter(|mime| *mime != "image/heic" && *mime != "image/tiff" && *mime != "image/bmp");
        let Some(mime) = mime else {
            return Err(Error::Invalid(format!(
                "{name} isn't a PNG, JPEG, GIF or WebP image, an MP4, MOV or WebM recording, or a \
                 .log or .txt output"
            )));
        };
        let kind = match mime {
            "text/plain" => match args.kind {
                Some(ProofKind::Test) => ProofKind::Test,
                _ => ProofKind::Log,
            },
            _ if mime.starts_with("video/") => ProofKind::Recording,
            _ => ProofKind::Screenshot,
        };
        let board = self.core.board(id).await?;
        let preview = match args.preview.as_deref().map(str::trim) {
            Some(wanted) if !wanted.is_empty() => {
                Some(board.previews.get(wanted).cloned().ok_or_else(|| {
                    Error::Invalid(format!("{wanted} isn't one of your previews"))
                })?)
            }
            _ => None,
        };
        // A browser_screenshot's page, kept beside it.
        let note = {
            let note = shot_note_path(&source);
            blocking(move || {
                Ok(std::fs::read(&note)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<ShotNote>(&bytes).ok()))
            })
            .await?
        };
        let taken_on = args
            .taken_on
            .map(|on| on.trim().to_owned())
            .filter(|on| !on.is_empty())
            .or_else(|| note.as_ref().map(|note| note.url.clone()))
            .or_else(|| preview.as_ref().and_then(|preview| preview.url.clone()));
        // A page of one of the session's previews is that preview's, the latest first.
        let preview = preview.map(|preview| preview.id).or_else(|| {
            let page = origin(taken_on.as_deref()?)?;
            board
                .sorted_previews()
                .into_iter()
                .rev()
                .find(|preview| preview.url.as_deref().and_then(origin) == Some(page))
                .map(|preview| preview.id)
        });
        let proof = format!("proof-{}", uuid::Uuid::new_v4().simple());
        let ext = source
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("bin")
            .to_ascii_lowercase();
        let target = self.thread_images_dir(id).join(format!("{proof}.{ext}"));
        let kept = target.clone();
        let changed_at = blocking(move || {
            let meta = std::fs::metadata(&source)
                .map_err(|_| Error::Invalid(format!("{} isn't there", source.display())))?;
            if !meta.is_file() {
                return Err(Error::Invalid(format!("{} isn't a file", source.display())));
            }
            if kind == ProofKind::Log || kind == ProofKind::Test {
                if meta.len() > MAX_TEXT {
                    return Err(Error::Invalid(
                        "It is over 1 MB: attach the part of the output that shows it".into(),
                    ));
                }
            } else if meta.len() > MAX_IMAGE {
                return Err(Error::Invalid(
                    "It is over 11 MB: attach a smaller screenshot or a shorter recording".into(),
                ));
            }
            std::fs::create_dir_all(kept.parent().expect("image folder")).map_err(image_io)?;
            std::fs::copy(&source, &kept).map_err(image_io)?;
            Ok(meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|at| i64::try_from(at.as_millis()).ok()))
        })
        .await?;
        self.orchestrator_step(
            id,
            OrchestratorStepKind::Proof {
                id: proof,
                name: name.clone(),
                mime: mime.to_owned(),
                caption: args
                    .caption
                    .map(|caption| caption.trim().to_owned())
                    .filter(|caption| !caption.is_empty()),
                file: target.to_string_lossy().into_owned(),
                kind: Some(kind),
                taken_on,
                preview,
                taken_at_ms: note.map(|note| note.taken_at_ms).or(changed_at),
            },
        )
        .await;
        Ok(format!(
            "Attached {name}: the user sees it under your reply. Don't link or describe the file again."
        ))
    }

    /// Returns the image named by an image row of this conversation, or a proof the thread
    /// attached, bounded before and during read.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two pages of one preview share an origin; another port is another preview.
    #[test]
    fn a_pages_origin_is_its_scheme_host_and_port() {
        assert_eq!(
            origin("http://127.0.0.1:5173/settings?x=1"),
            Some("http://127.0.0.1:5173")
        );
        assert_eq!(
            origin("http://localhost:3000"),
            Some("http://localhost:3000")
        );
        assert_eq!(
            origin("http://localhost:3000#top"),
            Some("http://localhost:3000")
        );
        assert_eq!(origin("iPhone 16 simulator"), None);
        assert_ne!(
            origin("http://localhost:3001/"),
            origin("http://localhost:3000/")
        );
    }
}
