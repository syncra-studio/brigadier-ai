//! Claude Code's own config file (`.claude.json`) as Claude Code 2.1.310 finds and keys it:
//! which file it is, the key a folder's settings go under, what a project's entry switches
//! (the per-project off flags `/mcp` writes, a team server's approval), and a careful writer
//! for those off flags.
//!
//! Claude Code writes the file whole, sometimes under its lock (`<file>.lock`, proper-lockfile)
//! and sometimes without, re-reading it from disk just before each write. So Brigadier takes
//! the same lock (which keeps out the writers that honour it), changes only the bytes of the
//! arrays it edits, checks the file is still what it read before putting its version in place,
//! and reads it again a moment later to make sure its edit stayed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

/// How long a lock may sit untouched before it counts as left behind (proper-lockfile's
/// default, which Claude Code keeps).
const STALE: Duration = Duration::from_secs(10);
/// How long Brigadier waits for Claude Code to let go of its lock.
const WAIT: Duration = Duration::from_secs(5);
/// How often an edit is put back after Claude Code wrote over it before Brigadier gives up.
const TRIES: usize = 3;
/// How often the file may change between Brigadier's read and its rename (each try takes a few
/// milliseconds; Claude Code writes when something happens, not continuously).
const RACES: usize = 20;

/// The config file: `.config.json` in Claude's folder when there is one (an old install, which
/// Claude Code still prefers), else `.claude.json` in `CLAUDE_CONFIG_DIR` or the home. The same
/// rule as `brigadier_providers::accounts::main_config_file`.
pub fn config_file(home: &Path, config_dir: Option<&Path>) -> PathBuf {
    let legacy = config_dir
        .map_or_else(|| home.join(".claude"), Path::to_owned)
        .join(".config.json");
    if legacy.is_file() {
        return legacy;
    }
    config_dir.unwrap_or(home).join(".claude.json")
}

/// The key Claude Code keeps `folder`'s settings under (`projects["<key>"]`): its repository's
/// main folder, for a subfolder and a linked worktree too, symlinks resolved. A folder outside
/// git is its own key; a submodule is its own repository. A flag under any other key is
/// ignored (checked with 2.1.310).
pub fn project_key(folder: &Path) -> PathBuf {
    let real = std::fs::canonicalize(folder).unwrap_or_else(|_| folder.to_owned());
    for dir in real.ancestors() {
        let dot = dir.join(".git");
        if dot.is_dir() {
            return dir.to_owned();
        }
        if dot.is_file() {
            return main_folder(dir, &dot).unwrap_or_else(|| dir.to_owned());
        }
    }
    real
}

/// A linked worktree's main folder: its `gitdir` names the main repository's `.git` through
/// `commondir`. `None` for anything else (a submodule's gitdir has no `commondir`).
fn main_folder(dir: &Path, dot: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(dot).ok()?;
    let gitdir = dir.join(
        text.lines()
            .find_map(|line| line.strip_prefix("gitdir:"))?
            .trim(),
    );
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok()?;
    let common = std::fs::canonicalize(gitdir.join(common.trim())).ok()?;
    if common.file_name()? != ".git" {
        return None;
    }
    common.parent().map(Path::to_owned)
}

/// What a project's entry in the config file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectSettings {
    /// `disabledMcpServers`: the servers switched off in the project (`/mcp`), by the name
    /// Claude Code lists them under (`plugin:<plugin>:<server>` for a plugin's).
    pub off: Vec<String>,
    /// `hasTrustDialogAccepted`.
    pub trusted: bool,
    /// `enabledMcpjsonServers`: the team servers approved there.
    pub approved: Vec<String>,
    /// `disabledMcpjsonServers`: the team servers turned down there.
    pub denied: Vec<String>,
}

