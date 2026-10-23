//! The files Claude Code keeps for a session, and removing exactly those.
//!
//! Everything is keyed by the session's UUID under Claude's configuration directory:
//! `projects/<encoded cwd>/<id>.jsonl` (the transcript) and `projects/<encoded cwd>/<id>/`,
//! plus `tasks/<id>`, `session-env/<id>`, `file-history/<id>`, `todos/<id>-*.json` and
//! `debug/<id>.txt`. Nothing else is touched.
//!
//! In the working directory, Claude Code stages its writes in `.claude/.cc-writes`; the part of
//! that path which did not exist before the session is removed with it, while it holds no files,
//! and already when the last of our CLIs in that directory exits, while it is empty
//! ([`CliInFolder`]).
//!
//! Claude also keeps per-session temp files in `<temp>/claude-<uid>/<encoded cwd>/<id>/`
//! (`<temp>` is `CLAUDE_CODE_TMPDIR`, else `/tmp` on macOS and the system temp directory
//! elsewhere), and its sandbox gives commands `<temp>/claude-<uid>` as their TMPDIR. A session
//! started with a TMPDIR of its own gets a short folder of its own as `<temp>`
//! ([`create_temp_dir`]), removed with everything in it, and leaves nothing in the shared
//! location; for the others the session's folder is removed with it, and the per-cwd folder
//! too when it is empty and belonged to a project directory the session created.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::model::Artifact;
use crate::{Error, Result};

/// Per-session directories and files directly under the config directory, by name.
const SESSION_DIRS: &[&str] = &["tasks", "session-env", "file-history"];
/// Where Claude Code stages atomic writes, under `<cwd>/.claude`.
const STAGING_DIR: &str = ".cc-writes";
/// Where a session's own temp folders live, and their name before the id.
#[cfg(unix)]
const TEMP_BASE: &str = "/tmp";
const TEMP_PREFIX: &str = "brigadier-";

pub(super) fn check_session_id(id: &str) -> Result<()> {
    uuid::Uuid::parse_str(id)
        .map(drop)
        .map_err(|_| Error::Invalid(format!("{id} is not a Claude session id")))
}

/// Where Claude keeps a working directory's sessions: every character that is not an ASCII
/// letter or digit becomes `-`.
pub(super) fn project_dir(config: &Path, cwd: &Path) -> PathBuf {
    let encoded: String = cwd
        .display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config.join("projects").join(encoded)
}

/// The directories Claude Code creates under `cwd` to stage writes, outermost first:
/// `.claude`, then `.claude/.cc-writes`.
pub(super) fn staging_dirs(cwd: &Path) -> [PathBuf; 2] {
    let claude = cwd.join(".claude");
    let staging = claude.join(STAGING_DIR);
    [claude, staging]
}

/// What of a project's `.claude` folder is never linked into a session's folder
/// ([`link_project`]): Claude's own staging and worktrees, and the instruction files the session
/// is given in its prompt already (`instructions` in the core), which would load twice.
#[cfg(any(unix, windows))]
const UNLINKED: &[&str] = &[
    STAGING_DIR,
    "worktrees",
    "CLAUDE.md",
    "CLAUDE.local.md",
    "rules",
    ".project-links.json",
];

/// Links `project`'s own Claude setup into `cwd`, a folder of Brigadier's where a thread or a
/// worker runs, since Claude reads a project's setup only from its working folder: its
/// `.mcp.json`, and what its `.claude` folder holds (settings, hooks, skills, agents, commands),
/// with the user's `settings.local.json` from their own checkout when a worktree has none (it
/// is gitignored). Links to what `project` lacks are removed, so a session that moves to
/// another checkout loads only that one's; only our links and unchanged copies are removed. A
/// file of Claude's own there is left as it is. On Unix each link is replaced in one rename: a CLI
/// starting meanwhile in the same folder (the session's workers share one) reads the old link
/// or the new one, never none. Windows falls back to junctions and tracked file copies.
#[cfg(any(unix, windows))]
pub(super) fn link_project(cwd: &Path, project: &Path) -> io::Result<()> {
    let _linking = LINKING.lock().unwrap_or_else(|e| e.into_inner());
    let dot_claude = cwd.join(".claude");
    let mut wanted: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mcp = project.join(".mcp.json");
    if mcp.is_file() {
        // Claude starts the project's servers in `cwd`: what they name in the project is made
        // absolute, in a copy of the file kept in the folder's `.claude`, where a session's
        // file tools can't edit it, and readable by the user alone (it may hold secrets).
        let target = match std::fs::read_to_string(&mcp)
            .ok()
            .and_then(|text| mcp_in_project(&text, project))
        {
            Some(text) => write_copy(cwd, MCP_COPY, &text)?,
            None => mcp,
        };
        wanted.push((cwd.join(".mcp.json"), target));
    }
    if let Ok(entries) = std::fs::read_dir(project.join(".claude")) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !UNLINKED.iter().any(|unlinked| name == *unlinked) {
                let target = settings_target(cwd, &name.to_string_lossy(), entry.path())?;
                wanted.push((dot_claude.join(&name), target));
            }
        }
    }
    let local = dot_claude.join("settings.local.json");
    if !wanted.iter().any(|(link, _)| *link == local)
        && let Some(main) = super::main_checkout(&project.join(".git"))
        && main.join(".claude/settings.local.json").is_file()
    {
        let target = settings_target(
            cwd,
            "settings.local.json",
            main.join(".claude/settings.local.json"),
        )?;
        wanted.push((local, target));
    }
    sync_links(cwd, &wanted)
}

/// What a link to the project's `.claude` entry `name` at `path` points to: a settings file
/// whose hooks run commands, a copy of it where they run in the CLI's project
/// ([`hooks_in_project`]); anything else, itself.
#[cfg(any(unix, windows))]
fn settings_target(cwd: &Path, name: &str, path: PathBuf) -> io::Result<PathBuf> {
    let Some((_, copy)) = SETTINGS_COPIES.iter().find(|(file, _)| *file == name) else {
        return Ok(path);
    };
    match std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| hooks_in_project(&text))
    {
        Some(text) => write_copy(cwd, copy, &text),
        None => Ok(path),
    }
}

