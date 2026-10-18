//! The Add project dialog's list: the git repositories on this computer, most recent first.
//! The ones the user's own CLI sessions worked in and the projects' come first-hand; the rest
//! from a quick, bounded look through the usual code folders, never the whole disk.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::past_projects::skipped_roots;
use super::{SessionManager, blocking};
use crate::Result;
use crate::model::RepoSuggestion;

/// The folders in the home folder code usually sits in, looked through two levels deep
/// (`~/code/app`, `~/code/team/app`).
const CODE_FOLDERS: &[&str] = &[
    "code",
    "Code",
    "Development",
    "Developer",
    "dev",
    "Projects",
    "projects",
    "src",
    "repos",
    "git",
    "GitHub",
    "workspace",
    "work",
    "Sites",
    "Documents/GitHub",
    "Documents/Code",
    "Documents/Projects",
];
/// How deep the look goes below a code folder.
const CODE_DEPTH: usize = 2;
/// Folders read in one look, and how long it may take, at most.
const MAX_FOLDERS: usize = 4000;
const LOOK_BUDGET: Duration = Duration::from_millis(1500);
/// Repositories listed, at most.
const MAX_REPOS: usize = 300;
/// Folders never looked into: packages, build output, the system's own.
const SKIPPED_NAMES: &[&str] = &[
    "node_modules",
    "target",
    "vendor",
    "Library",
    "Applications",
    "Pictures",
    "Music",
    "Movies",
];

impl SessionManager {
    /// The repositories on this computer, most recent first, each marked with its project.
    pub async fn find_repos(&self) -> Result<Vec<RepoSuggestion>> {
        let past = self.runtime.past_folders().await;
        let home = self.runtime.cli_env().home();
        let projects = self.project_repos();
        let git = self.git.clone();
        let data_dir = self.data_dir.clone();
        blocking(move || {
            let skipped = skipped_roots(&data_dir);
            let is_skipped = |path: &Path| skipped.iter().any(|root| path.starts_with(root));
            // The repository's top-level folder → when it was last used.
            let mut found: HashMap<PathBuf, i64> = HashMap::new();
            let mut note = |root: PathBuf, used: i64| {
                let at = found.entry(root).or_insert(i64::MIN);
                *at = (*at).max(used);
            };
            // Where the user worked lately: those repositories, and the folders beside them.
            let mut near = Vec::new();
            for (_, folder) in past {
                let Ok(cwd) = folder.cwd.canonicalize() else {
                    continue;
                };
                if is_skipped(&cwd) {
                    continue;
                }
                if let Ok(Some(repo)) = git.find_repo(&cwd)
                    && !is_skipped(&repo.root)
                {
                    near.extend(repo.root.parent().map(Path::to_owned));
                    note(repo.root, folder.last_active_ms);
                }
            }
            // The projects' own, wherever they are.
            for (_, path) in &projects {
                let root = canonical(Path::new(path));
                near.extend(root.parent().map(Path::to_owned));
                note(root, i64::MIN);
            }
            let mut looks: Vec<(PathBuf, usize)> = Vec::new();
            if let Some(home) = home.as_deref() {
                looks.push((home.to_owned(), 1));
                looks.extend(
                    CODE_FOLDERS
                        .iter()
                        .map(|name| (home.join(name), CODE_DEPTH)),
                );
            }
            looks.extend(near.into_iter().map(|dir| (dir, 1)));
            for root in look_for_repos(&looks, Instant::now() + LOOK_BUDGET) {
                if !is_skipped(&root) {
                    note(root, i64::MIN);
                }
            }
            let projects: Vec<_> = projects
                .iter()
                .map(|(id, path)| (id.clone(), canonical(Path::new(path))))
                .collect();
            let mut repos: Vec<RepoSuggestion> = found
                .into_iter()
                .map(|(root, used)| {
                    let path = root.display().to_string();
                    RepoSuggestion {
                        name: root
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.clone()),
                        last_used_ms: used.max(git_activity_ms(&root)).max(0),
                        project_id: projects
                            .iter()
                            .find(|(_, repo)| *repo == root)
                            .map(|(id, _)| id.clone()),
                        path,
                    }
                })
                .collect();
            repos.sort_by(|a, b| {
                b.last_used_ms
                    .cmp(&a.last_used_ms)
                    .then_with(|| a.name.cmp(&b.name))
            });
            repos.truncate(MAX_REPOS);
            Ok(repos)
        })
        .await
    }
}

