use rusqlite::{Connection, Transaction, params};
use rusqlite_migration::{HookResult, M, Migrations};

use crate::refs;

/// Schema migrations, applied in order by the writer when the store opens. Append only.
fn migrations() -> Migrations<'static> {
    Migrations::from_iter([
        M::up(
            "CREATE TABLE events (
                seq         INTEGER PRIMARY KEY AUTOINCREMENT,
                stream      TEXT    NOT NULL,
                stream_seq  INTEGER NOT NULL,
                kind        TEXT    NOT NULL,
                at_ms       INTEGER NOT NULL,
                payload     TEXT    NOT NULL,
                UNIQUE (stream, stream_seq)
            ) STRICT;",
        )
        .comment("append-only event log"),
        // Every blob hash an event mentions, so blob GC is an index lookup instead of a scan.
        // Rows go away with their event (stream delete, retention trim) through the cascade.
        M::up_with_hook(
            "CREATE TABLE blob_refs (
                hash  TEXT    NOT NULL,
                seq   INTEGER NOT NULL REFERENCES events (seq) ON DELETE CASCADE,
                PRIMARY KEY (hash, seq)
            ) STRICT, WITHOUT ROWID;
            CREATE INDEX blob_refs_by_seq ON blob_refs (seq);",
            backfill_blob_refs,
        )
        .comment("blob references per event"),
    ])
}

/// Indexes the blob references of the events stored before `blob_refs` existed.
fn backfill_blob_refs(tx: &Transaction) -> HookResult {
    let mut events = tx.prepare("SELECT seq, payload FROM events ORDER BY seq")?;
    let mut rows = events.query([])?;
    while let Some(row) = rows.next()? {
        let seq: i64 = row.get(0)?;
        let payload: String = row.get(1)?;
        record_blob_refs(tx, seq, &payload)?;
    }
    Ok(())
}

/// Records every blob hash `payload` mentions as referenced by event `seq`.
pub(crate) fn record_blob_refs(conn: &Connection, seq: i64, payload: &str) -> rusqlite::Result<()> {
    let hashes = refs::blob_hashes(payload);
    if hashes.is_empty() {
        return Ok(());
    }
    let mut insert =
        conn.prepare_cached("INSERT OR IGNORE INTO blob_refs (hash, seq) VALUES (?1, ?2)")?;
    for hash in hashes {
        insert.execute(params![hash, seq])?;
    }
    Ok(())
}

pub(crate) fn migrate(conn: &mut Connection) -> rusqlite_migration::Result<()> {
    migrations().to_latest(conn)
}

/// How long the writer waits for a lock or for readers before giving up.
pub(crate) const WRITER_BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Pragmas for the single read-write connection.
pub(crate) fn configure_writer(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // In WAL mode NORMAL is durable across application crashes; only an OS crash or power
    // loss can drop the most recent commits.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Required: `blob_refs` rows are removed with their event by `ON DELETE CASCADE`.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    // Checkpoint policy: SQLite auto-checkpoints every ~4 MB of WAL (1000 pages), the daemon
    // asks for a PASSIVE checkpoint every CHECKPOINT_INTERVAL, shutdown TRUNCATEs, and a
    // checkpointed WAL is trimmed back to this size.
    conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    conn.pragma_update(None, "journal_size_limit", 16 * 1024 * 1024)?;
    conn.busy_timeout(WRITER_BUSY_TIMEOUT)?;
    Ok(())
}

/// Pragmas for read-only pool connections.
pub(crate) fn configure_reader(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "query_only", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}