/// Writes `text` to `cwd`'s copy `name` unless it holds it already, made anew (never through
/// what the session's commands left at its name) and readable by the user alone (it may hold
/// secrets); then where it is.
#[cfg(any(unix, windows))]
fn write_copy(cwd: &Path, name: &str, text: &str) -> io::Result<PathBuf> {
    let copy = cwd.join(name);
    if std::fs::read_to_string(&copy).ok().as_deref() != Some(text) {
        if let Some(parent) = copy.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = cwd.join(format!("{name}.{}.tmp", std::process::id()));
        let _ = std::fs::remove_file(&temp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&temp)?, text.as_bytes())?;
        std::fs::rename(&temp, &copy)?;
    }
    Ok(copy)
}

/// The project's settings files whose copies [`link_project`] keeps, and where.
#[cfg(any(unix, windows))]
const SETTINGS_COPIES: &[(&str, &str)] = &[
    ("settings.json", ".claude/.settings.project.json"),
    (
        "settings.local.json",
        ".claude/.settings.local.project.json",
    ),
];

/// Where a CLI's linked project is, for its hooks ([`hooks_in_project`]): each CLI has its own,
/// and workers sharing a folder have different ones.
pub(super) const PROJECT_DIR_ENV: &str = "BRIGADIER_PROJECT_DIR";

/// `text`, a project's Claude settings, with each command hook run in the project: Claude runs
/// hooks in the session's folder with `CLAUDE_PROJECT_DIR` set to it, so a hook that runs
/// `npm`, `git` or a script relative to the project would fail there (#315). Each command
/// first goes to the CLI's project ([`PROJECT_DIR_ENV`]), with `CLAUDE_PROJECT_DIR` set to it,
/// as in the user's own checkout; the copy names no project, so workers sharing a folder share
/// it. None when it has no command hooks (or isn't JSON).
#[cfg(any(unix, windows))]
fn hooks_in_project(text: &str) -> Option<String> {
    let mut settings: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut changed = false;
    let events = settings.get_mut("hooks")?.as_object_mut()?;
    for hook in events
        .values_mut()
        .filter_map(|groups| groups.as_array_mut())
        .flatten()
        .filter_map(|group| group.get_mut("hooks")?.as_array_mut())
        .flatten()
        .filter_map(|hook| hook.as_object_mut())
    {
        if hook.get("type").and_then(|kind| kind.as_str()) != Some("command") {
            continue;
        }
        let Some(command) = hook.get("command").and_then(|c| c.as_str()) else {
            continue;
        };
        let wrapped = format!(
            "cd \"${PROJECT_DIR_ENV}\" || exit 1\nexport CLAUDE_PROJECT_DIR=\"$PWD\"\n{command}"
        );
        #[cfg(windows)]
        let wrapped = if hook.get("shell").and_then(|s| s.as_str()) == Some("powershell") {
            format!(
                "Set-Location -LiteralPath $env:{PROJECT_DIR_ENV} -ErrorAction Stop\n$env:CLAUDE_PROJECT_DIR = (Get-Location).Path\n{command}"
            )
        } else {
            wrapped
        };
        hook.insert("command".into(), wrapped.into());
        changed = true;
    }
    changed.then(|| serde_json::to_string_pretty(&settings).ok())?
}

/// [`link_project`]'s copy of a project's `.mcp.json` whose local servers have paths in the
/// project, in the session's folder, where its `.mcp.json` links to it.
#[cfg(any(unix, windows))]
const MCP_COPY: &str = ".claude/.mcp.project.json";

/// `text`, a project's `.mcp.json`, with what its local servers name in the project made
/// absolute in `project`: `${CLAUDE_PROJECT_DIR}`, a command that is a file there
/// (`./scripts/server`), and the paths a script runner or such a command is given (`node
/// scripts/mcp.js`); none when there is nothing to change (or it isn't JSON). Other commands'
/// arguments (`docker run … mcp/server`, `npx -y @acme/mcp`) stay as they are written.
#[cfg(any(unix, windows))]
fn mcp_in_project(text: &str, project: &Path) -> Option<String> {
    let mut config: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut changed = false;
    let servers = config.get_mut("mcpServers")?.as_object_mut()?;
    for server in servers
        .values_mut()
        .filter_map(|server| server.as_object_mut())
    {
        if server
            .get("type")
            .is_some_and(|kind| kind.as_str() != Some("stdio"))
        {
            continue;
        }
        let Some(command) = server.get("command").and_then(|c| c.as_str()) else {
            continue;
        };
        let own = with_project_dir(command, project)
            .or_else(|| path_in_project(command, project).filter(|_| has_separator(command)));
        let paths = (own.is_some() || runs_scripts(command))
            && !server
                .get("args")
                .and_then(|args| args.as_array())
                .is_some_and(|args| {
                    args.iter()
                        .any(|arg| arg.as_str().is_some_and(picks_folder))
                });
        if let Some(own) = own {
            server.insert("command".into(), own.into());
            changed = true;
        }
        let Some(args) = server.get_mut("args").and_then(|args| args.as_array_mut()) else {
            continue;
        };
        for arg in args {
            let Some(text) = arg.as_str() else {
                continue;
            };
            let resolved = with_project_dir(text, project)
                .or_else(|| paths.then(|| path_in_project(text, project)).flatten());
            if let Some(resolved) = resolved {
                *arg = resolved.into();
                changed = true;
            }
        }
    }
    changed.then(|| serde_json::to_string_pretty(&config).ok())?
}

/// An argument that chooses the folder the runner reads its paths from (`uv --directory`):
/// those paths are left to it ([`mcp_in_project`]).
#[cfg(any(unix, windows))]
fn picks_folder(arg: &str) -> bool {
    const FLAGS: &[&str] = &[
        "--directory",
        "--cwd",
        "--prefix",
        "--project",
        "--cd",
        "-C",
    ];
    let flag = arg.split('=').next().unwrap_or(arg);
    FLAGS.contains(&flag)
}

/// A command that runs the script it is given ([`mcp_in_project`]).
#[cfg(any(unix, windows))]
fn runs_scripts(command: &str) -> bool {
    const RUNNERS: &[&str] = &[
        "node",
        "nodejs",
        "deno",
        "bun",
        "tsx",
        "ts-node",
        "uv",
        "ruby",
        "perl",
        "php",
        "sh",
        "bash",
        "zsh",
        "powershell",
        "pwsh",
    ];
    let name = Path::new(command)
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    #[cfg(windows)]
    let name = name.to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    RUNNERS.contains(&name) || name.starts_with("python")
}

