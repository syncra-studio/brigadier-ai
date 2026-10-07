//! A worker's context pack (THREAD-PLAN.md Q8 lever 1): what the session's thread already
//! read and searched, and the definitions in the files its task names, so the worker starts
//! where the thread left off instead of searching again.
//!
//! The whole pack goes to `<scratch>/context.md`. Its first [`INLINE_MAX`] bytes, cut at a
//! section's end, go in the worker's first message with a pointer to the file, so nothing is
//! lost. Files are shown as they are in the worker's own worktree when it starts (the thread
//! may have read an older version), numbered like a file read, most recently read first.
//! Files outside the thread's workspace, and files the worktree doesn't have, are left out.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use brigadier_index::SymbolHit;

use super::SessionManager;
use super::reads::ThreadReads;
use crate::work::Task;

/// Bytes of the pack carried in the first message.
pub(crate) const INLINE_MAX: usize = 16 * 1024;
/// Bytes the pack file holds at most; files past it are only named.
const FILE_MAX: usize = 96 * 1024;
/// Bytes of one file shown at most.
const ONE_FILE_MAX: usize = 32 * 1024;
/// Files the task names that get their definitions listed, at most.
const NAMED_MAX: usize = 12;
/// Definitions listed per file, at most.
const OUTLINE_MAX: u32 = 40;
/// Searches listed, at most.
const SEARCHES_MAX: usize = 20;

impl SessionManager {
    /// Writes the task's context pack to `<scratch>/context.md` and returns what its first
    /// message carries, or `None` when the thread read nothing that applies and the task names
    /// no indexed file. Best effort: a pack that can't be made is left out.
    pub(crate) async fn context_pack(
        &self,
        task: &Task,
        worktree: &Path,
        scratch: &Path,
    ) -> Option<String> {
        let reads = match self.thread_reads_now(&task.conversation_id).await {
            Ok(reads) => reads,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "no context pack: the thread's reads");
                ThreadReads::default()
            }
        };
        let workspace = self
            .recorded_workspace(&task.conversation_id)
            .map(|workspace| workspace.path);
        let index = self.task_index(&task.conversation_id).await.ok();
        let (spec, worktree, scratch) =
            (task.spec.clone(), worktree.to_owned(), scratch.to_owned());
        let made = super::blocking(move || {
            let named = named_files(&spec, &worktree);
            let outlines: Vec<(String, Vec<SymbolHit>)> = match &index {
                Some(index) => named
                    .iter()
                    .filter_map(|rel| {
                        let found = index.outline(rel, OUTLINE_MAX).ok()?;
                        (!found.is_empty()).then(|| (rel.clone(), found))
                    })
                    .collect(),
                None => Vec::new(),
            };
            let pack = build(&reads, workspace.as_deref(), &worktree, &outlines);
            if pack.is_empty() {
                return Ok(None);
            }
            let file = scratch.join("context.md");
            std::fs::write(&file, &pack).map_err(|err| crate::Error::Invalid(err.to_string()))?;
            Ok(Some(inline(&pack, &file)))
        })
        .await;
        match made {
            Ok(pack) => pack,
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "no context pack");
                None
            }
        }
    }
}