/// The repositories' top-level folders (canonical) found in each `(folder, depth)` of `looks`:
/// a folder holding a `.git` folder is one, and isn't looked into; others are, `depth` levels
/// down. Hidden and skipped folders and links are passed over; the look stops at `deadline`
/// or after [`MAX_FOLDERS`] folders.
fn look_for_repos(looks: &[(PathBuf, usize)], deadline: Instant) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut repos = Vec::new();
    let mut read = 0usize;
    // Breadth first, so the shallow folders are all seen before a deep one uses the budget.
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = looks.iter().cloned().collect();
    while let Some((dir, depth)) = queue.pop_front() {
        if read >= MAX_FOLDERS || Instant::now() >= deadline {
            break;
        }
        let Ok(dir) = dir.canonicalize() else {
            continue;
        };
        // `~/code` and `~/Code` are one folder on a case-insensitive disk.
        if !seen.insert(dir.clone()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        read += 1;
        for entry in entries.filter_map(|entry| entry.ok()) {
            // `file_type` doesn't follow links: a link can't send the look round in circles.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIPPED_NAMES.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            if path.join(".git").is_dir() {
                repos.push(canonical(&path));
            } else if depth > 1 {
                queue.push_back((path, depth - 1));
            }
        }
    }
    repos
}

/// When git last touched the repository: its index, HEAD and their logs, in ms since the
/// epoch (`i64::MIN` when none can be read).
fn git_activity_ms(root: &Path) -> i64 {
    let git = root.join(".git");
    ["index", "HEAD", "logs/HEAD", "FETCH_HEAD"]
        .iter()
        .filter_map(|file| std::fs::metadata(git.join(file)).ok()?.modified().ok())
        .map(ms_since_epoch)
        .max()
        .unwrap_or(i64::MIN)
}

fn ms_since_epoch(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH).map_or(0, |since| {
        i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
    })
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch folder, removed when dropped.
    struct Temp(PathBuf);

    impl Temp {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-find-repos-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(canonical(&dir))
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn repo(path: &Path) {
        std::fs::create_dir_all(path.join(".git")).unwrap();
    }

    #[test]
    fn looks_two_levels_into_code_folders_and_never_into_repos_or_packages() {
        let temp = Temp::new();
        let home = temp.0.clone();
        repo(&home.join("direct"));
        repo(&home.join("code/app"));
        repo(&home.join("code/team/lib"));
        repo(&home.join("code/team/deeper/too-deep"));
        repo(&home.join("code/app/nested"));
        repo(&home.join("code/node_modules/package"));
        repo(&home.join("code/.hidden/secret"));
        repo(&home.join("notes/inside-notes"));
        std::fs::create_dir_all(home.join("plain")).unwrap();
        // A `.git` file is a linked worktree's: its main checkout stands for it.
        std::fs::create_dir_all(home.join("code/worktree")).unwrap();
        std::fs::write(home.join("code/worktree/.git"), "gitdir: elsewhere").unwrap();
        let looks = vec![(home.clone(), 1), (home.join("code"), CODE_DEPTH)];
        let mut found = look_for_repos(&looks, Instant::now() + LOOK_BUDGET);
        found.sort();
        assert_eq!(
            found,
            vec![
                home.join("code/app"),
                home.join("code/team/lib"),
                home.join("direct")
            ]
        );
    }

    #[test]
    fn a_folder_named_twice_is_looked_through_once_and_the_deadline_stops_the_look() {
        let temp = Temp::new();
        let home = temp.0.clone();
        repo(&home.join("code/app"));
        let twice = vec![(home.join("code"), 1), (home.join("code/../code"), 1)];
        assert_eq!(
            look_for_repos(&twice, Instant::now() + LOOK_BUDGET),
            vec![home.join("code/app")]
        );
        assert!(look_for_repos(&twice, Instant::now()).is_empty());
    }

    #[test]
    fn git_activity_is_the_newest_of_its_files() {
        let dir = Temp::new();
        assert_eq!(git_activity_ms(&dir.0), i64::MIN);
        repo(&dir.0);
        std::fs::write(dir.0.join(".git/HEAD"), "ref: refs/heads/main").unwrap();
        let now = ms_since_epoch(SystemTime::now());
        assert!((now - git_activity_ms(&dir.0)).abs() < 60_000);
    }
}