/// `value` with `${CLAUDE_PROJECT_DIR}` or `${CLAUDE_PROJECT_DIR:-default}` in it made
/// `project`: Claude sets that variable to the folder it runs in, here the session's.
#[cfg(any(unix, windows))]
fn with_project_dir(value: &str, project: &Path) -> Option<String> {
    const VAR: &str = "${CLAUDE_PROJECT_DIR";
    if !value.contains(VAR) {
        return None;
    }
    let (mut out, mut rest) = (String::new(), value);
    while let Some(at) = rest.find(VAR) {
        let after = &rest[at + VAR.len()..];
        let end = match after.chars().next() {
            Some('}') => 0,
            Some(':') => after.find('}')?,
            _ => {
                out.push_str(&rest[..at + VAR.len()]);
                rest = after;
                continue;
            }
        };
        out.push_str(&rest[..at]);
        out.push_str(&project.display().to_string());
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[cfg(any(unix, windows))]
fn has_separator(value: &str) -> bool {
    value.contains('/') || (cfg!(windows) && value.contains('\\'))
}

/// `value` made absolute in `project` when it is a path there ([`mcp_in_project`]): a file
/// (`scripts/mcp.js`, `server.py`), or anything named from the folder (`.`, `./data`); never a
/// flag or a bare word (`run`), nor a folder named like a package (`mcp/server`).
#[cfg(any(unix, windows))]
fn path_in_project(value: &str, project: &Path) -> Option<String> {
    if value.is_empty() || value.starts_with('-') || Path::new(value).is_absolute() {
        return None;
    }
    let explicit = value == "."
        || value == ".."
        || value.starts_with("./")
        || value.starts_with("../")
        || (cfg!(windows) && (value.starts_with(".\\") || value.starts_with("..\\")));
    let relative = value.trim_start_matches("./");
    let full = match relative {
        "" | "." => project.to_path_buf(),
        _ => project.join(relative),
    };
    // A bare word (a subcommand such as `run`) is never taken for a file.
    let found = if explicit {
        full.exists()
    } else {
        (has_separator(value) || value.contains('.')) && full.is_file()
    };
    found.then(|| full.display().to_string())
}

/// Takes [`link_project`]'s links out of `cwd` for a session that may not load a project's
/// setup (its folder's trust was taken back), so the links left from an earlier start don't
/// load it anyway. Only in a folder of Brigadier's outside any repository: a checkout's own
/// links are the user's.
#[cfg(any(unix, windows))]
pub(super) fn unlink_project(cwd: &Path) -> io::Result<()> {
    if cwd.ancestors().any(|dir| dir.join(".git").exists()) {
        return Ok(());
    }
    let _linking = LINKING.lock().unwrap_or_else(|e| e.into_inner());
    sync_links(cwd, &[])
}

/// One folder's links made or removed at a time: a session's workers share their folder and
/// can start together.
#[cfg(any(unix, windows))]
static LINKING: Mutex<()> = Mutex::new(());

/// Makes `cwd`'s links to a project's setup exactly `wanted` (link, target).
#[cfg(unix)]
fn sync_links(cwd: &Path, wanted: &[(PathBuf, PathBuf)]) -> io::Result<()> {
    let dot_claude = cwd.join(".claude");
    // Links of an earlier project that this one lacks.
    let stale = |dir: &Path, only: Option<&str>| -> io::Result<()> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Ok(());
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type()?.is_symlink()
                && only.is_none_or(|name| entry.file_name() == name)
                && !wanted.iter().any(|(link, _)| *link == path)
            {
                std::fs::remove_file(&path)?;
            }
        }
        Ok(())
    };
    stale(cwd, Some(".mcp.json"))?;
    stale(&dot_claude, None)?;
    if wanted.iter().any(|(link, _)| link.starts_with(&dot_claude)) {
        std::fs::create_dir_all(&dot_claude)?;
    }
    for (link, target) in wanted {
        match std::fs::symlink_metadata(link) {
            Ok(meta) if !meta.file_type().is_symlink() => continue,
            Ok(_) if std::fs::read_link(link)? == *target => continue,
            _ => {}
        }
        let name = link.file_name().unwrap_or_default().to_string_lossy();
        let temp = link.with_file_name(format!(".{name}.{}.link", std::process::id()));
        let _ = std::fs::remove_file(&temp);
        std::os::unix::fs::symlink(target, &temp)?;
        std::fs::rename(&temp, link)?;
    }
    // A copy of an `.mcp.json` or a settings file that no link reads any more.
    for name in std::iter::once(MCP_COPY).chain(SETTINGS_COPIES.iter().map(|(_, copy)| *copy)) {
        let copy = cwd.join(name);
        if !wanted.iter().any(|(_, target)| *target == copy) {
            match std::fs::remove_file(&copy) {
                Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn link_project(_cwd: &Path, _project: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn unlink_project(_cwd: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows::sync_links;

/// Our live CLIs per working directory, and the outermost staging directory one of them
/// created there.
static LIVE: Mutex<BTreeMap<PathBuf, Live>> = Mutex::new(BTreeMap::new());

struct Live {
    clis: usize,
    ours: Option<PathBuf>,
}

/// A CLI of ours about to start in `cwd`, registered before it is spawned; `ours`: the staging
/// directory it records as created by us (`.claude`, or `.claude/.cc-writes` under a `.claude`
/// that was there), if any. Dropped once it exited (or failed to start): see [`CliInFolder`].
pub(super) fn cli_started(cwd: &Path, ours: Option<PathBuf>) -> CliInFolder {
    let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());
    let entry = live.entry(cwd.to_path_buf()).or_insert(Live {
        clis: 0,
        ours: None,
    });
    entry.clis += 1;
    // `.claude` itself wins over the folder in it.
    if let Some(ours) = ours
        && entry
            .ours
            .as_ref()
            .is_none_or(|known| !ours.starts_with(known))
    {
        entry.ours = Some(ours);
    }
    CliInFolder(cwd.to_path_buf())
}

/// One of our CLIs in a folder, from before it is spawned until it exited. The last one there
/// to go removes the staging directories ours while they are empty: an empty
/// `.claude/.cc-writes` is not left in the workspace until the session is deleted. They are
/// removed under the registry's lock, so no CLI starts there meanwhile; and only with `rmdir`:
/// a file staged by anyone else keeps its folder.
pub(super) struct CliInFolder(PathBuf);

impl Drop for CliInFolder {
    fn drop(&mut self) {
        let cwd = &self.0;
        let mut live = LIVE.lock().unwrap_or_else(|p| p.into_inner());
        let Some(entry) = live.get_mut(cwd) else {
            return;
        };
        entry.clis = entry.clis.saturating_sub(1);
        if entry.clis > 0 {
            return;
        }
        let Some(ours) = live.remove(cwd).and_then(|entry| entry.ours) else {
            return;
        };
        let [claude, staging] = staging_dirs(cwd);
        let _ = std::fs::remove_dir(&staging);
        if ours == claude {
            let _ = std::fs::remove_dir(&claude);
        }
    }
}

pub(super) fn remove(config: &Path, artifacts: &[Artifact]) -> Result<()> {
    let mut failures = Vec::new();
    // Sessions first: a project directory can only go once the sessions in it are gone.
    let sessions = artifacts.iter().filter_map(|artifact| match artifact {
        Artifact::ClaudeSession {
            session_id, home, ..
        } => Some(
            remove_session(config, session_id).and_then(|()| match home {
                // An extra account's own per-session state; its transcript is in `config`.
                Some(home) if Path::new(home) != config && Path::new(home).is_dir() => {
                    remove_session(Path::new(home), session_id)
                }
                _ => Ok(()),
            }),
        ),
        _ => None,
    });
    let dirs = artifacts.iter().filter_map(|artifact| match artifact {
        Artifact::ClaudeProjectDir { path } => Some(
            remove_project_dir(config, Path::new(path))
                .and_then(|()| remove_temp_project_dir(config, Path::new(path))),
        ),
        _ => None,
    });
    let staging = artifacts.iter().filter_map(|artifact| match artifact {
        Artifact::ClaudeMemoryDir {
            path,
            identity,
            project_identity,
        } => Some(super::memory::remove(
            config,
            Path::new(path),
            *identity,
            *project_identity,
        )),
        Artifact::ClaudeStagingDir { path } => Some(remove_staging_dir(Path::new(path))),
        Artifact::ClaudeTempDir { path } => Some(remove_temp_dir(Path::new(path))),
        _ => None,
    });
    for result in sessions
        .collect::<Vec<_>>()
        .into_iter()
        .chain(dirs)
        .chain(staging)
    {
        if let Err(err) = result {
            failures.push(err.to_string());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(Error::Io(io::Error::other(failures.join("; "))))
    }
}

/// Whether the session's saved transcript (`projects/<project>/<id>.jsonl`) is known to be gone:
/// the config's projects folder exists and no project holds it. Unknown (no projects folder,
/// an unreadable one) is not missing.
pub(super) fn session_missing(config: &Path, id: &str) -> bool {
    if check_session_id(id).is_err() {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(config.join("projects")) else {
        return false;
    };
    let file = format!("{id}.jsonl");
    !entries
        .flatten()
        .any(|entry| entry.path().join(&file).is_file())
}

fn remove_session(config: &Path, id: &str) -> Result<()> {
    check_session_id(id)?;
    let projects = config.join("projects");
    if let Ok(entries) = std::fs::read_dir(&projects) {
        for entry in entries.flatten() {
            let dir = entry.path();
            remove_path(&dir.join(format!("{id}.jsonl")))?;
            remove_path(&dir.join(id))?;
        }
    }
    for name in SESSION_DIRS {
        remove_path(&config.join(name).join(id))?;
    }
    remove_path(&config.join("debug").join(format!("{id}.txt")))?;
    if let Some(temp) = temp_root(config)
        && let Ok(entries) = std::fs::read_dir(&temp)
    {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                remove_path(&entry.path().join(id))?;
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(config.join("todos")) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with(&format!("{id}-")) {
                remove_path(&entry.path())?;
            }
        }
    }
    Ok(())
}

/// Removes a project directory the session created, but only if nothing else lives in it now
/// (the user may have started their own session there meanwhile).
fn remove_project_dir(config: &Path, dir: &Path) -> Result<()> {
    if dir.parent() != Some(config.join("projects").as_path()) {
        return Err(Error::Invalid(format!(
            "{} is not a Claude project directory",
            dir.display()
        )));
    }
    let memory = dir.join("memory");
    if is_empty_dir(&memory) {
        std::fs::remove_dir(&memory)?;
    }
    if is_empty_dir(dir) {
        std::fs::remove_dir(dir)?;
    }
    Ok(())
}

/// A new temp folder for one session, private to the user. Its path is kept short: Claude's
/// sandbox makes sockets under `<temp>/claude-<uid>`, and gives commands that folder as their
/// TMPDIR only while its path fits them (about 30 characters for `<temp>` on macOS; a data
/// directory's scratch folder never does). Otherwise their TMPDIR is the shared
/// `/tmp/claude-<uid>`, which nothing cleans up. Returns `None` where Claude has no sandbox.
pub(super) fn temp_dir_path() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        let id = uuid::Uuid::new_v4().simple().to_string();
        Some(Path::new(TEMP_BASE).join(format!("{TEMP_PREFIX}{}", &id[..12])))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Creates the folder from [`temp_dir_path`], which must not exist yet, marked as `paths`'
/// data directory's ([`brigadier_sandbox::OWNER_MARKER`]), so that cleanup can tell it apart
/// from other Brigadier instances' folders.
pub(super) fn create_temp_dir(dir: &Path, paths: &brigadier_sandbox::AppPaths) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        let failed = |err: io::Error| {
            Error::Io(io::Error::new(
                err.kind(),
                format!("{}: {err}", dir.display()),
            ))
        };
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(failed)?;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(dir.join(brigadier_sandbox::OWNER_MARKER))
            .and_then(|mut file| file.write_all(paths.owner_marker().as_bytes()))
            .map_err(failed)
    }
    #[cfg(not(unix))]
    {
        let _ = (dir, paths);
        Ok(())
    }
}

/// Removes a session's temp folder with everything in it.
fn remove_temp_dir(dir: &Path) -> Result<()> {
    let ours = dir
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.strip_prefix(TEMP_PREFIX)
                .is_some_and(|id| id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit()))
        });
    #[cfg(unix)]
    let ours = ours && dir.parent() == Some(Path::new(TEMP_BASE));
    if !ours {
        return Err(Error::Invalid(format!(
            "{} is not a Claude session's temp folder",
            dir.display()
        )));
    }
    remove_path(dir)
}

/// Claude's shared temp folder for this user (see the module docs).
fn temp_root(config: &Path) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // The configuration directory is the user's own, so its owner is the uid Claude uses.
        let uid = std::fs::metadata(config).ok()?.uid();
        let base = if cfg!(target_os = "macos") {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        Some(base.join(format!("claude-{uid}")))
    }
    #[cfg(not(unix))]
    {
        let _ = config;
        Some(std::env::temp_dir().join("claude"))
    }
}

