//! One folder per session (G13): everything a session makes lives in
//! `<sessions root>/<short id>/` ([`brigadier_sandbox::AppPaths::sessions_dir`]), short and free
//! of spaces.
//!
//! | Part | Holds | Archive | Delete |
//! |---|---|---|---|
//! | `record/` | `workspace.json`: the environment and base chosen on the rail; `lineage.json`: its CLI sessions and their files | keeps | removes |
//! | `work/` | a New worktree session's worktree; a Chat's working folder | removes once merged | removes |
//! | `tmp/` | its CLI's `TMPDIR` | removes | removes |
//! | `dev/` | its previews' scratch and the thread's own files | removes | removes |
//! | `logs/` | its previews' logs | removes | removes |
//! | `workers/` | each worker's `<n>-<task8>/{work,tmp}`, and `home`, where Claude workers start | removes | removes |
//!
//! A part is recorded in the cleanup ledger under the owner that removes it (the session's
//! worktree under `session:`, `tmp/` under `thread-temp:`, …); the folder itself and `record/`
//! only under [`folder_owner`], which only Delete disposes of. A Local checkout session works
//! in the user's own checkout, never in `work/`. Tab chats keep their folder in the data
//! directory. Sessions made before these folders were archived at the upgrade
//! (`upgrade.rs`), never converted.

use std::path::{Path, PathBuf};

use brigadier_providers::{Artifact, ProviderKind};
use serde::{Deserialize, Serialize};

use super::{SessionManager, blocking};
use crate::model::{Conversation, ConversationId, Environment, Setup};
use crate::work::Task;
use crate::{Error, Result};

/// The cleanup-ledger owner of a session's folder and its `record/`: only Delete disposes of it.
pub(crate) fn folder_owner(id: &ConversationId) -> String {
    format!("session-folder:{id}")
}

/// A session's folder and its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionDir(PathBuf);

impl SessionDir {
    pub(crate) fn root(&self) -> &Path {
        &self.0
    }

    pub(crate) fn record(&self) -> PathBuf {
        self.0.join("record")
    }

    pub(crate) fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    pub(crate) fn tmp(&self) -> PathBuf {
        self.0.join("tmp")
    }

    pub(crate) fn dev(&self) -> PathBuf {
        self.0.join("dev")
    }

    pub(crate) fn logs(&self) -> PathBuf {
        self.0.join("logs")
    }

    /// The folder Claude workers of the session start in.
    pub(crate) fn workers_home(&self) -> PathBuf {
        self.0.join("workers").join("home")
    }

    /// Worker `task`'s folder: its worktree in `work/`, its scratch (`TMPDIR`) in `tmp/`.
    pub(crate) fn worker(&self, task: &Task) -> PathBuf {
        self.0.join("workers").join(format!(
            "{}-{}",
            task.number,
            &task.id.0[task.id.0.len().saturating_sub(8)..]
        ))
    }
}

/// `record/workspace.json`: where the session works and what its work lands on, as chosen on
/// the rail. Later diffs, landing and cleanup read the target from here and never guess it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceRecord {
    /// `localCheckout` or `newWorktree`.
    pub environment: &'static str,
    /// The branch the session works on.
    pub branch: String,
    /// What a New worktree's branch started from and merges back into; none for a Local
    /// checkout, which works on its branch directly.
    pub base: Option<String>,
}

impl WorkspaceRecord {
    pub(crate) fn of(environment: &Environment) -> Self {
        match environment {
            Environment::LocalCheckout { branch } => Self {
                environment: "localCheckout",
                branch: branch.clone(),
                base: None,
            },
            Environment::NewWorktree { base, branch, .. } => Self {
                environment: "newWorktree",
                branch: branch.clone(),
                base: Some(base.clone()),
            },
        }
    }
}

/// One CLI session of a thread, as `record/lineage.json` lists them, oldest first: the CLI's
/// own files are the thread's record (S8), and this says which they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineageEntry {
    pub provider: ProviderKind,
    pub session_id: String,
    /// The CLI's session file (Claude's JSONL, Codex's rollout), when it said where.
    pub file: Option<String>,
    pub started_at: i64,
}

