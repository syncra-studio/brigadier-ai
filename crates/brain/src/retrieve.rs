//! Hybrid retrieval: FTS5 bm25 and embedding cosine, fused by reciprocal rank, then one hop
//! along the graph to the decisions, conventions and contracts behind what matched.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params_from_iter};

use crate::db::{self, kind_str};
use crate::vectors::Vectors;
use crate::{BrainCaps, BrainHit, BrainQuery, Node, NodeKind, NodeState, Result};

/// Each ranking contributes this many candidates.
const CANDIDATES: usize = 50;
/// Reciprocal-rank fusion's constant: `score = Σ 1 / (K + rank)`.
const RRF_K: f64 = 60.0;
/// A hit brings at most this many linked nodes (the most recently updated).
const LINKED_PER_HIT: i64 = 4;
/// How many query words take part; the rest of a long question adds little.
const MAX_TERMS: usize = 24;
/// Title matches count this much more than body matches.
const TITLE_WEIGHT: f64 = 4.0;
/// A stale node's score is cut to this share, so a fresh match on the same subject leads.
const STALE_FACTOR: f64 = 0.5;

/// Words too common to help, dropped from a query that has others.
const STOP_WORDS: &[&str] = &[
    "a", "about", "an", "and", "are", "as", "at", "be", "by", "can", "do", "does", "for", "from",
    "how", "i", "in", "is", "it", "of", "on", "or", "should", "that", "the", "this", "to", "was",
    "we", "what", "when", "where", "which", "who", "why", "with",
];

/// A safe FTS5 expression for free text: its words, quoted and OR-ed, the last one as a
/// prefix. User syntax (quotes, operators, column filters) never reaches FTS5. `None` when the
/// text has no words.
pub(crate) fn fts_expression(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    let useful: Vec<&String> = words
        .iter()
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .collect();
    let chosen = if useful.is_empty() {
        words.iter().collect()
    } else {
        useful
    };
    let mut seen = HashSet::new();
    let terms: Vec<&String> = chosen
        .into_iter()
        .filter(|word| seen.insert(word.as_str()))
        .take(MAX_TERMS)
        .collect();
    let (last, rest) = terms.split_last()?;
    let mut expression = String::new();
    for term in rest {
        expression.push('"');
        expression.push_str(term);
        expression.push_str("\" OR ");
    }
    expression.push('"');
    expression.push_str(last);
    expression.push_str("\"*");
    Some(expression)
}

/// Node kinds whose hits bring their linked decisions, conventions and contracts.
fn expands(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Module
            | NodeKind::Service
            | NodeKind::FileSummary
            | NodeKind::Report
            | NodeKind::Task
    )
}

/// Kinds whose earlier versions are kept as history.
fn has_history(kind: NodeKind) -> bool {
    matches!(
        kind,
        NodeKind::Decision
            | NodeKind::Convention
            | NodeKind::Contract
            | NodeKind::Preference
            | NodeKind::Constraint
    )
}

/// Superseded nodes match only when history is asked for, or as earlier versions of a rule: a
/// match on an old decision brings the one that replaced it.
fn candidate(kind: NodeKind, superseded: bool, history: bool) -> bool {
    !superseded || history || has_history(kind)
}

/// `score`, cut for a stale node.
fn weighed(node: &Node, score: f64) -> f64 {
    match node.state {
        NodeState::Stale { .. } => score * STALE_FACTOR,
        _ => score,
    }
}