/// Removes the per-cwd temp folder of a project directory the session created, while empty.
fn remove_temp_project_dir(config: &Path, project: &Path) -> Result<()> {
    let (Some(temp), Some(name)) = (temp_root(config), project.file_name()) else {
        return Ok(());
    };
    let dir = temp.join(name);
    if is_empty_dir(&dir) {
        std::fs::remove_dir(&dir)?;
    }
    Ok(())
}

/// Removes a staging directory the session created, unless it holds a file: then someone put
/// something there that is not Claude's scratch, and it stays.
fn remove_staging_dir(dir: &Path) -> Result<()> {
    let is_staging = match dir.file_name().and_then(|name| name.to_str()) {
        Some(".claude") => true,
        Some(STAGING_DIR) => dir.parent().and_then(Path::file_name) == Some(".claude".as_ref()),
        _ => false,
    };
    if !is_staging {
        return Err(Error::Invalid(format!(
            "{} is not a Claude staging directory",
            dir.display()
        )));
    }
    if holds_only_dirs(dir) {
        remove_path(dir)?;
    }
    Ok(())
}

/// Whether `dir` is a directory tree with no files (or symlinks) anywhere in it.
fn holds_only_dirs(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => std::fs::read_dir(dir).is_ok_and(|entries| {
            entries
                .into_iter()
                .all(|entry| entry.is_ok_and(|entry| holds_only_dirs(&entry.path())))
        }),
        _ => false,
    }
}

