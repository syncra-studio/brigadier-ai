//! Warm task worktrees: a new worktree gets copy-on-write copies of the dependency installs and
//! build caches the user's checkout already has, so a worker's first build or test run doesn't
//! start from nothing.
//!
//! Best effort throughout: anything that can't be copied safely is left out, and the worker
//! installs or builds it as it would have anyway. Never fails the task. The rule above all
//! others: warming never writes to, renames in or deletes from the checkout, and a copy never
//! gives a worker a way to write into it. When in doubt, the cache is left out.
//!
//! - **What:** git-ignored folders named in [`CACHES`], at the root or in a tracked package
//!   folder (one holding a [`MANIFESTS`] file) up to [`MAX_DEPTH`] folders down. A package
//!   cache is copied only while the worktree's manifest and lockfiles match the checkout's
//!   ([`DEPENDENCY_FILES`]): otherwise its packages would be the wrong versions.
//! - **Never:** Python environments (a folder holding a `pyvenv.cfg`, a cache inside one, or a
//!   link to one). Their scripts point at the interpreter and packages they were made with, so
//!   a copy would keep using the user's own environment. A folder's name alone doesn't make one:
//!   a package folder called `env` is warmed like any other.
//! - **Only real folders:** every folder from the checkout or the worktree down to a cache is
//!   reached without following links ([`clone::Folder`]), so a link can't send a copy, a
//!   clean-up or a rename anywhere else.
//! - **Paths back into the checkout:** each copy is checked before it is published.
//!   - Links are resolved fully; one that lands in the checkout is pointed at the same place in
//!     the worktree, and one that leads nowhere drops the copy.
//!   - The files known to hold the checkout's path ([`known_kind`]: package shims and
//!     package-manager state, Cargo's build-script outputs and dep-info, Turbo's logs) are
//!     rewritten to the worktree, in each format's own spelling ([`Style`]).
//!   - Every executable file, every file in a `bin` folder and every small text file is read:
//!     one that still names the checkout means a kind we don't know how to rewrite, and the
//!     whole copy is dropped. So is a copy too big to read within [`MAX_PROBED_FILES`] and
//!     [`MAX_PROBED_BYTES`].
//!   - What this can't see: a binary file (other than an executable) or a text file over
//!     [`MAX_PROBE_BYTES`] that names the checkout. Package managers keep the paths they act on
//!     in text state and scripts, which are read, so the gap is a tool that writes its target
//!     path into a large or binary file at install time. In `target`, compiled programs are not
//!     read either (their debug information always names the folder they were built in); the
//!     ones in a profile folder (`target/debug/app`, the ones people run by hand) are removed
//!     instead, and Cargo rebuilds the workspace's own crates in a new worktree anyway.
//! - **Atomic:** each folder is copied into a temp folder beside the worktree (out of git's
//!   sight, as the session or worker may already work there), checked and rewritten there,
//!   then moved into place; on any failure the temp folder is removed. A destination that
//!   already exists is never touched.
//! - **Not mid-write:** a `target` is copied only while every profile's `.cargo-lock` can be
//!   held (and it is held for the whole copy). Other caches are skipped while an install runs in
//!   the checkout or anything at their top (or their package manager's state) changed in the
//!   last [`RECENT_WRITE`]. All of it is checked again after copying, and a copy whose cache
//!   changed meanwhile is dropped. Where a running install can't be seen (Windows), nothing is
//!   copied.
//! - **Bounded:** warming a worktree stops at [`WARM_DEADLINE`], and a copy with more than
//!   [`MAX_ENTRIES`] entries isn't checked; a copy not finished in time is removed.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

use brigadier_git::Repo;
use brigadier_sandbox::Platform;
use brigadier_sandbox::clone::{self, Folder};
use memchr::memmem::Finder;

/// Folders worth copying, as paths inside a package folder.
const CACHES: &[&str] = &[
    "node_modules",
    "target",
    ".next/cache",
    ".turbo",
    ".gradle",
    "Pods",
    ".build",
];

/// Files that make the folder holding them a package folder.
const MANIFESTS: &[&str] = &[
    "package.json",
    "Cargo.toml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
    "Podfile",
    "Package.swift",
];

/// The files, in its package folder, that decide what a package cache holds. A cache is copied
/// only when the worktree has them exactly as the checkout does. For `node_modules` the root's
/// count too (a workspace has one lockfile). Cargo's `target` needs none: Cargo rebuilds what
/// changed by itself.
const DEPENDENCY_FILES: &[(&str, &[&str])] = &[
    (
        "node_modules",
        &[
            "package.json",
            "pnpm-lock.yaml",
            "package-lock.json",
            "npm-shrinkwrap.json",
            "yarn.lock",
            "bun.lock",
            "bun.lockb",
        ],
    ),
    ("Pods", &["Podfile", "Podfile.lock"]),
    (
        ".gradle",
        &[
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            "gradle.properties",
            "gradle/libs.versions.toml",
            "gradle/wrapper/gradle-wrapper.properties",
        ],
    ),
    (".build", &["Package.swift", "Package.resolved"]),
];

/// How deep a package folder may sit below the root.
const MAX_DEPTH: usize = 3;

/// Marks a copy in progress: `<name>.brigadier-warming-<12 hex digits>` beside the worktree
/// (older ones: beside its destination).
const IN_PROGRESS: &str = ".brigadier-warming-";

/// A cache written to this recently may be mid-install or mid-build.
const RECENT_WRITE: Duration = Duration::from_secs(10);

/// How long warming one worktree may take, all its copies together.
const WARM_DEADLINE: Duration = Duration::from_secs(20);

/// A copy with more entries than this isn't checked (and so isn't published).
const MAX_ENTRIES: usize = 2_000_000;

/// A known file bigger than this can't be rewritten (and so the copy isn't published).
const MAX_REWRITE_BYTES: u64 = 4 << 20;

/// Text files up to this size are checked for the checkout's path, at any depth.
const MAX_PROBE_BYTES: u64 = 64 << 10;

/// How many files, and how many bytes, one copy may need read; past either it isn't published.
const MAX_PROBED_FILES: usize = 500_000;
const MAX_PROBED_BYTES: u64 = 2 << 30;

/// What warming did, for the log.
#[derive(Debug, Default)]
pub struct Warmed {
    /// Repo-relative folders copied in.
    pub copied: Vec<String>,
    /// Folders the checkout has that were left out, and why.
    pub skipped: Vec<(String, String)>,
}

/// Fills the new `worktree` of `source` (the user's checkout) with copies of its caches.
/// `aliases` are other spellings of the checkout's path (the one the project was added with);
/// `installing` says whether a package install runs in the checkout now ([`install_running`]).
pub fn warm_worktree(
    source: &Repo,
    aliases: &[PathBuf],
    worktree: &Repo,
    installing: &dyn Fn() -> bool,
) -> Warmed {
    warm_until(
        source,
        aliases,
        worktree,
        installing,
        Instant::now() + WARM_DEADLINE,
    )
}

fn warm_until(
    source: &Repo,
    aliases: &[PathBuf],
    worktree: &Repo,
    installing: &dyn Fn() -> bool,
    deadline: Instant,
) -> Warmed {
    let mut warmed = Warmed::default();
    // Where an install can't be seen (Windows) or a copy can't be published without possibly
    // replacing something, nothing is copied.
    if !clone::SUPPORTED {
        warmed
            .skipped
            .push((".".into(), "copies aren't made on this system".into()));
        return warmed;
    }
    // Both roots are canonical.
    let paths = Paths::new(source.root(), aliases, worktree.root());
    if paths.worktree.starts_with(&paths.source) || paths.source.starts_with(&paths.worktree) {
        warmed
            .skipped
            .push((".".into(), "the worktree and the checkout overlap".into()));
        return warmed;
    }
    let (tracked, in_worktree) = match (source.tracked_files(), worktree.tracked_files()) {
        (Ok(tracked), Ok(in_worktree)) => (tracked, in_worktree.into_iter().collect()),
        (Err(err), _) | (_, Err(err)) => {
            warmed.skipped.push((".".into(), err.to_string()));
            return warmed;
        }
    };
    let warm = Warm {
        source,
        worktree,
        paths: &paths,
        tracked: &in_worktree,
        installing,
        deadline,
    };
    if let Some(stage) = paths.worktree.parent() {
        clear_leftovers(stage, Path::new(""), &BTreeSet::new());
    }
    let mut cleared = BTreeSet::new();
    for rel in candidates(&package_dirs(&tracked)) {
        // Only real folders: a link would be copied as a link back into the checkout.
        if !fs::symlink_metadata(paths.source.join(&rel)).is_ok_and(|meta| meta.is_dir()) {
            continue;
        }
        if Instant::now() >= deadline {
            warmed.skipped.push((rel, "out of time".into()));
            continue;
        }
        let parent = parent_of(&rel);
        if cleared.insert(parent.to_owned()) {
            clear_leftovers(&paths.worktree, parent, &in_worktree);
        }
        match warm.one(&rel) {
            Ok(()) => warmed.copied.push(rel),
            Err(why) => warmed.skipped.push((rel, why)),
        }
    }
    warmed
}

/// What copying one cache needs.
struct Warm<'a> {
    source: &'a Repo,
    worktree: &'a Repo,
    paths: &'a Paths,
    /// The files the worktree tracks.
    tracked: &'a BTreeSet<String>,
    installing: &'a dyn Fn() -> bool,
    deadline: Instant,
}

