//! `routing.sqlite`: what routing learns across projects and launches. Quota samples (for the
//! rolling estimates and the Usage page's charts), Brigadier's own token use per turn, task
//! outcomes per project and model, and research notes on models the registry doesn't know.
//!
//! It is small and written rarely (a sample when a window's use changes, a row per turn or
//! task), so one connection behind a mutex serves it; every call runs off the async runtime.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_providers::ProviderKind;

use crate::accounts::AccountRef;
use brigadier_router::{Outcome, QuotaSample, ResearchNote};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};

use crate::{Error, Result};

/// Samples and turns older than this are dropped: the Usage page's Activity looks back 30 days
/// (the longest window is a week, and its rate looks back a day).
pub const HISTORY_MS: i64 = 31 * 24 * 60 * 60 * 1000;

fn migrations() -> Migrations<'static> {
    Migrations::from_iter([
        M::up(
            "CREATE TABLE quota_samples (
            provider      TEXT    NOT NULL,
            window        TEXT    NOT NULL,
            used_percent  REAL    NOT NULL,
            resets_at_ms  INTEGER,
            at_ms         INTEGER NOT NULL
        ) STRICT;
        CREATE INDEX quota_samples_at ON quota_samples (provider, window, at_ms);

        CREATE TABLE turn_usage (
            at_ms            INTEGER NOT NULL,
            provider         TEXT    NOT NULL,
            model            TEXT    NOT NULL,
            conversation_id  TEXT,
            project_id       TEXT,
            task_id          TEXT,
            input            INTEGER NOT NULL,
            cached_input     INTEGER NOT NULL,
            cache_write      INTEGER NOT NULL,
            output           INTEGER NOT NULL
        ) STRICT;
        CREATE INDEX turn_usage_at ON turn_usage (provider, at_ms);

        CREATE TABLE outcomes (
            task_id     TEXT    NOT NULL,
            provider    TEXT    NOT NULL,
            model       TEXT    NOT NULL,
            project_id  TEXT    NOT NULL,
            category    TEXT    NOT NULL,
            at_ms       INTEGER NOT NULL,
            body        TEXT    NOT NULL,
            PRIMARY KEY (task_id, provider, model)
        ) STRICT, WITHOUT ROWID;
        CREATE INDEX outcomes_project ON outcomes (project_id, at_ms);

        CREATE TABLE research (
            provider  TEXT    NOT NULL,
            model     TEXT    NOT NULL,
            at_ms     INTEGER NOT NULL,
            body      TEXT    NOT NULL,
            PRIMARY KEY (provider, model)
        ) STRICT, WITHOUT ROWID;

        CREATE TABLE meta (
            key    TEXT PRIMARY KEY,
            value  TEXT NOT NULL
        ) STRICT, WITHOUT ROWID;",
        ),
        // THREAD-PLAN.md Q8 lever 8 and Q13: each turn's step, time, request and context; the
        // Codex child threads metered from their rollouts; the thread's own edits per session.
        // Rows stored before keep NULL.
        M::up(
            "ALTER TABLE turn_usage ADD COLUMN step TEXT;
            ALTER TABLE turn_usage ADD COLUMN duration_ms INTEGER;
            ALTER TABLE turn_usage ADD COLUMN request_id TEXT;
            ALTER TABLE turn_usage ADD COLUMN context INTEGER;
            ALTER TABLE turn_usage ADD COLUMN child_thread TEXT;
            CREATE INDEX turn_usage_conversation ON turn_usage (conversation_id, at_ms);

            CREATE TABLE child_threads (
                thread_id        TEXT    PRIMARY KEY,
                conversation_id  TEXT,
                input            INTEGER NOT NULL,
                cached_input     INTEGER NOT NULL,
                cache_write      INTEGER NOT NULL,
                output           INTEGER NOT NULL,
                at_ms            INTEGER NOT NULL
            ) STRICT, WITHOUT ROWID;

            CREATE TABLE thread_edits (
                conversation_id  TEXT    PRIMARY KEY,
                branch           TEXT    NOT NULL,
                base             TEXT    NOT NULL,
                tip              TEXT    NOT NULL,
                commits          INTEGER NOT NULL,
                added            INTEGER NOT NULL,
                removed          INTEGER NOT NULL,
                at_ms            INTEGER NOT NULL
            ) STRICT, WITHOUT ROWID;",
        ),
        // THREAD-PLAN.md phase 4: what a Claude turn cost (the Inspector's per-request summary).
        M::up("ALTER TABLE turn_usage ADD COLUMN cost_usd REAL;"),
        // Several accounts per provider: whose quota a sample is, whose quota a turn used ('' for
        // the user's own login, as every row stored before).
        M::up(
            "ALTER TABLE quota_samples ADD COLUMN account TEXT NOT NULL DEFAULT '';
            ALTER TABLE turn_usage ADD COLUMN account TEXT NOT NULL DEFAULT '';",
        ),
        // BUILD-PLAN A0: the model calls behind a turn's use, and whether it is a turn's first
        // report (the Inspector's thread turns and model calls per request).
        M::up(
            "ALTER TABLE turn_usage ADD COLUMN model_calls INTEGER;
            ALTER TABLE turn_usage ADD COLUMN opens_turn INTEGER NOT NULL DEFAULT 0;",
        ),
        // BUILD-PLAN B0 (research P0.1): where cache writes come from. What was written for an
        // hour and for five minutes; the last call's prompt, which a later session of the same
        // owner (or a fork of it) is compared with; the estimated missed prefix and its calls;
        // a session's known cause on its first row, with what its first call wrote and missed;
        // and the session's CLI version and tool set, to tell an upgrade or a tool change.
        M::up(
            "ALTER TABLE turn_usage ADD COLUMN write_1h INTEGER;
            ALTER TABLE turn_usage ADD COLUMN write_5m INTEGER;
            ALTER TABLE turn_usage ADD COLUMN last_prompt INTEGER;
            ALTER TABLE turn_usage ADD COLUMN missed INTEGER;
            ALTER TABLE turn_usage ADD COLUMN missed_calls INTEGER;
            ALTER TABLE turn_usage ADD COLUMN unestimated_calls INTEGER;
            ALTER TABLE turn_usage ADD COLUMN cause TEXT;
            ALTER TABLE turn_usage ADD COLUMN cause_write INTEGER;
            ALTER TABLE turn_usage ADD COLUMN cause_missed INTEGER;
            ALTER TABLE turn_usage ADD COLUMN session_key TEXT;",
        ),
        // WORKSPACES-PLAN phase 4 (token doc item 3): each Brigadier tool call whose arguments
        // didn't match its schema, which the bench's tripwire report counts (nothing writes it
        // since the MCP server went, S16c).
        M::up(
            "CREATE TABLE bad_tool_calls (
                at_ms            INTEGER NOT NULL,
                conversation_id  TEXT,
                task_id          TEXT,
                tool             TEXT    NOT NULL
            ) STRICT;
            CREATE INDEX bad_tool_calls_at ON bad_tool_calls (at_ms);",
        ),
    ])
}

