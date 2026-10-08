//! Folder trust in the CLIs' own settings, for a folder the user trusted in Brigadier, so that
//! neither CLI asks again when a worker opens in a terminal there.
//!
//! - **Claude Code** keeps it as `projects["<path>"].hasTrustDialogAccepted` in its global
//!   config (`$CLAUDE_CONFIG_DIR/.claude.json`, else `~/.claude.json`). 2.1.294 writes that file
//!   under a `proper-lockfile` lock, the folder `<file>.lock` (stale after 10 s, its mtime
//!   refreshed while held), and re-reads the file under the lock before each write
//!   (`saveConfigWithLock`). Brigadier takes the same lock, so neither loses the other's write.
//! - **Codex** keeps it as `[projects."<path>"] trust_level = "trusted"` in
//!   `$CODEX_HOME/config.toml` (by default `~/.codex/config.toml`). Codex takes no lock for
//!   it; Brigadier's own writes take one beside it (`config.toml.brigadier.lock`, made like
//!   Claude's), so two of them can't lose each other's entry.
//!
//! Both CLIs key a linked worktree's trust on its main checkout, and neither lets a folder's
//! trust reach a repository inside it (checked with 2.1.294 and codex-cli 0.160.1; see
//! `docs/evidence/2026-10-08-trust-dialog.md`): one entry for the repository's top folder
//! covers every worktree Brigadier makes of it.
//!
//! Every edit is a splice of the file's text: everything but the one key changes by not a byte.
//! The new text goes to a temporary file beside the real one (a symlinked config is written
//! where it points), is synced, keeps the file's mode, and replaces it by a rename, only if
//! the file still reads as it did (else the edit starts over). What an edit replaced
//! ([`TrustBefore`]) is what [`undo`] puts back, and only while the value is still the one
//! Brigadier wrote. A layout Brigadier can't edit by a splice (a Codex `projects` written as
//! an inline or dotted table) is refused, never rewritten.

use std::fs;
use std::io::{self, Write as _};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::cli::CliEnv;
use crate::{Error, Result};

/// Which CLI's settings an entry is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum TrustCli {
    Claude,
    Codex,
}

impl TrustCli {
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
        }
    }

    /// The file the CLI keeps folder trust in, for the CLIs' environment `env`.
    pub fn file(self, env: &CliEnv) -> Option<PathBuf> {
        let set = |name: &str| {
            env.var(name)
                .filter(|dir| !dir.is_empty())
                .map(PathBuf::from)
        };
        match self {
            Self::Claude => match set("CLAUDE_CONFIG_DIR") {
                Some(dir) => Some(dir.join(".claude.json")),
                None => env.home().map(|home| home.join(".claude.json")),
            },
            Self::Codex => set("CODEX_HOME")
                .or_else(|| env.home().map(|home| home.join(".codex")))
                .map(|dir| dir.join("config.toml")),
        }
    }
}

/// What a folder's entry held before Brigadier trusted it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TrustBefore {
    /// No entry for the folder (or no file at all).
    NoEntry,
    /// An entry without the trust key.
    NoKey,
    /// The trust key with another value, as written in the file.
    Value { raw: String },
}

/// What [`write`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Written {
    Trusted,
    /// Trusted already when the write came to it: nothing changed.
    AlreadyTrusted,
    /// The entry no longer held `expected` but this.
    Changed(TrustBefore),
}

/// How the entry for `folder` stands now: `None` when it is trusted.
pub fn plan(cli: TrustCli, file: &Path, folder: &str) -> Result<Option<TrustBefore>> {
    let text = read(file)?;
    match cli {
        TrustCli::Claude => claude::state(&text, folder),
        TrustCli::Codex => codex::state(&text, folder),
    }
    .map(|state| state.before())
}

