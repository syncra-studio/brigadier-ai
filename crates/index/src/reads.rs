use crate::{
    CodeHit, CodeQuery, Error, FolderCount, LanguageCount, Manifest, Module, ProjectMap,
    ReferenceHit, Result, Script, SearchKind, Service, SymbolHit, SymbolRefs,
};
use rusqlite::{Connection, params};
use std::collections::HashSet;

fn err(e: impl std::fmt::Display) -> Error {
    Error::Db(e.to_string())
}

pub fn counts(conn: &Connection) -> Result<(u64, u64, u64, Vec<LanguageCount>)> {
    let files: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .map_err(err)?;
    let symbols: i64 = conn
        .query_row("SELECT count(*) FROM symbols WHERE is_def=1", [], |r| {
            r.get(0)
        })
        .map_err(err)?;
    let refs: i64 = conn
        .query_row("SELECT count(*) FROM symbols WHERE is_def=0", [], |r| {
            r.get(0)
        })
        .map_err(err)?;
    let mut stmt = conn
        .prepare("SELECT lang,count(*) FROM files GROUP BY lang ORDER BY lang")
        .map_err(err)?;
    let languages = stmt
        .query_map([], |r| {
            Ok(LanguageCount {
                language: r.get(0)?,
                files: r.get::<_, i64>(1)? as u64,
            })
        })
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    Ok((files as u64, symbols as u64, refs as u64, languages))
}

