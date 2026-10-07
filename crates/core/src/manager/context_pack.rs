//! A worker's context pack (THREAD-PLAN.md Q8 lever 1): what the session's thread already
//! read and searched, and the definitions in the files its task names, so the worker starts
//! where the thread left off instead of searching again.
//!
//! The whole pack goes in the worker's first message, at most [`PACK_MAX`] bytes, and nowhere
//! else, so the worker never reads it again. The searches come first, then the files the
//! thread read, most recently read first, each cut to what still fits (a file with no room
//! left is only named), then the definitions in the files the task names, as many as fit.
//! Files are shown as they are in the worker's own worktree when it starts (the thread may
//! have read an older version), numbered like a file read. Files outside the thread's
//! workspace, and files the worktree doesn't have, are left out.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use brigadier_index::SymbolHit;

use super::SessionManager;
use super::reads::ThreadReads;
use crate::work::Task;

/// Bytes of the whole pack at most.
pub(crate) const PACK_MAX: usize = 16 * 1024;
/// Bytes of one file shown at most, so one long file doesn't push out the rest.
const ONE_FILE_MAX: usize = 8 * 1024;
/// Bytes kept for the definitions while the files are filled in.
const OUTLINES_KEPT: usize = 3 * 1024;
/// Bytes of a file's section below which it is only named.
const FILE_MIN: usize = 1024;
/// Files the task names that get their definitions listed, at most.
const NAMED_MAX: usize = 12;
/// Definitions listed per file, at most.
const OUTLINE_MAX: u32 = 40;
/// Searches listed, at most.
const SEARCHES_MAX: usize = 20;

impl SessionManager {
    /// The task's context pack for its first message, or `None` when the thread read nothing
    /// that applies and the task names no indexed file. Best effort: a pack that can't be made
    /// is left out.
    pub(crate) async fn context_pack(&self, task: &Task, worktree: &Path) -> Option<String> {
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
        let (spec, worktree) = (task.spec.clone(), worktree.to_owned());
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
            Ok::<_, crate::Error>((!pack.is_empty()).then_some(pack))
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

const HEADER: &str = "# Context pack\n\nWhat the orchestrator already looked at for this task, so you don't search for it again. It is all here; read more of the files where you need it.\n\n";

/// The pack's text, at most [`PACK_MAX`] bytes: the thread's searches, the files it read, then
/// the definitions in the files the task names (`outlines`). Empty when there is nothing.
fn build(
    reads: &ThreadReads,
    workspace: Option<&Path>,
    worktree: &Path,
    outlines: &[(String, Vec<SymbolHit>)],
) -> String {
    // Reads are recorded by their real path; the recorded workspace may go through a link.
    let real = workspace.and_then(brigadier_providers::policy::real_path);
    let relative = |path: &str| -> Option<String> {
        let path = Path::new(path);
        let rel = [workspace, real.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|root| path.strip_prefix(root).ok())?;
        let rel = rel.to_string_lossy().into_owned();
        (!rel.is_empty()).then_some(rel)
    };

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
    let searches = if searches.is_empty() {
        String::new()
    } else {
        format!("## Searches it made\n\n{}\n\n", searches.join("\n"))
    };

    // What the definitions would take, kept for them up to [`OUTLINES_KEPT`].
    let outline_sections: Vec<(String, String)> = outlines
        .iter()
        .map(|(rel, symbols)| {
            let mut section = format!("### {rel}\n");
            for symbol in symbols {
                let _ = writeln!(
                    section,
                    "- line {}: {} `{}`",
                    symbol.line, symbol.kind, symbol.signature
                );
            }
            (rel.clone(), section)
        })
        .collect();
    let outlines_wanted: usize = outline_sections.iter().map(|(_, s)| s.len() + 1).sum();
    let kept = if outline_sections.is_empty() {
        0
    } else {
        outlines_wanted.min(OUTLINES_KEPT) + 64
    };

    const FILES_HEADING: &str =
        "## Files the orchestrator read (as they are in your worktree now)\n\n";
    const UNSHOWN_HEADING: &str = "Also read, not shown here (no room left in the pack):\n";
    let mut budget =
        PACK_MAX.saturating_sub(HEADER.len() + searches.len() + kept + FILES_HEADING.len());
    let mut sections: Vec<String> = Vec::new();
    let mut shown = BTreeSet::new();
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
                .filter(|(start, end)| *start >= 1 && start <= end)
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
        // The section's heading, fences and a possible cut note, and a name in the list.
        const CUT: &str = "[… cut here; read the rest from the file]\n";
        let frame = format!("### {rel} ({what})\n```\n```\n\n").len() + CUT.len();
        let room = budget
            .saturating_sub(frame + UNSHOWN_HEADING.len())
            .min(ONE_FILE_MAX);
        if room < FILE_MIN {
            unshown.push(format!("- {rel} ({what})"));
            continue;
        }
        let mut body = String::new();
        let mut cut = false;
        'ranges: for (start, end) in &ranges {
            for (number, line) in lines[start - 1..*end].iter().enumerate() {
                let numbered = format!("{:>6}\t{line}\n", start + number);
                if body.len() + numbered.len() > room {
                    cut = true;
                    break 'ranges;
                }
                body.push_str(&numbered);
            }
        }
        if cut {
            body.push_str(CUT);
        }
        let section = format!("### {rel} ({what})\n```\n{body}```\n\n");
        budget = budget.saturating_sub(section.len());
        shown.insert(rel);
        sections.push(section);
    }
    let mut files = String::new();
    if !sections.is_empty() || !unshown.is_empty() {
        files.push_str(FILES_HEADING);
        for section in &sections {
            files.push_str(section);
        }
        if !unshown.is_empty() {
            files.push_str(UNSHOWN_HEADING);
            files.push_str(&unshown.join("\n"));
            files.push_str("\n\n");
        }
    }

