//! Per-project secrets (PLAN §6 Phase 3): gitignored env files copied into each worker's
//! worktree, with their values redacted from everything the worker produces.
//!
//! A listed file is used only if it exists in the user's checkout and git ignores it there, so
//! a tracked file is never duplicated and a secret is never committed. Copies live inside the
//! worktree and go with it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::redact::{Redactor, env_file_values};

use super::{SessionManager, blocking, git_error};
use crate::{Error, Result};

/// Copies the project's secret files into `worktree` and returns their values.
pub(crate) async fn copy_secrets(
    manager: &SessionManager,
    _owner: &str,
    repo: &Path,
    worktree: &Path,
    files: &[String],
) -> Result<Vec<String>> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let (git, repo, worktree, files) = (
        manager.git.clone(),
        repo.to_owned(),
        worktree.to_owned(),
        files.to_vec(),
    );
    blocking(move || {
        let checkout = git.open(&repo).map_err(git_error)?;
        let mut values = Vec::new();
        for file in usable(&checkout, &repo, &files)? {
            let source = repo.join(&file);
            let target = worktree.join(&file);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
            }
            std::fs::copy(&source, &target)
                .map_err(|err| Error::Invalid(format!("copying {}: {err}", file.display())))?;
            if let Ok(text) = std::fs::read_to_string(&source) {
                values.extend(env_file_values(&text));
            }
        }
        Ok(values)
    })
    .await
}

/// The values of the project's secret files, read from the user's checkout.
pub(crate) async fn values(repo: &Path, files: &[String]) -> Vec<String> {
    if files.is_empty() {
        return Vec::new();
    }
    let (repo, files) = (repo.to_owned(), files.to_vec());
    blocking(move || {
        let mut values = Vec::new();
        for file in files {
            if let Ok(text) = std::fs::read_to_string(repo.join(clean(&file)?)) {
                values.extend(env_file_values(&text));
            }
        }
        Ok(values)
    })
    .await
    .unwrap_or_default()
}

impl SessionManager {
    /// The values of the secret files of the session `id`'s project, read from its repository;
    /// none for a Chat or a project without any.
    pub(crate) async fn session_secret_values(
        &self,
        id: &crate::model::ConversationId,
    ) -> Vec<String> {
        let Ok(conversation) = self.core.conversation(id) else {
            return Vec::new();
        };
        let Some(crate::model::Setup::Session { repo, .. }) = &conversation.setup else {
            return Vec::new();
        };
        let files = conversation
            .project_id
            .as_ref()
            .and_then(|project| self.core.project(project).ok())
            .map(|project| project.prefs.secret_files)
            .unwrap_or_default();
        values(Path::new(repo), &files).await
    }
}

/// A redactor for `values` (secrets and grants); None when there is nothing to hide.
pub(crate) fn redactor(values: Vec<String>) -> Option<Arc<Redactor>> {
    let redactor = Redactor::new(values);
    (!redactor.is_empty()).then(|| Arc::new(redactor))
}

/// The listed files that exist in the checkout and are ignored by git.
fn usable(checkout: &brigadier_git::Repo, repo: &Path, files: &[String]) -> Result<Vec<PathBuf>> {
    let mut usable = Vec::new();
    for file in files {
        let path = clean(file)?;
        if !repo.join(&path).is_file() {
            tracing::info!(file, "secret file not found in the checkout; skipped");
            continue;
        }
        if !checkout
            .is_ignored(&path.to_string_lossy())
            .map_err(git_error)?
        {
            tracing::warn!(file, "secret file is not ignored by git; not copied");
            continue;
        }
        usable.push(path);
    }
    Ok(usable)
}

/// A repo-relative path with no way out of the repository.
fn clean(file: &str) -> Result<PathBuf> {
    let path = PathBuf::from(file.trim());
    if path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(Error::Invalid(format!(
            "{file} is not a path inside the repository"
        )));
    }
    Ok(path)
}
