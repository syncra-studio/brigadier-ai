//! Project Brain: the knowledge graph, its retrieval (SQLite FTS5 plus local embeddings),
//! staleness by file content hashes, and the records index `brigadier brain search` reads
//! (STRIP-DOWN-PLAN S18).
//!
//! One [`Brain`] per project (`<data>/brains/<project>/brain.sqlite`), never in a repository.
//! Every node records its [`Provenance`].
//!
//! Retrieval merges FTS5 (bm25) with embedding similarity by reciprocal-rank fusion, then adds
//! the decisions and conventions linked to what matched. Embeddings come from a local static
//! model ([`Embedder`]); until it is downloaded and loaded, retrieval is FTS5 plus the graph.
//! There is no cloud embedder: Brigadier holds no API keys (a plan note in the README).
//!
//! Threads: every method blocks (SQLite, vector scoring, model loading, downloads). A Brain
//! runs one writer thread and keeps a few read connections; callers on an async runtime use
//! `spawn_blocking` (reads, writes) or a dedicated thread (downloads, backfills). A query never
//! waits for the embedding model to load.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub use crate::route::{MAX_NAMES, Route, route};
use crate::vectors::Vectors;

mod db;
mod download;
mod embed;
mod format;
mod records;
mod retrieve;
mod route;
mod schema;
#[cfg(test)]
mod tests;
mod vectors;
mod write;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("brain database: {0}")]
    Db(String),
    #[error("embedding model: {0}")]
    Model(String),
    #[error("downloading the embedding model: {0}")]
    Download(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    /// The Brain was closed.
    #[error("the brain is closed")]
    Closed,
}

/// Whose knowledge a Brain holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Project,
}

/// The node types of PLAN.md §6 Phase 4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum NodeKind {
    Module,
    Service,
    FileSummary,
    Decision,
    Convention,
    Preference,
    Task,
    Report,
    /// Dated research (freshness): expires after its TTL.
    Research,
    Contract,
    /// A standing constraint the user set, in their own words (the whole message or an exact
    /// span of it), with its scope in its key: it is briefed verbatim while it holds.
    Constraint,
}

/// Whether a node can be trusted as it stands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NodeState {
    Fresh,
    /// A file it describes changed (or its TTL passed) since it was recorded.
    Stale {
        reason: String,
        since_ms: i64,
    },
    /// A newer node replaces it: history, shown only when asked for.
    Superseded {
        by: String,
        /// Why it was replaced ("rewritten by the skeleton pass", "replaced by …").
        reason: Option<String>,
        since_ms: Option<i64>,
    },
}

/// Where a node's knowledge came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Origin {
    /// The static code index (no model).
    Index,
    /// A worker's report.
    Report,
    /// The orchestrator (`remember`).
    Orchestrator,
    /// The user: a card they answered, or a Chat memory.
    User,
    /// A model's background pass over the code (the skeleton pass on project add, and the
    /// idle-time passes of older Brigadiers, read as this). Last: any origin no longer known
    /// reads as this.
    #[serde(other)]
    Skeleton,
}

/// The CLI model that produced a node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkerRef {
    /// `claude` or `codex`.
    pub provider: String,
    pub model: Option<String>,
}

/// Which worker, which session, which commit (PLAN.md: every node records its provenance).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Provenance {
    pub origin: Origin,
    /// The conversation (session or Chat) it was learned in.
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    /// The Brain job (a skeleton pass) that wrote it.
    pub job_id: Option<String>,
    pub worker: Option<WorkerRef>,
    /// The repository commit the knowledge was read at, or landed as.
    pub commit: Option<String>,
    pub recorded_at_ms: i64,
    /// A claim's evidence state and source details (BUILD-PLAN A11): what a worker reported,
    /// what the thread kept on its own, what a skeleton pass wrote. `None` for the user's own
    /// decisions and for what the code index read from the code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claim: Option<Claim>,
}

/// Whether a claim's evidence was checked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Evidence {
    /// Nobody checked it yet: it is never briefed as settled.
    #[default]
    Unverified,
    /// Its evidence was checked (a check that passed, a cited line re-read).
    Verified,
}

/// What makes a node a claim rather than the user's word: its evidence state, and where in
/// its source it came from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    #[serde(default)]
    pub evidence: Evidence,
    /// The version of the worker's report it came from (1 for the first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub report_version: Option<u64>,
    /// The rebirth whose hand-off note named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub generation: Option<u32>,
    /// The evidence it names, for a later check: files with line ranges, check ids.
    #[serde(default)]
    pub cites: Vec<String>,
    /// The excerpts its evidence quotes (BUILD-PLAN E1): re-read whenever the claim is used.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[ts(skip)]
    pub sources: Vec<Source>,
    /// What the re-read of [`Self::sources`] found for this use; never stored.
    #[serde(skip)]
    #[ts(skip)]
    pub source: Option<SourceCheck>,
}

/// An excerpt a claim's evidence quotes: the lines of a file (`path:12-14`) or a check's stored
/// output (`out-…`), and the exact words they show.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    /// `path:12-14` (repository-relative), or a check's `out-…` id.
    pub cite: String,
    /// The words the lines or the output show, exactly.
    pub quote: String,
    /// A check output's content hash in the blob store, when the cite is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub blob: Option<String>,
}

