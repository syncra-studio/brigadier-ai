//! The records index (STRIP-DOWN-PLAN S18, G14): what sessions already keep, searchable by
//! FTS5. Each session's CLI transcript across its generations (with the whole outputs its CLI
//! saved under `tool-results/`), its ledger, `ledger-archive.md` and hand-off notes. Brigadier
//! writes none of it; it only reads the files and indexes them, a file again when it changed.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use sha2::{Digest, Sha256};

use crate::{Cited, RecordEntry, RecordFile, RecordHit, RecordSource, Result, retrieve};

/// About how much of an entry a hit shows, around its first match.
const SNIPPET_BYTES: usize = 600;
/// Most of an entry's first line a hit carries apart from its snippet.
const LEAD_CHARS: i64 = 800;
/// Marks a match in `highlight()` output; control characters never occur in the text
/// tokens, so they can't be confused with it.
const OPEN: char = '\u{2}';
const CLOSE: char = '\u{3}';

/// What `file` was when it was last indexed (its size and time, as the reader stamps it).
pub(crate) fn stamp(conn: &Connection, path: &str) -> Result<Option<String>> {
    Ok(conn
        .prepare_cached("SELECT stamp FROM record_files WHERE path = ?1")?
        .query_row([path], |row| row.get(0))
        .optional()?)
}

