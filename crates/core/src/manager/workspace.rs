//! The right sidebar's view of a checkout: its files with what it ignores, a folder read on
//! its own when an ignored one is opened, and a watch that says when the checkout changed.
//! It only reads, and never outside the checkout.

use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use super::{MENTION_FILES, SessionManager, blocking, git_error};
use crate::work::{WorkspaceEntry, WorkspaceFolder, WorkspaceTarget, WorkspaceTree};
use crate::{Error, Result};

/// Ignored entries listed in the tree; a checkout ignoring more lists the first ones.
const IGNORED_KEPT: usize = 5_000;
/// Entries listed when a folder is read on its own.
const FOLDER_ENTRIES: usize = 2_000;
/// How long the watch waits for a burst of changes (a save, a checkout) to end.
const SETTLE: Duration = Duration::from_millis(200);
/// How often a checkout that can't be watched is read again anyway.
const POLL: Duration = Duration::from_secs(10);

impl SessionManager {
    /// The folder a target's checkout is in: a session's, or a project's folder.
    pub(super) fn workspace_dir(&self, target: &WorkspaceTarget) -> Result<PathBuf> {
        match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                self.checkout_dir(conversation_id).map(PathBuf::from)
            }
            WorkspaceTarget::Project { project_id } => self.project_checkout(project_id),
        }
    }

    /// A checkout's files and what it ignores, for the right sidebar's tree.
    pub async fn workspace_tree(&self, target: &WorkspaceTarget) -> Result<WorkspaceTree> {
        let dir = self.workspace_dir(target)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            let (files, truncated) = repo.files(MENTION_FILES, None).map_err(git_error)?;
            let (ignored, ignored_truncated) = repo.ignored(IGNORED_KEPT).map_err(git_error)?;
            Ok(WorkspaceTree {
                files,
                truncated,
                ignored,
                ignored_truncated,
            })
        })
        .await
    }

    /// The folder at `path`, relative to the checkout's root, read from disk: what an ignored
    /// folder holds, which git doesn't list.
    pub async fn workspace_folder(
        &self,
        target: &WorkspaceTarget,
        path: String,
    ) -> Result<WorkspaceFolder> {
        let dir = self.workspace_dir(target)?;
        let git = self.git.clone();
        blocking(move || {
            let root = git.open(&dir).map_err(git_error)?.root().to_path_buf();
            read_folder(&root, &path)
        })
        .await
    }

    /// Watches a target's checkout: the watch's `changed` resolves after its files, its index,
    /// its HEAD or its branch change, a burst of changes once. What happens only in ignored folders (a
    /// build's output) doesn't count. A checkout that can't be watched is read again every
    /// so often instead.
    pub async fn watch_workspace(&self, target: &WorkspaceTarget) -> Result<WorkspaceWatch> {
        let dir = self.workspace_dir(target)?;
        let git = self.git.clone();
        let (root, status_files, ignored) = blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            let root = repo.root().to_path_buf();
            let status_files = repo.status_files().map_err(git_error)?;
            let ignored = repo.ignored(IGNORED_KEPT).map_err(git_error)?.0;
            Ok((root, status_files, ignored))
        })
        .await?;
        let git = self.git.clone();
        let reread_root = root.clone();
        let reread = move || {
            let repo = git.open(&reread_root).ok()?;
            let ignored = repo.ignored(IGNORED_KEPT).ok()?.0;
            Some((ignored, repo.status_files().ok()?))
        };
        Ok(WorkspaceWatch::start(root, status_files, ignored, reread))
    }
}

