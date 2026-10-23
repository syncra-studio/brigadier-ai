//! What a claim's evidence still shows (BUILD-PLAN E1, Q38).
//!
//! - **Two routes, nothing else.** A claim's evidence is the lines of a file (`path:12-14`) or a
//!   command's stored output (its `out-…` id), each with the exact words they show. A review
//!   counts only through such evidence: a finding's location, or an output its text names.
//!   Passing checks or a clean review of other work are no evidence for a claim.
//! - **Re-read on every use.** Whenever a claim is briefed or answered (a worker's brief,
//!   `query_brain`, a rebirth briefing), its sources are read again: the file in the reader's
//!   workspace, the output from the blob store. A quote that is still there makes the *source*
//!   verified; whether it supports the claim is the reader's judgement, so the claim stays
//!   unverified and says what its source shows (the LLM judges, the engine plumbs).
//! - **Independent of reviews.** Nothing here depends on "Review the work".
//!
//! A worker's `learned` and `decisions` lines carry theirs inline, as
//! `[evidence: path:12-14 `the words`]` or `[evidence: out-… `the words`]`; the thread's
//! `remember` names them in `files`, `check` or `finding`, with the words in `quote`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_brain::{Source, SourceCheck};
use brigadier_store::BlobHash;

use super::SessionManager;
use crate::model::ConversationId;

/// How far a quote may have moved from the cited lines and still count: an edit above it
/// shifts it a little.
const DRIFT_LINES: usize = 10;
/// The largest file or output a source is read from.
const READ_MAX: u64 = 4 * 1024 * 1024;
/// The fewest characters a quote needs to be checked at all.
const QUOTE_MIN: usize = 3;

/// A stored output's bytes, by content hash.
type BlobReader = Arc<dyn Fn(&BlobHash) -> Option<Vec<u8>> + Send + Sync>;

/// What a claim's evidence is read from for one use: the reader's workspace (and the project's
/// repositories after it) and the blob store.
#[derive(Clone)]
pub(crate) struct Sources {
    roots: Vec<PathBuf>,
    blobs: BlobReader,
}

impl Sources {
    /// `path` read from the first folder that has it, or why it can't be.
    fn read(&self, path: &str) -> std::result::Result<String, String> {
        use std::path::Component;
        let relative = Path::new(path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err(format!("{path} is outside the checkout"));
        }
        let Some(file) = self
            .roots
            .iter()
            .map(|root| root.join(path))
            .find(|file| file.is_file())
        else {
            return Err(format!("{path} is gone"));
        };
        match std::fs::metadata(&file) {
            Ok(meta) if meta.len() <= READ_MAX => Ok(std::fs::read(&file)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default()),
            _ => Err(format!("{path} can't be read")),
        }
    }

    /// Re-reads `source`: whether its quoted words are still where it says.
    pub(crate) fn check(&self, source: &Source) -> Option<SourceCheck> {
        let quote = normal(&source.quote);
        if quote.chars().count() < QUOTE_MIN {
            return None;
        }
        let (holds, why) = if let Some(alias) = output_alias(&source.cite) {
            self.output_says(alias, source.blob.as_deref(), &quote)
        } else if let Some((path, from, to)) = lines_cite(&source.cite) {
            self.lines_say(&path, from, to, &quote)
        } else {
            (
                false,
                Some("its cite is neither path:lines nor a check's out-… id".to_owned()),
            )
        };
        Some(SourceCheck {
            cite: source.cite.clone(),
            quote: source.quote.clone(),
            holds,
            why,
        })
    }

    fn output_says(&self, alias: &str, blob: Option<&str>, quote: &str) -> (bool, Option<String>) {
        let Some(hash) = blob.and_then(|blob| blob.parse::<BlobHash>().ok()) else {
            return (false, Some(format!("this session has no output {alias}")));
        };
        match (self.blobs)(&hash) {
            Some(bytes) if bytes.len() as u64 <= READ_MAX => {
                if normal(&String::from_utf8_lossy(&bytes)).contains(quote) {
                    (true, None)
                } else {
                    (false, Some(format!("{alias} doesn't say it")))
                }
            }
            Some(_) => (false, Some(format!("{alias} is too long to read"))),
            _ => (false, Some(format!("{alias} is no longer kept"))),
        }
    }

    fn lines_say(&self, path: &str, from: usize, to: usize, quote: &str) -> (bool, Option<String>) {
        let text = match self.read(path) {
            Ok(text) => text,
            Err(why) => return (false, Some(why)),
        };
        let lines: Vec<&str> = text.lines().collect();
        let first = from.saturating_sub(DRIFT_LINES + 1);
        let last = (to + DRIFT_LINES).min(lines.len());
        if first < last && normal(&lines[first..last].join("\n")).contains(quote) {
            (true, None)
        } else {
            (
                false,
                Some(format!("{path} doesn't say it at those lines now")),
            )
        }
    }
}