/// What a claim's source showed when it was re-read for one use. Only the excerpt is checked:
/// whether it supports the claim is the reader's judgement, so the claim stays unverified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceCheck {
    /// The source re-read: `path:12-14` (where the quote is now, when it moved a little), or
    /// the check's `out-…` id.
    pub cite: String,
    pub quote: String,
    /// The quoted words are still there.
    pub holds: bool,
    /// Why not, in a few words, when they aren't.
    pub why: Option<String>,
}

impl Claim {
    /// A claim nobody checked yet.
    pub fn unverified() -> Self {
        Self::default()
    }

    /// A claim checked when it was recorded.
    pub fn verified() -> Self {
        Self {
            evidence: Evidence::Verified,
            ..Self::default()
        }
    }
}

/// Where a node's knowledge came from and whether it was checked, in a few words: "user
/// decision", "worker claim, unverified", "thread claim, unverified", "skeleton
/// pass, unverified", "code index".
pub fn source_label(kind: NodeKind, provenance: &Provenance) -> &'static str {
    let verified = provenance
        .claim
        .as_ref()
        .is_some_and(|claim| claim.evidence == Evidence::Verified);
    match provenance.origin {
        Origin::User if kind == NodeKind::Preference => "user preference",
        Origin::User => "user decision",
        Origin::Index => "code index",
        Origin::Skeleton if verified => "skeleton pass, verified",
        Origin::Skeleton => "skeleton pass, unverified",
        // A thread-kept preference from before claims: the user's, as the thread noted it.
        Origin::Orchestrator if kind == NodeKind::Preference && provenance.claim.is_none() => {
            "user preference"
        }
        Origin::Orchestrator if verified => "thread claim, verified",
        Origin::Orchestrator => "thread claim, unverified",
        // A report's own passages are its findings, not rules.
        Origin::Report if provenance.claim.is_none() && !is_rule(kind) => "worker report",
        Origin::Report if verified => "worker claim, verified",
        Origin::Report => "worker claim, unverified",
    }
}

/// The node is a claim (or a skeleton pass's) nobody checked yet: briefs say so, and list it after
/// what is settled.
pub fn unverified(kind: NodeKind, provenance: &Provenance) -> bool {
    source_label(kind, provenance).ends_with("unverified")
}

/// [`source_label`], and for a claim whose source was re-read for this use, what it showed
/// (BUILD-PLAN E1): "worker claim, unverified · verified source: path:12-14 `words`". The claim
/// itself stays unverified: the source is checked, its reading is the reader's.
pub fn source_words(kind: NodeKind, provenance: &Provenance) -> String {
    let label = source_label(kind, provenance);
    let Some(checked) = provenance
        .claim
        .as_ref()
        .and_then(|claim| claim.source.as_ref())
    else {
        return label.to_owned();
    };
    let quote: String = checked
        .quote
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let quote = match quote.char_indices().nth(SOURCE_QUOTE_SHOWN) {
        Some((at, _)) => format!("{}…", &quote[..at]),
        None => quote,
    };
    if checked.holds {
        format!("{label} · verified source: {} `{quote}`", checked.cite)
    } else {
        format!(
            "{label} · its source no longer shows it: {} ({})",
            checked.cite,
            checked.why.as_deref().unwrap_or("the words aren't there")
        )
    }
}

/// How much of a quoted source a brief line shows, in characters.
const SOURCE_QUOTE_SHOWN: usize = 160;

/// Sets each claim's [`Claim::source`] for this use: `check` re-reads its sources, the first
/// that still shows its words standing for all (else the first that was read).
pub fn check_sources(nodes: &mut [&mut Node], check: &dyn Fn(&Source) -> Option<SourceCheck>) {
    for node in nodes {
        let Some(claim) = node.provenance.claim.as_mut() else {
            continue;
        };
        let checked: Vec<SourceCheck> = claim.sources.iter().filter_map(check).collect();
        claim.source = checked
            .iter()
            .find(|checked| checked.holds)
            .or_else(|| checked.first())
            .cloned();
    }
}

/// Hits in their order, except that claims nobody checked yet go after the rest (BUILD-PLAN
/// A11: verified facts first), and among those, a claim whose source still shows it goes first.
fn settled_first(hits: &mut [BrainHit]) {
    hits.sort_by_key(|hit| {
        let held = hit
            .node
            .provenance
            .claim
            .as_ref()
            .and_then(|claim| claim.source.as_ref())
            .is_some_and(|checked| checked.holds);
        (unverified(hit.node.kind, &hit.node.provenance), !held)
    });
}

/// Decisions, conventions and contracts: what later work is told to respect.
fn is_rule(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Decision | NodeKind::Convention | NodeKind::Contract | NodeKind::Constraint
    )
}

/// A file a node describes, with its content hash when the node was recorded. A different
/// hash later makes the node stale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileRef {
    /// Repository-relative, `/`-separated.
    pub path: String,
    /// BLAKE3 hex of the content; `None` if unknown (never marked stale by content).
    pub hash: Option<String>,
}

/// A stored node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub id: String,
    pub kind: NodeKind,
    /// A stable identity for nodes that are updated in place (`module:crates/core`,
    /// `file:src/main.rs`, `skeleton:stack`); `None` for one-off nodes.
    pub key: Option<String>,
    pub title: String,
    pub body: String,
    pub state: NodeState,
    pub provenance: Provenance,
    pub files: Vec<FileRef>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    /// Research nodes: when it goes stale by age.
    pub expires_at_ms: Option<i64>,
    /// It has an embedding (from the current model).
    pub embedded: bool,
}

