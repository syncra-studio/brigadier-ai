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
//!
//! Only the reads that bear on the task go in (BUILD-PLAN D1.6): the files its spec names,
//! the files beside them and those it mentions by name, with the searches that found them or
//! that its spec repeats, and the ledger's decisions that name its files; a spec that touches
//! none of what the thread read gets all of it, as before.
//!
//! Before the pack, the brief carries what the project's Brain knows about the task
//! (SESSION-FIXES-PLAN.md Q6.6): the Brain is asked about its title, spec and named files, and
//! the best matches go in one line each, with their node ids.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use brigadier_brain::{BrainCaps, BrainHit, BrainQuery, NodeKind, NodeState};
use brigadier_index::SymbolHit;

use super::SessionManager;
use super::brains::{brain_error, cut};
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
/// Bytes of the searches at most, and of one search's line.
const SEARCHES_BYTES: usize = 4 * 1024;
const ONE_SEARCH_MAX: usize = 512;

/// The heading of the brief's Brain section; `tools/ab/worker_tokens.py` counts the `- ` lines
/// under it as the facts a brief carried.
pub(crate) const BRAIN_HEADING: &str = "## What the Brain knows about this";
/// Bytes of the Brain section at most (about 3k tokens).
const BRAIN_MAX: usize = 12 * 1024;
/// Bytes of one fact's line at most.
const BRAIN_FACT_MAX: usize = 800;
/// Bytes of the spec that go into the Brain's question; the Brain weighs only its first words.
const BRAIN_QUESTION_SPEC: usize = 2 * 1024;
/// How many matches of each kind the section holds at most.
const BRAIN_CAPS: BrainCaps = BrainCaps {
    facts: 8,
    entities: 4,
    passages: 3,
    body: BRAIN_FACT_MAX as u32,
};

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
        let decisions = match self.core.board(&task.conversation_id).await {
            Ok(board) => board.decisions,
            Err(_) => Vec::new(),
        };
        let (spec, worktree) = (task.spec.clone(), worktree.to_owned());
        let made = super::blocking(move || {
            let named = named_files(&spec, &worktree);
            // The reads of this task's files, not the whole conversation's (BUILD-PLAN D1.6).
            let reads = task_reads(&reads, &spec, workspace.as_deref());
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
            let mut pack = build(&reads, workspace.as_deref(), &worktree, &outlines);
            let decided = decisions_about(&decisions, &named);
            if !decided.is_empty() {
                if pack.is_empty() {
                    pack.push_str(HEADER.trim_end());
                }
                pack.push_str("\n\n");
                pack.push_str(DECISIONS_HEADING);
                pack.push_str(&decided);
            }
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

impl SessionManager {
    /// The brief's Brain section, and how many facts it holds: the project Brain's best
    /// matches for the task's title, the files its spec names in `worktree`, and its spec.
    /// `None` without a project, or when the Brain has nothing on it. Best effort: a Brain
    /// that can't be read leaves the section out.
    pub(crate) async fn brain_section(
        &self,
        task: &Task,
        worktree: Option<&Path>,
    ) -> Option<(String, u32)> {
        let project = self
            .core
            .conversation(&task.conversation_id)
            .ok()?
            .project_id?;
        let brain = match self.project_brain(&project).await {
            Ok(project) => project.brain.clone(),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "no Brain section: the project's Brain");
                return None;
            }
        };
        // Each claim's sources are re-read in the worker's worktree (BUILD-PLAN E1).
        let reader = self.sources_for(&task.conversation_id, worktree);
        let (title, spec, worktree) = (
            task.title.clone(),
            task.spec.clone(),
            worktree.map(Path::to_owned),
        );
        let found = super::blocking(move || {
            let named = worktree
                .as_deref()
                .map(|worktree| named_files(&spec, worktree))
                .unwrap_or_default();
            let text = format!(
                "{title}\n{}\n{}",
                named.join(" "),
                cut(&spec, BRAIN_QUESTION_SPEC)
            );
            brain
                .query_with(
                    &BrainQuery {
                        text,
                        kinds: Vec::new(),
                        limit: None,
                        max_tokens: None,
                        files: false,
                        history: false,
                        caps: Some(BRAIN_CAPS),
                        page: None,
                    },
                    &|source| reader.check(source),
                )
                .map_err(brain_error)
        })
        .await;
        match found {
            Ok(answer) => brain_lines(&answer.hits),
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "no Brain section");
                None
            }
        }
    }
}