/// Reads one folder of a checkout. The path is relative, without `.`, `..` or `.git`, and once
/// resolved (links followed) still inside the checkout.
fn read_folder(root: &Path, path: &str) -> Result<WorkspaceFolder> {
    let relative = Path::new(path);
    let outside = || Error::Invalid(format!("`{path}` is not a folder in the checkout"));
    if path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(outside());
    }
    let unreadable = |err: std::io::Error| Error::Invalid(format!("couldn't read {path}: {err}"));
    let root = root.canonicalize().map_err(unreadable)?;
    let full = root.join(relative).canonicalize().map_err(|err| {
        if err.kind() == std::io::ErrorKind::NotFound {
            Error::NotFound(path.to_owned())
        } else {
            unreadable(err)
        }
    })?;
    if !full.starts_with(&root)
        || full == root
        || full
            .strip_prefix(&root)
            .is_ok_and(|path| path.components().any(|part| part.as_os_str() == ".git"))
        || !full.is_dir()
    {
        return Err(outside());
    }
    let mut entries: Vec<WorkspaceEntry> = std::fs::read_dir(&full)
        .map_err(unreadable)?
        .filter_map(|entry| entry.ok())
        .map(|entry| WorkspaceEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            // A link counts as what it leads to; opening one that leads out is refused.
            folder: std::fs::metadata(entry.path()).is_ok_and(|meta| meta.is_dir()),
        })
        .filter(|entry| entry.name != ".git")
        .collect();
    entries.sort_by(|a, b| b.folder.cmp(&a.folder).then_with(|| a.name.cmp(&b.name)));
    let truncated = entries.len() > FOLDER_ENTRIES;
    entries.truncate(FOLDER_ENTRIES);
    Ok(WorkspaceFolder { entries, truncated })
}

/// A watched checkout. Dropping it stops the watch.
pub struct WorkspaceWatch {
    changes: mpsc::Receiver<()>,
    stop: Arc<AtomicBool>,
    _watcher: Arc<Mutex<Option<notify::RecommendedWatcher>>>,
}

impl Drop for WorkspaceWatch {
    fn drop(&mut self) {
        // The thread notices within one wait and ends on its own; nothing here blocks.
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl WorkspaceWatch {
    /// Resolves once the checkout changed since the last time it did; `None` once the watch
    /// has ended.
    pub async fn changed(&mut self) -> Option<()> {
        self.changes.recv().await
    }

    fn start(
        root: PathBuf,
        status_files: Vec<PathBuf>,
        ignored: Vec<String>,
        reread: impl Fn() -> Option<(Vec<String>, Vec<PathBuf>)> + Send + 'static,
    ) -> Self {
        // One pending change is enough: the reader reads the whole checkout again.
        let (changes_tx, changes) = mpsc::channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let root = resolved(&root);
        let status_files: Vec<PathBuf> = status_files.iter().map(|file| resolved(file)).collect();
        let (events_tx, events) = std_mpsc::channel::<notify::Result<notify::Event>>();
        let watcher = watcher(&root, &status_files, events_tx)
            .inspect_err(|err| tracing::warn!(%err, root = %root.display(), "watching a checkout failed; reading it again every so often"))
            .ok();
        let mut watching = watcher.is_some();
        let watcher = Arc::new(Mutex::new(watcher));
        let thread_watcher = watcher.clone();
        let thread_stop = stop.clone();
        let spawned = thread::Builder::new()
            .name("workspace-watcher".into())
            .spawn(move || {
                let mut filter = ChangeFilter::new(root, status_files, &ignored);
                let mut polled = Instant::now();
                while !thread_stop.load(Ordering::SeqCst) {
                    let first = match events.recv_timeout(SETTLE) {
                        Ok(event) => event,
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {
                            if !watching && polled.elapsed() >= POLL {
                                polled = Instant::now();
                                if changes_tx.try_send(()).is_err() && changes_tx.is_closed() {
                                    return;
                                }
                            }
                            continue;
                        }
                        // Without a watcher there is nothing to hear: wait out the poll instead.
                        Err(std_mpsc::RecvTimeoutError::Disconnected) if !watching => {
                            thread::sleep(SETTLE);
                            if polled.elapsed() >= POLL {
                                polled = Instant::now();
                                if changes_tx.try_send(()).is_err() && changes_tx.is_closed() {
                                    return;
                                }
                            }
                            continue;
                        }
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => return,
                    };
                    let mut batch = vec![first];
                    let settled = Instant::now() + SETTLE;
                    while let Some(left) = settled.checked_duration_since(Instant::now()) {
                        match events.recv_timeout(left) {
                            Ok(event) => batch.push(event),
                            Err(_) => break,
                        }
                    }
                    if batch.iter().any(|event| event.is_err()) {
                        // A backend that fails after starting must also get the poll fallback.
                        watching = false;
                    }
                    if !batch.iter().any(|event| filter.counts(event)) {
                        continue;
                    }
                    if changes_tx.try_send(()).is_err() && changes_tx.is_closed() {
                        return;
                    }
                    // A new build folder is ignored from now on, and a switched branch's refs
                    // are watched.
                    if let Some((ignored, status_files)) = reread() {
                        filter.set_ignored(&ignored);
                        let status_files: Vec<PathBuf> =
                            status_files.iter().map(|file| resolved(file)).collect();
                        if status_files != filter.status_files {
                            if let Ok(mut watcher) = thread_watcher.lock()
                                && let Some(watcher) = watcher.as_mut()
                            {
                                watch_status_dirs(watcher, &filter.root, &status_files);
                            }
                            filter.status_files = status_files;
                        }
                    }
                }
            });
        if let Err(err) = spawned {
            tracing::warn!(%err, "starting a checkout's watch failed");
        }
        Self {
            changes,
            stop,
            _watcher: watcher,
        }
    }
}

fn watcher(
    root: &Path,
    status_files: &[PathBuf],
    events: std_mpsc::Sender<notify::Result<notify::Event>>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher as _;

    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = events.send(event);
    })?;
    watcher.watch(root, notify::RecursiveMode::Recursive)?;
    watch_status_dirs(&mut watcher, root, status_files);
    Ok(watcher)
}

