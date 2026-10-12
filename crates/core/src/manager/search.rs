//! The right sidebar's Search: the text or pattern the user looks for across a checkout,
//! found file by file and sent as it goes, and Replace all, which rewrites the files listed only
//! if every one is still as the search found it, all of them or none. It reads what git would
//! (ignored files and `.git` stay out) and never writes outside the checkout.

use std::collections::{HashMap, HashSet};
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
/// How many matches in one file pass between looks at whether the search was stopped.
const STOP_CHECK_EVERY: u32 = 4_096;
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

    /// Replaces every match of `query` with `replacement` in the files listed, only if each of
    /// them still holds what the search found (the same bytes and as many matches): all of them
    /// or none (see [`replace`]). With a regular
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
                let repo = git.open(&dir).map_err(git_error)?;
                let root = repo.root().to_path_buf();
                let eligible: HashSet<_> = repo
                    .files(usize::MAX, None)
                    .map_err(git_error)?
                    .0
                    .into_iter()
                    .collect();
                Ok(replace(
                    &root,
                    &matcher,
                    &replacement,
                    files,
                    &eligible,
                    |_| {},
                ))
            })
        };
        match target {
            WorkspaceTarget::Conversation { conversation_id } => {
                let _fence = self.enter(conversation_id)?;
                // Not the repository a session that makes its own copy hasn't made yet.
                let conv = self.conv(conversation_id)?;
                let _landing = conv.merge_lock.lock().await;
                let (dir, _) = self.checkout(conversation_id)?;
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
        if let Some(found) = search_file(matcher, relative, hash, &text, stop) {
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

/// One file's matches, line by line, or none when it has none or the search is stopped.
fn search_file(
    matcher: &Matcher,
    path: String,
    hash: String,
    text: &str,
    stop: &AtomicBool,
) -> Option<SearchFile> {
    let bytes = text.as_bytes();
    let line_end_from =
        |start: usize| memchr::memchr(b'\n', &bytes[start..]).map_or(text.len(), |at| start + at);
    let mut lines: Vec<SearchLine> = Vec::new();
    let mut count = 0u32;
    // The current line: where it starts and ends and its number, moved forward as matches
    // come, so each line is scanned once however many matches it holds.
    let (mut line_start, mut line_number) = (0usize, 1u32);
    let mut line_end = line_end_from(0);
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
        if count.is_multiple_of(STOP_CHECK_EVERY) && stop.load(Ordering::Acquire) {
            return None;
        }
        // The line the match starts on; a match at a line's newline stays on that line.
        if line_end < found.start() {
            flush(&mut current, line_number, &mut lines);
            while line_end < found.start() {
                line_start = line_end + 1;
                line_number += 1;
                line_end = line_end_from(line_start);
            }
        }
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
    // Matches at a line's end can sit past its trimmed carriage return.
    let first = ranges.first().map_or(0, |range| range.0).min(line.len());
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
        .map(|&(a, b)| (a.min(line.len()), b))
        .filter(|range| range.0 >= from && range.0 < to.max(from + 1))
        .map(|(a, b)| SearchRange {
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

/// One listed file's rewrite, ready to go: its new text written beside it, not yet in place.
struct Staged {
    path: String,
    full: PathBuf,
    /// What the search found in it, and what the rewrite holds.
    hash: String,
    new_hash: String,
    matches: u32,
    temp: PathBuf,
    /// Where the file is moved aside while the rewrite takes its place, until all are in.
    aside: PathBuf,
    /// Whether this directory supports links, probed before any original is moved.
    hard_links: bool,
}

/// Rewrites the listed files, all of them or none. Each is checked against what the search
/// found and its rewrite is written beside it; any that changed, can't be searched or written
/// stops it all before a byte moves. Then each in turn is moved aside, checked once more (it may
/// have changed since), and its rewrite linked or exclusively renamed into its place, refusing
/// anything that took the place meanwhile. A failure puts back every file already rewritten; one that something else
/// changed meanwhile stays as it is, its text from before kept beside it, and is listed in
/// `unrestored`. `interfere` runs before each file goes in place (tests change files there).
fn replace(
    root: &Path,
    matcher: &Matcher,
    replacement: &str,
    files: Vec<ReplaceFile>,
    eligible: &HashSet<String>,
    interfere: impl FnMut(usize),
) -> ReplaceOutcome {
    replace_with_link(
        root,
        matcher,
        replacement,
        files,
        eligible,
        interfere,
        |from, to| fs::hard_link(from, to),
    )
}

fn replace_with_link(
    root: &Path,
    matcher: &Matcher,
    replacement: &str,
    files: Vec<ReplaceFile>,
    eligible: &HashSet<String>,
    mut interfere: impl FnMut(usize),
    mut link: impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> ReplaceOutcome {
    let mut outcome = ReplaceOutcome {
        files: 0,
        matches: 0,
        skipped: Vec::new(),
        unrestored: Vec::new(),
    };
    let mut staged: Vec<Staged> = Vec::new();
    for file in files {
        let result = if !eligible.contains(&file.path) || !matcher.wants(&file.path) {
            Err("it is not included in the checkout's searchable files".into())
        } else {
            stage_file(root, matcher, replacement, &file)
        };
        match result {
            Ok(file) => staged.push(file),
            Err(reason) => outcome.skipped.push(ReplaceSkip {
                path: file.path,
                reason,
            }),
        }
    }
    let discard = |staged: &[Staged]| {
        for file in staged {
            let _ = fs::remove_file(&file.temp);
        }
    };
    if !outcome.skipped.is_empty() {
        discard(&staged);
        return outcome;
    }
    let mut directories = HashMap::new();
    for file in &mut staged {
        let parent = file.full.parent().expect("a checkout file has a parent");
        match directories.entry(parent.to_owned()).or_insert_with(|| {
            probe_placement(file, &mut link)
                .map_err(|err| format!("couldn't prepare to replace it: {err}"))
        }) {
            Ok(hard_links) => file.hard_links = *hard_links,
            Err(reason) => {
                outcome.skipped.push(ReplaceSkip {
                    path: file.path.clone(),
                    reason: reason.clone(),
                });
                break;
            }
        }
    }
    if !outcome.skipped.is_empty() {
        discard(&staged);
        return outcome;
    }
    let mut placed = 0;
    let mut failed = None;
    for (index, file) in staged.iter().enumerate() {
        interfere(index);
        match place(file, &mut link) {
            Ok(()) => placed += 1,
            Err((reason, moved)) => {
                // Moved aside but not replaced: it goes back with the rest.
                if moved {
                    placed += 1;
                }
                failed = Some(ReplaceSkip {
                    path: file.path.clone(),
                    reason,
                });
                break;
            }
        }
    }
    if let Some(failed) = failed {
        for file in staged[..placed].iter().rev() {
            if let Err(reason) = put_back(file, &mut link) {
                outcome.unrestored.push(ReplaceSkip {
                    path: file.path.clone(),
                    reason,
                });
            }
        }
        outcome.skipped.push(failed);
        discard(&staged);
        return outcome;
    }
    for file in &staged {
        let _ = fs::remove_file(&file.aside);
        let _ = fs::remove_file(&file.temp);
        outcome.files += 1;
        outcome.matches = outcome.matches.saturating_add(file.matches);
    }
    outcome
}

/// Checks a listed file against its search and writes its rewrite beside it.
fn stage_file(
    root: &Path,
    matcher: &Matcher,
    replacement: &str,
    file: &ReplaceFile,
) -> std::result::Result<Staged, String> {
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
    let temp =
        write_beside(&full, out.as_bytes()).map_err(|err| format!("couldn't write it: {err}"))?;
    Ok(Staged {
        path: file.path.clone(),
        aside: sibling(&full, "replaced"),
        full,
        hash,
        new_hash: self::hash(out.as_bytes()),
        matches: count,
        temp,
        hard_links: true,
    })
}

/// Probe each directory before moving originals. If links fail, prove that an exclusive
/// rename works there using only the rewrite. A failed probe leaves every original untouched.
fn probe_placement(
    file: &Staged,
    link: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::io::Result<bool> {
    let probe = sibling(&file.full, "replace-probe");
    if link(&file.temp, &probe).is_ok() {
        fs::remove_file(&probe)?;
        return Ok(true);
    }
    rename_into(&file.temp, &probe)?;
    let result = fs::rename(&probe, &file.temp);
    if result.is_err() {
        let _ = fs::remove_file(&probe);
    }
    result.map(|()| false)
}

/// Move a rewrite or original into an empty path, refusing a concurrent writer atomically.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn rename_into(from: &Path, to: &Path) -> std::io::Result<()> {
    use nix::libc;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let from = CString::new(from.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    let to = CString::new(to.as_os_str().as_bytes()).map_err(std::io::Error::other)?;
    // SAFETY: both paths are NUL-terminated and live for the entire call.
    if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn rename_into(from: &Path, to: &Path) -> std::io::Result<()> {
    use nix::fcntl::{AT_FDCWD, RenameFlags, renameat2};
    renameat2(AT_FDCWD, from, AT_FDCWD, to, RenameFlags::RENAME_NOREPLACE)
        .map_err(std::io::Error::from)
}

#[cfg(windows)]
fn rename_into(from: &Path, to: &Path) -> std::io::Result<()> {
    // Windows' rename refuses an existing destination.
    fs::rename(from, to)
}

#[cfg(not(any(
    target_os = "macos",
    all(target_os = "linux", target_env = "gnu"),
    windows
)))]
fn rename_into(_: &Path, _: &Path) -> std::io::Result<()> {
    // Without an exclusive rename, fail the probe before moving an original.
    Err(std::io::ErrorKind::Unsupported.into())
}

/// Puts a staged rewrite in its file's place: the file moved aside, checked unchanged, and the
/// rewrite placed where nothing may have appeared meanwhile. On failure, whether the file
/// was moved aside (and so needs putting back).
fn place(
    file: &Staged,
    link: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::result::Result<(), (String, bool)> {
    fs::rename(&file.full, &file.aside)
        .map_err(|err| (format!("couldn't move it: {err}"), false))?;
    let unchanged = fs::read(&file.aside).is_ok_and(|bytes| hash(&bytes) == file.hash);
    if !unchanged {
        return Err((
            "it changed while the files were being replaced".into(),
            true,
        ));
    }
    // Both methods refuse a file that took the place meanwhile.
    let placed = if file.hard_links {
        link(&file.temp, &file.full)
    } else {
        rename_into(&file.temp, &file.full)
    };
    placed.map_err(|err| {
        let reason = if err.kind() == std::io::ErrorKind::AlreadyExists {
            "it was written again while the files were being replaced".into()
        } else {
            format!("couldn't write it: {err}")
        };
        (reason, true)
    })
}

/// Puts back a file `place` moved aside: its rewrite (if still as written) goes, and the file
/// from before comes back. When something else is in its place, both stay: why, and where the
/// text from before is.
fn put_back(
    file: &Staged,
    link: &mut impl FnMut(&Path, &Path) -> std::io::Result<()>,
) -> std::result::Result<(), String> {
    match fs::read(&file.full) {
        Ok(bytes) if hash(&bytes) == file.new_hash => {
            fs::remove_file(&file.full).map_err(|err| format!("couldn't undo it: {err}"))?;
        }
        Ok(_) => {
            return Err(format!(
                "it changed while the files were being replaced; its text from before is in {}",
                file.aside.display()
            ));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("couldn't undo it: {err}")),
    }
    let restored = if file.hard_links {
        link(&file.aside, &file.full).and_then(|()| fs::remove_file(&file.aside))
    } else {
        rename_into(&file.aside, &file.full)
    };
    restored.map_err(|err| {
        format!(
            "couldn't undo it ({err}); its text from before is in {}",
            file.aside.display()
        )
    })
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
    // Search never follows links. Replace must not turn a listed link into a write to its target.
    if full != root.join(relative) {
        return Err(outside());
    }
    let inside = full.strip_prefix(&root).map_err(|_| outside())?;
    if inside.components().any(|part| part.as_os_str() == ".git") || !full.is_file() {
        return Err(outside());
    }
    Ok(full)
}

/// A new name beside `path`, hidden, that no one else will guess.
fn sibling(path: &Path, what: &str) -> PathBuf {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(".{name}.{what}-{}", uuid::Uuid::new_v4()))
}

/// Writes `bytes` to a new file beside `path`, with its permissions: where it is.
fn write_beside(path: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let temp = sibling(path, "replace");
    // Exclusive creation refuses existing files and symlinks, even if a name is guessed.
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        out.write_all(bytes)?;
        out.sync_all()?;
        fs::set_permissions(&temp, fs::metadata(path)?.permissions())
    })();
    match result {
        Ok(()) => Ok(temp),
        Err(err) => {
            let _ = fs::remove_file(&temp);
            Err(err)
        }
    }
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
        let found = search_file(
            &matcher,
            "f".into(),
            String::new(),
            &text,
            &AtomicBool::new(false),
        )
        .unwrap();
        let line = &found.lines[0];
        assert!(line.preview.starts_with('…') && line.preview.ends_with('…'));
        let range = line.ranges[0];
        let units: Vec<u16> = line.preview.encode_utf16().collect();
        let hit = String::from_utf16(&units[range.start as usize..range.end as usize]).unwrap();
        assert_eq!(hit, "needle");
        let indented = "\t    let needle = 1;\n";
        let found = search_file(
            &matcher,
            "f".into(),
            String::new(),
            indented,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            found.lines[0].preview, "let needle = 1;",
            "the indent is left out"
        );
        assert_eq!(found.lines[0].ranges[0], SearchRange { start: 4, end: 10 });
        let short = "a 😀 needle\n";
        let found = search_file(
            &matcher,
            "f".into(),
            String::new(),
            short,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(found.lines[0].ranges[0], SearchRange { start: 5, end: 11 });
    }

    #[test]
    fn a_match_at_a_crlf_line_end_previews_without_its_carriage_return() {
        let mut regex = query(r"\n");
        regex.regex = true;
        let matcher = Matcher::new(&regex).unwrap();
        let text = "one\r\n  two\r\nthree";
        let found = search_file(
            &matcher,
            "f".into(),
            String::new(),
            text,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(found.matches, 2);
        let lines: Vec<(u32, &str)> = found
            .lines
            .iter()
            .map(|line| (line.line, line.preview.as_str()))
            .collect();
        assert_eq!(lines, [(1, "one"), (2, "two")]);
    }

    #[test]
    fn many_matches_on_one_line_are_counted_once_each_and_lines_stay_numbered() {
        let matcher = Matcher::new(&query("a")).unwrap();
        let text = format!("b\n{}\nb\nab\n", "a".repeat(400_000));
        let started = Instant::now();
        let found = search_file(
            &matcher,
            "f".into(),
            String::new(),
            &text,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(found.matches, 400_001);
        let numbers: Vec<u32> = found.lines.iter().map(|line| line.line).collect();
        assert_eq!(numbers, [2, 4]);
        let stopped = search_file(
            &matcher,
            "f".into(),
            String::new(),
            &text,
            &AtomicBool::new(true),
        );
        assert!(
            stopped.is_none(),
            "a stopped search leaves a long file part way"
        );
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

    /// Three files holding `hit`, listed as a search found them, in a new folder.
    fn three_files(name: &str) -> (PathBuf, Vec<ReplaceFile>, HashSet<String>) {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut listed = Vec::new();
        for path in ["a.txt", "b.txt", "c.txt"] {
            let text = format!("{path} hit\n");
            fs::write(dir.join(path), &text).unwrap();
            listed.push(ReplaceFile {
                path: path.into(),
                hash: hash(text.as_bytes()),
                matches: 1,
            });
        }
        let eligible = listed.iter().map(|file| file.path.clone()).collect();
        (dir, listed, eligible)
    }

    fn read(dir: &Path, path: &str) -> String {
        fs::read_to_string(dir.join(path)).unwrap()
    }

    /// The names in `dir` other than the files listed.
    fn others(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| !["a.txt", "b.txt", "c.txt"].contains(&name.as_str()))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_file_changed_while_others_are_replaced_puts_them_all_back() {
        let (dir, listed, eligible) = three_files("replace-rollback");
        let matcher = Matcher::new(&query("hit")).unwrap();
        let outcome = replace(&dir, &matcher, "miss", listed, &eligible, |index| {
            if index == 1 {
                fs::write(dir.join("b.txt"), "b.txt hit, edited\n").unwrap();
            }
        });
        assert_eq!((outcome.files, outcome.matches), (0, 0));
        assert_eq!(outcome.skipped.len(), 1);
        assert_eq!(outcome.skipped[0].path, "b.txt");
        assert!(outcome.skipped[0].reason.contains("changed while"));
        assert!(outcome.unrestored.is_empty(), "{:?}", outcome.unrestored);
        assert_eq!(read(&dir, "a.txt"), "a.txt hit\n", "put back");
        assert_eq!(read(&dir, "b.txt"), "b.txt hit, edited\n", "the edit kept");
        assert_eq!(read(&dir, "c.txt"), "c.txt hit\n");
        assert_eq!(
            others(&dir),
            Vec::<String>::new(),
            "nothing left beside them"
        );
        fs::remove_dir_all(&dir).unwrap();

        // The last file gone before its turn: the two rewritten already go back.
        let (dir, listed, eligible) = three_files("replace-rollback-last");
        let outcome = replace(&dir, &matcher, "miss", listed, &eligible, |index| {
            if index == 2 {
                fs::remove_file(dir.join("c.txt")).unwrap();
            }
        });
        assert_eq!(outcome.files, 0);
        assert_eq!(outcome.skipped[0].path, "c.txt");
        assert!(outcome.unrestored.is_empty());
        assert_eq!(read(&dir, "a.txt"), "a.txt hit\n");
        assert_eq!(read(&dir, "b.txt"), "b.txt hit\n");
        assert_eq!(others(&dir), Vec::<String>::new());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsupported_hard_links_replace_every_file_after_one_probe() {
        let (dir, listed, eligible) =
            three_files("20261012-2245-review-fixes-w04-unsupported-success");
        let matcher = Matcher::new(&query("hit")).unwrap();
        let mut probes = 0;
        let outcome = replace_with_link(
            &dir,
            &matcher,
            "miss",
            listed,
            &eligible,
            |_| {},
            |_, _| {
                probes += 1;
                for path in ["a.txt", "b.txt", "c.txt"] {
                    assert_eq!(
                        read(&dir, path),
                        format!("{path} hit\n"),
                        "probe before moving any original"
                    );
                }
                Err(std::io::ErrorKind::Unsupported.into())
            },
        );
        assert_eq!(probes, 1, "probe once in each directory");
        assert_eq!((outcome.files, outcome.matches), (3, 3));
        assert!(outcome.skipped.is_empty());
        assert!(outcome.unrestored.is_empty());
        for path in ["a.txt", "b.txt", "c.txt"] {
            assert_eq!(read(&dir, path), format!("{path} miss\n"));
        }
        assert!(others(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rename_without_links_refuses_an_existing_destination() {
        let (dir, _, _) = three_files("20261012-2245-review-fixes-w04-exclusive-rename");
        let error = rename_into(&dir.join("a.txt"), &dir.join("b.txt")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(read(&dir, "a.txt"), "a.txt hit\n");
        assert_eq!(read(&dir, "b.txt"), "b.txt hit\n");
        assert!(others(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn unsupported_hard_links_restore_every_original_without_hidden_files() {
        let (dir, listed, eligible) =
            three_files("20261012-2245-review-fixes-w04-unsupported-rollback");
        let matcher = Matcher::new(&query("hit")).unwrap();
        let outcome = replace_with_link(
            &dir,
            &matcher,
            "miss",
            listed,
            &eligible,
            |index| {
                if index == 1 {
                    fs::write(dir.join("b.txt"), "b.txt hit, edited\n").unwrap();
                }
            },
            |_, _| Err(std::io::Error::from(std::io::ErrorKind::Unsupported)),
        );
        // Check the paths first: the old implementation leaves a.txt only under its aside.
        assert_eq!(read(&dir, "a.txt"), "a.txt hit\n");
        assert_eq!(read(&dir, "b.txt"), "b.txt hit, edited\n");
        assert_eq!(read(&dir, "c.txt"), "c.txt hit\n");
        assert_eq!(outcome.files, 0);
        assert_eq!(outcome.skipped[0].path, "b.txt");
        assert!(outcome.unrestored.is_empty(), "{:?}", outcome.unrestored);
        assert!(others(&dir).is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_rewritten_file_edited_before_the_rollback_stays_and_says_where_its_text_was_kept() {
        let (dir, listed, eligible) = three_files("replace-unrestored");
        let matcher = Matcher::new(&query("hit")).unwrap();
        let outcome = replace(&dir, &matcher, "miss", listed, &eligible, |index| {
            if index == 1 {
                // a.txt is already rewritten; then both it and b.txt are edited.
                fs::write(dir.join("a.txt"), "a.txt edited\n").unwrap();
                fs::write(dir.join("b.txt"), "b.txt edited\n").unwrap();
            }
        });
        assert_eq!(outcome.files, 0);
        assert_eq!(outcome.skipped[0].path, "b.txt");
        assert_eq!(outcome.unrestored.len(), 1);
        assert_eq!(outcome.unrestored[0].path, "a.txt");
        assert_eq!(
            read(&dir, "a.txt"),
            "a.txt edited\n",
            "the edit isn't undone"
        );
        assert_eq!(read(&dir, "b.txt"), "b.txt edited\n");
        assert_eq!(read(&dir, "c.txt"), "c.txt hit\n");
        let kept = others(&dir);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert!(kept[0].starts_with(".a.txt.replaced-"));
        assert!(outcome.unrestored[0].reason.contains(&kept[0]));
        assert_eq!(read(&dir, &kept[0]), "a.txt hit\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
