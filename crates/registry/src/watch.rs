//! Watching the configs and skill folders the user's catalog is read from.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache};

/// What to watch: config files (and what they link to), folders whose whole tree counts, and
/// folders whose entries count (a plugin's versions).
#[derive(Debug, Clone, Default)]
pub(crate) struct Watched {
    pub files: Vec<PathBuf>,
    pub trees: Vec<PathBuf>,
    pub dirs: Vec<PathBuf>,
}

/// The folders watched and the events that count, symlinks resolved (events name real paths).
#[derive(Debug, Default, PartialEq)]
struct Plan {
    /// Each file's folder, watched alone (not its subfolders).
    folders: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
    trees: BTreeSet<PathBuf>,
    /// Folders whose own entries count (in [`Self::folders`]).
    dirs: BTreeSet<PathBuf>,
    /// Configs and roots whose folder doesn't exist yet, under their nearest existing
    /// ancestor (watched in [`Self::folders`]): making any folder on the way counts.
    missing: BTreeSet<PathBuf>,
}

/// `path` under its nearest existing ancestor, symlinks resolved, and that ancestor.
fn nearest(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut rest = Vec::new();
    let mut at = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(at) {
            let target = rest
                .iter()
                .rev()
                .fold(real.clone(), |dir, name| dir.join(name));
            return Some((real, target));
        }
        rest.push(at.file_name()?.to_owned());
        at = at.parent()?;
    }
}

impl Plan {
    /// Only what exists: a config or skill folder made later is found on the next refresh.
    fn new(watched: &Watched) -> Self {
        let mut plan = Self::default();
        for file in &watched.files {
            if let (Some(parent), Some(name)) = (file.parent(), file.file_name()) {
                if let Ok(parent) = std::fs::canonicalize(parent) {
                    plan.files.insert(parent.join(name));
                    plan.folders.insert(parent);
                } else if let Some((ancestor, target)) = nearest(file) {
                    plan.folders.insert(ancestor);
                    plan.missing.insert(target);
                }
            }
            // A config linked from elsewhere (a dotfiles folder) changes where it really is.
            if let Ok(target) = std::fs::canonicalize(file)
                && let Some(folder) = target.parent()
            {
                plan.folders.insert(folder.to_owned());
                plan.files.insert(target);
            }
        }
        for dir in &watched.dirs {
            if let Ok(dir) = std::fs::canonicalize(dir) {
                plan.folders.insert(dir.clone());
                plan.dirs.insert(dir);
            }
        }
        let mut trees = Vec::new();
        for tree in &watched.trees {
            match std::fs::canonicalize(tree) {
                Ok(tree) => trees.push(tree),
                Err(_) => {
                    if let Some((ancestor, target)) = nearest(tree) {
                        plan.folders.insert(ancestor);
                        plan.missing.insert(target);
                    }
                }
            }
        }
        trees.sort();
        for tree in trees {
            if !plan.trees.iter().any(|known| tree.starts_with(known)) {
                plan.trees.insert(tree);
            }
        }
        plan
    }

    fn counts(&self, path: &Path) -> bool {
        self.files.contains(path)
            || self.dirs.contains(path)
            || path
                .parent()
                .is_some_and(|parent| self.dirs.contains(parent))
            || self.trees.iter().any(|tree| path.starts_with(tree))
            || self.missing.iter().any(|target| target.starts_with(path))
    }

    fn watches(&self) -> impl Iterator<Item = (&PathBuf, RecursiveMode)> {
        self.folders
            .iter()
            .map(|folder| (folder, RecursiveMode::NonRecursive))
            .chain(
                self.trees
                    .iter()
                    .map(|tree| (tree, RecursiveMode::Recursive)),
            )
    }
}

/// Calls back, half a second after the last change that counts.
pub(crate) struct Watcher {
    debouncer: Debouncer<notify::RecommendedWatcher, NoCache>,
    plan: Arc<RwLock<Plan>>,
}

impl Watcher {
    pub fn start(watched: &Watched, changed: Box<dyn Fn() + Send + Sync>) -> std::io::Result<Self> {
        let plan = Arc::new(RwLock::new(Plan::default()));
        let filter = plan.clone();
        // No file-id cache: an event only says to read the configs again.
        let debouncer =
            notify_debouncer_full::new_debouncer_opt::<_, notify::RecommendedWatcher, _>(
                Duration::from_millis(500),
                None,
                move |result: DebounceEventResult| {
                    let counts = match &result {
                        Ok(events) => {
                            let plan = filter.read().unwrap_or_else(|p| p.into_inner());
                            events.iter().any(|event| {
                                event.need_rescan()
                                    || event.paths.iter().any(|path| plan.counts(path))
                            })
                        }
                        Err(_) => true,
                    };
                    if counts {
                        changed();
                    }
                },
                NoCache,
                notify::Config::default(),
            )
            .map_err(std::io::Error::other)?;
        let mut watcher = Self { debouncer, plan };
        watcher.rewatch(watched);
        Ok(watcher)
    }

    /// Watches `watched` instead of what it watched; whether that changed.
    pub fn rewatch(&mut self, watched: &Watched) -> bool {
        let next = Plan::new(watched);
        let mut plan = self.plan.write().unwrap_or_else(|p| p.into_inner());
        if *plan == next {
            return false;
        }
        for (path, _) in plan.watches() {
            let _ = self.debouncer.unwatch(path);
        }
        for (path, mode) in next.watches() {
            if let Err(error) = self.debouncer.watch(path, mode) {
                tracing::warn!(path = %path.display(), %error, "can't watch for MCP and skill changes");
            }
        }
        *plan = next;
        true
    }
}
