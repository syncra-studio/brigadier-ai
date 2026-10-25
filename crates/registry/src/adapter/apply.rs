//! Carries out an adapter's plan: runs the CLI's own commands, writes the user's config through
//! the CLI's own writer, and edits a project's file the CLI reads (D8), keeping it out of the
//! repository for "just me".
//!
//! A plan is checked whole before its first step: a step that needs a terminal, a project the
//! CLI doesn't trust or a committed file stops it before anything changes. A step that fails
//! once it runs leaves the steps before it done.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use toml_edit::{DocumentMut, Item, Table};

use super::{AgentAdapter, CliCall, Step, TomlEdit};
use crate::{Catalog, Roots};

/// Why a plan stopped, in plain words for the page ([`fmt::Display`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplyError {
    /// The command must run in a terminal (a sign-in): it has its own path.
    NeedsTerminal(String),
    /// The CLI doesn't read the project's file until the user trusts the project: its label.
    Untrusted(&'static str),
    /// The project's file is committed: a "just me" change would reach the team. Its path in
    /// the repository.
    TrackedFile(String),
    /// The CLI refused: its own words.
    Cli(String),
    /// Anything else.
    Failed(String),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsTerminal(command) => write!(f, "`{command}` needs a terminal."),
            Self::Untrusted(label) => write!(
                f,
                "{label} only reads this project's settings once you trust the project."
            ),
            Self::TrackedFile(file) => write!(
                f,
                "This project's {file} is committed, so a change here would reach your team. \
                 Use Share with the team instead."
            ),
            Self::Cli(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for ApplyError {}

/// What a plan did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Each command's output, in order.
    pub output: Vec<String>,
}

/// Carries out `steps`, planned by `adapter`. `env` is the environment the CLIs run with (the
/// login shell's); the adapter's [`AgentAdapter::home_env`] for `roots` is added to it.
/// `binary` is the agent's CLI as sessions run it (the one chosen in Settings, or found where
/// its installer puts it); `None` looks its name up on `env`'s PATH. `catalog` says which
/// projects the agent trusts.
pub fn apply(
    adapter: &dyn AgentAdapter,
    roots: &Roots,
    catalog: &Catalog,
    steps: &[Step],
    env: &[(String, String)],
    binary: Option<&Path>,
) -> Result<Applied, ApplyError> {
    let mut env = env.to_vec();
    env.extend(adapter.home_env(roots));
    for step in steps {
        check(adapter, catalog, step, &env)?;
    }
    let program = binary.unwrap_or_else(|| Path::new(adapter.program()));
    let mut applied = Applied::default();
    for step in steps {
        match step {
            Step::Run(call) => {
                // The adapter's own CLI is the resolved one; anything else is looked up.
                let binary = if call.program == adapter.program() {
                    program
                } else {
                    Path::new(call.program)
                };
                applied.output.push(run(call, binary, &env)?);
            }
            Step::UserConfig(edit) => adapter.write_user_config(roots, &env, program, edit)?,
            Step::ProjectFile { file, edit, shared } => project_file(file, edit, *shared, &env)?,
        }
    }
    Ok(applied)
}

/// Whether `step` can run at all.
fn check(
    adapter: &dyn AgentAdapter,
    catalog: &Catalog,
    step: &Step,
    env: &[(String, String)],
) -> Result<(), ApplyError> {
    match step {
        Step::Run(call) if call.needs_terminal => Err(ApplyError::NeedsTerminal(command(call))),
        Step::Run(_) | Step::UserConfig(_) => Ok(()),
        Step::ProjectFile { file, edit, shared } => {
            let project = project_of(file)?;
            if !adapter.trusts(catalog, project) {
                return Err(ApplyError::Untrusted(adapter.label()));
            }
            // A file it can't edit stops the plan before the steps ahead of it.
            read_and_edit(file, edit)?;
            if !shared
                && let Some(repo) = Repo::find(project, file, env)
                && repo.tracks_file(env)
            {
                return Err(ApplyError::TrackedFile(repo.file));
            }
            Ok(())
        }
    }
}

/// The command line of `call`, as the user would type it.
fn command(call: &CliCall) -> String {
    std::iter::once(call.program)
        .chain(call.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs the CLI's command with `binary`; its output, or its own words on why it failed.
fn run(call: &CliCall, binary: &Path, env: &[(String, String)]) -> Result<String, ApplyError> {
    let mut cmd = Command::new(binary);
    cmd.args(&call.args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null());
    if let Some(dir) = &call.cwd {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .map_err(|err| spawn_error(call.program, &err))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() || already_so(&stdout) {
        return Ok(stdout);
    }
    let words = stderr_line(&String::from_utf8_lossy(&output.stderr))
        .or_else(|| stderr_line(&String::from_utf8_lossy(&output.stdout)));
    Err(ApplyError::Cli(words.unwrap_or_else(|| {
        let code = output
            .status
            .code()
            .map_or_else(|| "a signal".to_owned(), |code| format!("code {code}"));
        format!("`{}` stopped with {code}.", command(call))
    })))
}

/// A `--json` answer saying nothing needed doing (Claude Code's `alreadyInGoalState`: enabling
/// a plugin that is on already): what was asked for holds.
fn already_so(stdout: &str) -> bool {
    stdout.lines().any(|line| {
        serde_json::from_str::<serde_json::Value>(line).is_ok_and(|answer| {
            answer.get("alreadyInGoalState") == Some(&serde_json::Value::Bool(true))
        })
    })
}

/// The line of a CLI's output that says why it failed: its `error` line, else its last.
pub(super) fn stderr_line(text: &str) -> Option<String> {
    let lines = || text.lines().map(str::trim).filter(|line| !line.is_empty());
    lines()
        .find(|line| line.to_ascii_lowercase().starts_with("error"))
        .or_else(|| lines().next_back())
        .map(str::to_owned)
}

pub(super) fn spawn_error(program: &str, err: &std::io::Error) -> ApplyError {
    ApplyError::Failed(if err.kind() == std::io::ErrorKind::NotFound {
        format!("Brigadier couldn't find `{program}` on your PATH.")
    } else {
        format!("Brigadier couldn't start `{program}`: {err}")
    })
}

/// A key path as the CLIs' config writers take it: `plugins."x@lib".enabled`.
pub(super) fn key_path(key: &[String]) -> String {
    key.iter()
        .map(|part| {
            toml_edit::Key::new(part.as_str())
                .display_repr()
                .into_owned()
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// What to remove from the config `text` to take `key` out: the key, or the highest table
/// that holds nothing else, so no table is left empty by the removal.
pub(super) fn removal_key(text: Option<&str>, key: &[String]) -> Vec<String> {
    let Some(config) = text.and_then(|text| text.parse::<toml::Table>().ok()) else {
        return key.to_vec();
    };
    let mut target = key.len();
    while target > 1 {
        let parent = &key[..target - 1];
        match lookup(&config, parent) {
            Some(toml::Value::Table(table)) if table.len() == 1 => target -= 1,
            _ => break,
        }
    }
    key[..target].to_vec()
}

fn lookup<'a>(table: &'a toml::Table, key: &[String]) -> Option<&'a toml::Value> {
    let (first, rest) = key.split_first()?;
    let value = table.get(first)?;
    if rest.is_empty() {
        return Some(value);
    }
    match value {
        toml::Value::Table(table) => lookup(table, rest),
        _ => None,
    }
}

/// The project whose file `file` (`<project>/.codex/config.toml`) is.
fn project_of(file: &Path) -> Result<&Path, ApplyError> {
    file.parent()
        .and_then(Path::parent)
        .ok_or_else(|| ApplyError::Failed(format!("{} is not in a project.", file.display())))
}

/// Edits the project's file, then keeps it out of the repository for "just me", or takes a
/// "just me" exclusion away once it is shared. [`check`] has passed.
fn project_file(
    file: &Path,
    edit: &TomlEdit,
    shared: bool,
    env: &[(String, String)],
) -> Result<(), ApplyError> {
    let project = project_of(file)?;
    let (text, new) = read_and_edit(file, edit)?;
    if let Some(repo) = Repo::find(project, file, env) {
        let line = repo.exclude_line();
        if shared {
            remove_line(&repo.exclude, &line)
        } else {
            add_line(&repo.exclude, &line)
        }
        .map_err(|err| {
            ApplyError::Failed(format!(
                "Brigadier couldn't update {}: {err}",
                repo.exclude.display()
            ))
        })?;
    }
    // The user may have changed it meanwhile (an editor, a terminal): the edit is made again on
    // what it holds now, never over it.
    let (text, new) = match read_and_edit(file, edit)? {
        (now, again) if now != text => (now, again),
        _ => (text, new),
    };
    if new != text {
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|err| failed(file, "create the folder of", &err))?;
        }
        write_whole(file, &new).map_err(|err| failed(file, "write", &err))?;
    }
    Ok(())
}

/// Writes `text` to `file` whole: into a file beside it, then moved over it.
fn write_whole(file: &Path, text: &str) -> std::io::Result<()> {
    let name = file
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = file.with_file_name(format!(".{name}.brigadier-{}", std::process::id()));
    std::fs::write(&temp, text)?;
    std::fs::rename(&temp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

fn failed(file: &Path, what: &str, err: &dyn fmt::Display) -> ApplyError {
    ApplyError::Failed(format!(
        "Brigadier couldn't {what} {}: {err}",
        file.display()
    ))
}

/// The file's text (empty when missing), and that text with `edit` made.
fn read_and_edit(file: &Path, edit: &TomlEdit) -> Result<(String, String), ApplyError> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(failed(file, "read", &err)),
    };
    let new = edit_text(&text, edit).map_err(|err| failed(file, "edit", &err))?;
    Ok((text, new))
}

/// `text` with `edit` made, everything else in it as it was: comments, order, other entries.
pub(super) fn edit_text(text: &str, edit: &TomlEdit) -> Result<String, String> {
    if edit.key.is_empty() {
        return Err("the edit names no key".to_owned());
    }
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|err: toml_edit::TomlError| format!("it isn't valid TOML ({})", err.message()))?;
    let holder = &edit.key[..edit.key.len() - 1];
    let new_table = find_mut(doc.as_item_mut(), holder).is_none();
    match &edit.value {
        Some(value) => set(doc.as_item_mut(), &edit.key, edit_value(value))?,
        None => {
            remove(doc.as_item_mut(), &edit.key)?;
        }
    }
    let mut new = doc.to_string();
    // A new table at the end goes after the file's closing comments, not before them.
    let trailing = doc.trailing().as_str().unwrap_or_default().to_owned();
    if edit.value.is_some()
        && new_table
        && !trailing.trim().is_empty()
        && let Some(Item::Table(table)) = find_mut(doc.as_item_mut(), holder)
        && !table.is_dotted()
    {
        table.decor_mut().set_prefix(format!("{trailing}\n"));
        doc.set_trailing("");
        let moved = doc.to_string();
        if moved.starts_with(text) {
            new = moved;
        }
    }
    // What was written must read back as exactly that.
    let read = new
        .parse::<toml::Table>()
        .map_err(|err| format!("the edit did not read back ({err})"))?;
    if lookup(&read, &edit.key) != edit.value.as_ref() {
        return Err("the edit did not read back as written".to_owned());
    }
    Ok(new)
}

/// Sets `key` under the table `item`, making the tables on the way.
fn set(item: &mut Item, key: &[String], value: toml_edit::Value) -> Result<(), String> {
    let dotted = matches!(item, Item::Table(table) if table.is_dotted());
    let table = item
        .as_table_like_mut()
        .ok_or_else(|| "a key on the way is not a table".to_owned())?;
    let (first, rest) = key.split_first().ok_or("the edit names no key")?;
    if rest.is_empty() {
        match table.get_mut(first) {
            Some(Item::Value(old)) => {
                let decor = old.decor().clone();
                *old = value;
                *old.decor_mut() = decor;
            }
            Some(Item::None) | None => {
                table.insert(first, Item::Value(value));
            }
            Some(_) => return Err(format!("`{first}` is a table, not a value")),
        }
        return Ok(());
    }
    if table.get(first).is_none_or(Item::is_none) {
        let mut child = Table::new();
        child.set_implicit(true);
        child.set_dotted(dotted);
        table.insert(first, Item::Table(child));
    }
    let child = table
        .get_mut(first)
        .ok_or_else(|| format!("`{first}` went missing"))?;
    set(child, rest, value)
}

fn find_mut<'a>(item: &'a mut Item, key: &[String]) -> Option<&'a mut Item> {
    match key.split_first() {
        None => Some(item),
        Some((first, rest)) => find_mut(item.as_table_like_mut()?.get_mut(first)?, rest),
    }
}