/// A linked worktree's index, HEAD and refs live outside it. Their folders are watched, not
/// the files: git replaces them by renaming a new one over them. A branch's folder that isn't
/// there yet (its ref only packed) is heard from the nearest folder above it that is. A folder
/// that can't be watched leaves its files to the next read.
fn watch_status_dirs(
    watcher: &mut notify::RecommendedWatcher,
    root: &Path,
    status_files: &[PathBuf],
) {
    use notify::Watcher as _;

    for dir in status_files.iter().filter_map(|file| file.parent()) {
        if dir.starts_with(root) {
            continue;
        }
        let Some(existing) = dir.ancestors().find(|dir| dir.is_dir()) else {
            continue;
        };
        let mode = if existing == dir {
            notify::RecursiveMode::NonRecursive
        } else {
            notify::RecursiveMode::Recursive
        };
        if let Err(err) = watcher.watch(existing, mode) {
            tracing::warn!(%err, dir = %existing.display(), "watching a checkout's git folder failed");
        }
    }
}

/// A path as events name it, with links resolved (`/private/tmp`, not `/tmp`), also when its
/// last parts aren't there yet.
fn resolved(path: &Path) -> PathBuf {
    for existing in path.ancestors() {
        if let (Ok(base), Ok(rest)) = (existing.canonicalize(), path.strip_prefix(existing)) {
            return if rest.as_os_str().is_empty() {
                base
            } else {
                base.join(rest)
            };
        }
    }
    path.to_path_buf()
}

/// Which reported paths change what the sidebar shows.
struct ChangeFilter {
    root: PathBuf,
    status_files: Vec<PathBuf>,
    ignored: Vec<PathBuf>,
}

impl ChangeFilter {
    fn new(root: PathBuf, status_files: Vec<PathBuf>, ignored: &[String]) -> Self {
        let mut filter = Self {
            root,
            status_files,
            ignored: Vec::new(),
        };
        filter.set_ignored(ignored);
        filter
    }