/// `file` as it is now: its entries from the first one that differs from what was indexed
/// replace the rest, so a file that only grew adds its new entries, and a rewritten one is
/// indexed again from where it changed. A cited file's hash, once taken, stays with the entry
/// that cites it, here or after it moved to another file of the same session (a ledger entry
/// moved to the archive), so a later change still reads as stale.
pub(crate) fn sync(tx: &Transaction, file: &RecordFile, entries: &[RecordEntry]) -> Result<()> {
    let held: Vec<(i64, String)> = tx
        .prepare_cached(
            "SELECT position, fingerprint FROM records WHERE path = ?1 AND source = ?2 \
             ORDER BY position",
        )?
        .query_map(params![file.path, file.source.as_str()], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let prints: Vec<String> = entries.iter().map(fingerprint).collect();
    let same = held
        .iter()
        .zip(entries.iter().zip(&prints))
        .take_while(|((position, held), (entry, print))| {
            *position == entry.position && held == *print
        })
        .count();
    let from = entries.get(same).map(|entry| entry.position);
    let hashes = cited_hashes(tx, &file.conversation_id)?;
    if held.len() > same {
        let cut = held[same].0;
        tx.prepare_cached(
            "DELETE FROM records WHERE path = ?1 AND source = ?2 AND position >= ?3",
        )?
        .execute(params![file.path, file.source.as_str(), cut])?;
    }
    if from.is_some() {
        let mut insert = tx.prepare_cached(
            "INSERT INTO records (conversation_id, session, source, path, position, role, at_ms, \
             text, fingerprint, cited_path, cited_line, cited_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?;
        for (entry, print) in entries[same..].iter().zip(&prints[same..]) {
            let cited = entry.cited.as_ref();
            let hash = cited.and_then(|cited| {
                hashes
                    .get(&(entry.text.clone(), cited.path.clone()))
                    .cloned()
                    .or_else(|| cited.hash.clone())
            });
            insert.execute(params![
                file.conversation_id,
                file.session,
                file.source.as_str(),
                file.path,
                entry.position,
                entry.role,
                entry.at_ms,
                entry.text,
                print,
                cited.map(|cited| cited.path.as_str()),
                cited.and_then(|cited| cited.line),
                hash,
            ])?;
        }
    }
    tx.prepare_cached(
        "INSERT INTO record_files (path, conversation_id, stamp) VALUES (?1, ?2, ?3) \
         ON CONFLICT (path) DO UPDATE SET conversation_id = excluded.conversation_id, \
         stamp = excluded.stamp",
    )?
    .execute(params![file.path, file.conversation_id, file.stamp])?;
    Ok(())
}

/// The hash each of a session's entries took of the file it cites, by its text and that file.
fn cited_hashes(
    tx: &Transaction,
    conversation_id: &str,
) -> Result<HashMap<(String, String), String>> {
    Ok(tx
        .prepare_cached(
            "SELECT text, cited_path, cited_hash FROM records \
             WHERE conversation_id = ?1 AND cited_hash IS NOT NULL",
        )?
        .query_map([conversation_id], |row| {
            Ok(((row.get(0)?, row.get(1)?), row.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn fingerprint(entry: &RecordEntry) -> String {
    let mut hash = Sha256::new();
    for part in [
        entry.role.as_str(),
        entry.text.as_str(),
        entry.cited.as_ref().map_or("", |cited| cited.path.as_str()),
    ] {
        hash.update(part.as_bytes());
        hash.update([0]);
    }
    hash.update(entry.at_ms.unwrap_or(-1).to_le_bytes());
    hash.finalize()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Drops everything indexed from a conversation's records, its workers' too.
pub(crate) fn forget(tx: &Transaction, conversation_id: &str) -> Result<()> {
    tx.prepare_cached("DELETE FROM records WHERE conversation_id = ?1")?
        .execute([conversation_id])?;
    tx.prepare_cached("DELETE FROM record_files WHERE conversation_id = ?1")?
        .execute([conversation_id])?;
    Ok(())
}

/// The entries that best match `query`, best first (then the most recent): every session's,
/// or only `session`'s and its workers' (`<session>/…`).
pub(crate) fn search(
    conn: &Connection,
    query: &str,
    session: Option<&str>,
    limit: u32,
) -> Result<Vec<RecordHit>> {
    let Some(expression) = retrieve::fts_expression(query) else {
        return Ok(Vec::new());
    };
    let mut statement = conn.prepare_cached(
        "SELECT r.session, r.source, r.path, r.position, r.role, r.at_ms, \
                highlight(records_fts, 0, char(2), char(3)), \
                substr(r.text, 1, min(instr(r.text || char(10), char(10)) - 1, ?4)), \
                r.cited_path, r.cited_line, r.cited_hash \
         FROM records_fts JOIN records r ON r.rid = records_fts.rowid \
         WHERE records_fts MATCH ?1 \
           AND (?2 IS NULL OR r.session = ?2 OR substr(r.session, 1, length(?2) + 1) = ?2 || '/') \
         ORDER BY bm25(records_fts), r.at_ms DESC LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![expression, session, limit.clamp(1, 100), LEAD_CHARS],
        |row| {
            let source: String = row.get(1)?;
            let marked: String = row.get(6)?;
            let cited_path: Option<String> = row.get(8)?;
            Ok(RecordHit {
                session: row.get(0)?,
                source: RecordSource::parse(&source),
                path: row.get(2)?,
                position: row.get(3)?,
                role: row.get(4)?,
                at_ms: row.get(5)?,
                snippet: snippet(&marked),
                lead: row.get(7)?,
                cited: cited_path.map(|path| Cited {
                    path,
                    line: row.get(9).ok().flatten(),
                    hash: row.get(10).ok().flatten(),
                }),
            })
        },
    )?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// How many entries are indexed.
pub(crate) fn count(conn: &Connection) -> Result<u64> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))?;
    Ok(count.max(0) as u64)
}

/// About [`SNIPPET_BYTES`] of `marked` (text with its matches between [`OPEN`] and
/// [`CLOSE`]) around the first match, cut at word boundaries, without the marks.
fn snippet(marked: &str) -> String {
    let text: String = marked.chars().filter(|c| !is_mark(*c)).collect();
    // Where the first match starts in `text`: the marks before it are one byte each.
    let first = marked.find(OPEN).map_or(0, |at| {
        at - marked[..at].chars().filter(|c| is_mark(*c)).count()
    });
    if text.len() <= SNIPPET_BYTES {
        return text;
    }
    // A third of the room before the match, the rest after it.
    let start = first.saturating_sub(SNIPPET_BYTES / 3);
    let end = (start + SNIPPET_BYTES).min(text.len());
    let start = end.saturating_sub(SNIPPET_BYTES).min(start);
    let mut start = floor_char(&text, start);
    let mut end = floor_char(&text, end);
    if start > 0
        && let Some(space) = text[start..first.max(start)].find(char::is_whitespace)
    {
        start += space + 1;
    }
    if end < text.len()
        && let Some(space) = text[first.min(end)..end].rfind(char::is_whitespace)
    {
        end = first.min(end) + space;
    }
    let mut out = String::with_capacity(end - start + 8);
    if start > 0 {
        out.push('…');
    }
    out.push_str(text[start..end].trim());
    if end < text.len() {
        out.push('…');
    }
    out
}

fn is_mark(c: char) -> bool {
    c == OPEN || c == CLOSE
}

fn floor_char(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}
