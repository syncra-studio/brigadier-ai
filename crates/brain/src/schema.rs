use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

/// Schema migrations, applied in order by the writer when a Brain opens. Append only: unlike
/// the code index, a Brain is not a cache and cannot be rebuilt from anything.
fn migrations() -> Migrations<'static> {
    Migrations::from_iter([
        M::up(
            // `rid` is the rowid the FTS tables point at; an explicit INTEGER PRIMARY KEY keeps it
            // stable across VACUUM, which an implicit rowid is not.
            "CREATE TABLE meta (
            key    TEXT PRIMARY KEY,
            value  TEXT NOT NULL
        ) STRICT, WITHOUT ROWID;

        CREATE TABLE nodes (
            rid             INTEGER PRIMARY KEY,
            id              TEXT    NOT NULL UNIQUE,
            kind            TEXT    NOT NULL,
            key             TEXT,
            title           TEXT    NOT NULL,
            body            TEXT    NOT NULL,
            state           TEXT    NOT NULL DEFAULT 'fresh',
            stale_reason    TEXT,
            stale_since_ms  INTEGER,
            superseded_by   TEXT,
            provenance      TEXT    NOT NULL,
            session_id      TEXT,
            created_ms      INTEGER NOT NULL,
            updated_ms      INTEGER NOT NULL,
            expires_ms      INTEGER,
            embedding       BLOB,
            embed_model     TEXT
        ) STRICT;
        CREATE UNIQUE INDEX nodes_by_kind_key ON nodes (kind, key) WHERE key IS NOT NULL;
        CREATE INDEX nodes_by_key ON nodes (key) WHERE key IS NOT NULL;
        CREATE INDEX nodes_by_session ON nodes (session_id) WHERE session_id IS NOT NULL;
        CREATE INDEX nodes_by_updated ON nodes (updated_ms);
        CREATE INDEX nodes_by_superseder ON nodes (superseded_by) WHERE superseded_by IS NOT NULL;
        CREATE INDEX nodes_expiring ON nodes (expires_ms) WHERE expires_ms IS NOT NULL;

        CREATE TABLE edges (
            from_id  TEXT NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
            to_id    TEXT NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
            kind     TEXT NOT NULL,
            PRIMARY KEY (from_id, to_id, kind)
        ) STRICT, WITHOUT ROWID;
        CREATE INDEX edges_by_to ON edges (to_id);

        CREATE TABLE node_files (
            node_id  TEXT NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
            path     TEXT NOT NULL,
            hash     TEXT,
            PRIMARY KEY (node_id, path)
        ) STRICT, WITHOUT ROWID;
        CREATE INDEX node_files_by_path ON node_files (path);

        CREATE VIRTUAL TABLE nodes_fts USING fts5 (
            title, body,
            content = 'nodes', content_rowid = 'rid',
            tokenize = 'porter unicode61'
        );
        CREATE TRIGGER nodes_fts_insert AFTER INSERT ON nodes BEGIN
            INSERT INTO nodes_fts (rowid, title, body) VALUES (new.rid, new.title, new.body);
        END;
        CREATE TRIGGER nodes_fts_delete AFTER DELETE ON nodes BEGIN
            INSERT INTO nodes_fts (nodes_fts, rowid, title, body)
                VALUES ('delete', old.rid, old.title, old.body);
        END;
        CREATE TRIGGER nodes_fts_update AFTER UPDATE OF title, body ON nodes BEGIN
            INSERT INTO nodes_fts (nodes_fts, rowid, title, body)
                VALUES ('delete', old.rid, old.title, old.body);
            INSERT INTO nodes_fts (rowid, title, body) VALUES (new.rid, new.title, new.body);
        END;

        CREATE TABLE transcript (
            rid              INTEGER PRIMARY KEY,
            conversation_id  TEXT    NOT NULL,
            seq              INTEGER NOT NULL,
            role             TEXT    NOT NULL,
            request_id       TEXT,
            at_ms            INTEGER NOT NULL,
            text             TEXT    NOT NULL,
            UNIQUE (conversation_id, seq)
        ) STRICT;
        CREATE VIRTUAL TABLE transcript_fts USING fts5 (
            text,
            content = 'transcript', content_rowid = 'rid',
            tokenize = 'porter unicode61'
        );
        CREATE TRIGGER transcript_fts_insert AFTER INSERT ON transcript BEGIN
            INSERT INTO transcript_fts (rowid, text) VALUES (new.rid, new.text);
        END;
        CREATE TRIGGER transcript_fts_delete AFTER DELETE ON transcript BEGIN
            INSERT INTO transcript_fts (transcript_fts, rowid, text)
                VALUES ('delete', old.rid, old.text);
        END;",
        )
        .comment("knowledge graph, its full-text index and the transcript index"),
        // History instead of overwrites: a rewritten decision, convention or contract keeps its
        // old text as a superseded node with the same key, so only current nodes hold a key.
        M::up(
            "ALTER TABLE nodes ADD COLUMN superseded_reason TEXT;
        ALTER TABLE nodes ADD COLUMN superseded_ms INTEGER;
        ALTER TABLE nodes ADD COLUMN fold_key TEXT;
        DROP INDEX nodes_by_kind_key;
        CREATE UNIQUE INDEX nodes_by_kind_key ON nodes (kind, key)
            WHERE key IS NOT NULL AND state != 'superseded';
        CREATE INDEX nodes_by_fold_key ON nodes (fold_key) WHERE fold_key IS NOT NULL;",
        )
        .comment("superseded history: reasons, times and the same-content key of rules"),
        // Every source that supports a node: forgetting one removes only what no other
        // supports. `source` is its identity (origin, conversation, task, job); `provenance`
        // the full record, so a node can show its latest remaining source.
        M::up(
            "CREATE TABLE node_sources (
            node_id      TEXT    NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
            source       TEXT    NOT NULL,
            origin       TEXT    NOT NULL,
            session_id   TEXT,
            task_id      TEXT,
            job_id       TEXT,
            provider     TEXT,
            model        TEXT,
            commit_id    TEXT,
            provenance   TEXT    NOT NULL,
            recorded_ms  INTEGER NOT NULL,
            PRIMARY KEY (node_id, source)
        ) STRICT, WITHOUT ROWID;
        CREATE INDEX node_sources_by_session ON node_sources (session_id)
            WHERE session_id IS NOT NULL;
        CREATE INDEX node_sources_by_task ON node_sources (task_id) WHERE task_id IS NOT NULL;
        CREATE INDEX node_sources_by_job ON node_sources (job_id) WHERE job_id IS NOT NULL;
        CREATE INDEX node_sources_by_origin ON node_sources (origin);
        INSERT INTO node_sources (node_id, source, origin, session_id, task_id, job_id,
                                  provider, model, commit_id, provenance, recorded_ms)
        SELECT id,
               concat_ws('|', provenance ->> '$.origin',
                         coalesce(provenance ->> '$.sessionId', ''),
                         coalesce(provenance ->> '$.taskId', ''),
                         coalesce(provenance ->> '$.jobId', '')),
               provenance ->> '$.origin', provenance ->> '$.sessionId',
               provenance ->> '$.taskId', provenance ->> '$.jobId',
               provenance ->> '$.worker.provider', provenance ->> '$.worker.model',
               provenance ->> '$.commit', provenance,
               coalesce(provenance ->> '$.recordedAtMs', updated_ms)
        FROM nodes;",
        )
        .comment("the sources that support each node"),
        M::up(
            "CREATE TABLE node_source_files (
            node_id TEXT NOT NULL,
            source  TEXT NOT NULL,
            path    TEXT NOT NULL,
            hash    TEXT,
            PRIMARY KEY (node_id, source, path),
            FOREIGN KEY (node_id, source) REFERENCES node_sources (node_id, source)
                ON DELETE CASCADE
        ) STRICT, WITHOUT ROWID;
        INSERT INTO node_source_files (node_id, source, path, hash)
        SELECT f.node_id, s.source, f.path, f.hash FROM node_files f
        JOIN node_sources s ON s.node_id = f.node_id;
        DROP TABLE node_files;
        ALTER TABLE node_source_files RENAME TO node_files;
        CREATE INDEX node_files_by_path ON node_files (path);",
        )
        .comment("file evidence belongs to each supporting source"),
        // BUILD-PLAN A11: what a worker reported and what the thread kept on its own become
        // unverified claims, with their provenance kept; enrichment is unverified too; a
        // check that passed stays verified; the user's own decisions are untouched. A node a
        // user source supports shows that source. Nothing is deleted.
        M::up(
            "UPDATE nodes SET provenance = (
                SELECT s.provenance FROM node_sources s
                WHERE s.node_id = nodes.id AND s.origin = 'user'
                ORDER BY s.recorded_ms DESC LIMIT 1)
            WHERE provenance ->> '$.origin' != 'user'
              AND EXISTS (SELECT 1 FROM node_sources s
                          WHERE s.node_id = nodes.id AND s.origin = 'user');
            UPDATE nodes SET provenance = json_set(provenance, '$.claim',
                json(iif(key LIKE 'checks:%', '{\"evidence\":\"verified\",\"cites\":[]}',
                         '{\"evidence\":\"unverified\",\"cites\":[]}')))
            WHERE provenance ->> '$.claim' IS NULL
              AND (provenance ->> '$.origin' IN ('skeleton', 'enrichment')
                   OR (provenance ->> '$.origin' IN ('report', 'orchestrator')
                       AND kind IN ('decision', 'convention', 'contract')));
            UPDATE node_sources SET provenance = json_set(provenance, '$.claim',
                json(iif((SELECT key FROM nodes WHERE id = node_id) LIKE 'checks:%',
                         '{\"evidence\":\"verified\",\"cites\":[]}',
                         '{\"evidence\":\"unverified\",\"cites\":[]}')))
            WHERE provenance ->> '$.claim' IS NULL
              AND (origin IN ('skeleton', 'enrichment')
                   OR (origin IN ('report', 'orchestrator')
                       AND (SELECT kind FROM nodes WHERE id = node_id)
                           IN ('decision', 'convention', 'contract')));",
        )
        .comment("worker and thread rules and decisions become unverified claims"),
    ])
}

