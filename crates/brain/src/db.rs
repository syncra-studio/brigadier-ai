//! The Brain's connections (one writer thread, a few read connections) and the row codec
//! shared by reads and writes.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, mpsc};

use rusqlite::{Connection, OpenFlags, Row, Transaction, params_from_iter};

use crate::{EdgeKind, Error, FileRef, Node, NodeKind, NodeState, Result, embed, schema};

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error::Db(err.to_string())
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// The one thread that writes. Commands run in queue order, each in its own transaction; the
/// caller blocks until its command has committed (or failed).
pub(crate) struct Writer {
    jobs: mpsc::Sender<Job>,
}

impl Writer {
    /// Opens the database at `path`, migrates it and starts the writer thread.
    pub(crate) fn spawn(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        schema::configure_writer(&conn)?;
        schema::migrate(&mut conn).map_err(|err| Error::Db(format!("migrating: {err}")))?;
        let (jobs, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("brain-writer".into())
            .spawn(move || {
                // Ends when every Brain handle is gone, after the queued commands.
                for job in rx {
                    // A panicking command drops its reply (the caller sees an error); the
                    // thread and its connection stay usable.
                    if catch_unwind(AssertUnwindSafe(|| job(&mut conn))).is_err() {
                        tracing::error!("brain write panicked");
                    }
                }
            })
            .map_err(|err| Error::Db(format!("starting the writer: {err}")))?;
        Ok(Self { jobs })
    }

    /// Runs `write` in a transaction on the writer thread, then `after` (still on that thread,
    /// so side effects apply in commit order), and waits for the result.
    pub(crate) fn run<T: Send + 'static>(
        &self,
        write: impl FnOnce(&Transaction) -> Result<T> + Send + 'static,
        after: impl FnOnce(&T) + Send + 'static,
    ) -> Result<T> {
        let (reply, answer) = mpsc::sync_channel(1);
        let job: Job = Box::new(move |conn| {
            let result = conn.transaction().map_err(Error::from).and_then(|tx| {
                let value = write(&tx)?;
                tx.commit()?;
                Ok(value)
            });
            if let Ok(value) = &result {
                after(value);
            }
            let _ = reply.send(result);
        });
        self.jobs.send(job).map_err(|_| Error::Closed)?;
        answer.recv().map_err(|_| Error::Closed)?
    }
}

/// A few read-only connections; a read takes whichever is free.
pub(crate) struct Readers {
    conns: Vec<Mutex<Connection>>,
    next: AtomicUsize,
}

/// Enough for the tool calls, the Inspector and a background job to read at once.
const READERS: usize = 3;

impl Readers {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let conns = (0..READERS)
            .map(|_| {
                let conn = Connection::open_with_flags(
                    path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY
                        | OpenFlags::SQLITE_OPEN_NO_MUTEX
                        | OpenFlags::SQLITE_OPEN_URI,
                )?;
                schema::configure_reader(&conn)?;
                Ok(Mutex::new(conn))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            conns,
            next: AtomicUsize::new(0),
        })
    }

    pub(crate) fn run<T>(&self, read: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let start = self.next.fetch_add(1, Ordering::Relaxed);
        for offset in 0..self.conns.len() {
            if let Ok(conn) = self.conns[(start + offset) % self.conns.len()].try_lock() {
                return read(&conn);
            }
        }
        read(&lock(&self.conns[start % self.conns.len()]))
    }
}

pub(crate) fn kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Module => "module",
        NodeKind::Service => "service",
        NodeKind::FileSummary => "fileSummary",
        NodeKind::Decision => "decision",
        NodeKind::Convention => "convention",
        NodeKind::Preference => "preference",
        NodeKind::Task => "task",
        NodeKind::Report => "report",
        NodeKind::Research => "research",
        NodeKind::Contract => "contract",
        NodeKind::Constraint => "constraint",
    }
}

pub(crate) fn parse_kind(text: &str) -> Option<NodeKind> {
    Some(match text {
        "module" => NodeKind::Module,
        "service" => NodeKind::Service,
        "fileSummary" => NodeKind::FileSummary,
        "decision" => NodeKind::Decision,
        "convention" => NodeKind::Convention,
        "preference" => NodeKind::Preference,
        "task" => NodeKind::Task,
        "report" => NodeKind::Report,
        "research" => NodeKind::Research,
        "contract" => NodeKind::Contract,
        "constraint" => NodeKind::Constraint,
        _ => return None,
    })
}

pub(crate) fn edge_kind_str(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Contains => "contains",
        EdgeKind::DependsOn => "dependsOn",
        EdgeKind::About => "about",
        EdgeKind::DecidedIn => "decidedIn",
        EdgeKind::Supersedes => "supersedes",
        EdgeKind::Implements => "implements",
        EdgeKind::Consumes => "consumes",
        EdgeKind::Relates => "relates",
    }
}

pub(crate) fn parse_edge_kind(text: &str) -> Option<EdgeKind> {
    Some(match text {
        "contains" => EdgeKind::Contains,
        "dependsOn" => EdgeKind::DependsOn,
        "about" => EdgeKind::About,
        "decidedIn" => EdgeKind::DecidedIn,
        "supersedes" => EdgeKind::Supersedes,
        "implements" => EdgeKind::Implements,
        "consumes" => EdgeKind::Consumes,
        "relates" => EdgeKind::Relates,
        _ => return None,
    })
}