/// One sample of a window, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSample {
    pub account: AccountRef,
    pub window: String,
    pub sample: QuotaSample,
    pub resets_at_ms: Option<i64>,
}

/// The kind of step a turn's use belongs to (THREAD-PLAN.md Q8 lever 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// A session thread's turn.
    Thread,
    /// A Chat's turn.
    Chat,
    /// A task's worker.
    Worker,
    /// A one-shot review.
    Review,
    /// A Codex child thread of a worker's or a thread's, whose use only its rollout holds: the
    /// auto-review ("guardian"), the only kind seen so far.
    Guardian,
    /// A project's Brain job.
    Brain,
    /// Brigadier's own upkeep (researching a new model).
    Upkeep,
    /// Brigadier's reviewer of a Claude thread's command that leaves its sandbox.
    Escalation,
}

impl StepKind {
    const ALL: [Self; 8] = [
        Self::Thread,
        Self::Chat,
        Self::Worker,
        Self::Review,
        Self::Guardian,
        Self::Brain,
        Self::Upkeep,
        Self::Escalation,
    ];

    /// How it is stored.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Thread => "thread",
            Self::Chat => "chat",
            Self::Worker => "worker",
            Self::Review => "review",
            Self::Guardian => "guardian",
            Self::Brain => "brain",
            Self::Upkeep => "upkeep",
            Self::Escalation => "escalation",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Tokens one turn used, as its CLI reported them.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsage {
    pub at_ms: i64,
    pub provider: ProviderKind,
    pub model: String,
    pub conversation_id: Option<String>,
    pub project_id: Option<String>,
    pub task_id: Option<String>,
    pub input: i64,
    pub cached_input: i64,
    pub cache_write: i64,
    pub output: i64,
    /// The step it belongs to. Absent on rows stored before it was kept, as are the rest.
    pub step: Option<StepKind>,
    /// The wall time this use took: since its turn started, or since the turn's previous
    /// report (a Codex turn reports after each model call).
    pub duration_ms: Option<i64>,
    /// The user request it served, when known.
    pub request_id: Option<String>,
    /// The context its latest model call read (input, cached input and cache writes).
    pub context: Option<i64>,
    /// For use read from a Codex child thread's rollout ([`StepKind::Guardian`]): that thread.
    /// The A/B tools, which add Codex child threads from rollouts, leave these out.
    pub child_thread: Option<String>,
    /// What the use cost, as Claude reports it (Codex doesn't).
    pub cost_usd: Option<f64>,
    /// The extra account whose quota it used; absent: the user's own login.
    pub account: Option<String>,
    /// The model calls behind it, when its CLI's reports showed them.
    pub model_calls: Option<i64>,
    /// It is the first use reported in its turn (counted for a thread's or a review's turns).
    pub opens_turn: bool,
    /// Where its cache writes came from (BUILD-PLAN B0). Default on rows stored before.
    pub cache: CacheTrace,
}

/// Where a use's cache writes came from (BUILD-PLAN B0, research P0.1), worked out per model
/// call and summed for the row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheTrace {
    /// Of the row's cache writes, what was written for an hour and for five minutes (Claude).
    pub write_1h: i64,
    pub write_5m: i64,
    /// The prompt of its last call in the main chain: what the session's next call, or a
    /// later session of the same owner or a fork of it, should read back from the cache.
    pub last_prompt: Option<i64>,
    /// An estimate: what its calls didn't read back of the prompt before them (the previous
    /// prompt less the cache read), summed over the calls that had one to compare with.
    pub missed: i64,
    /// Its calls that missed more than [`BIG_MISS`].
    pub missed_calls: i64,
    /// Its calls with nothing to compare with, reported apart: a session's first call with
    /// nothing to inherit, a subagent's first, the first after a compaction.
    pub unestimated_calls: i64,
    /// The session's known cause, on the session's first row.
    pub cause: Option<WriteCause>,
    /// What the session's first call wrote and (estimated) missed: the cause's share.
    pub cause_write: i64,
    pub cause_missed: i64,
    /// The session's CLI version and tool set, on its first row: a later session of the same
    /// owner that differs was an upgrade or a tool change.
    pub session_key: Option<String>,
}