/// Trusts `folder` in `cli`'s settings at `file`, when its entry still holds `expected`.
pub fn write(cli: TrustCli, file: &Path, folder: &str, expected: &TrustBefore) -> Result<Written> {
    edit(cli, file, |text| {
        let state = match cli {
            TrustCli::Claude => claude::state(text, folder)?,
            TrustCli::Codex => codex::state(text, folder)?,
        };
        match state.before() {
            None => Ok(Edit::Keep(Written::AlreadyTrusted)),
            Some(before) if before != *expected => Ok(Edit::Keep(Written::Changed(before))),
            Some(_) => {
                let new = match cli {
                    TrustCli::Claude => claude::trust(text, folder, &state)?,
                    TrustCli::Codex => codex::trust(text, folder, &state)?,
                };
                Ok(Edit::Replace(new, Written::Trusted))
            }
        }
    })
}

/// Puts back what [`write`] replaced, while `folder` is still trusted as Brigadier left it; a
/// value the user changed since is theirs and stays.
pub fn undo(cli: TrustCli, file: &Path, folder: &str, before: &TrustBefore) -> Result<()> {
    if !file.exists() {
        return Ok(());
    }
    edit(cli, file, |text| {
        let new = match cli {
            TrustCli::Claude => claude::undo(text, folder, before)?,
            TrustCli::Codex => codex::undo(text, folder, before)?,
        };
        Ok(match new {
            Some(new) => Edit::Replace(new, ()),
            None => Edit::Keep(()),
        })
    })
}

/// Where a folder's trust stands in a file.
#[derive(Debug)]
enum State {
    Trusted,
    /// No `projects` table or object at all.
    NoProjects,
    NoEntry,
    NoKey,
    Other {
        raw: String,
        span: Range<usize>,
    },
}

impl State {
    fn before(&self) -> Option<TrustBefore> {
        match self {
            Self::Trusted => None,
            Self::NoProjects | Self::NoEntry => Some(TrustBefore::NoEntry),
            Self::NoKey => Some(TrustBefore::NoKey),
            Self::Other { raw, .. } => Some(TrustBefore::Value { raw: raw.clone() }),
        }
    }
}

enum Edit<T> {
    Keep(T),
    Replace(String, T),
}

/// How often an edit starts over when the file changed under it.
const ATTEMPTS: usize = 5;

/// Reads the file (none: empty), edits its text with `change`, and writes the result in its
/// place atomically, if it still reads the same. The whole read, edit and rename holds the
/// file's lock: Claude's own for its file; for Codex's, which has none, one of Brigadier's
/// beside it (`config.toml.brigadier.lock`), so two Brigadier writes can't lose one another.
/// Writes of this process wait their turn before it, so they don't use up its wait.
fn edit<T>(cli: TrustCli, file: &Path, change: impl Fn(&str) -> Result<Edit<T>>) -> Result<T> {
    let turn = local_turn(file);
    let _turn = turn
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let suffix = match cli {
        TrustCli::Claude => ".lock",
        TrustCli::Codex => ".brigadier.lock",
    };
    let mut lock = Lock::take(file, suffix)?;
    let target = resolve(file)?;
    for _ in 0..ATTEMPTS {
        let text = read(&target)?;
        let (new, result) = match change(&text)? {
            Edit::Keep(result) => return Ok(result),
            Edit::Replace(new, result) => (new, result),
        };
        let temp = write_temp(&target, &new)?;
        let still = read(&target).map(|now| now == text);
        let held = lock.check();
        match (still, held) {
            (Ok(true), Ok(())) => {
                if let Err(err) = fs::rename(&temp, &target) {
                    let _ = fs::remove_file(&temp);
                    return Err(failed(&target, err));
                }
                if let Some(dir) = target.parent()
                    && let Ok(dir) = fs::File::open(dir)
                {
                    let _ = dir.sync_all();
                }
                return Ok(result);
            }
            (still, held) => {
                let _ = fs::remove_file(&temp);
                still?;
                held?;
            }
        }
    }
    Err(Error::Invalid(format!(
        "{} kept changing while Brigadier wrote to it; try again",
        target.display()
    )))
}

/// The in-process turn for writing `file`: one at a time per config path.
fn local_turn(file: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    static TURNS: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashMap<PathBuf, std::sync::Arc<std::sync::Mutex<()>>>>,
    > = std::sync::LazyLock::new(Default::default);
    TURNS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(file.to_owned())
        .or_default()
        .clone()
}