impl Warm<'_> {
    /// Copies one cache folder, or says why not.
    fn one(&self, rel: &str) -> Result<(), String> {
        let ignored =
            |repo: &Repo, path: &str| repo.is_ignored(path).map_err(|err| err.to_string());
        // The worktree has no such folder yet, and git only matches a folder pattern
        // (`node_modules/`) against one it can see: so ask about something inside it.
        if !ignored(self.source, rel)? || !ignored(self.worktree, &format!("{rel}/{IN_PROGRESS}"))?
        {
            return Err("git doesn't ignore it".into());
        }
        if self.paths.worktree.join(rel).symlink_metadata().is_ok() || is_tracked(self.tracked, rel)
        {
            return Err("the worktree already has it".into());
        }
        let from = self.paths.source.join(rel);
        if let Some(env) = python_env(&self.paths.source, rel) {
            return Err(if env == rel {
                "it is a Python environment".into()
            } else {
                format!("it is inside a Python environment ({env})")
            });
        }
        let name = last_name(rel);
        let package = package_of(rel);
        if !same_dependencies(&self.paths.source, &self.paths.worktree, package, name) {
            return Err("the worktree's dependencies differ from the checkout's".into());
        }
        let busy = || name == "node_modules" && (self.installing)();
        let before = activity(&from, name);
        if busy()
            || before
                .iter()
                .any(|(_, modified)| is_recent(*modified, SystemTime::now()))
        {
            return Err(if name == "node_modules" {
                "an install is running".into()
            } else {
                "it is being written to".into()
            });
        }
        // Every folder on the way, on both sides, is a real one; the package folder exists in
        // the worktree (it is tracked), `.next` may not.
        let parent = parent_of(rel);
        let src = Folder::open(&self.paths.source, parent, false)
            .map_err(|err| format!("the checkout's folder: {err}"))?;
        Folder::open(&self.paths.worktree, Path::new(package), false)
            .map_err(|_| "the worktree has no such package folder")?;
        let dst = Folder::open(&self.paths.worktree, parent, true)
            .map_err(|err| format!("the worktree's folder: {err}"))?;
        // The copy is made beside the worktree, where git can't see it (nor `git add` take it)
        // while the session or worker already works there, then moved in.
        let stage = self
            .paths
            .worktree
            .parent()
            .ok_or("the worktree has no parent folder")?;
        let stage = Folder::open(stage, Path::new(""), false)
            .map_err(|err| format!("the worktree's parent folder: {err}"))?;
        let temp = format!(
            "{name}{IN_PROGRESS}{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        );
        let copy = stage.path().join(&temp);
        let result = (|| {
            {
                // Held for the whole copy: a build can't start writing halfway through.
                let locks = if name == "target" {
                    Some(lock_cargo(&from).ok_or("a build is running")?)
                } else {
                    None
                };
                stage
                    .clone_in(&src, OsStr::new(name), OsStr::new(&temp), self.deadline)
                    .map_err(|err| format!("copy failed: {err}"))?;
                // A build of a profile that had no lock yet may have started meanwhile.
                if let Some(locks) = &locks
                    && cargo_locks(&from) != locks.paths
                {
                    return Err("a build started during the copy".to_owned());
                }
            }
            let published = self.paths.worktree.join(rel);
            Settle {
                copy: &copy,
                from: &from,
                published: &published,
                name,
                paths: self.paths,
                deadline: self.deadline,
            }
            .run()?;
            if busy() || activity(&from, name) != before {
                return Err("it changed during the copy".into());
            }
            if Instant::now() >= self.deadline {
                return Err("out of time".into());
            }
            stage
                .move_new(OsStr::new(&temp), &dst, OsStr::new(name))
                .map_err(|err| format!("publishing failed: {err}"))
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&copy);
        }
        result
    }
}

/// Package folders among the tracked files: the root (`""`) and every folder up to
/// [`MAX_DEPTH`] down that holds a manifest.
fn package_dirs(tracked: &[String]) -> BTreeSet<String> {
    let mut dirs = BTreeSet::from([String::new()]);
    for path in tracked {
        let (dir, file) = path.rsplit_once('/').unwrap_or(("", path));
        if MANIFESTS.contains(&file) && (dir.is_empty() || dir.split('/').count() <= MAX_DEPTH) {
            dirs.insert(dir.to_owned());
        }
    }
    dirs
}

/// The repo-relative cache folders to look for in each package folder.
fn candidates(dirs: &BTreeSet<String>) -> Vec<String> {
    dirs.iter()
        .flat_map(|dir| {
            CACHES.iter().map(move |cache| match dir.as_str() {
                "" => (*cache).to_owned(),
                dir => format!("{dir}/{cache}"),
            })
        })
        .collect()
}

/// The folder from `rel` up (short of the checkout itself) that is a Python environment: one
/// holding a `pyvenv.cfg`.
fn python_env<'a>(source: &Path, rel: &'a str) -> Option<&'a str> {
    let mut folder = Some(rel).filter(|rel| !rel.is_empty());
    while let Some(rel) = folder {
        if source
            .join(rel)
            .join("pyvenv.cfg")
            .symlink_metadata()
            .is_ok()
        {
            return Some(rel);
        }
        folder = rel.rsplit_once('/').map(|(parent, _)| parent);
    }
    None
}

fn last_name(rel: &str) -> &str {
    rel.rsplit('/').next().unwrap_or(rel)
}

fn parent_of(rel: &str) -> &Path {
    Path::new(rel).parent().unwrap_or(Path::new(""))
}

/// The package folder a cache path belongs to.
fn package_of(rel: &str) -> &str {
    let cache = CACHES
        .iter()
        .find(|cache| rel == **cache || rel.ends_with(&format!("/{cache}")))
        .map_or(0, |cache| cache.len());
    rel[..rel.len() - cache].trim_end_matches('/')
}

/// Whether git tracks `rel` or anything in it.
fn is_tracked(tracked: &BTreeSet<String>, rel: &str) -> bool {
    tracked.contains(rel)
        || tracked
            .range(format!("{rel}/")..)
            .next()
            .is_some_and(|path| path.starts_with(&format!("{rel}/")))
}

/// Whether `name` is one Brigadier gave a copy in progress: `<cache>.brigadier-warming-` and
/// 12 lowercase hex digits.
fn is_leftover(name: &str) -> bool {
    name.split_once(IN_PROGRESS).is_some_and(|(cache, random)| {
        CACHES.iter().any(|known| last_name(known) == cache)
            && random.len() == 12
            && random
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// Removes copies a crash left half done in `worktree`'s folder `rel` (or beside the worktree:
/// `rel` empty, nothing tracked): only folders named as Brigadier names them that git doesn't
/// track.
fn clear_leftovers(worktree: &Path, rel: &Path, tracked: &BTreeSet<String>) {
    let Ok(folder) = Folder::open(worktree, rel, false) else {
        return;
    };
    let Ok(entries) = fs::read_dir(folder.path()) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let in_repo = rel.join(&name).to_string_lossy().into_owned();
        if is_leftover(&name)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && !is_tracked(tracked, &in_repo)
        {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

/// Whether the worktree has the files that decide what the cache `name` in `package` holds
/// exactly as the checkout does.
fn same_dependencies(source: &Path, worktree: &Path, package: &str, name: &str) -> bool {
    let Some((_, files)) = DEPENDENCY_FILES.iter().find(|(cache, _)| *cache == name) else {
        return true;
    };
    let mut dirs = vec![package];
    if name == "node_modules" && !package.is_empty() {
        dirs.push("");
    }
    // A file's content, a link's target, or nothing; anything else never matches.
    let read = |path: &Path| -> Result<Option<Vec<u8>>, ()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_file() => fs::read(path).map(Some).map_err(drop),
            Ok(meta) if meta.is_symlink() => fs::read_link(path)
                .map(|target| Some(target.into_os_string().into_encoded_bytes()))
                .map_err(drop),
            Ok(_) => Err(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(()),
        }
    };
    dirs.iter().all(|dir| {
        files.iter().all(|file| {
            let rel = Path::new(dir).join(file);
            matches!(
                (read(&source.join(&rel)), read(&worktree.join(&rel))),
                (Ok(a), Ok(b)) if a == b
            )
        })
    })
}

/// When the cache folder, each entry at its top and its package manager's state files last
/// changed: compared before and after copying, and none may be recent beforehand.
fn activity(dir: &Path, name: &str) -> Vec<(PathBuf, Option<SystemTime>)> {
    // Cargo's locks say when a `target` is busy.
    if name == "target" {
        return Vec::new();
    }
    let modified = |path: &Path| fs::symlink_metadata(path).ok()?.modified().ok();
    let mut seen = vec![(PathBuf::new(), modified(dir))];
    if let Ok(entries) = fs::read_dir(dir) {
        let mut top: Vec<_> = entries
            .flatten()
            .map(|entry| (PathBuf::from(entry.file_name()), modified(&entry.path())))
            .collect();
        top.sort();
        seen.extend(top);
    }
    if name == "node_modules" {
        let state = Path::new(".pnpm/lock.yaml");
        seen.push((state.to_owned(), modified(&dir.join(state))));
    }
    seen
}

fn is_recent(modified: Option<SystemTime>, now: SystemTime) -> bool {
    // A time in the future counts as recent: the clock can't be trusted to say otherwise.
    modified.is_some_and(|modified| {
        now.duration_since(modified)
            .map_or(true, |age| age < RECENT_WRITE)
    })
}

/// The `.cargo-lock` of every profile in a `target` folder (`target/debug`,
/// `target/<triple>/release`, …), held, or `None` when a build holds any of them.
struct CargoLocks {
    paths: BTreeSet<PathBuf>,
    _held: Vec<File>,
}

fn cargo_locks(target: &Path) -> BTreeSet<PathBuf> {
    let subfolders = |dir: &PathBuf| -> Vec<PathBuf> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| entry.path())
            .collect()
    };
    let mut dirs = vec![target.to_owned()];
    let mut level = dirs.clone();
    for _ in 0..2 {
        level = level.iter().flat_map(subfolders).collect();
        dirs.extend(level.iter().cloned());
    }
    dirs.into_iter()
        .map(|dir| dir.join(".cargo-lock"))
        .filter(|path| fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file()))
        .collect()
}

fn lock_cargo(target: &Path) -> Option<CargoLocks> {
    let paths = cargo_locks(target);
    let mut held = Vec::new();
    for path in &paths {
        let file = File::open(path).ok()?;
        file.try_lock().ok()?;
        held.push(file);
    }
    Some(CargoLocks { paths, _held: held })
}

/// How a file of some format writes a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    /// As it is.
    Raw,
    /// In a JSON string: `\` and `"` escaped.
    Json,
    /// In a JSON string with `/` escaped too.
    JsonSlash,
    /// In a Makefile rule (Cargo's and rustc's dep-info): spaces, `#` and `$` escaped.
    Make,
}

