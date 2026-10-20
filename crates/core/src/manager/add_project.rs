//! Adding a folder as a project: the Add project dialog's folder suggestions, what adding a
//! folder does, a new repository for a folder that has none, and clones.

use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use super::{SessionManager, blocking, git_error};
use crate::model::{FolderCheck, FolderEntry, FolderListing, Project, ProjectId};
use crate::{Error, Result};

/// Suggestions listed for one typed path.
const MAX_ENTRIES: usize = 100;

impl SessionManager {
    /// The folders `typed` points into whose names start with its last part: `~/code/br`
    /// lists `~/code`'s folders starting with "br", `~/code/` all of them. Hidden folders only
    /// when that part starts with a dot. An unreadable or missing folder lists nothing.
    pub async fn browse_folders(&self, typed: String) -> Result<FolderListing> {
        let home = self.runtime.cli_env().home();
        let projects = self.project_repos();
        blocking(move || {
            let Some(path) = expand(&typed, home.as_deref()) else {
                return Ok(FolderListing::default());
            };
            let (dir, prefix) = if ends_with_separator(&typed) {
                (path, String::new())
            } else {
                let prefix = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match path.parent() {
                    Some(parent) => (parent.to_owned(), prefix),
                    None => (path, String::new()),
                }
            };
            let Ok(read) = std::fs::read_dir(&dir) else {
                return Ok(FolderListing::default());
            };
            let wanted = prefix.to_lowercase();
            let mut found: Vec<(String, PathBuf)> = read
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let hidden = name.starts_with('.') && !prefix.starts_with('.');
                    let matches = name.to_lowercase().starts_with(&wanted);
                    // `is_dir` follows links, so linked folders are offered too.
                    (!hidden && matches && entry.path().is_dir()).then(|| (name, entry.path()))
                })
                .collect();
            found.sort_by_key(|(name, _)| name.to_lowercase());
            let truncated = found.len() > MAX_ENTRIES;
            found.truncate(MAX_ENTRIES);
            let entries = found
                .into_iter()
                .map(|(name, path)| {
                    let repo = path.join(".git").exists();
                    let project_id = if repo {
                        project_of(&projects, &canonical(&path))
                    } else {
                        None
                    };
                    FolderEntry {
                        name,
                        path: path.display().to_string(),
                        repo,
                        project_id,
                    }
                })
                .collect();
            Ok(FolderListing {
                dir: dir.display().to_string(),
                entries,
                truncated,
            })
        })
        .await
    }

    /// What adding the folder `typed` (`~` is the home folder) as a project does.
    pub async fn check_folder(&self, typed: String) -> Result<FolderCheck> {
        let home = self.runtime.cli_env().home();
        let projects = self.project_repos();
        let git = self.git.clone();
        let data_dir = self.data_dir.clone();
        blocking(move || {
            let Some(path) = expand(&typed, home.as_deref()) else {
                return Ok(invalid(if cfg!(windows) {
                    "Type the folder's full path, like C:\\code\\app or ~\\code\\app."
                } else {
                    "Type the folder's full path, like ~/code/app."
                }));
            };
            let data_dir = canonical(&data_dir);
            let brigadier_own = |path: &Path| path.starts_with(&data_dir);
            let metadata = match std::fs::metadata(&path) {
                Ok(metadata) => metadata,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    let path = canonical_missing(&path);
                    if brigadier_own(&path) {
                        return Ok(invalid("That folder belongs to Brigadier itself."));
                    }
                    return Ok(FolderCheck::Missing {
                        name: folder_name(&path),
                        path: path.display().to_string(),
                    });
                }
                Err(err) => return Ok(invalid(&format!("{}: {err}", path.display()))),
            };
            if !metadata.is_dir() {
                return Ok(invalid("That's a file. Choose a folder."));
            }
            let path = canonical(&path);
            if brigadier_own(&path) {
                return Ok(invalid("That folder belongs to Brigadier itself."));
            }
            if let Some(repo) = git.find_repo(&path).map_err(git_error)? {
                return Ok(FolderCheck::Repo {
                    name: folder_name(&repo.root),
                    nested: repo.root != path,
                    project_id: project_of(&projects, &repo.root),
                    root: repo.root.display().to_string(),
                });
            }
            let home = home.map(|home| canonical(&home));
            if home.is_some_and(|home| home.starts_with(&path)) {
                return Ok(invalid(
                    "Brigadier won't make a repository of your home folder or a folder above it. Choose a folder inside it.",
                ));
            }
            Ok(FolderCheck::Plain {
                name: folder_name(&path),
                path: path.display().to_string(),
            })
        })
        .await
    }

    /// Adds the folder `typed` as a project: the top-level folder of the repository it is in,
    /// or with `init`, a new repository made there (the folder too, when missing).
    pub async fn add_project(&self, typed: String, name: String, init: bool) -> Result<Project> {
        let root = match self.check_folder(typed).await? {
            FolderCheck::Repo { root, .. } => root,
            FolderCheck::Plain { path, .. } | FolderCheck::Missing { path, .. } if init => {
                let git = self.git.clone();
                blocking(move || {
                    let dir = PathBuf::from(&path);
                    if let Some(err) = git.init(&dir).map_err(git_error)? {
                        tracing::warn!(error = %err, path, "no first commit in the new repository");
                    }
                    Ok(canonical(&dir).display().to_string())
                })
                .await?
            }
            FolderCheck::Plain { path, .. } => {
                return Err(Error::Invalid(format!("{path} is not in a git repository")));
            }
            FolderCheck::Missing { path, .. } => {
                return Err(Error::Invalid(format!("{path} does not exist")));
            }
            FolderCheck::Invalid { reason } => return Err(Error::Invalid(reason)),
        };
        self.created(name, root).await
    }

    /// Clones `url` into the new folder `folder` in `parent` (created when missing) and adds
    /// the clone as a project.
    pub async fn clone_project(
        &self,
        url: String,
        parent: String,
        folder: String,
        name: String,
    ) -> Result<Project> {
        let url = url.trim().to_owned();
        if url.is_empty() {
            return Err(Error::Invalid("Paste the repository's URL.".into()));
        }
        if url.starts_with('-') {
            return Err(Error::Invalid(format!("{url} is not a repository URL")));
        }
        let folder = folder.trim().to_owned();
        if folder.is_empty()
            || folder == "."
            || folder == ".."
            || folder.contains(['/', '\\', '\0'])
        {
            return Err(Error::Invalid(format!(
                "\"{folder}\" can't be a folder name"
            )));
        }
        let home = self.runtime.cli_env().home();
        let parent = expand(&parent, home.as_deref()).ok_or_else(|| {
            Error::Invalid("Type the full path of the folder to clone into.".into())
        })?;
        let dest = parent.join(&folder);
        let data_dir = self.data_dir.clone();
        let git = self.git.clone();
        let root = blocking(move || {
            if dest.exists() {
                return Err(Error::Invalid(format!(
                    "{} already exists. Choose another folder name.",
                    dest.display()
                )));
            }
            std::fs::create_dir_all(&parent)
                .map_err(|err| Error::Invalid(format!("{}: {err}", parent.display())))?;
            let dest = canonical(&parent).join(&folder);
            if dest.starts_with(canonical(&data_dir)) {
                return Err(Error::Invalid(
                    "That folder belongs to Brigadier itself.".into(),
                ));
            }
            git.clone_into(&url, &dest).map_err(git_error)?;
            Ok(dest.display().to_string())
        })
        .await?;
        self.created(name, root).await
    }

    /// Creates the project on `root` and opens its Brain and index.
    async fn created(&self, name: String, root: String) -> Result<Project> {
        let project = self.core.create_project(name, Some(root)).await?;
        self.project_changed(&project).await;
        Ok(project)
    }

    /// Every project's repositories, to tell which folders are projects already.
    pub(super) fn project_repos(&self) -> Vec<(ProjectId, String)> {
        self.core
            .catalog()
            .projects
            .into_iter()
            .flat_map(|project| {
                let id = project.id;
                project
                    .repos
                    .into_iter()
                    .map(move |repo| (id.clone(), repo.path))
            })
            .collect()
    }
}