fn symbol(row: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolHit> {
    Ok(SymbolHit {
        name: row.get(0)?,
        kind: row.get(1)?,
        path: row.get(2)?,
        line: row.get(3)?,
        end_line: row.get(4)?,
        signature: row.get(5)?,
        doc: row.get(6)?,
    })
}

/// The definitions in the file at `path` (repository-relative), in line order, at most
/// `limit`.
pub fn outline(conn: &Connection, path: &str, limit: u32) -> Result<Vec<SymbolHit>> {
    let err = |e: rusqlite::Error| Error::Db(e.to_string());
    let mut stmt = conn
        .prepare(
            "SELECT name,kind,file,line,end_line,signature,doc FROM symbols \
             WHERE file=?1 AND is_def=1 ORDER BY line LIMIT ?2",
        )
        .map_err(err)?;
    let rows = stmt.query_map(params![path, limit], symbol).map_err(err)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
}

fn rank(needle: &str, candidate: &str) -> u32 {
    let n = needle.to_lowercase();
    let c = candidate.to_lowercase();
    if c == n {
        0
    } else if c.starts_with(&n) {
        1
    } else if c.contains(&n) {
        2
    } else {
        3
    }
}

fn fts_expression(needle: &str, fuzzy: bool) -> String {
    if !fuzzy {
        return format!("\"{}\"", needle.replace('"', "\"\""));
    }
    let chars: Vec<char> = needle.chars().collect();
    let mut grams = Vec::new();
    for gram in chars.windows(3).take(12) {
        let text: String = gram.iter().collect();
        if text.chars().all(|c| c.is_alphanumeric()) {
            let term = format!("\"{text}\"");
            if !grams.contains(&term) {
                grams.push(term);
            }
        }
    }
    grams.join(" OR ")
}

/// `text` inside a `LIKE … ESCAPE '\'` pattern: `%` and `_` match only themselves.
fn like_literal(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// SQL that holds when `column` is the folder (or file) in parameter `?n`, or inside it; the
/// empty folder is the repository root. Compared as text, so `_` and `%` are literal and
/// `src` doesn't take in `src-old/`.
fn under_folder(column: &str, n: u8) -> String {
    format!("(?{n} = '' OR {column} = ?{n} OR substr({column}, 1, length(?{n}) + 1) = ?{n} || '/')")
}

fn distance(a: &str, b: &str) -> u32 {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<u32> = (0..=b.len() as u32).collect();
    for (i, ch) in a.chars().enumerate() {
        let mut next = vec![(i + 1) as u32];
        for (j, other) in b.iter().enumerate() {
            next.push(
                (previous[j + 1] + 1)
                    .min(next[j] + 1)
                    .min(previous[j] + u32::from(ch != *other)),
            );
        }
        previous = next;
    }
    previous[b.len()]
}

/// Exact paths or suffixes beginning at a path boundary, filtered before the result limit.
pub fn lookup_files(conn: &Connection, name: &str, limit: u32) -> Result<Vec<CodeHit>> {
    let mut statement = conn
        .prepare_cached(
            "SELECT path, lang, size FROM files \
         WHERE path = ?1 OR substr(path, -(length(?1) + 1)) = '/' || ?1 \
         ORDER BY length(path), path LIMIT ?2",
        )
        .map_err(err)?;
    let rows = statement
        .query_map(params![name, limit], |row| {
            Ok(CodeHit::File {
                path: row.get(0)?,
                language: row.get(1)?,
                bytes: row.get::<_, i64>(2)? as u64,
            })
        })
        .map_err(err)?;
    rows.collect::<rusqlite::Result<_>>().map_err(err)
}

pub fn search(conn: &Connection, query: &CodeQuery) -> Result<Vec<CodeHit>> {
    let needle = query.query.trim();
    if needle.is_empty() {
        return Ok(Vec::new());
    }
    let limit = query.limit.unwrap_or(30).clamp(1, 200) as usize;
    // The folder (or file) the search is limited to; the root means no limit.
    let folder = query
        .path
        .as_deref()
        .map(|path| path.trim().trim_start_matches("./").trim_end_matches('/'))
        .filter(|path| !path.is_empty());
    let mut hits = Vec::<(u32, u32, String, CodeHit)>::new();
    if query.kind != SearchKind::File {
        let long = needle.chars().count() >= 3;
        for fuzzy in [false, true] {
            if fuzzy && (!long || hits.len() >= limit) {
                break;
            }
            let matches = if long {
                "symbol_fts JOIN symbols s ON s.rowid=symbol_fts.rowid JOIN files f ON f.path=s.file WHERE symbol_fts MATCH ?1"
            } else {
                "symbols s JOIN files f ON f.path=s.file WHERE s.is_def=1 AND s.name LIKE ?1 ESCAPE '\\'"
            };
            // Filtered before the limit, so a match past the first candidates still counts.
            let sql = format!(
                "SELECT s.name,s.kind,s.file,s.line,s.end_line,s.signature,s.doc,f.lang FROM {matches} AND (?2 IS NULL OR f.lang=?2) AND (?3 IS NULL OR {}) LIMIT 2000",
                under_folder("s.file", 3)
            );
            let pattern = if long {
                fts_expression(needle, fuzzy)
            } else {
                format!("%{}%", like_literal(needle))
            };
            if pattern.is_empty() {
                continue;
            }
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let rows = stmt
                .query_map(params![pattern, query.language, folder], symbol)
                .map_err(err)?;
            for row in rows {
                let s = row.map_err(err)?;
                let rank = rank(needle, &s.name);
                if fuzzy && rank < 3 {
                    continue;
                }
                let edit = if rank == 3 {
                    distance(needle, &s.name)
                } else {
                    0
                };
                hits.push((rank, edit, s.name.clone(), CodeHit::Symbol { symbol: s }));
            }
        }
    }
    if query.kind != SearchKind::Symbol {
        let long = needle.chars().count() >= 3;
        for fuzzy in [false, true] {
            if fuzzy && (!long || hits.len() >= limit) {
                break;
            }
            let matches = if long {
                "file_fts JOIN files f ON f.rowid=file_fts.rowid WHERE file_fts MATCH ?1"
            } else {
                "files f WHERE f.path LIKE ?1 ESCAPE '\\'"
            };
            let sql = format!(
                "SELECT f.path,f.lang,f.size FROM {matches} AND (?2 IS NULL OR f.lang=?2) AND (?3 IS NULL OR {}) LIMIT 2000",
                under_folder("f.path", 3)
            );
            let pattern = if long {
                fts_expression(needle, fuzzy)
            } else {
                format!("%{}%", like_literal(needle))
            };
            if pattern.is_empty() {
                continue;
            }
            let mut stmt = conn.prepare(&sql).map_err(err)?;
            let rows = stmt
                .query_map(params![pattern, query.language, folder], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)? as u64,
                    ))
                })
                .map_err(err)?;
            for row in rows {
                let (path, language, bytes) = row.map_err(err)?;
                let rank = rank(needle, &path);
                if fuzzy && rank < 3 {
                    continue;
                }
                let edit = if rank == 3 {
                    distance(needle, &path)
                } else {
                    0
                };
                hits.push((
                    rank,
                    edit,
                    path.clone(),
                    CodeHit::File {
                        path,
                        language,
                        bytes,
                    },
                ));
            }
        }
    }
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.len().cmp(&b.2.len()))
            .then_with(|| a.2.cmp(&b.2))
    });
    let mut seen = HashSet::new();
    Ok(hits
        .into_iter()
        .filter(|(_, _, _, hit)| seen.insert(format!("{hit:?}")))
        .take(limit)
        .map(|x| x.3)
        .collect())
}

pub fn refs(conn: &Connection, name: &str, limit: u32) -> Result<SymbolRefs> {
    let mut stmt=conn.prepare("SELECT name,kind,file,line,end_line,signature,doc FROM symbols WHERE name=?1 AND is_def=1 ORDER BY file,line").map_err(err)?;
    let definitions = stmt
        .query_map([name], symbol)
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let cap = i64::from(limit);
    let mut stmt=conn.prepare("SELECT file,line,kind,signature FROM symbols WHERE name=?1 AND is_def=0 ORDER BY file,line LIMIT ?2").map_err(err)?;
    let mut references = stmt
        .query_map(params![name, cap + 1], |r| {
            Ok(ReferenceHit {
                path: r.get(0)?,
                line: r.get(1)?,
                kind: r.get(2)?,
                context: r.get(3)?,
            })
        })
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let truncated = references.len() > cap as usize;
    references.truncate(cap as usize);
    Ok(SymbolRefs {
        name: name.into(),
        definitions,
        references,
        truncated,
    })
}