impl Style {
    const ALL: [Style; 4] = [Style::Raw, Style::Json, Style::JsonSlash, Style::Make];

    fn spell(self, path: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(path.len() + 8);
        for &byte in path {
            match (self, byte) {
                (Style::Json | Style::JsonSlash, b'\\' | b'"') => out.extend([b'\\', byte]),
                (Style::JsonSlash, b'/') => out.extend(b"\\/"),
                (Style::Make, b' ' | b'#') => out.extend([b'\\', byte]),
                (Style::Make, b'$') => out.extend(b"$$"),
                _ => out.push(byte),
            }
        }
        out
    }
}

/// How a known file is written, which decides how a path in it is spelled and where it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Plain,
    Json,
    Make,
}

impl Format {
    fn of(rel: &Path) -> Self {
        match rel.extension().and_then(OsStr::to_str) {
            Some("d") => Format::Make,
            Some("json") => Format::Json,
            _ => Format::Plain,
        }
    }

    fn styles(self) -> &'static [Style] {
        match self {
            Format::Plain => &[Style::Raw],
            Format::Json => &[Style::Json, Style::JsonSlash],
            Format::Make => &[Style::Make],
        }
    }
}

/// The checkout's path in each spelling, and the worktree that takes its place in a copy.
struct Paths {
    /// The checkout and the worktree, canonical.
    source: PathBuf,
    worktree: PathBuf,
    /// The checkout's path as tools may have written it, longest first (so `/private/var/x`
    /// is rewritten before `/var/x`).
    forms: Vec<Vec<u8>>,
    /// Each of them in every [`Style`], ready to be searched for.
    spellings: Vec<Finder<'static>>,
}

impl Paths {
    fn new(source: &Path, aliases: &[PathBuf], worktree: &Path) -> Self {
        let forms: Vec<Vec<u8>> = source_forms(source, aliases)
            .into_iter()
            .map(|form| form.into_os_string().into_encoded_bytes())
            .collect();
        let mut spellings: Vec<Finder<'static>> = Vec::new();
        for form in &forms {
            for style in Style::ALL {
                let spelled = style.spell(form);
                if !spellings.iter().any(|seen| seen.needle() == spelled) {
                    spellings.push(Finder::new(&spelled).into_owned());
                }
            }
        }
        Self {
            source: source.to_owned(),
            worktree: worktree.to_owned(),
            forms,
            spellings,
        }
    }

    /// The worktree's path in `style`; `None` when it holds a byte some format couldn't take
    /// as it is (a quote, `$`, `:`, a control character, …).
    fn worktree_in(&self, style: Style) -> Option<Vec<u8>> {
        let path = self.worktree.as_os_str().as_encoded_bytes();
        path.iter()
            .all(|&byte| {
                byte.is_ascii_alphanumeric() || b"/._-+@~% ".contains(&byte) || byte >= 0x80
            })
            .then(|| style.spell(path))
    }

    /// Whether `bytes`, a file of `format`, names the checkout in any spelling.
    fn named_in(&self, bytes: &[u8], format: Format) -> bool {
        self.spellings
            .iter()
            .any(|spelled| path_matches(bytes, spelled, format).next().is_some())
    }
}

/// The checkout's path and its aliases, longest first.
fn source_forms(source: &Path, aliases: &[PathBuf]) -> Vec<PathBuf> {
    let mut forms: Vec<PathBuf> = std::iter::once(source.to_owned())
        .chain(aliases.iter().cloned())
        .filter(|form| form.is_absolute() && form.components().count() > 1)
        .collect();
    forms.sort_by(|a, b| (b.as_os_str().len().cmp(&a.as_os_str().len())).then_with(|| a.cmp(b)));
    forms.dedup();
    forms
}

/// Checks a finished copy of `from` (a cache folder named `name`) at `copy`, to be published
/// at `published`: rewrites what points into the checkout to point into the worktree instead,
/// and fails when something still does or can't be checked.
struct Settle<'a> {
    copy: &'a Path,
    from: &'a Path,
    published: &'a Path,
    name: &'a str,
    paths: &'a Paths,
    deadline: Instant,
}

/// A file to read for the checkout's path: all of it, or only if it is small text.
type Probe = (PathBuf, bool);

impl Settle<'_> {
    /// Walks the copy, while a few readers check the files the walk hands them.
    fn run(&self) -> Result<(), String> {
        let (send, receive) = std::sync::mpsc::channel::<Probe>();
        let receive = Mutex::new(receive);
        let failed = Mutex::new(None::<String>);
        let readers = std::thread::available_parallelism().map_or(4, |count| count.get().min(8));
        let walked = std::thread::scope(|scope| {
            for _ in 0..readers {
                scope.spawn(|| self.read(&receive, &failed));
            }
            // Dropping the sender (also on failure) lets the readers finish.
            self.walk(send, &failed)
        });
        walked?;
        match failed.into_inner().unwrap_or_else(|err| err.into_inner()) {
            Some(why) => Err(why),
            None => Ok(()),
        }
    }

    fn walk(
        &self,
        probes: std::sync::mpsc::Sender<Probe>,
        failed: &Mutex<Option<String>>,
    ) -> Result<(), String> {
        let (mut seen, mut probed, mut probed_bytes) = (0usize, 0usize, 0u64);
        let mut pending = vec![PathBuf::new()];
        while let Some(dir) = pending.pop() {
            let entries = fs::read_dir(self.copy.join(&dir)).map_err(|err| err.to_string())?;
            for entry in entries {
                let entry = entry.map_err(|err| err.to_string())?;
                seen += 1;
                if seen > MAX_ENTRIES {
                    return Err("too many files to check".into());
                }
                if Instant::now() >= self.deadline {
                    return Err("out of time".into());
                }
                if failed
                    .lock()
                    .unwrap_or_else(|err| err.into_inner())
                    .is_some()
                {
                    return Ok(());
                }
                let rel = dir.join(entry.file_name());
                let file_name = entry.file_name().to_string_lossy().into_owned();
                // By name first: a linked one is no better.
                if file_name == "pyvenv.cfg" {
                    return Err(format!("{} is in a Python environment", rel.display()));
                }
                let kind = entry.file_type().map_err(|err| err.to_string())?;
                if kind.is_symlink() {
                    relink(
                        &self.copy.join(&rel),
                        &self.from.join(&rel),
                        &self.published.join(&rel),
                        self.paths,
                    )
                    .map_err(|why| format!("{}: {why}", rel.display()))?;
                } else if kind.is_dir() {
                    if file_name == ".venv" || file_name == "venv" {
                        return Err(format!("{} is a Python environment", rel.display()));
                    }
                    pending.push(rel);
                } else if !kind.is_file() {
                    // A pipe, socket or device: reading one could block forever.
                    return Err(format!("{} is not a regular file", rel.display()));
                } else {
                    let meta = entry.metadata().map_err(|err| err.to_string())?;
                    let Some(whole) = self
                        .file(&rel, &meta)
                        .map_err(|why| format!("{}: {why}", rel.display()))?
                    else {
                        continue;
                    };
                    probed += 1;
                    probed_bytes += meta.len();
                    if probed > MAX_PROBED_FILES || probed_bytes > MAX_PROBED_BYTES {
                        return Err("too much to check".into());
                    }
                    let _ = probes.send((rel, whole));
                }
            }
        }
        Ok(())
    }

    /// Rewrites one regular file if it is a known kind, or says whether to read it for the
    /// checkout's path: `Some(true)` all of it, `Some(false)` if it is small text.
    fn file(&self, rel: &Path, meta: &fs::Metadata) -> Result<Option<bool>, String> {
        let path = self.copy.join(rel);
        let executable = is_executable(meta);
        // A program Cargo built for the checkout, in a profile folder: one a person might run
        // by hand. Cargo builds it again when it is needed.
        if self.name == "target"
            && executable
            && path
                .parent()
                .is_some_and(|dir| dir.join(".cargo-lock").symlink_metadata().is_ok())
        {
            return fs::remove_file(&path)
                .map(|()| None)
                .map_err(|err| err.to_string());
        }
        if let Some(kind) = known_kind(self.name, rel)
            && rewrite_file(&path, meta, Format::of(rel), kind, self.paths)?
        {
            return Ok(None);
        }
        let in_bin = rel
            .parent()
            .is_some_and(|dir| dir.iter().any(|part| part == "bin" || part == ".bin"));
        let whole = self.name != "target" && (executable || in_bin);
        Ok((whole || meta.len() <= MAX_PROBE_BYTES).then_some(whole))
    }

    /// Reads the files the walk hands over until there are no more or one names the checkout.
    fn read(
        &self,
        probes: &Mutex<std::sync::mpsc::Receiver<Probe>>,
        failed: &Mutex<Option<String>>,
    ) {
        let fail = |why: String| {
            failed
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .get_or_insert(why);
        };
        loop {
            let next = probes.lock().unwrap_or_else(|err| err.into_inner()).recv();
            let Ok((rel, whole)) = next else {
                return;
            };
            if failed
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .is_some()
            {
                continue;
            }
            if Instant::now() >= self.deadline {
                fail("out of time".into());
                continue;
            }
            match names_checkout(&self.copy.join(&rel), whole, self.paths) {
                Ok(false) => {}
                Ok(true) => fail(format!("{} refers to the checkout", rel.display())),
                Err(why) => fail(format!("{}: {why}", rel.display())),
            }
        }
    }
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