/// A node to record. With a `key`, an existing node of that key and kind is updated in place
/// (and becomes fresh again); without one, a new node is created.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NewNode {
    pub kind: NodeKind,
    pub key: Option<String>,
    pub title: String,
    pub body: String,
    pub provenance: Provenance,
    #[serde(default)]
    pub files: Vec<FileRef>,
    #[serde(default)]
    pub expires_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    /// A module contains a file summary, a service contains a module, a report contains the
    /// parts of its findings files.
    Contains,
    DependsOn,
    /// A decision, report or convention is about this module or file summary.
    About,
    /// A decision was made in this task or report.
    DecidedIn,
    Supersedes,
    Implements,
    /// A service consumes this contract.
    Consumes,
    Relates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// An edge to record, by node id or by node key (`key:module:crates/core`).
pub type NewEdge = Edge;

/// A retrieval request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainQuery {
    pub text: String,
    /// Only these kinds (all when empty).
    #[serde(default)]
    pub kinds: Vec<NodeKind>,
    /// At most this many nodes (default 12).
    #[serde(default)]
    pub limit: Option<u32>,
    /// The answer text's budget in tokens, about 4 bytes each (default 1500).
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// The answer names each hit's files, so knowledge leads to code.
    #[serde(default)]
    pub files: bool,
    /// Earlier versions (superseded nodes) may match too, marked as such. Without it, a match
    /// on an earlier version of a rule brings the current one.
    #[serde(default)]
    pub history: bool,
    /// Fixed caps per kind of result in place of `limit`; the answer says how many more
    /// there are, for `page`.
    #[serde(default)]
    pub caps: Option<BrainCaps>,
    /// With `caps`: which page of each kind's results, from 1.
    #[serde(default)]
    pub page: Option<u32>,
}

/// How many results of each kind an answer holds, and how much of each body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainCaps {
    /// Decisions, conventions, contracts and preferences.
    pub facts: u32,
    /// Modules, services and file summaries.
    pub entities: u32,
    /// Reports, tasks and research: passages of findings.
    pub passages: u32,
    /// Bytes of one body at most.
    pub body: u32,
}

impl Default for BrainCaps {
    /// Measured on eleven real questions (2026-10): with a 1,000-token answer, the node each
    /// needed was in 10 answers of 11 (9 with 12 uncapped hits in 1,500 tokens).
    fn default() -> Self {
        Self {
            facts: 4,
            entities: 3,
            passages: 2,
            body: 600,
        }
    }
}

/// One retrieved node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainHit {
    pub node: Node,
    /// Fused score, higher is better.
    pub score: f64,
    /// Found through a link from another hit, not by the query itself.
    pub linked: bool,
}

/// What a query found, with the text the orchestrator reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainAnswer {
    pub hits: Vec<BrainHit>,
    /// The hits formatted for a model within the token budget: each with its kind, title,
    /// state (stale ones say so), body and a provenance line.
    pub text: String,
    /// Embeddings took part (the model was loaded).
    pub semantic: bool,
    /// Microseconds the query took inside the Brain.
    pub took_us: u64,
}

/// Which nodes to list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct NodeFilter {
    #[serde(default)]
    pub kinds: Vec<NodeKind>,
    /// Only nodes this conversation supports (learned in it, or learned again in it).
    #[serde(default)]
    pub session_id: Option<String>,
    /// Leave out superseded nodes.
    #[serde(default)]
    pub current_only: bool,
    /// Title/body substring.
    #[serde(default)]
    pub text: Option<String>,
    /// Newest first; at most this many (default 200).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Nodes and the edges among them, for the Inspector's graph viewer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// More nodes match than were returned.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct KindCount {
    pub kind: NodeKind,
    pub nodes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainStats {
    pub nodes: u64,
    pub edges: u64,
    pub stale: u64,
    /// When a node was last added or rewritten (none in an empty Brain): the counts miss a
    /// node updated in place.
    pub updated_ms: Option<i64>,
    /// Nodes without an embedding from the current model.
    pub unembedded: u64,
    pub kinds: Vec<KindCount>,
    /// Record entries indexed (transcripts, ledgers, hand-off notes).
    pub transcript_entries: u64,
    pub bytes_on_disk: u64,
}

/// What a record entry was read from (STRIP-DOWN-PLAN S18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordSource {
    /// A CLI session file of the session's lineage: one entry per item, numbered across its
    /// generations.
    Transcript,
    /// A whole output the CLI saved under `tool-results/`, at the position of the call that
    /// made it.
    ToolOutput,
    /// `record/ledger.md`, by line.
    Ledger,
    /// `record/ledger-archive.md`, by line.
    LedgerArchive,
    /// A hand-off note under `record/handoff/`, by line.
    Handoff,
}

impl RecordSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Transcript => "transcript",
            Self::ToolOutput => "tool-output",
            Self::Ledger => "ledger",
            Self::LedgerArchive => "ledger-archive",
            Self::Handoff => "handoff",
        }
    }

    pub(crate) fn parse(text: &str) -> Self {
        match text {
            "tool-output" => Self::ToolOutput,
            "ledger" => Self::Ledger,
            "ledger-archive" => Self::LedgerArchive,
            "handoff" => Self::Handoff,
            _ => Self::Transcript,
        }
    }
}