/// The ranked hits for `query`, best first, at most `limit`.
pub(crate) fn hits(
    conn: &Connection,
    vectors: &Vectors,
    query: &BrainQuery,
    embedding: Option<&[f32]>,
    limit: usize,
) -> Result<Vec<BrainHit>> {
    let allowed = |kind: NodeKind| query.kinds.is_empty() || query.kinds.contains(&kind);

    let mut fused: HashMap<String, f64> = HashMap::new();
    let mut add = |ranked: &[String], keep: &dyn Fn(&str) -> bool| {
        for (rank, id) in ranked.iter().enumerate() {
            if keep(id) {
                *fused.entry(id.clone()).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
            }
        }
    };
    let text_ranked = match fts_expression(&query.text) {
        Some(expression) => full_text(conn, &expression, &query.kinds, query.history)?,
        None => Vec::new(),
    };
    add(&text_ranked, &|_| true);
    if let Some(embedding) = embedding {
        let nearest: Vec<String> = vectors
            .nearest(conn, embedding, CANDIDATES, |kind, superseded| {
                allowed(kind) && candidate(kind, superseded, query.history)
            })?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        add(&nearest, &|_| true);
        // A node not embedded yet (learned while the model was unloaded) can't be in the
        // semantic ranking: its full-text rank stands in, so new knowledge isn't outranked by
        // older nodes for being new.
        add(&text_ranked, &|id| !vectors.contains(id));
    }
    // Every candidate is weighed before the cut, so a stale node can't take a fresh one's
    // place.
    let ids: Vec<String> = fused.keys().cloned().collect();
    let mut nodes: HashMap<String, Node> = db::nodes_by_id(conn, &ids)?
        .into_iter()
        .map(|node| (node.id.clone(), node))
        .collect();
    let mut ranked: Vec<(String, f64)> = fused
        .into_iter()
        .filter_map(|(id, score)| {
            let score = weighed(nodes.get(&id)?, score);
            Some((id, score))
        })
        .collect();
    ranked.sort_unstable_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    // Keep all candidates until history has resolved to current nodes and duplicate ids
    // have collapsed. Several matching revisions must use only one result slot.

    // Direct hits first, then what they bring, each just below its source.
    let mut scored: HashMap<String, (f64, bool)> = HashMap::new();
    let mut brought: Vec<(String, f64)> = Vec::new();
    for (id, score) in &ranked {
        let Some(node) = nodes.get(id) else { continue };
        match &node.state {
            NodeState::Superseded { .. } => {
                if query.history {
                    scored.insert(id.clone(), (*score, false));
                }
                if let Some(current) = current_version(conn, id)?
                    && current != *id
                {
                    brought.push((current, score * 0.999));
                }
            }
            _ => {
                scored.insert(id.clone(), (*score, false));
                if expands(node.kind) {
                    for (index, linked) in linked_rules(conn, id)?.into_iter().enumerate() {
                        brought.push((linked, score * (1.0 - 0.001 * (index as f64 + 1.0))));
                    }
                }
            }
        }
    }
    let missing: Vec<String> = brought
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| !nodes.contains_key(id))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    for node in db::nodes_by_id(conn, &missing)? {
        nodes.insert(node.id.clone(), node);
    }
    for (id, score) in brought {
        let Some(node) = nodes.get(&id) else { continue };
        if !allowed(node.kind) || matches!(node.state, NodeState::Superseded { .. }) {
            continue;
        }
        let score = weighed(node, score);
        let entry = scored.entry(id).or_insert((score, true));
        if entry.1 && entry.0 < score {
            entry.0 = score;
        }
    }

    let mut hits: Vec<BrainHit> = scored
        .into_iter()
        .filter_map(|(id, (score, linked))| {
            nodes.remove(&id).map(|node| BrainHit {
                node,
                score,
                linked,
            })
        })
        .collect();
    hits.sort_unstable_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.node.id.cmp(&b.node.id))
    });
    hits.truncate(limit);
    Ok(hits)
}