/// Removes `key` under the table `item`, and each table the removal leaves empty; whether
/// `item` itself is now empty.
fn remove(item: &mut Item, key: &[String]) -> Result<bool, String> {
    let Some(table) = item.as_table_like_mut() else {
        return Ok(false);
    };
    let (first, rest) = key.split_first().ok_or("the edit names no key")?;
    if rest.is_empty() {
        return Ok(table.remove(first).is_some() && table.is_empty());
    }
    let Some(child) = table.get_mut(first) else {
        return Ok(false);
    };
    if remove(child, rest)? {
        table.remove(first);
        return Ok(table.is_empty());
    }
    Ok(false)
}

fn edit_value(value: &toml::Value) -> toml_edit::Value {
    match value {
        toml::Value::String(text) => text.as_str().into(),
        toml::Value::Integer(number) => (*number).into(),
        toml::Value::Float(number) => (*number).into(),
        toml::Value::Boolean(on) => (*on).into(),
        toml::Value::Datetime(when) => when
            .to_string()
            .parse::<toml_edit::Value>()
            .unwrap_or_else(|_| when.to_string().into()),
        toml::Value::Array(items) => items
            .iter()
            .map(edit_value)
            .collect::<toml_edit::Array>()
            .into(),
        toml::Value::Table(table) => table
            .iter()
            .map(|(key, value)| (key.as_str(), edit_value(value)))
            .collect::<toml_edit::InlineTable>()
            .into(),
    }
}

