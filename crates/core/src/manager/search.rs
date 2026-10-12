//! The right sidebar's Search: the text or pattern the user looks for across a checkout,
//! found file by file and sent as it goes, and Replace all, which rewrites only files still
//! as the search found them. It reads what git would (ignored files and `.git` stay out) and
//! never writes outside the checkout.

use std::fs;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use regex::{Regex, RegexBuilder};
use tokio::sync::mpsc;

use super::{SessionManager, blocking, git_error};
use crate::work::{
    ReplaceFile, ReplaceOutcome, ReplaceSkip, SearchFile, SearchLine, SearchQuery, SearchRange,
    SearchUpdate, WorkspaceTarget,
};
use crate::{Error, Result};

/// Files bigger than this are left out: generated or data files, not what is searched for.
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// How far into a file a NUL byte marks it as binary (as git decides).
const BINARY_PROBE: usize = 8000;
/// A search stops once it found this many matches, or matches in this many files.
pub const MAX_MATCHES: u32 = 20_000;
pub const MAX_FILES: u32 = 2_000;
/// Lines listed for one file; its count still has every match.
const LINES_PER_FILE: usize = 1_000;
/// How much of a long line its preview keeps: some text before the first match, then up to
/// the whole preview's length.
const PREVIEW_BEFORE: usize = 40;
const PREVIEW_CHARS: usize = 250;
/// Found files are sent together at most this often, or once this many are waiting.
const BATCH_EVERY: Duration = Duration::from_millis(50);
const BATCH_FILES: usize = 64;
/// The longest pattern compiled, in bytes of its program.
const REGEX_SIZE: usize = 10 * 1024 * 1024;

/// A running search. Its updates end with `Done`; dropping it stops the search.
pub struct SearchRun {
    updates: mpsc::Receiver<SearchUpdate>,
    stop: Arc<AtomicBool>,
}

impl SearchRun {
    /// The next update, or `None` once the search ended.
    pub async fn next(&mut self) -> Option<SearchUpdate> {
        self.updates.recv().await
    }
}

impl Drop for SearchRun {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl SessionManager {
    /// Starts searching a target's checkout. A pattern that isn't a valid regular expression
    /// fails here, before anything is read.
    pub async fn start_search(
        &self,
        target: &WorkspaceTarget,
        query: SearchQuery,
    ) -> Result<SearchRun> {
        let dir = self.workspace_dir(target)?;
        let git = self.git.clone();
        let root =
            blocking(move || Ok(git.open(&dir).map_err(git_error)?.root().to_path_buf())).await?;
        let matcher = Matcher::new(&query)?;
        let (tx, updates) = mpsc::channel(16);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        thread::Builder::new()
            .name("workspace-search".into())
            .spawn(move || search(&root, &matcher, &stopped, &tx))
            .map_err(|err| Error::Invalid(format!("couldn't start the search: {err}")))?;
        Ok(SearchRun { updates, stop })
    }

    /// Replaces every match of `query` with `replacement` in the files listed, each only if it
    /// still holds what the search found: the same bytes and as many matches. With a regular
    /// expression, `$1` or `${name}` in the replacement stand for what a group matched. A
    /// session's checkout is written only once it has one of its own or works in the
    /// repository, never while its landing moves it.
    pub async fn replace_in_files(
        &self,
        target: &WorkspaceTarget,
        query: SearchQuery,
        replacement: String,
        files: Vec<ReplaceFile>,
    ) -> Result<ReplaceOutcome> {
        self.admit()?;
        let matcher = Matcher::new(&query)?;
        let git = self.git.clone();
        let write = move |dir: PathBuf| {
            blocking(move || {
                let root = git.open(&dir).map_err(git_error)?.root().to_path_buf();
                Ok(replace(&root, &matcher, &replacement, files))
            })
        };
        match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                let _fence = self.enter(conversation_id)?;
                // Not the repository a session that makes its own copy hasn't made yet.
                let (dir, _) = self.checkout(conversation_id)?;
                let conv = self.conv(conversation_id)?;
                let _landing = conv.merge_lock.lock().await;
                write(dir).await
            }
            WorkspaceTarget::Project { project_id } => {
                write(self.project_checkout(project_id)?).await
            }
        }
    }
}

/// The compiled query, with which files it reads.
struct Matcher {
    regex: Regex,
    /// Literal text replaces as it is: `$` means `$`.
    literal: bool,
    include: Option<GlobSet>,
    exclude: Option<GlobSet>,
}

