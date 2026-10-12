//! Pages: the user's own rich-text documents, one Markdown file each at
//! `<data dir>/pages/<id>.md`, with the title and emoji in its front matter.
//!
//! The daemon is the files' only writer. A page's revision is a hash of its file's bytes, so an
//! edit made by another program reads as a new revision, and a save based on an older one is
//! refused with what the file holds now. Saves to one page happen one at a time and replace the
//! file whole (a temporary file beside it, flushed, renamed over it): a failed save leaves the
//! old file as it was. The folder is watched, so a change made by anyone reaches every open
//! editor as a [`PageChange`].

mod front_matter;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use ts_rs::TS;

use front_matter::PageFile;

/// The pages folder, in the data folder.
pub const PAGES_DIR: &str = "pages";
const EXTENSION: &str = "md";
/// A page id: a UUID's 32 lowercase hex digits, and nothing else.
const ID_LEN: usize = 32;
/// Changes buffered for a slow listener before it misses some.
const CHANGES: usize = 256;
/// How long the watcher gathers a burst of file events before reading the pages they touch.
const SETTLE: Duration = Duration::from_millis(100);

/// Why a page request failed.
#[derive(Debug, thiserror::Error)]
pub enum PageError {
    #[error("page {0} not found")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("could not {action} page {id}: {source}")]
    Io {
        action: &'static str,
        id: String,
        source: io::Error,
    },
}

/// What the user edits: the front matter's title and emoji, and the Markdown under them.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PageContent {
    /// Empty until the user names the page.
    pub title: String,
    pub emoji: Option<String>,
    /// Markdown.
    pub body: String,
}

/// A page as it is on disk now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub id: String,
    /// What the next `pageWrite` is based on.
    pub revision: String,
    pub content: PageContent,
    /// When it was made (ms since the Unix epoch); none for a file without that key.
    pub created_ms: Option<i64>,
    /// When its file last changed (ms since the Unix epoch).
    pub updated_ms: i64,
}

/// A page in the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PageSummary {
    pub id: String,
    pub title: String,
    pub emoji: Option<String>,
    /// When its file last changed (ms since the Unix epoch).
    pub updated_ms: i64,
}

/// What a `pageWrite` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PageWriteOutcome {
    /// Saved: the page's new revision.
    Saved { revision: String },
    /// Not saved: the file changed since the revision the write was based on. This is the page
    /// as it is now.
    Conflict { current: Page },
}

/// A page changed on disk (saved here, edited by another program, or deleted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PageChange {
    pub id: String,
    /// The page's revision now; none when it was deleted.
    pub revision: Option<String>,
}

/// The pages folder. Its methods do blocking file work: call them off the async runtime.
pub struct Pages {
    inner: Arc<Inner>,
    watch: Mutex<Option<Watch>>,
}

struct Inner {
    dir: PathBuf,
    /// One lock per page: its reads, saves and deletes happen one at a time.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// The revision last seen of each page, so a change is told once.
    known: Mutex<HashMap<String, String>>,
    changes: broadcast::Sender<PageChange>,
    /// Tests: the next save fails after writing its temporary file.
    #[cfg(test)]
    fail_next_save: AtomicBool,
}