fn is_empty_dir(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

fn remove_path(path: &Path) -> Result<()> {
    let result = match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path),
        Ok(_) => std::fs::remove_file(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => Err(err),
    };
    match result {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(io::Error::new(
            err.kind(),
            format!("{}: {err}", path.display()),
        ))),
    }
}

/// Lines of a transcript read for its working folder and how it was started.
const HEAD_LINES: usize = 64;
/// A line longer than this is skipped, not read on (a pasted image can be megabytes).
const LINE_LIMIT: u64 = 16 * 1024 * 1024;
/// Brigadier's tools, as Claude Code names an MCP server's tools.
const BRIGADIER_TOOLS: &str = "mcp__brigadier__";
/// A Brigadier daemon's socket, under its data directory.
const DAEMON_SOCKET: &str = "/run/brigadierd.sock";

/// Claude sessions whose working folder `wanted` accepts, with their files: the transcript
/// and its folder in `<history>/projects/<encoded cwd>/`, and in every home the per-session
/// state Claude keeps there. The project folder comes last, removed only once empty.
pub(crate) fn leftovers(
    history: &Path,
    homes: &[PathBuf],
    wanted: &dyn Fn(&Path) -> bool,
) -> Vec<crate::leftovers::FoundSession> {
    use crate::leftovers::{FoundEntry, FoundSession, is_entry, last_change};
    let projects = history.join("projects");
    let Ok(folders) = std::fs::read_dir(&projects) else {
        return Vec::new();
    };
    // The same folder reached twice (a home given twice, or through a link) is read once.
    let mut seen = std::collections::HashSet::new();
    let homes: Vec<&PathBuf> = homes
        .iter()
        .filter(|home| seen.insert(home.canonicalize().unwrap_or_else(|_| (*home).clone())))
        .collect();
    let mut found = Vec::new();
    let mut folders: Vec<PathBuf> = folders
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    folders.sort();
    for folder in folders {
        let Ok(files) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut transcripts: Vec<PathBuf> = files
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect();
        transcripts.sort();
        for transcript in transcripts {
            let Some(id) = transcript
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|id| check_session_id(id).is_ok())
                .map(str::to_owned)
            else {
                continue;
            };
            let Some((cwd, headless)) = head(&transcript) else {
                continue;
            };
            if !wanted(&cwd) {
                continue;
            }
            let fingerprint = if headless {
                fingerprint(&transcript)
            } else {
                None
            };
            let mut entries = vec![FoundEntry {
                root: projects.clone(),
                path: transcript.clone(),
                empty_only: false,
            }];
            let own = folder.join(&id);
            if is_entry(&own) {
                entries.push(FoundEntry {
                    root: projects.clone(),
                    path: own,
                    empty_only: false,
                });
            }
            for home in &homes {
                let mut state: Vec<PathBuf> = SESSION_DIRS
                    .iter()
                    .map(|name| home.join(name).join(&id))
                    .chain([home.join("debug").join(format!("{id}.txt"))])
                    .collect();
                if let Ok(todos) = std::fs::read_dir(home.join("todos")) {
                    let prefix = format!("{id}-");
                    let mut named: Vec<PathBuf> = todos
                        .flatten()
                        .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                        .map(|entry| entry.path())
                        .collect();
                    named.sort();
                    state.extend(named);
                }
                entries.extend(state.into_iter().filter(|path| is_entry(path)).map(|path| {
                    FoundEntry {
                        root: (*home).clone(),
                        path,
                        empty_only: false,
                    }
                }));
            }
            entries.push(FoundEntry {
                root: projects.clone(),
                path: folder.clone(),
                empty_only: true,
            });
            let last_change = entries
                .iter()
                .filter(|entry| !entry.empty_only)
                .filter_map(|entry| last_change(&entry.path))
                .max();
            found.push(FoundSession {
                kind: crate::model::ProviderKind::Claude,
                id,
                cwd: Some(cwd),
                fingerprint,
                entries,
                last_change,
            });
        }
    }
    found
}

/// A transcript's working folder, and whether a program started it (`entrypoint` `sdk-cli`,
/// as stream-json sessions say), from its first lines.
fn head(transcript: &Path) -> Option<(PathBuf, bool)> {
    let mut reader = std::io::BufReader::new(std::fs::File::open(transcript).ok()?);
    for _ in 0..HEAD_LINES {
        let line = read_line(&mut reader)?;
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(serde_json::Value::as_str) {
            let headless =
                value.get("entrypoint").and_then(serde_json::Value::as_str) == Some("sdk-cli");
            return Some((PathBuf::from(cwd), headless));
        }
    }
    None
}