/// The entry of `folder` in config file `file`; nothing when the file or the entry isn't there.
/// An unreadable or malformed file is an error, never an empty entry: a flag Brigadier wrote
/// must not look as if the user had taken it away.
pub fn project_settings(file: &Path, folder: &Path) -> Result<ProjectSettings, String> {
    let Some(config) = read_config(file)? else {
        return Ok(ProjectSettings::default());
    };
    if !config.is_object() || config.get("projects").is_some_and(|p| !p.is_object()) {
        return Err(unreadable(
            file,
            "expected a settings object and a projects object",
        ));
    }
    let key = key_text(&project_key(folder));
    let Some(entry) = config
        .get("projects")
        .and_then(|projects| projects.get(&key))
    else {
        return Ok(ProjectSettings::default());
    };
    if !entry.is_object()
        || entry.get("disabledMcpServers").is_some_and(|v| {
            v.as_array()
                .is_none_or(|a| a.iter().any(|v| !v.is_string()))
        })
    {
        return Err(unreadable(
            file,
            "expected a project object with a disabledMcpServers list of names",
        ));
    }
    Ok(ProjectSettings {
        off: names(entry.get("disabledMcpServers")),
        trusted: entry.get("hasTrustDialogAccepted").and_then(Value::as_bool) == Some(true),
        approved: names(entry.get("enabledMcpjsonServers")),
        denied: names(entry.get("disabledMcpjsonServers")),
    })
}

/// Whether Claude Code loads a team server (a project's `.mcp.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    /// Not approved yet: "⏸ Pending approval", not loaded.
    Pending,
    /// Turned down (`disabledMcpjsonServers`): not listed at all.
    Denied,
}

/// The approval of team server `name` in `project`, as 2.1.310 decides it: turned down in the
/// project's entry or any settings file wins; otherwise approved only in a project the user
/// trusts, by the project's entry or a settings file (`enabledMcpjsonServers`,
/// `enableAllProjectMcpServers`). `claude_dir` holds the user's `settings.json`.
pub fn team_approval(
    settings: &ProjectSettings,
    claude_dir: &Path,
    project: &Path,
    name: &str,
) -> Approval {
    let files: Vec<Value> = [
        claude_dir.join("settings.json"),
        project.join(".claude/settings.json"),
        project.join(".claude/settings.local.json"),
    ]
    .iter()
    .filter_map(|file| read_config(file).ok().flatten())
    .collect();
    let listed = |key: &str| {
        files
            .iter()
            .any(|file| names(file.get(key)).iter().any(|known| known == name))
    };
    if settings.denied.iter().any(|known| known == name) || listed("disabledMcpjsonServers") {
        return Approval::Denied;
    }
    let all = files.iter().any(|file| {
        file.get("enableAllProjectMcpServers")
            .and_then(Value::as_bool)
            == Some(true)
    });
    if settings.trusted
        && (all
            || listed("enabledMcpjsonServers")
            || settings.approved.iter().any(|known| known == name))
    {
        Approval::Approved
    } else {
        Approval::Pending
    }
}

/// One off flag: server `name` (as Claude Code lists it) switched off (`off`) or not in the
/// folder `folder` (its key is worked out from it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffFlag {
    pub folder: PathBuf,
    pub name: String,
    pub off: bool,
}

/// Sets every flag of `flags` in config file `file`, keeping every other byte of it; then waits
/// `settle` and reads it again, putting back what Claude Code wrote over meanwhile. Whether it
/// changed the file. A project with no entry gets one in the shape Claude Code gives a new
/// project. An unreadable or malformed file is an error and is left alone.
pub fn set_off_flags(file: &Path, flags: &[OffFlag], settle: Duration) -> Result<bool, String> {
    let keyed: Vec<(String, &OffFlag)> = flags
        .iter()
        .map(|flag| (key_text(&project_key(&flag.folder)), flag))
        .collect();
    let mut changed = false;
    for _ in 0..TRIES {
        changed |= write_once(file, &keyed)?;
        if !settle.is_zero() {
            std::thread::sleep(settle);
        }
        if holds(file, &keyed)? {
            return Ok(changed);
        }
    }
    Err("Claude Code rewrote its settings at the same moment; try again.".to_owned())
}

/// Whether `file` has every flag of `keyed` as it should be.
fn holds(file: &Path, keyed: &[(String, &OffFlag)]) -> Result<bool, String> {
    let config = read_config(file)?.unwrap_or(Value::Null);
    Ok(keyed.iter().all(|(key, flag)| {
        let off = names(
            config
                .get("projects")
                .and_then(|projects| projects.get(key))
                .and_then(|entry| entry.get("disabledMcpServers")),
        );
        off.contains(&flag.name) == flag.off
    }))
}