/// The Brain section for `hits`, best first, one line each and at most [`BRAIN_MAX`] bytes,
/// with the number of lines; `None` without hits.
fn brain_lines(hits: &[BrainHit]) -> Option<(String, u32)> {
    let mut text = format!(
        "{BRAIN_HEADING}\n\nWhat earlier work in this project found and decided that matches your task, best match first, each with where it came from. A line marked unverified is a claim nobody checked: check it in the code before you rely on it; a verified source shows only that its quoted words are still there, not that they bear the claim out. Start from it; `query_brain <node id>` gives a node in full, and `query_brain` answers more.\n\n"
    );
    let mut count = 0;
    for hit in hits {
        let node = &hit.node;
        // A constraint has a section of its own, in the user's words (BUILD-PLAN D1.1).
        if matches!(node.state, NodeState::Superseded { .. }) || node.kind == NodeKind::Constraint {
            continue;
        }
        let kind = match node.kind {
            NodeKind::Module => "module",
            NodeKind::Service => "service",
            NodeKind::FileSummary => "file",
            NodeKind::Decision => "decision",
            NodeKind::Convention => "convention",
            NodeKind::Preference => "preference",
            NodeKind::Task => "task",
            NodeKind::Report => "report",
            NodeKind::Research => "research",
            NodeKind::Contract => "contract",
            NodeKind::Constraint => "constraint",
        };
        let stale = match &node.state {
            NodeState::Stale { reason, .. } => format!(" (may be out of date: {reason})"),
            _ => String::new(),
        };
        // A kept report is its task's spec, then its answer: the title names the task, and the
        // answer is what was found.
        let body = node
            .body
            .split_once("\n\nAnswer: ")
            .map_or(node.body.as_str(), |(_, answer)| answer);
        let body = body.split_whitespace().collect::<Vec<_>>().join(" ");
        let title = node.title.split_whitespace().collect::<Vec<_>>().join(" ");
        let label = brigadier_brain::source_words(node.kind, &node.provenance);
        let line = cut(
            &format!("- {kind} [{label}]: {title}{stale}. {body}"),
            BRAIN_FACT_MAX,
        );
        let line = format!("{line} (node {})\n", node.id);
        if text.len() + line.len() > BRAIN_MAX {
            break;
        }
        text.push_str(&line);
        count += 1;
    }
    (count > 0).then(|| (text.trim_end().to_owned(), count))
}

const HEADER: &str = "# Context pack\n\nWhat the thread already looked at for this task, so you don't search for it again. It is all here; read more of the files where you need it.\n\n";

/// The pack's text, at most [`PACK_MAX`] bytes: the thread's searches, the files it read, then
/// the definitions in the files the task names (`outlines`). Empty when there is nothing.
/// The heading of the pack's decisions about the task's files.
const DECISIONS_HEADING: &str = "## Decisions about these files\n\n";
/// Bytes of the decisions at most.
const DECISIONS_MAX: usize = 2 * 1024;