/// A call that missed more than this is flagged (research P0.1).
pub const BIG_MISS: i64 = 20_000;

/// Why a session started on a cache it may not share (BUILD-PLAN B0): the causes Brigadier
/// knows. What is missed beyond them is unexplained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WriteCause {
    /// A new session with nothing to inherit.
    FreshStart,
    /// The same session resumed while its cache should still live.
    Resume,
    /// A seam's successor: a new session from its hand-off.
    Seam,
    /// A worker handed on to a new session.
    Handoff,
    /// The same session moved to another model or account.
    Reroute,
    /// The same session resumed at another effort.
    EffortChange,
    /// Resumed with another tool set than its last session had.
    ToolSetChange,
    /// Resumed on another CLI version than its last session ran.
    CliUpgrade,
}

impl WriteCause {
    const ALL: [Self; 8] = [
        Self::FreshStart,
        Self::Resume,
        Self::Seam,
        Self::Handoff,
        Self::Reroute,
        Self::EffortChange,
        Self::ToolSetChange,
        Self::CliUpgrade,
    ];

    /// How it is stored and shown.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FreshStart => "fresh start",
            Self::Resume => "resume",
            Self::Seam => "seam",
            Self::Handoff => "hand-off",
            Self::Reroute => "reroute",
            Self::EffortChange => "effort change",
            Self::ToolSetChange => "tool-set change",
            Self::CliUpgrade => "CLI upgrade",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.as_str() == text)
    }

    /// Whether the session carries on an earlier one's conversation: its first call is
    /// compared with that session's last prompt.
    pub fn continues(self) -> bool {
        !matches!(self, Self::FreshStart | Self::Seam | Self::Handoff)
    }
}

/// The last session of an owner, as its rows recorded it: its last call's prompt and its
/// CLI version and tool set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastSession {
    pub last_prompt: Option<i64>,
    pub session_key: Option<String>,
}

impl TurnUsage {
    /// Everything the turn's calls read and wrote.
    pub fn total(&self) -> i64 {
        self.input + self.cached_input + self.cache_write + self.output
    }
}

/// The thread's own commits on a session's branch, as last counted (THREAD-PLAN.md Q13). Kept
/// so a merged and deleted branch still shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredEdits {
    pub branch: String,
    /// Where the thread's commits were first looked for on it.
    pub base: String,
    /// The branch's tip they were counted at.
    pub tip: String,
    pub commits: u32,
    pub added: i64,
    pub removed: i64,
    pub at_ms: i64,
}

/// The columns [`turn_of`] reads, after `provider`.
const TURN_COLUMNS: &str = "at_ms, model, conversation_id, project_id, task_id, input, \
     cached_input, cache_write, output, step, duration_ms, request_id, context, child_thread, \
     cost_usd, account, model_calls, opens_turn, write_1h, write_5m, last_prompt, missed, \
     missed_calls, unestimated_calls, cause, cause_write, cause_missed, session_key";