/// Whether the file at `path` names the checkout. With `whole`, all of it is read, binary or
/// not; otherwise it is a small file, checked only if it is text.
fn names_checkout(path: &Path, whole: bool, paths: &Paths) -> Result<bool, String> {
    const CHUNK: usize = 1 << 20;
    let mut file = File::open(path).map_err(|err| err.to_string())?;
    if !whole {
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|err| err.to_string())?;
        return Ok(!bytes.contains(&0) && paths.named_in(&bytes, Format::Plain));
    }
    // Read in chunks, keeping the end of the last one: a mention may span two. One whose
    // following byte isn't read yet is looked at again with the next chunk.
    let keep = paths
        .spellings
        .iter()
        .map(|spelled| spelled.needle().len())
        .max()
        .unwrap_or(0)
        + 1;
    let mut window = Vec::with_capacity(CHUNK + keep);
    let mut chunk = vec![0u8; CHUNK];
    loop {
        let read = file.read(&mut chunk).map_err(|err| err.to_string())?;
        window.extend_from_slice(&chunk[..read]);
        let ended = read == 0;
        let named = paths.spellings.iter().any(|spelled| {
            path_matches(&window, spelled, Format::Plain)
                .any(|(at, _)| ended || at + spelled.needle().len() < window.len())
        });
        if named {
            return Ok(true);
        }
        if ended {
            return Ok(false);
        }
        let drop = window.len().saturating_sub(keep);
        window.drain(..drop);
    }
}

/// Points a copied link at the same place it pointed at in the checkout: its target is
/// resolved in full from where the original sits (`original`), through every link on the way.
/// A target in the checkout becomes the same place in the worktree, where the link will sit
/// at `published`; one elsewhere is kept by its real path. A link that leads nowhere (missing
/// or a loop) fails the copy, as does one to a Python environment.
fn relink(link: &Path, original: &Path, published: &Path, paths: &Paths) -> Result<(), String> {
    let text = fs::read_link(link).map_err(|err| err.to_string())?;
    let spelled = original.parent().unwrap_or(original).join(&text);
    let target =
        fs::canonicalize(&spelled).map_err(|err| format!("leads nowhere usable ({err})"))?;
    if target.file_name() == Some(OsStr::new("pyvenv.cfg"))
        || target.join("pyvenv.cfg").symlink_metadata().is_ok()
    {
        return Err("leads to a Python environment".into());
    }
    let new = match target.strip_prefix(&paths.source) {
        Ok(rest) => {
            let want = paths.worktree.join(rest);
            // A relative link that passes through no other link already means the same place
            // in the worktree.
            if text.is_relative()
                && lexical(&spelled) == target
                && lexical(&published.parent().unwrap_or(published).join(&text)) == want
            {
                return Ok(());
            }
            want
        }
        Err(_) if text == target => return Ok(()),
        Err(_) => target,
    };
    #[cfg(unix)]
    {
        fs::remove_file(link).map_err(|err| err.to_string())?;
        std::os::unix::fs::symlink(&new, link).map_err(|err| err.to_string())
    }
    #[cfg(not(unix))]
    {
        Err(format!(
            "{} can't be pointed at {}",
            link.display(),
            new.display()
        ))
    }
}

/// `..` and `.` resolved without looking at the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            part => out.push(part),
        }
    }
    out
}

/// How a known file must be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Known {
    /// It must be rewritten, or the copy is dropped.
    Must,
    /// Rewritten when it is text and not too big; otherwise checked like any other file.
    IfText,
}

/// Files known to hold the checkout's path, by where they sit in a cache folder named `name`:
/// package-manager shims and state, and Cargo's build-script outputs and dep-info.
fn known_kind(name: &str, rel: &Path) -> Option<Known> {
    let parts: Vec<String> = std::iter::once(name.to_owned())
        .chain(
            rel.components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned()),
        )
        .collect();
    let at = |back: usize| {
        parts
            .len()
            .checked_sub(back + 1)
            .map(|index| parts[index].as_str())
    };
    let file = at(0).unwrap_or_default();
    let extension = Path::new(file)
        .extension()
        .map(|ext| ext.to_string_lossy().into_owned())
        .unwrap_or_default();
    let must = match name {
        // Shims, and the package manager's own state beside the packages (`.modules.yaml`,
        // `.pnpm-workspace-state-v1.json`, `.package-lock.json`, …).
        "node_modules" => {
            (at(1) == Some(".bin") && at(2) == Some("node_modules"))
                || (at(1) == Some("node_modules")
                    && file.starts_with('.')
                    && ["json", "yaml", "yml"].contains(&extension.as_str()))
        }
        "target" => {
            ((file == "output" || file == "root-output") && at(2) == Some("build"))
                || extension == "d"
        }
        // Turbo's task logs.
        ".turbo" => extension == "log",
        _ => false,
    };
    if must {
        return Some(Known::Must);
    }
    // What build scripts write (`target/debug/build/<crate>/out/…`) and print: the paths in
    // it are where a build in the worktree would have put them.
    let build_script = name == "target"
        && parts.iter().enumerate().any(|(index, part)| {
            part == "build"
                && index + 2 < parts.len()
                && (parts[index + 2] == "out"
                    || (parts[index + 2] == "stderr" && index + 3 == parts.len()))
        });
    build_script.then_some(Known::IfText)
}

/// Rewrites a known file's mentions of the checkout to the worktree. Says whether it handled
/// the file: `false` for an [`Known::IfText`] file it leaves to the usual check. The new content
/// goes to a new file renamed over the old one, so a file hard-linked elsewhere is never
/// written through.
fn rewrite_file(
    path: &Path,
    meta: &fs::Metadata,
    format: Format,
    kind: Known,
    paths: &Paths,
) -> Result<bool, String> {
    if meta.len() > MAX_REWRITE_BYTES {
        return match kind {
            Known::Must => Err("too big to check".into()),
            Known::IfText => Ok(false),
        };
    }
    let bytes = fs::read(path).map_err(|err| err.to_string())?;
    if kind == Known::IfText && bytes.contains(&0) {
        return Ok(false);
    }
    let Some(bytes) = rewrite(&bytes, format, paths)? else {
        return Ok(true);
    };
    // A fresh name, created only if nothing is there: never a path a link already holds.
    let mut name = path.file_name().unwrap_or_default().to_owned();
    name.push(format!(
        ".brigadier-rewrite-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let fresh = path.with_file_name(name);
    // The old time is kept: build tools compare it with their outputs' (Cargo rebuilds what
    // depends on a build script whose `output` looks newer).
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&fresh)?;
        std::io::Write::write_all(&mut file, &bytes)?;
        file.set_permissions(meta.permissions())?;
        file.set_modified(meta.modified()?)?;
        drop(file);
        fs::rename(&fresh, path)
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&fresh);
        return Err(err.to_string());
    }
    Ok(true)
}

/// `bytes`, a file of `format`, with every mention of the checkout replaced by the worktree;
/// `None` when there is none. Fails when a mention can't be rewritten for sure (it may be the
/// start of a longer name, or the worktree's path can't be written in the format) or one is
/// left in a spelling the format doesn't use.
fn rewrite(bytes: &[u8], format: Format, paths: &Paths) -> Result<Option<Vec<u8>>, String> {
    let mut out = bytes.to_vec();
    let mut changed = false;
    for form in &paths.forms {
        for &style in format.styles() {
            let spelled = style.spell(form);
            let finder = Finder::new(&spelled);
            let found: Vec<(usize, Fit)> = path_matches(&out, &finder, format).collect();
            if found.is_empty() {
                continue;
            }
            if found.iter().any(|(_, fit)| *fit == Fit::Unsure) {
                return Err("names the checkout in a way that can't be rewritten".into());
            }
            let to = paths
                .worktree_in(style)
                .ok_or("the worktree's path can't be written in it")?;
            let mut next = Vec::with_capacity(out.len());
            let mut copied = 0;
            for (at, _) in found {
                // A match inside the one just replaced (a path that repeats itself) is already
                // gone.
                if at < copied {
                    continue;
                }
                next.extend_from_slice(&out[copied..at]);
                next.extend_from_slice(&to);
                copied = at + spelled.len();
            }
            next.extend_from_slice(&out[copied..]);
            out = next;
            changed = true;
        }
    }
    if paths.named_in(&out, format) {
        return Err("still names the checkout".into());
    }
    Ok(changed.then_some(out))
}

/// Bytes that can continue a file or folder name: a match followed or preceded by one is part
/// of a longer name (`/Users/me/app` in `/Users/me/app-old`), not the checkout's path.
fn name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"._-+@~%".contains(&byte)
}

/// Whether `bytes` ends with a terminal color code (`ESC [ 4 m`), as tools' logs put just
/// before a path.
fn ends_with_color(bytes: &[u8]) -> bool {
    let Some(rest) = bytes.strip_suffix(b"m") else {
        return false;
    };
    let digits = rest
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_digit() || **byte == b';')
        .count();
    rest[..rest.len() - digits].ends_with(b"\x1b[")
}