fn corrupt(column: usize, message: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(column, rusqlite::types::Type::Text, message.into())
}

/// The node columns [`node_row`] reads, in order.
pub(crate) const NODE_COLUMNS: &str = "id, kind, key, title, body, state, stale_reason, \
    stale_since_ms, superseded_by, provenance, created_ms, updated_ms, expires_ms, \
    embedding IS NOT NULL, embed_model, superseded_reason, superseded_ms";

/// A node from a row of [`NODE_COLUMNS`], without its files.
pub(crate) fn node_row(row: &Row<'_>) -> rusqlite::Result<Node> {
    let kind: String = row.get(1)?;
    let state: String = row.get(5)?;
    let provenance: String = row.get(9)?;
    let has_embedding: bool = row.get(13)?;
    let embed_model: Option<String> = row.get(14)?;
    Ok(Node {
        id: row.get(0)?,
        kind: parse_kind(&kind).ok_or_else(|| corrupt(1, format!("unknown node kind {kind}")))?,
        key: row.get(2)?,
        title: row.get(3)?,
        body: row.get(4)?,
        state: match state.as_str() {
            "fresh" => NodeState::Fresh,
            "stale" => NodeState::Stale {
                reason: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                since_ms: row.get::<_, Option<i64>>(7)?.unwrap_or_default(),
            },
            "superseded" => NodeState::Superseded {
                by: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
                reason: row.get(15)?,
                since_ms: row.get(16)?,
            },
            other => return Err(corrupt(5, format!("unknown node state {other}"))),
        },
        provenance: serde_json::from_str(&provenance)
            .map_err(|err| corrupt(9, format!("provenance: {err}")))?,
        files: Vec::new(),
        created_at_ms: row.get(10)?,
        updated_at_ms: row.get(11)?,
        expires_at_ms: row.get(12)?,
        embedded: has_embedding && embed_model.as_deref() == Some(embed::MODEL_ID),
    })
}

/// SQLite's bound-parameter limit is far above this; it keeps statements small.
const CHUNK: usize = 500;

/// `?, ?, …` for `n` parameters.
pub(crate) fn placeholders(n: usize) -> String {
    let mut out = String::with_capacity(n * 3);
    for index in 0..n {
        out.push_str(if index == 0 { "?" } else { ", ?" });
    }
    out
}

/// Fills in the files of `nodes`.
pub(crate) fn attach_files(conn: &Connection, nodes: &mut [Node]) -> Result<()> {
    let mut files: HashMap<String, Vec<FileRef>> = HashMap::new();
    for chunk in nodes.chunks(CHUNK) {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT node_id, path, hash FROM ( \
                 SELECT f.node_id, f.path, f.hash, row_number() OVER ( \
                     PARTITION BY f.node_id, f.path \
                     ORDER BY s.recorded_ms DESC, s.source DESC) AS newest \
                 FROM node_files f JOIN node_sources s \
                 ON s.node_id = f.node_id AND s.source = f.source \
                 WHERE f.node_id IN ({})) \
             WHERE newest = 1 ORDER BY node_id, path",
            placeholders(chunk.len())
        ))?;
        let mut rows = statement.query(params_from_iter(chunk.iter().map(|node| &node.id)))?;
        while let Some(row) = rows.next()? {
            files.entry(row.get(0)?).or_default().push(FileRef {
                path: row.get(1)?,
                hash: row.get(2)?,
            });
        }
    }
    for node in nodes {
        if let Some(list) = files.remove(&node.id) {
            node.files = list;
        }
    }
    Ok(())
}

/// The nodes with these ids (unknown ids are skipped), in no particular order, with their
/// files.
pub(crate) fn nodes_by_id(conn: &Connection, ids: &[String]) -> Result<Vec<Node>> {
    let mut nodes = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(CHUNK) {
        let mut statement = conn.prepare_cached(&format!(
            "SELECT {NODE_COLUMNS} FROM nodes WHERE id IN ({})",
            placeholders(chunk.len())
        ))?;
        let rows = statement.query_map(params_from_iter(chunk), node_row)?;
        for node in rows {
            nodes.push(node?);
        }
    }
    attach_files(conn, &mut nodes)?;
    Ok(nodes)
}

/// Resolves an edge endpoint: a node id, or `key:<node key>` (any kind; a key two kinds
/// share is ambiguous).
pub(crate) fn resolve(conn: &Connection, endpoint: &str) -> Result<String> {
    if let Some(key) = endpoint.strip_prefix("key:") {
        let mut statement = conn.prepare_cached(
            "SELECT id FROM nodes WHERE key = ?1 AND state != 'superseded' LIMIT 2",
        )?;
        let ids = statement
            .query_map([key], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        return match ids.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(Error::NotFound(format!("no node has the key {key}"))),
            _ => Err(Error::Invalid(format!(
                "the key {key} names nodes of more than one kind"
            ))),
        };
    }
    let mut statement = conn.prepare_cached("SELECT 1 FROM nodes WHERE id = ?1")?;
    if statement.exists([endpoint])? {
        Ok(endpoint.to_owned())
    } else {
        Err(Error::NotFound(format!("no node has the id {endpoint}")))
    }
}