fn turn_of(provider: ProviderKind, row: &rusqlite::Row<'_>) -> rusqlite::Result<TurnUsage> {
    Ok(TurnUsage {
        at_ms: row.get(1)?,
        provider,
        model: row.get(2)?,
        conversation_id: row.get(3)?,
        project_id: row.get(4)?,
        task_id: row.get(5)?,
        input: row.get(6)?,
        cached_input: row.get(7)?,
        cache_write: row.get(8)?,
        output: row.get(9)?,
        step: row
            .get::<_, Option<String>>(10)?
            .as_deref()
            .and_then(StepKind::parse),
        duration_ms: row.get(11)?,
        request_id: row.get(12)?,
        context: row.get(13)?,
        child_thread: row.get(14)?,
        cost_usd: row.get(15)?,
        account: Some(row.get::<_, String>(16)?).filter(|account| !account.is_empty()),
        model_calls: row.get(17)?,
        opens_turn: row.get(18)?,
        cache: CacheTrace {
            write_1h: row.get::<_, Option<i64>>(19)?.unwrap_or_default(),
            write_5m: row.get::<_, Option<i64>>(20)?.unwrap_or_default(),
            last_prompt: row.get(21)?,
            missed: row.get::<_, Option<i64>>(22)?.unwrap_or_default(),
            missed_calls: row.get::<_, Option<i64>>(23)?.unwrap_or_default(),
            unestimated_calls: row.get::<_, Option<i64>>(24)?.unwrap_or_default(),
            cause: row
                .get::<_, Option<String>>(25)?
                .as_deref()
                .and_then(WriteCause::parse),
            cause_write: row.get::<_, Option<i64>>(26)?.unwrap_or_default(),
            cause_missed: row.get::<_, Option<i64>>(27)?.unwrap_or_default(),
            session_key: row.get(28)?,
        },
    })
}

fn insert_turn(conn: &Connection, turn: &TurnUsage) -> rusqlite::Result<()> {
    conn.prepare_cached(
        "INSERT INTO turn_usage (at_ms, provider, model, conversation_id, project_id, task_id,
             input, cached_input, cache_write, output, step, duration_ms, request_id, context,
             child_thread, cost_usd, account, model_calls, opens_turn, write_1h, write_5m,
             last_prompt, missed, missed_calls, unestimated_calls, cause, cause_write,
             cause_missed, session_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29)",
    )?
    .execute(params![
        turn.at_ms,
        turn.provider.to_string(),
        turn.model,
        turn.conversation_id,
        turn.project_id,
        turn.task_id,
        turn.input,
        turn.cached_input,
        turn.cache_write,
        turn.output,
        turn.step.map(StepKind::as_str),
        turn.duration_ms,
        turn.request_id,
        turn.context,
        turn.child_thread,
        turn.cost_usd,
        turn.account.as_deref().unwrap_or(""),
        turn.model_calls,
        turn.opens_turn,
        turn.cache.write_1h,
        turn.cache.write_5m,
        turn.cache.last_prompt,
        turn.cache.missed,
        turn.cache.missed_calls,
        turn.cache.unestimated_calls,
        turn.cache.cause.map(WriteCause::as_str),
        turn.cache.cause_write,
        turn.cache.cause_missed,
        turn.cache.session_key,
    ])?;
    Ok(())
}

pub struct RoutingStore {
    conn: Mutex<Connection>,
}

fn db(err: impl std::fmt::Display) -> Error {
    Error::Invalid(format!("routing store: {err}"))
}

fn provider_of(text: &str) -> Option<ProviderKind> {
    ProviderKind::ALL
        .into_iter()
        .find(|kind| kind.to_string() == text)
}