impl Matcher {
    fn new(query: &SearchQuery) -> Result<Self> {
        if query.text.is_empty() {
            return Err(Error::Invalid("there is nothing to search for".into()));
        }
        let pattern = if query.regex {
            query.text.clone()
        } else {
            regex::escape(&query.text)
        };
        // Non-capturing, so the user's own groups keep their numbers in the replacement.
        let pattern = if query.whole_word {
            format!(r"\b(?:{pattern})\b")
        } else {
            pattern
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!query.match_case)
            .multi_line(true)
            .size_limit(REGEX_SIZE)
            .build()
            .map_err(|err| match err {
                regex::Error::Syntax(text) => Error::Invalid(format!(
                    "the search isn't a valid regular expression: {}",
                    text.lines().last().unwrap_or(&text).trim()
                )),
                other => Error::Invalid(format!("the search is too large: {other}")),
            })?;
        Ok(Self {
            regex,
            literal: !query.regex,
            include: globs(&query.include, "files to include")?,
            exclude: globs(&query.exclude, "files to exclude")?,
        })
    }

    /// Whether a file at `path` (relative, with `/`) is searched.
    fn wants(&self, path: &str) -> bool {
        self.include.as_ref().is_none_or(|set| set.is_match(path))
            && !self.exclude.as_ref().is_some_and(|set| set.is_match(path))
    }

    /// The matches in `text`; an empty match (of `a*`, say) is none.
    fn find<'t>(&self, text: &'t str) -> impl Iterator<Item = regex::Match<'t>> {
        self.regex.find_iter(text).filter(|found| !found.is_empty())
    }
}

/// Comma-separated globs as a set, none for an empty list. A glob without a `/` matches at
/// any depth (`*.ts`, `node_modules`), one starting with `/` or `./` only from the root, and
/// a folder's glob matches what it holds.
fn globs(list: &str, field: &str) -> Result<Option<GlobSet>> {
    let mut set = GlobSetBuilder::new();
    let mut any = false;
    for glob in list
        .split(',')
        .map(str::trim)
        .filter(|glob| !glob.is_empty())
    {
        let anchored = glob.starts_with('/') || glob.starts_with("./");
        let glob = glob.trim_start_matches("./").trim_start_matches('/');
        let glob = glob.trim_end_matches('/');
        if glob.is_empty() {
            continue;
        }
        let glob = if anchored || glob.contains('/') {
            glob.to_owned()
        } else {
            format!("**/{glob}")
        };
        for pattern in [glob.clone(), format!("{glob}/**")] {
            let built = GlobBuilder::new(&pattern)
                .literal_separator(true)
                .build()
                .map_err(|err| Error::Invalid(format!("{field}: `{glob}` {err}")))?;
            set.add(built);
        }
        any = true;
    }
    if !any {
        return Ok(None);
    }
    set.build()
        .map(Some)
        .map_err(|err| Error::Invalid(format!("{field}: {err}")))
}