    // The definitions take what is left, file by file, each cut at a whole line.
    const OUTLINES_HEADING: &str = "## Definitions in the files the task names\n\n";
    let used = HEADER.len() + searches.len() + files.len() + OUTLINES_HEADING.len();
    let mut left = PACK_MAX.saturating_sub(used);
    let mut defined = String::new();
    for (_, section) in outline_sections
        .iter()
        .filter(|(rel, _)| !shown.contains(rel))
    {
        let mut part = String::new();
        for line in section.split_inclusive('\n') {
            if part.len() + line.len() + 1 > left {
                break;
            }
            part.push_str(line);
        }
        // A heading with no definition under it says nothing.
        if part.lines().count() < 2 {
            break;
        }
        part.push('\n');
        left -= part.len();
        defined.push_str(&part);
    }

    let mut text = searches;
    text.push_str(&files);
    if !defined.is_empty() {
        text.push_str(OUTLINES_HEADING);
        text.push_str(&defined);
    }
    if text.is_empty() {
        return text;
    }
    format!("{HEADER}{}", text.trim_end())
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
    fn a_long_pack_fits_its_budget_cutting_files_and_naming_the_ones_with_no_room() {
        let root = std::env::temp_dir().join(format!("pack-long-{}", uuid::Uuid::new_v4()));
        let _temp = Temp(root.clone());
        let worktree = root.join("task");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        let long: String = (1..=2_000)
            .map(|n| format!("let value_{n} = {n};\n"))
            .collect();
        let names = ["a", "b", "c", "d"];
        for name in names {
            std::fs::write(worktree.join(format!("src/{name}.rs")), &long).unwrap();
        }
        let reads = ThreadReads {
            files: names
                .iter()
                .map(|name| file(&format!("/ws/src/{name}.rs"), true, Vec::new()))
                .collect(),
            searches: Vec::new(),
            dropped_files: 0,
            dropped_searches: 0,
        };
        let symbols: Vec<SymbolHit> = (1..=40)
            .map(|n| SymbolHit {
                name: format!("f{n}"),
                kind: "function".into(),
                path: "src/e.rs".into(),
                line: n,
                end_line: n,
                signature: format!("pub fn f{n}()"),
                doc: None,
            })
            .collect();
        let outline = vec![("src/e.rs".to_owned(), symbols)];
        let pack = build(&reads, Some(Path::new("/ws")), &worktree, &outline);
        assert!(pack.len() <= PACK_MAX, "{}", pack.len());
        // The most recently read files are shown, cut at a whole line; the last is only named.
        assert!(
            pack.contains("### src/a.rs (the whole file, 2000 lines)"),
            "{pack}"
        );
        assert!(pack.contains("[… cut here; read the rest from the file]"));
        assert!(
            pack.contains("- src/d.rs (the whole file, 2000 lines)"),
            "{pack}"
        );
        // The definitions keep their room.
        assert!(
            pack.contains("### src/e.rs\n- line 1: function `pub fn f1()`"),
            "{pack}"
        );
        assert!(!pack.contains("context.md"));
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
