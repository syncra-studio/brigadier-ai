//! Watching the configs and skill folders the user's catalog is read from.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_full::{DebounceEventResult, Debouncer, NoCache};

/// What to watch: config files (and what they link to), and folders whose whole tree counts.
#[derive(Debug, Clone, Default)]
pub(crate) struct Watched {
    pub files: Vec<PathBuf>,
    pub trees: Vec<PathBuf>,
}

/// The folders watched and the events that count, symlinks resolved (events name real paths).
#[derive(Debug, Default, PartialEq)]
struct Plan {
    /// Each file's folder, watched alone (not its subfolders).
    folders: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
    trees: BTreeSet<PathBuf>,
}

impl Plan {
    /// Only what exists: a config or skill folder made later is found on the next refresh.
    fn new(watched: &Watched) -> Self {
        let mut plan = Self::default();
        for file in &watched.files {
            if let (Some(parent), Some(name)) = (file.parent(), file.file_name())
                && let Ok(parent) = std::fs::canonicalize(parent)
            {
                plan.files.insert(parent.join(name));
                plan.folders.insert(parent);
            }
            // A config linked from elsewhere (a dotfiles folder) changes where it really is.
            if let Ok(target) = std::fs::canonicalize(file)
                && let Some(folder) = target.parent()
            {
                plan.folders.insert(folder.to_owned());
                plan.files.insert(target);
            }
        }
        let mut trees: Vec<PathBuf> = watched
            .trees
            .iter()
            .filter_map(|tree| std::fs::canonicalize(tree).ok())
            .collect();
        trees.sort();
        for tree in trees {
            if !plan.trees.iter().any(|known| tree.starts_with(known)) {
                plan.trees.insert(tree);
            }
        }
        plan
    }

    fn counts(&self, path: &Path) -> bool {
        self.files.contains(path) || self.trees.iter().any(|tree| path.starts_with(tree))
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

    /// Watches `watched` instead of what it watched.
    pub fn rewatch(&mut self, watched: &Watched) {
        let next = Plan::new(watched);
        let mut plan = self.plan.write().unwrap_or_else(|p| p.into_inner());
        if *plan == next {
            return;
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
    }
}