    /// Only ignored folders are kept: an ignored file still shows in the tree, so its changes
    /// count.
    fn set_ignored(&mut self, ignored: &[String]) {
        self.ignored = ignored
            .iter()
            .filter_map(|entry| entry.strip_suffix('/'))
            .map(PathBuf::from)
            .collect();
    }

    fn counts(&self, event: &notify::Result<notify::Event>) -> bool {
        match event {
            Ok(event) if matches!(event.kind, notify::EventKind::Access(_)) => false,
            Ok(event) if !event.need_rescan() => {
                event.paths.iter().any(|path| self.counts_path(path))
            }
            // Missed or unreadable events: read again to be safe.
            _ => true,
        }
    }

    fn counts_path(&self, path: &Path) -> bool {
        if self.status_files.iter().any(|file| file == path) {
            return true;
        }
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return false;
        };
        let in_git = relative
            .components()
            .any(|part| part == Component::Normal(".git".as_ref()));
        !in_git
            && !self
                .ignored
                .iter()
                .any(|folder| relative.starts_with(folder))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(paths: &[&str]) -> notify::Result<notify::Event> {
        let mut event = notify::Event::new(notify::EventKind::Any);
        for path in paths {
            event = event.add_path(PathBuf::from(path));
        }
        Ok(event)
    }

    #[test]
    fn changes_count_unless_only_in_ignored_folders_or_git_internals() {
        let filter = ChangeFilter::new(
            PathBuf::from("/repo"),
            vec![
                PathBuf::from("/repo/.git/index"),
                PathBuf::from("/common/worktrees/w/HEAD"),
            ],
            &[
                "target/".into(),
                "notes.log".into(),
                "web/node_modules/".into(),
            ],
        );
        for (paths, counts) in [
            (vec!["/repo/src/main.rs"], true),
            (vec!["/repo/notes.log"], true),
            (vec!["/repo/.git/index"], true),
            (vec!["/common/worktrees/w/HEAD"], true),
            (vec!["/repo/target/debug/build"], false),
            (vec!["/repo/web/node_modules/x/index.js"], false),
            (vec!["/repo/.git/objects/ab/cdef"], false),
            (vec!["/repo/.git/index.lock"], false),
            (vec!["/elsewhere/file"], false),
            (vec!["/repo/target/x", "/repo/README.md"], true),
        ] {
            assert_eq!(filter.counts(&event(&paths)), counts, "{paths:?}");
        }
        let mut rescan = notify::Event::new(notify::EventKind::Other);
        rescan = rescan.set_flag(notify::event::Flag::Rescan);
        assert!(filter.counts(&Ok(rescan)));
        let access = notify::Event::new(notify::EventKind::Access(notify::event::AccessKind::Any))
            .add_path(PathBuf::from("/repo/README.md"));
        assert!(
            !filter.counts(&Ok(access)),
            "reading the tree must not trigger another refresh"
        );
    }

    #[tokio::test]
    async fn failed_watch_polls_and_drop_stops_its_thread() {
        let path = std::env::temp_dir().join(format!(
            "shell-tabs-sidebar-watch-missing-{}",
            uuid::Uuid::new_v4()
        ));
        let mut watch = WorkspaceWatch::start(path, Vec::new(), Vec::new(), || None);
        let stop = watch.stop.clone();
        assert!(watch._watcher.lock().unwrap().is_none());
        assert_eq!(
            tokio::time::timeout(POLL + Duration::from_secs(2), watch.changed())
                .await
                .unwrap(),
            Some(())
        );
        drop(watch);
        assert!(stop.load(Ordering::SeqCst));
        // The thread's own stop reference goes away within its bounded receive/sleep.
        tokio::time::sleep(SETTLE * 3).await;
        assert_eq!(Arc::strong_count(&stop), 1);
    }
}
