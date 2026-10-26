//! More than one login per CLI: each extra account is a CLI home of its own that only Brigadier
//! uses (`CLAUDE_CONFIG_DIR` for Claude, `CODEX_HOME` for Codex). The CLI keeps its own login
//! in it, in its own way (Claude Code keys its Keychain entry on the folder; Codex writes
//! `auth.json` or a keyring entry keyed on it); Brigadier never reads or writes a credential.
//! The user's own login, the one their terminal uses, stays the first account and is never
//! touched.
//!
//! Every account shares the user's chat history and their personal CLI files: the account's
//! home links each of [`shared`] back to the main home. Both CLIs resume a session only from
//! their own home (Claude from `projects`, Codex from `sessions`), so the links are what let a
//! conversation move from one account to another and still resume (checked with Claude Code
//! 2.1.295 and codex-cli 0.161.0, `docs/evidence/2026-10-09-accounts.md`).
//!
//! A login in the environment would win over the home's own login, so an account's processes
//! run without the variables in [`auth_vars`].
//!
//! Claude keeps the user's MCP servers in its config file, beside the login, so that file can't
//! be linked: [`carry_servers`] copies the servers into the account's own file instead.

use std::io;
use std::path::{Path, PathBuf};

use crate::cli::CliEnv;
use crate::model::ProviderKind;

/// The variable that moves a CLI's home.
pub fn home_var(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "CLAUDE_CONFIG_DIR",
        ProviderKind::Codex => "CODEX_HOME",
    }
}

/// Variables that sign a CLI in on their own, ahead of the login kept in its home: Claude
/// Code's API key, bearer token and long-lived OAuth token
/// (<https://code.claude.com/docs/en/authentication#authentication-precedence>); Codex's
/// API keys and access token (named in codex-cli 0.161.0's binary).
pub fn auth_vars(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        ProviderKind::Codex => &["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"],
    }
}

/// What an account's home links back to the main home: the session history the CLI resumes
/// from, and the user's personal instructions, agents, skills and the like. Folders
/// ([`Shared::Dir`]) are made in the main home when missing, so a session written from any
/// account lands where every other account (and the user's terminal) finds it.
pub fn shared(kind: ProviderKind) -> &'static [Shared] {
    use Shared::{Dir, Optional};
    match kind {
        ProviderKind::Claude => &[
            Dir("projects"),
            Optional("CLAUDE.md"),
            Optional("agents"),
            Optional("commands"),
            Optional("skills"),
            // The installed plugins, and the settings that enable them and name their
            // marketplaces (`enabledPlugins`, `extraKnownMarketplaces`): the plugins folder
            // alone loads none of them. Claude writes the settings through the link.
            Optional("plugins"),
            Optional("settings.json"),
        ],
        ProviderKind::Codex => &[
            Dir("sessions"),
            Dir("archived_sessions"),
            Dir("generated_images"),
            Optional("config.toml"),
            Optional("AGENTS.md"),
            Optional("skills"),
            Optional("prompts"),
            Optional("rules"),
            // The installed plugins (`config.toml` enables them) and the user's hooks.
            Optional("plugins"),
            Optional("hooks.json"),
        ],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shared {
    /// A folder every account needs: made in the main home when missing.
    Dir(&'static str),
    /// Linked only while the main home has it.
    Optional(&'static str),
}

impl Shared {
    pub fn name(self) -> &'static str {
        match self {
            Self::Dir(name) | Self::Optional(name) => name,
        }
    }
}

/// The user's own home for `kind`, the one their terminal uses: what `env` sets, else the
/// default (`~/.claude`, `~/.codex`).
pub fn main_home(kind: ProviderKind, env: &CliEnv) -> Option<PathBuf> {
    match env.var(home_var(kind)).filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => env.home().map(|home| {
            home.join(match kind {
                ProviderKind::Claude => ".claude",
                ProviderKind::Codex => ".codex",
            })
        }),
    }
}

impl CliEnv {
    /// This environment for an account whose CLI home is `home`: the home set, and no login
    /// variable left that would win over the home's own login.
    pub fn for_account(&self, kind: ProviderKind, home: &Path) -> CliEnv {
        let mut vars: Vec<_> = self
            .vars()
            .into_iter()
            .filter(|(key, _)| {
                key.to_str()
                    .is_none_or(|key| !auth_vars(kind).contains(&key) && key != home_var(kind))
            })
            .collect();
        vars.push((home_var(kind).into(), home.as_os_str().to_owned()));
        self.with_vars(vars)
    }
}

/// Makes an account's home (owner-only) and links it to the main home `main` (see
/// [`shared`]). Safe to repeat: an existing link is kept, and so is anything the CLI put in
/// its place. A link the main home no longer backs is left for the CLI to report.
///
/// A new Claude home starts with a `.claude.json` that says its first-run setup is done
/// (`hasCompletedOnboarding`, the key Claude Code 2.1.295 keeps there): the user went
/// through it with their own login, and a terminal that resumes a session on this account
/// must not ask again. Nothing else of the user's own `.claude.json` is copied.
pub fn prepare_home(kind: ProviderKind, main: &Path, home: &Path) -> io::Result<()> {
    create_private_dir(home)?;
    if kind == ProviderKind::Claude {
        let config = home.join(".claude.json");
        if std::fs::symlink_metadata(&config).is_err() {
            std::fs::write(&config, "{\n  \"hasCompletedOnboarding\": true\n}\n")?;
        }
    }
    for entry in shared(kind) {
        let target = main.join(entry.name());
        if let Shared::Dir(_) = entry {
            std::fs::create_dir_all(&target)?;
        }
        let link = home.join(entry.name());
        if std::fs::symlink_metadata(&link).is_ok() {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&target) else {
            continue;
        };
        link_to(&target, &link, meta.is_dir())?;
    }
    Ok(())
}

/// Claude's config file for the user's own login, where their user servers (`mcpServers`) and
/// each project's local ones (`projects.<path>.mcpServers`) are: `.claude.json` in
/// `CLAUDE_CONFIG_DIR`, else in the home folder, as Claude Code itself picks it (an old
/// `.config.json` in the config folder wins when present).
pub fn main_config_file(env: &CliEnv) -> Option<PathBuf> {
    let set = env
        .var(home_var(ProviderKind::Claude))
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from);
    let home = env.home().or_else(|| set.clone())?;
    Some(brigadier_registry::claude_config::config_file(
        &home,
        set.as_deref(),
    ))
}