/// One locked edit: read, splice, and rename only if the file is still what was read.
fn write_once(file: &Path, keyed: &[(String, &OffFlag)]) -> Result<bool, String> {
    rewrite_locked(file, |before| {
        if before.trim().is_empty() && file.exists() {
            return Err(unreadable(file, "the settings file is empty"));
        }
        let mut after = if before.trim().is_empty() {
            "{}\n".to_owned()
        } else {
            before.to_owned()
        };
        for (key, flag) in keyed {
            after = splice::set_flag(&after, key, &flag.name, flag.off)
                .map_err(|err| unreadable(file, &err))?;
        }
        Ok((after != before).then_some(after))
    })
}

/// Rewrites Claude Code's config file `file` (through a symlink, keeping its permissions) to
/// what `edit` makes of its text (`""` when it isn't there; `None`: nothing to change), under
/// Claude Code's lock, and only if the file is still what `edit` read: when Claude Code wrote it
/// meanwhile (it doesn't always take its lock), `edit` runs again on what it wrote. Whether
/// the file changed.
pub fn rewrite_locked(
    file: &Path,
    mut edit: impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<bool, String> {
    let target = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_owned());
    let lock = Lock::take(&target)?;
    for _ in 0..RACES {
        let before = match std::fs::read_to_string(&target) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(unreadable(&target, &err.to_string())),
        };
        let Some(after) = edit(&before)? else {
            return Ok(false);
        };
        if after == before {
            return Ok(false);
        }
        let temp = target.with_file_name(format!(
            ".{}.brigadier-{}.tmp",
            target
                .file_name()
                .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
            std::process::id()
        ));
        write_like(&temp, &after, &target).map_err(|err| unreadable(&target, &err.to_string()))?;
        let now = std::fs::read_to_string(&target).unwrap_or_default();
        lock.check().inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })?;
        if now != before {
            // Claude Code wrote it meanwhile: start again from what it wrote.
            let _ = std::fs::remove_file(&temp);
            continue;
        }
        std::fs::rename(&temp, &target).map_err(|err| {
            let _ = std::fs::remove_file(&temp);
            unreadable(&target, &err.to_string())
        })?;
        return Ok(true);
    }
    Err("Claude Code kept rewriting its settings; try again.".to_owned())
}

/// Writes `text` to `temp` with `like`'s permissions (owner-only for a new file).
fn write_like(temp: &Path, text: &str, like: &Path) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = std::fs::File::create(temp)?;
    out.write_all(text.as_bytes())?;
    out.sync_all()?;
    match std::fs::metadata(like) {
        Ok(meta) => std::fs::set_permissions(temp, meta.permissions())?,
        #[cfg(unix)]
        Err(_) => {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(temp, std::fs::Permissions::from_mode(0o600))?;
        }
        #[cfg(not(unix))]
        Err(_) => {}
    }
    Ok(())
}

/// Claude Code's lock on its config file: the folder `<file>.lock`, as proper-lockfile makes
/// it. Held for milliseconds, far under the time after which another program may take it.
struct Lock(PathBuf, std::fs::Metadata);

