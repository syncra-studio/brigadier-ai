//! Brigadier's event store.
//!
//! - **One writer.** A dedicated OS thread owns the only read-write SQLite connection. Callers
//!   hand it work over a bounded Tokio channel (`send().await` on the async side,
//!   `blocking_recv` on the thread), so a slow disk applies backpressure instead of growing a
//!   queue or blocking the runtime. Everything queued when the writer wakes up is committed in
//!   one transaction, up to [`MAX_BATCH`] commands.
//! - **A read pool.** A fixed set of OS threads with read-only connections. Every read
//!   materializes a bounded page and finishes its statement before the result leaves the
//!   thread, so no WAL snapshot is ever held while a client is being served.
//! - **Append-only streams.** Events are appended per stream with a per-stream sequence and a
//!   global, monotonically increasing `seq` that subscribers use as a resync cursor. The only
//!   removals are retention trims and [`Store::delete_streams`] (a permanent Delete); neither
//!   renumbers anything, and a `seq` is never reused.
//! - **Blobs.** Large payloads live in a content-addressed store on disk ([`BlobStore`]). The
//!   writer indexes every blob hash an event payload mentions, and [`Store::gc_blobs`] deletes
//!   the blobs no remaining event mentions (the rule is on [`BlobStore`]);
//!   [`Store::delete_streams_and_blobs`] takes a permanent Delete's own blobs at once.
//!
//! Nothing here ever blocks a Tokio worker thread.

mod blob;
mod reader;
mod refs;
mod schema;
mod writer;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::value::RawValue;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

pub use blob::{BlobHash, BlobStore};
use reader::ReadPool;
use writer::{WriteCommand, WriteOp};

/// Most commands the writer folds into one transaction.
pub const MAX_BATCH: usize = 256;
/// Capacity of the async → writer queue; senders wait when it is full.
pub const WRITE_QUEUE: usize = 1024;
/// Largest page any read returns.
pub const MAX_PAGE: u32 = 1000;
/// Capacity of the live event feed; slower subscribers are cut off and must resync.
pub const FEED_CAPACITY: usize = 4096;
/// How long a blob that no event references is kept after it was last put or touched. It
/// covers the gap between storing a blob and appending the event that references it, which
/// for a composer attachment is as long as the user takes to send the message.
pub const BLOB_GC_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
/// Blobs the writer checks and deletes per command, so appends interleave with a large GC.
const GC_CHUNK: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration: {0}")]
    Migration(#[from] rusqlite_migration::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("the store is shutting down")]
    ShuttingDown,
    #[error("the store writer has stopped")]
    WriterGone,
    #[error("the store read pool has stopped")]
    ReaderGone,
    #[error("invalid blob hash")]
    InvalidBlobHash,
    #[error("invalid request: {0}")]
    Invalid(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;

/// An event to append.
#[derive(Debug, Clone)]
pub struct NewEvent {
    pub stream: String,
    pub kind: String,
    /// Wall-clock time the daemon ingested the event, in ms since the Unix epoch.
    pub at_ms: i64,
    /// JSON payload.
    pub payload: Box<RawValue>,
}

impl NewEvent {
    pub fn new(
        stream: impl Into<String>,
        kind: impl Into<String>,
        at_ms: i64,
        payload: &impl Serialize,
    ) -> Result<Self> {
        Ok(Self {
            stream: stream.into(),
            kind: kind.into(),
            at_ms,
            payload: serde_json::value::to_raw_value(payload)?,
        })
    }
}

/// A committed event.
#[derive(Debug, Clone)]
pub struct StoredEvent {
    /// Global sequence number, unique and increasing across all streams.
    pub seq: i64,
    pub stream: String,
    /// Position within the stream, starting at 1.
    pub stream_seq: i64,
    pub kind: String,
    pub at_ms: i64,
    pub payload: Box<RawValue>,
}

/// Keeps only the newest `keep_last` events of the appended streams (used for diagnostics).
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    pub keep_last: u32,
}

/// Direction and bounds for a stream page.
#[derive(Debug, Clone, Default)]
pub struct StreamPage {
    /// Only events with `stream_seq` below this (paging backwards from the newest).
    pub before: Option<i64>,
    /// Only events of these kinds (empty = all).
    pub kinds: Vec<String>,
    pub limit: u32,
}

/// The newest event of a stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamHead {
    pub stream: String,
    pub stream_seq: i64,
    pub at_ms: i64,
}