/// What [`carry_servers`] last copied from the main config file, kept in the account's home.
const CARRIED: &str = ".brigadier-carried.json";

/// Copies the user's MCP servers from the main config file `main` (read only) into the
/// account's own `.claude.json` in `home`, leaving its login and everything else its own.
/// Run before each session, so a server the user adds is there at the next start. The servers
/// each project switches off (`disabledMcpServers`, what `/mcp` and Brigadier's per-project
/// switches write) come with them.
///
/// A three-way merge on what was copied last time: a server the account changed or added
/// itself (a `claude mcp add` in one of its sessions) stays, one the main file added,
/// changed or removed follows it there; likewise each off flag.
///
/// The account's sessions write the file too, some under Claude Code's lock and some without:
/// the merge runs under that lock and goes in only if the file is still what it was read from,
/// so no write of theirs is lost.
pub fn carry_servers(main: &Path, home: &Path) -> io::Result<()> {
    carry_servers_with_settle(main, home, || std::thread::sleep(SETTLE))
}

fn carry_servers_with_settle(main: &Path, home: &Path, mut settle: impl FnMut()) -> io::Result<()> {
    let main = read_json(main)?.unwrap_or_default();
    let config_path = home.join(".claude.json");
    let carried_path = home.join(CARRIED);
    let last = read_json(&carried_path).ok().flatten().unwrap_or_default();
    let Some((wrote, now)) = carry_once(&config_path, |config| carried(&main, &last, config))?
    else {
        return Ok(());
    };
    // A session that read the file just before the rename may write its older copy over it:
    // a moment later, whatever it took back is put back (only after a change, which is rare).
    for _ in 0..2 {
        if wrote.is_empty() {
            break;
        }
        settle();
        let config = read_json(&config_path)?.unwrap_or_default();
        if !wrote
            .iter()
            .any(|edit| edit.current(&config) == edit.before)
        {
            break;
        }
        carry_once(&config_path, |mut config| {
            let observed = serde_json::Value::Object(config.clone());
            for edit in &wrote {
                // A value different from both snapshots is the user's edit. Entries this
                // carry never changed aren't in wrote at all, including user removals.
                if edit.current(&observed) == edit.before {
                    edit.restore(&mut config);
                }
            }
            (config, serde_json::Map::new())
        })?;
    }
    if last != serde_json::Value::Object(now.clone()) {
        write_private(&carried_path, &serde_json::Value::Object(now))?;
    }
    Ok(())
}

/// How long after a carry its values are checked again.
const SETTLE: std::time::Duration = std::time::Duration::from_millis(100);

/// Each server/flag this carry changed, and the main values carried now.
type Carried = (Vec<CarryEdit>, serde_json::Map<String, serde_json::Value>);

struct CarryEdit {
    at: String,
    /// A flag is membership of one name, never a replacement of the whole array.
    flag: Option<String>,
    before: Option<serde_json::Value>,
    after: Option<serde_json::Value>,
}

impl CarryEdit {
    fn current(&self, config: &serde_json::Value) -> Option<serde_json::Value> {
        match &self.flag {
            None => config.pointer(&self.at).cloned(),
            Some(name) => Some(serde_json::Value::Bool(
                config
                    .pointer(&self.at)
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|flags| flags.iter().any(|flag| flag.as_str() == Some(name))),
            )),
        }
    }

    fn restore(&self, config: &mut serde_json::Map<String, serde_json::Value>) {
        let value = match &self.flag {
            None => self.after.clone(),
            Some(name) => {
                let mut flags = serde_json::Value::Object(config.clone())
                    .pointer(&self.at)
                    .and_then(serde_json::Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                flags.retain(|flag| flag.as_str() != Some(name));
                if self.after == Some(serde_json::Value::Bool(true)) {
                    flags.push(name.clone().into());
                }
                Some(serde_json::Value::Array(flags))
            }
        };
        put(config, &self.at, value);
    }
}

/// Record only actual per-name changes, including removals. Retrying a captured container
/// would overwrite unrelated edits made during settle or between the check and the lock.
fn carry_edits(before: &serde_json::Value, after: &serde_json::Value) -> Vec<CarryEdit> {
    use serde_json::Value;
    let paths = carried_values(before)
        .into_iter()
        .chain(carried_values(after))
        .map(|(path, _)| path)
        .collect::<std::collections::BTreeSet<_>>();
    let mut edits = Vec::new();
    for at in paths {
        let flag = at.ends_with("/disabledMcpServers");
        let names = |config: &Value| -> Vec<String> {
            if flag {
                config
                    .pointer(&at)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            } else {
                config
                    .pointer(&at)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| name.clone())
                    .collect()
            }
        };
        let names = names(before)
            .into_iter()
            .chain(names(after))
            .collect::<std::collections::BTreeSet<_>>();
        for name in names {
            let mut edit = CarryEdit {
                at: if flag {
                    at.clone()
                } else {
                    format!("{at}/{}", name.replace('~', "~0").replace('/', "~1"))
                },
                flag: flag.then_some(name),
                before: None,
                after: None,
            };
            edit.before = edit.current(before);
            edit.after = edit.current(after);
            if edit.before != edit.after {
                edits.push(edit);
            }
        }
    }
    edits
}

