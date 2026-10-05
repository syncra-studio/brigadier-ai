use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant, SystemTime};

use rusqlite::{Connection, OptionalExtension, params};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::blob::{BlobStore, Removal};
use crate::{
    BlobHash, Compacted, Counters, Error, MAX_BATCH, Maintenance, NewEvent, Result, Retention,
    StoredEvent, WriterState, schema,
};

/// SQLite virtual machine steps between two looks at whether a compaction should stop.
const STOP_CHECK_STEPS: i32 = 1000;
/// How long an automatic compaction's TRUNCATE checkpoint waits for a reader still on the
/// snapshot from before the `VACUUM`, holding up every write meanwhile.
const AUTOMATIC_CHECKPOINT_WAIT: Duration = Duration::from_millis(50);

pub(crate) struct WriteCommand {
    pub(crate) op: WriteOp,
}

pub(crate) enum WriteOp {
    Append {
        events: Vec<NewEvent>,
        retention: Option<Retention>,
        reply: AppendReply,
    },
    Delete {
        streams: Vec<String>,
        prefixes: Vec<String>,
        reply: oneshot::Sender<Result<u64>>,
    },
    /// Each blob with its own cutoff.
    CollectBlobs {
        hashes: Vec<(BlobHash, SystemTime)>,
        reply: oneshot::Sender<Result<Collected>>,
    },
    Checkpoint {
        reply: oneshot::Sender<Result<()>>,
    },
    /// Rebuilds the database file without its free pages (`VACUUM`), then truncates the WAL.
    /// `automatic`: it gives way to any write that comes meanwhile.
    Compact {
        automatic: bool,
        reply: oneshot::Sender<Result<Compacted>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<()>>,
    },
}

type Feed = broadcast::Sender<Arc<StoredEvent>>;
type AppendReply = oneshot::Sender<Result<Vec<Arc<StoredEvent>>>>;

/// A command that changes the event log, committed in queue order within one transaction.
enum Mutation {
    Append {
        events: Vec<NewEvent>,
        retention: Option<Retention>,
        reply: AppendReply,
    },
    Delete {
        streams: Vec<String>,
        prefixes: Vec<String>,
        reply: oneshot::Sender<Result<u64>>,
    },
}

/// A blob collection request, run after the batch's transaction has committed.
struct Collection {
    hashes: Vec<(BlobHash, SystemTime)>,
    reply: oneshot::Sender<Result<Collected>>,
}

/// Outcome of one [`WriteOp::CollectBlobs`] chunk.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Collected {
    pub(crate) removed: u64,
    pub(crate) removed_bytes: u64,
    pub(crate) referenced: u64,
    pub(crate) recent: u64,
    pub(crate) failed: u64,
}

/// Sorts commands into mutations (in order), blob collections, checkpoints and a shutdown.
#[derive(Default)]
struct Sorted {
    mutations: Vec<Mutation>,
    collections: Vec<Collection>,
    checkpoints: Vec<oneshot::Sender<Result<()>>>,
    compactions: Vec<(bool, oneshot::Sender<Result<Compacted>>)>,
    shutdown: Option<oneshot::Sender<Result<()>>>,
}