/// Store tuning.
#[derive(Debug, Clone)]
pub struct StoreConfig {
    pub db_path: PathBuf,
    pub blobs_dir: PathBuf,
    pub readers: usize,
}

/// Live counters for the Inspector.
#[derive(Debug, Clone, Copy, Default)]
pub struct StoreStats {
    pub last_seq: i64,
    pub queued_writes: usize,
    pub committed_batches: u64,
    pub committed_events: u64,
    pub last_batch_commands: u64,
    pub last_commit_us: u64,
    pub wal_bytes: u64,
    pub checkpoints: u64,
}

/// The database's size and the space compacting it would give back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbSpace {
    pub file_bytes: u64,
    /// Free pages inside the file.
    pub free_bytes: u64,
    pub wal_bytes: u64,
}

/// What a [`Store::gc_blobs`] run found and did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GcStats {
    /// Blob files found.
    pub blobs: u64,
    /// Kept because an event references them.
    pub referenced: u64,
    /// Kept because they were put or touched within the grace period.
    pub recent: u64,
    /// Deleted.
    pub removed: u64,
    pub removed_bytes: u64,
    /// Unreferenced and stale, but deleting failed (logged); retried by the next run.
    pub failed: u64,
}

/// How the writer thread ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriterState {
    Running,
    /// Stopped after an orderly [`Store::shutdown`].
    Stopped,
    /// Stopped for any other reason (panic, fatal SQLite error). The daemon must exit.
    Died(String),
}

/// Coordinates a compaction with the writes around it. Every ordering is `SeqCst`: a write
/// admitted while an automatic compaction arms either finds it armed (and makes it give way)
/// or is counted before the compaction looks (and it doesn't start).
#[derive(Debug, Default)]
pub(crate) struct Maintenance {
    /// Shutdown has begun: no compaction starts, and one running stops.
    stopped: AtomicBool,
    /// An automatic compaction is about to start or is running.
    armed: AtomicBool,
    /// A write arrived while it was armed: it gives way. Kept until the next one arms, so a
    /// write just before `VACUUM` starts still stops it.
    yielded: AtomicBool,
    /// Writes admitted and not answered yet (queued, or in the writer's batch).
    in_flight: AtomicUsize,
    /// Writes ever admitted: activity, for the daemon's quiet period.
    admitted: AtomicU64,
}

impl Maintenance {
    /// Whether the compaction running now should stop (`automatic`: also when a write came).
    pub(crate) fn should_stop(&self, automatic: bool) -> bool {
        self.stopped.load(Ordering::SeqCst) || (automatic && self.yielded.load(Ordering::SeqCst))
    }

    fn admit(&self) -> InFlight<'_> {
        self.admitted.fetch_add(1, Ordering::SeqCst);
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        if self.armed.load(Ordering::SeqCst) {
            self.yielded.store(true, Ordering::SeqCst);
        }
        InFlight(self)
    }
}

/// A write admitted and not answered yet.
struct InFlight<'a>(&'a Maintenance);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An automatic compaction, armed: from now on any write makes it give way. Disarmed when
/// dropped. See [`Store::arm_compaction`].
pub struct ArmedCompaction(Store);

impl ArmedCompaction {
    /// Runs it, unless a write is under way or came since it was armed. A write that comes
    /// while it runs waits only until it has stopped, except at the end of the `VACUUM`, its
    /// copy back into the file, which can't be stopped: about 2.5 ms per MB of data that
    /// stays (measured), which callers bound.
    pub async fn run(self) -> Result<Compacted> {
        if self.0.maintenance.in_flight.load(Ordering::SeqCst) > 0 {
            return Ok(Compacted::GaveWay);
        }
        self.0
            .command(|reply| WriteOp::Compact {
                automatic: true,
                reply,
            })
            .await?
    }
}

impl Drop for ArmedCompaction {
    fn drop(&mut self) {
        self.0.maintenance.armed.store(false, Ordering::SeqCst);
    }
}

/// How an [`ArmedCompaction`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compacted {
    Done,
    /// A write came, or one was under way: it did not start, or stopped with nothing changed.
    GaveWay,
}

#[derive(Debug, Default)]
struct Counters {
    last_seq: AtomicI64,
    committed_batches: AtomicU64,
    committed_events: AtomicU64,
    last_batch_commands: AtomicU64,
    last_commit_us: AtomicU64,
    wal_bytes: AtomicU64,
    checkpoints: AtomicU64,
}