/// Runs `merge` on the account's config file at `path` and writes the result, under Claude
/// Code's lock and only if the file is still what was read (else merges again from what it
/// is). `None` when there is no file to carry into.
fn carry_once(
    path: &Path,
    merge: impl Fn(
        serde_json::Map<String, serde_json::Value>,
    ) -> (
        serde_json::Map<String, serde_json::Value>,
        serde_json::Map<String, serde_json::Value>,
    ),
) -> io::Result<Option<Carried>> {
    let mut result = None;
    brigadier_registry::claude_config::rewrite_locked(path, |before| {
        if before.is_empty() && !path.exists() {
            return Ok(None);
        }
        let value: serde_json::Value = serde_json::from_str(before).map_err(|e| e.to_string())?;
        let serde_json::Value::Object(config) = value.clone() else {
            return Ok(None);
        };
        let (config, now) = merge(config);
        let after = serde_json::Value::Object(config);
        let wrote = carry_edits(&value, &after);
        result = Some((wrote, now));
        if after == value {
            Ok(None)
        } else {
            serde_json::to_string_pretty(&after)
                .map(|text| Some(text + "\n"))
                .map_err(|e| e.to_string())
        }
    })
    .map_err(io::Error::other)?;
    Ok(result)
}

/// The values carrying sets in `config`, by JSON pointer: the user's servers, and each
/// project's servers and off flags.
fn carried_values(config: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    let escape = |key: &str| key.replace('~', "~0").replace('/', "~1");
    let mut values = Vec::new();
    if let Some(servers) = config.get("mcpServers") {
        values.push(("/mcpServers".to_owned(), servers.clone()));
    }
    for (path, project) in config
        .get("projects")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flatten()
    {
        for key in ["mcpServers", "disabledMcpServers"] {
            if let Some(value) = project.get(key) {
                values.push((format!("/projects/{}/{key}", escape(path)), value.clone()));
            }
        }
    }
    values
}

/// `config` with `value` at JSON pointer `at` (`/a/b/c`), making the objects on the way.
fn put(
    config: &mut serde_json::Map<String, serde_json::Value>,
    at: &str,
    value: Option<serde_json::Value>,
) {
    let keys: Vec<String> = at
        .split('/')
        .skip(1)
        .map(|key| key.replace("~1", "/").replace("~0", "~"))
        .collect();
    let Some((last, path)) = keys.split_last() else {
        return;
    };
    let mut holder = config;
    for key in path {
        let next = holder
            .entry(key.clone())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if !next.is_object() {
            *next = serde_json::Value::Object(serde_json::Map::new());
        }
        let serde_json::Value::Object(next) = next else {
            return;
        };
        holder = next;
    }
    match value {
        Some(value) => {
            holder.insert(last.clone(), value);
        }
        None => {
            holder.remove(last);
        }
    }
}

/// The JSON in `path`; `None` when it isn't there.
fn read_json(path: &Path) -> io::Result<Option<serde_json::Value>> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// The account's `config` with the main file's servers and off flags merged in against `last`
/// (what was carried before), and what is carried now.
fn carried(
    main: &serde_json::Value,
    last: &serde_json::Value,
    mut config: serde_json::Map<String, serde_json::Value>,
) -> (
    serde_json::Map<String, serde_json::Value>,
    serde_json::Map<String, serde_json::Value>,
) {
    use serde_json::{Map, Value};
    let entry = |root: &Value, project: Option<&str>| -> Option<Value> {
        match project {
            Some(path) => root.get("projects").and_then(|all| all.get(path)).cloned(),
            None => Some(root.clone()),
        }
    };
    let servers = |root: &Value, project: Option<&str>| -> Map<String, Value> {
        entry(root, project)
            .and_then(|holder| holder.get("mcpServers").cloned())
            .and_then(|servers| servers.as_object().cloned())
            .unwrap_or_default()
    };
    let flags = |root: &Value, path: &str| -> Vec<String> {
        entry(root, Some(path))
            .and_then(|holder| holder.get("disabledMcpServers").cloned())
            .and_then(|flags| flags.as_array().cloned())
            .into_iter()
            .flatten()
            .filter_map(|flag| flag.as_str().map(str::to_owned))
            .collect()
    };
    let merge =
        |mine: &mut Map<String, Value>, theirs: &Map<String, Value>, was: &Map<String, Value>| {
            let names: std::collections::BTreeSet<String> =
                theirs.keys().chain(was.keys()).cloned().collect();
            for name in names {
                if mine.get(&name) != was.get(&name) {
                    continue;
                }
                match theirs.get(&name) {
                    Some(server) => mine.insert(name, server.clone()),
                    None => mine.remove(&name),
                };
            }
        };

    let mut now = Map::new();
    let main_user = servers(main, None);
    let mut user = servers(&Value::Object(config.clone()), None);
    merge(&mut user, &main_user, &servers(last, None));
    now.insert("mcpServers".into(), Value::Object(main_user));

    let mut projects: std::collections::BTreeSet<String> = Default::default();
    for root in [main, last] {
        if let Some(all) = root.get("projects").and_then(Value::as_object) {
            projects.extend(all.keys().cloned());
        }
    }
    let mut carried_projects = Map::new();
    let mut project_servers = Vec::new();
    let mut project_flags = Vec::new();
    for path in projects {
        let theirs = servers(main, Some(&path));
        let was = servers(last, Some(&path));
        let had = servers(&Value::Object(config.clone()), Some(&path));
        let mut mine = had.clone();
        merge(&mut mine, &theirs, &was);
        // Off flags: one the main file gained since last time comes, one it lost goes; the
        // account's own stay.
        let their_flags = flags(main, &path);
        let was_flags = flags(last, &path);
        let had_flags = flags(&Value::Object(config.clone()), &path);
        let mut my_flags = had_flags.clone();
        for flag in their_flags.iter().filter(|flag| !was_flags.contains(flag)) {
            if !my_flags.contains(flag) {
                my_flags.push(flag.clone());
            }
        }
        my_flags.retain(|flag| their_flags.contains(flag) || !was_flags.contains(flag));
        let mut kept = Map::new();
        if !theirs.is_empty() {
            kept.insert("mcpServers".into(), Value::Object(theirs));
        }
        if !their_flags.is_empty() {
            kept.insert(
                "disabledMcpServers".into(),
                Value::Array(their_flags.into_iter().map(Value::String).collect()),
            );
        }
        if !kept.is_empty() {
            carried_projects.insert(path.clone(), Value::Object(kept));
        }
        if mine != had {
            project_servers.push((path.clone(), mine));
        }
        if my_flags != had_flags {
            project_flags.push((path, my_flags));
        }
    }
    now.insert("projects".into(), Value::Object(carried_projects));

    config.insert("mcpServers".into(), Value::Object(user));
    if !project_servers.is_empty() || !project_flags.is_empty() {
        let all = config
            .entry("projects")
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(all) = all {
            for (path, mine) in project_servers {
                if let Value::Object(project) =
                    all.entry(path).or_insert_with(|| Value::Object(Map::new()))
                {
                    project.insert("mcpServers".into(), Value::Object(mine));
                }
            }
            for (path, flags) in project_flags {
                if let Value::Object(project) =
                    all.entry(path).or_insert_with(|| Value::Object(Map::new()))
                {
                    project.insert(
                        "disabledMcpServers".into(),
                        Value::Array(flags.into_iter().map(Value::String).collect()),
                    );
                }
            }
        }
    }
    (config, now)
}