/// The git checkout a project's file is in.
struct Repo {
    /// The file's path from the checkout's top, with `/`.
    file: String,
    /// The checkout's `info/exclude` (shared by its linked worktrees).
    exclude: PathBuf,
    top: PathBuf,
}

impl Repo {
    /// `None` when the project is not in a git checkout (or git can't run).
    fn find(project: &Path, file: &Path, env: &[(String, String)]) -> Option<Self> {
        let top = PathBuf::from(git(project, &["rev-parse", "--show-toplevel"], env)?);
        let exclude = git(&top, &["rev-parse", "--git-path", "info/exclude"], env)?;
        let project = std::fs::canonicalize(project).ok()?;
        let in_project = file.strip_prefix(project_of(file).ok()?).ok()?;
        let rel = project.join(in_project);
        let rel = rel
            .strip_prefix(std::fs::canonicalize(&top).unwrap_or_else(|_| top.clone()))
            .ok()?;
        let file = rel
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        Some(Self {
            file,
            exclude: top.join(exclude),
            top,
        })
    }

    /// Whether git tracks the file (committed or staged).
    fn tracks_file(&self, env: &[(String, String)]) -> bool {
        let literal = format!(":(literal){}", self.file);
        git_command(&self.top, env)
            .args(["ls-files", "--error-unmatch", "--", &literal])
            .output()
            .is_ok_and(|output| output.status.success())
    }