impl RoutingStore {
    /// Opens (and migrates) the store at `path`.
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        let mut conn = Connection::open(path).map_err(db)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(db)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(db)?;
        conn.pragma_update(None, "journal_size_limit", 4 * 1024 * 1024)
            .map_err(db)?;
        // Deleted rows are overwritten, not left in free pages (see [`Self::scrub`]).
        conn.pragma_update(None, "secure_delete", "ON")
            .map_err(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db)?;
        migrations().to_latest(&mut conn).map_err(db)?;
        Ok(Arc::new(Self {
            conn: Mutex::new(conn),
        }))
    }

    /// Runs `work` on the connection off the async runtime.
    pub async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        work: impl FnOnce(&Connection) -> rusqlite::Result<T> + Send + 'static,
    ) -> Result<T> {
        let store = self.clone();
        tokio::task::spawn_blocking(move || {
            let conn = store.conn.lock().unwrap_or_else(PoisonError::into_inner);
            work(&conn).map_err(db)
        })
        .await
        .map_err(db)?
    }

    pub async fn add_samples(self: &Arc<Self>, samples: Vec<StoredSample>) -> Result<()> {
        if samples.is_empty() {
            return Ok(());
        }
        self.run(move |conn| {
            let mut insert = conn.prepare_cached(
                "INSERT INTO quota_samples
                     (provider, window, used_percent, resets_at_ms, at_ms, account)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for stored in &samples {
                insert.execute(params![
                    stored.account.provider.to_string(),
                    stored.window,
                    stored.sample.used_percent,
                    stored.resets_at_ms,
                    stored.sample.at_ms,
                    stored.account.key(),
                ])?;
            }
            Ok(())
        })
        .await
    }

    /// Samples taken since `since_ms`, oldest first.
    pub async fn samples_since(self: &Arc<Self>, since_ms: i64) -> Result<Vec<StoredSample>> {
        self.run(move |conn| {
            let mut query = conn.prepare(
                "SELECT provider, window, used_percent, resets_at_ms, at_ms, account
                 FROM quota_samples WHERE at_ms >= ?1 ORDER BY at_ms",
            )?;
            let rows = query.query_map(params![since_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            let mut samples = Vec::new();
            for row in rows {
                let (provider, window, used_percent, resets_at_ms, at_ms, account) = row?;
                if let Some(provider) = provider_of(&provider) {
                    samples.push(StoredSample {
                        account: AccountRef::new(
                            provider,
                            (!account.is_empty()).then_some(account),
                        ),
                        window,
                        sample: QuotaSample {
                            at_ms,
                            used_percent,
                        },
                        resets_at_ms,
                    });
                }
            }
            Ok(samples)
        })
        .await
    }

    pub async fn add_turn(self: &Arc<Self>, turn: TurnUsage) -> Result<()> {
        self.run(move |conn| insert_turn(conn, &turn)).await
    }

    /// The last session of an owner (a conversation's `step`, or a task's), from its newest
    /// rows: its last call's prompt and its CLI version and tool set (BUILD-PLAN B0).
    pub async fn last_session(
        self: &Arc<Self>,
        conversation_id: String,
        step: StepKind,
        task_id: Option<String>,
    ) -> Result<LastSession> {
        self.run(move |conn| {
            let newest = |column: &str| -> rusqlite::Result<Option<rusqlite::types::Value>> {
                conn.prepare_cached(&format!(
                    "SELECT {column} FROM turn_usage
                     WHERE conversation_id = ?1 AND step = ?2 AND task_id IS ?3
                       AND {column} IS NOT NULL
                     ORDER BY at_ms DESC, rowid DESC LIMIT 1"
                ))?
                .query_row(params![conversation_id, step.as_str(), task_id], |row| {
                    row.get(0)
                })
                .optional()
            };
            Ok(LastSession {
                last_prompt: match newest("last_prompt")? {
                    Some(rusqlite::types::Value::Integer(prompt)) => Some(prompt),
                    _ => None,
                },
                session_key: match newest("session_key")? {
                    Some(rusqlite::types::Value::Text(key)) => Some(key),
                    _ => None,
                },
            })
        })
        .await
    }

    /// A provider's turns since `since_ms`.
    pub async fn turns_since(
        self: &Arc<Self>,
        provider: ProviderKind,
        since_ms: i64,
    ) -> Result<Vec<TurnUsage>> {
        self.run(move |conn| {
            let mut query = conn.prepare_cached(&format!(
                "SELECT provider, {TURN_COLUMNS} FROM turn_usage WHERE provider = ?1 AND at_ms >= ?2"
            ))?;
            let rows = query.query_map(params![provider.to_string(), since_ms], |row| {
                turn_of(provider, row)
            })?;
            rows.collect()
        })
        .await
    }

    /// A conversation's turns of `step`, oldest first.
    pub async fn step_turns(
        self: &Arc<Self>,
        conversation_id: String,
        step: StepKind,
    ) -> Result<Vec<TurnUsage>> {
        self.run(move |conn| {
            let mut query = conn.prepare_cached(&format!(
                "SELECT provider, {TURN_COLUMNS} FROM turn_usage
                 WHERE conversation_id = ?1 AND step = ?2 ORDER BY at_ms, rowid"
            ))?;
            let rows = query.query_map(params![conversation_id, step.as_str()], |row| {
                let provider: String = row.get(0)?;
                provider_of(&provider)
                    .map(|provider| turn_of(provider, row))
                    .transpose()
            })?;
            rows.filter_map(|row| row.transpose()).collect()
        })
        .await
    }

    /// All of a conversation's turns, every step, oldest first.
    pub async fn conversation_turns(
        self: &Arc<Self>,
        conversation_id: String,
    ) -> Result<Vec<TurnUsage>> {
        self.run(move |conn| {
            let mut query = conn.prepare_cached(&format!(
                "SELECT provider, {TURN_COLUMNS} FROM turn_usage
                 WHERE conversation_id = ?1 ORDER BY at_ms, rowid"
            ))?;
            let rows = query.query_map(params![conversation_id], |row| {
                let provider: String = row.get(0)?;
                provider_of(&provider)
                    .map(|provider| turn_of(provider, row))
                    .transpose()
            })?;
            rows.filter_map(|row| row.transpose()).collect()
        })
        .await
    }

    /// Records what a Codex child thread used since it was last metered: `turn` carries its
    /// total so far, and becomes a row of what is new (none when nothing is). Once per use,
    /// however often the thread is looked at, and whatever turn rows were pruned since.
    pub async fn meter_child_thread(
        self: &Arc<Self>,
        thread_id: String,
        mut turn: TurnUsage,
    ) -> Result<Option<TurnUsage>> {
        self.run(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let metered: (i64, i64, i64, i64) = tx
                .query_row(
                    "SELECT input, cached_input, cache_write, output FROM child_threads
                     WHERE thread_id = ?1",
                    [&thread_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?
                .unwrap_or_default();
            let total = (turn.input, turn.cached_input, turn.cache_write, turn.output);
            turn.input = (total.0 - metered.0).max(0);
            turn.cached_input = (total.1 - metered.1).max(0);
            turn.cache_write = (total.2 - metered.2).max(0);
            turn.output = (total.3 - metered.3).max(0);
            if turn.total() == 0 {
                return Ok(None);
            }
            turn.child_thread = Some(thread_id.clone());
            insert_turn(&tx, &turn)?;
            tx.execute(
                "INSERT OR REPLACE INTO child_threads
                     (thread_id, conversation_id, input, cached_input, cache_write, output, at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    thread_id,
                    turn.conversation_id,
                    total.0.max(metered.0),
                    total.1.max(metered.1),
                    total.2.max(metered.2),
                    total.3.max(metered.3),
                    turn.at_ms,
                ],
            )?;
            tx.commit()?;
            Ok(Some(turn))
        })
        .await
    }

    /// The thread's edits as last counted for a session.
    pub async fn thread_edits(
        self: &Arc<Self>,
        conversation_id: String,
    ) -> Result<Option<StoredEdits>> {
        self.run(move |conn| {
            conn.query_row(
                "SELECT branch, base, tip, commits, added, removed, at_ms FROM thread_edits
                 WHERE conversation_id = ?1",
                [conversation_id],
                |row| {
                    Ok(StoredEdits {
                        branch: row.get(0)?,
                        base: row.get(1)?,
                        tip: row.get(2)?,
                        commits: row.get(3)?,
                        added: row.get(4)?,
                        removed: row.get(5)?,
                        at_ms: row.get(6)?,
                    })
                },
            )
            .optional()
        })
        .await
    }

    pub async fn put_thread_edits(
        self: &Arc<Self>,
        conversation_id: String,
        edits: StoredEdits,
    ) -> Result<()> {
        self.run(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO thread_edits
                     (conversation_id, branch, base, tip, commits, added, removed, at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    conversation_id,
                    edits.branch,
                    edits.base,
                    edits.tip,
                    edits.commits,
                    edits.added,
                    edits.removed,
                    edits.at_ms,
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Records (or replaces) one model's outcome on one task.
    pub async fn put_outcome(self: &Arc<Self>, outcome: Outcome) -> Result<()> {
        let body = serde_json::to_string(&outcome).map_err(db)?;
        let category = serde_json::to_value(outcome.category)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.run(move |conn| {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO outcomes
                     (task_id, provider, model, project_id, category, at_ms, body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?
            .execute(params![
                outcome.task_id,
                outcome.provider.to_string(),
                outcome.model,
                outcome.project_id,
                category,
                outcome.at_ms,
                body,
            ])?;
            Ok(())
        })
        .await
    }

    /// Every outcome recorded in `project_id` (every project when absent).
    pub async fn outcomes(self: &Arc<Self>, project_id: Option<String>) -> Result<Vec<Outcome>> {
        let bodies: Vec<String> = self
            .run(move |conn| {
                let mut query = conn.prepare_cached(
                    "SELECT body FROM outcomes WHERE ?1 IS NULL OR project_id = ?1 ORDER BY at_ms",
                )?;
                let rows = query.query_map(params![project_id], |row| row.get(0))?;
                rows.collect()
            })
            .await?;
        Ok(bodies
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect())
    }

    pub async fn put_research(self: &Arc<Self>, note: ResearchNote) -> Result<()> {
        let body = serde_json::to_string(&note).map_err(db)?;
        self.run(move |conn| {
            conn.prepare_cached(
                "INSERT OR REPLACE INTO research (provider, model, at_ms, body)
                 VALUES (?1, ?2, ?3, ?4)",
            )?
            .execute(params![
                note.provider.to_string(),
                note.model,
                note.at_ms,
                body
            ])?;
            Ok(())
        })
        .await
    }

    pub async fn research(self: &Arc<Self>) -> Result<Vec<ResearchNote>> {
        let bodies: Vec<String> = self
            .run(|conn| {
                let mut query = conn.prepare_cached("SELECT body FROM research")?;
                let rows = query.query_map([], |row| row.get(0))?;
                rows.collect()
            })
            .await?;
        Ok(bodies
            .iter()
            .filter_map(|body| serde_json::from_str(body).ok())
            .collect())
    }

    pub async fn meta(self: &Arc<Self>, key: &'static str) -> Result<Option<String>> {
        self.run(move |conn| {
            conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
                row.get(0)
            })
            .optional()
        })
        .await
    }

    pub async fn set_meta(self: &Arc<Self>, key: &'static str, value: String) -> Result<()> {
        self.run(move |conn| {
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
                params![key, value],
            )?;
            Ok(())
        })
        .await
    }

    /// Forgets a deleted conversation: its turns (also those recorded only under one of its
    /// tasks) and its tasks' outcomes. How many turns and outcomes went.
    pub async fn forget_conversation(
        self: &Arc<Self>,
        conversation_id: String,
        task_ids: Vec<String>,
    ) -> Result<(usize, usize)> {
        self.run(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let mut turns = tx.execute(
                "DELETE FROM turn_usage WHERE conversation_id = ?1",
                [&conversation_id],
            )?;
            tx.execute(
                "DELETE FROM child_threads WHERE conversation_id = ?1",
                [&conversation_id],
            )?;
            tx.execute(
                "DELETE FROM thread_edits WHERE conversation_id = ?1",
                [&conversation_id],
            )?;
            let mut outcomes = 0;
            {
                let mut task_turns = tx.prepare("DELETE FROM turn_usage WHERE task_id = ?1")?;
                let mut task_outcomes = tx.prepare("DELETE FROM outcomes WHERE task_id = ?1")?;
                for task in &task_ids {
                    turns += task_turns.execute([task])?;
                    outcomes += task_outcomes.execute([task])?;
                }
            }
            tx.commit()?;
            Ok((turns, outcomes))
        })
        .await
    }

    /// Empties the WAL after a tab's chat was forgotten, so no copy of its rows stays behind.
    pub async fn scrub(self: &Arc<Self>) -> Result<()> {
        self.run(|conn| {
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
            Ok(())
        })
        .await
    }

    /// Drops samples and turns older than [`HISTORY_MS`].
    pub async fn prune(self: &Arc<Self>, now_ms: i64) -> Result<()> {
        let before = now_ms - HISTORY_MS;
        self.run(move |conn| {
            conn.execute("DELETE FROM quota_samples WHERE at_ms < ?1", [before])?;
            conn.execute("DELETE FROM turn_usage WHERE at_ms < ?1", [before])?;
            conn.execute("DELETE FROM bad_tool_calls WHERE at_ms < ?1", [before])?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn turn(conversation: Option<&str>, task: Option<&str>) -> TurnUsage {
        TurnUsage {
            at_ms: 1,
            provider: ProviderKind::Claude,
            model: "opus".into(),
            conversation_id: conversation.map(str::to_owned),
            project_id: Some("p".into()),
            task_id: task.map(str::to_owned),
            input: 1,
            cached_input: 0,
            cache_write: 0,
            output: 1,
            step: None,
            duration_ms: None,
            request_id: None,
            context: None,
            child_thread: None,
            cost_usd: None,
            account: None,
            model_calls: None,
            opens_turn: false,
            cache: Default::default(),
        }
    }

    fn outcome(task: &str) -> Outcome {
        serde_json::from_value(serde_json::json!({
            "projectId": "p",
            "taskId": task,
            "provider": "claude",
            "model": "opus",
            "category": "implement",
            "areas": [],
            "result": "landed",
            "reviewPassedFirst": null,
            "reviews": 0,
            "reworkRounds": 0,
            "verification": null,
            "durationMs": 1,
            "tokens": 1,
            "quotaPercent": null,
            "atMs": 1
        }))
        .expect("an outcome")
    }

    #[test]
    fn turns_stored_before_the_steps_were_kept_read_back_without_them() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let path = dir.join("routing.sqlite");
        {
            // The store as its first version left it, with a turn in it.
            let mut conn = Connection::open(&path).unwrap();
            migrations().to_version(&mut conn, 1).unwrap();
            conn.execute(
                "INSERT INTO turn_usage VALUES (1, 'claude', 'opus', 'c1', 'p', NULL, 1, 2, 3, 4)",
                [],
            )
            .unwrap();
        }
        let store = RoutingStore::open(&path).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let turns = runtime
            .block_on(store.turns_since(ProviderKind::Claude, 0))
            .unwrap();
        let mut old = turn(Some("c1"), None);
        (old.input, old.cached_input, old.cache_write, old.output) = (1, 2, 3, 4);
        assert_eq!(turns, [old]);
    }

    #[tokio::test]
    async fn samples_stored_before_accounts_read_back_as_the_users_own() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let path = dir.join("routing.sqlite");
        {
            let mut conn = Connection::open(&path).unwrap();
            migrations().to_version(&mut conn, 1).unwrap();
            conn.execute(
                "INSERT INTO quota_samples VALUES ('codex', 'primary', 40.0, NULL, 5)",
                [],
            )
            .unwrap();
        }
        let store = RoutingStore::open(&path).unwrap();
        let work = AccountRef::new(ProviderKind::Codex, Some("acct-1".into()));
        store
            .add_samples(vec![StoredSample {
                account: work.clone(),
                window: "primary".into(),
                sample: QuotaSample {
                    at_ms: 6,
                    used_percent: 10.0,
                },
                resets_at_ms: None,
            }])
            .await
            .unwrap();
        let samples = store.samples_since(0).await.unwrap();
        let accounts: Vec<_> = samples
            .iter()
            .map(|stored| stored.account.clone())
            .collect();
        assert_eq!(accounts, [AccountRef::own(ProviderKind::Codex), work]);
    }

    #[tokio::test]
    async fn a_deleted_conversation_leaves_no_turns_or_outcomes() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let store = RoutingStore::open(&dir.join("routing.sqlite")).unwrap();
        // Its orchestrator's turn, a worker's turn, and a turn known only by its task.
        store.add_turn(turn(Some("c1"), None)).await.unwrap();
        store.add_turn(turn(Some("c1"), Some("t1"))).await.unwrap();
        store.add_turn(turn(None, Some("t2"))).await.unwrap();
        // Another conversation's and a project Brain job's stay.
        store.add_turn(turn(Some("c2"), Some("t3"))).await.unwrap();
        store.add_turn(turn(None, None)).await.unwrap();
        for task in ["t1", "t2", "t3"] {
            store.put_outcome(outcome(task)).await.unwrap();
        }
        let forgotten = store
            .forget_conversation("c1".into(), vec!["t1".into(), "t2".into()])
            .await
            .unwrap();
        assert_eq!(forgotten, (3, 2));
        let left = store.turns_since(ProviderKind::Claude, 0).await.unwrap();
        assert_eq!(left.len(), 2);
        assert!(
            left.iter()
                .all(|turn| turn.conversation_id.as_deref() != Some("c1"))
        );
        let outcomes = store.outcomes(None).await.unwrap();
        assert_eq!(
            outcomes
                .iter()
                .map(|o| o.task_id.as_str())
                .collect::<Vec<_>>(),
            ["t3"]
        );
    }

    /// BUILD-PLAN A0: a turn's model calls and whether it opens its turn are kept.
    #[tokio::test]
    async fn a_turns_calls_and_its_first_use_read_back() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let store = RoutingStore::open(&dir.join("routing.sqlite")).unwrap();
        let counted = TurnUsage {
            model_calls: Some(3),
            opens_turn: true,
            ..turn(Some("c1"), None)
        };
        store.add_turn(counted.clone()).await.unwrap();
        store.add_turn(turn(Some("c1"), None)).await.unwrap();
        let read = store.conversation_turns("c1".into()).await.unwrap();
        assert_eq!(read, vec![counted, turn(Some("c1"), None)]);
    }

    /// BUILD-PLAN B0: a row's cache trace reads back, and an owner's last session is its
    /// newest prompt and its newest CLI version and tool set, each its own owner's.
    #[tokio::test]
    async fn a_turns_cache_trace_and_its_owners_last_session_read_back() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir_all(&*dir).unwrap();
        let store = RoutingStore::open(&dir.join("routing.sqlite")).unwrap();
        let thread = |at_ms: i64, cache: CacheTrace| TurnUsage {
            at_ms,
            step: Some(StepKind::Thread),
            cache,
            ..turn(Some("c1"), None)
        };
        let first = thread(
            1,
            CacheTrace {
                write_1h: 900,
                write_5m: 100,
                last_prompt: Some(40_000),
                missed: 25_000,
                missed_calls: 1,
                unestimated_calls: 0,
                cause: Some(WriteCause::Resume),
                cause_write: 1_000,
                cause_missed: 25_000,
                session_key: Some("2.1.300|Thread brigadier".into()),
            },
        );
        let later = thread(
            2,
            CacheTrace {
                last_prompt: Some(41_000),
                ..CacheTrace::default()
            },
        );
        store.add_turn(first.clone()).await.unwrap();
        store.add_turn(later.clone()).await.unwrap();
        // A worker's row of the same conversation is another owner.
        store
            .add_turn(TurnUsage {
                at_ms: 3,
                step: Some(StepKind::Worker),
                cache: CacheTrace {
                    last_prompt: Some(9),
                    ..CacheTrace::default()
                },
                ..turn(Some("c1"), Some("t1"))
            })
            .await
            .unwrap();
        let read = store
            .step_turns("c1".into(), StepKind::Thread)
            .await
            .unwrap();
        assert_eq!(read, vec![first, later]);
        assert_eq!(
            store
                .last_session("c1".into(), StepKind::Thread, None)
                .await
                .unwrap(),
            LastSession {
                last_prompt: Some(41_000),
                session_key: Some("2.1.300|Thread brigadier".into()),
            }
        );
        assert_eq!(
            store
                .last_session("c1".into(), StepKind::Worker, Some("t1".into()))
                .await
                .unwrap()
                .last_prompt,
            Some(9)
        );
        assert_eq!(
            store
                .last_session("c2".into(), StepKind::Thread, None)
                .await
                .unwrap(),
            LastSession::default()
        );
    }
}
