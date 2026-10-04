//! `routing.sqlite`: what routing learns across projects and launches. Quota samples (for the
//! rolling estimates and the Usage page's charts), Brigadier's own token use per turn, task
//! outcomes per project and model, and research notes on models the registry doesn't know.
//!
//! It is small and written rarely (a sample when a window's use changes, a row per turn or
//! task), so one connection behind a mutex serves it; every call runs off the async runtime.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_providers::ProviderKind;
use brigadier_router::{Outcome, QuotaSample, ResearchNote};
use rusqlite::{Connection, OptionalExtension, params};
use rusqlite_migration::{M, Migrations};

use crate::{Error, Result};

/// Samples and turns older than this are dropped: the longest window is a week, and its rate
/// looks back a day.
pub const HISTORY_MS: i64 = 8 * 24 * 60 * 60 * 1000;

fn migrations() -> Migrations<'static> {
    Migrations::from_iter([M::up(
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
    )])
}

/// One sample of a window, as stored.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredSample {
    pub provider: ProviderKind,
    pub window: String,
    pub sample: QuotaSample,
    pub resets_at_ms: Option<i64>,
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
                "INSERT INTO quota_samples (provider, window, used_percent, resets_at_ms, at_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for stored in &samples {
                insert.execute(params![
                    stored.provider.to_string(),
                    stored.window,
                    stored.sample.used_percent,
                    stored.resets_at_ms,
                    stored.sample.at_ms,
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
                "SELECT provider, window, used_percent, resets_at_ms, at_ms FROM quota_samples
                 WHERE at_ms >= ?1 ORDER BY at_ms",
            )?;
            let rows = query.query_map(params![since_ms], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            })?;
            let mut samples = Vec::new();
            for row in rows {
                let (provider, window, used_percent, resets_at_ms, at_ms) = row?;
                if let Some(provider) = provider_of(&provider) {
                    samples.push(StoredSample {
                        provider,
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
        self.run(move |conn| {
            conn.prepare_cached(
                "INSERT INTO turn_usage (at_ms, provider, model, conversation_id, project_id,
                     task_id, input, cached_input, cache_write, output)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
            ])?;
            Ok(())
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
            let mut query = conn.prepare_cached(
                "SELECT at_ms, model, conversation_id, project_id, task_id, input, cached_input,
                     cache_write, output
                 FROM turn_usage WHERE provider = ?1 AND at_ms >= ?2",
            )?;
            let rows = query.query_map(params![provider.to_string(), since_ms], |row| {
                Ok(TurnUsage {
                    at_ms: row.get(0)?,
                    provider,
                    model: row.get(1)?,
                    conversation_id: row.get(2)?,
                    project_id: row.get(3)?,
                    task_id: row.get(4)?,
                    input: row.get(5)?,
                    cached_input: row.get(6)?,
                    cache_write: row.get(7)?,
                    output: row.get(8)?,
                })
            })?;
            rows.collect()
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

    /// Drops samples and turns older than [`HISTORY_MS`].
    pub async fn prune(self: &Arc<Self>, now_ms: i64) -> Result<()> {
        let before = now_ms - HISTORY_MS;
        self.run(move |conn| {
            conn.execute("DELETE FROM quota_samples WHERE at_ms < ?1", [before])?;
            conn.execute("DELETE FROM turn_usage WHERE at_ms < ?1", [before])?;
            Ok(())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[tokio::test]
    async fn a_deleted_conversation_leaves_no_turns_or_outcomes() {
        let dir = std::env::temp_dir().join(format!("brigadier-routing-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