/// What the thread read and searched that bears on this task: the files its spec names, the
/// files beside them, and files whose name it mentions, and the searches its spec repeats or
/// that found those files. A spec that touches none of what the thread read gets all of it,
/// as before (the pack is then the thread's working set).
fn task_reads(reads: &ThreadReads, spec: &str, workspace: Option<&Path>) -> ThreadReads {
    let real = workspace.and_then(brigadier_providers::policy::real_path);
    let relative = |path: &str| -> Option<String> {
        let path = Path::new(path);
        [workspace, real.as_deref()]
            .into_iter()
            .flatten()
            .find_map(|root| path.strip_prefix(root).ok())
            .map(|rel| rel.to_string_lossy().into_owned())
            .filter(|rel| !rel.is_empty())
    };
    let lower = spec.to_lowercase();
    let named: Vec<&str> = spec
        .split(|c: char| {
            c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | ',' | '[' | ']')
        })
        .map(|word| {
            let word = word
                .trim_end_matches(['.', ':', ';'])
                .trim_start_matches("./");
            word.split(':').next().unwrap_or(word)
        })
        .filter(|word| word.contains('/') && !word.starts_with('/'))
        .collect();
    let applies = |rel: &str| -> bool {
        let path = Path::new(rel);
        let beside = named.iter().any(|named| {
            let named = Path::new(named);
            path == named || (named.extension().is_some() && path.parent() == named.parent())
        });
        let mentioned = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_lowercase())
            .is_some_and(|stem| stem.len() >= 4 && lower.contains(&stem));
        beside || mentioned
    };
    let files: Vec<_> = reads
        .files
        .iter()
        .filter(|file| relative(&file.path).is_some_and(|rel| applies(&rel)))
        .cloned()
        .collect();
    if files.is_empty() {
        return reads.clone();
    }
    let searches = reads
        .searches
        .iter()
        .filter(|search| {
            search
                .pattern
                .as_deref()
                .is_some_and(|pattern| lower.contains(&pattern.to_lowercase()))
                || search
                    .hits
                    .iter()
                    .any(|hit| relative(hit).is_some_and(|rel| applies(&rel)))
        })
        .cloned()
        .collect();
    ThreadReads {
        files,
        searches,
        dropped_files: 0,
        dropped_searches: 0,
    }
}