/// A fresh temporary file beside `path`.
fn temp_beside(path: &Path) -> PathBuf {
    path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()))
}

/// Writes `value` to `path` whole (a temporary file renamed over it), owner-only.
fn write_private(path: &Path, value: &serde_json::Value) -> io::Result<()> {
    let temp = temp_beside(path);
    write_new_private(&temp, value)?;
    std::fs::rename(&temp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Writes `value` to the new file `temp`, owner-only and on disk.
fn write_new_private(temp: &Path, value: &serde_json::Value) -> io::Result<()> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(value)? + "\n";
    let mut out = std::fs::File::create(temp)?;
    out.write_all(text.as_bytes())?;
    out.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(temp, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Deletes an account's home. Links are removed, never followed: what they point to in the
/// main home stays.
pub fn remove_home(home: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(home) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

pub(crate) fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

#[cfg(unix)]
pub(crate) fn link_to(target: &Path, link: &Path, _dir: bool) -> io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Windows: a folder is linked by a junction (no privilege needed), a file by a hard link (a
/// CLI that replaces the file then keeps its own copy).
#[cfg(windows)]
pub(crate) fn link_to(target: &Path, link: &Path, dir: bool) -> io::Result<()> {
    if dir {
        junction::create(target, link)
    } else {
        std::fs::hard_link(target, link)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    /// A fresh folder, removed after the test however it ends.
    #[cfg(unix)]
    struct Temp(PathBuf);

    #[cfg(unix)]
    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    #[cfg(unix)]
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn scratch(name: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-accounts-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }

    #[test]
    fn an_account_runs_on_its_own_home_with_no_login_variable() {
        let env = CliEnv::from_vars([
            (OsString::from("PATH"), OsString::from("/usr/bin")),
            (OsString::from("HOME"), OsString::from("/home/me")),
            (
                OsString::from("ANTHROPIC_API_KEY"),
                OsString::from("sentinel-key"),
            ),
            (
                OsString::from("ANTHROPIC_AUTH_TOKEN"),
                OsString::from("sentinel-token"),
            ),
            (
                OsString::from("CLAUDE_CODE_OAUTH_TOKEN"),
                OsString::from("sentinel-oauth"),
            ),
            (
                OsString::from("OPENAI_API_KEY"),
                OsString::from("sentinel-openai"),
            ),
            (
                OsString::from("CODEX_API_KEY"),
                OsString::from("sentinel-codex"),
            ),
            (
                OsString::from("CODEX_ACCESS_TOKEN"),
                OsString::from("sentinel-access"),
            ),
            (
                OsString::from("CLAUDE_CONFIG_DIR"),
                OsString::from("/elsewhere"),
            ),
        ]);
        let claude = env.for_account(ProviderKind::Claude, Path::new("/data/accounts/a1"));
        let codex = env.for_account(ProviderKind::Codex, Path::new("/data/accounts/a2"));
        for (account, kind) in [
            (&claude, ProviderKind::Claude),
            (&codex, ProviderKind::Codex),
        ] {
            for name in auth_vars(kind) {
                assert_eq!(account.var(name), None, "{name} reached a {kind} account");
            }
            let leaked = account
                .vars()
                .into_iter()
                .filter(|(key, value)| {
                    value.to_string_lossy().starts_with("sentinel")
                        && auth_vars(kind).contains(&key.to_str().unwrap())
                })
                .count();
            assert_eq!(leaked, 0);
            assert_eq!(account.var("PATH"), Some(std::ffi::OsStr::new("/usr/bin")));
        }
        assert_eq!(
            claude.var("CLAUDE_CONFIG_DIR"),
            Some(std::ffi::OsStr::new("/data/accounts/a1"))
        );
        assert_eq!(
            codex.var("CODEX_HOME"),
            Some(std::ffi::OsStr::new("/data/accounts/a2"))
        );
        // The user's own login (the base environment) keeps every variable.
        assert_eq!(
            env.var("ANTHROPIC_API_KEY"),
            Some(std::ffi::OsStr::new("sentinel-key"))
        );
    }

    #[test]
    fn the_main_home_is_the_variable_or_the_default() {
        let set = CliEnv::from_vars([(OsString::from("CODEX_HOME"), OsString::from("/x"))]);
        assert_eq!(
            main_home(ProviderKind::Codex, &set),
            Some(PathBuf::from("/x"))
        );
        let unset = CliEnv::from_vars([(OsString::from("HOME"), OsString::from("/home/me"))]);
        assert_eq!(
            main_home(ProviderKind::Claude, &unset),
            Some(PathBuf::from("/home/me/.claude"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_home_links_the_shared_history_and_removing_it_keeps_the_main_home() {
        let root = scratch("links");
        let main = root.join("main");
        std::fs::create_dir_all(main.join("skills")).unwrap();
        std::fs::write(main.join("CLAUDE.md"), "be brief").unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        // `projects` is made in the main home, and linked.
        assert!(main.join("projects").is_dir());
        for name in ["projects", "skills", "CLAUDE.md"] {
            assert_eq!(
                std::fs::read_link(home.join(name)).unwrap(),
                main.join(name),
                "{name}"
            );
        }
        // Missing in the main home: not linked.
        assert!(std::fs::symlink_metadata(home.join("agents")).is_err());
        // Its own config, with the first-run setup done; kept as the CLI leaves it.
        let config = home.join(".claude.json");
        let first: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(first["hasCompletedOnboarding"], true);
        std::fs::write(&config, "{}").unwrap();
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        assert_eq!(std::fs::read_to_string(&config).unwrap(), "{}");
        // Owner-only.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&home).unwrap().permissions().mode() & 0o777,
            0o700
        );
        // A session written through the account lands in the main home.
        std::fs::create_dir_all(home.join("projects/-work")).unwrap();
        std::fs::write(home.join("projects/-work/s1.jsonl"), "{}").unwrap();
        assert!(main.join("projects/-work/s1.jsonl").is_file());
        // Again: nothing changes; something the CLI put in a link's place stays.
        std::fs::create_dir_all(main.join("agents")).unwrap();
        std::fs::create_dir(home.join("agents")).unwrap();
        prepare_home(ProviderKind::Claude, &main, &home).unwrap();
        assert!(
            !std::fs::symlink_metadata(home.join("agents"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::write(home.join(".claude.json"), "{}").unwrap();

        remove_home(&home).unwrap();
        assert!(!home.exists());
        assert!(main.join("projects/-work/s1.jsonl").is_file());
        assert_eq!(
            std::fs::read_to_string(main.join("CLAUDE.md")).unwrap(),
            "be brief"
        );
        assert!(main.join("skills").is_dir());
        // Already gone: fine.
        remove_home(&home).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_codex_home_shares_sessions_archive_and_config() {
        let root = scratch("codex");
        let main = root.join("main");
        std::fs::create_dir_all(&main).unwrap();
        std::fs::write(main.join("config.toml"), "model = \"x\"\n").unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Codex, &main, &home).unwrap();
        for name in ["sessions", "archived_sessions"] {
            assert!(main.join(name).is_dir(), "{name}");
            assert_eq!(
                std::fs::read_link(home.join(name)).unwrap(),
                main.join(name)
            );
        }
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "model = \"x\"\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_account_gets_the_users_servers_and_keeps_its_own() {
        use serde_json::{Value, json};
        let root = scratch("carry");
        let main_dir = root.join("main");
        std::fs::create_dir_all(&main_dir).unwrap();
        let main = main_dir.join(".claude.json");
        let write = |path: &Path, value: Value| {
            std::fs::write(path, serde_json::to_string(&value).unwrap()).unwrap()
        };
        let read = |path: &Path| -> Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        };
        let server = |cmd: &str| json!({ "type": "stdio", "command": cmd, "args": [] });
        write(
            &main,
            json!({
                "oauthAccount": { "emailAddress": "main@example.com" },
                "userID": "main-user",
                "mcpServers": { "docs": server("docs"), "gone": server("gone") },
                "projects": {
                    "/work/a": { "mcpServers": { "db": server("db") }, "hasTrustDialogAccepted": true },
                    "/work/b": { "allowedTools": [] }
                }
            }),
        );
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        let config = home.join(".claude.json");
        let mut own = read(&config);
        own["oauthAccount"] = json!({ "emailAddress": "work@example.com" });
        own["userID"] = json!("work-user");
        write(&config, own);
        let main_text = std::fs::read_to_string(&main).unwrap();

        carry_servers(&main, &home).unwrap();
        let got = read(&config);
        assert_eq!(
            got["mcpServers"],
            json!({ "docs": server("docs"), "gone": server("gone") })
        );
        assert_eq!(
            got["projects"]["/work/a"]["mcpServers"],
            json!({ "db": server("db") })
        );
        // Only the servers: the login, the identity and the trust stay the account's own.
        assert_eq!(got["oauthAccount"]["emailAddress"], "work@example.com");
        assert_eq!(got["userID"], "work-user");
        assert_eq!(
            got["projects"]["/work/a"].get("hasTrustDialogAccepted"),
            None
        );
        assert_eq!(got["projects"].get("/work/b"), None);
        assert_eq!(got["hasCompletedOnboarding"], true);
        // The main file is only read.
        assert_eq!(std::fs::read_to_string(&main).unwrap(), main_text);

        // A session on the account adds and changes servers of its own.
        let mut own = read(&config);
        own["mcpServers"]["mine"] = server("mine");
        own["mcpServers"]["docs"] = server("docs-mine");
        own["projects"]["/work/a"]["mcpServers"]["local"] = server("local");
        write(&config, own);
        // The user adds one, changes one and removes one in their own file.
        let mut theirs = read(&main);
        theirs["mcpServers"] = json!({ "docs": server("docs-2"), "new": server("new") });
        theirs["projects"]["/work/a"]["mcpServers"]["db"] = server("db-2");
        write(&main, theirs);

        carry_servers(&main, &home).unwrap();
        let got = read(&config);
        assert_eq!(
            got["mcpServers"],
            json!({ "docs": server("docs-mine"), "mine": server("mine"), "new": server("new") })
        );
        assert_eq!(
            got["projects"]["/work/a"]["mcpServers"],
            json!({ "db": server("db-2"), "local": server("local") })
        );
        // Again: nothing to do, the file is left as it is.
        let before = std::fs::metadata(&config).unwrap().modified().unwrap();
        carry_servers(&main, &home).unwrap();
        assert_eq!(
            std::fs::metadata(&config).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn an_account_gets_the_users_off_flags_and_keeps_its_own() {
        use serde_json::{Value, json};
        let root = scratch("carry-flags");
        let main_dir = root.join("main");
        std::fs::create_dir_all(&main_dir).unwrap();
        let main = main_dir.join(".claude.json");
        let write = |path: &Path, value: Value| {
            std::fs::write(path, serde_json::to_string(&value).unwrap()).unwrap()
        };
        let read = |path: &Path| -> Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        };
        write(
            &main,
            json!({
                "projects": {
                    "/work/a": { "disabledMcpServers": ["railway", "plugin:copy:railway"] },
                    "/work/b": { "allowedTools": [] }
                }
            }),
        );
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        let config = home.join(".claude.json");
        carry_servers(&main, &home).unwrap();
        let got = read(&config);
        assert_eq!(
            got["projects"]["/work/a"]["disabledMcpServers"],
            json!(["railway", "plugin:copy:railway"])
        );
        // A project with nothing to carry gets no entry.
        assert_eq!(got["projects"].get("/work/b"), None);

        // The account switches one more off itself (`/mcp` in one of its sessions); the user
        // switches railway back on and another off in their own file.
        let mut own = read(&config);
        own["projects"]["/work/a"]["disabledMcpServers"] =
            json!(["railway", "plugin:copy:railway", "mine"]);
        write(&config, own);
        write(
            &main,
            json!({
                "projects": {
                    "/work/a": { "disabledMcpServers": ["plugin:copy:railway", "figma"] }
                }
            }),
        );
        carry_servers(&main, &home).unwrap();
        assert_eq!(
            read(&config)["projects"]["/work/a"]["disabledMcpServers"],
            json!(["plugin:copy:railway", "mine", "figma"])
        );
        // Again: nothing to do, the file is left as it is.
        let before = std::fs::metadata(&config).unwrap().modified().unwrap();
        carry_servers(&main, &home).unwrap();
        assert_eq!(
            std::fs::metadata(&config).unwrap().modified().unwrap(),
            before
        );
    }

    #[test]
    fn an_account_server_added_during_carry_settle_is_preserved() {
        use serde_json::{Value, json};
        let root = scratch("carry-settle-add");
        let main_dir = root.join("main");
        let main = main_dir.join(".claude.json");
        std::fs::create_dir_all(&main_dir).unwrap();
        std::fs::write(
            &main,
            json!({"mcpServers":{"carried":{"command":"main"}}}).to_string(),
        )
        .unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        let config = home.join(".claude.json");
        carry_servers_with_settle(&main, &home, || {
            let mut value: Value =
                serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
            value["mcpServers"]["mine"] = json!({"command":"user-added"});
            std::fs::write(&config, value.to_string()).unwrap();
        })
        .unwrap();
        let got: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(got["mcpServers"]["mine"], json!({"command":"user-added"}));
        assert_eq!(got["mcpServers"]["carried"], json!({"command":"main"}));
    }

    #[test]
    fn carry_settle_recovers_only_its_deltas_and_preserves_user_edits_and_flags() {
        use serde_json::{Value, json};
        let root = scratch("carry-settle-edits");
        let main_dir = root.join("main");
        std::fs::create_dir_all(&main_dir).unwrap();
        let main = main_dir.join(".claude.json");
        let home = root.join("account");
        let before_servers = json!({
            "keep_change":{"command":"k"}, "keep_delete":{"command":"d"},
            "updated":{"command":"u0"}, "recover":{"command":"r0"},
            "removed_main":{"command":"gone"}
        });
        let before = json!({"mcpServers": before_servers, "projects": {
            "/work/a~b": {"mcpServers":before_servers, "disabledMcpServers":["kept_off","removed_from_main"]}
        }});
        std::fs::write(&main, before.to_string()).unwrap();
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        carry_servers(&main, &home).unwrap();
        let after_servers = json!({
            "keep_change":{"command":"k"}, "keep_delete":{"command":"d"},
            "updated":{"command":"u1"}, "recover":{"command":"r1"}, "added":{"command":"new"}
        });
        std::fs::write(&main, json!({"mcpServers":after_servers, "projects": {
            "/work/a~b": {"mcpServers":after_servers, "disabledMcpServers":["kept_off","added_from_main"]}
        }}).to_string()).unwrap();
        let config = home.join(".claude.json");
        let mut first = true;
        carry_servers_with_settle(&main, &home, || {
            if !first {
                return;
            }
            first = false;
            let mut value: Value =
                serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
            for at in ["/mcpServers", "/projects/~1work~1a~0b/mcpServers"] {
                let servers = value.pointer_mut(at).unwrap().as_object_mut().unwrap();
                servers.insert("mine".into(), json!({"command":"user-added"}));
                servers.insert(
                    "keep_change".into(),
                    json!({"command":"user-changed-unchanged-entry"}),
                );
                servers.remove("keep_delete");
                servers.insert(
                    "updated".into(),
                    json!({"command":"user-changed-carried-entry"}),
                );
                // A stale CLI write lost these carry changes. Recover them individually.
                servers.insert("recover".into(), json!({"command":"r0"}));
                servers.insert("removed_main".into(), json!({"command":"gone"}));
                servers.remove("added");
            }
            value["projects"]["/work/a~b"]["disabledMcpServers"] =
                json!(["mine_off", "removed_from_main"]);
            value["login"] = json!("user-login-edit");
            std::fs::write(&config, value.to_string()).unwrap();
        })
        .unwrap();
        let got: Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        for at in ["/mcpServers", "/projects/~1work~1a~0b/mcpServers"] {
            let servers = got.pointer(at).unwrap();
            assert_eq!(servers["mine"]["command"], "user-added");
            assert_eq!(
                servers["keep_change"]["command"],
                "user-changed-unchanged-entry"
            );
            assert!(servers.get("keep_delete").is_none());
            assert_eq!(servers["updated"]["command"], "user-changed-carried-entry");
            assert_eq!(servers["recover"]["command"], "r1");
            assert!(servers.get("removed_main").is_none());
            assert_eq!(servers["added"]["command"], "new");
        }
        assert_eq!(
            got["projects"]["/work/a~b"]["disabledMcpServers"],
            json!(["mine_off", "added_from_main"])
        );
        assert_eq!(got["login"], "user-login-edit");
        carry_servers(&main, &home).unwrap();
        let repeated: Value =
            serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(repeated, got);
    }

    #[test]
    fn a_session_writing_the_accounts_file_meanwhile_loses_nothing() {
        use serde_json::{Value, json};
        let root = scratch("carry-race");
        let main_dir = root.join("main");
        std::fs::create_dir_all(&main_dir).unwrap();
        let main = main_dir.join(".claude.json");
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        let config = home.join(".claude.json");
        let read = |path: &Path| -> Value {
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
        };
        // Like Claude Code's unlocked writes in the account's session: read, change, write.
        let session = {
            let config = config.clone();
            std::thread::spawn(move || {
                for n in 0..40 {
                    let mut value: Value =
                        serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
                    value["numStartups"] = json!(n);
                    let temp = config.with_extension("claude-tmp");
                    std::fs::write(&temp, serde_json::to_string(&value).unwrap()).unwrap();
                    std::fs::rename(&temp, &config).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            })
        };
        for n in 0..8 {
            std::fs::write(
                &main,
                json!({ "mcpServers": { format!("s{n}"): { "command": "x" } } }).to_string(),
            )
            .unwrap();
            carry_servers(&main, &home).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        session.join().unwrap();
        carry_servers(&main, &home).unwrap();
        let got = read(&config);
        assert_eq!(got["numStartups"], json!(39));
        assert_eq!(got["mcpServers"], json!({ "s7": { "command": "x" } }));
    }

    #[test]
    fn carrying_waits_for_claude_codes_lock() {
        use serde_json::json;
        let root = scratch("carry-lock");
        let main_dir = root.join("main");
        std::fs::create_dir_all(&main_dir).unwrap();
        let main = main_dir.join(".claude.json");
        std::fs::write(
            &main,
            json!({ "mcpServers": { "docs": { "command": "d" } } }).to_string(),
        )
        .unwrap();
        let home = root.join("account");
        prepare_home(ProviderKind::Claude, &main_dir, &home).unwrap();
        let lock = home.join(".claude.json.lock");
        std::fs::create_dir(&lock).unwrap();
        let release = {
            let lock = lock.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                std::fs::remove_dir(&lock).unwrap();
            })
        };
        let start = std::time::Instant::now();
        carry_servers(&main, &home).unwrap();
        assert!(start.elapsed() >= std::time::Duration::from_millis(250));
        release.join().unwrap();
        assert!(!lock.exists());
        let got: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .unwrap();
        assert_eq!(got["mcpServers"]["docs"], json!({ "command": "d" }));
    }

    #[test]
    fn claudes_main_config_file_follows_its_config_dir_or_the_home() {
        let set = CliEnv::from_vars([
            (OsString::from("HOME"), OsString::from("/home/me")),
            (OsString::from("CLAUDE_CONFIG_DIR"), OsString::from("/cfg")),
        ]);
        assert_eq!(
            main_config_file(&set),
            Some(PathBuf::from("/cfg/.claude.json"))
        );
        let unset = CliEnv::from_vars([(OsString::from("HOME"), OsString::from("/home/me"))]);
        assert_eq!(
            main_config_file(&unset),
            Some(PathBuf::from("/home/me/.claude.json"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_account_shares_the_plugins_and_the_settings_that_enable_them() {
        let root = scratch("plugins");
        for (kind, names) in [
            (ProviderKind::Claude, &["plugins", "settings.json"][..]),
            (
                ProviderKind::Codex,
                &["plugins", "hooks.json", "config.toml"][..],
            ),
        ] {
            let main = root.join(format!("{kind}-main"));
            std::fs::create_dir_all(main.join("plugins")).unwrap();
            for file in ["settings.json", "hooks.json", "config.toml"] {
                std::fs::write(main.join(file), "").unwrap();
            }
            let home = root.join(format!("{kind}-account"));
            prepare_home(kind, &main, &home).unwrap();
            for name in names {
                assert_eq!(
                    std::fs::read_link(home.join(name)).unwrap(),
                    main.join(name)
                );
            }
        }
    }

    /// Real CLIs on throwaway homes: run with `--ignored`.
    #[cfg(unix)]
    mod live {
        use super::*;
        use std::process::Command;

        fn root(name: &str) -> PathBuf {
            let dir = PathBuf::from(format!(
                "/tmp/brigadier-accounts-live-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("os-home")).unwrap();
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::create_dir_all(dir.join("project")).unwrap();
            // A stdio MCP server that answers `initialize` and `tools/list`.
            std::fs::write(
                dir.join("bin/server.py"),
                "import json, sys\nfor line in sys.stdin:\n    req = json.loads(line)\n    if 'id' not in req:\n        continue\n    m = req.get('method')\n    r = {}\n    if m == 'initialize':\n        r = {'protocolVersion': req['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '0.1.0'}}\n    elif m == 'tools/list':\n        r = {'tools': [{'name': 'probe', 'description': 'fixture', 'inputSchema': {'type': 'object', 'properties': {}}}]}\n    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': r}), flush=True)\n",
            )
            .unwrap();
            let market = dir.join("market");
            std::fs::create_dir_all(market.join(".claude-plugin")).unwrap();
            std::fs::create_dir_all(market.join("alpha/.claude-plugin")).unwrap();
            std::fs::write(
                market.join(".claude-plugin/marketplace.json"),
                r#"{"name":"w05c-local","owner":{"name":"w05c"},"plugins":[{"name":"alpha","source":"./alpha","description":"fixture"}]}"#,
            )
            .unwrap();
            std::fs::write(
                market.join("alpha/.claude-plugin/plugin.json"),
                r#"{"name":"alpha","version":"0.1.0","description":"fixture"}"#,
            )
            .unwrap();
            std::fs::write(
                market.join("alpha/.mcp.json"),
                format!(
                    r#"{{"mcpServers":{{"shared":{{"command":"/usr/bin/python3","args":["{}","alpha"]}}}}}}"#,
                    dir.join("bin/server.py").display()
                ),
            )
            .unwrap();
            dir
        }

        fn cli(dir: &Path, program: &str, var: &str, home: &Path, args: &[&str]) -> String {
            let out = Command::new(program)
                .args(args)
                .current_dir(dir.join("project"))
                .env_clear()
                .env("PATH", std::env::var_os("PATH").unwrap())
                .env("HOME", dir.join("os-home"))
                .env(var, home)
                .env("TERM", "dumb")
                .env("NO_COLOR", "1")
                .env("DISABLE_AUTOUPDATER", "1")
                .output()
                .unwrap();
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            println!(
                "$ {var}={} {program} {}\n{text}",
                home.display(),
                args.join(" ")
            );
            assert!(out.status.success(), "{program} {args:?} failed");
            text
        }

        #[test]
        #[ignore = "runs the real claude on throwaway homes"]
        fn live_an_extra_claude_account_lists_the_users_servers_and_plugins() {
            let dir = root("claude");
            let main = dir.join("main");
            let server = format!("/usr/bin/python3 {}", dir.join("bin/server.py").display());
            let server: Vec<&str> = server.split(' ').collect();
            let claude =
                |home: &Path, args: &[&str]| cli(&dir, "claude", "CLAUDE_CONFIG_DIR", home, args);
            let mut add = vec!["mcp", "add", "user-one", "--scope", "user", "--"];
            add.extend(&server);
            claude(&main, &add);
            let mut add = vec!["mcp", "add", "local-one", "--scope", "local", "--"];
            add.extend(&server);
            add.push("local");
            claude(&main, &add);
            claude(
                &main,
                &[
                    "plugin",
                    "marketplace",
                    "add",
                    dir.join("market").to_str().unwrap(),
                ],
            );
            claude(&main, &["plugin", "install", "alpha@w05c-local"]);

            let extra = dir.join("extra");
            prepare_home(ProviderKind::Claude, &main, &extra).unwrap();
            carry_servers(&main.join(".claude.json"), &extra).unwrap();
            let listed = claude(&extra, &["mcp", "list"]);
            for name in ["user-one:", "local-one:", "plugin:alpha:shared:"] {
                assert!(listed.contains(name), "{name} missing:\n{listed}");
            }
            assert_eq!(listed.matches("Connected").count(), 3, "{listed}");
            std::fs::remove_dir_all(&dir).unwrap();
        }

        #[test]
        #[ignore = "runs the real codex on throwaway homes"]
        fn live_a_codex_mcp_add_on_an_account_survives_the_next_launch() {
            let dir = root("codex");
            let main = dir.join("main");
            std::fs::create_dir_all(&main).unwrap();
            let codex = |home: &Path, args: &[&str]| cli(&dir, "codex", "CODEX_HOME", home, args);
            codex(
                &main,
                &["mcp", "add", "user-one", "--", "/usr/bin/python3", "x.py"],
            );
            codex(
                &main,
                &[
                    "plugin",
                    "marketplace",
                    "add",
                    dir.join("market").to_str().unwrap(),
                ],
            );
            codex(&main, &["plugin", "add", "alpha@w05c-local"]);
            let main_text = std::fs::read_to_string(main.join("config.toml")).unwrap();

            let extra = dir.join("extra");
            prepare_home(ProviderKind::Codex, &main, &extra).unwrap();
            let listed = codex(&extra, &["mcp", "list"]);
            assert!(
                listed.contains("user-one") && listed.contains("shared"),
                "{listed}"
            );
            // Added inside a session on the account, as `codex mcp add` does there.
            codex(
                &extra,
                &["mcp", "add", "in-session", "--", "/usr/bin/python3", "y.py"],
            );
            // The next launch prepares the home again.
            prepare_home(ProviderKind::Codex, &main, &extra).unwrap();
            for home in [&extra, &main] {
                let listed = codex(home, &["mcp", "list", "--json"]);
                assert!(listed.contains("\"in-session\""), "{listed}");
            }
            // It went in through Codex's own writer; the rest of the user's file is as it was.
            let now = std::fs::read_to_string(main.join("config.toml")).unwrap();
            let added =
                "[mcp_servers.in-session]\ncommand = \"/usr/bin/python3\"\nargs = [\"y.py\"]\n\n";
            assert_eq!(now.replace(added, ""), main_text, "{now}");
            assert!(
                std::fs::symlink_metadata(extra.join("config.toml"))
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}