/// The file a ledger entry cites ("Learned from reading: <path>:<line> …"), and its content
/// hash when the entry was first indexed: a change since reads as stale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cited {
    pub path: String,
    pub line: Option<u32>,
    /// `None` when the file couldn't be read then.
    pub hash: Option<String>,
}

/// One file of a session's records, as the reader found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFile {
    /// The conversation whose folder holds it (a worker's: its session's).
    pub conversation_id: String,
    /// How hits name the session: its folder's short id, a worker's `<short>/<n>-<task8>`.
    pub session: String,
    pub source: RecordSource,
    /// The file (a transcript's: its `record/lineage.json`).
    pub path: String,
    /// Its size and time when read: an unchanged stamp is not read again.
    pub stamp: String,
}

/// One entry of a record file.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordEntry {
    /// The transcript item's number, or the file's line (from 1).
    pub position: i64,
    /// `user`, `assistant`, `command`, `tool`, `output`; a ledger line's `ledger`.
    pub role: String,
    pub at_ms: Option<i64>,
    pub text: String,
    pub cited: Option<Cited>,
}

/// A records search result.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordHit {
    pub session: String,
    pub source: RecordSource,
    pub path: String,
    pub position: i64,
    pub role: String,
    pub at_ms: Option<i64>,
    /// The matching passage, with some context (at most about 600 bytes).
    pub snippet: String,
    /// The entry's first line (a command or call), which the snippet may not reach.
    pub lead: String,
    /// The file it cites, with the hash taken when it was indexed.
    pub cited: Option<Cited>,
}

/// The local embedding model's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum EmbedderState {
    NotInstalled,
    Downloading {
        received: u64,
        total: u64,
    },
    /// Downloaded and checked, not in memory.
    Installed,
    Loading,
    Loaded,
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct EmbedderStatus {
    /// `minishlab/potion-retrieval-32M@<revision>`.
    pub model: String,
    pub model_bytes: u64,
    pub dimensions: u32,
    pub state: EmbedderState,
}

/// The local embedding model (Model2Vec static embeddings), shared by every Brain. Loaded on
/// demand, quantized in memory, and unloaded when idle.
pub struct Embedder {
    inner: Arc<embed::Inner>,
}

impl Embedder {
    /// An embedder whose model files live in `models_dir` (e.g. `<data>/models/embeddings`).
    /// Does no I/O.
    pub fn new(models_dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            inner: embed::Inner::new(models_dir),
        })
    }

    pub fn status(&self) -> EmbedderStatus {
        self.inner.status()
    }

    /// Downloads the pinned model and checks its SHA-256 (resuming a partial download).
    /// Blocks; `cancel` stops it and keeps the partial file. Already installed, it returns at
    /// once.
    pub fn download(&self, cancel: &AtomicBool) -> Result<()> {
        self.inner.download(cancel)
    }

    /// Loads the model if it is installed and not loaded yet, on the embedder's own thread;
    /// returns at once. A load that failed is retried a minute later at the soonest.
    pub fn request_load(&self) {
        self.inner.request_load();
    }

    /// Embeds `texts` if the model is loaded (normalized vectors); `None` otherwise.
    pub fn embed(&self, texts: &[&str]) -> Option<Vec<Vec<f32>>> {
        self.inner.embed(texts)
    }

    /// Frees the model if it has not been used for `idle`, and every Brain's vector cache
    /// with it.
    pub fn unload_if_idle(&self, idle: std::time::Duration) {
        self.inner.unload_if_idle(idle);
    }
}

/// A Brain: the project's (or the user's) knowledge graph. Cheap to clone (a handle).
#[derive(Clone)]
pub struct Brain {
    inner: Arc<BrainInner>,
}

struct BrainInner {
    path: PathBuf,
    scope: Scope,
    writer: db::Writer,
    readers: db::Readers,
    vectors: Arc<Vectors>,
    embedder: Arc<Embedder>,
}

/// Nodes embedded per batch by [`Brain::embed_pending`].
const EMBED_BATCH: usize = 64;

fn scope_str(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
    }
}

/// `origin` as provenance stores it.
pub(crate) fn origin_str(origin: &Origin) -> Result<String> {
    serde_json::to_value(origin)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .ok_or_else(|| Error::Invalid(format!("unnamed origin {origin:?}")))
}

fn check(node: &NewNode) -> Result<()> {
    if node.title.trim().is_empty() {
        return Err(Error::Invalid("a node needs a title".into()));
    }
    if node.key.as_deref().is_some_and(|key| key.trim().is_empty()) {
        return Err(Error::Invalid("a node's key can't be empty".into()));
    }
    Ok(())
}

/// What a node's embedding is computed from.
fn embed_text(title: &str, body: &str) -> String {
    format!("{title}\n{body}")
}

