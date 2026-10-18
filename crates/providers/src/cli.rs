//! Finding the user's CLIs and the environment they run in.
//!
//! GUI apps on macOS do not inherit the login shell's PATH, so the environment comes from the
//! user's login shell. Variables that would make a CLI think it runs nested inside another agent
//! session are dropped.

use std::collections::{BTreeMap, HashMap};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use brigadier_sandbox::{Platform, SpawnSpec};

use crate::model::ProviderKind;

/// Set by an agent CLI for the processes it starts; a CLI that sees them changes behavior.
const NESTED_SESSION_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_AGENT_SDK_VERSION",
    // An interactive Claude that sees it answers but saves nothing of its session (checked on
    // 2.1.292, docs/evidence/2026-10-07-thread-open-in-terminal-spike.md).
    "CLAUDE_CODE_CHILD_SESSION",
    // Another session's own.
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_PID",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CODEX_THREAD_ID",
    "CODEX_CI",
];

/// The environment every CLI is started with.
#[derive(Debug, Clone)]
pub struct CliEnv {
    vars: BTreeMap<OsString, OsString>,
    /// The binary the user chose for an agent instead of the one on PATH. Shared by every copy
    /// of this environment and each extra account's, so a choice reaches them all at once.
    chosen: Arc<RwLock<HashMap<ProviderKind, PathBuf>>>,
}

impl CliEnv {
    /// Captures the login shell's environment. This spawns the shell, so call it off the async
    /// runtime. Falls back to this process's environment when the shell cannot be run.
    pub fn capture(platform: &Arc<dyn Platform>) -> Self {
        let mut vars = match platform.shell().login_environment() {
            Ok(vars) => vars,
            Err(err) => {
                tracing::warn!(error = %err, "login environment unavailable; using the daemon's");
                std::env::vars_os().collect()
            }
        };
        for name in NESTED_SESSION_VARS {
            vars.remove(OsStr::new(name));
        }
        add_toolchain_bins(&mut vars);
        Self {
            vars,
            chosen: Arc::default(),
        }
    }

    /// Exactly these variables: for tests, and tools that run a CLI they set up themselves.
    pub fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self {
            vars: vars.into_iter().collect(),
            chosen: Arc::default(),
        }
    }

    /// These variables, sharing this environment's chosen binaries (an extra account's).
    pub(crate) fn with_vars(&self, vars: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self {
            vars: vars.into_iter().collect(),
            chosen: self.chosen.clone(),
        }
    }

    /// Uses `path` for the agent instead of the one on PATH; `None` goes back to PATH.
    pub fn choose(&self, provider: ProviderKind, path: Option<PathBuf>) {
        let mut chosen = self.chosen.write().unwrap_or_else(|p| p.into_inner());
        match path {
            Some(path) => chosen.insert(provider, path),
            None => chosen.remove(&provider),
        };
    }

    /// Absolute path of a CLI: the one the user chose while it is there, else the one on the
    /// login PATH.
    pub fn resolve(&self, provider: ProviderKind) -> Option<PathBuf> {
        let chosen = self
            .chosen
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .get(&provider)
            .cloned();
        if let Some(path) = chosen.filter(|path| path.is_file()) {
            return Some(path);
        }
        let cwd = self.home().unwrap_or_else(|| PathBuf::from("/"));
        let on_path = self
            .vars
            .get(OsStr::new("PATH"))
            .and_then(|path| which::which_in(provider.binary(), Some(path), &cwd).ok());
        on_path.or_else(|| {
            let folder = self.installer_folder(provider)?;
            which::which_in(provider.binary(), Some(folder), &cwd).ok()
        })
    }

    /// Where the agent's own native installer puts its binary: `~/.local/bin` for both (Codex's
    /// moves with `CODEX_INSTALL_DIR`). The login PATH may not hold it: Claude Code's installer
    /// leaves the shell profiles alone, and Codex's adds it to a profile read after this
    /// environment was captured. So a binary there counts as installed, PATH first.
    pub fn installer_folder(&self, provider: ProviderKind) -> Option<PathBuf> {
        if provider == ProviderKind::Codex
            && let Some(dir) = self.var("CODEX_INSTALL_DIR").filter(|dir| !dir.is_empty())
        {
            return Some(PathBuf::from(dir));
        }
        // Only this environment's own home (USERPROFILE on Windows): one without it (a test's)
        // looks nowhere else.
        let home = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
        self.var(home)
            .filter(|home| !home.is_empty())
            .map(|home| Path::new(home).join(".local").join("bin"))
    }

    /// Absolute path of any program on the login PATH.
    pub fn which(&self, program: &str) -> Option<PathBuf> {
        let path = self.vars.get(OsStr::new("PATH"))?;
        let cwd = self.home().unwrap_or_else(|| PathBuf::from("/"));
        which::which_in(program, Some(path), cwd).ok()
    }

    /// Every variable, for running other tools (git) with exactly this environment.
    pub fn vars(&self) -> Vec<(OsString, OsString)> {
        self.vars
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    pub fn home(&self) -> Option<PathBuf> {
        self.vars
            .get(OsStr::new("HOME"))
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
    }

    pub fn var(&self, name: &str) -> Option<&OsStr> {
        self.vars.get(OsStr::new(name)).map(OsString::as_os_str)
    }

    /// A spawn spec for `program` with exactly this environment.
    pub fn spec(&self, program: &Path) -> SpawnSpec {
        SpawnSpec {
            program: program.to_owned(),
            env: self
                .vars
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            clear_env: true,
            ..SpawnSpec::default()
        }
    }
}