/// The file a config path names: where a symlink points (the CLIs write through it too).
fn resolve(file: &Path) -> Result<PathBuf> {
    match fs::canonicalize(file) {
        Ok(real) => Ok(real),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            match fs::symlink_metadata(file) {
                // A dangling link: its target is created.
                Ok(meta) if meta.file_type().is_symlink() => {
                    let to = fs::read_link(file).map_err(|err| failed(file, err))?;
                    Ok(file.parent().unwrap_or(Path::new("/")).join(to))
                }
                _ => Ok(file.to_owned()),
            }
        }
        Err(err) => Err(failed(file, err)),
    }
}

fn read(file: &Path) -> Result<String> {
    match fs::read_to_string(file) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(failed(file, err)),
    }
}

/// Writes `text` to a new temporary file beside `target`, synced, with `target`'s mode (a new
/// file: readable by the user only, as the CLIs make theirs).
fn write_temp(target: &Path, text: &str) -> Result<PathBuf> {
    let dir = target.parent().unwrap_or(Path::new("/"));
    fs::create_dir_all(dir).map_err(|err| failed(dir, err))?;
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let id = uuid::Uuid::new_v4().simple().to_string();
    let temp = dir.join(format!(".{name}.brigadier-{}.tmp", &id[..12]));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mode = fs::metadata(target).map_or(0o600, |meta| meta.permissions().mode() & 0o7777);
        options.mode(mode);
        mode
    };
    let written = options.open(&temp).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
        // The umask may have narrowed the mode.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            out.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        out.sync_all()
    });
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(failed(&temp, err));
    }
    Ok(temp)
}

fn failed(path: &Path, err: io::Error) -> Error {
    Error::Io(io::Error::new(
        err.kind(),
        format!("{}: {err}", path.display()),
    ))
}

/// A config lock as `proper-lockfile` keeps it: the folder `<file><suffix>` (Claude's is
/// `<file>.lock`), made atomically, taken over once its mtime is older than [`Lock::STALE`].
struct Lock {
    dir: PathBuf,
    /// Its mtime as Brigadier last set it: a lock taken over since has another.
    stamp: SystemTime,
}

impl Lock {
    const STALE: Duration = Duration::from_secs(10);
    /// How long a write waits for its holder to release it (well under [`Lock::STALE`]).
    const WAIT: Duration = if cfg!(test) {
        Duration::from_millis(300)
    } else {
        Duration::from_secs(5)
    };

    fn take(file: &Path, suffix: &str) -> Result<Self> {
        let mut dir = file.as_os_str().to_owned();
        dir.push(suffix);
        let dir = PathBuf::from(dir);
        let start = Instant::now();
        let mut pause = Duration::from_millis(20);
        loop {
            match fs::create_dir(&dir) {
                Ok(()) => {
                    let stamp = fs::metadata(&dir)
                        .and_then(|meta| meta.modified())
                        .map_err(|err| failed(&dir, err))?;
                    return Ok(Self { dir, stamp });
                }
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    let stale = fs::metadata(&dir)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Self::STALE);
                    if stale {
                        let _ = fs::remove_dir(&dir);
                        continue;
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    // The config's folder doesn't exist yet.
                    if let Some(parent) = dir.parent() {
                        fs::create_dir_all(parent).map_err(|err| failed(parent, err))?;
                    }
                    continue;
                }
                Err(err) => return Err(failed(&dir, err)),
            }
            if start.elapsed() > Self::WAIT {
                return Err(Error::Invalid(format!(
                    "{} is being written right now; try again in a moment",
                    file.display()
                )));
            }
            std::thread::sleep(pause);
            pause = (pause * 2).min(Duration::from_millis(500));
        }
    }

    /// Still Brigadier's (nobody took it over as stale), and fresh again: refreshed as
    /// `proper-lockfile` refreshes a held lock, so it can't turn stale before the rename.
    fn check(&mut self) -> Result<()> {
        let modified = || {
            fs::metadata(&self.dir)
                .and_then(|meta| meta.modified())
                .ok()
        };
        if modified() != Some(self.stamp) {
            return Err(Error::Invalid(format!(
                "{} was taken over while Brigadier held it",
                self.dir.display()
            )));
        }
        let _ = fs::File::open(&self.dir).and_then(|dir| dir.set_modified(SystemTime::now()));
        if let Some(now) = modified() {
            self.stamp = now;
        }
        Ok(())
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // Only while it is still ours.
        if fs::metadata(&self.dir)
            .and_then(|meta| meta.modified())
            .ok()
            == Some(self.stamp)
        {
            let _ = fs::remove_dir(&self.dir);
        }
    }
}