/// The ledger's decisions that name one of the task's files (its path, or its file name), most
/// recent first, one line each, at most [`DECISIONS_MAX`] bytes.
fn decisions_about(decisions: &[crate::work::Decision], named: &[String]) -> String {
    let names: Vec<&str> = named
        .iter()
        .flat_map(|path| {
            let name = Path::new(path).file_name().and_then(|name| name.to_str());
            [Some(path.as_str()), name].into_iter().flatten()
        })
        .collect();
    let mut text = String::new();
    for decision in decisions.iter().rev() {
        let said = format!("{} {}", decision.what, decision.why);
        if !names.iter().any(|name| said.contains(name)) {
            continue;
        }
        let line = match decision.why.trim() {
            "" => format!("- {}\n", decision.what),
            why => format!("- {} ({why})\n", decision.what),
        };
        if text.len() + line.len() > DECISIONS_MAX {
            break;
        }
        text.push_str(&line);
    }
    text
}

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
            let line = format!("- {pattern} in {scope}{glob}: ");
            // Hits are named while the line has room; the rest are counted.
            let mut hits: Vec<String> = Vec::new();
            let mut more = search.more_hits as usize;
            let mut used = line.len();
            for hit in search.hits.iter().filter_map(|hit| relative(hit)) {
                if used + hit.len() + 2 > ONE_SEARCH_MAX {
                    more += 1;
                    continue;
                }
                used += hit.len() + 2;
                hits.push(hit);
            }
            let found = match (hits.is_empty(), more) {
                (true, 0) => "found nothing it named".to_owned(),
                (true, more) => format!("found {more} files"),
                (_, 0) => format!("found {}", hits.join(", ")),
                (_, more) => format!("found {} and {more} more", hits.join(", ")),
            };
            format!("{line}{found}")
        })
        .collect();
    let searches = if searches.is_empty() {
        String::new()
    } else {
        // Searches are listed while they fit; the rest are counted.
        let mut listed = String::new();
        let mut left_out = 0;
        for search in &searches {
            if left_out == 0 && listed.len() + search.len() < SEARCHES_BYTES {
                listed.push_str(search);
                listed.push('\n');
            } else {
                left_out += 1;
            }
        }
        if left_out > 0 {
            let _ = writeln!(listed, "- and {left_out} more searches");
        }
        format!("## Searches it made\n\n{listed}\n")
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

    const FILES_HEADING: &str = "## Files the thread read (as they are in your worktree now)\n\n";
    const UNSHOWN_HEADING: &str = "Also read, not shown here (no room left in the pack):\n";
    // Kept for the list of files not shown: its heading and the count of those not named
    // (`UNSHOWN_KEPT`), and some names (`NAMES_KEPT`, while files are shown).
    const UNSHOWN_MORE_MAX: usize = 48;
    const UNSHOWN_KEPT: usize = UNSHOWN_HEADING.len() + UNSHOWN_MORE_MAX + 1;
    const NAMES_KEPT: usize = 256;
    let mut budget = PACK_MAX
        .saturating_sub(HEADER.len() + searches.len() + kept + FILES_HEADING.len() + UNSHOWN_KEPT);
    let mut sections: Vec<String> = Vec::new();
    let mut shown = BTreeSet::new();
    let mut unshown: Vec<String> = Vec::new();
    let mut unnamed = 0;
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
        let room = budget.saturating_sub(frame + NAMES_KEPT).min(ONE_FILE_MAX);
        if room < FILE_MIN {
            // Named while there is room for the name, else only counted.
            let name = format!("- {rel} ({what})\n");
            if budget >= name.len() {
                budget -= name.len();
                unshown.push(name);
            } else {
                unnamed += 1;
            }
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
    if !sections.is_empty() || !unshown.is_empty() || unnamed > 0 {
        files.push_str(FILES_HEADING);
        for section in &sections {
            files.push_str(section);
        }
        if !unshown.is_empty() || unnamed > 0 {
            files.push_str(UNSHOWN_HEADING);
            for name in &unshown {
                files.push_str(name);
            }
            if unnamed > 0 {
                let _ = writeln!(files, "- and {unnamed} more files");
            }
            files.push('\n');
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

/// How the project at `worktree` starts its app for a live check, when the repository says
/// (SESSION-FIXES-PLAN.md "Computer use"): Brigadier's own dev build, or a root `dev` script
/// run with the package manager its lockfile names.
pub(crate) fn dev_app_command(worktree: &Path) -> Option<String> {
    let manifest = std::fs::read_to_string(worktree.join("package.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&manifest).ok()?;
    let scripts = manifest.get("scripts")?;
    let manager = [
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("bun.lockb", "bun"),
        ("bun.lock", "bun"),
    ]
    .into_iter()
    .find(|(lockfile, _)| worktree.join(lockfile).exists())
    .map_or("npm", |(_, manager)| manager);
    // Built and launched as one app preview, so the session's Activities show it Building,
    // Starting, then Running, with a Stop; its data folder is a scratch one, never the
    // installed app's, and the dev build has its own identity.
    if scripts.get("tauri:debug-app").is_some() && worktree.join("tools/open-dev-app.sh").exists() {
        let app = worktree.join("target/debug/bundle/macos/Brigadier Dev.app");
        return Some(format!(
            "here: build and open it as one `brigadier preview start --kind app --env BRIGADIER_DATA_DIR=<a fresh folder under /tmp>` with the command `{manager} tauri:debug-app && \"{}/Contents/MacOS/brigadier-app\"`; then find its window with the computer tools' list_windows",
            app.display()
        ));
    }
    scripts
        .get("dev")
        .is_some()
        .then(|| format!("here: `{manager} run dev`"))
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
    fn many_searches_and_unread_files_keep_the_pack_in_its_budget() {
        let root = std::env::temp_dir().join(format!("pack-many-{}", uuid::Uuid::new_v4()));
        let _temp = Temp(root.clone());
        let worktree = root.join("task");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        let long: String = (1..=2_000)
            .map(|n| format!("let value_{n} = {n};\n"))
            .collect();
        let names: Vec<String> = (0..300)
            .map(|n| format!("src/a_rather_long_module_name_{n:03}.rs"))
            .collect();
        for name in &names {
            std::fs::write(worktree.join(name), &long).unwrap();
        }
        let hits: Vec<String> = names
            .iter()
            .take(50)
            .map(|name| format!("/ws/{name}"))
            .collect();
        let reads = ThreadReads {
            files: names
                .iter()
                .map(|name| file(&format!("/ws/{name}"), true, Vec::new()))
                .collect(),
            searches: (0..20)
                .map(|n| ThreadSearch {
                    kind: SearchKind::Content,
                    pattern: Some(format!("value_{n}")),
                    scope: "/ws/src".into(),
                    glob: None,
                    hits: hits.clone(),
                    more_hits: 0,
                    outside: false,
                })
                .collect(),
            dropped_files: 0,
            dropped_searches: 0,
        };
        let pack = build(&reads, Some(Path::new("/ws")), &worktree, &[]);
        assert!(pack.len() <= PACK_MAX, "{}", pack.len());
        // Searches and files past the budget are counted, not dropped silently.
        assert!(pack.contains(" more searches"), "{pack}");
        assert!(pack.contains("more files"), "{pack}");
        assert!(
            pack.contains("### src/a_rather_long_module_name_000.rs"),
            "{pack}"
        );
    }

    /// D1.6: a worker's pack carries the thread's reads of its own files (named, beside a
    /// named one, or mentioned by name) and the searches that bear on them, not the whole
    /// conversation's; a spec that names nothing it read gets them all, as before.
    #[test]
    fn a_pack_carries_the_tasks_own_reads() {
        let search = |pattern: &str, hits: &[&str]| ThreadSearch {
            kind: SearchKind::Content,
            pattern: Some(pattern.into()),
            scope: "/ws".into(),
            glob: None,
            hits: hits.iter().map(|hit| (*hit).to_owned()).collect(),
            more_hits: 0,
            outside: false,
        };
        let reads = ThreadReads {
            files: vec![
                file("/ws/apps/web/src/page.tsx", true, Vec::new()),
                file("/ws/apps/web/src/nav.tsx", true, Vec::new()),
                file("/ws/packages/api/routes.ts", true, Vec::new()),
                file("/ws/packages/api/billing.ts", true, Vec::new()),
                file("/ws/README.md", true, Vec::new()),
            ],
            searches: vec![
                search("healthCheck", &["/ws/packages/api/routes.ts"]),
                search("NavLink", &["/ws/apps/web/src/nav.tsx"]),
                search("invoice", &["/ws/packages/api/billing.ts"]),
            ],
            dropped_files: 3,
            dropped_searches: 0,
        };
        let spec = "Add a health route to `packages/api/routes.ts`; the invoice code stays.";
        let mine = task_reads(&reads, spec, Some(Path::new("/ws")));
        let paths: Vec<&str> = mine.files.iter().map(|file| file.path.as_str()).collect();
        // Named, and beside the named file; the web's and the README aren't this task's.
        assert_eq!(
            paths,
            ["/ws/packages/api/routes.ts", "/ws/packages/api/billing.ts"]
        );
        let patterns: Vec<&str> = mine
            .searches
            .iter()
            .filter_map(|search| search.pattern.as_deref())
            .collect();
        assert_eq!(patterns, ["healthCheck", "invoice"]);
        // A file mentioned by name only counts too.
        let named = task_reads(&reads, "Tidy the billing totals.", Some(Path::new("/ws")));
        assert_eq!(named.files.len(), 1);
        assert_eq!(named.files[0].path, "/ws/packages/api/billing.ts");
        // Nothing it read applies: the whole working set, as before.
        assert_eq!(
            task_reads(&reads, "Bump the version.", Some(Path::new("/ws"))),
            reads
        );
    }

    /// D1.6: the ledger's decisions that name a task's files go in its pack, newest first.
    #[test]
    fn a_pack_carries_the_decisions_that_name_its_files() {
        let decision = |what: &str, why: &str| crate::work::Decision {
            id: what.into(),
            request_id: None,
            source: crate::work::DecisionSource::Orchestrator,
            what: what.into(),
            kind: Default::default(),
            why: why.into(),
            at_ms: 0,
            position: 0,
            short: None,
        };
        let decisions = [
            decision("Kept routes.ts's v1 paths", "clients still call them"),
            decision("Landed task-2", ""),
            decision("Moved the billing tests", "packages/api/billing.ts grew"),
        ];
        let named = ["packages/api/routes.ts".to_owned()];
        assert_eq!(
            decisions_about(&decisions, &named),
            "- Kept routes.ts's v1 paths (clients still call them)\n"
        );
        assert_eq!(decisions_about(&decisions, &[]), "");
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

    /// A Brain node as a query returns it, as JSON for brevity.
    fn hit(id: &str, kind: &str, title: &str, body: &str, state: serde_json::Value) -> BrainHit {
        serde_json::from_value(serde_json::json!({
            "node": {
                "id": id, "kind": kind, "key": null, "title": title, "body": body, "state": state,
                "provenance": {"origin": "report", "sessionId": null, "taskId": null, "jobId": null,
                               "worker": null, "commit": null, "recordedAtMs": 0},
                "files": [], "createdAtMs": 0, "updatedAtMs": 0, "expiresAtMs": null, "embedded": false
            },
            "score": 1.0,
            "linked": false
        }))
        .unwrap()
    }

    #[test]
    fn the_brain_section_holds_one_line_per_current_fact_and_a_reports_answer() {
        let fresh = serde_json::json!({"type": "fresh"});
        let hits = [
            hit(
                "n1",
                "decision",
                "Flags are off by default",
                "Every new flag starts off.\nThe user turns it on.",
                fresh.clone(),
            ),
            hit(
                "n2",
                "task",
                "task-4 Add the flag",
                "Task: Add the flag\nYou build it.\n\nAnswer: Flags live in config/flags.rs.\nChanges:\n- config/flags.rs",
                fresh.clone(),
            ),
            hit(
                "n3",
                "convention",
                "Old rule",
                "Gone.",
                serde_json::json!({"type": "superseded", "by": "n1", "reason": "newer"}),
            ),
            hit(
                "n4",
                "module",
                "crates/config",
                "Reads the flags.",
                serde_json::json!({"type": "stale", "reason": "flags.rs changed", "sinceMs": 0}),
            ),
        ];
        let (text, count) = brain_lines(&hits).unwrap();
        assert_eq!(count, 3);
        assert!(text.starts_with(&format!("{BRAIN_HEADING}\n\n")));
        let lines: Vec<&str> = text.lines().filter(|line| line.starts_with("- ")).collect();
        assert_eq!(
            lines,
            [
                "- decision [worker claim, unverified]: Flags are off by default. Every new flag starts off. The user turns it on. (node n1)",
                "- task [worker report]: task-4 Add the flag. Flags live in config/flags.rs. Changes: - config/flags.rs (node n2)",
                "- module [worker report]: crates/config (may be out of date: flags.rs changed). Reads the flags. (node n4)",
            ]
        );
        assert_eq!(brain_lines(&hits[2..3]), None);
        // A long body is cut, its node id kept.
        let long = [hit("n5", "report", "Notes", &"word ".repeat(400), fresh)];
        let (text, _) = brain_lines(&long).unwrap();
        let line = text.lines().last().unwrap();
        assert!(
            line.ends_with("… (node n5)") && line.len() < BRAIN_FACT_MAX + 20,
            "{line}"
        );
    }
}
