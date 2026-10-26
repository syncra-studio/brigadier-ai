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
//! | `workers/` | each worker's `<n>-<task8>/{record,work,tmp}`, what else it wrote there, and `home`, where Claude workers start | keeps each worker's `record/` and what else it wrote; removes its `tmp/`, and its `work/` once merged | removes |
//!
//! A part is recorded in the cleanup ledger under the owner that removes it (the session's
//! worktree under `session:`, `tmp/` under `thread-temp:`, …); the folder itself and `record/`
//! only under [`folder_owner`], which only Delete disposes of. A Local checkout session works
//! in the user's own checkout, never in `work/`. Tab chats keep their folder in the data
//! directory. Sessions made before these folders were archived at the upgrade
//! (`upgrade.rs`), never converted. A page the user saved lives in the data directory's
//! `pages/`, outside the folder, and stays after Delete.

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

/// The ledger's name in `record/`.
pub(crate) const LEDGER_FILE: &str = "ledger.md";

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

    /// What the standing instructions name, for the CLI's environment ([`standing_env`]).
    pub(crate) fn standing_env(&self) -> Vec<(String, String)> {
        standing_env(self.root())
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

/// What the standing instructions name, for the CLI's environment: the folder `root` of a
/// session or worker and the ledger in its `record/` ([`super::prompts::SESSION_DIR_ENV`],
/// [`super::prompts::LEDGER_ENV`]).
pub(crate) fn standing_env(root: &Path) -> Vec<(String, String)> {
    vec![
        (
            super::prompts::SESSION_DIR_ENV.to_owned(),
            root.display().to_string(),
        ),
        (
            super::prompts::LEDGER_ENV.to_owned(),
            root.join("record").join(LEDGER_FILE).display().to_string(),
        ),
    ]
}

/// `record/workspace.json`: where the session works and what its work lands on, as chosen on
/// the rail. Later diffs, landing and cleanup read the target from here and never guess it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkspaceRecord {
    /// `localCheckout` or `newWorktree`; `folder` for a Chat's worker with no repository (S28).
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
    /// Gone for good (a Chat's rework, the upgrade pass): never resumed, and the next turn
    /// starts a fresh session (#540).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub forgotten: bool,
}

/// Adds a CLI session start to the `lineage.json` in `record`: a session's or a worker's.
pub(crate) async fn note_lineage_at(record: PathBuf, entry: LineageEntry) -> Result<()> {
    blocking(move || {
        let mut lineage = read_lineage(&record);
        match lineage
            .iter_mut()
            .find(|known| known.provider == entry.provider && known.session_id == entry.session_id)
        {
            Some(known) if !known.forgotten && (known.file.is_some() || entry.file.is_none()) => {
                return Ok(());
            }
            Some(known) => {
                known.forgotten = false;
                if known.file.is_none() {
                    known.file = entry.file;
                }
            }
            None => lineage.push(entry),
        }
        std::fs::create_dir_all(&record).map_err(|err| Error::Invalid(err.to_string()))?;
        let text = serde_json::to_string_pretty(&lineage)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        write_atomic(&record.join("lineage.json"), text.as_bytes())
    })
    .await
}

/// Marks every CLI session in the `lineage.json` in `record` forgotten: none is resumed.
pub(crate) async fn forget_lineage_at(record: PathBuf) -> Result<()> {
    blocking(move || {
        let mut lineage = read_lineage(&record);
        if lineage.iter().all(|entry| entry.forgotten) {
            return Ok(());
        }
        for entry in &mut lineage {
            entry.forgotten = true;
        }
        let text = serde_json::to_string_pretty(&lineage)
            .map_err(|err| Error::Invalid(err.to_string()))?;
        write_atomic(&record.join("lineage.json"), text.as_bytes())
    })
    .await
}

/// The CLI session a turn on `provider` resumes, from the `lineage.json` in `record`: the
/// newest of that provider's, unless it was forgotten (#540).
pub(crate) fn resumed_session(record: &Path, provider: ProviderKind) -> Option<String> {
    read_lineage(record)
        .into_iter()
        .rev()
        .find(|entry| entry.provider == provider)
        .filter(|entry| !entry.forgotten)
        .map(|entry| entry.session_id)
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
    /// `private`: a tab chat's, never named in the logs.
    pub(crate) async fn note_lineage(
        &self,
        id: &ConversationId,
        entry: LineageEntry,
        private: bool,
    ) {
        let record = self.session_dir(id).record();
        let written = note_lineage_at(record, entry).await;
        if let Err(err) = written {
            crate::logging::private_trace!(private, warn, "could not note the CLI session in lineage.json"; conversation = %id, error = %err);
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
    /// and writes `record/workspace.json` for a session. A tab chat's folder holds only its
    /// record (its thread's lineage and turns) and is its own: it goes with the chat.
    pub(crate) async fn prepare_session_dir(&self, conversation: &Conversation) -> Result<()> {
        let dir = self.session_dir(&conversation.id);
        let owner = if conversation.tab_chat.is_some() {
            super::lifecycle::conversation_owner(conversation).0
        } else {
            folder_owner(&conversation.id)
        };
        let ledger = self.runtime.ledger();
        let path = dir.root().to_string_lossy().into_owned();
        let recorded = ledger.artifacts(&owner).iter().any(
            |artifact| matches!(artifact, Artifact::ScratchDir { path: held } if *held == path),
        );
        if !recorded {
            ledger
                .record(&owner, Artifact::ScratchDir { path: path.clone() })
                .await?;
            // Open sessions keep detached commands across daemon restarts. Archive and
            // Delete record their process cleanup when the user closes the session.
            if conversation.tab_chat.is_some() {
                ledger
                    .record(&owner, Artifact::ProcessesIn { dir: path })
                    .await?;
            }
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
    pub(super) async fn remove_session_dir(&self, id: &ConversationId) -> bool {
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
            return false;
        }
        let leftovers = ledger.dispose(&owner).await;
        if !leftovers.is_clean() {
            tracing::warn!(conversation = %id, ?leftovers, "some of the session's folder will be retried at the next launch");
        }
        leftovers.is_clean()
    }
}

/// Writes `path` whole: a reader sees the old file or the new one, never a part.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let part = path.with_extension("part");
    std::fs::write(&part, bytes)
        .and_then(|()| std::fs::rename(&part, path))
        .map_err(|err| Error::Invalid(format!("{}: {err}", path.display())))
}