/// `~/.claude.json`: JSON as Claude writes it (`JSON.stringify(config, null, 2)`), edited by
/// byte ranges found with a small scanner; `serde_json` only checks that the file parses.
mod claude {
    use super::*;

    const KEY: &str = "hasTrustDialogAccepted";

    /// A member of an object: its key and the ranges of the key (with its quotes) and value.
    struct Member {
        key: String,
        key_span: Range<usize>,
        value: Range<usize>,
    }

    /// An object's members and the positions of its braces.
    struct Object {
        open: usize,
        close: usize,
        members: Vec<Member>,
    }

    impl Object {
        fn get(&self, key: &str) -> Option<&Member> {
            self.members.iter().find(|member| member.key == key)
        }
    }

    fn invalid(why: &str) -> Error {
        Error::Invalid(format!(
            "Claude's settings file can't be edited safely: {why}"
        ))
    }

    fn parse(text: &str) -> Result<Option<Object>> {
        if text.trim().is_empty() {
            return Ok(None);
        }
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|err| invalid(&err.to_string()))?;
        if !value.is_object() {
            return Err(invalid("it is not a JSON object"));
        }
        let open = skip_ws(text, 0);
        object(text, open).map(Some)
    }

    pub(super) fn state(text: &str, folder: &str) -> Result<State> {
        let Some(root) = parse(text)? else {
            return Ok(State::NoProjects);
        };
        let Some(projects) = root.get("projects") else {
            return Ok(State::NoProjects);
        };
        if !text[projects.value.clone()].starts_with('{') {
            return Err(invalid("\"projects\" is not an object"));
        }
        let projects = object(text, projects.value.start)?;
        let Some(entry) = projects.get(folder) else {
            return Ok(State::NoEntry);
        };
        if !text[entry.value.clone()].starts_with('{') {
            return Err(invalid("the folder's entry is not an object"));
        }
        let entry = object(text, entry.value.start)?;
        Ok(match entry.get(KEY) {
            None => State::NoKey,
            Some(member) if &text[member.value.clone()] == "true" => State::Trusted,
            Some(member) => State::Other {
                raw: text[member.value.clone()].to_owned(),
                span: member.value.clone(),
            },
        })
    }

    pub(super) fn trust(text: &str, folder: &str, state: &State) -> Result<String> {
        let key = serde_json::to_string(folder).map_err(|err| invalid(&err.to_string()))?;
        let entry = |depth: usize| {
            format!(
                "{{\n{}\"{KEY}\": true\n{}}}",
                indent(depth + 1),
                indent(depth)
            )
        };
        Ok(match state {
            State::Trusted => text.to_owned(),
            State::NoProjects => match parse(text)? {
                None => format!(
                    "{{\n  \"projects\": {{\n    {key}: {}\n  }}\n}}\n",
                    entry(2)
                ),
                Some(root) => {
                    let value = format!("{{\n{}{key}: {}\n{}}}", indent(2), entry(2), indent(1));
                    insert(text, &root, 0, "\"projects\"", &value)
                }
            },
            State::NoEntry => {
                let projects = projects(text)?;
                insert(text, &projects, 1, &key, &entry(2))
            }
            State::NoKey => {
                let entry = entry_object(text, folder)?;
                insert(text, &entry, 2, &format!("\"{KEY}\""), "true")
            }
            State::Other { span, .. } => splice(text, span.clone(), "true"),
        })
    }

    pub(super) fn undo(text: &str, folder: &str, before: &TrustBefore) -> Result<Option<String>> {
        if !matches!(state(text, folder)?, State::Trusted) {
            return Ok(None);
        }
        let entry = entry_object(text, folder)?;
        let index = entry
            .members
            .iter()
            .position(|member| member.key == KEY)
            .ok_or_else(|| invalid("the trust key went missing"))?;
        Ok(Some(match before {
            TrustBefore::NoEntry if entry.members.len() == 1 => {
                let projects = projects(text)?;
                let at = projects
                    .members
                    .iter()
                    .position(|member| member.key == folder)
                    .ok_or_else(|| invalid("the folder's entry went missing"))?;
                remove(text, &projects, at)
            }
            TrustBefore::NoEntry | TrustBefore::NoKey => remove(text, &entry, index),
            TrustBefore::Value { raw } => splice(text, entry.members[index].value.clone(), raw),
        }))
    }

    fn projects(text: &str) -> Result<Object> {
        let root = parse(text)?.ok_or_else(|| invalid("it is empty"))?;
        let member = root
            .get("projects")
            .ok_or_else(|| invalid("\"projects\" went missing"))?;
        object(text, member.value.start)
    }

    fn entry_object(text: &str, folder: &str) -> Result<Object> {
        let projects = projects(text)?;
        let member = projects
            .get(folder)
            .ok_or_else(|| invalid("the folder's entry went missing"))?;
        object(text, member.value.start)
    }

    fn indent(depth: usize) -> String {
        "  ".repeat(depth)
    }

    fn splice(text: &str, range: Range<usize>, with: &str) -> String {
        let mut out = String::with_capacity(text.len() + with.len());
        out.push_str(&text[..range.start]);
        out.push_str(with);
        out.push_str(&text[range.end..]);
        out
    }

    /// Adds `"key": value` as the last member of `object`, which sits at `depth`.
    fn insert(text: &str, object: &Object, depth: usize, key: &str, value: &str) -> String {
        match object.members.last() {
            Some(last) => splice(
                text,
                last.value.end..last.value.end,
                &format!(",\n{}{key}: {value}", indent(depth + 1)),
            ),
            None => splice(
                text,
                object.open..object.close + 1,
                &format!(
                    "{{\n{}{key}: {value}\n{}}}",
                    indent(depth + 1),
                    indent(depth)
                ),
            ),
        }
    }

    /// Removes member `index` of `object` with its separator.
    fn remove(text: &str, object: &Object, index: usize) -> String {
        let members = &object.members;
        let member = &members[index];
        let range = if index > 0 {
            members[index - 1].value.end..member.value.end
        } else if let Some(next) = members.get(1) {
            member.key_span.start..next.key_span.start
        } else {
            object.open..object.close + 1
        };
        let with = if members.len() == 1 { "{}" } else { "" };
        splice(text, range, with)
    }

    fn skip_ws(text: &str, mut at: usize) -> usize {
        let bytes = text.as_bytes();
        while at < bytes.len() && matches!(bytes[at], b' ' | b'\t' | b'\n' | b'\r') {
            at += 1;
        }
        at
    }

    /// The end of the string starting at `at` (its opening quote), past the closing quote.
    fn string_end(text: &str, at: usize) -> Result<usize> {
        let bytes = text.as_bytes();
        let mut i = at + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => return Ok(i + 1),
                _ => i += 1,
            }
        }
        Err(invalid("a string never ends"))
    }

    /// The end of the value starting at `at`.
    fn value_end(text: &str, at: usize) -> Result<usize> {
        let bytes = text.as_bytes();
        match bytes.get(at) {
            Some(b'"') => string_end(text, at),
            Some(b'{' | b'[') => {
                let mut depth = 0usize;
                let mut i = at;
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => {
                            i = string_end(text, i)?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return Ok(i + 1);
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
                Err(invalid("an object never ends"))
            }
            Some(_) => {
                let mut i = at;
                while i < bytes.len()
                    && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                {
                    i += 1;
                }
                Ok(i)
            }
            None => Err(invalid("a value is missing")),
        }
    }

    /// The object whose `{` is at `open`.
    fn object(text: &str, open: usize) -> Result<Object> {
        let bytes = text.as_bytes();
        if bytes.get(open) != Some(&b'{') {
            return Err(invalid("an object was expected"));
        }
        let mut members = Vec::new();
        let mut at = skip_ws(text, open + 1);
        if bytes.get(at) == Some(&b'}') {
            return Ok(Object {
                open,
                close: at,
                members,
            });
        }
        loop {
            let key_end = string_end(text, at)?;
            let key: String = serde_json::from_str(&text[at..key_end])
                .map_err(|err| invalid(&err.to_string()))?;
            let colon = skip_ws(text, key_end);
            if bytes.get(colon) != Some(&b':') {
                return Err(invalid("a key has no value"));
            }
            let start = skip_ws(text, colon + 1);
            let end = value_end(text, start)?;
            members.push(Member {
                key,
                key_span: at..key_end,
                value: start..end,
            });
            let next = skip_ws(text, end);
            match bytes.get(next) {
                Some(b',') => at = skip_ws(text, next + 1),
                Some(b'}') => {
                    return Ok(Object {
                        open,
                        close: next,
                        members,
                    });
                }
                _ => return Err(invalid("an object is malformed")),
            }
        }
    }
}