fn invalid(reason: &str) -> FolderCheck {
    FolderCheck::Invalid {
        reason: reason.to_owned(),
    }
}

/// `typed` as an absolute path: `~` is the home folder. `None` for a relative path.
fn expand(typed: &str, home: Option<&Path>) -> Option<PathBuf> {
    let typed = typed.trim();
    let rest = typed
        .strip_prefix('~')
        .filter(|rest| rest.is_empty() || rest.starts_with(['/', MAIN_SEPARATOR]));
    let path = match rest {
        Some(rest) => home?.join(rest.trim_start_matches(['/', MAIN_SEPARATOR])),
        None => PathBuf::from(typed),
    };
    path.is_absolute().then_some(path)
}

fn ends_with_separator(typed: &str) -> bool {
    let typed = typed.trim();
    typed.ends_with('/') || typed.ends_with(MAIN_SEPARATOR)
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

/// A missing path with its deepest existing folder made canonical (`/tmp/new` →
/// `/private/tmp/new` on macOS), so it compares with canonical paths.
fn canonical_missing(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    while !existing.exists() {
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
    let mut out = canonical(existing);
    out.extend(rest.iter().rev());
    out
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn project_of(projects: &[(ProjectId, String)], root: &Path) -> Option<ProjectId> {
    let root = root.display().to_string();
    projects
        .iter()
        .find(|(_, path)| *path == root)
        .map(|(id, _)| id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_the_home_folder_and_rejects_relative_paths() {
        // Absolute where the tests run: a drive's on Windows.
        let (home, srv) = if cfg!(windows) {
            (Path::new(r"C:\Users\me"), r"C:\srv\app")
        } else {
            (Path::new("/home/me"), "/srv/app")
        };
        assert_eq!(expand("~", Some(home)), Some(home.to_path_buf()));
        assert_eq!(
            expand(" ~/code/app ", Some(home)),
            Some(home.join("code/app"))
        );
        assert_eq!(expand(srv, Some(home)), Some(PathBuf::from(srv)));
        assert_eq!(expand("~other/app", Some(home)), None);
        assert_eq!(expand("code/app", Some(home)), None);
        assert_eq!(expand("~/app", None), None);
    }

    #[test]
    fn a_missing_path_keeps_its_existing_part_canonical() {
        let dir = std::env::temp_dir();
        let missing = dir.join("brigadier-missing-folder-test").join("deeper");
        assert_eq!(
            canonical_missing(&missing),
            canonical(&dir).join("brigadier-missing-folder-test/deeper")
        );
    }
}
