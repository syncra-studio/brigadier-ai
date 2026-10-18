//! The first run's project suggestions: the repositories the user's own Claude Code and Codex
//! sessions worked in, one per repository however many folders and worktrees of it they used.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use brigadier_providers::ProviderKind;

use super::{SessionManager, blocking};
use crate::Result;
use crate::model::ProjectCandidate;

impl SessionManager {
    /// Repositories the user worked in with a CLI, most recent first. Folders outside any
    /// repository are left out, and so are Brigadier's own (its data and temporary folders).
    pub async fn find_projects(&self) -> Result<Vec<ProjectCandidate>> {
        let past = self.runtime.past_folders().await;
        let projects = self.core.catalog().projects;
        let git = self.git.clone();
        let data_dir = self.data_dir.clone();
        blocking(move || {
            let skipped = skipped_roots(&data_dir);
            let is_skipped = |path: &Path| skipped.iter().any(|root| path.starts_with(root));
            // Several CLIs (and sessions) share folders: look each one up once.
            let mut repos: HashMap<PathBuf, Option<brigadier_git::FoundRepo>> = HashMap::new();
            let mut found: HashMap<PathBuf, ProjectCandidate> = HashMap::new();
            for (provider, folder) in past {
                let Ok(cwd) = folder.cwd.canonicalize() else {
                    continue;
                };
                if is_skipped(&cwd) {
                    continue;
                }
                let repo = repos
                    .entry(cwd.clone())
                    .or_insert_with(|| git.find_repo(&cwd).ok().flatten());
                let Some(repo) = repo else {
                    continue;
                };
                if is_skipped(&repo.root) {
                    continue;
                }
                let candidate = found.entry(repo.root.clone()).or_insert_with(|| {
                    let path = repo.root.display().to_string();
                    ProjectCandidate {
                        name: repo_name(repo.origin.as_deref(), &repo.root),
                        project_id: projects
                            .iter()
                            .find(|project| project.repos.iter().any(|r| r.path == path))
                            .map(|project| project.id.clone()),
                        path,
                        providers: Vec::new(),
                        sessions: 0,
                        last_active_ms: 0,
                    }
                });
                if !candidate.providers.contains(&provider) {
                    candidate.providers.push(provider);
                }
                candidate.sessions = candidate.sessions.saturating_add(folder.sessions);
                candidate.last_active_ms = candidate.last_active_ms.max(folder.last_active_ms);
            }
            let mut candidates: Vec<_> = found.into_values().collect();
            for candidate in &mut candidates {
                candidate
                    .providers
                    .sort_by_key(|kind| provider_order(*kind));
            }
            candidates.sort_by(|a, b| {
                b.last_active_ms
                    .cmp(&a.last_active_ms)
                    .then_with(|| a.name.cmp(&b.name))
            });
            Ok(candidates)
        })
        .await
    }
}

fn provider_order(kind: ProviderKind) -> usize {
    ProviderKind::ALL
        .iter()
        .position(|known| *known == kind)
        .unwrap_or(usize::MAX)
}

/// Brigadier's data folder (its orchestrators, chats and worker worktrees run there) and the
/// temporary folders, canonical so they match canonical working folders.
pub(super) fn skipped_roots(data_dir: &Path) -> BTreeSet<PathBuf> {
    let mut roots = vec![data_dir.to_owned(), std::env::temp_dir()];
    if cfg!(unix) {
        roots.push(PathBuf::from("/tmp"));
    }
    roots
        .into_iter()
        .map(|root| root.canonicalize().unwrap_or(root))
        .collect()
}

/// `owner/name` from a remote URL (`git@host:owner/name.git`, `https://host/owner/name`),
/// else the folder's name.
fn repo_name(origin: Option<&str>, root: &Path) -> String {
    let from_remote = origin.and_then(|url| {
        let url = url.trim_end_matches('/');
        let url = url.strip_suffix(".git").unwrap_or(url);
        // `scp`-like addresses separate the host with a colon.
        let path = match url.split_once("://") {
            Some((_, rest)) => rest.split_once('/')?.1,
            None => url.split_once(':')?.1,
        };
        let mut parts = path.rsplit('/').filter(|part| !part.is_empty());
        let name = parts.next()?;
        Some(match parts.next() {
            Some(owner) => format!("{owner}/{name}"),
            None => name.to_owned(),
        })
    });
    from_remote.unwrap_or_else(|| {
        root.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string())
    })
}