/// `text` with every run of whitespace one space, trimmed: a quote matches across line breaks
/// and indentation.
fn normal(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The `out-…` id a cite names (`out-1a2b3c4d`, `check out-1a2b3c4d`).
fn output_alias(cite: &str) -> Option<&str> {
    let cite = cite.trim();
    let cite = cite.strip_prefix("check ").unwrap_or(cite).trim();
    (cite.starts_with("out-") && cite.len() > 4 && !cite.contains(char::is_whitespace))
        .then_some(cite)
}

/// The file and lines a cite names: `path:12`, `path:12-14`, `path#L12-L14`, `./path:12`.
pub(crate) fn lines_cite(cite: &str) -> Option<(String, usize, usize)> {
    let cite = cite.trim().trim_matches('`');
    let (path, lines) = cite.rsplit_once("#L").or_else(|| cite.rsplit_once(':'))?;
    let lines = lines.replace('L', "");
    let (from, to) = match lines.split_once(['-', '–']) {
        Some((from, to)) => (from.trim().parse().ok()?, to.trim().parse().ok()?),
        None => {
            let line: usize = lines.trim().parse().ok()?;
            (line, line)
        }
    };
    let path = path.trim().trim_start_matches("./");
    if path.is_empty() || from == 0 || to < from {
        return None;
    }
    Some((path.to_owned(), from, to))
}

/// A line of a worker's report without its `[evidence: …]` tags, and what they cite with
/// their words.
pub(crate) fn evidence_tags(line: &str) -> (String, Vec<(String, String)>) {
    const OPEN: &str = "[evidence:";
    let mut rest = line;
    let mut text = String::new();
    let mut found = Vec::new();
    while let Some(at) = rest.find(OPEN) {
        text.push_str(&rest[..at]);
        let after = rest[at + OPEN.len()..].trim_start();
        let cite_end = after
            .find(|c: char| c.is_whitespace() || c == ']')
            .unwrap_or(after.len());
        let cite = &after[..cite_end];
        let tail = after[cite_end..].trim_start();
        let (quote, next) = match tail.strip_prefix('`').and_then(|q| q.split_once("`]")) {
            Some((quote, next)) => (Some(quote), next),
            None => match tail.split_once(']') {
                Some((_, next)) => (None, next),
                None => (None, ""),
            },
        };
        if let (false, Some(quote)) = (cite.is_empty(), quote) {
            found.push((cite.to_owned(), quote.to_owned()));
        }
        rest = next;
    }
    text.push_str(rest);
    (normal(&text), found)
}

impl SessionManager {
    /// What claims are re-read from for a use in conversation `id`: `worktree` (a worker's) or
    /// the thread's workspace first, then the project's repositories.
    pub(crate) fn sources_for(&self, id: &ConversationId, worktree: Option<&Path>) -> Sources {
        let mut roots: Vec<PathBuf> = worktree.map(Path::to_owned).into_iter().collect();
        if let Some(workspace) = self.recorded_workspace(id) {
            roots.push(workspace.path);
            roots.push(workspace.repo);
        }
        if let Some(project) = self
            .core
            .conversation(id)
            .ok()
            .and_then(|conversation| conversation.project_id)
            .and_then(|project| self.core.project(&project).ok())
        {
            roots.extend(
                project
                    .repos
                    .into_iter()
                    .map(|repo| PathBuf::from(repo.path)),
            );
        }
        roots.dedup();
        let blobs = self.core.store().blobs().clone();
        Sources {
            roots,
            blobs: Arc::new(move |hash| blobs.get_blocking(hash).ok().flatten()),
        }
    }

    /// `nodes` with each claim's sources re-read for a use in conversation `id`.
    pub(crate) async fn sources_checked(
        &self,
        id: &ConversationId,
        nodes: Vec<brigadier_brain::Node>,
    ) -> Vec<brigadier_brain::Node> {
        let reader = self.sources_for(id, None);
        let unread = nodes.clone();
        let mut nodes = nodes;
        super::blocking(move || {
            brigadier_brain::check_sources(&mut nodes.iter_mut().collect::<Vec<_>>(), &|source| {
                reader.check(source)
            });
            Ok(nodes)
        })
        .await
        .unwrap_or(unread)
    }

    /// `cites` with their words as a claim's sources, each check output's `out-…` id taken
    /// with its content from conversation `id`'s outputs.
    pub(crate) async fn claim_sources(
        &self,
        id: &ConversationId,
        cites: Vec<(String, String)>,
    ) -> Vec<Source> {
        let board = self.core.board(id).await.ok();
        cites
            .into_iter()
            .filter(|(_, quote)| !quote.trim().is_empty())
            .map(|(cite, quote)| {
                let blob = output_alias(&cite).and_then(|alias| {
                    board
                        .as_ref()
                        .and_then(|board| board.outputs.get(alias))
                        .map(|output| output.blob.clone())
                });
                let cite = output_alias(&cite).map_or(cite.trim().to_owned(), str::to_owned);
                Source { cite, quote, blob }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sources(root: &Path, output: &'static [u8]) -> Sources {
        let hash = blake3::hash(output).to_hex().to_string();
        Sources {
            roots: vec![root.to_owned()],
            blobs: Arc::new(move |asked| (asked.as_str() == hash).then(|| output.to_vec())),
        }
    }

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn source(cite: &str, quote: &str, blob: Option<&str>) -> Source {
        Source {
            cite: cite.into(),
            quote: quote.into(),
            blob: blob.map(str::to_owned),
        }
    }

    #[test]
    fn cites_name_lines_or_a_check_output() {
        assert_eq!(lines_cite("src/a.rs:12"), Some(("src/a.rs".into(), 12, 12)));
        assert_eq!(
            lines_cite("./src/a.rs:12-14"),
            Some(("src/a.rs".into(), 12, 14))
        );
        assert_eq!(
            lines_cite("src/a.rs#L3-L5"),
            Some(("src/a.rs".into(), 3, 5))
        );
        assert_eq!(lines_cite("src/a.rs"), None);
        assert_eq!(lines_cite("src/a.rs:0"), None);
        assert_eq!(output_alias("check out-1a2b3c4d"), Some("out-1a2b3c4d"));
        assert_eq!(output_alias("run-3"), None);
    }

    #[test]
    fn a_report_line_gives_up_its_evidence_tags() {
        let (text, cites) = evidence_tags(
            "Retries stop at 3 [evidence: src/retry.rs:14 `const MAX_RETRIES: u32 = 3;`] and the test shows it [evidence: out-1a2b3c4d `retry_stops ... ok`].",
        );
        assert_eq!(text, "Retries stop at 3 and the test shows it .");
        assert_eq!(
            cites,
            [
                (
                    "src/retry.rs:14".to_owned(),
                    "const MAX_RETRIES: u32 = 3;".to_owned()
                ),
                ("out-1a2b3c4d".to_owned(), "retry_stops ... ok".to_owned()),
            ]
        );
        // A tag without words is no evidence.
        let (text, cites) = evidence_tags("Fast [evidence: src/a.rs:1]");
        assert_eq!(text, "Fast");
        assert!(cites.is_empty());
    }

    #[test]
    fn a_source_holds_while_its_words_are_near_the_cited_lines_or_in_the_output() {
        let dir = Temp(std::env::temp_dir().join(format!("evidence-{}", uuid::Uuid::new_v4())));
        let root = dir.0.join("repo");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let mut text = String::new();
        for line in 1..=40 {
            text.push_str(&format!("// line {line}\n"));
        }
        text.push_str("const MAX_RETRIES: u32 =\n    3;\n");
        std::fs::write(root.join("src/retry.rs"), &text).unwrap();
        const OUTPUT: &[u8] = b"test retry_stops ... ok\n";
        let hash = blake3::hash(OUTPUT).to_hex().to_string();
        let sources = sources(&root, OUTPUT);

        // Across a line break and a few lines off.
        let held = sources
            .check(&source(
                "src/retry.rs:38",
                "const MAX_RETRIES: u32 = 3;",
                None,
            ))
            .unwrap();
        assert!(held.holds, "{held:?}");
        // Too far from the cited lines.
        let far = sources
            .check(&source(
                "src/retry.rs:2",
                "const MAX_RETRIES: u32 = 3;",
                None,
            ))
            .unwrap();
        assert!(!far.holds);
        // Words the file doesn't hold.
        let wrong = sources
            .check(&source(
                "src/retry.rs:41",
                "const MAX_RETRIES: u32 = 5;",
                None,
            ))
            .unwrap();
        assert!(!wrong.holds);
        assert!(
            sources
                .check(&source("src/gone.rs:1", "anything here", None))
                .is_some_and(|checked| !checked.holds)
        );
        // A check's output.
        let output = sources
            .check(&source(
                "out-1a2b3c4d",
                "retry_stops ... ok",
                Some(hash.as_str()),
            ))
            .unwrap();
        assert!(output.holds, "{output:?}");
        let not_said = sources
            .check(&source(
                "out-1a2b3c4d",
                "retry_stops ... FAILED",
                Some(hash.as_str()),
            ))
            .unwrap();
        assert!(!not_said.holds);
        let unknown = sources
            .check(&source("out-99999999", "retry_stops ... ok", None))
            .unwrap();
        assert!(!unknown.holds);
        // Too short to check.
        assert!(
            sources
                .check(&source("src/retry.rs:41", "3", None))
                .is_none()
        );
    }
}