/// `record/lineage.json`'s sessions; none when it is missing or unreadable.
pub(crate) fn read_lineage(record: &Path) -> Vec<LineageEntry> {
    std::fs::read(record.join("lineage.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl SessionManager {
    /// Adds a CLI session start to conversation `id`'s `record/lineage.json`. A session already
    /// listed (resumed, or started twice by its CLI) is listed once, with the file it named
    /// first unless it named none.
    pub(crate) async fn note_lineage(&self, id: &ConversationId, entry: LineageEntry) {
        let record = self.session_dir(id).record();
        let written = blocking(move || {
            let mut lineage = read_lineage(&record);
            match lineage.iter_mut().find(|known| {
                known.provider == entry.provider && known.session_id == entry.session_id
            }) {
                Some(known) if known.file.is_some() || entry.file.is_none() => return Ok(()),
                Some(known) => known.file = entry.file,
                None => lineage.push(entry),
            }
            std::fs::create_dir_all(&record).map_err(|err| Error::Invalid(err.to_string()))?;
            let text = serde_json::to_string_pretty(&lineage)
                .map_err(|err| Error::Invalid(err.to_string()))?;
            write_atomic(&record.join("lineage.json"), text.as_bytes())
        })
        .await;
        if let Err(err) = written {
            tracing::warn!(conversation = %id, error = %err, "could not note the CLI session in lineage.json");
        }
    }

    /// Conversation `id`'s folder (not created).
    pub(crate) fn session_dir(&self, id: &ConversationId) -> SessionDir {
        SessionDir(
            self.runtime
                .platform()
                .paths()
                .sessions_dir
                .join(id.short()),
        )
    }

    /// Makes `conversation`'s folder and its `record/`, recorded under [`folder_owner`] first,
    /// and writes `record/workspace.json` for a session. Tab chats have none.
    pub(crate) async fn prepare_session_dir(&self, conversation: &Conversation) -> Result<()> {
        if conversation.tab_chat.is_some() {
            return Ok(());
        }
        let dir = self.session_dir(&conversation.id);
        let owner = folder_owner(&conversation.id);
        let ledger = self.runtime.ledger();
        let path = dir.root().to_string_lossy().into_owned();
        let recorded = ledger.artifacts(&owner).iter().any(
            |artifact| matches!(artifact, Artifact::ScratchDir { path: held } if *held == path),
        );
        if !recorded {
            ledger
                .record(&owner, Artifact::ScratchDir { path: path.clone() })
                .await?;
            ledger
                .record(&owner, Artifact::ProcessesIn { dir: path })
                .await?;
        }
        let workspace = match &conversation.setup {
            Some(Setup::Session { environment, .. }) => Some(WorkspaceRecord::of(environment)),
            _ => None,
        };
        let record = dir.record();
        blocking(move || {
            std::fs::create_dir_all(&record).map_err(|err| Error::Invalid(err.to_string()))?;
            if let Some(workspace) = workspace {
                let text = serde_json::to_string_pretty(&workspace)
                    .map_err(|err| Error::Invalid(err.to_string()))?;
                write_atomic(&record.join("workspace.json"), text.as_bytes())?;
            }
            Ok(())
        })
        .await
    }

    /// Delete's last step for a conversation's folder: the folder and its `record/` go
    /// ([`folder_owner`]), once nothing else of the ledger still holds a worktree in it (one a
    /// WIP commit couldn't keep stays, with its folder, as the archive leaves it).
    pub(super) async fn remove_session_dir(&self, id: &ConversationId) {
        let owner = folder_owner(id);
        let ledger = self.runtime.ledger();
        let dir = self.session_dir(id);
        let worktree_left = ledger.owners().into_iter().any(|(holder, artifacts, _)| {
            holder != owner
                && artifacts.iter().any(|artifact| {
                    matches!(artifact, Artifact::Worktree { path, .. } if Path::new(path).starts_with(dir.root()))
                })
        });
        if worktree_left {
            tracing::warn!(conversation = %id, "a worktree stays in the deleted session's folder, so the folder stays");
            return;
        }
        let leftovers = ledger.dispose(&owner).await;
        if !leftovers.is_clean() {
            tracing::warn!(conversation = %id, ?leftovers, "some of the session's folder will be retried at the next launch");
        }
    }
}

/// Writes `path` whole: a reader sees the old file or the new one, never a part.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)
        .and_then(|()| std::fs::rename(&part, path))
        .map_err(|err| Error::Invalid(format!("{}: {err}", path.display())))
}