impl Sorted {
    fn push(&mut self, op: WriteOp) {
        match op {
            WriteOp::Append {
                events,
                retention,
                reply,
            } => self.mutations.push(Mutation::Append {
                events,
                retention,
                reply,
            }),
            WriteOp::Delete {
                streams,
                prefixes,
                reply,
            } => self.mutations.push(Mutation::Delete {
                streams,
                prefixes,
                reply,
            }),
            WriteOp::CollectBlobs { hashes, reply } => {
                self.collections.push(Collection { hashes, reply })
            }
            WriteOp::Checkpoint { reply } => self.checkpoints.push(reply),
            WriteOp::Compact { automatic, reply } => self.compactions.push((automatic, reply)),
            WriteOp::Shutdown { reply } => match self.shutdown {
                // A second shutdown racing the first just succeeds.
                Some(_) => {
                    let _ = reply.send(Ok(()));
                }
                None => self.shutdown = Some(reply),
            },
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    db_path: PathBuf,
    blobs: BlobStore,
    rx: mpsc::Receiver<WriteCommand>,
    feed: Feed,
    counters: Arc<Counters>,
    maintenance: Arc<Maintenance>,
    state: watch::Sender<WriterState>,
    ready: std::sync::mpsc::Sender<Result<()>>,
) -> Result<()> {
    std::thread::Builder::new()
        .name("store-writer".into())
        .spawn(move || {
            let mut guard = StateGuard {
                state,
                outcome: None,
            };
            let mut writer = match Writer::open(&db_path, blobs, feed, counters, maintenance) {
                Ok(writer) => {
                    let _ = ready.send(Ok(()));
                    writer
                }
                Err(err) => {
                    guard.outcome = Some(WriterState::Died(err.to_string()));
                    let _ = ready.send(Err(err));
                    return;
                }
            };
            guard.outcome = Some(match writer.run(rx) {
                Ok(()) => WriterState::Stopped,
                Err(err) => {
                    tracing::error!(error = %err, "store writer failed");
                    WriterState::Died(err.to_string())
                }
            });
        })?;
    Ok(())
}

/// Publishes how the writer ended, including when it unwinds from a panic.
struct StateGuard {
    state: watch::Sender<WriterState>,
    outcome: Option<WriterState>,
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or_else(|| {
            WriterState::Died(if std::thread::panicking() {
                "store writer panicked".into()
            } else {
                "store writer exited unexpectedly".into()
            })
        });
        self.state.send_replace(outcome);
    }
}

struct Writer {
    conn: Connection,
    blobs: BlobStore,
    wal_path: PathBuf,
    feed: Feed,
    counters: Arc<Counters>,
    maintenance: Arc<Maintenance>,
    /// Last `stream_seq` per stream. Only this thread writes, so the cache is authoritative.
    heads: HashMap<String, i64>,
}

/// Bound on cached stream heads; the cache is simply reloaded from the index when cleared.
const MAX_CACHED_HEADS: usize = 16_384;

impl Writer {
    fn open(
        db_path: &Path,
        blobs: BlobStore,
        feed: Feed,
        counters: Arc<Counters>,
        maintenance: Arc<Maintenance>,
    ) -> Result<Self> {
        let mut conn = Connection::open(db_path)?;
        schema::configure_writer(&conn)?;
        schema::migrate(&mut conn)?;
        // AUTOINCREMENT's counter, not MAX(seq): after the newest events were deleted the next
        // seq is still above them, and so must be the cursor subscribers resync from.
        let last_seq: i64 = conn.query_row(
            "SELECT MAX(COALESCE((SELECT seq FROM sqlite_sequence WHERE name = 'events'), 0), \
                        COALESCE((SELECT MAX(seq) FROM events), 0))",
            [],
            |row| row.get(0),
        )?;
        counters.last_seq.store(last_seq, Ordering::Release);
        let mut wal_path = db_path.as_os_str().to_owned();
        wal_path.push("-wal");
        let writer = Self {
            conn,
            blobs,
            wal_path: wal_path.into(),
            feed,
            counters,
            maintenance,
            heads: HashMap::new(),
        };
        writer.record_wal_size();
        Ok(writer)
    }

    fn run(&mut self, mut rx: mpsc::Receiver<WriteCommand>) -> Result<()> {
        let mut batch = Vec::with_capacity(MAX_BATCH);
        loop {
            // Sleep until there is work, then take whatever else is already queued.
            let Some(first) = rx.blocking_recv() else {
                // Every handle was dropped without an explicit shutdown: still close cleanly.
                return self.checkpoint("TRUNCATE");
            };
            batch.push(first);
            while batch.len() < MAX_BATCH {
                match rx.try_recv() {
                    Ok(command) => batch.push(command),
                    Err(_) => break,
                }
            }

            let mut sorted = Sorted::default();
            for command in batch.drain(..) {
                sorted.push(command.op);
            }

            if sorted.shutdown.is_some() {
                // Admission is already closed; commit anything that raced in behind us.
                rx.close();
                while let Ok(command) = rx.try_recv() {
                    sorted.push(command.op);
                }
            }

            let Sorted {
                mutations,
                collections,
                checkpoints,
                compactions,
                shutdown,
            } = sorted;
            if !mutations.is_empty() {
                self.commit(mutations)?;
            }
            // Only after the commit: a rolled-back delete must not have freed its blobs.
            for collection in collections {
                let _ = collection.reply.send(self.collect(collection.hashes));
            }
            for reply in checkpoints {
                let _ = reply.send(self.checkpoint("PASSIVE"));
            }
            for (automatic, reply) in compactions {
                // A shutdown in this batch has stopped maintenance already.
                let _ = reply.send(self.compact(automatic));
            }
            if let Some(reply) = shutdown {
                let result = self.checkpoint("TRUNCATE");
                let failed = result.as_ref().err().map(ToString::to_string);
                let _ = reply.send(result);
                tracing::info!("store writer stopped");
                return match failed {
                    Some(err) => Err(Error::Io(std::io::Error::other(err))),
                    None => Ok(()),
                };
            }
        }
    }