pub(crate) fn migrate(conn: &mut Connection) -> rusqlite_migration::Result<()> {
    migrations().to_latest(conn)
}

/// Pragmas for the writer's connection.
pub(crate) fn configure_writer(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Durable across application crashes in WAL mode; only an OS crash can drop the latest
    // commits.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    // Required: edges and file rows go with their node through `ON DELETE CASCADE`.
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "journal_size_limit", 8 * 1024 * 1024)?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

/// Pragmas for the read connections.
pub(crate) fn configure_reader(conn: &Connection) -> rusqlite::Result<()> {
    conn.pragma_update(None, "query_only", "ON")?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_file_evidence_migrates_to_supporting_sources() {
        let mut conn = Connection::open_in_memory().unwrap();
        configure_writer(&conn).unwrap();
        migrations().to_version(&mut conn, 3).unwrap();
        conn.execute_batch(
            "INSERT INTO nodes (id, kind, title, body, provenance, created_ms, updated_ms)
             VALUES ('n', 'convention', 'Rule', '', '{}', 1, 1);
             INSERT INTO node_sources (node_id, source, origin, provenance, recorded_ms)
             VALUES ('n', 's1', 'enrichment', '{}', 1), ('n', 's2', 'enrichment', '{}', 2);
             INSERT INTO node_files (node_id, path, hash) VALUES ('n', 'a.rs', 'h1');",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let count = |conn: &Connection| {
            conn.query_row("SELECT count(*) FROM node_files", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap()
        };
        assert_eq!(count(&conn), 2);
        conn.execute("DELETE FROM node_sources WHERE source = 's2'", [])
            .unwrap();
        assert_eq!(count(&conn), 1);
        assert!(
            !conn
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        conn.execute("DELETE FROM nodes", []).unwrap();
        assert_eq!(count(&conn), 0);
    }

    /// BUILD-PLAN A11's migration: report and thread rules become unverified claims with
    /// their provenance kept, enrichment is unverified, a passed check stays verified, the
    /// user's decisions are untouched (a user-supported node shows its user source), and
    /// nothing is deleted.
    #[test]
    fn worker_and_thread_rules_become_unverified_claims_and_user_decisions_stay() {
        use crate::{NodeKind, Provenance, source_label};

        let mut conn = Connection::open_in_memory().unwrap();
        configure_writer(&conn).unwrap();
        migrations().to_version(&mut conn, 4).unwrap();
        let rows = [
            // id, kind, key, origin, task, extra user source
            ("learned", "convention", None, "report", Some("t1"), false),
            ("chose", "decision", None, "report", Some("t1"), false),
            ("kept", "decision", None, "orchestrator", None, false),
            ("rule", "contract", None, "orchestrator", None, false),
            ("asked", "decision", None, "user", None, false),
            ("both", "decision", None, "orchestrator", None, true),
            (
                "module",
                "module",
                Some("module:a"),
                "enrichment",
                None,
                false,
            ),
            (
                "stack",
                "convention",
                Some("skeleton:stack"),
                "skeleton",
                None,
                false,
            ),
            (
                "checks",
                "convention",
                Some("checks:crates/a"),
                "report",
                Some("t2"),
                false,
            ),
            (
                "found",
                "report",
                Some("task:t1"),
                "report",
                Some("t1"),
                false,
            ),
            ("index", "module", Some("module:b"), "index", None, false),
        ];
        for (id, kind, key, origin, task, user) in rows {
            let provenance = serde_json::json!({
                "origin": origin, "sessionId": "s1", "taskId": task, "jobId": null,
                "worker": null, "commit": "abc", "recordedAtMs": 5,
            })
            .to_string();
            conn.execute(
                "INSERT INTO nodes (id, kind, key, title, body, provenance, session_id, \
                 created_ms, updated_ms) VALUES (?1, ?2, ?3, ?1, '', ?4, 's1', 1, 6)",
                rusqlite::params![id, kind, key, provenance],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO node_sources (node_id, source, origin, session_id, task_id, \
                 provenance, recorded_ms) VALUES (?1, ?2, ?2, 's1', ?3, ?4, 5)",
                rusqlite::params![id, origin, task, provenance],
            )
            .unwrap();
            if user {
                let said = serde_json::json!({
                    "origin": "user", "sessionId": "s1", "taskId": null, "jobId": null,
                    "worker": null, "commit": null, "recordedAtMs": 3,
                })
                .to_string();
                conn.execute(
                    "INSERT INTO node_sources (node_id, source, origin, session_id, \
                     provenance, recorded_ms) VALUES (?1, 'user', 'user', 's1', ?2, 3)",
                    rusqlite::params![id, said],
                )
                .unwrap();
            }
        }
        migrate(&mut conn).unwrap();
        let node = |id: &str| -> (NodeKind, Provenance) {
            conn.query_row(
                "SELECT kind, provenance FROM nodes WHERE id = ?1",
                [id],
                |row| {
                    let kind: String = row.get(0)?;
                    let provenance: String = row.get(1)?;
                    Ok((
                        serde_json::from_value(serde_json::Value::String(kind)).unwrap(),
                        serde_json::from_str(&provenance).unwrap(),
                    ))
                },
            )
            .unwrap()
        };
        let label = |id: &str| {
            let (kind, provenance) = node(id);
            source_label(kind, &provenance)
        };
        assert_eq!(label("learned"), "worker claim, unverified");
        assert_eq!(label("chose"), "worker claim, unverified");
        assert_eq!(label("kept"), "thread claim, unverified");
        assert_eq!(label("rule"), "thread claim, unverified");
        assert_eq!(label("asked"), "user decision");
        assert_eq!(label("both"), "user decision");
        assert_eq!(label("module"), "enrichment, unverified");
        assert_eq!(label("stack"), "enrichment, unverified");
        assert_eq!(label("checks"), "worker claim, verified");
        assert_eq!(label("found"), "worker report");
        assert_eq!(label("index"), "code index");
        // Provenance kept: the claim names its task, session and commit as before.
        let (_, learned) = node("learned");
        assert_eq!(
            (
                learned.task_id.as_deref(),
                learned.session_id.as_deref(),
                learned.commit.as_deref(),
                learned.recorded_at_ms
            ),
            (Some("t1"), Some("s1"), Some("abc"), 5)
        );
        assert!(learned.claim.is_some());
        // The user's decision is as it was: no claim.
        assert_eq!(node("asked").1.claim, None);
        // Each source of a claim says so too, and the user's source stays the user's.
        let source_claims: Vec<(String, String, Option<String>)> = conn
            .prepare(
                "SELECT node_id, origin, provenance ->> '$.claim.evidence' FROM node_sources \
                 ORDER BY node_id, origin",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let evidence = |id: &str, origin: &str| {
            source_claims
                .iter()
                .find(|(node, of, _)| node == id && of == origin)
                .and_then(|(_, _, evidence)| evidence.clone())
        };
        assert_eq!(evidence("learned", "report").as_deref(), Some("unverified"));
        assert_eq!(evidence("both", "user"), None);
        assert_eq!(evidence("asked", "user"), None);
        assert_eq!(evidence("checks", "report").as_deref(), Some("verified"));
        assert_eq!(evidence("found", "report"), None);
        // Nothing deleted.
        let nodes: i64 = conn
            .query_row("SELECT count(*) FROM nodes", [], |row| row.get(0))
            .unwrap();
        assert_eq!(nodes, rows.len() as i64);
        assert_eq!(source_claims.len(), rows.len() + 1);
    }
}