impl Brain {
    /// Opens (creating and migrating) the Brain database at `path` and starts its writer
    /// thread. A database made for another scope is refused. Open each file once and share
    /// clones of the handle: its writer and vector cache belong to the handle.
    pub fn open(path: &Path, scope: Scope, embedder: Arc<Embedder>) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| Error::Db(format!("couldn't make {}: {err}", dir.display())))?;
        }
        let writer = db::Writer::spawn(path)?;
        let stored = writer.run(
            move |tx| {
                tx.execute(
                    "INSERT OR IGNORE INTO meta (key, value) VALUES ('scope', ?1)",
                    [scope_str(scope)],
                )?;
                // The vector cache isn't loaded yet; it reads the settled states.
                write::settle_rules(tx, db::now_ms(), &mut Vec::new())?;
                Ok(
                    tx.query_row("SELECT value FROM meta WHERE key = 'scope'", [], |row| {
                        row.get::<_, String>(0)
                    })?,
                )
            },
            |_| {},
        )?;
        if stored != scope_str(scope) {
            return Err(Error::Invalid(format!(
                "{} holds a {stored} brain, not a {} one",
                path.display(),
                scope_str(scope)
            )));
        }
        let readers = db::Readers::open(path)?;
        let vectors = Arc::new(Vectors::default());
        embedder.inner.register(&vectors);
        Ok(Self {
            inner: Arc::new(BrainInner {
                path: path.to_owned(),
                scope,
                writer,
                readers,
                vectors,
                embedder,
            }),
        })
    }

    pub fn scope(&self) -> Scope {
        self.inner.scope
    }

    /// Runs `write` in one transaction on the writer thread; its vector changes apply once it
    /// commits.
    fn write<T: Send + 'static>(
        &self,
        write: impl FnOnce(&rusqlite::Transaction, &mut Vec<vectors::Change>) -> Result<T>
        + Send
        + 'static,
    ) -> Result<T> {
        let vectors = self.inner.vectors.clone();
        self.inner
            .writer
            .run(
                move |tx| {
                    let mut changes = Vec::new();
                    let value = write(tx, &mut changes)?;
                    Ok((value, changes))
                },
                move |(_, changes)| vectors.apply(changes),
            )
            .map(|(value, _)| value)
    }

    fn read<T>(&self, read: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        self.inner.readers.run(read)
    }

    /// Records a node (see [`NewNode`] for updates by key) and returns its id. It is embedded
    /// now if the model is loaded, later by [`Self::embed_pending`] otherwise.
    ///
    /// A changed decision, convention, contract or preference keeps its earlier text as a
    /// superseded node (history). A decision, convention or contract with the same content as
    /// a current one (same scope, same words) confirms that one instead of adding a copy
    /// when unkeyed. Distinct keyed identities stay separate so lookup and rewrites keep
    /// resolving to the same node.
    pub fn record(&self, node: NewNode) -> Result<String> {
        check(&node)?;
        let embedding = self.embedding(&node);
        let now = db::now_ms();
        self.write(move |tx, changes| write::record(tx, &node, embedding, now, changes))
    }

    /// `node`'s embedding, if the model is loaded.
    fn embedding(&self, node: &NewNode) -> Option<Vec<u8>> {
        self.inner
            .embedder
            .embed(&[&embed_text(&node.title, &node.body)])
            .and_then(|mut vectors| vectors.pop())
            .map(|vector| vectors::encode(&vector))
    }

    /// Records edges; an endpoint may be a node id or `key:<node key>`. Unknown endpoints are
    /// an error; an edge that exists already is kept once.
    pub fn link(&self, edges: Vec<NewEdge>) -> Result<()> {
        self.write(move |tx, _| write::link(tx, &edges))
    }

    /// Replaces the module and service edges (`contains`, `dependsOn`) leaving `sources` with
    /// `edges`, in one write: the code index's current structure, without the dependencies a
    /// manifest dropped since. Endpoints as in [`Brain::link`].
    pub fn relink_structure(&self, sources: Vec<String>, edges: Vec<NewEdge>) -> Result<()> {
        self.write(move |tx, _| write::relink_structure(tx, &sources, &edges))
    }

    /// Marks `old` superseded by `new`, saying why (and links them). Either may be
    /// `key:<node key>`. `new` takes over the edges `old`'s current fact has; `old` keeps its
    /// history.
    pub fn supersede(&self, old: &str, new: &str, reason: &str) -> Result<()> {
        let (old, new, reason) = (old.to_owned(), new.to_owned(), reason.to_owned());
        let now = db::now_ms();
        self.write(move |tx, changes| write::supersede(tx, &old, &new, &reason, now, changes))
    }

    /// Records `node` (as [`Self::record`]) and marks `old` (an id or `key:<node key>`)
    /// superseded by it, in one write: both happen or neither. Recording the node in place of
    /// `old` itself supersedes nothing. Returns the node's id.
    pub fn record_replacing(&self, node: NewNode, old: &str, reason: &str) -> Result<String> {
        check(&node)?;
        let embedding = self.embedding(&node);
        let now = db::now_ms();
        let (old, reason) = (old.to_owned(), reason.to_owned());
        self.write(move |tx, changes| {
            let id = write::record(tx, &node, embedding, now, changes)?;
            if db::resolve(tx, &old)? != id {
                write::supersede(tx, &old, &id, &reason, now, changes)?;
            }
            Ok(id)
        })
    }

    /// The user's correction of a node's text, in place: the same id, key, provenance and
    /// edges, and no earlier version kept, so what they corrected away is gone and can't come
    /// back when the node is deleted.
    pub fn edit(&self, id: &str, title: String, body: String) -> Result<()> {
        let node = self
            .node(id)?
            .ok_or_else(|| Error::NotFound(format!("no node has the id {id}")))?;
        let embedding = self
            .inner
            .embedder
            .embed(&[&embed_text(&title, &body)])
            .and_then(|mut vectors| vectors.pop())
            .map(|vector| vectors::encode(&vector));
        let fold = write::fold_key(
            node.kind,
            node.provenance.session_id.as_deref(),
            &title,
            &body,
        );
        let id = id.to_owned();
        let now = db::now_ms();
        self.write(move |tx, changes| {
            write::edit(
                tx, &id, node.kind, &title, &body, fold, embedding, now, changes,
            )
        })
    }

    /// Deletes a node and its edges. Its latest earlier version is current again.
    pub fn delete(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        let now = db::now_ms();
        self.write(move |tx, changes| write::delete(tx, &id, now, changes))
    }

    /// Forgets what the Brain learned in `session_id`, and its records ("forget what the
    /// Brain learned from this session" on delete): nodes only this session supports go,
    /// nodes another session, task or job also supports stay. Returns how many nodes went.
    pub fn forget_session(&self, session_id: &str) -> Result<u64> {
        let session = session_id.to_owned();
        let now = db::now_ms();
        self.write(move |tx, changes| {
            let gone = write::forget(tx, &write::Sources::Session(&session), now, changes)?;
            records::forget(tx, &session)?;
            Ok(gone)
        })
    }

    /// Forgets what came from one of `origins` (what the code index or a skeleton pass
    /// learned of a repository the project no longer uses): nodes no other origin
    /// supports go, and their latest earlier versions are current again. Returns how many
    /// went.
    pub fn forget_origins(&self, origins: &[Origin]) -> Result<u64> {
        let origins = origins
            .iter()
            .map(origin_str)
            .collect::<Result<Vec<String>>>()?;
        let now = db::now_ms();
        self.write(move |tx, changes| {
            let mut gone = 0;
            for origin in &origins {
                gone += write::forget(tx, &write::Sources::Origin(origin), now, changes)?;
            }
            Ok(gone)
        })
    }

    /// Records a task's nodes (its report, the parts of its findings, its decisions) and the
    /// edges `edges` makes from their ids (in `nodes`' order), in one write. Nodes the task
    /// recorded before but not now lose its support, and go if nothing else supports them.
    /// Returns the ids.
    pub fn refresh_task(
        &self,
        task_id: &str,
        nodes: Vec<NewNode>,
        edges: impl FnOnce(&[String]) -> Vec<NewEdge> + Send + 'static,
    ) -> Result<Vec<String>> {
        for node in &nodes {
            check(node)?;
        }
        let embedded: Vec<(NewNode, Option<Vec<u8>>)> = nodes
            .into_iter()
            .map(|node| {
                let embedding = self.embedding(&node);
                (node, embedding)
            })
            .collect();
        let task = task_id.to_owned();
        let now = db::now_ms();
        self.write(move |tx, changes| {
            let mut ids = Vec::with_capacity(embedded.len());
            for (node, embedding) in &embedded {
                ids.push(write::record(tx, node, embedding.clone(), now, changes)?);
            }
            write::link(tx, &edges(&ids))?;
            write::release_task(tx, &task, &ids, now, changes)?;
            Ok(ids)
        })
    }

    /// Whether any node came from `origin`.
    pub fn holds_origin(&self, origin: Origin) -> Result<bool> {
        let origin = origin_str(&origin)?;
        self.read(|conn| {
            Ok(conn
                .prepare_cached("SELECT EXISTS (SELECT 1 FROM node_sources WHERE origin = ?1)")?
                .query_row([origin], |row| row.get(0))?)
        })
    }

    /// Files changed (each with its new content hash, `None` when deleted): every fresh node
    /// recorded against another hash of one of them goes stale. Returns the ids of the nodes
    /// that went stale.
    pub fn files_changed(&self, changes: &[FileRef]) -> Result<Vec<String>> {
        let changes = changes.to_vec();
        let now = db::now_ms();
        self.write(move |tx, _| write::files_changed(tx, &changes, now))
    }

    /// Nodes past their TTL (research) go stale. Returns how many.
    pub fn expire(&self, now_ms: i64) -> Result<u64> {
        self.write(move |tx, _| write::expire(tx, now_ms))
    }

    /// Embeds up to `limit` nodes that have no embedding from the current model, newest first.
    /// The model must be loaded: until it is, this asks for it and embeds nothing. Returns how
    /// many were embedded.
    pub fn embed_pending(&self, limit: u32) -> Result<u32> {
        let embedder = &self.inner.embedder;
        if !embedder.inner.loaded() {
            embedder.request_load();
            return Ok(0);
        }
        let pending: Vec<(String, String, String)> = self.read(|conn| {
            let mut statement = conn.prepare_cached(
                "SELECT id, title, body FROM nodes \
                 WHERE embedding IS NULL OR embed_model IS NOT ?1 \
                 ORDER BY updated_ms DESC LIMIT ?2",
            )?;
            let rows = statement.query_map(rusqlite::params![embed::MODEL_ID, limit], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        })?;
        let mut stored = 0;
        for batch in pending.chunks(EMBED_BATCH) {
            let texts: Vec<String> = batch
                .iter()
                .map(|(_, title, body)| embed_text(title, body))
                .collect();
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            // Unloaded meanwhile: the rest waits for the next call.
            let Some(embeddings) = embedder.embed(&texts) else {
                break;
            };
            let embedded: Vec<write::Embedded> = batch
                .iter()
                .zip(embeddings)
                .map(|((id, title, body), vector)| write::Embedded {
                    id: id.clone(),
                    title: title.clone(),
                    body: body.clone(),
                    blob: vectors::encode(&vector),
                })
                .collect();
            stored +=
                self.write(move |tx, changes| write::set_embeddings(tx, embedded, changes))?;
        }
        Ok(stored)
    }

    /// Hybrid retrieval: FTS5 and embeddings fused, plus linked decisions and conventions.
    /// Stale nodes are included and marked.
    pub fn query(&self, query: &BrainQuery) -> Result<BrainAnswer> {
        self.query_with(query, &|_| None)
    }

    /// [`Self::query`], with each claim's sources re-read by `check` before the answer is
    /// written, so it shows what each one's source says now (BUILD-PLAN E1).
    pub fn query_with(
        &self,
        query: &BrainQuery,
        check: &dyn Fn(&Source) -> Option<SourceCheck>,
    ) -> Result<BrainAnswer> {
        let started = std::time::Instant::now();
        let limit = match query.caps {
            // Every candidate, for the caps to choose from.
            Some(_) => 100,
            None => query.limit.unwrap_or(12).clamp(1, 100) as usize,
        };
        let budget =
            query.max_tokens.unwrap_or(1500).clamp(50, 100_000) as usize * format::BYTES_PER_TOKEN;
        let embedder = &self.inner.embedder;
        let embedding = if query.text.trim().is_empty() {
            None
        } else {
            match embedder.embed(&[query.text.as_str()]) {
                Some(mut vectors) => vectors.pop(),
                None => {
                    // Never wait for the model: this query is full-text only.
                    embedder.request_load();
                    None
                }
            }
        };
        let hits = self.read(|conn| {
            retrieve::hits(
                conn,
                &self.inner.vectors,
                query,
                embedding.as_deref(),
                limit,
            )
        })?;
        // Only what the answer holds is re-read.
        let checked = |hits: &mut Vec<BrainHit>| {
            check_sources(
                &mut hits.iter_mut().map(|hit| &mut hit.node).collect::<Vec<_>>(),
                check,
            );
        };
        let (hits, text) = match query.caps {
            Some(caps) => {
                let (mut hits, more) = retrieve::capped(hits, caps, query.page.unwrap_or(1));
                checked(&mut hits);
                settled_first(&mut hits);
                let mut text = format::answer(&hits, budget, query.files, caps.body as usize);
                if !more.is_empty() && !hits.is_empty() {
                    text.push_str(&format!(
                        "\n\n{}; ask with page {} or a narrower question.",
                        more.join(", "),
                        query.page.unwrap_or(1).max(1) + 1
                    ));
                }
                (hits, text)
            }
            None => {
                let mut hits = hits;
                checked(&mut hits);
                settled_first(&mut hits);
                let text = format::answer(&hits, budget, query.files, format::MAX_BODY);
                (hits, text)
            }
        };
        Ok(BrainAnswer {
            hits,
            text,
            semantic: embedding.is_some(),
            took_us: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
        })
    }

    pub fn node(&self, id: &str) -> Result<Option<Node>> {
        self.read(|conn| Ok(db::nodes_by_id(conn, &[id.to_owned()])?.pop()))
    }

    /// The current node with this key and kind.
    pub fn node_by_key(&self, kind: NodeKind, key: &str) -> Result<Option<Node>> {
        self.read(|conn| {
            let node = conn
                .prepare_cached(&format!(
                    "SELECT {} FROM nodes WHERE kind = ?1 AND key = ?2 \
                     AND state != 'superseded'",
                    db::NODE_COLUMNS
                ))?
                .query_row(rusqlite::params![db::kind_str(kind), key], db::node_row)
                .optional()?;
            let mut nodes: Vec<Node> = node.into_iter().collect();
            db::attach_files(conn, &mut nodes)?;
            Ok(nodes.pop())
        })
    }

    pub fn nodes(&self, filter: &NodeFilter) -> Result<Vec<Node>> {
        self.read(|conn| list_nodes(conn, filter, 0))
    }

    /// Edges touching any of `ids`.
    pub fn edges(&self, ids: &[String]) -> Result<Vec<Edge>> {
        self.read(|conn| edges_touching(conn, ids))
    }

    /// Nodes matching `filter` and the edges among them.
    pub fn graph(&self, filter: &NodeFilter) -> Result<BrainGraph> {
        self.read(|conn| {
            // One snapshot for the nodes and their edges.
            let tx = conn.unchecked_transaction()?;
            let limit = filter_limit(filter);
            let mut nodes = list_nodes(&tx, filter, 1)?;
            let truncated = nodes.len() > limit;
            nodes.truncate(limit);
            let ids: Vec<String> = nodes.iter().map(|node| node.id.clone()).collect();
            let among: HashSet<&str> = ids.iter().map(String::as_str).collect();
            let edges = edges_touching(&tx, &ids)?
                .into_iter()
                .filter(|edge| {
                    among.contains(edge.from.as_str()) && among.contains(edge.to.as_str())
                })
                .collect();
            Ok(BrainGraph {
                nodes,
                edges,
                truncated,
            })
        })
    }

    pub fn stats(&self) -> Result<BrainStats> {
        let mut stats = self.read(|conn| {
            let count = |sql: &str, args: &[&str]| -> Result<u64> {
                let count: i64 = conn
                    .prepare_cached(sql)?
                    .query_row(rusqlite::params_from_iter(args), |row| row.get(0))?;
                Ok(count.max(0) as u64)
            };
            let mut kinds = Vec::new();
            let mut statement = conn
                .prepare_cached("SELECT kind, COUNT(*) FROM nodes GROUP BY kind ORDER BY kind")?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let kind: String = row.get(0)?;
                if let Some(kind) = db::parse_kind(&kind) {
                    kinds.push(KindCount {
                        kind,
                        nodes: row.get::<_, i64>(1)?.max(0) as u64,
                    });
                }
            }
            Ok(BrainStats {
                nodes: count("SELECT COUNT(*) FROM nodes", &[])?,
                edges: count("SELECT COUNT(*) FROM edges", &[])?,
                stale: count("SELECT COUNT(*) FROM nodes WHERE state = 'stale'", &[])?,
                updated_ms: conn
                    .prepare_cached("SELECT MAX(updated_ms) FROM nodes")?
                    .query_row([], |row| row.get(0))?,
                unembedded: count(
                    "SELECT COUNT(*) FROM nodes WHERE embedding IS NULL OR embed_model IS NOT ?1",
                    &[embed::MODEL_ID],
                )?,
                kinds,
                transcript_entries: records::count(conn)?,
                bytes_on_disk: 0,
            })
        })?;
        let mut wal = self.inner.path.clone().into_os_string();
        wal.push("-wal");
        stats.bytes_on_disk = [self.inner.path.as_os_str(), wal.as_os_str()]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|meta| meta.len())
            .sum();
        Ok(stats)
    }

    /// What record file `path` was when it was last indexed ([`RecordFile::stamp`]).
    pub fn record_stamp(&self, path: &str) -> Result<Option<String>> {
        self.read(|conn| records::stamp(conn, path))
    }

    /// Indexes record file `file` as it is now (its `entries`, in order), from the first entry
    /// that differs from what was indexed.
    pub fn sync_records(&self, file: RecordFile, entries: Vec<RecordEntry>) -> Result<()> {
        self.write(move |tx, _| records::sync(tx, &file, &entries))
    }

    /// Full-text search over the records, best matches first (then the most recent): every
    /// session's, or only `session`'s and its workers'.
    pub fn search_records(
        &self,
        query: &str,
        session: Option<&str>,
        limit: u32,
    ) -> Result<Vec<RecordHit>> {
        self.read(|conn| records::search(conn, query, session, limit))
    }

    /// Drops what was indexed from a conversation's records, its workers' too.
    pub fn forget_records(&self, conversation_id: &str) -> Result<()> {
        let conversation = conversation_id.to_owned();
        self.write(move |tx, _| records::forget(tx, &conversation))
    }
}

