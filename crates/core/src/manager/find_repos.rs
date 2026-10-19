//! The Add project dialog's list: the git repositories on this computer, most recent first.
//! The ones the user's own CLI sessions worked in and the projects' come first-hand; the rest
//! from a quick, bounded look through the usual code folders, never the whole disk.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use brigadier_git::Git;

use super::past_projects::{repos_of, skipped_roots, usable_folder, usable_root};
use super::protected_folders::ProtectedFolders;
use super::{SessionManager, blocking};
use crate::Result;
use crate::model::{ProjectId, RepoSuggestion};

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
    // Never looked at on macOS, which asks the user about Documents.
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
            let chosen = projects.iter().map(|(_, path)| PathBuf::from(path));
            let protected = ProtectedFolders::here(home.as_deref(), chosen);
            let past = past
                .into_iter()
                .map(|(_, folder)| (folder.cwd, folder.last_active_ms))
                .collect();
            let skipped = skipped_roots(&data_dir);
            Ok(suggest_repos(
                &git,
                past,
                home.as_deref(),
                &projects,
                &skipped,
                &protected,
            ))
        })
        .await
    }
}

/// [`SessionManager::find_repos`]' list, from the past sessions' folders and when each was
/// last used, the home folder and the projects' repositories. Brigadier's own folders
/// (`skipped`) and the ones macOS asks the user about (`protected`) are left out unlooked at.
fn suggest_repos(
    git: &Git,
    past: Vec<(PathBuf, i64)>,
    home: Option<&Path>,
    projects: &[(ProjectId, String)],
    skipped: &BTreeSet<PathBuf>,
    protected: &ProtectedFolders,
) -> Vec<RepoSuggestion> {
    let is_skipped = |path: &Path| skipped.iter().any(|root| path.starts_with(root));
    // The repository's top-level folder → when it was last used.
    let mut found: HashMap<PathBuf, i64> = HashMap::new();
    let mut note = |root: PathBuf, used: i64| {
        let at = found.entry(root).or_insert(i64::MIN);
        *at = (*at).max(used);
    };
    // Where the user worked lately: those repositories, and the folders beside them.
    let mut near = Vec::new();
    let past: Vec<_> = past
        .into_iter()
        .filter_map(|(cwd, used)| Some((usable_folder(&cwd, skipped, protected)?, used)))
        .collect();
    let cwds: Vec<_> = past.iter().map(|(cwd, _)| cwd.clone()).collect();
    let repos = repos_of(git, &cwds);
    for (cwd, used) in past {
        if let Some(repo) = repos.get(&cwd)
            && usable_root(&repo.root, skipped, protected)
        {
            near.extend(repo.root.parent().map(Path::to_owned));
            note(repo.root.clone(), used);
        }
    }
    // The projects' own, wherever they are: the user chose them.
    for (_, path) in projects {
        let root = canonical(Path::new(path));
        near.extend(root.parent().map(Path::to_owned));
        note(root, i64::MIN);
    }
    let mut looks: Vec<(PathBuf, usize)> = Vec::new();
    if let Some(home) = home {
        looks.push((home.to_owned(), 1));
        looks.extend(
            CODE_FOLDERS
                .iter()
                .map(|name| (home.join(name), CODE_DEPTH)),
        );
    }
    looks.extend(near.into_iter().map(|dir| (dir, 1)));
    for root in look_for_repos(&looks, protected, Instant::now() + LOOK_BUDGET) {
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
    repos
}

/// The repositories' top-level folders (canonical) found in each `(folder, depth)` of `looks`:
/// a folder holding a `.git` folder is one, and isn't looked into; others are, `depth` levels
/// down. Hidden and skipped folders, links and the folders macOS asks the user about
/// (`protected`, told before they are touched) are passed over; the look stops at `deadline`
/// or after [`MAX_FOLDERS`] folders.
fn look_for_repos(
    looks: &[(PathBuf, usize)],
    protected: &ProtectedFolders,
    deadline: Instant,
) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    let mut repos = Vec::new();
    let mut read = 0usize;
    // Breadth first, so the shallow folders are all seen before a deep one uses the budget.
    let mut queue: std::collections::VecDeque<(PathBuf, usize)> = looks.iter().cloned().collect();
    while let Some((dir, depth)) = queue.pop_front() {
        if read >= MAX_FOLDERS || Instant::now() >= deadline {
            break;
        }
        if protected.covers(&dir) {
            continue;
        }
        let Ok(dir) = dir.canonicalize() else {
            continue;
        };
        // `~/code` and `~/Code` are one folder on a case-insensitive disk.
        if protected.covers(&dir) || !seen.insert(dir.clone()) {
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
            if protected.covers(&path) {
                continue;
            }
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
        let mut found = look_for_repos(
            &looks,
            &ProtectedFolders::default(),
            Instant::now() + LOOK_BUDGET,
        );
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
            look_for_repos(
                &twice,
                &ProtectedFolders::default(),
                Instant::now() + LOOK_BUDGET
            ),
            vec![home.join("code/app")]
        );
        assert!(look_for_repos(&twice, &ProtectedFolders::default(), Instant::now()).is_empty());
    }

    /// The first run's scan in a home folder whose past sessions ran in folders macOS asks
    /// the user about: git is never run there and nothing in them is listed, so no prompt is
    /// shown; the project the user chose in Documents still is.
    #[cfg(unix)]
    #[test]
    fn past_sessions_in_folders_macos_guards_are_never_looked_at_unless_the_user_chose_them() {
        let temp = Temp::new();
        let home = temp.0.join("home");
        let real = Git::new("git".into(), std::env::vars_os().collect());
        real.init(&home.join("code/app")).unwrap();
        let guarded = [
            "Documents/app",
            "documents/other",
            "Desktop/site",
            "Downloads/tool",
            "Documents/GitHub/lib",
            "Library/Mobile Documents/com~apple~CloudDocs/notes",
        ];
        for folder in guarded {
            real.init(&home.join(folder)).unwrap();
            std::fs::create_dir_all(home.join(folder).join("src")).unwrap();
        }
        repo(&home.join("Documents/picked"));
        // git, writing down each folder it is run in.
        let log = temp.0.join("git-runs");
        let logging = temp.0.join("git");
        std::fs::write(
            &logging,
            format!("#!/bin/sh\npwd >> '{}'\nexec git \"$@\"\n", log.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&logging).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
        std::fs::set_permissions(&logging, permissions).unwrap();
        let git = Git::new(logging, std::env::vars_os().collect());
        let mut past: Vec<_> = guarded
            .iter()
            .map(|folder| (home.join(folder).join("src"), 1_000))
            .collect();
        past.push((home.join("code/app"), 2_000));
        let picked = home.join("Documents/picked");
        let projects = vec![(ProjectId("p".into()), picked.display().to_string())];
        let protected = ProtectedFolders::macos(Some(&home), [picked.clone()]);
        let repos = suggest_repos(
            &git,
            past,
            Some(&home),
            &projects,
            &BTreeSet::new(),
            &protected,
        );
        let runs = std::fs::read_to_string(&log).unwrap_or_default();
        let mut runs: Vec<_> = runs.lines().map(PathBuf::from).collect();
        runs.sort();
        runs.dedup();
        assert_eq!(runs, vec![home.join("code/app")]);
        let mut listed: Vec<_> = repos.iter().map(|repo| PathBuf::from(&repo.path)).collect();
        listed.sort();
        assert_eq!(listed, vec![picked, home.join("code/app")]);
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