/// A file's text and its hash when it is searched: not too big, not binary, UTF-8.
fn searchable(path: &Path) -> Option<(String, String)> {
    if fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    if memchr::memchr(0, &bytes[..bytes.len().min(BINARY_PROBE)]).is_some() {
        return None;
    }
    let hash = hash(&bytes);
    Some((hash, String::from_utf8(bytes).ok()?))
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Walks the checkout in path order, sending what it finds in batches, until it is done,
/// reaches its limits or is stopped.
fn search(root: &Path, matcher: &Matcher, stop: &AtomicBool, tx: &mpsc::Sender<SearchUpdate>) {
    let walk = WalkBuilder::new(root)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .sort_by_file_name(|a, b| a.cmp(b))
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    let (mut files, mut matches, mut capped) = (0u32, 0u32, false);
    let mut batch = Vec::new();
    let mut sent = Instant::now();
    for entry in walk {
        if stop.load(Ordering::Acquire) {
            return;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Some(relative) = relative_path(root, entry.path()) else {
            continue;
        };
        if !matcher.wants(&relative) {
            continue;
        }
        let Some((hash, text)) = searchable(entry.path()) else {
            continue;
        };
        if let Some(found) = search_file(matcher, relative, hash, &text) {
            files += 1;
            matches = matches.saturating_add(found.matches);
            batch.push(found);
        }
        capped = files >= MAX_FILES || matches >= MAX_MATCHES;
        if !batch.is_empty()
            && (capped || batch.len() >= BATCH_FILES || sent.elapsed() >= BATCH_EVERY)
        {
            let files = std::mem::take(&mut batch);
            if tx.blocking_send(SearchUpdate::Files { files }).is_err() {
                return;
            }
            sent = Instant::now();
        }
        if capped {
            break;
        }
    }
    if !batch.is_empty()
        && tx
            .blocking_send(SearchUpdate::Files { files: batch })
            .is_err()
    {
        return;
    }
    let _ = tx.blocking_send(SearchUpdate::Done {
        files,
        matches,
        capped,
    });
}

/// `path` relative to `root`, with `/` between its parts; none when it isn't valid UTF-8.
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = relative
        .components()
        .map(|part| match part {
            Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect();
    Some(parts?.join("/"))
}

/// The line being collected: where it starts, and its matches' byte ranges in it.
type Pending = (usize, Vec<(usize, usize)>);

/// One file's matches, line by line, or none when it has none.
fn search_file(matcher: &Matcher, path: String, hash: String, text: &str) -> Option<SearchFile> {
    let mut lines: Vec<SearchLine> = Vec::new();
    let mut count = 0u32;
    // Where the current line starts and its number, moved forward as matches come.
    let (mut line_start, mut line_number) = (0usize, 1u32);
    let mut current: Option<Pending> = None;
    let flush = |current: &mut Option<Pending>, number: u32, lines: &mut Vec<SearchLine>| {
        if let Some((start, ranges)) = current.take()
            && lines.len() < LINES_PER_FILE
        {
            lines.push(preview(text, start, number, &ranges));
        }
    };
    for found in matcher.find(text) {
        count = count.saturating_add(1);
        // The line the match starts on.
        let mut moved = false;
        while let Some(newline) = memchr::memchr(b'\n', &text.as_bytes()[line_start..found.start()])
        {
            if !moved {
                flush(&mut current, line_number, &mut lines);
                moved = true;
            }
            line_start += newline + 1;
            line_number += 1;
        }
        let line_end = memchr::memchr(b'\n', &text.as_bytes()[line_start..])
            .map_or(text.len(), |at| line_start + at);
        let range = (
            found.start() - line_start,
            found.end().min(line_end) - line_start,
        );
        match &mut current {
            Some((_, ranges)) => ranges.push(range),
            None => current = Some((line_start, vec![range])),
        }
    }
    flush(&mut current, line_number, &mut lines);
    (count > 0).then_some(SearchFile {
        path,
        matches: count,
        lines,
        hash,
    })
}

/// A line's preview: the whole line, or around its first match when long. `ranges` are byte
/// offsets in the line; the preview's are UTF-16 offsets in it.
fn preview(text: &str, start: usize, number: u32, ranges: &[(usize, usize)]) -> SearchLine {
    let end = memchr::memchr(b'\n', &text.as_bytes()[start..]).map_or(text.len(), |at| start + at);
    let line = text[start..end]
        .strip_suffix('\r')
        .unwrap_or(&text[start..end]);
    let first = ranges.first().map_or(0, |range| range.0);
    // The line's indent is left out, unless a match is in it.
    let indent = (line.len() - line.trim_start().len()).min(first);
    // Character boundaries: up to PREVIEW_BEFORE characters before the first match.
    let from = if line[indent..first].chars().count() > PREVIEW_BEFORE {
        line[..first]
            .char_indices()
            .rev()
            .nth(PREVIEW_BEFORE - 1)
            .map_or(0, |(at, _)| at)
    } else {
        indent
    };
    let to = line[from..]
        .char_indices()
        .nth(PREVIEW_CHARS)
        .map_or(line.len(), |(at, _)| from + at);
    let mut preview = String::new();
    if from > indent {
        preview.push('…');
    }
    let offset = preview.encode_utf16().count() as u32;
    preview.push_str(&line[from..to]);
    let units = |bytes: usize| offset + line[from..from + bytes].encode_utf16().count() as u32;
    let ranges = ranges
        .iter()
        .filter(|range| range.0 >= from && range.0 < to.max(from + 1))
        .map(|&(a, b)| SearchRange {
            start: units(a - from),
            end: units(b.min(line.len()).min(to) - from),
        })
        .collect();
    if to < line.len() {
        preview.push('…');
    }
    SearchLine {
        line: number,
        preview,
        ranges,
    }
}

/// Rewrites each listed file that still holds what the search found.
fn replace(
    root: &Path,
    matcher: &Matcher,
    replacement: &str,
    files: Vec<ReplaceFile>,
) -> ReplaceOutcome {
    let mut outcome = ReplaceOutcome {
        files: 0,
        matches: 0,
        skipped: Vec::new(),
    };
    for file in files {
        match replace_file(root, matcher, replacement, &file) {
            Ok(count) => {
                outcome.files += 1;
                outcome.matches = outcome.matches.saturating_add(count);
            }
            Err(reason) => outcome.skipped.push(ReplaceSkip {
                path: file.path,
                reason,
            }),
        }
    }
    outcome
}

fn replace_file(
    root: &Path,
    matcher: &Matcher,
    replacement: &str,
    file: &ReplaceFile,
) -> std::result::Result<u32, String> {
    let full = checkout_file(root, &file.path)?;
    let (hash, text) = searchable(&full).ok_or("it can't be searched any more")?;
    if hash != file.hash {
        return Err("it changed since the search".into());
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut count = 0u32;
    for captures in matcher.regex.captures_iter(&text) {
        let whole = captures.get(0).expect("a match has its whole text");
        if whole.is_empty() {
            continue;
        }
        count += 1;
        out.push_str(&text[last..whole.start()]);
        if matcher.literal {
            out.push_str(replacement);
        } else {
            captures.expand(replacement, &mut out);
        }
        last = whole.end();
    }
    out.push_str(&text[last..]);
    if count != file.matches {
        return Err("its matches changed since the search".into());
    }
    write_atomically(&full, out.as_bytes()).map_err(|err| format!("couldn't write it: {err}"))?;
    Ok(count)
}

/// The file at `path` in the checkout: relative, without `.`, `..` or `.git`, and once
/// resolved (links followed) still a file inside it.
fn checkout_file(root: &Path, path: &str) -> std::result::Result<PathBuf, String> {
    let outside = || "it is not a file in the checkout".to_owned();
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(name) if name != ".git"))
    {
        return Err(outside());
    }
    let root = root.canonicalize().map_err(|err| err.to_string())?;
    let full = root.join(relative).canonicalize().map_err(|_| outside())?;
    let inside = full.strip_prefix(&root).map_err(|_| outside())?;
    if inside.components().any(|part| part.as_os_str() == ".git") || !full.is_file() {
        return Err(outside());
    }
    Ok(full)
}

/// Writes `bytes` beside `path`, then moves it into place, keeping the file's permissions.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
    let name = path.file_name().ok_or(std::io::ErrorKind::NotFound)?;
    let temp = dir.join(format!(
        ".{}.replace-{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut out = fs::File::create(&temp)?;
        out.write_all(bytes)?;
        out.sync_all()?;
        fs::set_permissions(&temp, fs::metadata(path)?.permissions())?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(text: &str) -> SearchQuery {
        SearchQuery {
            text: text.into(),
            match_case: false,
            whole_word: false,
            regex: false,
            include: String::new(),
            exclude: String::new(),
        }
    }

    #[test]
    fn a_long_line_previews_around_its_match_in_utf16_offsets() {
        let matcher = Matcher::new(&query("needle")).unwrap();
        let text = format!("{}é needle {}\nnone\n", "x".repeat(100), "y".repeat(400));
        let found = search_file(&matcher, "f".into(), String::new(), &text).unwrap();
        let line = &found.lines[0];
        assert!(line.preview.starts_with('…') && line.preview.ends_with('…'));
        let range = line.ranges[0];
        let units: Vec<u16> = line.preview.encode_utf16().collect();
        let hit = String::from_utf16(&units[range.start as usize..range.end as usize]).unwrap();
        assert_eq!(hit, "needle");
        let indented = "\t    let needle = 1;\n";
        let found = search_file(&matcher, "f".into(), String::new(), indented).unwrap();
        assert_eq!(
            found.lines[0].preview, "let needle = 1;",
            "the indent is left out"
        );
        assert_eq!(found.lines[0].ranges[0], SearchRange { start: 4, end: 10 });
        let short = "a 😀 needle\n";
        let found = search_file(&matcher, "f".into(), String::new(), short).unwrap();
        assert_eq!(found.lines[0].ranges[0], SearchRange { start: 5, end: 11 });
    }

    #[test]
    fn a_stopped_search_reads_and_sends_nothing_more() {
        let dir = std::env::temp_dir().join(format!("search-stop-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.txt"), "hit\n").unwrap();
        let matcher = Matcher::new(&query("hit")).unwrap();
        let (tx, mut rx) = mpsc::channel(16);
        search(&dir, &matcher, &AtomicBool::new(true), &tx);
        assert!(rx.try_recv().is_err(), "not even Done");
        search(&dir, &matcher, &AtomicBool::new(false), &tx);
        assert!(matches!(rx.try_recv(), Ok(SearchUpdate::Files { .. })));
        assert!(matches!(
            rx.try_recv(),
            Ok(SearchUpdate::Done { files: 1, .. })
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn globs_match_at_any_depth_unless_anchored_and_a_folder_holds_its_files() {
        let set = globs("*.ts, /docs, node_modules", "x").unwrap().unwrap();
        for path in [
            "a.ts",
            "src/deep/b.ts",
            "docs/guide.md",
            "web/node_modules/x/y.js",
        ] {
            assert!(set.is_match(path), "{path}");
        }
        for path in ["a.tsx", "src/docs/guide.md", "readme.md"] {
            assert!(!set.is_match(path), "{path}");
        }
        assert!(globs(" , ", "x").unwrap().is_none());
        assert!(globs("src/[", "files to include").is_err());
    }
}
