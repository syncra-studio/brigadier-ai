//! The Brain's writes, each run in one transaction on the writer thread. Each returns the
//! changes to the in-memory vectors it made, applied once it commits.

use std::collections::{BTreeMap, HashSet};

use rusqlite::{OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::db::{self, edge_kind_str, kind_str};
use crate::vectors::Change;
use crate::{Error, FileRef, NewEdge, NewNode, NodeKind, Result, embed, format};

/// Edges a node's current fact needs, which a node replacing it takes over. The others
/// (`decidedIn`, `supersedes`) are its history and stay with it.
const CURRENT_EDGES: &str = "'contains', 'about', 'dependsOn', 'implements', 'consumes', 'relates'";

/// Kinds whose rewrites keep the old text as a superseded node: rules and choices, where what
/// held before still explains the code. Summaries of modules and files describe what is there
/// now; their old text is in the repository's history.
fn keeps_history(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Decision | NodeKind::Convention | NodeKind::Contract | NodeKind::Preference
    )
}

/// Text compared for sameness: lowercase; letters, digits and the symbols that change a
/// meaning (`c++` is not `c#`) kept; `.`, `-` and `/` kept between letters or digits (`v1.2`,
/// `src/lib`); anything else is a space, and runs of spaces are one.
fn normalize(text: &str) -> String {
    let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
    let joins = |at: usize| {
        at > 0
            && chars[at - 1].is_alphanumeric()
            && chars.get(at + 1).is_some_and(|next| next.is_alphanumeric())
    };
    let mut out = String::with_capacity(chars.len());
    let mut gap = false;
    for (at, &c) in chars.iter().enumerate() {
        let keep = c.is_alphanumeric()
            || "+#$%&*=<>@^~|\\_".contains(c)
            || (matches!(c, '.' | '-' | '/') && joins(at));
        if !keep {
            gap = true;
            continue;
        }
        if gap && !out.is_empty() {
            out.push(' ');
        }
        gap = false;
        out.push(c);
    }
    out
}

