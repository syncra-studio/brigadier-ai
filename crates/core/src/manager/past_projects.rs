//! The first run's project suggestions: the repositories the user's own Claude Code and Codex
//! sessions worked in, one per repository however many folders and worktrees of it they used.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use brigadier_git::{FoundRepo, Git};
use brigadier_providers::ProviderKind;

use super::protected_folders::ProtectedFolders;
use super::{SessionManager, blocking};
use crate::Result;
use crate::model::ProjectCandidate;

impl SessionManager {
    /// Repositories the user worked in with a CLI, most recent first. Folders outside any
    /// repository are left out, and so are Brigadier's own (its data and temporary folders).
    pub async fn find_projects(&self) -> Result<Vec<ProjectCandidate>> {
        let past = self.runtime.past_folders().await;
        let home = self.runtime.cli_env().home();
        let projects = self.core.catalog().projects;
        let git = self.git.clone();
        let data_dir = self.data_dir.clone();
        blocking(move || {
            let skipped = skipped_roots(&data_dir);
            let chosen = projects
                .iter()
                .flat_map(|project| project.repos.iter().map(|repo| PathBuf::from(&repo.path)));
            let protected = ProtectedFolders::here(home.as_deref(), chosen);
            let past: Vec<_> = past
                .into_iter()
                .filter_map(|(provider, folder)| {
                    let cwd = usable_folder(&folder.cwd, &skipped, &protected)?;
                    Some((provider, folder, cwd))
                })
                .collect();
            let cwds: Vec<_> = past.iter().map(|(_, _, cwd)| cwd.clone()).collect();
            let repos = repos_of(&git, &cwds);
            let mut found: HashMap<PathBuf, ProjectCandidate> = HashMap::new();
            for (provider, folder, cwd) in past {
                let Some(repo) = repos.get(&cwd) else {
                    continue;
                };
                if !usable_root(&repo.root, &skipped, &protected) {
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

/// A past session's folder, canonical, when the scan may look at it: not Brigadier's own, and
/// not in a folder macOS asks the user about, which is told from the path before the folder is
/// touched (resolving it reads the disk) and again once it is.
pub(super) fn usable_folder(
    cwd: &Path,
    skipped: &BTreeSet<PathBuf>,
    protected: &ProtectedFolders,
) -> Option<PathBuf> {
    if protected.covers(cwd) {
        return None;
    }
    let cwd = cwd.canonicalize().ok()?;
    usable_root(&cwd, skipped, protected).then_some(cwd)
}

/// Whether a canonical folder is neither Brigadier's own nor in one macOS asks about.
pub(super) fn usable_root(
    path: &Path,
    skipped: &BTreeSet<PathBuf>,
    protected: &ProtectedFolders,
) -> bool {
    !skipped.iter().any(|root| path.starts_with(root)) && !protected.covers(path)
}

/// How long git may take to say which repository one past session's folder is in, and all of
/// them together: a folder git gets stuck in is passed over, and so are the rest once the time
/// is up, so the lists built from them always come.
const PROBE_LIMIT: Duration = Duration::from_secs(1);
const PROBES_BUDGET: Duration = Duration::from_secs(3);
/// Folders looked up at once.
const PROBES_AT_ONCE: usize = 8;

/// The repository each of `folders` (canonical) is in, those in none left out. Each folder is
/// looked up once, a few at a time, the first ones first.
pub(super) fn repos_of(git: &Git, folders: &[PathBuf]) -> HashMap<PathBuf, FoundRepo> {
    let deadline = Instant::now() + PROBES_BUDGET;
    let mut seen = HashSet::new();
    let folders: Vec<&PathBuf> = folders.iter().filter(|dir| seen.insert(*dir)).collect();
    let next = AtomicUsize::new(0);
    let found = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        for _ in 0..PROBES_AT_ONCE.min(folders.len()) {
            scope.spawn(|| {
                while let Some(folder) = folders.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        break;
                    }
                    if let Ok(Some(repo)) = git.find_repo_within(folder, left.min(PROBE_LIMIT)) {
                        found
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert((*folder).clone(), repo);
                    }
                }
            });
        }
    });
    found.into_inner().unwrap_or_else(PoisonError::into_inner)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn folders_git_never_answers_for_are_passed_over_without_holding_up_the_rest() {
        let dir =
            std::env::temp_dir().join(format!("brigadier-repo-probe-{}", uuid::Uuid::now_v7()));
        let folders: Vec<_> = (0..20).map(|n| dir.join(format!("folder-{n}"))).collect();
        for folder in &folders {
            std::fs::create_dir_all(folder).unwrap();
        }
        let stuck = dir.join("git");
        std::fs::write(&stuck, "#!/bin/sh\nsleep 20\n").unwrap();
        let mut permissions = std::fs::metadata(&stuck).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&stuck, permissions).unwrap();
        let started = Instant::now();
        let found = repos_of(&Git::new(stuck, Vec::new()), &folders);
        let took = started.elapsed();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found.is_empty());
        assert!(
            took < PROBES_BUDGET + Duration::from_secs(2),
            "took {took:?}"
        );
    }

    #[test]
    fn many_folders_git_answers_for_quickly_are_all_looked_up_in_time() {
        let git = Git::new("git".into(), std::env::vars_os().collect());
        let dir =
            std::env::temp_dir().join(format!("brigadier-repo-probe-{}", uuid::Uuid::now_v7()));
        git.init(&dir.join("repo")).unwrap();
        std::fs::create_dir_all(dir.join("plain")).unwrap();
        let root = dir.join("repo").canonicalize().unwrap();
        // Well within the time even on a busy machine: a slow wait for each git would not be.
        let mut folders: Vec<_> = (0..40).map(|n| root.join(format!("folder-{n}"))).collect();
        for folder in &folders {
            std::fs::create_dir_all(folder).unwrap();
        }
        let plain = dir.join("plain").canonicalize().unwrap();
        folders.push(plain.clone());
        folders.push(folders[0].clone());
        let found = repos_of(&git, &folders);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.len(), 40);
        assert!(found.values().all(|repo| repo.root == root));
        assert!(!found.contains_key(&plain));
    }
}