pub fn project_map(conn: &Connection) -> Result<ProjectMap> {
    let mut map = ProjectMap::default();
    let mut stmt = conn
        .prepare("SELECT json FROM manifests ORDER BY path")
        .map_err(err)?;
    map.manifests = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .filter_map(|r| {
            r.ok()
                .and_then(|s| serde_json::from_str::<Manifest>(&s).ok())
        })
        .collect();
    let mut stmt = conn
        .prepare("SELECT name,command,source FROM scripts ORDER BY source,name")
        .map_err(err)?;
    map.scripts = stmt
        .query_map([], |r| {
            Ok(Script {
                name: r.get(0)?,
                command: r.get(1)?,
                source: r.get(2)?,
            })
        })
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let mut stmt = conn
        .prepare("SELECT json FROM services ORDER BY name")
        .map_err(err)?;
    map.services = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(err)?
        .filter_map(|r| {
            r.ok()
                .and_then(|s| serde_json::from_str::<Service>(&s).ok())
        })
        .collect();
    let mut stmt=conn.prepare("SELECT substr(path,1,instr(path,'/')-1),count(*) FROM files WHERE instr(path,'/')>0 GROUP BY 1 ORDER BY 1").map_err(err)?;
    map.folders = stmt
        .query_map([], |r| {
            Ok(FolderCount {
                path: r.get(0)?,
                files: r.get::<_, i64>(1)? as u64,
            })
        })
        .map_err(err)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(err)?;
    let names: HashSet<String> = map
        .manifests
        .iter()
        .filter_map(|m| m.name.clone())
        .collect();
    for m in &map.manifests {
        let Some(name) = &m.name else { continue };
        let folder = m.path.rsplit_once('/').map(|x| x.0).unwrap_or("");
        let mut stmt = conn
            .prepare(&format!(
                "SELECT lang,count(*) FROM files WHERE {} GROUP BY lang ORDER BY lang",
                under_folder("path", 1)
            ))
            .map_err(err)?;
        let languages = stmt
            .query_map([folder], |r| {
                Ok(LanguageCount {
                    language: r.get(0)?,
                    files: r.get::<_, i64>(1)? as u64,
                })
            })
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        let total = languages.iter().map(|x| x.files).sum();
        let mut depends_on = m
            .dependencies
            .iter()
            .filter(|d| names.contains(&d.name) && d.name != *name)
            .map(|d| d.name.clone())
            .collect::<Vec<_>>();
        depends_on.sort();
        depends_on.dedup();
        map.modules.push(Module {
            name: name.clone(),
            path: folder.into(),
            manifest: m.path.clone(),
            depends_on,
            files: total,
            languages,
        });
    }
    Ok(map)
}

pub fn digest(conn: &Connection, max_bytes: usize) -> Result<String> {
    let map = project_map(conn)?;
    let mut lines = Vec::new();
    lines.push("Project map".to_owned());
    for f in &map.folders {
        lines.push(format!("Folder {}: {} files", f.path, f.files));
    }
    for m in &map.modules {
        lines.push(format!(
            "Module {} ({}): {} files; depends on {}",
            m.name,
            m.path,
            m.files,
            m.depends_on.join(", ")
        ));
    }
    for m in &map.manifests {
        lines.push(format!(
            "Manifest {} [{}] {} deps: {}",
            m.path,
            m.kind,
            m.name.as_deref().unwrap_or_default(),
            m.dependencies
                .iter()
                .take(12)
                .map(|d| d.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    for s in &map.scripts {
        lines.push(format!("Script {} ({}): {}", s.name, s.source, s.command));
    }
    for s in &map.services {
        lines.push(format!(
            "Service {} ({}): image {:?}, ports {}, depends on {}",
            s.name,
            s.source,
            s.image,
            s.ports.join(","),
            s.depends_on.join(",")
        ));
    }
    let mut stmt = conn
        .prepare("SELECT name,file,refs FROM popular ORDER BY refs DESC LIMIT 2000")
        .map_err(err)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? as u64,
            ))
        })
        .map_err(err)?;
    let popular = rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)?;
    for module in &map.modules {
        let mut count = 0;
        for (name, path, refs) in &popular {
            if module.path.is_empty() || path.starts_with(&format!("{}/", module.path)) {
                lines.push(format!("Symbol {name} ({path}): {refs} references"));
                count += 1;
                if count == 2 {
                    break;
                }
            }
        }
    }
    let mut out = String::new();
    for line in lines {
        if out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}