/// What makes two rules the same without a model: the same kind family, the same scope (a
/// decision belongs to the conversation it was made in; conventions and contracts to the
/// project) and the same title and body once [`normalize`]d. `None` for other kinds.
pub(crate) fn fold_key(
    kind: NodeKind,
    session: Option<&str>,
    title: &str,
    body: &str,
) -> Option<String> {
    let (family, scope) = match kind {
        NodeKind::Decision => ("decision", session.unwrap_or_default()),
        NodeKind::Convention | NodeKind::Contract => ("rule", "project"),
        _ => return None,
    };
    let mut hasher = Sha256::new();
    for part in [family, scope, &normalize(title), &normalize(body)] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    Some(
        hasher.finalize()[..16]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

/// The current node a new one updates: the one with its kind and key, else a rule with the
/// same content (`by_key` false).
struct Existing {
    id: String,
    title: String,
    body: String,
    stale_reason: Option<String>,
    by_key: bool,
}

fn existing(tx: &Transaction, node: &NewNode, fold: Option<&str>) -> Result<Option<Existing>> {
    let row = |by_key: bool| {
        move |row: &rusqlite::Row<'_>| {
            Ok(Existing {
                id: row.get(0)?,
                title: row.get(1)?,
                body: row.get(2)?,
                stale_reason: row.get(3)?,
                by_key,
            })
        }
    };
    if let Some(key) = &node.key {
        let found = tx
            .prepare_cached(
                "SELECT id, title, body, stale_reason FROM nodes \
                 WHERE kind = ?1 AND key = ?2 AND state != 'superseded'",
            )?
            .query_row(params![kind_str(node.kind), key], row(true))
            .optional()?;
        if found.is_some() {
            return Ok(found);
        }
        // A different keyed identity cannot be folded without an alias that survives
        // rewrites and revival. Keep keyed nodes distinct; unkeyed rules may still fold.
        return Ok(None);
    }
    let Some(fold) = fold else { return Ok(None) };
    Ok(tx
        .prepare_cached(
            "SELECT id, title, body, stale_reason FROM nodes \
             WHERE fold_key = ?1 AND state != 'superseded' \
             ORDER BY state = 'fresh' DESC, updated_ms DESC, rid DESC LIMIT 1",
        )?
        .query_row([fold], row(false))
        .optional()?)
}

pub(crate) fn record(
    tx: &Transaction,
    node: &NewNode,
    embedding: Option<Vec<u8>>,
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<String> {
    let provenance = serde_json::to_string(&node.provenance)
        .map_err(|err| Error::Invalid(format!("provenance: {err}")))?;
    let session = node.provenance.session_id.as_deref();
    let fold = fold_key(node.kind, session, &node.title, &node.body);
    let id = match existing(tx, node, fold.as_deref())? {
        // The same text again (or a rule already known in other words): it is confirmed, so
        // fresh, with this recording's provenance and its own supporting files.
        Some(found) if !found.by_key || (found.title == node.title && found.body == node.body) => {
            let embedding = embedding.filter(|_| found.by_key);
            // A claim that says what the user decided adds a source; the node stays the
            // user's (BUILD-PLAN A11).
            tx.prepare_cached(
                "UPDATE nodes SET state = 'fresh', stale_reason = NULL, stale_since_ms = NULL, \
                 provenance = iif(provenance ->> '$.origin' = 'user' \
                 AND ?2 ->> '$.origin' != 'user', provenance, ?2), \
                 session_id = ?3, updated_ms = ?4, expires_ms = ?5, \
                 embedding = coalesce(?6, embedding), \
                 embed_model = iif(?6 IS NULL, embed_model, ?7) WHERE id = ?1",
            )?
            .execute(params![
                found.id,
                provenance,
                session,
                now,
                node.expires_at_ms,
                embedding,
                embed::MODEL_ID
            ])?;
            if let Some(blob) = embedding {
                changes.push(Change::Set {
                    id: found.id.clone(),
                    kind: node.kind,
                    superseded: false,
                    blob,
                });
            }
            add_source(tx, &found.id, node, &provenance)?;
            found.id
        }
        Some(found) => {
            if keeps_history(node.kind) {
                let mut reason = format!(
                    "rewritten by {}",
                    format::origin_label(node.provenance.origin)
                );
                if let Some(stale) = &found.stale_reason {
                    reason.push_str(&format!("; it was stale: {stale}"));
                }
                archive(tx, &found.id, &reason, now, changes)?;
            }
            let model = embedding.as_ref().map(|_| embed::MODEL_ID);
            tx.prepare_cached(
                "UPDATE nodes SET title = ?2, body = ?3, state = 'fresh', stale_reason = NULL, \
                 stale_since_ms = NULL, superseded_by = NULL, provenance = ?4, \
                 session_id = ?5, updated_ms = ?6, expires_ms = ?7, embedding = ?8, \
                 embed_model = ?9, fold_key = ?10 WHERE id = ?1",
            )?
            .execute(params![
                found.id,
                node.title,
                node.body,
                provenance,
                session,
                now,
                node.expires_at_ms,
                embedding,
                model,
                fold
            ])?;
            changes.push(match embedding {
                Some(blob) => Change::Set {
                    id: found.id.clone(),
                    kind: node.kind,
                    superseded: false,
                    blob,
                },
                None => Change::Remove(found.id.clone()),
            });
            // Only this recording supports the new text; the earlier sources stay with the
            // earlier version.
            tx.prepare_cached("DELETE FROM node_sources WHERE node_id = ?1")?
                .execute([&found.id])?;
            add_source(tx, &found.id, node, &provenance)?;
            found.id
        }
        None => {
            let id = uuid::Uuid::now_v7().to_string();
            let model = embedding.as_ref().map(|_| embed::MODEL_ID);
            tx.prepare_cached(
                "INSERT INTO nodes (id, kind, key, title, body, provenance, session_id, \
                 created_ms, updated_ms, expires_ms, embedding, embed_model, fold_key) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?11, ?12)",
            )?
            .execute(params![
                id,
                kind_str(node.kind),
                node.key,
                node.title,
                node.body,
                provenance,
                session,
                now,
                node.expires_at_ms,
                embedding,
                model,
                fold
            ])?;
            changes.push(match embedding {
                Some(blob) => Change::Set {
                    id: id.clone(),
                    kind: node.kind,
                    superseded: false,
                    blob,
                },
                None => Change::Remove(id.clone()),
            });
            add_source(tx, &id, node, &provenance)?;
            id
        }
    };
    Ok(id)
}

/// Adds (or renews) `node`'s provenance as one of `id`'s sources.
fn add_source(tx: &Transaction, id: &str, node: &NewNode, json: &str) -> Result<()> {
    let provenance = &node.provenance;
    let origin = crate::origin_str(&provenance.origin)?;
    let source = [
        origin.as_str(),
        provenance.session_id.as_deref().unwrap_or_default(),
        provenance.task_id.as_deref().unwrap_or_default(),
        provenance.job_id.as_deref().unwrap_or_default(),
    ]
    .join("|");
    let worker = provenance.worker.as_ref();
    tx.prepare_cached(
        "INSERT OR REPLACE INTO node_sources (node_id, source, origin, session_id, task_id, \
         job_id, provider, model, commit_id, provenance, recorded_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
    )?
    .execute(params![
        id,
        source,
        origin,
        provenance.session_id,
        provenance.task_id,
        provenance.job_id,
        worker.map(|worker| worker.provider.as_str()),
        worker.and_then(|worker| worker.model.as_deref()),
        provenance.commit,
        json,
        provenance.recorded_at_ms
    ])?;
    // Renewing a source replaces only its evidence (the old rows cascade on REPLACE).
    // A path listed twice keeps its last hash.
    let files: BTreeMap<&str, Option<&str>> = node
        .files
        .iter()
        .map(|file| (file.path.as_str(), file.hash.as_deref()))
        .collect();
    let mut insert = tx.prepare_cached(
        "INSERT INTO node_files (node_id, source, path, hash) VALUES (?1, ?2, ?3, ?4)",
    )?;
    for (path, hash) in files {
        insert.execute(params![id, source, path, hash])?;
    }
    Ok(())
}

/// Keeps `id` as it stands now as a superseded copy, before `id` is rewritten in place: the
/// copy has its text, files, embedding and history edges, and the older versions hang off it,
/// so the history stays one line. `id` keeps its id, so links to it stay valid.
fn archive(
    tx: &Transaction,
    id: &str,
    reason: &str,
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<String> {
    let copy = uuid::Uuid::now_v7().to_string();
    tx.prepare_cached(
        "INSERT INTO nodes (id, kind, key, title, body, state, stale_reason, stale_since_ms, \
         superseded_by, superseded_reason, superseded_ms, provenance, session_id, created_ms, \
         updated_ms, expires_ms, embedding, embed_model, fold_key) \
         SELECT ?2, kind, key, title, body, 'superseded', stale_reason, stale_since_ms, id, ?3, \
         ?4, provenance, session_id, created_ms, updated_ms, expires_ms, embedding, embed_model, \
         fold_key FROM nodes WHERE id = ?1",
    )?
    .execute(params![id, copy, reason, now])?;
    tx.prepare_cached(
        "INSERT OR IGNORE INTO edges (from_id, to_id, kind) \
         SELECT ?2, to_id, kind FROM edges WHERE from_id = ?1 AND kind = 'decidedIn'",
    )?
    .execute(params![id, copy])?;
    tx.prepare_cached(
        "INSERT INTO node_sources (node_id, source, origin, session_id, task_id, job_id, \
         provider, model, commit_id, provenance, recorded_ms) \
         SELECT ?2, source, origin, session_id, task_id, job_id, provider, model, commit_id, \
        provenance, recorded_ms FROM node_sources WHERE node_id = ?1",
    )?
    .execute(params![id, copy])?;
    tx.prepare_cached(
        "INSERT INTO node_files (node_id, source, path, hash) \
         SELECT ?2, source, path, hash FROM node_files WHERE node_id = ?1",
    )?
    .execute(params![id, copy])?;
    tx.prepare_cached("UPDATE nodes SET superseded_by = ?2 WHERE superseded_by = ?1 AND id != ?2")?
        .execute(params![id, copy])?;
    tx.prepare_cached("UPDATE edges SET from_id = ?2 WHERE from_id = ?1 AND kind = 'supersedes'")?
        .execute(params![id, copy])?;
    tx.prepare_cached("INSERT INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'supersedes')")?
        .execute(params![id, copy])?;
    let stored = tx
        .prepare_cached(
            "SELECT kind, embedding FROM nodes \
             WHERE id = ?1 AND embedding IS NOT NULL AND embed_model = ?2",
        )?
        .query_row(params![copy, embed::MODEL_ID], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .optional()?;
    if let Some((kind, blob)) = stored
        && let Some(kind) = db::parse_kind(&kind)
    {
        changes.push(Change::Set {
            id: copy.clone(),
            kind,
            superseded: true,
            blob,
        });
    }
    Ok(copy)
}

pub(crate) fn link(tx: &Transaction, edges: &[NewEdge]) -> Result<()> {
    let mut insert = tx
        .prepare_cached("INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, ?3)")?;
    for edge in edges {
        let from = db::resolve(tx, &edge.from)?;
        let to = db::resolve(tx, &edge.to)?;
        if from == to {
            return Err(Error::Invalid(format!(
                "a node can't link to itself ({from})"
            )));
        }
        insert.execute(params![from, to, edge_kind_str(edge.kind)])?;
    }
    Ok(())
}

/// Replaces the structural edges (`contains`, `dependsOn`) from each of `sources` to modules
/// and services with `edges`. Their other edges (a module's file summaries) stay.
pub(crate) fn relink_structure(
    tx: &Transaction,
    sources: &[String],
    edges: &[NewEdge],
) -> Result<()> {
    let mut clear = tx.prepare_cached(
        "DELETE FROM edges WHERE from_id = ?1 AND kind IN (?2, ?3) \
         AND to_id IN (SELECT id FROM nodes WHERE kind IN (?4, ?5))",
    )?;
    for source in sources {
        let from = db::resolve(tx, source)?;
        clear.execute(params![
            from,
            edge_kind_str(crate::EdgeKind::Contains),
            edge_kind_str(crate::EdgeKind::DependsOn),
            kind_str(crate::NodeKind::Module),
            kind_str(crate::NodeKind::Service)
        ])?;
    }
    link(tx, edges)
}

pub(crate) fn supersede(
    tx: &Transaction,
    old: &str,
    new: &str,
    reason: &str,
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<()> {
    let old = db::resolve(tx, old)?;
    let new = db::resolve(tx, new)?;
    if old == new {
        return Err(Error::Invalid(format!("{old} can't supersede itself")));
    }
    // A chain that loops would leave no current version of either.
    let mut next = tx.prepare_cached("SELECT superseded_by FROM nodes WHERE id = ?1")?;
    let (mut at, mut seen) = (new.clone(), std::collections::HashSet::new());
    while let Some(by) = next
        .query_row([&at], |row| row.get::<_, Option<String>>(0))
        .optional()?
        .flatten()
        .filter(|by| seen.insert(by.clone()))
    {
        if by == old {
            return Err(Error::Invalid(format!(
                "{new} is already superseded by {old}, directly or through later versions"
            )));
        }
        at = by;
    }
    // Its stale reason stays: if it is ever current again, it is as stale as it was.
    tx.prepare_cached(
        "UPDATE nodes SET state = 'superseded', superseded_by = ?2, superseded_reason = ?3, \
         superseded_ms = ?4 WHERE id = ?1",
    )?
    .execute(params![old, new, reason, now])?;
    copy_current_edges(tx, &old, &new)?;
    tx.prepare_cached("INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, ?3)")?
        .execute(params![
            new,
            old,
            edge_kind_str(crate::EdgeKind::Supersedes)
        ])?;
    changes.push(Change::Superseded {
        id: old,
        superseded: true,
    });
    Ok(())
}

/// Gives `to` the edges `from`'s current fact has ([`CURRENT_EDGES`]), either way.
fn copy_current_edges(tx: &Transaction, from: &str, to: &str) -> Result<()> {
    tx.prepare_cached(&format!(
        "INSERT OR IGNORE INTO edges (from_id, to_id, kind) \
         SELECT ?2, to_id, kind FROM edges \
         WHERE from_id = ?1 AND to_id != ?2 AND kind IN ({CURRENT_EDGES})"
    ))?
    .execute(params![from, to])?;
    tx.prepare_cached(&format!(
        "INSERT OR IGNORE INTO edges (from_id, to_id, kind) \
         SELECT from_id, ?2, kind FROM edges \
         WHERE to_id = ?1 AND from_id != ?2 AND kind IN ({CURRENT_EDGES})"
    ))?
    .execute(params![from, to])?;
    Ok(())
}

/// Hangs `children` (superseded versions) off `parent`.
fn repoint(tx: &Transaction, children: &[String], parent: &str) -> Result<()> {
    let mut update = tx.prepare_cached("UPDATE nodes SET superseded_by = ?2 WHERE id = ?1")?;
    let mut link = tx.prepare_cached(
        "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, 'supersedes')",
    )?;
    for child in children {
        update.execute(params![child, parent])?;
        link.execute(params![parent, child])?;
    }
    Ok(())
}

/// Before `doomed` are deleted: the history they hold moves up, and where a current node goes,
/// its latest earlier version is current again (unless another node holds its key by now),
/// with the freshness it had.
fn revive(tx: &Transaction, doomed: &[String], now: i64, changes: &mut Vec<Change>) -> Result<()> {
    let gone: HashSet<&str> = doomed.iter().map(String::as_str).collect();
    let mut state = tx.prepare_cached("SELECT state, superseded_by FROM nodes WHERE id = ?1")?;
    let mut children = tx.prepare_cached(
        "SELECT id, kind, key FROM nodes WHERE superseded_by = ?1 AND state = 'superseded' \
         ORDER BY superseded_ms IS NULL, superseded_ms DESC, rid DESC",
    )?;
    let mut current = Vec::new();
    for id in doomed {
        let Some((state, by)) = state
            .query_row([id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .optional()?
        else {
            continue;
        };
        match by.filter(|_| state == "superseded") {
            // Its earlier versions now hang off what replaced it (read again each time: that
            // may have moved too).
            Some(by) => {
                let below: Vec<String> = children
                    .query_map([id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<_>>()?;
                repoint(tx, &below, &by)?;
            }
            None => current.push(id),
        }
    }
    // The keys of current nodes going away are free for their earlier versions.
    let mut unkey = tx.prepare_cached("UPDATE nodes SET key = NULL WHERE id = ?1")?;
    for id in &current {
        unkey.execute([id])?;
    }
    let mut holder = tx.prepare_cached(
        "SELECT id FROM nodes WHERE kind = ?1 AND key = ?2 AND state != 'superseded'",
    )?;
    let mut restore = tx.prepare_cached(
        "UPDATE nodes SET superseded_by = NULL, superseded_reason = NULL, superseded_ms = NULL, \
         state = CASE WHEN stale_reason IS NOT NULL THEN 'stale' \
                      WHEN expires_ms IS NOT NULL AND expires_ms <= ?2 THEN 'stale' \
                      ELSE 'fresh' END, \
         stale_since_ms = CASE WHEN stale_reason IS NOT NULL THEN coalesce(stale_since_ms, ?2) \
                               WHEN expires_ms IS NOT NULL AND expires_ms <= ?2 THEN ?2 END, \
         stale_reason = CASE WHEN stale_reason IS NOT NULL THEN stale_reason \
                             WHEN expires_ms IS NOT NULL AND expires_ms <= ?2 \
                             THEN 'older than its TTL' END \
         WHERE id = ?1",
    )?;
    for id in current {
        let versions: Vec<(String, String, Option<String>)> = children
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<(String, String, Option<String>)>>>()?
            .into_iter()
            .filter(|(child, _, _)| !gone.contains(child.as_str()))
            .collect();
        let Some((latest, kind, key)) = versions.first() else {
            continue;
        };
        let held = match key {
            Some(key) => holder
                .query_map(params![kind, key], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .find(|other| !gone.contains(other.as_str())),
            None => None,
        };
        let ids: Vec<String> = versions.iter().map(|(child, _, _)| child.clone()).collect();
        match held {
            Some(other) => repoint(tx, &ids, &other)?,
            None => {
                restore.execute(params![latest, now])?;
                copy_current_edges(tx, id, latest)?;
                repoint(tx, &ids[1..], latest)?;
                changes.push(Change::Superseded {
                    id: latest.clone(),
                    superseded: false,
                });
            }
        }
    }
    Ok(())
}

pub(crate) fn delete(
    tx: &Transaction,
    id: &str,
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<()> {
    let ids = [id.to_owned()];
    revive(tx, &ids, now, changes)?;
    if tx
        .prepare_cached("DELETE FROM nodes WHERE id = ?1")?
        .execute([id])?
        == 0
    {
        return Err(Error::NotFound(format!("no node has the id {id}")));
    }
    changes.push(Change::Remove(id.to_owned()));
    Ok(())
}

/// Which sources to forget.
pub(crate) enum Sources<'a> {
    Session(&'a str),
    Origin(&'a str),
}

/// Takes away the support of `sources`: a node no other source supports goes (its latest
/// earlier version may be current again), the others show their latest remaining source.
/// Returns how many nodes went.
pub(crate) fn forget(
    tx: &Transaction,
    sources: &Sources<'_>,
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<u64> {
    let (column, value) = match sources {
        Sources::Session(session) => ("session_id", *session),
        Sources::Origin(origin) => ("origin", *origin),
    };
    let affected: Vec<String> = tx
        .prepare_cached(&format!(
            "SELECT DISTINCT node_id FROM node_sources WHERE {column} = ?1"
        ))?
        .query_map([value], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    tx.prepare_cached(&format!("DELETE FROM node_sources WHERE {column} = ?1"))?
        .execute([value])?;
    drop_unsupported(tx, &affected, now, changes)
}

/// Of `affected` (nodes that just lost a source), deletes those no source supports any more
/// and gives the others the provenance of their latest remaining source. Returns how many
/// went.
fn drop_unsupported(
    tx: &Transaction,
    affected: &[String],
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<u64> {
    let mut supported =
        tx.prepare_cached("SELECT EXISTS (SELECT 1 FROM node_sources WHERE node_id = ?1)")?;
    let mut doomed = Vec::new();
    let mut kept = Vec::new();
    for id in affected {
        if supported.query_row([id], |row| row.get::<_, bool>(0))? {
            kept.push(id);
        } else {
            doomed.push(id.clone());
        }
    }
    let mut restore = tx.prepare_cached(
        "UPDATE nodes SET (provenance, session_id) = ( \
             SELECT provenance, session_id FROM node_sources WHERE node_id = ?1 \
             ORDER BY recorded_ms DESC, source DESC LIMIT 1) \
         WHERE id = ?1",
    )?;
    for id in kept {
        restore.execute([id])?;
    }
    revive(tx, &doomed, now, changes)?;
    let mut delete = tx.prepare_cached("DELETE FROM nodes WHERE id = ?1")?;
    for id in &doomed {
        delete.execute([id])?;
    }
    // A node revived above but deleted too is simply gone.
    changes.extend(doomed.iter().cloned().map(Change::Remove));
    Ok(doomed.len() as u64)
}

/// After a task's nodes were recorded again (`recorded`): current nodes it supported before
/// but didn't record now lose its support, and go if nothing else supports them. Returns how
/// many went.
pub(crate) fn release_task(
    tx: &Transaction,
    task: &str,
    recorded: &[String],
    now: i64,
    changes: &mut Vec<Change>,
) -> Result<u64> {
    let recorded: HashSet<&str> = recorded.iter().map(String::as_str).collect();
    let dropped: Vec<String> = tx
        .prepare_cached(
            "SELECT DISTINCT s.node_id FROM node_sources s JOIN nodes n ON n.id = s.node_id \
             WHERE s.task_id = ?1 AND n.state != 'superseded'",
        )?
        .query_map([task], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .filter(|id| !recorded.contains(id.as_str()))
        .collect();
    let mut release =
        tx.prepare_cached("DELETE FROM node_sources WHERE node_id = ?1 AND task_id = ?2")?;
    for id in &dropped {
        release.execute(params![id, task])?;
    }
    drop_unsupported(tx, &dropped, now, changes)
}

pub(crate) fn files_changed(
    tx: &Transaction,
    changes: &[FileRef],
    now: i64,
) -> Result<Vec<String>> {
    let mut recorded = tx.prepare_cached(
        "SELECT n.id, f.hash FROM node_files f JOIN nodes n ON n.id = f.node_id \
         WHERE f.path = ?1 AND f.hash IS NOT NULL AND n.state != 'stale'",
    )?;
    let mut mark = tx.prepare_cached(
        "UPDATE nodes SET state = 'stale', stale_reason = ?2, stale_since_ms = ?3 \
         WHERE id = ?1 AND state = 'fresh'",
    )?;
    // An earlier version goes stale quietly: it stays history, and is stale if it is ever
    // current again.
    let mut mark_history = tx.prepare_cached(
        "UPDATE nodes SET stale_reason = ?2, stale_since_ms = ?3 \
         WHERE id = ?1 AND state = 'superseded' AND stale_reason IS NULL",
    )?;
    let mut stale = Vec::new();
    for change in changes {
        let rows = recorded
            .query_map([&change.path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let reason = match change.hash {
            Some(_) => format!("{} changed", change.path),
            None => format!("{} was deleted", change.path),
        };
        for (id, hash) in rows {
            if change.hash.as_deref() == Some(hash.as_str()) {
                continue;
            }
            if mark.execute(params![id, reason, now])? > 0 {
                stale.push(id);
            } else {
                mark_history.execute(params![id, reason, now])?;
            }
        }
    }
    Ok(stale)
}

pub(crate) fn expire(tx: &Transaction, now: i64) -> Result<u64> {
    let expired = tx
        .prepare_cached(
            "UPDATE nodes SET state = 'stale', stale_reason = 'older than its TTL', \
             stale_since_ms = ?1 WHERE expires_ms IS NOT NULL AND expires_ms <= ?1 \
             AND state = 'fresh'",
        )?
        .execute([now])?;
    Ok(expired as u64)
}

/// A node's text as it was read for embedding, and its embedding.
pub(crate) struct Embedded {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) blob: Vec<u8>,
}

/// Stores embeddings; a node whose text changed since it was read keeps what its own write
/// gave it. Returns how many were stored.
pub(crate) fn set_embeddings(
    tx: &Transaction,
    embedded: Vec<Embedded>,
    changes: &mut Vec<Change>,
) -> Result<u32> {
    let mut update = tx.prepare_cached(
        "UPDATE nodes SET embedding = ?4, embed_model = ?5 \
         WHERE id = ?1 AND title = ?2 AND body = ?3 \
         RETURNING kind, state = 'superseded'",
    )?;
    let mut stored = 0;
    for node in embedded {
        let updated = update
            .query_row(
                params![node.id, node.title, node.body, node.blob, embed::MODEL_ID],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
            )
            .optional()?;
        let Some((kind, superseded)) = updated else {
            continue;
        };
        stored += 1;
        if let Some(kind) = db::parse_kind(&kind) {
            changes.push(Change::Set {
                id: node.id,
                kind,
                superseded,
                blob: node.blob,
            });
        }
    }
    Ok(stored)
}

/// Gives decisions, conventions and contracts recorded before same-content keys existed their
/// [`fold_key`], then folds unkeyed current rules that say the same thing. Prefer a keyed
/// keeper, then the fresh one (else the newest). Distinct keyed identities stay current.
/// Returns how many were folded.
pub(crate) fn settle_rules(tx: &Transaction, now: i64, changes: &mut Vec<Change>) -> Result<u64> {
    let unkeyed: Vec<(String, String, Option<String>, String, String)> = tx
        .prepare_cached(
            "SELECT id, kind, session_id, title, body FROM nodes \
             WHERE fold_key IS NULL AND kind IN ('decision', 'convention', 'contract')",
        )?
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if unkeyed.is_empty() {
        return Ok(0);
    }
    let mut set = tx.prepare_cached("UPDATE nodes SET fold_key = ?2 WHERE id = ?1")?;
    for (id, kind, session, title, body) in &unkeyed {
        let fold =
            db::parse_kind(kind).and_then(|kind| fold_key(kind, session.as_deref(), title, body));
        set.execute(params![id, fold])?;
    }
    let groups: Vec<String> = tx
        .prepare_cached(
            "SELECT fold_key FROM nodes WHERE fold_key IS NOT NULL AND state != 'superseded' \
             GROUP BY fold_key HAVING COUNT(*) > 1",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut members = tx.prepare_cached(
        "SELECT id, key FROM nodes WHERE fold_key = ?1 AND state != 'superseded' \
         ORDER BY key IS NOT NULL DESC, state = 'fresh' DESC, updated_ms DESC, rid DESC",
    )?;
    let mut folded = 0;
    for group in groups {
        let ids: Vec<(String, Option<String>)> = members
            .query_map([group], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let Some(((keeper, _), rest)) = ids.split_first() else {
            continue;
        };
        for (id, key) in rest {
            if key.is_some() {
                continue;
            }
            supersede(
                tx,
                id,
                keeper,
                "the same rule as a newer node",
                now,
                changes,
            )?;
            folded += 1;
        }
    }
    Ok(folded)
}