/// Whether a path can start right after `before`: at a byte that can't continue a name, after
/// a terminal color code, right after a one-letter command-line option that starts a word
/// (`-I/x`, `-L/x`, `-S/x`), as a C build's log gives compilers and CMake their folders, or
/// right after a build step's kind (`<CreateBuildDirectory-/x>`), as SwiftPM's build data names
/// its steps.
fn starts_path(before: &[u8]) -> bool {
    let free =
        |prev: Option<&u8>| prev.is_none_or(|prev| !(name_byte(*prev) || b"/\\".contains(prev)));
    free(before.last())
        || ends_with_color(before)
        || (matches!(before, [.., b'-', letter] if letter.is_ascii_alphabetic())
            && free(before.len().checked_sub(3).map(|index| &before[index])))
        || ends_with_step_kind(before)
}

/// Whether `before` ends with `<` and a build step's kind (`<CreateBuildDirectory-`).
fn ends_with_step_kind(before: &[u8]) -> bool {
    let Some(rest) = before.strip_suffix(b"-") else {
        return false;
    };
    let letters = rest
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_alphabetic())
        .count();
    letters > 0 && rest[..rest.len() - letters].ends_with(b"<")
}

/// How sure a mention of the checkout's path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fit {
    /// The whole path or the start of one, at component boundaries on both sides.
    Sure,
    /// Maybe the path, maybe part of a longer one (`/me/app backup`): it can't be rewritten,
    /// but counts when checking.
    Unsure,
}

/// Where `path` (one spelling) appears in `bytes`, a file of `format`, and how sure each is.
/// One followed by a byte that continues a name (`/me/app-old`, `/me/app\ backup` in a
/// Makefile) is another name and not listed.
fn path_matches<'a>(
    bytes: &'a [u8],
    path: &'a Finder<'a>,
    format: Format,
) -> impl Iterator<Item = (usize, Fit)> + 'a {
    let len = path.needle().len();
    path.find_iter(bytes).filter_map(move |at| {
        let after = &bytes[at + len..];
        let ends = match (format, after.first().copied(), after.get(1).copied()) {
            (_, None, _) | (_, Some(b'/'), _) => Some(true),
            (_, Some(next), _) if name_byte(next) => None,
            (Format::Json, Some(b'\\'), Some(b'/')) => Some(true),
            // In a Makefile, `\` escapes a byte of the name unless it ends the line.
            (Format::Make, Some(b'\\'), Some(b'\n' | b'\r') | None) => Some(true),
            (Format::Make, Some(b'\\' | b'$'), _) => None,
            (Format::Make, Some(b' ' | b'\t' | b':' | b'\n' | b'\r'), _) => Some(true),
            (Format::Plain | Format::Json, Some(next), _)
                if b"\"'`\n\r\t\0:;,)]}>|".contains(&next) =>
            {
                Some(true)
            }
            _ => Some(false),
        }?;
        let starts = starts_path(&bytes[..at]);
        Some((
            at,
            if ends && starts {
                Fit::Sure
            } else {
                Fit::Unsure
            },
        ))
    })
}

/// Whether a package install (`pnpm install`, `npm ci`, `yarn`, …) runs in `checkout` now.
/// `true` when it can't be told, so nothing is copied from under an install.
pub fn install_running(platform: &dyn Platform, checkout: &Path) -> bool {
    let Ok(pids) = platform.processes().in_dir(checkout) else {
        return true;
    };
    pids.into_iter()
        .filter_map(command_line)
        .any(|line| is_install(&line))
}