/// The pack's text: the files the thread read, its searches, then the definitions in the
/// files the task names (`outlines`). Empty when there is nothing.
fn build(
    reads: &ThreadReads,
    workspace: Option<&Path>,
    worktree: &Path,
    outlines: &[(String, Vec<SymbolHit>)],
) -> String {
    let relative = |path: &str| -> Option<String> {
        let rel = Path::new(path).strip_prefix(workspace?).ok()?;
        let rel = rel.to_string_lossy().into_owned();
        (!rel.is_empty()).then_some(rel)
    };
    let mut sections: Vec<String> = Vec::new();
    let mut shown = BTreeSet::new();
    let mut size = 0;
    let mut unshown = Vec::new();
    for file in reads.files.iter().filter(|file| !file.outside) {
        let Some(rel) = relative(&file.path) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(worktree.join(&rel)) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        let ranges: Vec<(usize, usize)> = if file.whole {
            vec![(1, lines.len())]
        } else {
            file.lines
                .iter()
                .map(|range| {
                    let end = range.end.map_or(lines.len(), |end| end as usize);
                    (range.start as usize, end.min(lines.len()))
                })
                .filter(|(start, end)| start <= end)
                .collect()
        };
        if ranges.is_empty() {
            continue;
        }
        let what = if file.whole {
            format!("the whole file, {} lines", lines.len())
        } else {
            let parts: Vec<String> = ranges
                .iter()
                .map(|(start, end)| format!("{start}–{end}"))
                .collect();
            format!("lines {} of {}", parts.join(", "), lines.len())
        };
        let mut body = String::new();
        let mut cut = false;
        'ranges: for (start, end) in &ranges {
            for (number, line) in lines[start - 1..*end].iter().enumerate() {
                if body.len() + line.len() > ONE_FILE_MAX {
                    cut = true;
                    break 'ranges;
                }
                let _ = writeln!(body, "{:>6}\t{line}", start + number);
            }
        }
        if cut {
            body.push_str("[… cut here; read the rest from the file]\n");
        }
        let section = format!("### {rel} ({what})\n```\n{body}```\n");
        if size + section.len() > FILE_MAX {
            unshown.push(format!("- {rel} ({what})"));
            continue;
        }
        size += section.len();
        shown.insert(rel);
        sections.push(section);
    }
    let mut text = String::new();
    if !sections.is_empty() || !unshown.is_empty() {
        text.push_str("## Files the orchestrator read (as they are in your worktree now)\n\n");
        for section in &sections {
            text.push_str(section);
            text.push('\n');
        }
        if !unshown.is_empty() {
            text.push_str("Also read, not shown here (too much for the pack):\n");
            text.push_str(&unshown.join("\n"));
            text.push_str("\n\n");
        }
    }
    let searches: Vec<String> = reads
        .searches
        .iter()
        .filter(|search| !search.outside)
        .take(SEARCHES_MAX)
        .map(|search| {
            let pattern = search
                .pattern
                .as_deref()
                .map_or_else(|| "a listing".to_owned(), |pattern| format!("`{pattern}`"));
            let scope = relative(&search.scope).unwrap_or_else(|| ".".into());
            let glob = search
                .glob
                .as_deref()
                .map(|glob| format!(" ({glob})"))
                .unwrap_or_default();
            let hits: Vec<String> = search.hits.iter().filter_map(|hit| relative(hit)).collect();
            let found = match (hits.is_empty(), search.more_hits) {
                (true, 0) => "found nothing it named".to_owned(),
                (_, 0) => format!("found {}", hits.join(", ")),
                (_, more) => format!("found {} and {more} more", hits.join(", ")),
            };
            format!("- {pattern} in {scope}{glob}: {found}")
        })
        .collect();
    if !searches.is_empty() {
        text.push_str("## Searches it made\n\n");
        text.push_str(&searches.join("\n"));
        text.push_str("\n\n");
    }
    let outlines: Vec<String> = outlines
        .iter()
        .filter(|(rel, _)| !shown.contains(rel))
        .map(|(rel, symbols)| {
            let mut section = format!("### {rel}\n");
            for symbol in symbols {
                let _ = writeln!(
                    section,
                    "- line {}: {} `{}`",
                    symbol.line, symbol.kind, symbol.signature
                );
            }
            section
        })
        .collect();
    if !outlines.is_empty() {
        text.push_str("## Definitions in the files the task names\n\n");
        text.push_str(&outlines.join("\n"));
        text.push('\n');
    }
    if text.is_empty() {
        return text;
    }
    format!(
        "# Context pack\n\nWhat the orchestrator already looked at for this task, so you don't search for it again. Read more where you need it.\n\n{text}"
    )
}

/// What the first message carries of `pack`: all of it when it fits [`INLINE_MAX`], else
/// its leading sections and where the rest is.
fn inline(pack: &str, file: &Path) -> String {
    if pack.len() <= INLINE_MAX {
        return format!("{}\n(Also saved as {}.)", pack.trim_end(), file.display());
    }
    // Cut before a heading, so a file's section is never cut in two.
    let mut end = INLINE_MAX;
    while !pack.is_char_boundary(end) {
        end -= 1;
    }
    let head = &pack[..end];
    let cut = [head.rfind("\n### "), head.rfind("\n## ")]
        .into_iter()
        .flatten()
        .max()
        .or_else(|| head.rfind('\n'))
        .unwrap_or(0);
    format!(
        "{}\n\n[The pack goes on: read the rest of it in {} ({} bytes).]",
        pack[..cut].trim_end(),
        file.display(),
        pack.len()
    )
}