impl Pages {
    /// The pages in `dir` (made if missing), watched for changes made by other programs. A
    /// folder that can't be made or watched is logged; reads still find outside edits.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        let pages = Self::unwatched(dir.into());
        match watch(pages.inner.clone()) {
            Ok(watch) => *lock(&pages.watch) = Some(watch),
            Err(error) => {
                tracing::warn!(%error, dir = %pages.inner.dir.display(), "pages are not watched");
            }
        }
        pages
    }

    fn unwatched(dir: PathBuf) -> Self {
        if let Err(error) = fs::create_dir_all(&dir) {
            tracing::warn!(%error, dir = %dir.display(), "could not make the pages folder");
        }
        // The watcher reports resolved paths (on macOS /var is /private/var).
        let dir = dir.canonicalize().unwrap_or(dir);
        Self {
            inner: Arc::new(Inner {
                dir,
                locks: Mutex::new(HashMap::new()),
                known: Mutex::new(HashMap::new()),
                changes: broadcast::channel(CHANGES).0,
                #[cfg(test)]
                fail_next_save: AtomicBool::new(false),
            }),
            watch: Mutex::new(None),
        }
    }

    /// Hears every page change from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<PageChange> {
        self.inner.changes.subscribe()
    }

    /// Makes a new, empty page.
    pub fn create(&self) -> Result<Page, PageError> {
        let inner = &self.inner;
        let id = uuid::Uuid::now_v7().simple().to_string();
        fs::create_dir_all(&inner.dir).map_err(io_error("make", &id))?;
        let page_lock = inner.page_lock(&id);
        let _held = lock(&page_lock);
        let file = PageFile {
            created_ms: Some(crate::now_ms()),
            ..PageFile::default()
        };
        let bytes = front_matter::render(&file).into_bytes();
        inner.save(&id, &bytes).map_err(io_error("save", &id))?;
        let page = inner.page(&id, &bytes);
        inner.observe(&id, Some(&page.revision), true);
        Ok(page)
    }

    /// The page as it is on disk now.
    pub fn read(&self, id: &str) -> Result<Page, PageError> {
        check_id(id)?;
        let inner = &self.inner;
        let page_lock = inner.page_lock(id);
        let _held = lock(&page_lock);
        let bytes = inner.read_bytes(id)?;
        let page = inner.page(id, &bytes);
        // An outside edit the watcher hasn't told yet is told now.
        inner.observe(id, Some(&page.revision), false);
        Ok(page)
    }

    /// Saves `content` over the page if it is still at `base_revision`; else nothing is
    /// written and the page as it is now comes back. Front matter keys other than the title
    /// and emoji are kept.
    pub fn write(
        &self,
        id: &str,
        content: &PageContent,
        base_revision: &str,
    ) -> Result<PageWriteOutcome, PageError> {
        check_id(id)?;
        let inner = &self.inner;
        let page_lock = inner.page_lock(id);
        let _held = lock(&page_lock);
        let current = inner.read_bytes(id)?;
        let current_revision = revision(&current);
        if current_revision != base_revision {
            let page = inner.page(id, &current);
            inner.observe(id, Some(&page.revision), false);
            return Ok(PageWriteOutcome::Conflict { current: page });
        }
        let mut file = front_matter::parse(&String::from_utf8_lossy(&current));
        file.title.clone_from(&content.title);
        file.emoji = content.emoji.clone().filter(|emoji| !emoji.is_empty());
        file.body.clone_from(&content.body);
        let bytes = front_matter::render(&file).into_bytes();
        if bytes == current {
            return Ok(PageWriteOutcome::Saved {
                revision: current_revision,
            });
        }
        inner.save(id, &bytes).map_err(io_error("save", id))?;
        let revision = revision(&bytes);
        inner.observe(id, Some(&revision), true);
        Ok(PageWriteOutcome::Saved { revision })
    }

    /// Every page, the most recently changed first.
    pub fn list(&self) -> Result<Vec<PageSummary>, PageError> {
        let inner = &self.inner;
        let mut pages = Vec::new();
        for id in inner.ids_on_disk().map_err(io_error("list", "folder"))? {
            let page_lock = inner.page_lock(&id);
            let _held = lock(&page_lock);
            // Gone since the folder was read.
            let Ok(bytes) = inner.read_bytes(&id) else {
                continue;
            };
            let page = inner.page(&id, &bytes);
            inner.observe(&id, Some(&page.revision), false);
            pages.push(PageSummary {
                id: page.id,
                title: page.content.title,
                emoji: page.content.emoji,
                updated_ms: page.updated_ms,
            });
        }
        pages.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| b.id.cmp(&a.id))
        });
        Ok(pages)
    }

    /// Deletes the page; one already gone is fine.
    pub fn delete(&self, id: &str) -> Result<(), PageError> {
        check_id(id)?;
        let inner = &self.inner;
        let page_lock = inner.page_lock(id);
        let _held = lock(&page_lock);
        match fs::remove_file(inner.path(id)) {
            Ok(()) => inner.sync_dir(),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error("delete", id)(error)),
        }
        lock(&inner.known).remove(id);
        let _ = inner.changes.send(PageChange {
            id: id.to_owned(),
            revision: None,
        });
        Ok(())
    }

    /// Reads every page again and tells what changed: what the watcher does after a burst it
    /// can't follow.
    pub fn rescan(&self) {
        self.inner.rescan_all();
    }

    #[cfg(test)]
    fn fail_next_save(&self) {
        self.inner.fail_next_save.store(true, Ordering::SeqCst);
    }
}

impl Drop for Pages {
    fn drop(&mut self) {
        // Stops the watcher before its thread is joined.
        drop(lock(&self.watch).take());
    }
}