fn filter_limit(filter: &NodeFilter) -> usize {
    filter.limit.unwrap_or(200).clamp(1, 10_000) as usize
}

/// Nodes matching `filter`, newest first: its limit plus `extra`.
fn list_nodes(conn: &rusqlite::Connection, filter: &NodeFilter, extra: usize) -> Result<Vec<Node>> {
    use rusqlite::types::Value;
    let mut sql = format!("SELECT {} FROM nodes WHERE 1 = 1", db::NODE_COLUMNS);
    let mut args: Vec<Value> = Vec::new();
    if !filter.kinds.is_empty() {
        sql.push_str(&format!(
            " AND kind IN ({})",
            db::placeholders(filter.kinds.len())
        ));
        args.extend(
            filter
                .kinds
                .iter()
                .map(|kind| Value::Text(db::kind_str(*kind).to_owned())),
        );
    }
    if let Some(session) = &filter.session_id {
        // Shared knowledge counts for every conversation that supports it.
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM node_sources s \
             WHERE s.node_id = nodes.id AND s.session_id = ?)",
        );
        args.push(Value::Text(session.clone()));
    }
    if filter.current_only {
        sql.push_str(" AND state != 'superseded'");
    }
    if let Some(text) = filter.text.as_deref().filter(|text| !text.is_empty()) {
        let escaped: String = text
            .chars()
            .flat_map(|c| match c {
                '%' | '_' | '\\' => vec!['\\', c],
                c => vec![c],
            })
            .collect();
        let pattern = format!("%{escaped}%");
        sql.push_str(" AND (title LIKE ? ESCAPE '\\' OR body LIKE ? ESCAPE '\\')");
        args.push(Value::Text(pattern.clone()));
        args.push(Value::Text(pattern));
    }
    sql.push_str(" ORDER BY updated_ms DESC, rid DESC LIMIT ?");
    args.push(Value::Integer((filter_limit(filter) + extra) as i64));
    let mut statement = conn.prepare_cached(&sql)?;
    let mut nodes = statement
        .query_map(rusqlite::params_from_iter(args), db::node_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    db::attach_files(conn, &mut nodes)?;
    Ok(nodes)
}

fn edges_touching(conn: &rusqlite::Connection, ids: &[String]) -> Result<Vec<Edge>> {
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for chunk in ids.chunks(250) {
        let marks = db::placeholders(chunk.len());
        let mut statement = conn.prepare_cached(&format!(
            "SELECT from_id, to_id, kind FROM edges WHERE from_id IN ({marks}) \
             UNION SELECT from_id, to_id, kind FROM edges WHERE to_id IN ({marks})"
        ))?;
        let mut rows = statement.query(rusqlite::params_from_iter(chunk.iter().chain(chunk)))?;
        while let Some(row) = rows.next()? {
            let (from, to, kind): (String, String, String) =
                (row.get(0)?, row.get(1)?, row.get(2)?);
            let Some(kind) = db::parse_edge_kind(&kind) else {
                continue;
            };
            if seen.insert((from.clone(), to.clone(), kind)) {
                edges.push(Edge { from, to, kind });
            }
        }
    }
    Ok(edges)
}
