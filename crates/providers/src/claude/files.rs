//! The files Claude Code keeps for a session, and removing exactly those.
//!
//! Everything is keyed by the session's UUID under Claude's configuration directory:
//! `projects/<encoded cwd>/<id>.jsonl` (the transcript) and `projects/<encoded cwd>/<id>/`,
//! plus `tasks/<id>`, `session-env/<id>`, `file-history/<id>`, `todos/<id>-*.json` and
//! `debug/<id>.txt`. Nothing else is touched.
//!
//! In the working directory, Claude Code stages its writes in `.claude/.cc-writes`; the part of
//! that path which did not exist before the session is removed with it, while it holds no files.
//!
//! Claude also keeps per-session temp files in `<temp>/claude-<uid>/<encoded cwd>/<id>/`
//! (`<temp>` is `CLAUDE_CODE_TMPDIR`, else `/tmp` on macOS and the system temp directory
//! elsewhere), and its sandbox gives commands `<temp>/claude-<uid>` as their TMPDIR. A session
//! started with a TMPDIR of its own gets a short folder of its own as `<temp>`
//! ([`create_temp_dir`]), removed with everything in it, and leaves nothing in the shared
//! location; for the others the session's folder is removed with it, and the per-cwd folder
//! too when it is empty and belonged to a project directory the session created.

use std::io;
use std::path::{Path, PathBuf};

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