/// Handle to the event store. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    writes: mpsc::Sender<WriteCommand>,
    reads: ReadPool,
    feed: broadcast::Sender<Arc<StoredEvent>>,
    admitting: Arc<AtomicBool>,
    maintenance: Arc<Maintenance>,
    counters: Arc<Counters>,
    writer_state: watch::Receiver<WriterState>,
    blobs: BlobStore,
}

impl Store {
    /// Opens (creating and migrating if needed) the store and starts its threads.
    ///
    /// This does blocking file I/O. Call it before starting the async runtime or from
    /// `spawn_blocking`.
    pub fn open(config: StoreConfig) -> Result<Self> {
        if let Some(parent) = config.db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let blobs = BlobStore::open(config.blobs_dir.clone())?;
        let counters = Arc::new(Counters::default());
        let maintenance = Arc::new(Maintenance::default());
        let (feed, _) = broadcast::channel(FEED_CAPACITY);
        let (writes, writer_rx) = mpsc::channel(WRITE_QUEUE);
        let (state_tx, writer_state) = watch::channel(WriterState::Running);

        // The writer migrates the schema before any reader opens a connection.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        writer::spawn(
            config.db_path.clone(),
            blobs.clone(),
            writer_rx,
            feed.clone(),
            counters.clone(),
            maintenance.clone(),
            state_tx,
            ready_tx,
        )?;
        ready_rx.recv().map_err(|_| Error::WriterGone)??;