/// A process's command line, its arguments separated by spaces.
#[cfg(target_os = "linux")]
fn command_line(pid: u32) -> Option<String> {
    let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    Some(
        raw.split(|byte| *byte == 0)
            .map(String::from_utf8_lossy)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(all(unix, not(target_os = "linux")))]
fn command_line(pid: u32) -> Option<String> {
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

#[cfg(not(unix))]
fn command_line(_pid: u32) -> Option<String> {
    None
}

/// Whether a command line runs a package manager command that writes `node_modules`.
fn is_install(line: &str) -> bool {
    const MANAGERS: &[&str] = &[
        "npm",
        "npm-cli.js",
        "pnpm",
        "pnpm.cjs",
        "pnpm.js",
        "yarn",
        "yarn.js",
        "yarn.cjs",
        "bun",
    ];
    const WRITES: &[&str] = &[
        "install",
        "i",
        "add",
        "ci",
        "update",
        "up",
        "upgrade",
        "remove",
        "rm",
        "uninstall",
        "un",
        "rebuild",
        "link",
        "dedupe",
        "prune",
        "import",
    ];
    const OTHERS: &[&str] = &[
        "run", "exec", "dlx", "x", "test", "start", "build", "dev", "lint", "why", "list", "ls",
        "outdated", "audit", "view", "info", "init", "create", "publish", "pack",
    ];
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(at) = words
        .iter()
        .position(|word| MANAGERS.contains(&word.rsplit(['/', '\\']).next().unwrap_or(word)))
    else {
        return false;
    };
    let manager = words[at].rsplit(['/', '\\']).next().unwrap_or_default();
    let mut commands = words[at + 1..]
        .iter()
        .filter(|word| !word.starts_with('-'))
        .peekable();
    if commands.peek().is_none() {
        // A bare `yarn` installs.
        return manager.starts_with("yarn");
    }
    // The command may come after an option's value (`pnpm --filter web add x`): the first
    // known command decides.
    commands
        .find(|word| WRITES.contains(*word) || OTHERS.contains(*word))
        .is_some_and(|word| WRITES.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use brigadier_git::{Git, WorktreeSpec};
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    use std::ffi::OsString;

    #[test]
    fn package_folders_are_tracked_manifests_up_to_three_down() {
        let tracked = [
            "package.json",
            "apps/web/package.json",
            "a/b/c/Cargo.toml",
            "a/b/c/d/package.json",
            "docs/readme.md",
            "ios/Podfile",
        ]
        .map(String::from);
        let dirs = package_dirs(&tracked);
        assert_eq!(
            dirs.iter().map(String::as_str).collect::<Vec<_>>(),
            ["", "a/b/c", "apps/web", "ios"]
        );
        let found = candidates(&dirs);
        assert!(found.contains(&"node_modules".to_owned()));
        assert!(found.contains(&"apps/web/.next/cache".to_owned()));
        assert!(found.contains(&"a/b/c/target".to_owned()));
        assert_eq!(package_of("apps/web/.next/cache"), "apps/web");
        assert_eq!(package_of(".next/cache"), "");
        assert_eq!(package_of("node_modules"), "");
    }

    fn paths(source: &str, worktree: &str) -> Paths {
        Paths::new(Path::new(source), &[], Path::new(worktree))
    }

    fn rewritten(paths: &Paths, format: Format, text: &str) -> Result<Option<String>, String> {
        rewrite(text.as_bytes(), format, paths)
            .map(|out| out.map(|out| String::from_utf8(out).unwrap()))
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path spellings, not absolute paths on Windows")]
    fn only_whole_paths_are_rewritten() {
        let p = paths("/Users/me/app", "/data/worktrees/task-1");
        assert_eq!(
            rewritten(
                &p,
                Format::Plain,
                "NODE_PATH=\"/Users/me/app/node_modules/.pnpm\" /Users/me/app-old /Users/me/apps:/Users/me/app"
            )
            .unwrap()
            .unwrap(),
            "NODE_PATH=\"/data/worktrees/task-1/node_modules/.pnpm\" /Users/me/app-old /Users/me/apps:/data/worktrees/task-1"
        );
        assert!(p.named_in(
            b"cargo:rustc-link-search=/Users/me/app/target",
            Format::Plain
        ));
        assert!(!p.named_in(b"/Users/me/application", Format::Plain));
        assert_eq!(rewritten(&p, Format::Plain, "nothing here").unwrap(), None);
        // A space may end the path or continue its name: it can't be rewritten, and counts.
        assert!(rewritten(&p, Format::Plain, "cd /Users/me/app backup").is_err());
        assert!(p.named_in(b"/Users/me/app backup", Format::Plain));
        // Inside a longer path it is another folder, and counts too.
        assert!(rewritten(&p, Format::Plain, "/x/Users/me/app/y").is_err());
        // After a terminal color code, as in a build log.
        assert_eq!(
            rewritten(
                &p,
                Format::Plain,
                "config: \x1b[4m/Users/me/app/a.ts\x1b[24m"
            )
            .unwrap()
            .unwrap(),
            "config: \x1b[4m/data/worktrees/task-1/a.ts\x1b[24m"
        );
        assert!(rewritten(&p, Format::Plain, "\x1b[4mm/Users/me/app").is_err());
        // Right after a one-letter option, as compilers and CMake are given folders in a C
        // build's log (whisper-rs-sys prints all of it to its build-script output).
        assert_eq!(
            rewritten(
                &p,
                Format::Plain,
                "cmake -S/Users/me/app/out/src -B/Users/me/app/out/build\nc++ -DX=600 -I/Users/me/app/out/src/. -o x.o\n"
            )
            .unwrap()
            .unwrap(),
            "cmake -S/data/worktrees/task-1/out/src -B/data/worktrees/task-1/out/build\nc++ -DX=600 -I/data/worktrees/task-1/out/src/. -o x.o\n"
        );
        // An option that doesn't start a word is part of a longer name.
        assert!(rewritten(&p, Format::Plain, "x/-I/Users/me/app/y").is_err());
        assert!(rewritten(&p, Format::Plain, "a-I/Users/me/app/y").is_err());
        assert!(rewritten(&p, Format::Plain, "--I/Users/me/app/y").is_err());
        // After a build step's kind, as SwiftPM's build data (a Swift package a build script
        // builds) names its steps.
        assert_eq!(
            rewritten(
                &p,
                Format::Json,
                r#"{"inputs":["<CreateBuildDirectory-/Users/me/app/out/PCH>","/Users/me/app/out/a.o"]}"#
            )
            .unwrap()
            .unwrap(),
            r#"{"inputs":["<CreateBuildDirectory-/data/worktrees/task-1/out/PCH>","/data/worktrees/task-1/out/a.o"]}"#
        );
        assert!(rewritten(&p, Format::Json, r#""<x/Create-/Users/me/app/y>""#).is_err());
        assert!(rewritten(&p, Format::Json, r#""a-/Users/me/app/y""#).is_err());
        assert!(rewritten(&p, Format::Json, r#""<-/Users/me/app/y""#).is_err());
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path spellings, not absolute paths on Windows")]
    fn paths_are_rewritten_in_each_formats_own_spelling() {
        let p = paths(
            "/Users/me/my app",
            "/Users/me/Library/Application Support/B/wt",
        );
        // Dep-info escapes spaces; `\ ` after the path continues its name.
        assert_eq!(
            rewritten(
                &p,
                Format::Make,
                "/Users/me/my\\ app/target/debug/x.d: /Users/me/my\\ app/src/lib.rs /Users/me/my\\ app\\ backup/a.rs\n"
            )
            .unwrap()
            .unwrap(),
            "/Users/me/Library/Application\\ Support/B/wt/target/debug/x.d: /Users/me/Library/Application\\ Support/B/wt/src/lib.rs /Users/me/my\\ app\\ backup/a.rs\n"
        );
        // JSON with or without escaped slashes.
        assert_eq!(
            rewritten(
                &p,
                Format::Json,
                r#"{"a":"/Users/me/my app/x","b":"\/Users\/me\/my app\/y"}"#
            )
            .unwrap()
            .unwrap(),
            r#"{"a":"/Users/me/Library/Application Support/B/wt/x","b":"\/Users\/me\/Library\/Application Support\/B\/wt\/y"}"#
        );
        // An escaped spelling in a file of another format can't be rewritten.
        assert!(rewritten(&p, Format::Plain, r#"x="\/Users\/me\/my app\/y""#).is_err());
        assert!(p.named_in(br#""\/Users\/me\/my app\/y""#, Format::Plain));
        assert!(p.named_in(b"/Users/me/my\\ app/src", Format::Plain));
        // A worktree path no format can take as it is: nothing is rewritten.
        let q = paths("/Users/me/app", "/data/it's/wt");
        assert!(rewritten(&q, Format::Plain, "\"/Users/me/app/x\"").is_err());
        assert_eq!(rewritten(&q, Format::Plain, "elsewhere").unwrap(), None);
    }

    #[test]
    #[cfg_attr(windows, ignore = "Unix path spellings, not absolute paths on Windows")]
    fn longer_spellings_are_rewritten_first() {
        let forms = source_forms(
            Path::new("/private/var/me/app"),
            &[
                PathBuf::from("/var/me/app"),
                PathBuf::from("relative/app"),
                PathBuf::from("/private/var/me/app"),
            ],
        );
        assert_eq!(
            forms,
            [
                PathBuf::from("/private/var/me/app"),
                PathBuf::from("/var/me/app")
            ]
        );
    }

    #[test]
    fn known_kinds_are_package_shims_state_and_cargo_build_outputs() {
        let known = |name: &str, rel: &str| known_kind(name, Path::new(rel));
        let must = Some(Known::Must);
        assert_eq!(known("node_modules", ".bin/vite"), must);
        assert_eq!(
            known("node_modules", ".pnpm/vite@5/node_modules/.bin/esbuild"),
            must
        );
        assert_eq!(known("node_modules", ".modules.yaml"), must);
        assert_eq!(known("node_modules", ".pnpm-workspace-state-v1.json"), must);
        assert_eq!(known("node_modules", ".package-lock.json"), must);
        assert_eq!(known("node_modules", "vite/.state.json"), None);
        assert_eq!(known(".turbo", "turbo-build.log"), must);
        assert_eq!(known("node_modules", "vite/package.json"), None);
        assert_eq!(known("node_modules", "vite/.bin/x"), None);
        assert_eq!(known("target", "debug/build/ring-1a2b/output"), must);
        assert_eq!(known("target", "debug/build/ring-1a2b/root-output"), must);
        assert_eq!(
            known("target", "x86_64-apple-darwin/release/build/x-1/output"),
            must
        );
        assert_eq!(known("target", "debug/deps/brigadier_core-1a2b.d"), must);
        assert_eq!(
            known("target", "debug/build/tauri-1a2b/out/permissions/files"),
            Some(Known::IfText)
        );
        assert_eq!(
            known("target", "debug/build/whisper-1a2b/stderr"),
            Some(Known::IfText)
        );
        assert_eq!(known("target", "debug/output"), None);
        assert_eq!(known("Pods", "Manifest.lock"), None);
    }

    #[test]
    fn installs_are_told_from_other_package_commands() {
        assert!(is_install(
            "node /opt/homebrew/bin/pnpm install --frozen-lockfile"
        ));
        assert!(is_install(
            "/usr/local/bin/node /usr/local/lib/node_modules/npm/bin/npm-cli.js ci"
        ));
        assert!(is_install("pnpm --filter web add react"));
        assert!(is_install("node /Users/me/.yarn/bin/yarn.js"));
        assert!(!is_install("pnpm build"));
        assert!(!is_install("npm run dev"));
        assert!(!is_install("node server.js install"));
        assert!(!is_install("vim package.json"));
    }

    #[test]
    fn writes_within_ten_seconds_are_recent() {
        let now = SystemTime::now();
        assert!(is_recent(Some(now - Duration::from_secs(3)), now));
        assert!(is_recent(Some(now + Duration::from_secs(60)), now));
        assert!(!is_recent(Some(now - Duration::from_secs(30)), now));
        assert!(!is_recent(None, now));
    }

    #[test]
    fn only_brigadiers_own_temp_names_are_leftovers() {
        assert!(is_leftover("node_modules.brigadier-warming-0123456789ab"));
        assert!(is_leftover("cache.brigadier-warming-abcdef012345"));
        assert!(!is_leftover("docs.brigadier-warming-examples"));
        assert!(!is_leftover("node_modules.brigadier-warming-0123456789abc"));
        assert!(!is_leftover("node_modules.brigadier-warming-0123456789AB"));
        assert!(!is_leftover("src.brigadier-warming-0123456789ab"));
        let tracked: BTreeSet<String> = ["a/b.txt", "a-b/c", "ab"].map(String::from).into();
        assert!(is_tracked(&tracked, "a"));
        assert!(is_tracked(&tracked, "ab"));
        assert!(!is_tracked(&tracked, "a/b"));
        assert!(!is_tracked(&tracked, "c"));
    }

    /// A checkout with a commit, and a worktree of it, on a file system that can make
    /// copy-on-write copies (`None` elsewhere: the test has nothing to check there).
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    struct Fixture {
        dir: PathBuf,
        git: Git,
        repo: Repo,
        worktree: PathBuf,
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    impl Fixture {
        fn new(ignore: &str) -> Option<Self> {
            Self::with(ignore, |_| {})
        }

        /// `setup` adds to the checkout before its first commit.
        fn with(ignore: &str, setup: impl FnOnce(&Path)) -> Option<Self> {
            let dir = fs::canonicalize(std::env::temp_dir())
                .unwrap()
                .join(format!("brigadier-warm-{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir_all(&dir).unwrap();
            let config = dir.join("gitconfig");
            fs::write(&config, "").unwrap();
            let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
                .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
                .collect();
            env.push(("GIT_CONFIG_GLOBAL".into(), config.into_os_string()));
            for (key, value) in [
                ("GIT_CONFIG_NOSYSTEM", "1"),
                ("GIT_AUTHOR_NAME", "Test"),
                ("GIT_AUTHOR_EMAIL", "test@example.com"),
                ("GIT_COMMITTER_NAME", "Test"),
                ("GIT_COMMITTER_EMAIL", "test@example.com"),
            ] {
                env.push((key.into(), value.into()));
            }
            let git = Git::new(PathBuf::from("git"), env);
            let root = dir.join("checkout");
            assert!(git.init(&root).unwrap().is_none());
            let repo = git.open(&root).unwrap();
            fs::write(root.join(".gitignore"), ignore).unwrap();
            fs::write(root.join("package.json"), "{}").unwrap();
            fs::write(root.join("Cargo.toml"), "").unwrap();
            setup(&root);
            let commit = repo.commit_changes("Start", true).unwrap();
            let worktree = dir.join("worktree");
            repo.add_worktree(&worktree, WorktreeSpec::Detached { at: commit })
                .unwrap();
            let fixture = Self {
                dir,
                git,
                repo,
                worktree,
            };
            let folder = Folder::open(&fixture.dir, Path::new(""), false).unwrap();
            let clones = folder
                .clone_in(
                    &folder,
                    OsStr::new("gitconfig"),
                    OsStr::new("clone-check"),
                    Instant::now() + WARM_DEADLINE,
                )
                .is_ok();
            clones.then_some(fixture)
        }

        fn root(&self) -> &Path {
            self.repo.root()
        }

        fn write(&self, rel: &str, content: &[u8]) {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, content).unwrap();
        }

        fn link(&self, target: impl AsRef<Path>, rel: &str) {
            let path = self.root().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::os::unix::fs::symlink(target, path).unwrap();
        }

        /// Dates a cache folder and what is at its top a minute back, so it doesn't look
        /// mid-install.
        fn age(&self, rel: &str) {
            let past = SystemTime::now() - Duration::from_secs(60);
            let dir = self.root().join(rel);
            let mut paths = vec![dir.clone()];
            paths.extend(
                fs::read_dir(&dir)
                    .unwrap()
                    .flatten()
                    .map(|entry| entry.path()),
            );
            for path in paths {
                if !fs::symlink_metadata(&path).unwrap().is_symlink() {
                    File::open(path).unwrap().set_modified(past).unwrap();
                }
            }
        }

        fn warm_until(&self, deadline: Instant) -> Warmed {
            let worktree = self.git.open(&self.worktree).unwrap();
            warm_until(&self.repo, &[], &worktree, &|| false, deadline)
        }

        fn warm(&self) -> Warmed {
            self.warm_until(Instant::now() + WARM_DEADLINE)
        }

        fn why(warmed: &Warmed, rel: &str) -> String {
            warmed
                .skipped
                .iter()
                .find(|(path, _)| path == rel)
                .map(|(_, why)| why.clone())
                .unwrap_or_else(|| panic!("{rel} wasn't skipped: {warmed:?}"))
        }

        /// Copies in progress in the worktree, or beside it.
        fn leftovers(&self) -> Vec<String> {
            fs::read_dir(&self.worktree)
                .unwrap()
                .chain(fs::read_dir(self.worktree.parent().unwrap()).unwrap())
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.contains(IN_PROGRESS))
                .collect()
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn copies_are_separate_and_point_into_the_worktree() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        fx.write("node_modules/vite/index.js", b"export {}");
        fx.write(
            "node_modules/.bin/vite",
            format!("#!/bin/sh\nNODE_PATH=\"{source}/node_modules/.pnpm\" exec node \"$@\"\n")
                .as_bytes(),
        );
        fx.link(
            fx.root().join("node_modules/vite"),
            "node_modules/a/absolute",
        );
        fx.link("../vite", "node_modules/a/relative");
        // Through a link to the checkout itself, spelled by no form of its path.
        fx.link(fx.root(), "../alias");
        fx.link(
            fx.dir.join("alias/node_modules/vite"),
            "node_modules/a/aliased",
        );
        // A relative link that only gets there through another link.
        fx.link("absolute/index.js", "node_modules/a/chained");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["node_modules"], "{warmed:?}");
        let copy = fx.worktree.join("node_modules");
        let shim = fs::read_to_string(copy.join(".bin/vite")).unwrap();
        let worktree = fs::canonicalize(&fx.worktree).unwrap();
        assert!(shim.contains(&format!("{}/node_modules/.pnpm", worktree.display())));
        assert!(!shim.contains(&source));
        // Rewritten files keep their time, so build tools don't see them as new.
        let modified = |path: &Path| fs::metadata(path).unwrap().modified().unwrap();
        assert_eq!(
            modified(&copy.join(".bin/vite")),
            modified(&fx.root().join("node_modules/.bin/vite"))
        );
        let target = |name: &str| fs::read_link(copy.join("a").join(name)).unwrap();
        assert_eq!(target("absolute"), worktree.join("node_modules/vite"));
        assert_eq!(target("relative"), Path::new("../vite"));
        assert_eq!(target("aliased"), worktree.join("node_modules/vite"));
        assert_eq!(
            target("chained"),
            worktree.join("node_modules/vite/index.js")
        );
        // The checkout's own files are untouched, and the copy is a different file.
        assert!(
            fs::read_to_string(fx.root().join("node_modules/.bin/vite"))
                .unwrap()
                .contains(&source)
        );
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            fs::metadata(copy.join("vite/index.js")).unwrap().ino(),
            fs::metadata(fx.root().join("node_modules/vite/index.js"))
                .unwrap()
                .ino()
        );
        // A second warming leaves what is there alone.
        let again = fx.warm();
        assert!(again.copied.is_empty());
        assert_eq!(again.skipped[0].1, "the worktree already has it");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_rewrite_never_writes_through_a_link_in_the_copy() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        fx.write("important", b"keep me");
        fx.write(
            "node_modules/.bin/tool",
            format!("#!/bin/sh\nexec \"{source}/node_modules/tool/cli.js\"\n").as_bytes(),
        );
        // Where a fixed temp name would have been, a link to a file in the checkout.
        fx.link(
            fx.root().join("important"),
            "node_modules/.bin/tool.brigadier-rewrite",
        );
        fx.age("node_modules");
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["node_modules"], "{warmed:?}");
        assert_eq!(fs::read(fx.root().join("important")).unwrap(), b"keep me");
        assert!(
            !fs::read_to_string(fx.worktree.join("node_modules/.bin/tool"))
                .unwrap()
                .contains(&source)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_copy_that_still_refers_to_the_checkout_is_removed_whole() {
        let Some(fx) = Fixture::new("node_modules/\n.venv/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        fx.write("node_modules/a/index.js", b"export {}");
        // Deep down, where an install wrote the checkout's path.
        fx.write(
            "node_modules/.pnpm/a@1/node_modules/a/dist/config.js",
            format!("export const out = \"{source}/dist\";").as_bytes(),
        );
        fx.write(".venv/pyvenv.cfg", b"home = /usr/bin");
        fx.age("node_modules");
        // Copies a crash left behind, in the worktree or beside it, are cleared too.
        fs::create_dir_all(
            fx.worktree
                .join(format!("node_modules{IN_PROGRESS}0123456789ab/a")),
        )
        .unwrap();
        fs::create_dir_all(
            fx.worktree
                .parent()
                .unwrap()
                .join(format!("target{IN_PROGRESS}abcdef012345/debug")),
        )
        .unwrap();
        let warmed = fx.warm();
        assert!(warmed.copied.is_empty(), "{warmed:?}");
        assert_eq!(warmed.skipped.len(), 1, "{warmed:?}");
        assert!(warmed.skipped[0].1.contains("refers to the checkout"));
        assert!(!fx.worktree.join("node_modules").exists());
        assert!(!fx.worktree.join(".venv").exists());
        assert!(fx.leftovers().is_empty(), "{:?}", fx.leftovers());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn big_executables_and_bin_files_are_read_whole() {
        use std::os::unix::fs::PermissionsExt;
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        // Past the small-file size, with the path straddling two chunks.
        let mut big = vec![b'x'; (1 << 20) - 5];
        big.extend(format!("\0{source}/out\0").bytes());
        big.resize(3 << 20, 0);
        fx.write("node_modules/tool/native/run", &big);
        let run = fx.root().join("node_modules/tool/native/run");
        fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("refers to the checkout"));
        // Not executable, but in a `bin` folder.
        fs::set_permissions(&run, fs::Permissions::from_mode(0o644)).unwrap();
        fs::remove_dir_all(fx.root().join("node_modules/tool/native")).unwrap();
        fx.write("node_modules/tool/bin/run", &big);
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("refers to the checkout"));
        // Neither: a big binary file isn't read.
        fs::remove_dir_all(fx.root().join("node_modules/tool/bin")).unwrap();
        fx.write("node_modules/tool/data/blob", &big);
        fx.age("node_modules");
        assert_eq!(fx.warm().copied, ["node_modules"]);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn python_environments_inside_a_cache_drop_the_copy() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        fx.write("node_modules/tool/venv/pyvenv.cfg", b"home = /usr/bin");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(warmed.copied.is_empty(), "{warmed:?}");
        assert!(warmed.skipped[0].1.contains("Python environment"));
        assert!(!fx.worktree.join("node_modules").exists());
        assert!(fx.leftovers().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_package_folder_named_env_is_warmed_but_not_a_python_environment() {
        let Some(fx) = Fixture::with("node_modules/\n", |root| {
            for package in ["packages/env", "tools/venv"] {
                fs::create_dir_all(root.join(package)).unwrap();
                fs::write(root.join(package).join("package.json"), "{}").unwrap();
            }
        }) else {
            return;
        };
        for package in ["packages/env", "tools/venv"] {
            fx.write(&format!("{package}/node_modules/a/index.js"), b"export {}");
            fx.age(&format!("{package}/node_modules"));
        }
        // `tools/venv` is a Python environment too: a package inside it is left out, and said so.
        fx.write("tools/venv/pyvenv.cfg", b"home = /usr/bin");
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["packages/env/node_modules"], "{warmed:?}");
        assert!(
            fx.worktree
                .join("packages/env/node_modules/a/index.js")
                .exists()
        );
        assert_eq!(
            Fixture::why(&warmed, "tools/venv/node_modules"),
            "it is inside a Python environment (tools/venv)"
        );
        assert!(!fx.worktree.join("tools/venv/node_modules").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_linked_python_environment_drops_the_copy() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        let shared = fx.dir.join("shared-env");
        fs::create_dir_all(shared.join("bin")).unwrap();
        fs::write(shared.join("pyvenv.cfg"), "home = /usr/bin").unwrap();
        // A `pyvenv.cfg` that is itself a link.
        fx.write("node_modules/tool/env/bin/python", b"");
        fx.link(
            shared.join("pyvenv.cfg"),
            "node_modules/tool/env/pyvenv.cfg",
        );
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("Python environment"));
        // A link to a folder holding one.
        fs::remove_dir_all(fx.root().join("node_modules/tool")).unwrap();
        fx.link(&shared, "node_modules/tool/env");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("Python environment"));
        assert!(!fx.worktree.join("node_modules").exists());
        assert!(fx.leftovers().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn links_that_lead_nowhere_drop_the_copy() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        fx.write("node_modules/a/index.js", b"export {}");
        fx.link("missing", "node_modules/a/gone");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("leads nowhere"));
        fs::remove_file(fx.root().join("node_modules/a/gone")).unwrap();
        fx.link("loop-b", "node_modules/a/loop-a");
        fx.link("loop-a", "node_modules/a/loop-b");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("leads nowhere"));
        assert!(fx.leftovers().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_pipe_in_a_cache_drops_the_copy_without_blocking() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        fx.write("node_modules/a/index.js", b"export {}");
        let status = std::process::Command::new("mkfifo")
            .arg(fx.root().join("node_modules/a/install.pipe"))
            .status()
            .unwrap();
        assert!(status.success());
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("not a regular file"));
        assert!(!fx.worktree.join("node_modules").exists());
        assert!(fx.leftovers().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn nothing_is_done_through_a_linked_folder_in_the_worktree() {
        // The worktree's `.next` is a tracked link into the checkout.
        let Some(fx) = Fixture::with("cache/\n", |root| {
            fs::create_dir_all(root.join("real-next")).unwrap();
            fs::write(root.join("real-next/keep"), "").unwrap();
            std::os::unix::fs::symlink(root.join("real-next"), root.join(".next")).unwrap();
        }) else {
            return;
        };
        let leftover = format!("cache{IN_PROGRESS}0123456789ab");
        fs::create_dir_all(fx.root().join("real-next").join(&leftover)).unwrap();
        // The checkout's own `.next` is a real folder now.
        fs::remove_file(fx.root().join(".next")).unwrap();
        fx.write(".next/cache/a.json", b"{}");
        fx.age(".next/cache");
        let warmed = fx.warm();
        assert!(warmed.copied.is_empty(), "{warmed:?}");
        Fixture::why(&warmed, ".next/cache");
        assert!(fx.root().join("real-next").join(&leftover).is_dir());
        assert!(!fx.root().join("real-next/cache").exists());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn leftovers_are_only_brigadiers_untracked_temp_folders() {
        let tracked = format!("node_modules{IN_PROGRESS}0123456789ab");
        let Some(fx) = Fixture::with("node_modules/\n", |root| {
            for dir in [tracked.as_str(), "docs.brigadier-warming-examples"] {
                fs::create_dir_all(root.join(dir)).unwrap();
                fs::write(root.join(dir).join("README"), "").unwrap();
            }
        }) else {
            return;
        };
        let untracked = format!("node_modules{IN_PROGRESS}abcdef012345");
        fs::create_dir_all(fx.worktree.join(&untracked)).unwrap();
        fx.write("node_modules/a/index.js", b"export {}");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["node_modules"], "{warmed:?}");
        assert!(!fx.worktree.join(&untracked).exists());
        assert!(fx.worktree.join(&tracked).join("README").exists());
        assert!(
            fx.worktree
                .join("docs.brigadier-warming-examples/README")
                .exists()
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn different_dependencies_in_the_worktree_skip_the_cache() {
        let Some(fx) = Fixture::with("node_modules/\n", |root| {
            fs::write(root.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n").unwrap();
        }) else {
            return;
        };
        fx.write("node_modules/a/index.js", b"export {}");
        fx.age("node_modules");
        // The checkout has moved on: its lockfile isn't the worktree's.
        fx.write("pnpm-lock.yaml", b"lockfileVersion: '9.0'\npackages: {}\n");
        let warmed = fx.warm();
        assert!(Fixture::why(&warmed, "node_modules").contains("dependencies differ"));
        assert!(!fx.worktree.join("node_modules").exists());
        fx.write("pnpm-lock.yaml", b"lockfileVersion: '9.0'\n");
        assert_eq!(fx.warm().copied, ["node_modules"]);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_busy_build_or_a_fresh_install_is_skipped() {
        let Some(fx) = Fixture::new("target/\nnode_modules/\n.turbo/\n") else {
            return;
        };
        fx.write("target/debug/.cargo-lock", b"");
        fx.write("target/debug/deps/lib.d", b"");
        fx.write("node_modules/a/index.js", b"export {}");
        fx.write(".turbo/cache/a.json", b"{}");
        let build = File::open(fx.root().join("target/debug/.cargo-lock")).unwrap();
        build.lock().unwrap();
        let warmed = fx.warm();
        assert!(warmed.copied.is_empty(), "{warmed:?}");
        assert_eq!(Fixture::why(&warmed, "target"), "a build is running");
        assert_eq!(
            Fixture::why(&warmed, "node_modules"),
            "an install is running"
        );
        assert_eq!(Fixture::why(&warmed, ".turbo"), "it is being written to");
        assert!(fx.leftovers().is_empty());
        // An install that can be seen running.
        drop(build);
        fx.age("node_modules");
        fx.age(".turbo");
        let worktree = fx.git.open(&fx.worktree).unwrap();
        let warmed = warm_until(
            &fx.repo,
            &[],
            &worktree,
            &|| true,
            Instant::now() + WARM_DEADLINE,
        );
        assert_eq!(
            Fixture::why(&warmed, "node_modules"),
            "an install is running"
        );
        assert_eq!(warmed.copied, ["target", ".turbo"], "{warmed:?}");
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["node_modules"], "{warmed:?}");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn a_new_build_lock_during_the_copy_is_seen() {
        let Some(fx) = Fixture::new("target/\n") else {
            return;
        };
        fx.write("target/debug/.cargo-lock", b"");
        let target = fx.root().join("target");
        let held = lock_cargo(&target).unwrap();
        assert_eq!(cargo_locks(&target), held.paths);
        fx.write("target/release/.cargo-lock", b"");
        assert_ne!(cargo_locks(&target), held.paths);
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn programs_cargo_built_for_the_checkout_are_left_out() {
        use std::os::unix::fs::PermissionsExt;
        let Some(fx) = Fixture::new("target/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        fx.write("target/debug/.cargo-lock", b"");
        for exe in ["target/debug/app", "target/debug/deps/app-1a2b"] {
            fx.write(exe, format!("\0{source}\0").as_bytes());
            fs::set_permissions(fx.root().join(exe), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fx.write(
            "target/debug/deps/app-1a2b.d",
            format!("{source}/target/debug/deps/app-1a2b: {source}/src/main.rs\n").as_bytes(),
        );
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["target"], "{warmed:?}");
        let copy = fx.worktree.join("target/debug");
        assert!(!copy.join("app").exists());
        assert!(copy.join("deps/app-1a2b").exists());
        let worktree = fs::canonicalize(&fx.worktree).unwrap();
        assert_eq!(
            fs::read_to_string(copy.join("deps/app-1a2b.d")).unwrap(),
            format!(
                "{0}/target/debug/deps/app-1a2b: {0}/src/main.rs\n",
                worktree.display()
            )
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn c_and_swift_build_data_from_build_scripts_is_rewritten() {
        let Some(fx) = Fixture::new("target/\n") else {
            return;
        };
        let source = fx.root().to_string_lossy().into_owned();
        let out = format!("{source}/target/debug/build/whisper-rs-sys-1a2b/out");
        fx.write("target/debug/.cargo-lock", b"");
        // What whisper-rs-sys's build script prints: CMake's whole verbose build, then Cargo's
        // instructions.
        let output = |root: &str| {
            format!(
                "/opt/homebrew/bin/cmake -S{root}/whisper.cpp -B{root}/build --check-build-system CMakeFiles/Makefile.cmake 0\n\
                 cd {root}/build/ggml/src && /usr/bin/c++ -D_XOPEN_SOURCE=600 -I{root}/whisper.cpp/ggml/src/. -I{root}/whisper.cpp/ggml/src/../include -o ggml.c.o -c {root}/whisper.cpp/ggml/src/ggml.c\n\
                 cargo:rustc-link-search={root}/lib\n"
            )
        };
        fx.write(
            "target/debug/build/whisper-rs-sys-1a2b/output",
            output(&out).as_bytes(),
        );
        fx.write(
            "target/debug/build/whisper-rs-sys-1a2b/out/build/flags.make",
            format!("C_INCLUDES = -I{out}/whisper.cpp/include\n").as_bytes(),
        );
        // And what a Swift package built by a build script keeps (the computer-use helper's).
        let steps = |root: &str| {
            format!(r#"{{"inputs":["<CreateBuildDirectory-{root}/swift-build/PCH>"]}}"#)
        };
        fx.write(
            "target/debug/build/whisper-rs-sys-1a2b/out/swift-build/manifest.json",
            steps(&out).as_bytes(),
        );
        let warmed = fx.warm();
        assert_eq!(warmed.copied, ["target"], "{warmed:?}");
        let worktree = fs::canonicalize(&fx.worktree).unwrap();
        let moved = format!(
            "{}/target/debug/build/whisper-rs-sys-1a2b/out",
            worktree.display()
        );
        let copy = fx.worktree.join("target/debug/build/whisper-rs-sys-1a2b");
        assert_eq!(
            fs::read_to_string(copy.join("output")).unwrap(),
            output(&moved)
        );
        assert_eq!(
            fs::read_to_string(copy.join("out/build/flags.make")).unwrap(),
            format!("C_INCLUDES = -I{moved}/whisper.cpp/include\n")
        );
        assert_eq!(
            fs::read_to_string(copy.join("out/swift-build/manifest.json")).unwrap(),
            steps(&moved)
        );
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn warming_stops_at_its_deadline() {
        let Some(fx) = Fixture::new("node_modules/\n") else {
            return;
        };
        fx.write("node_modules/a/index.js", b"export {}");
        fx.age("node_modules");
        let warmed = fx.warm_until(Instant::now());
        assert!(warmed.copied.is_empty());
        assert_eq!(Fixture::why(&warmed, "node_modules"), "out of time");
        assert!(!fx.worktree.join("node_modules").exists());
        assert!(fx.leftovers().is_empty());
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn folders_git_does_not_ignore_are_left_alone() {
        let Some(fx) = Fixture::new("") else {
            return;
        };
        fx.write("node_modules/a/index.js", b"export {}");
        fx.age("node_modules");
        let warmed = fx.warm();
        assert!(warmed.copied.is_empty());
        assert_eq!(warmed.skipped[0].1, "git doesn't ignore it");
    }
}