impl Lock {
    fn check(&self) -> Result<(), String> {
        let same = std::fs::metadata(&self.0).is_ok_and(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                meta.dev() == self.1.dev() && meta.ino() == self.1.ino()
            }
            #[cfg(not(unix))]
            {
                meta.created().ok() == self.1.created().ok()
                    && meta.modified().ok() == self.1.modified().ok()
            }
        });
        if same {
            Ok(())
        } else {
            Err("Claude Code's settings lock changed while saving; try again.".into())
        }
    }

    fn take(file: &Path) -> Result<Self, String> {
        let mut path = file.as_os_str().to_owned();
        path.push(".lock");
        let path = PathBuf::from(path);
        let start = Instant::now();
        loop {
            match std::fs::create_dir(&path) {
                Ok(()) => {
                    let meta = std::fs::metadata(&path).map_err(|e| e.to_string())?;
                    return Ok(Self(path, meta));
                }
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .is_ok_and(|at| at.elapsed().is_ok_and(|age| age > STALE));
                    if stale {
                        std::fs::remove_dir(&path)
                            .map_err(|e| format!("Couldn't remove the stale settings lock: {e}"))?;
                        continue;
                    }
                    if start.elapsed() > WAIT {
                        return Err(
                            "Claude Code is busy with its settings; try again in a moment."
                                .to_owned(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    return Err(format!("Brigadier couldn't lock {}: {err}", path.display()));
                }
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if self.check().is_ok() {
            let _ = std::fs::remove_dir(&self.0);
        }
    }
}

/// The file parsed; `None` when it isn't there.
fn read_config(file: &Path) -> Result<Option<Value>, String> {
    match std::fs::read_to_string(file) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| unreadable(file, &err.to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(unreadable(file, &err.to_string())),
    }
}

fn unreadable(file: &Path, why: &str) -> String {
    format!("Brigadier couldn't read {}: {why}", file.display())
}

fn names(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// A key as Claude Code writes it: forward slashes on every platform.
fn key_text(key: &Path) -> String {
    let text = key.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text
    }
}

/// The members of a new project's entry, in Claude Code's order (2.1.310), then its flags.
fn new_project() -> Vec<(String, Value)> {
    [
        ("allowedTools", Value::Array(Vec::new())),
        ("mcpContextUris", Value::Array(Vec::new())),
        ("mcpServers", Value::Object(serde_json::Map::new())),
        ("enabledMcpjsonServers", Value::Array(Vec::new())),
        ("disabledMcpjsonServers", Value::Array(Vec::new())),
        ("hasTrustDialogAccepted", Value::Bool(false)),
        ("hasClaudeMdExternalIncludesApproved", Value::Bool(false)),
        (
            "hasClaudeMdExternalIncludesWarningShown",
            Value::Bool(false),
        ),
        ("disabledMcpServers", Value::Array(Vec::new())),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect()
}

/// Edits of one JSON text that keep every byte outside what they change.
mod splice {
    use serde_json::Value;

    /// A member of an object: its key and where its value is (`start..end`).
    struct Member {
        key: String,
        key_start: usize,
        start: usize,
        end: usize,
    }

    /// An object: where its braces are, and its members in order.
    struct Object {
        open: usize,
        close: usize,
        members: Vec<Member>,
    }

    /// `text` with `name` in (`off`) or out of `projects[key].disabledMcpServers`.
    pub(super) fn set_flag(text: &str, key: &str, name: &str, off: bool) -> Result<String, String> {
        let mut text = text.to_owned();
        let top = object(&text, first(&text)?)?;
        let projects = match find(&top, "projects") {
            Some(member) => member.start,
            None => {
                text = insert(&text, &top, "projects", &[])?;
                find(&object(&text, first(&text)?)?, "projects")
                    .ok_or("the new projects entry is missing")?
                    .start
            }
        };
        let all = object(&text, projects)?;
        let entry = match find(&all, key) {
            Some(member) => member.start,
            None => {
                if !off {
                    return Ok(text);
                }
                text = insert(&text, &all, key, &super::new_project())?;
                find(&object(&text, projects)?, key)
                    .ok_or("the new project entry is missing")?
                    .start
            }
        };
        let project = object(&text, entry)?;
        let list = match find(&project, "disabledMcpServers") {
            Some(member) => (member.start, member.end),
            None => {
                if !off {
                    return Ok(text);
                }
                text = insert_value(&text, &project, "disabledMcpServers", "[]")?;
                let project = object(&text, entry)?;
                let member =
                    find(&project, "disabledMcpServers").ok_or("the new flag list is missing")?;
                (member.start, member.end)
            }
        };
        let mut names: Vec<Value> = serde_json::from_str(&text[list.0..list.1])
            .map_err(|_| "disabledMcpServers isn't a list".to_owned())?;
        let has = names.iter().any(|known| known.as_str() == Some(name));
        if has == off {
            return Ok(text);
        }
        if off {
            names.push(Value::String(name.to_owned()));
        } else {
            names.retain(|known| known.as_str() != Some(name));
        }
        let indent = line_indent(&text, list.0);
        let rendered = render(&Value::Array(names), &indent, &text);
        text.replace_range(list.0..list.1, &rendered);
        Ok(text)
    }

    fn first(text: &str) -> Result<usize, String> {
        let at = skip_ws(text.as_bytes(), 0);
        if text.as_bytes().get(at) == Some(&b'{') {
            Ok(at)
        } else {
            Err("it isn't a JSON object".to_owned())
        }
    }

    fn find<'a>(object: &'a Object, key: &str) -> Option<&'a Member> {
        object.members.iter().find(|member| member.key == key)
    }

    /// `text` with member `key` added at the end of `object`, its value an object of
    /// `members` in their order.
    fn insert(
        text: &str,
        object: &Object,
        key: &str,
        members: &[(String, Value)],
    ) -> Result<String, String> {
        let multiline = text.contains('\n');
        let at = |indent: &str| ordered(members, indent, multiline);
        place(text, object, key, &at)
    }

    /// `text` with member `key: value` (already JSON) added at the end of `object`.
    fn insert_value(text: &str, object: &Object, key: &str, value: &str) -> Result<String, String> {
        place(text, object, key, &|_: &str| value.to_owned())
    }

    /// `text` with member `key` added at the end of `object`, laid out like its neighbours
    /// (Claude Code writes two-space indented JSON); `value` renders it at an indent.
    fn place(
        text: &str,
        object: &Object,
        key: &str,
        value: &dyn Fn(&str) -> String,
    ) -> Result<String, String> {
        let key = serde_json::to_string(key).map_err(|err| err.to_string())?;
        let mut out = text.to_owned();
        match object.members.last() {
            Some(last) => {
                let indent = line_indent(text, last.key_start);
                let (gap, inner) = if text[object.open..object.close].contains('\n') {
                    (format!(",\n{indent}"), indent.clone())
                } else {
                    (", ".to_owned(), String::new())
                };
                let value = value(&inner);
                out.insert_str(last.end, &format!("{gap}{key}: {value}"));
            }
            None if text.contains('\n') => {
                let outer = line_indent(text, object.open);
                let inner = format!("{outer}  ");
                let value = value(&inner);
                out.replace_range(
                    object.open..=object.close,
                    &format!("{{\n{inner}{key}: {value}\n{outer}}}"),
                );
            }
            None => {
                let value = value("");
                out.replace_range(object.open..=object.close, &format!("{{{key}:{value}}}"));
            }
        }
        Ok(out)
    }

    /// An object of `members` in their order, its later lines at `indent` (on one line unless
    /// `multiline`).
    fn ordered(members: &[(String, Value)], indent: &str, multiline: bool) -> String {
        if members.is_empty() {
            return "{}".to_owned();
        }
        let key = |key: &str| Value::String(key.to_owned());
        if !multiline {
            let inner: Vec<String> = members
                .iter()
                .map(|(name, value)| format!("{}:{}", key(name), compact(value)))
                .collect();
            return format!("{{{}}}", inner.join(","));
        }
        let inner = format!("{indent}  ");
        let lines: Vec<String> = members
            .iter()
            .map(|(name, value)| format!("{inner}{}: {}", key(name), pretty(value, &inner)))
            .collect();
        format!("{{\n{}\n{indent}}}", lines.join(",\n"))
    }

    /// `value` as Claude Code writes it, its later lines at `indent`; on one line when the
    /// file is.
    fn render(value: &Value, indent: &str, text: &str) -> String {
        let empty = match value {
            Value::Array(items) => items.is_empty(),
            Value::Object(members) => members.is_empty(),
            _ => true,
        };
        if empty || !text.contains('\n') {
            return compact(value);
        }
        pretty(value, indent)
    }

    fn compact(value: &Value) -> String {
        match value {
            Value::Object(members) => {
                let inner: Vec<String> = members
                    .iter()
                    .map(|(key, value)| {
                        format!("{}:{}", Value::String(key.clone()), compact(value))
                    })
                    .collect();
                format!("{{{}}}", inner.join(","))
            }
            Value::Array(items) => {
                let inner: Vec<String> = items.iter().map(compact).collect();
                format!("[{}]", inner.join(","))
            }
            other => other.to_string(),
        }
    }

    /// Two-space indented.
    fn pretty(value: &Value, indent: &str) -> String {
        let inner = format!("{indent}  ");
        match value {
            Value::Object(members) if !members.is_empty() => {
                let lines: Vec<String> = members
                    .iter()
                    .map(|(key, value)| {
                        format!(
                            "{inner}{}: {}",
                            Value::String(key.clone()),
                            pretty(value, &inner)
                        )
                    })
                    .collect();
                format!("{{\n{}\n{indent}}}", lines.join(",\n"))
            }
            Value::Array(items) if !items.is_empty() => {
                let lines: Vec<String> = items
                    .iter()
                    .map(|item| format!("{inner}{}", pretty(item, &inner)))
                    .collect();
                format!("[\n{}\n{indent}]", lines.join(",\n"))
            }
            other => compact(other),
        }
    }

    /// The whitespace that starts the line `at` is on.
    fn line_indent(text: &str, at: usize) -> String {
        let start = text[..at].rfind('\n').map_or(0, |line| line + 1);
        text[start..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }

    fn skip_ws(bytes: &[u8], mut at: usize) -> usize {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    }

    /// The object whose `{` is at `open`.
    fn object(text: &str, open: usize) -> Result<Object, String> {
        let bytes = text.as_bytes();
        if bytes.get(open) != Some(&b'{') {
            return Err("an entry isn't an object".to_owned());
        }
        let mut members = Vec::new();
        let mut at = skip_ws(bytes, open + 1);
        if bytes.get(at) == Some(&b'}') {
            return Ok(Object {
                open,
                close: at,
                members,
            });
        }
        loop {
            if bytes.get(at) != Some(&b'"') {
                return Err("a key is missing".to_owned());
            }
            let key_start = at;
            let key_end = string_end(bytes, at)?;
            let key: String =
                serde_json::from_str(&text[key_start..key_end]).map_err(|err| err.to_string())?;
            at = skip_ws(bytes, key_end);
            if bytes.get(at) != Some(&b':') {
                return Err("a colon is missing".to_owned());
            }
            let start = skip_ws(bytes, at + 1);
            let end = value_end(bytes, start)?;
            members.push(Member {
                key,
                key_start,
                start,
                end,
            });
            at = skip_ws(bytes, end);
            match bytes.get(at) {
                Some(b',') => at = skip_ws(bytes, at + 1),
                Some(b'}') => {
                    return Ok(Object {
                        open,
                        close: at,
                        members,
                    });
                }
                _ => return Err("an object isn't closed".to_owned()),
            }
        }
    }

    /// Just past the string whose `"` is at `at`.
    fn string_end(bytes: &[u8], at: usize) -> Result<usize, String> {
        let mut i = at + 1;
        while let Some(byte) = bytes.get(i) {
            match byte {
                b'\\' => i += 2,
                b'"' => return Ok(i + 1),
                _ => i += 1,
            }
        }
        Err("a string isn't closed".to_owned())
    }

    /// Just past the value that starts at `at`.
    fn value_end(bytes: &[u8], at: usize) -> Result<usize, String> {
        match bytes.get(at) {
            Some(b'"') => string_end(bytes, at),
            Some(b'{' | b'[') => {
                let mut depth = 0usize;
                let mut i = at;
                while let Some(byte) = bytes.get(i) {
                    match byte {
                        b'"' => {
                            i = string_end(bytes, i)?;
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
                Err("a value isn't closed".to_owned())
            }
            Some(_) => {
                let mut i = at;
                while bytes.get(i).is_some_and(|byte| {
                    !matches!(byte, b',' | b'}' | b']') && !byte.is_ascii_whitespace()
                }) {
                    i += 1;
                }
                Ok(i)
            }
            None => Err("a value is missing".to_owned()),
        }
    }
}

#[cfg(test)]
#[path = "claude_config_tests.rs"]
mod tests;