        let reads = ReadPool::spawn(&config.db_path, config.readers.max(1))?;
        Ok(Self {
            writes,
            reads,
            feed,
            admitting: Arc::new(AtomicBool::new(true)),
            maintenance,
            counters,
            writer_state,
            blobs,
        })
    }

    /// Appends events atomically, in order. Returns them with their assigned sequence numbers
    /// once committed.
    pub async fn append(&self, events: Vec<NewEvent>) -> Result<Vec<Arc<StoredEvent>>> {
        self.append_with(events, None).await
    }

    /// Like [`Store::append`], then trims each touched stream to its newest events.
    pub async fn append_with(
        &self,
        events: Vec<NewEvent>,
        retention: Option<Retention>,
    ) -> Result<Vec<Arc<StoredEvent>>> {
        if !self.admitting.load(Ordering::Acquire) {
            return Err(Error::ShuttingDown);
        }
        self.write(|reply| WriteOp::Append {
            events,
            retention,
            reply,
        })
        .await?
    }

    /// Permanently removes every event of these exact streams, atomically and in order with
    /// the appends around it (through the single writer). Returns how many events were
    /// removed. Other streams and all sequence numbers are untouched; subscribers see nothing
    /// (the feed only carries appends). Blobs the events referenced stay until
    /// [`Store::gc_blobs`]; [`Store::delete_streams_and_blobs`] takes them too.
    pub async fn delete_streams(&self, streams: Vec<String>) -> Result<u64> {
        self.delete(streams, Vec::new()).await
    }

    /// [`Store::delete_streams`], then deletes right away each blob those events referenced that
    /// no remaining event references, unless it was put or touched after the millisecond of its
    /// last reference there (an event is stamped when it is appended, after the blobs it
    /// references were put): content that only came from them, which
    /// [`Store::gc_blobs`] would otherwise keep for [`BLOB_GC_GRACE`]. A blob something else
    /// put again since, such as an identical attachment waiting in a composer, is kept.
    pub async fn delete_streams_and_blobs(&self, streams: Vec<String>) -> Result<(u64, GcStats)> {
        let own = self
            .reads
            .run({
                let streams = streams.clone();
                move |conn| reader::blob_refs_of(conn, &streams)
            })
            .await?;
        let removed = self.delete_streams(streams).await?;
        let hashes: Vec<_> = own
            .into_iter()
            .filter_map(|(hash, at_ms)| {
                // The end of that millisecond: a file time is finer than an event's.
                let last = SystemTime::UNIX_EPOCH
                    + Duration::from_millis(u64::try_from(at_ms).unwrap_or_default() + 1)
                    - Duration::from_nanos(1);
                Some((hash.parse().ok()?, last))
            })
            .collect();
        let found = GcStats {
            blobs: hashes.len() as u64,
            ..GcStats::default()
        };
        let stats = self.collect_blobs(hashes, found).await?;
        Ok((removed, stats))
    }

    /// Like [`Store::delete_streams`], for every stream whose name starts with one of
    /// `prefixes` (e.g. `"task:"` plus an id prefix). An empty prefix is refused.
    pub async fn delete_stream_prefixes(&self, prefixes: Vec<String>) -> Result<u64> {
        if prefixes.iter().any(String::is_empty) {
            return Err(Error::Invalid("an empty prefix would delete every stream"));
        }
        self.delete(Vec::new(), prefixes).await
    }

    async fn delete(&self, streams: Vec<String>, prefixes: Vec<String>) -> Result<u64> {
        if !self.admitting.load(Ordering::Acquire) {
            return Err(Error::ShuttingDown);
        }
        if streams.is_empty() && prefixes.is_empty() {
            return Ok(0);
        }
        self.write(|reply| WriteOp::Delete {
            streams,
            prefixes,
            reply,
        })
        .await?
    }

    /// Deletes the blob files no stored event references, keeping any put or touched within
    /// [`BLOB_GC_GRACE`] (the rule is on [`BlobStore`]).
    ///
    /// Listing the blob directory runs on Tokio's blocking pool; the reference check and the
    /// deletes run on the writer thread in chunks, between batches, so no append can slip in
    /// between the check and the delete. Nothing runs on the async runtime.
    pub async fn gc_blobs(&self) -> Result<GcStats> {
        self.gc_blobs_with(BLOB_GC_GRACE).await
    }

    /// [`Store::gc_blobs`] with a different grace period. Shorter than [`BLOB_GC_GRACE`] is only
    /// safe when nothing can hold an unreferenced blob for longer (e.g. no composer is open).
    pub async fn gc_blobs_with(&self, grace: Duration) -> Result<GcStats> {
        if !self.admitting.load(Ordering::Acquire) {
            return Err(Error::ShuttingDown);
        }
        let cutoff = SystemTime::now()
            .checked_sub(grace)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let blobs = self.blobs.clone();
        let listed = tokio::task::spawn_blocking(move || blobs.list_blocking())
            .await
            .map_err(|err| Error::Io(std::io::Error::other(err)))??;

        let mut stats = GcStats {
            blobs: listed.len() as u64,
            ..GcStats::default()
        };
        let mut candidates = Vec::new();
        for entry in listed {
            if entry.modified > cutoff {
                stats.recent += 1;
            } else {
                candidates.push((entry.hash, cutoff));
            }
        }
        let stats = self.collect_blobs(candidates, stats).await?;
        tracing::info!(?stats, "blob gc finished");
        Ok(stats)
    }

    /// Has the writer delete each blob no event references and nothing put or touched after
    /// its cutoff, in chunks, adding to `stats`.
    async fn collect_blobs(
        &self,
        candidates: Vec<(BlobHash, SystemTime)>,
        mut stats: GcStats,
    ) -> Result<GcStats> {
        if !self.admitting.load(Ordering::Acquire) {
            return Err(Error::ShuttingDown);
        }
        for chunk in candidates.chunks(GC_CHUNK) {
            let hashes = chunk.to_vec();
            let collected = self
                .write(|reply| WriteOp::CollectBlobs { hashes, reply })
                .await??;
            stats.referenced += collected.referenced;
            stats.recent += collected.recent;
            stats.removed += collected.removed;
            stats.removed_bytes += collected.removed_bytes;
            stats.failed += collected.failed;
        }
        Ok(stats)
    }

    /// Every distinct blob the events of `streams` reference.
    pub async fn blob_hashes_of(&self, streams: Vec<String>) -> Result<Vec<BlobHash>> {
        let hashes = self
            .reads
            .run(move |conn| reader::blob_hashes_of(conn, &streams))
            .await?;
        Ok(hashes
            .into_iter()
            .filter_map(|hash| hash.parse().ok())
            .collect())
    }

    /// What [`Store::gc_blobs`] would remove now: the blobs no event references, outside their
    /// grace period, and the space they take.
    pub async fn collectable_blobs(&self) -> Result<(u64, u64)> {
        let cutoff = SystemTime::now()
            .checked_sub(BLOB_GC_GRACE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let blobs = self.blobs.clone();
        let listed = tokio::task::spawn_blocking(move || blobs.list_blocking())
            .await
            .map_err(|err| Error::Io(std::io::Error::other(err)))??;
        let old: Vec<String> = listed
            .into_iter()
            .filter(|entry| entry.modified <= cutoff)
            .map(|entry| entry.hash.as_str().to_owned())
            .collect();
        let free = self
            .reads
            .run(move |conn| reader::unreferenced(conn, &old))
            .await?;
        let count = free.len() as u64;
        let blobs = self.blobs.clone();
        let bytes = tokio::task::spawn_blocking(move || {
            free.iter()
                .filter_map(|hash| hash.parse::<BlobHash>().ok())
                .map(|hash| std::fs::metadata(blobs.file_of(&hash)).map_or(0, |meta| meta.len()))
                .sum::<u64>()
        })
        .await
        .map_err(|err| Error::Io(std::io::Error::other(err)))?;
        Ok((count, bytes))
    }

    /// The database file's free space (pages a `VACUUM` gives back) and the WAL's size.
    pub async fn free_space(&self) -> Result<DbSpace> {
        let (page_size, pages, free) = self.reads.run(reader::pages).await?;
        Ok(DbSpace {
            file_bytes: page_size * pages,
            free_bytes: page_size * free,
            wal_bytes: self.counters.wal_bytes.load(Ordering::Relaxed),
        })
    }

    /// Rebuilds the database without its free pages and truncates the WAL, through the writer
    /// (so between batches). Only when nothing is working: it holds up every write meanwhile.
    /// A shutdown stops it, with nothing changed.
    pub async fn compact(&self) -> Result<()> {
        if !self.admitting.load(Ordering::Acquire) || self.maintenance.should_stop(false) {
            return Err(Error::ShuttingDown);
        }
        match self
            .command(|reply| WriteOp::Compact {
                automatic: false,
                reply,
            })
            .await??
        {
            Compacted::Done => Ok(()),
            Compacted::GaveWay => Err(Error::ShuttingDown),
        }
    }

    /// Arms an automatic [`Store::compact`]: a write that comes from now until it has finished
    /// stops it, with nothing changed, and goes through. The caller checks once more that
    /// nothing works after arming, then runs it: work that starts in between writes, and so
    /// stops it.
    pub fn arm_compaction(&self) -> Result<ArmedCompaction> {
        let maintenance = &*self.maintenance;
        if !self.admitting.load(Ordering::Acquire) || maintenance.should_stop(false) {
            return Err(Error::ShuttingDown);
        }
        maintenance.yielded.store(false, Ordering::SeqCst);
        maintenance.armed.store(true, Ordering::SeqCst);
        Ok(ArmedCompaction(self.clone()))
    }

    /// Writes ever admitted: whether anything was written since the last look.
    pub fn writes_admitted(&self) -> u64 {
        self.maintenance.admitted.load(Ordering::SeqCst)
    }

    /// Shutdown has begun: no compaction starts any more, and one running stops (its writes
    /// still go through until [`Store::shutdown`]).
    pub fn stop_maintenance(&self) {
        self.maintenance.stopped.store(true, Ordering::SeqCst);
    }

    /// Asks the writer for a PASSIVE WAL checkpoint without waiting behind a full queue.
    pub fn request_checkpoint(&self) {
        let (reply, _) = oneshot::channel();
        let _ = self.writes.try_send(WriteCommand {
            op: WriteOp::Checkpoint { reply },
        });
    }

    /// Stops admitting writes, commits everything already queued, truncates the WAL and stops
    /// the writer thread.
    pub async fn shutdown(&self) -> Result<()> {
        self.admitting.store(false, Ordering::Release);
        self.stop_maintenance();
        self.command(|reply| WriteOp::Shutdown { reply }).await?
    }

    /// A stream page, newest first (optionally only events before `page.before`).
    pub async fn read_stream(&self, stream: String, page: StreamPage) -> Result<Vec<StoredEvent>> {
        self.reads
            .run(move |conn| reader::read_stream(conn, &stream, &page))
            .await
    }

    /// Every event with `seq > after`, oldest first, at most `limit`.
    pub async fn read_since(&self, after: i64, limit: u32) -> Result<Vec<StoredEvent>> {
        self.reads
            .run(move |conn| reader::read_since(conn, after, limit))
            .await
    }

    /// Every event of `stream` with `stream_seq > after`, oldest first, at most `limit`.
    pub async fn read_stream_since(
        &self,
        stream: String,
        after: i64,
        limit: u32,
    ) -> Result<Vec<StoredEvent>> {
        self.reads
            .run(move |conn| reader::read_stream_since(conn, &stream, after, limit))
            .await
    }

    /// The newest event of every stream whose name starts with `prefix`.
    pub async fn stream_heads(&self, prefix: String) -> Result<Vec<StreamHead>> {
        self.reads
            .run(move |conn| reader::stream_heads(conn, &prefix))
            .await
    }

    /// Subscribes to committed events. A receiver that falls more than [`FEED_CAPACITY`] events
    /// behind gets `Lagged` and must resync with [`Store::read_since`].
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<StoredEvent>> {
        self.feed.subscribe()
    }

    pub fn last_seq(&self) -> i64 {
        self.counters.last_seq.load(Ordering::Acquire)
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    pub fn stats(&self) -> StoreStats {
        let c = &self.counters;
        StoreStats {
            last_seq: c.last_seq.load(Ordering::Acquire),
            queued_writes: WRITE_QUEUE - self.writes.capacity(),
            committed_batches: c.committed_batches.load(Ordering::Relaxed),
            committed_events: c.committed_events.load(Ordering::Relaxed),
            last_batch_commands: c.last_batch_commands.load(Ordering::Relaxed),
            last_commit_us: c.last_commit_us.load(Ordering::Relaxed),
            wal_bytes: c.wal_bytes.load(Ordering::Relaxed),
            checkpoints: c.checkpoints.load(Ordering::Relaxed),
        }
    }

    /// Resolves when the writer thread stops, with the reason.
    pub async fn writer_stopped(&self) -> WriterState {
        let mut state = self.writer_state.clone();
        match state.wait_for(|state| *state != WriterState::Running).await {
            Ok(state) => state.clone(),
            Err(_) => WriterState::Died("writer state channel closed".into()),
        }
    }

    /// [`Store::command`] for a write: counted while it is under way, and it makes an
    /// automatic compaction give way.
    async fn write<T>(&self, op: impl FnOnce(oneshot::Sender<T>) -> WriteOp) -> Result<T> {
        let _in_flight = self.maintenance.admit();
        self.command(op).await
    }

    async fn command<T>(&self, op: impl FnOnce(oneshot::Sender<T>) -> WriteOp) -> Result<T> {
        let (reply, rx) = oneshot::channel();
        self.writes
            .send(WriteCommand { op: op(reply) })
            .await
            .map_err(|_| Error::WriterGone)?;
        rx.await.map_err(|_| Error::WriterGone)
    }
}

/// Interval for the periodic PASSIVE checkpoint the daemon requests.
pub const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as i64)
    }

    #[tokio::test]
    async fn deleting_streams_takes_only_the_blobs_that_came_from_them() {
        let dir = std::env::temp_dir().join(format!("brigadier-store-{}", uuid::Uuid::new_v4()));
        let store = Store::open(StoreConfig {
            db_path: dir.join("db.sqlite"),
            blobs_dir: dir.join("blobs"),
            readers: 1,
        })
        .expect("a store");
        let blobs = store.blobs();
        let own = blobs
            .put(b"only the deleted session".to_vec())
            .await
            .unwrap();
        let shared = blobs.put(b"in both sessions".to_vec()).await.unwrap();
        // Put again (say, attached in another composer) after the deleted session last used it.
        let put_since = blobs
            .put(b"attached again elsewhere".to_vec())
            .await
            .unwrap();
        let event = |stream: &str, at_ms: i64, hash: &BlobHash| {
            NewEvent::new(
                stream,
                "message",
                at_ms,
                &serde_json::json!({ "blob": hash.as_str() }),
            )
            .unwrap()
        };
        let (now, earlier) = (now_ms(), now_ms() - 10 * 60 * 1000);
        store
            .append(vec![
                event("conversation:a", now, &own),
                event("task:a1", now, &shared),
                event("conversation:a", earlier, &put_since),
                event("conversation:b", now, &shared),
            ])
            .await
            .unwrap();

        // Attached again elsewhere just after the deleted session last used it.
        let soon = blobs.put(b"attached again at once".to_vec()).await.unwrap();
        store
            .append(vec![event("conversation:a", now_ms(), &soon)])
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        blobs.put(b"attached again at once".to_vec()).await.unwrap();

        let (removed, stats) = store
            .delete_streams_and_blobs(vec!["conversation:a".into(), "task:a1".into()])
            .await
            .unwrap();

        assert_eq!(removed, 4);
        assert_eq!((stats.removed, stats.referenced, stats.recent), (1, 1, 2));
        assert!(blobs.get(own).await.unwrap().is_none());
        assert!(blobs.get(shared).await.unwrap().is_some());
        assert!(blobs.get(put_since).await.unwrap().is_some());
        assert!(blobs.get(soon).await.unwrap().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }
}