/// One line, at most [`LINE_LIMIT`] long (the rest of a longer one is skipped); `None` at the
/// end.
fn read_line(reader: &mut impl std::io::BufRead) -> Option<String> {
    use std::io::{BufRead as _, Read as _};
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(LINE_LIMIT)
        .read_until(b'\n', &mut line)
        .ok()?;
    if read == 0 {
        return None;
    }
    if line.last() != Some(&b'\n') && read as u64 == LINE_LIMIT {
        // Skip the rest of a line too long to read.
        let mut rest = Vec::new();
        let _ = reader.read_until(b'\n', &mut rest);
        return Some(String::new());
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Brigadier's marks in a transcript: a call of one of its tools, or the sandbox letting the
/// session reach a Brigadier daemon's socket. Read from the parsed records, never from text a
/// message merely quotes.
fn fingerprint(transcript: &Path) -> Option<crate::leftovers::Fingerprint> {
    use crate::leftovers::Fingerprint;
    use serde_json::Value;
    let mut reader = std::io::BufReader::new(std::fs::File::open(transcript).ok()?);
    let mut socket = None;
    while let Some(line) = read_line(&mut reader) {
        // A cheap look first: most lines are neither.
        let tools = line.contains(BRIGADIER_TOOLS);
        let sandbox = line.contains(DAEMON_SOCKET) && line.contains("sandbox_instructions");
        if !tools && !sandbox {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if tools
            && value.get("type").and_then(Value::as_str) == Some("assistant")
            && value
                .pointer("/message/content")
                .and_then(Value::as_array)
                .is_some_and(|parts| {
                    parts.iter().any(|part| {
                        part.get("type").and_then(Value::as_str) == Some("tool_use")
                            && part
                                .get("name")
                                .and_then(Value::as_str)
                                .is_some_and(|name| name.starts_with(BRIGADIER_TOOLS))
                    })
                })
        {
            return Some(Fingerprint::BrigadierTools);
        }
        if sandbox
            && socket.is_none()
            && value.get("type").and_then(Value::as_str) == Some("attachment")
            && value.pointer("/attachment/type").and_then(Value::as_str)
                == Some("sandbox_instructions")
            && let Some(content) = value.pointer("/attachment/content").and_then(Value::as_str)
        {
            socket = socket_data_dir(content);
        }
    }
    socket.map(|data_dir| Fingerprint::BrigadierSocket { data_dir })
}

/// The data directory of the Brigadier daemon socket named in a sandbox description's
/// `allowUnixSockets` list.
fn socket_data_dir(content: &str) -> Option<PathBuf> {
    let at = content.find("allowUnixSockets")?;
    let rest = &content[at..];
    let open = rest.find('[')?;
    let close = rest[open..].find(']')? + open;
    let list: Vec<String> = serde_json::from_str(&rest[open..=close]).ok()?;
    list.iter().find_map(|socket| {
        socket
            .strip_suffix(DAEMON_SOCKET)
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
    })
}

#[cfg(test)]
mod staging_tests {
    use super::*;

    /// The last of our CLIs in a folder takes the empty staging folders it made there; one
    /// still running, a staged file, or a `.claude` that was there before keeps them (issue #67).
    #[test]
    fn the_last_cli_in_a_folder_takes_the_empty_staging_folders_it_made() {
        let root = std::env::temp_dir().join(format!("brigadier-staging-{}", std::process::id()));
        let (fresh, kept) = (root.join("fresh"), root.join("kept"));
        let [claude, staging] = staging_dirs(&fresh);
        std::fs::create_dir_all(&fresh).unwrap();
        let first = cli_started(&fresh, Some(claude.clone()));
        let second = cli_started(&fresh, Some(staging.clone()));
        std::fs::create_dir_all(&staging).unwrap();
        drop(first);
        assert!(staging.is_dir(), "a CLI still runs there");
        drop(second);
        assert!(!claude.exists(), "both folders were ours and empty");

        // A staged file keeps its folder.
        let cli = cli_started(&fresh, Some(claude.clone()));
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("write.tmp"), "x").unwrap();
        drop(cli);
        assert!(staging.join("write.tmp").is_file());

        // `.claude` was there: only the folder made in it goes.
        let [claude, staging] = staging_dirs(&kept);
        std::fs::create_dir_all(&claude).unwrap();
        let cli = cli_started(&kept, Some(staging.clone()));
        std::fs::create_dir_all(&staging).unwrap();
        drop(cli);
        assert!(claude.is_dir() && !staging.exists());

        // Nothing made by us: nothing goes.
        std::fs::create_dir_all(&staging).unwrap();
        drop(cli_started(&kept, None));
        assert!(staging.is_dir());
        std::fs::remove_dir_all(&root).unwrap();
    }
}

#[cfg(all(test, unix))]
mod link_tests {
    use super::*;

    const GIT_PATHS: &[&str] = &[
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_PREFIX",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    ];

    /// Run the real link fixture in a child; inherited Git paths must not reach the caller.
    #[test]
    fn link_git_helpers_leave_the_callers_repository_untouched() {
        let scratch = std::env::temp_dir().join(format!(
            "20261018-1945-issues-run-2-w42-provider-git-env-{}",
            uuid::Uuid::new_v4()
        ));
        let caller = scratch.join("caller");
        std::fs::create_dir_all(&caller).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(scratch.clone());
        std::fs::create_dir_all(scratch.join("temp")).unwrap();
        let caller_git = |args: &[&str]| {
            let mut command = std::process::Command::new("git");
            for key in GIT_PATHS {
                command.env_remove(key);
            }
            let output = command
                .current_dir(&caller)
                .args([
                    "-c",
                    "user.name=T",
                    "-c",
                    "user.email=t@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            output.stdout
        };
        caller_git(&["init", "-q"]);
        std::fs::write(caller.join("caller.txt"), "keep this\n").unwrap();
        caller_git(&["add", "caller.txt"]);
        caller_git(&["commit", "-qm", "Caller"]);
        let head = caller_git(&["rev-parse", "HEAD"]);
        let index = std::fs::read(caller.join(".git/index")).unwrap();
        for all_paths in [false, true] {
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            for key in GIT_PATHS {
                command.env_remove(key);
            }
            command
                .args([
                    "--exact",
                    &format!(
                        "{}::a_sessions_folder_links_the_projects_own_setup",
                        module_path!().split_once("::").unwrap().1
                    ),
                    "--nocapture",
                ])
                .env("GIT_DIR", caller.join(".git"))
                .env("TMPDIR", scratch.join("temp"));
            if all_paths {
                command
                    .env("GIT_COMMON_DIR", caller.join(".git"))
                    .env("GIT_WORK_TREE", &caller)
                    .env("GIT_INDEX_FILE", caller.join(".git/index"))
                    .env("GIT_OBJECT_DIRECTORY", caller.join(".git/objects"))
                    .env(
                        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                        caller.join(".git/objects"),
                    )
                    .env("GIT_PREFIX", "caller/");
            }
            let output = command.output().unwrap();
            assert_eq!(caller_git(&["rev-parse", "HEAD"]), head);
            assert_eq!(std::fs::read(caller.join(".git/index")).unwrap(), index);
            assert!(output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }

    fn git(dir: &Path, args: &[&str]) {
        let mut command = std::process::Command::new("git");
        for key in GIT_PATHS {
            command.env_remove(key);
        }
        let status = command.args(args).current_dir(dir).status().unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A session's folder gets links to the project's `.mcp.json` and `.claude` entries, the
    /// user's local settings from their own checkout, and never the instruction files; moving
    /// to another checkout replaces them, and a file of Claude's own there is left alone.
    #[test]
    fn a_sessions_folder_links_the_projects_own_setup() {
        let root = std::env::temp_dir().join(format!("brigadier-link-{}", uuid::Uuid::new_v4()));
        let (main, worktree, cwd) = (root.join("main"), root.join("wt"), root.join("orch"));
        std::fs::create_dir_all(main.join(".claude/hooks")).unwrap();
        std::fs::create_dir_all(main.join(".claude/rules")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        for (path, text) in [
            (".mcp.json", r#"{"mcpServers":{}}"#),
            (".claude/settings.json", "{}"),
            (".claude/hooks/check.sh", "true"),
            (".claude/CLAUDE.md", "rules"),
            (".claude/rules/style.md", "style"),
        ] {
            std::fs::write(main.join(path), text).unwrap();
        }
        git(&main, &["init", "-q"]);
        git(&main, &["add", "-A"]);
        git(
            &main,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-qm",
                "init",
            ],
        );
        // Gitignored in real projects: only the user's own checkout has it.
        std::fs::write(main.join(".claude/settings.local.json"), "{}").unwrap();
        git(&main, &["worktree", "add", "-q", wt(&worktree)]);

        link_project(&cwd, &worktree).unwrap();
        let target = |link: &str| std::fs::read_link(cwd.join(link)).ok();
        assert_eq!(target(".mcp.json"), Some(worktree.join(".mcp.json")));
        assert_eq!(
            target(".claude/settings.json"),
            Some(worktree.join(".claude/settings.json"))
        );
        assert_eq!(
            target(".claude/hooks"),
            Some(worktree.join(".claude/hooks"))
        );
        assert_eq!(
            target(".claude/settings.local.json"),
            Some(
                main.canonicalize()
                    .unwrap()
                    .join(".claude/settings.local.json")
            )
        );
        assert!(!cwd.join(".claude/CLAUDE.md").exists(), "the prompt has it");
        assert!(!cwd.join(".claude/rules").exists(), "the prompt has them");
        assert!(cwd.join(".claude/hooks/check.sh").is_file());

        // Claude's own file stays; the next checkout's setup replaces the links, and what it
        // lacks goes.
        std::fs::create_dir_all(cwd.join(".claude/.cc-writes")).unwrap();
        std::fs::remove_file(worktree.join(".mcp.json")).unwrap();
        std::fs::remove_dir_all(worktree.join(".claude/hooks")).unwrap();
        link_project(&cwd, &worktree).unwrap();
        assert_eq!(target(".mcp.json"), None);
        assert_eq!(target(".claude/hooks"), None);
        assert!(cwd.join(".claude/.cc-writes").is_dir());
        assert_eq!(
            target(".claude/settings.json"),
            Some(worktree.join(".claude/settings.json"))
        );
        link_project(&cwd, &main).unwrap();
        assert_eq!(
            target(".claude/settings.json"),
            Some(main.join(".claude/settings.json"))
        );
        assert_eq!(target(".mcp.json"), Some(main.join(".mcp.json")));

        // The folder's trust taken back: the links go, Claude's own file stays.
        unlink_project(&cwd).unwrap();
        assert_eq!(target(".mcp.json"), None);
        assert_eq!(target(".claude/settings.json"), None);
        assert_eq!(target(".claude/settings.local.json"), None);
        assert!(cwd.join(".claude/.cc-writes").is_dir());
        // A checkout's own links are the user's.
        std::os::unix::fs::symlink(main.join(".mcp.json"), worktree.join(".mcp.json")).unwrap();
        unlink_project(&worktree).unwrap();
        assert!(worktree.join(".mcp.json").is_symlink());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Claude runs a project's hooks in the session's folder: a settings file whose hooks run
    /// commands is linked as a copy where each first goes to the project, with
    /// `CLAUDE_PROJECT_DIR` set to it (#315); one without stays a link to the file itself.
    #[test]
    fn a_projects_hooks_run_in_the_project() {
        let root = std::env::temp_dir().join(format!("brigadier-link-{}", uuid::Uuid::new_v4()));
        let (project, cwd) = (root.join("it's a project"), root.join("orch"));
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let settings = serde_json::json!({
            "permissions": { "allow": ["Bash(npm test)"] },
            "hooks": { "PostToolUse": [{
                "matcher": "Edit",
                "hooks": [
                    { "type": "command", "command": "pwd > where.txt; echo \"$CLAUDE_PROJECT_DIR\" >> where.txt" },
                    { "type": "prompt", "prompt": "Check it." },
                ],
            }]},
        });
        std::fs::write(project.join(".claude/settings.json"), settings.to_string()).unwrap();
        link_project(&cwd, &project).unwrap();
        let link = cwd.join(".claude/settings.json");
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            cwd.join(".claude/.settings.project.json")
        );
        let copied: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&link).unwrap()).unwrap();
        assert_eq!(copied["permissions"], settings["permissions"]);
        let text = std::fs::read_to_string(&link).unwrap();
        assert!(
            !text.contains("it's a project"),
            "workers share the copy: {text}"
        );
        let hooks = &copied["hooks"]["PostToolUse"][0]["hooks"];
        assert_eq!(hooks[1], settings["hooks"]["PostToolUse"][0]["hooks"][1]);
        // Run from the session's folder, as Claude runs it, it runs in the project.
        let ran = std::process::Command::new("/bin/sh")
            .args(["-c", hooks[0]["command"].as_str().unwrap()])
            .current_dir(&cwd)
            .env("CLAUDE_PROJECT_DIR", &cwd)
            .env(PROJECT_DIR_ENV, &project)
            .status()
            .unwrap();
        assert!(ran.success());
        let real = project.canonicalize().unwrap();
        let where_ = std::fs::read_to_string(project.join("where.txt")).unwrap();
        let lines: Vec<&str> = where_.lines().collect();
        assert_eq!(
            lines
                .iter()
                .map(|line| std::path::Path::new(line).canonicalize().unwrap())
                .collect::<Vec<_>>(),
            [real.clone(), real]
        );
        // Without command hooks it is the file itself, and the copy goes.
        std::fs::write(project.join(".claude/settings.json"), "{}").unwrap();
        link_project(&cwd, &project).unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            project.join(".claude/settings.json")
        );
        assert!(!cwd.join(".claude/.settings.project.json").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Workers of a session start together in their shared folder: every one links the
    /// project's setup in full.
    #[test]
    fn workers_starting_together_each_link_the_whole_setup() {
        let root = std::env::temp_dir().join(format!("brigadier-link-{}", uuid::Uuid::new_v4()));
        let (project, cwd) = (root.join("project"), root.join("workers"));
        std::fs::create_dir_all(project.join(".claude/skills")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::write(project.join(".mcp.json"), "{}").unwrap();
        std::fs::write(project.join(".claude/settings.json"), "{}").unwrap();
        for _ in 0..20 {
            let starts: Vec<_> = (0..8)
                .map(|_| {
                    let (cwd, project) = (cwd.clone(), project.clone());
                    std::thread::spawn(move || {
                        unlink_project(&cwd)?;
                        link_project(&cwd, &project)
                    })
                })
                .collect();
            for start in starts {
                start.join().unwrap().unwrap();
            }
        }
        link_project(&cwd, &project).unwrap();
        for link in [".mcp.json", ".claude/settings.json", ".claude/skills"] {
            assert!(cwd.join(link).is_symlink(), "{link}");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Claude starts the project's MCP servers in the session's folder: what a server names in
    /// the project is made absolute there, so `node scripts/mcp.js` finds the project's script;
    /// every other value stays as written (#352).
    #[test]
    fn a_projects_mcp_servers_keep_their_paths_in_the_project() {
        let root = std::env::temp_dir().join(format!("brigadier-link-{}", uuid::Uuid::new_v4()));
        let (project, cwd) = (root.join("project"), root.join("orch"));
        std::fs::create_dir_all(project.join("scripts")).unwrap();
        std::fs::create_dir_all(project.join("mcp/server")).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        for file in ["scripts/mcp.js", "scripts/server", "server.py", "run"] {
            std::fs::write(project.join(file), "").unwrap();
        }
        std::fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers":{
                "notes":{"command":"node","args":["scripts/mcp.js","--port","3000"]},
                "local":{"command":"./scripts/server","args":["--data","${CLAUDE_PROJECT_DIR:-.}/data"]},
                "uv":{"command":"uv","args":["run","server.py"]},
                "elsewhere":{"command":"uv","args":["--directory","/external/mcp","run","server.py"]},
                "npm":{"command":"npm","args":["run","mcp"]},
                "box":{"command":"docker","args":["run","-v","${CLAUDE_PROJECT_DIR}:/app","-w","/app","node:22","node","scripts/mcp.js"]},
                "npx":{"command":"npx","args":["-y","@acme/mcp"]},
                "docker":{"command":"docker","args":["run","--rm","-i","mcp/server"]},
                "web":{"type":"http","url":"https://example.com/mcp"}
            }}"#,
        )
        .unwrap();

        // A link the session's commands left at the copy's temporary name isn't written through.
        let outside = root.join("outside");
        std::fs::write(&outside, "keep").unwrap();
        let temp = cwd.join(format!("{MCP_COPY}.{}.tmp", std::process::id()));
        std::fs::create_dir_all(cwd.join(".claude")).unwrap();
        std::os::unix::fs::symlink(&outside, &temp).unwrap();
        link_project(&cwd, &project).unwrap();
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        assert!(!cwd.join(MCP_COPY).is_symlink());
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(cwd.join(MCP_COPY)).unwrap().permissions(),
        );
        assert_eq!(mode & 0o777, 0o600, "the user's alone");
        assert_eq!(
            std::fs::read_link(cwd.join(".mcp.json")).unwrap(),
            cwd.join(MCP_COPY)
        );
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(cwd.join(".mcp.json")).unwrap()).unwrap();
        let servers = &config["mcpServers"];
        let p = |rest: &str| project.join(rest).display().to_string();
        assert_eq!(servers["notes"]["command"], "node");
        assert_eq!(
            servers["notes"]["args"],
            serde_json::json!([p("scripts/mcp.js"), "--port", "3000"])
        );
        assert_eq!(servers["local"]["command"], p("scripts/server"));
        assert_eq!(
            servers["local"]["args"],
            serde_json::json!(["--data", format!("{}/data", project.display())])
        );
        assert_eq!(
            servers["uv"]["args"],
            serde_json::json!(["run", p("server.py")])
        );
        // A runner told where to read its paths from reads them there.
        assert_eq!(
            servers["elsewhere"]["args"],
            serde_json::json!(["--directory", "/external/mcp", "run", "server.py"])
        );
        // Another command's arguments stay: an npm script's name, a container's own paths.
        assert_eq!(servers["npm"]["args"], serde_json::json!(["run", "mcp"]));
        assert_eq!(
            servers["box"]["args"],
            serde_json::json!([
                "run",
                "-v",
                format!("{}:/app", project.display()),
                "-w",
                "/app",
                "node:22",
                "node",
                "scripts/mcp.js"
            ])
        );
        assert_eq!(
            servers["npx"],
            serde_json::json!({ "command": "npx", "args": ["-y", "@acme/mcp"] })
        );
        assert_eq!(
            servers["docker"]["args"],
            serde_json::json!(["run", "--rm", "-i", "mcp/server"])
        );
        assert_eq!(servers["web"]["url"], "https://example.com/mcp");

        // Nothing in the project left: the link reads the project's own file, and the copy goes.
        std::fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers":{"npx":{"command":"npx","args":["-y","@acme/mcp"]}}}"#,
        )
        .unwrap();
        link_project(&cwd, &project).unwrap();
        assert_eq!(
            std::fs::read_link(cwd.join(".mcp.json")).unwrap(),
            project.join(".mcp.json")
        );
        assert!(!cwd.join(MCP_COPY).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn wt(path: &Path) -> &str {
        path.to_str().unwrap()
    }
}