/// `config.toml`: parsed with `toml_edit` for its spans only; the edit is a splice.
mod codex {
    use toml_edit::{Document, Item, Table};

    use super::*;

    const KEY: &str = "trust_level";
    const TRUSTED: &str = "trusted";

    fn invalid(why: &str) -> Error {
        Error::Invalid(format!("Codex's config.toml can't be edited safely: {why}"))
    }

    fn parse(text: &str) -> Result<Document<&str>> {
        Document::parse(text).map_err(|err| invalid(err.message()))
    }

    /// The folder's table and the `projects` table, when they are standard tables.
    fn tables<'a>(
        doc: &'a Document<&str>,
        folder: &str,
    ) -> Result<(Option<&'a Table>, Option<&'a Table>)> {
        let Some(projects) = doc.as_table().get("projects") else {
            return Ok((None, None));
        };
        let projects = match projects {
            Item::Table(table) if !table.is_dotted() => table,
            _ => return Err(invalid("[projects] is written inline or with dotted keys")),
        };
        match projects.get(folder) {
            None => Ok((Some(projects), None)),
            Some(Item::Table(table)) if !table.is_dotted() && !table.is_implicit() => {
                Ok((Some(projects), Some(table)))
            }
            Some(_) => Err(invalid(
                "the folder's entry is written inline or with dotted keys",
            )),
        }
    }

    pub(super) fn state(text: &str, folder: &str) -> Result<State> {
        let doc = parse(text)?;
        let (projects, entry) = tables(&doc, folder)?;
        let Some(entry) = entry else {
            return Ok(if projects.is_some() {
                State::NoEntry
            } else {
                State::NoProjects
            });
        };
        Ok(match entry.get(KEY) {
            None => State::NoKey,
            Some(item) if item.as_str() == Some(TRUSTED) => State::Trusted,
            Some(item) => {
                let span = item
                    .span()
                    .ok_or_else(|| invalid("a value has no position"))?;
                State::Other {
                    raw: text[span.clone()].to_owned(),
                    span,
                }
            }
        })
    }

    /// The table header a folder's entry gets: `[projects."<folder>"]`.
    fn header(folder: &str) -> String {
        format!("[projects.{}]", basic_string(folder))
    }

    /// A TOML basic string.
    fn basic_string(text: &str) -> String {
        let mut out = String::from("\"");
        for c in text.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\t' => out.push_str("\\t"),
                '\r' => out.push_str("\\r"),
                c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    pub(super) fn trust(text: &str, folder: &str, state: &State) -> Result<String> {
        let line = format!("{KEY} = \"{TRUSTED}\"");
        let new = match state {
            State::Trusted => return Ok(text.to_owned()),
            State::NoProjects | State::NoEntry => {
                let mut new = text.to_owned();
                if !new.is_empty() && !new.ends_with('\n') {
                    new.push('\n');
                }
                if !new.is_empty() {
                    new.push('\n');
                }
                new.push_str(&format!("{}\n{line}\n", header(folder)));
                new
            }
            State::NoKey => {
                let doc = parse(text)?;
                let (_, entry) = tables(&doc, folder)?;
                let span = entry
                    .and_then(Table::span)
                    .ok_or_else(|| invalid("the folder's table has no position"))?;
                // Right after its header line.
                let at = text[span.end..]
                    .find('\n')
                    .map_or(text.len(), |i| span.end + i + 1);
                let mut new = text[..at].to_owned();
                if !new.ends_with('\n') {
                    new.push('\n');
                }
                new.push_str(&line);
                new.push('\n');
                new.push_str(&text[at..]);
                new
            }
            State::Other { span, .. } => {
                format!("{}\"{TRUSTED}\"{}", &text[..span.start], &text[span.end..])
            }
        };
        // What was spliced in must read back as exactly that.
        match self::state(&new, folder)? {
            State::Trusted => Ok(new),
            _ => Err(invalid("the edit did not read back as trusted")),
        }
    }

    pub(super) fn undo(text: &str, folder: &str, before: &TrustBefore) -> Result<Option<String>> {
        if !matches!(state(text, folder)?, State::Trusted) {
            return Ok(None);
        }
        let doc = parse(text)?;
        let (_, Some(entry)) = tables(&doc, folder)? else {
            return Ok(None);
        };
        let item = entry
            .get(KEY)
            .ok_or_else(|| invalid("the trust key went missing"))?;
        let value = item
            .span()
            .ok_or_else(|| invalid("a value has no position"))?;
        let line = line_of(text, value.clone());
        let ours = format!("{KEY} = \"{TRUSTED}\"");
        let new = match before {
            TrustBefore::Value { raw } => {
                format!("{}{raw}{}", &text[..value.start], &text[value.end..])
            }
            TrustBefore::NoEntry if entry.len() == 1 => {
                let header = entry
                    .span()
                    .ok_or_else(|| invalid("the folder's table has no position"))?;
                // The table runs to the next table's header, or the end.
                let end = next_table_start(doc.as_table(), header.end).unwrap_or(text.len());
                let mut start = line_of(text, header.clone()).start;
                // With the blank line Brigadier put before it, when it is the last table.
                if end == text.len() && text[..start].ends_with("\n\n") {
                    start -= 1;
                }
                if text[line.clone()].trim() != ours {
                    return Err(invalid("the trust line was changed"));
                }
                format!("{}{}", &text[..start], &text[end..])
            }
            TrustBefore::NoEntry | TrustBefore::NoKey => {
                if text[line.clone()].trim() != ours {
                    return Err(invalid("the trust line was changed"));
                }
                format!("{}{}", &text[..line.start], &text[line.end..])
            }
        };
        parse(&new)?;
        Ok(Some(new))
    }

    /// The whole line around `range`, with its newline.
    fn line_of(text: &str, range: Range<usize>) -> Range<usize> {
        let start = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
        let end = text[range.end..]
            .find('\n')
            .map_or(text.len(), |i| range.end + i + 1);
        start..end
    }

    /// Where the first table header at or after `after` starts (an implicit table, such as
    /// `projects` in `[projects."…"]`, has no header of its own).
    fn next_table_start(root: &Table, after: usize) -> Option<usize> {
        let mut best: Option<usize> = None;
        let mut stack = vec![root];
        while let Some(table) = stack.pop() {
            for (_, item) in table.iter() {
                let tables: Vec<&Table> = match item {
                    Item::Table(table) => vec![table],
                    Item::ArrayOfTables(array) => array.iter().collect(),
                    _ => Vec::new(),
                };
                for table in tables {
                    if let Some(span) = table.span()
                        && !table.is_implicit()
                        && span.start >= after
                        && best.is_none_or(|best| span.start < best)
                    {
                        best = Some(span.start);
                    }
                    stack.push(table);
                }
            }
        }
        best
    }
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