/// Toolchains that install their programs in a folder of the home directory and add it to
/// PATH from a shell profile, which an install may have left out: the folder, and the variable
/// that moves it (the programs are then in its `bin`).
const TOOLCHAIN_BINS: &[(&str, &str)] = &[(".cargo/bin", "CARGO_HOME")];

/// Puts each toolchain folder that exists but isn't on PATH at its end, so a worker's `cargo`
/// works without hunting for it. What PATH already holds keeps its order and wins.
fn add_toolchain_bins(vars: &mut BTreeMap<OsString, OsString>) {
    let home = vars
        .get(OsStr::new("HOME"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir);
    let mut paths: Vec<PathBuf> = vars
        .get(OsStr::new("PATH"))
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let mut added = false;
    for (folder, moved_by) in TOOLCHAIN_BINS {
        let dir = match vars
            .get(OsStr::new(moved_by))
            .filter(|value| !value.is_empty())
        {
            Some(root) => PathBuf::from(root).join("bin"),
            None => match &home {
                Some(home) => home.join(folder),
                None => continue,
            },
        };
        if dir.is_dir() && !paths.contains(&dir) {
            paths.push(dir);
            added = true;
        }
    }
    if added && let Ok(path) = std::env::join_paths(paths) {
        vars.insert(OsString::from("PATH"), path);
    }
}

/// Puts a Full-access session's CLI, and everything it starts, inside Brigadier's
/// allow-all-but profile of [`SessionSpec::protection`](crate::SessionSpec::protection), with
/// the CLI's own homes (`homes`: its configuration, history) and its own temp folder
/// (`CLAUDE_CODE_TMPDIR`, kept in place) given back. Any other session, or an empty
/// protection, runs as it is.
pub fn protect(
    platform: &dyn Platform,
    process: SpawnSpec,
    session: &crate::SessionSpec,
    homes: &[PathBuf],
) -> crate::Result<SpawnSpec> {
    if session.access != crate::Access::Full || session.protection.is_empty() {
        return Ok(process);
    }
    let mut protection = session.protection.clone();
    protection.allow.extend(homes.iter().cloned());
    if let Some((_, temp)) = process
        .env
        .iter()
        .find(|(name, _)| name == OsStr::new("CLAUDE_CODE_TMPDIR"))
    {
        protection.allow.push(PathBuf::from(temp));
        protection.keep.push(PathBuf::from(temp));
    }
    platform
        .sandbox()
        .protect(process, &protection)
        .map_err(|err| crate::Error::Io(std::io::Error::other(err.to_string())))
}

/// Adds a session's extra environment to a spawn spec.
pub fn apply_session_env(spec: &mut SpawnSpec, env: &[(String, String)], unset: &[String]) {
    spec.env
        .retain(|(key, _)| !unset.iter().any(|name| key == OsStr::new(name)));
    for (name, value) in env {
        spec.env.retain(|(key, _)| key != OsStr::new(name));
        spec.env.push((name.into(), value.into()));
    }
}

/// Whether `version` (dotted numbers, as [`parse_version`] gives) is `minimum` or newer. A
/// version that does not read as numbers is not.
pub fn version_at_least(version: &str, minimum: &str) -> bool {
    let parts = |text: &str| -> Option<Vec<u64>> {
        text.split(['-', '+'])
            .next()?
            .split('.')
            .map(|part| part.parse().ok())
            .collect()
    };
    match (parts(version), parts(minimum)) {
        (Some(version), Some(minimum)) => version >= minimum,
        _ => false,
    }
}

/// The first line of a CLI's `--version` output, trimmed to the version number
/// (`2.1.281 (Claude Code)` → `2.1.281`, `codex-cli 0.156.1` → `0.156.1`).
pub fn parse_version(output: &str) -> Option<String> {
    output
        .lines()
        .next()?
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}

/// How long a picked binary's `--version` may take.
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);

