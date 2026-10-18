//! Per-project secrets (PLAN §6 Phase 3): gitignored env files copied into each new worktree
//! (a tab's and a worker's), with their values redacted from everything the session produces.
//!
//! A new worktree gets the checkout's gitignored `.env*` files, what its `.worktreeinclude`
//! names among the files git ignores ([`brigadier_git::Repo::worktree_copies`]), and the
//! project's listed secret files. A listed file is used only if it exists in the user's
//! checkout and git ignores it there, so a tracked file is never duplicated and a secret is
//! never committed. Copies live inside the worktree and go with it. The values of the env files
//! and the listed ones are hidden; other `.worktreeinclude` files are copied as they are.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use brigadier_providers::redact::{Redactor, env_file_values};

use super::{SessionManager, blocking, git_error};
use crate::{Error, Result};

/// The most files a new worktree gets copies of besides the listed ones.
const COPIES_MAX: usize = 500;
/// How long a checkout's list of copied files is taken as it was, for redaction.
const COPIES_FRESH: Duration = Duration::from_secs(30);

/// Copies the checkout's files a new worktree gets into `worktree` and returns the values to
/// hide.
pub(crate) async fn copy_secrets(
    manager: &SessionManager,
    _owner: &str,
    repo: &Path,
    worktree: &Path,
    files: &[String],
) -> Result<Vec<String>> {
    let (git, repo, worktree, files) = (
        manager.git.clone(),
        repo.to_owned(),
        worktree.to_owned(),
        files.to_vec(),
    );
    blocking(move || copy_into(&git, &repo, &worktree, &files).map(|(_, values)| values)).await
}

/// [`copy_secrets`], blocking: the files copied and the values to hide.
pub(crate) fn copy_into(
    git: &brigadier_git::Git,
    repo: &Path,
    worktree: &Path,
    files: &[String],
) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let checkout = git.open(repo).map_err(git_error)?;
    let mut values = Vec::new();
    let mut copied = Vec::new();
    for (file, secret) in worktree_files(&checkout, repo, files)? {
        let source = repo.join(&file);
        let target = worktree.join(&file);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Invalid(err.to_string()))?;
        }
        std::fs::copy(&source, &target)
            .map_err(|err| Error::Invalid(format!("copying {}: {err}", file.display())))?;
        if secret && let Ok(text) = std::fs::read_to_string(&source) {
            values.extend(env_file_values(&text));
        }
        copied.push(file);
    }
    Ok((copied, values))
}

/// The values of the checkout's env files and the project's secret files, read from the
/// user's checkout.
pub(crate) async fn values(git: &brigadier_git::Git, repo: &Path, files: &[String]) -> Vec<String> {
    let (git, repo, files) = (git.clone(), repo.to_owned(), files.to_vec());
    blocking(move || {
        let mut names: Vec<PathBuf> = cached_env_files(&git, &repo);
        for file in &files {
            let file = clean(file)?;
            if !names.contains(&file) {
                names.push(file);
            }
        }
        let mut values = Vec::new();
        for file in names {
            if let Ok(text) = std::fs::read_to_string(repo.join(file)) {
                values.extend(env_file_values(&text));
            }
        }
        Ok(values)
    })
    .await
    .unwrap_or_default()
}

/// Each checkout's env files, and when they were listed.
type Listed = HashMap<PathBuf, (Instant, Vec<PathBuf>)>;

/// The checkout's env files a new worktree gets, as listed at most [`COPIES_FRESH`] ago.
fn cached_env_files(git: &brigadier_git::Git, repo: &Path) -> Vec<PathBuf> {
    static LISTED: OnceLock<Mutex<Listed>> = OnceLock::new();
    let listed = LISTED.get_or_init(Mutex::default);
    if let Some((at, files)) = listed.lock().unwrap_or_else(|e| e.into_inner()).get(repo)
        && at.elapsed() < COPIES_FRESH
    {
        return files.clone();
    }
    let files: Vec<PathBuf> = git
        .open(repo)
        .and_then(|checkout| checkout.worktree_copies(COPIES_MAX))
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .filter(|file| is_env_file(file))
        .collect();
    listed
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(repo.to_owned(), (Instant::now(), files.clone()));
    files
}

/// Whether `file` is an env file (`.env`, `.env.local`, …), whose values are hidden.
fn is_env_file(file: &Path) -> bool {
    file.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".env"))
}

/// The checkout's files a new worktree gets, each with whether its values are hidden: the
/// default set, then the listed files that exist and git ignores.
fn worktree_files(
    checkout: &brigadier_git::Repo,
    repo: &Path,
    files: &[String],
) -> Result<Vec<(PathBuf, bool)>> {
    let mut all: Vec<(PathBuf, bool)> = checkout
        .worktree_copies(COPIES_MAX)
        .map_err(git_error)?
        .into_iter()
        .map(|file| {
            let file = PathBuf::from(file);
            let secret = is_env_file(&file);
            (file, secret)
        })
        .collect();
    for file in usable(checkout, repo, files)? {
        match all.iter_mut().find(|(known, _)| *known == file) {
            Some(known) => known.1 = true,
            None => all.push((file, true)),
        }
    }
    Ok(all)
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
        values(&self.git, Path::new(repo), &files).await
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