/// The FTS5 top candidates for `expression`, best first.
fn full_text(
    conn: &Connection,
    expression: &str,
    kinds: &[NodeKind],
    history: bool,
) -> Result<Vec<String>> {
    let mut sql = String::from(
        "SELECT n.id FROM nodes_fts JOIN nodes n ON n.rid = nodes_fts.rowid \
         WHERE nodes_fts MATCH ?",
    );
    if !history {
        sql.push_str(
            " AND (n.state != 'superseded' \
             OR n.kind IN ('decision', 'convention', 'contract', 'preference'))",
        );
    }
    let mut args: Vec<&str> = vec![expression];
    if !kinds.is_empty() {
        sql.push_str(&format!(
            " AND n.kind IN ({})",
            db::placeholders(kinds.len())
        ));
        args.extend(kinds.iter().map(|kind| kind_str(*kind)));
    }
    sql.push_str(&format!(
        " ORDER BY bm25(nodes_fts, {TITLE_WEIGHT}, 1.0) LIMIT {CANDIDATES}"
    ));
    let mut statement = conn.prepare_cached(&sql)?;
    let rows = statement.query_map(params_from_iter(args), |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The current decisions, conventions and contracts linked to `id` either way, newest first.
fn linked_rules(conn: &Connection, id: &str) -> Result<Vec<String>> {
    let mut statement = conn.prepare_cached(
        "SELECT n.id FROM nodes n WHERE n.id IN ( \
             SELECT to_id FROM edges WHERE from_id = ?1 \
             UNION SELECT from_id FROM edges WHERE to_id = ?1) \
         AND n.kind IN ('decision', 'convention', 'contract') AND n.state != 'superseded' \
         ORDER BY n.updated_ms DESC LIMIT ?2",
    )?;
    let rows = statement.query_map(rusqlite::params![id, LINKED_PER_HIT], |row| {
        row.get::<_, String>(0)
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The end of `id`'s chain of replacements: the node that supersedes it, or the one that
/// supersedes that, and so on.
fn current_version(conn: &Connection, id: &str) -> Result<Option<String>> {
    let mut statement =
        conn.prepare_cached("SELECT state, superseded_by FROM nodes WHERE id = ?1")?;
    let mut current = id.to_owned();
    // A chain longer than this is a cycle.
    for _ in 0..32 {
        let row = statement
            .query_row([&current], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .optional()?;
        match row {
            Some((state, Some(by))) if state == "superseded" => current = by,
            Some(_) => return Ok(Some(current)),
            None => return Ok(None),
        }
    }
    Ok(None)
}

/// The kinds of result an answer caps separately.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Family {
    Facts,
    Entities,
    Passages,
}

fn family(kind: NodeKind) -> Family {
    match kind {
        NodeKind::Decision
        | NodeKind::Convention
        | NodeKind::Contract
        | NodeKind::Preference
        | NodeKind::Constraint => Family::Facts,
        NodeKind::Module | NodeKind::Service | NodeKind::FileSummary => Family::Entities,
        NodeKind::Report | NodeKind::Task | NodeKind::Research => Family::Passages,
    }
}

/// Page `page` (from 1) of `hits` (best first) under `caps`, still best first, and what is
/// left after it per kind (`3 more facts`).
pub(crate) fn capped(
    hits: Vec<BrainHit>,
    caps: BrainCaps,
    page: u32,
) -> (Vec<BrainHit>, Vec<String>) {
    let skip = page.max(1) as usize - 1;
    let families = [
        (Family::Facts, caps.facts as usize, "facts"),
        (
            Family::Entities,
            caps.entities as usize,
            "modules and files",
        ),
        (Family::Passages, caps.passages as usize, "findings"),
    ];
    let mut seen = [0usize; 3];
    let mut kept = Vec::new();
    for hit in hits {
        let at = families
            .iter()
            .position(|(of, _, _)| *of == family(hit.node.kind))
            .unwrap_or_default();
        let cap = families[at].1;
        let index = seen[at];
        seen[at] += 1;
        if index >= skip * cap && index < (skip + 1) * cap {
            kept.push(hit);
        }
    }
    let more = families
        .iter()
        .zip(seen)
        .filter_map(|((_, cap, label), seen)| {
            let left = seen.saturating_sub((skip + 1) * cap);
            (left > 0).then(|| format!("{left} more {label}"))
        })
        .collect();
    (kept, more)
}