    /// The exclude line for the file: anchored to the checkout's top, special characters
    /// escaped.
    fn exclude_line(&self) -> String {
        let mut line = String::from("/");
        for c in self.file.chars() {
            if matches!(c, '*' | '?' | '[' | '\\') {
                line.push('\\');
            }
            line.push(c);
        }
        if line.ends_with(' ') {
            line.pop();
            line.push_str("\\ ");
        }
        line
    }
}

fn git_command(dir: &Path, env: &[(String, String)]) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null());
    cmd
}

/// `git`'s one-line answer, when it succeeds.
fn git(dir: &Path, args: &[&str], env: &[(String, String)]) -> Option<String> {
    let output = git_command(dir, env).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let out = String::from_utf8(output.stdout).ok()?;
    let out = out.trim_end_matches(['\n', '\r']);
    (!out.is_empty()).then(|| out.to_owned())
}

/// Adds `line` to the file at `path` once, making the file (and its folder) when missing.
fn add_line(path: &Path, line: &str) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    if text.lines().any(|existing| existing == line) {
        return Ok(());
    }
    let mut new = text;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(line);
    new.push('\n');
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, new)
}

/// Takes `line` out of the file at `path`, when it is there.
fn remove_line(path: &Path, line: &str) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let kept: String = text
        .split_inclusive('\n')
        .filter(|existing| existing.trim_end_matches(['\n', '\r']) != line)
        .collect();
    if kept == text {
        return Ok(());
    }
    std::fs::write(path, kept)
}

#[cfg(test)]
#[path = "apply_tests.rs"]
mod tests;