    /// Commits every mutation in one transaction, in queue order, isolating each command in a
    /// savepoint so one bad command fails alone. Replies and the live feed only see committed
    /// events; deletes are not broadcast.
    fn commit(&mut self, mutations: Vec<Mutation>) -> Result<()> {
        let started = Instant::now();
        let commands = mutations.len() as u64;
        let mut outcomes = Vec::with_capacity(mutations.len());

        let mut tx = self.conn.transaction()?;
        for mutation in mutations {
            let mut savepoint = tx.savepoint()?;
            let outcome = match mutation {
                Mutation::Append {
                    events,
                    retention,
                    reply,
                } => Outcome::Append(
                    reply,
                    append(&savepoint, &mut self.heads, events, retention),
                ),
                Mutation::Delete {
                    streams,
                    prefixes,
                    reply,
                } => Outcome::Delete(
                    reply,
                    delete(&savepoint, &mut self.heads, &streams, &prefixes),
                ),
            };
            if outcome.is_ok() {
                savepoint.commit()?;
            } else {
                savepoint.rollback()?;
                // Heads touched by the failed command may disagree with the table.
                self.heads.clear();
            }
            outcomes.push(outcome);
        }
        if let Err(err) = tx.commit() {
            self.heads.clear();
            let message = err.to_string();
            for outcome in outcomes {
                outcome.fail(&message);
            }
            return Err(err.into());
        }

        let mut committed = 0u64;
        for outcome in outcomes {
            match outcome {
                Outcome::Append(reply, outcome) => {
                    if let Ok(stored) = &outcome {
                        for event in stored {
                            committed += 1;
                            self.counters.last_seq.store(event.seq, Ordering::Release);
                            // No receivers is fine; lagging receivers are handled on their side.
                            let _ = self.feed.send(event.clone());
                        }
                    }
                    let _ = reply.send(outcome);
                }
                Outcome::Delete(reply, outcome) => {
                    let _ = reply.send(outcome);
                }
            }
        }

        let c = &self.counters;
        c.committed_batches.fetch_add(1, Ordering::Relaxed);
        c.committed_events.fetch_add(committed, Ordering::Relaxed);
        c.last_batch_commands.store(commands, Ordering::Relaxed);
        c.last_commit_us
            .store(started.elapsed().as_micros() as u64, Ordering::Relaxed);
        self.record_wal_size();
        if self.heads.len() > MAX_CACHED_HEADS {
            self.heads.clear();
        }
        Ok(())
    }

    /// Deletes each blob that no event references and that was not put or touched after its
    /// cutoff. Runs on this thread, between batches, so no event can be appended between the
    /// reference check and the delete; the blob lock orders it against concurrent puts.
    fn collect(&self, hashes: Vec<(BlobHash, SystemTime)>) -> Result<Collected> {
        let mut referenced = self
            .conn
            .prepare_cached("SELECT EXISTS (SELECT 1 FROM blob_refs WHERE hash = ?1)")?;
        let mut collected = Collected::default();
        for (hash, cutoff) in hashes {
            if referenced.query_row([hash.as_str()], |row| row.get::<_, bool>(0))? {
                collected.referenced += 1;
                continue;
            }
            match self.blobs.remove_if_stale_blocking(&hash, cutoff) {
                Ok(Removal::Removed { bytes }) => {
                    collected.removed += 1;
                    collected.removed_bytes += bytes;
                }
                Ok(Removal::Recent) => collected.recent += 1,
                Ok(Removal::Missing) => {}
                Err(err) => {
                    tracing::warn!(blob = %hash, error = %err, "could not remove an unreferenced blob");
                    collected.failed += 1;
                }
            }
        }
        Ok(collected)
    }

