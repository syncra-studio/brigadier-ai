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
use crate::{Error, ProviderKind, Result};

/// How one CLI keeps folder trust in its own settings: its adapter's `trust`.
pub(crate) struct Trust {
    /// The file it keeps folder trust in, for the CLIs' environment.
    pub(crate) file: fn(&CliEnv) -> Option<PathBuf>,
    /// What the lock an edit of the file holds adds to the file's name (`.lock`).
    pub(crate) lock: &'static str,
    /// How the entry for a folder stands in the file's text.
    pub(crate) state: fn(&str, &str) -> Result<State>,
    /// The text with the folder trusted, from how its entry stands.
    pub(crate) trust: fn(&str, &str, &State) -> Result<String>,
    /// The text with what [`write`] replaced put back; `None` when nothing changes.
    pub(crate) undo: fn(&str, &str, &TrustBefore) -> Result<Option<String>>,
    /// Whether the CLI is set up where its file is: one that isn't gets nothing written.
    pub(crate) set_up: fn(&Path) -> bool,
    /// The file an extra account's home keeps its own trust in; none when the account's is
    /// the user's own, linked.
    pub(crate) account_file: Option<fn(&Path) -> PathBuf>,
}

/// The CLIs that keep folder trust in their own settings, in [`crate::adapters::ADAPTERS`]
/// order.
pub fn clis() -> Vec<ProviderKind> {
    crate::adapters::ADAPTERS
        .iter()
        .filter(|adapter| adapter.trust.is_some())
        .map(|adapter| adapter.kind)
        .collect()
}

/// Whether `cli` is set up where `file` is: one that isn't gets nothing written; the next
/// start writes it once it is.
pub fn set_up(cli: ProviderKind, file: &Path) -> bool {
    trust_of(cli).is_ok_and(|trust| (trust.set_up)(file))
}

/// The file the extra account of `cli` whose home is `home` keeps its own folder trust in;
/// none when the account's is the user's own, linked.
pub fn account_file(cli: ProviderKind, home: &Path) -> Option<PathBuf> {
    Some((trust_of(cli).ok()?.account_file?)(home))
}

/// How `cli` keeps folder trust, or the error of a CLI that keeps none Brigadier can write.
fn trust_of(cli: ProviderKind) -> Result<&'static Trust> {
    crate::adapters::adapter(cli)
        .and_then(|adapter| adapter.trust)
        .ok_or_else(|| Error::Invalid(format!("Brigadier can't write {cli}'s folder trust")))
}

/// The file `cli` keeps folder trust in, for the CLIs' environment `env`.
pub fn file(cli: ProviderKind, env: &CliEnv) -> Option<PathBuf> {
    (trust_of(cli).ok()?.file)(env)
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
pub fn plan(cli: ProviderKind, file: &Path, folder: &str) -> Result<Option<TrustBefore>> {
    let trust = trust_of(cli)?;
    let text = read(file)?;
    (trust.state)(&text, folder).map(|state| state.before())
}

/// Trusts `folder` in `cli`'s settings at `file`, when its entry still holds `expected`.
pub fn write(
    cli: ProviderKind,
    file: &Path,
    folder: &str,
    expected: &TrustBefore,
) -> Result<Written> {
    let trust = trust_of(cli)?;
    edit(file, trust.lock, |text| {
        let state = (trust.state)(text, folder)?;
        match state.before() {
            None => Ok(Edit::Keep(Written::AlreadyTrusted)),
            Some(before) if before != *expected => Ok(Edit::Keep(Written::Changed(before))),
            Some(_) => {
                let new = (trust.trust)(text, folder, &state)?;
                Ok(Edit::Replace(new, Written::Trusted))
            }
        }
    })
}

/// Puts back what [`write`] replaced, while `folder` is still trusted as Brigadier left it; a
/// value the user changed since is theirs and stays.
pub fn undo(cli: ProviderKind, file: &Path, folder: &str, before: &TrustBefore) -> Result<()> {
    if !file.exists() {
        return Ok(());
    }
    let trust = trust_of(cli)?;
    edit(file, trust.lock, |text| {
        let new = (trust.undo)(text, folder, before)?;
        Ok(match new {
            Some(new) => Edit::Replace(new, ()),
            None => Edit::Keep(()),
        })
    })
}

/// Where a folder's trust stands in a file.
#[derive(Debug)]
pub(crate) enum State {
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

pub(crate) enum Edit<T> {
    Keep(T),
    Replace(String, T),
}

/// How often an edit starts over when the file changed under it.
const ATTEMPTS: usize = 5;

/// Reads the file (none: empty), edits its text with `change`, and writes the result in its
/// place atomically, if it still reads the same. The whole read, edit and rename holds the
/// file's lock, `<file><lock>`: the CLI's own when it takes one (Claude's); else one of
/// Brigadier's beside it (Codex's `config.toml.brigadier.lock`), so two Brigadier writes can't
/// lose one another. Writes of this process wait their turn before it, so they don't use up
/// its wait.
pub(crate) fn edit<T>(
    file: &Path,
    lock: &str,
    change: impl Fn(&str) -> Result<Edit<T>>,
) -> Result<T> {
    let turn = local_turn(file);
    let _turn = turn
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut lock = Lock::take(file, lock)?;
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
        let _ = set_folder_modified(&self.dir, SystemTime::now());
        if let Some(now) = modified() {
            self.stamp = now;
        }
        Ok(())
    }
}

/// Sets a folder's mtime. Windows opens a folder only with FILE_FLAG_BACKUP_SEMANTICS (a plain
/// open is refused, access denied), and writes its times only with write access.
fn set_folder_modified(dir: &Path, at: SystemTime) -> io::Result<()> {
    #[cfg(windows)]
    let folder = {
        use std::os::windows::fs::OpenOptionsExt as _;
        fs::OpenOptions::new()
            .write(true)
            .custom_flags(0x0200_0000)
            .open(dir)
    };
    #[cfg(not(windows))]
    let folder = fs::File::open(dir);
    folder?.set_modified(at)
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

#[cfg(test)]
#[path = "trust_tests.rs"]
mod tests;