impl Inner {
    fn path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.{EXTENSION}"))
    }

    fn page_lock(&self, id: &str) -> Arc<Mutex<()>> {
        lock(&self.locks).entry(id.to_owned()).or_default().clone()
    }

    fn read_bytes(&self, id: &str) -> Result<Vec<u8>, PageError> {
        fs::read(self.path(id)).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                PageError::NotFound(id.to_owned())
            } else {
                io_error("read", id)(error)
            }
        })
    }

    fn page(&self, id: &str, bytes: &[u8]) -> Page {
        let updated_ms = fs::metadata(self.path(id))
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |since| {
                i64::try_from(since.as_millis()).unwrap_or(i64::MAX)
            });
        let file = front_matter::parse(&String::from_utf8_lossy(bytes));
        Page {
            id: id.to_owned(),
            revision: revision(bytes),
            content: PageContent {
                title: file.title,
                emoji: file.emoji,
                body: file.body,
            },
            created_ms: file.created_ms,
            updated_ms,
        }
    }

    /// Replaces the page's file whole: a temporary file beside it, flushed to disk, renamed
    /// over it, then the folder flushed. On failure the old file is untouched and the temporary
    /// one removed.
    fn save(&self, id: &str, bytes: &[u8]) -> io::Result<()> {
        let target = self.path(id);
        let temp = self
            .dir
            .join(format!(".{id}.{}.tmp", uuid::Uuid::new_v4().simple()));
        let written = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            #[cfg(test)]
            if self.fail_next_save.swap(false, Ordering::SeqCst) {
                return Err(io::Error::other("injected failure"));
            }
            fs::rename(&temp, &target)
        })();
        if let Err(error) = written {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        self.sync_dir();
        Ok(())
    }

    /// Makes a rename or removal in the folder durable. The new file is already in place, so a
    /// failure here is only logged.
    fn sync_dir(&self) {
        #[cfg(unix)]
        if let Err(error) = fs::File::open(&self.dir).and_then(|dir| dir.sync_all()) {
            tracing::warn!(%error, "could not flush the pages folder");
        }
    }

    /// The ids of the page files in the folder.
    fn ids_on_disk(&self) -> io::Result<Vec<String>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        Ok(entries
            .filter_map(|entry| page_id_of(&entry.ok()?.path()))
            .collect())
    }

    /// Notes the page's revision now (none: gone) and tells listeners when it changed. A page
    /// not seen before is told only with `announce_new`: a new file in the folder, not one
    /// merely read for the first time since the daemon started.
    fn observe(&self, id: &str, revision: Option<&str>, announce_new: bool) {
        let mut known = lock(&self.known);
        let changed = match (known.get(id), revision) {
            (Some(before), Some(now)) => before != now,
            (Some(_), None) => true,
            (None, Some(_)) => announce_new,
            (None, None) => false,
        };
        match revision {
            Some(now) => {
                known.insert(id.to_owned(), now.to_owned());
            }
            None => {
                known.remove(id);
            }
        }
        drop(known);
        if changed {
            let _ = self.changes.send(PageChange {
                id: id.to_owned(),
                revision: revision.map(str::to_owned),
            });
        }
    }

    /// Reads one page's file again and tells a change.
    fn refresh(&self, id: &str) {
        let page_lock = self.page_lock(id);
        let _held = lock(&page_lock);
        match fs::read(self.path(id)) {
            Ok(bytes) => self.observe(id, Some(&revision(&bytes)), true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => self.observe(id, None, true),
            Err(error) => tracing::debug!(%error, id, "could not read a changed page"),
        }
    }

    fn rescan_all(&self) {
        let mut ids: HashSet<String> = lock(&self.known).keys().cloned().collect();
        match self.ids_on_disk() {
            Ok(on_disk) => ids.extend(on_disk),
            Err(error) => tracing::debug!(%error, "could not list the pages folder"),
        }
        for id in ids {
            self.refresh(&id);
        }
    }
}

/// A page's revision: a hash of its file's bytes.
fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex()[..32].to_owned()
}

/// Refuses anything but a page id, so no request reaches a file outside the pages folder.
fn check_id(id: &str) -> Result<(), PageError> {
    if is_page_id(id) {
        Ok(())
    } else {
        Err(PageError::Invalid(format!("{id:?} is not a page id")))
    }
}

fn is_page_id(id: &str) -> bool {
    id.len() == ID_LEN
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The page id a file in the folder is for: `<id>.md`, nothing else (temporary files included).
fn page_id_of(path: &Path) -> Option<String> {
    if path.extension()? != EXTENSION {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    is_page_id(stem).then(|| stem.to_owned())
}

fn io_error(action: &'static str, id: &str) -> impl FnOnce(io::Error) -> PageError {
    let id = id.to_owned();
    move |source| PageError::Io { action, id, source }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The folder's watcher and the thread reading what it reports; dropping it stops both.
struct Watch {
    watcher: Option<notify::RecommendedWatcher>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.watcher.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn watch(inner: Arc<Inner>) -> notify::Result<Watch> {
    use notify::Watcher as _;

    let (tx, rx) = mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = tx.send(event);
    })?;
    watcher.watch(&inner.dir, notify::RecursiveMode::NonRecursive)?;
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = stop.clone();
    let thread = thread::Builder::new()
        .name("pages-watcher".into())
        .spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let first = match rx.recv_timeout(Duration::from_millis(200)) {
                    Ok(event) => event,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                // A save is several events (the temporary file, the rename): read once after.
                let mut events = vec![first];
                let settled = std::time::Instant::now() + SETTLE;
                while let Some(left) = settled.checked_duration_since(std::time::Instant::now()) {
                    match rx.recv_timeout(left) {
                        Ok(event) => events.push(event),
                        Err(_) => break,
                    }
                }
                let mut ids = HashSet::new();
                let mut everything = false;
                for event in events {
                    match event {
                        Ok(event) if !event.need_rescan() => {
                            ids.extend(event.paths.iter().filter_map(|path| page_id_of(path)));
                        }
                        _ => everything = true,
                    }
                }
                if everything {
                    inner.rescan_all();
                } else {
                    for id in ids {
                        inner.refresh(&id);
                    }
                }
            }
        })
        .map_err(|error| notify::Error::generic(&error.to_string()))?;
    Ok(Watch {
        watcher: Some(watcher),
        stop,
        thread: Some(thread),
    })
}