    fn checkpoint(&self, mode: &str) -> Result<()> {
        let (busy, _log, _done): (i64, i64, i64) =
            self.conn
                .query_row(&format!("PRAGMA wal_checkpoint({mode})"), [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        if busy != 0 {
            tracing::debug!(
                mode,
                "wal checkpoint could not complete while readers were active"
            );
        }
        self.counters.checkpoints.fetch_add(1, Ordering::Relaxed);
        self.record_wal_size();
        Ok(())
    }

    /// `VACUUM` (outside any transaction: the batch's has committed), then a TRUNCATE
    /// checkpoint so the WAL gives its space back too. It stops, rolled back whole, once
    /// maintenance should, up to its copy back into the file (SQLite can't stop that part): the
    /// progress handler looks only while this `VACUUM` runs, so no other statement is ever
    /// stopped. An automatic one's checkpoint waits for readers only briefly.
    fn compact(&self, automatic: bool) -> Result<Compacted> {
        if self.maintenance.should_stop(automatic) {
            return self.stopped();
        }
        let maintenance = self.maintenance.clone();
        self.conn.progress_handler(
            STOP_CHECK_STEPS,
            Some(move || maintenance.should_stop(automatic)),
        )?;
        let vacuumed = self.conn.execute_batch("VACUUM");
        self.conn.progress_handler(0, None::<fn() -> bool>)?;
        match vacuumed {
            Ok(()) => {}
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::OperationInterrupted =>
            {
                return self.stopped();
            }
            Err(err) => return Err(err.into()),
        }
        // A reader on the old snapshot would hold the checkpoint, and every write behind it,
        // for the whole busy timeout: an automatic one waits only briefly and leaves what it
        // can't copy yet to the next checkpoint.
        if automatic {
            self.conn.busy_timeout(AUTOMATIC_CHECKPOINT_WAIT)?;
        }
        let checkpointed = self.checkpoint("TRUNCATE");
        if automatic {
            self.conn.busy_timeout(schema::WRITER_BUSY_TIMEOUT)?;
        }
        checkpointed?;
        Ok(Compacted::Done)
    }

    /// How a compaction that stopped ended: for the quit, or else (automatic) for a write.
    fn stopped(&self) -> Result<Compacted> {
        if self.maintenance.stopped.load(Ordering::SeqCst) {
            Err(Error::ShuttingDown)
        } else {
            Ok(Compacted::GaveWay)
        }
    }

    fn record_wal_size(&self) {
        let bytes = std::fs::metadata(&self.wal_path)
            .map(|m| m.len())
            .unwrap_or(0);
        self.counters.wal_bytes.store(bytes, Ordering::Relaxed);
    }
}

/// A mutation's result, held until the transaction commits.
enum Outcome {
    Append(AppendReply, Result<Vec<Arc<StoredEvent>>>),
    Delete(oneshot::Sender<Result<u64>>, Result<u64>),
}

impl Outcome {
    fn is_ok(&self) -> bool {
        match self {
            Self::Append(_, outcome) => outcome.is_ok(),
            Self::Delete(_, outcome) => outcome.is_ok(),
        }
    }

    /// Replies with the commit failure.
    fn fail(self, message: &str) {
        let err = || Error::Io(std::io::Error::other(message.to_owned()));
        match self {
            Self::Append(reply, _) => {
                let _ = reply.send(Err(err()));
            }
            Self::Delete(reply, _) => {
                let _ = reply.send(Err(err()));
            }
        }
    }
}

/// Removes every event of the exact `streams` and of every stream starting with one of
/// `prefixes`. Their `blob_refs` rows go with them (cascade); no other row changes, so nothing
/// else is renumbered. A deleted stream that is appended to again starts over at 1.
fn delete(
    conn: &Connection,
    heads: &mut HashMap<String, i64>,
    streams: &[String],
    prefixes: &[String],
) -> Result<u64> {
    let mut removed = 0u64;
    let mut exact = conn.prepare_cached("DELETE FROM events WHERE stream = ?1")?;
    for stream in streams {
        removed += exact.execute([stream])? as u64;
        heads.remove(stream);
    }
    // A range on the (stream, stream_seq) index, like `stream_heads`.
    let mut range = conn.prepare_cached("DELETE FROM events WHERE stream >= ?1 AND stream < ?2")?;
    for prefix in prefixes {
        let mut upper = prefix.clone();
        upper.push(char::MAX);
        removed += range.execute(params![prefix, upper])? as u64;
        heads.retain(|stream, _| !stream.starts_with(prefix.as_str()));
    }
    Ok(removed)
}

fn append(
    conn: &Connection,
    heads: &mut HashMap<String, i64>,
    events: Vec<NewEvent>,
    retention: Option<Retention>,
) -> Result<Vec<Arc<StoredEvent>>> {
    let mut insert = conn.prepare_cached(
        "INSERT INTO events (stream, stream_seq, kind, at_ms, payload) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    let mut stored = Vec::with_capacity(events.len());
    for event in events {
        let head = match heads.get(&event.stream) {
            Some(head) => *head,
            None => conn
                .query_row(
                    "SELECT MAX(stream_seq) FROM events WHERE stream = ?1",
                    [&event.stream],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()?
                .flatten()
                .unwrap_or(0),
        };
        let stream_seq = head + 1;
        insert.execute(params![
            event.stream,
            stream_seq,
            event.kind,
            event.at_ms,
            event.payload.get()
        ])?;
        let seq = conn.last_insert_rowid();
        schema::record_blob_refs(conn, seq, event.payload.get())?;
        heads.insert(event.stream.clone(), stream_seq);
        stored.push(Arc::new(StoredEvent {
            seq,
            stream: event.stream,
            stream_seq,
            kind: event.kind,
            at_ms: event.at_ms,
            payload: event.payload,
        }));
    }
    if let Some(Retention { keep_last }) = retention {
        let mut trim =
            conn.prepare_cached("DELETE FROM events WHERE stream = ?1 AND stream_seq <= ?2")?;
        let mut trimmed: Vec<&str> = Vec::new();
        for event in &stored {
            if trimmed.contains(&event.stream.as_str()) {
                continue;
            }
            trimmed.push(&event.stream);
            let head = heads[&event.stream];
            trim.execute(params![event.stream, head - i64::from(keep_last)])?;
        }
    }
    Ok(stored)
}