/// Checks that `path` is `provider`'s own CLI before the user's choice of it is kept: a file
/// that runs and names itself in its `--version` (`2.1.302 (Claude Code)`, `codex-cli 0.163.0`).
/// Answers its version, or what is wrong in plain words.
pub async fn check_binary(
    platform: &Arc<dyn Platform>,
    env: &CliEnv,
    provider: ProviderKind,
    path: &Path,
) -> Result<String, String> {
    let label = provider.label();
    if !path.is_absolute() {
        return Err("Choose the file by its full path.".into());
    }
    let metadata =
        std::fs::metadata(path).map_err(|_| format!("{} doesn't exist.", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is a folder, not a program.", path.display()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(format!("{} isn't a program that can run.", path.display()));
        }
    }
    let output = crate::process::run(platform, &env.spec(path).arg("--version"), CHECK_TIMEOUT)
        .await
        .map_err(|err| format!("{} didn't run: {err}", path.display()))?;
    let first = output.stdout.lines().next().unwrap_or_default();
    let named = match provider {
        ProviderKind::Claude => first.contains("Claude Code"),
        ProviderKind::Codex => first
            .split_whitespace()
            .next()
            .is_some_and(|word| word.starts_with("codex")),
    };
    match parse_version(&output.stdout) {
        Some(version) if named && output.code == Some(0) => Ok(version),
        _ => Err(format!("{} isn't {label}.", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn vars(pairs: &[(&str, &OsStr)]) -> BTreeMap<OsString, OsString> {
        pairs
            .iter()
            .map(|(key, value)| (OsString::from(key), value.to_os_string()))
            .collect()
    }

    /// Writes an executable script at `path` that prints `answer` for `--version`.
    #[cfg(unix)]
    fn script(path: &Path, answer: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, format!("#!/bin/sh\necho '{answer}'\n")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_chosen_binary_wins_over_path_while_it_is_there_and_reaches_extra_accounts() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-cli-chosen-{}", std::process::id())));
        let (on_path, other) = (dir.join("bin"), dir.join("elsewhere"));
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        script(&on_path.join("claude"), "2.1.0 (Claude Code)");
        let chosen = other.join("claude-nightly");
        script(&chosen, "2.2.0 (Claude Code)");
        let env = CliEnv::from_vars(vars(&[
            ("HOME", dir.as_os_str()),
            ("PATH", on_path.as_os_str()),
        ]));
        assert_eq!(
            env.resolve(ProviderKind::Claude),
            Some(on_path.join("claude"))
        );

        env.choose(ProviderKind::Claude, Some(chosen.clone()));
        assert_eq!(env.resolve(ProviderKind::Claude), Some(chosen.clone()));
        // Copies and an extra account's environment see the same choice, made before or after.
        let account = env.for_account(ProviderKind::Claude, &dir.join("account"));
        assert_eq!(
            env.clone().resolve(ProviderKind::Claude),
            Some(chosen.clone())
        );
        assert_eq!(account.resolve(ProviderKind::Claude), Some(chosen.clone()));
        // The other agent keeps its own lookup.
        assert_eq!(env.resolve(ProviderKind::Codex), None);

        // Gone from disk: PATH's again, until it is back.
        std::fs::remove_file(&chosen).unwrap();
        assert_eq!(
            account.resolve(ProviderKind::Claude),
            Some(on_path.join("claude"))
        );
        script(&chosen, "2.2.0 (Claude Code)");
        assert_eq!(account.resolve(ProviderKind::Claude), Some(chosen));

        env.choose(ProviderKind::Claude, None);
        assert_eq!(
            account.resolve(ProviderKind::Claude),
            Some(on_path.join("claude"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_agent_its_installer_put_off_the_login_path_is_found_there() {
        let dir = Temp(
            std::env::temp_dir().join(format!("brigadier-cli-installed-{}", std::process::id())),
        );
        let (on_path, installed) = (dir.join("bin"), dir.join(".local").join("bin"));
        std::fs::create_dir_all(&on_path).unwrap();
        std::fs::create_dir_all(&installed).unwrap();
        let env = CliEnv::from_vars(vars(&[
            ("HOME", dir.as_os_str()),
            ("PATH", on_path.as_os_str()),
        ]));
        // Nowhere yet.
        assert_eq!(env.resolve(ProviderKind::Claude), None);
        assert_eq!(env.resolve(ProviderKind::Codex), None);

        // The native installers' folder, which the login PATH doesn't hold.
        script(&installed.join("claude"), "2.1.302 (Claude Code)");
        script(&installed.join("codex"), "codex-cli 0.163.0");
        assert_eq!(
            env.resolve(ProviderKind::Claude),
            Some(installed.join("claude"))
        );
        assert_eq!(
            env.resolve(ProviderKind::Codex),
            Some(installed.join("codex"))
        );

        // One on PATH comes first.
        script(&on_path.join("claude"), "2.1.0 (Claude Code)");
        assert_eq!(
            env.resolve(ProviderKind::Claude),
            Some(on_path.join("claude"))
        );

        // Codex's installer folder moves with CODEX_INSTALL_DIR.
        let moved = dir.join("codex-bin");
        std::fs::create_dir_all(&moved).unwrap();
        let env = CliEnv::from_vars(vars(&[
            ("HOME", dir.as_os_str()),
            ("PATH", on_path.as_os_str()),
            ("CODEX_INSTALL_DIR", moved.as_os_str()),
        ]));
        assert_eq!(env.resolve(ProviderKind::Codex), None);
        script(&moved.join("codex"), "codex-cli 0.163.0");
        assert_eq!(env.resolve(ProviderKind::Codex), Some(moved.join("codex")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_picked_binary_is_kept_only_when_it_runs_and_names_its_agent() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-cli-check-{}", std::process::id())));
        std::fs::create_dir_all(&*dir).unwrap();
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(dir.join("data")),
        })
        .unwrap();
        let env = CliEnv::from_vars(vars(&[
            ("HOME", dir.as_os_str()),
            ("PATH", OsStr::new("/usr/bin:/bin")),
        ]));
        let check = |provider, path: PathBuf| {
            let (platform, env) = (platform.clone(), env.clone());
            async move { check_binary(&platform, &env, provider, &path).await }
        };
        let claude = dir.join("claude");
        script(&claude, "2.1.302 (Claude Code)");
        let codex = dir.join("codex");
        script(&codex, "codex-cli 0.163.0");
        assert_eq!(
            check(ProviderKind::Claude, claude.clone()).await,
            Ok("2.1.302".into())
        );
        assert_eq!(
            check(ProviderKind::Codex, codex.clone()).await,
            Ok("0.163.0".into())
        );
        // Another agent's binary, or any other program, isn't taken.
        assert_eq!(
            check(ProviderKind::Codex, claude.clone()).await,
            Err(format!("{} isn't Codex.", claude.display()))
        );
        assert_eq!(
            check(ProviderKind::Claude, PathBuf::from("/bin/ls")).await,
            Err("/bin/ls isn't Claude Code.".into())
        );
        let plain = dir.join("notes.txt");
        std::fs::write(&plain, "hello").unwrap();
        assert_eq!(
            check(ProviderKind::Claude, plain.clone()).await,
            Err(format!("{} isn't a program that can run.", plain.display()))
        );
        assert_eq!(
            check(ProviderKind::Claude, dir.join("missing")).await,
            Err(format!("{} doesn't exist.", dir.join("missing").display()))
        );
        assert_eq!(
            check(ProviderKind::Claude, dir.to_path_buf()).await,
            Err(format!("{} is a folder, not a program.", dir.display()))
        );
        assert_eq!(
            check(ProviderKind::Claude, PathBuf::from("claude")).await,
            Err("Choose the file by its full path.".into())
        );
    }

    #[test]
    fn a_toolchain_the_login_path_misses_is_added_at_the_end_once() {
        let home =
            Temp(std::env::temp_dir().join(format!("brigadier-cli-env-{}", std::process::id())));
        let bin = home.join(".cargo/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut env = vars(&[
            ("HOME", home.as_os_str()),
            ("PATH", OsStr::new("/usr/bin:/bin")),
        ]);
        add_toolchain_bins(&mut env);
        let expected = format!("/usr/bin:/bin:{}", bin.display());
        assert_eq!(env[OsStr::new("PATH")], OsString::from(&expected));
        // Already there: nothing changes.
        add_toolchain_bins(&mut env);
        assert_eq!(env[OsStr::new("PATH")], OsString::from(&expected));
        // `CARGO_HOME` moves it; one that doesn't exist adds nothing.
        let mut moved = vars(&[
            ("HOME", home.as_os_str()),
            ("CARGO_HOME", home.join("nowhere").as_os_str()),
            ("PATH", OsStr::new("/usr/bin")),
        ]);
        add_toolchain_bins(&mut moved);
        assert_eq!(moved[OsStr::new("PATH")], OsString::from("/usr/bin"));
    }
}