/// The files `spec` names that the worktree has, repository-relative, in the order named:
/// words with a `/` or a file extension, without quotes, backticks or a trailing
/// `:line`.
fn named_files(spec: &str, worktree: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for word in spec.split(|c: char| {
        c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | '[' | ']')
    }) {
        let word = word
            .trim_end_matches(['.', ':', ';'])
            .trim_start_matches("./");
        let word = word.split(':').next().unwrap_or(word);
        let looks_like_file = word.contains('/')
            || word
                .rsplit_once('.')
                .is_some_and(|(stem, ext)| !stem.is_empty() && (1..=5).contains(&ext.len()));
        if !looks_like_file || word.starts_with('/') || word.contains("..") {
            continue;
        }
        let path: PathBuf = worktree.join(word);
        if path.is_file() && !found.iter().any(|known| known == word) {
            found.push(word.to_owned());
            if found.len() == NAMED_MAX {
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manager::reads::ReadFile;
    use crate::work::ThreadSearch;
    use brigadier_providers::{LineRange, SearchKind};

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn file(path: &str, whole: bool, lines: Vec<LineRange>) -> ReadFile {
        ReadFile {
            path: path.into(),
            whole,
            lines,
            outside: false,
            recency: 0,
        }
    }

    #[test]
    fn a_pack_shows_what_the_thread_read_as_the_worktree_has_it() {
        let root = std::env::temp_dir().join(format!("pack-{}", uuid::Uuid::new_v4()));
        let _temp = Temp(root.clone());
        let worktree = root.join("task");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        let numbered: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        std::fs::write(worktree.join("src/a.rs"), &numbered).unwrap();
        std::fs::write(worktree.join("src/b.rs"), "fn b() {}\n").unwrap();
        let reads = ThreadReads {
            files: vec![
                file(
                    "/ws/src/a.rs",
                    false,
                    vec![LineRange {
                        start: 3,
                        end: Some(4),
                    }],
                ),
                file("/ws/src/b.rs", true, Vec::new()),
                // Not in the worktree: left out.
                file("/ws/src/gone.rs", true, Vec::new()),
            ],
            searches: vec![ThreadSearch {
                kind: SearchKind::Content,
                pattern: Some("fn b".into()),
                scope: "/ws/src".into(),
                glob: None,
                hits: vec!["/ws/src/b.rs".into()],
                more_hits: 2,
                outside: false,
            }],
            dropped_files: 0,
            dropped_searches: 0,
        };
        let outline = vec![(
            "src/c.rs".to_owned(),
            vec![SymbolHit {
                name: "c".into(),
                kind: "function".into(),
                path: "src/c.rs".into(),
                line: 7,
                end_line: 9,
                signature: "pub fn c()".into(),
                doc: None,
            }],
        )];
        let pack = build(&reads, Some(Path::new("/ws")), &worktree, &outline);
        assert!(
            pack.contains(
                "### src/a.rs (lines 3–4 of 30)\n```\n     3\tline 3\n     4\tline 4\n```"
            ),
            "{pack}"
        );
        assert!(pack.contains("### src/b.rs (the whole file, 1 lines)"));
        assert!(!pack.contains("gone.rs"));
        assert!(pack.contains("- `fn b` in src: found src/b.rs and 2 more"));
        assert!(pack.contains("- line 7: function `pub fn c()`"));
        assert!(
            build(
                &ThreadReads::default(),
                Some(Path::new("/ws")),
                &worktree,
                &[]
            )
            .is_empty()
        );
    }

    #[test]
    fn a_long_pack_is_cut_after_a_whole_file_and_says_where_the_rest_is() {
        let section = format!("### f\n```\n{}```\n", "x\n".repeat(3_000));
        let pack = format!("# Context pack\n\n{section}\n{section}\n{section}");
        let shown = inline(&pack, Path::new("/s/context.md"));
        assert!(shown.len() < INLINE_MAX + 200);
        assert!(
            shown.contains("```\n\n[The pack goes on: read the rest of it in /s/context.md"),
            "{shown}"
        );
        assert_eq!(shown.matches("### f").count(), 2);
    }

    #[test]
    fn the_files_a_spec_names_are_found_in_the_worktree() {
        let root = std::env::temp_dir().join(format!("named-{}", uuid::Uuid::new_v4()));
        let _temp = Temp(root.clone());
        std::fs::create_dir_all(root.join("apps/ui")).unwrap();
        std::fs::write(root.join("apps/ui/sidebar.tsx"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        let spec = "Change `apps/ui/sidebar.tsx:40` and README.md. See ../secret and /etc/hosts, e.g. v1.2.";
        assert_eq!(
            named_files(spec, &root),
            ["apps/ui/sidebar.tsx", "README.md"]
        );
    }
}
