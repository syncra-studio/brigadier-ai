//! Finding the user's CLIs and the environment they run in.
//!
//! GUI apps on macOS do not inherit the login shell's PATH, so the environment comes from the
//! user's login shell. Variables that would make a CLI think it runs nested inside another agent
//! session are dropped.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        Self { vars }
    }

    /// Exactly these variables: for tests, and tools that run a CLI they set up themselves.
    pub fn from_vars(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Self {
        Self {
            vars: vars.into_iter().collect(),
        }
    }

    /// Absolute path of a CLI on the login PATH.
    pub fn resolve(&self, provider: ProviderKind) -> Option<PathBuf> {
        let path = self.vars.get(OsStr::new("PATH"))?;
        let cwd = self.home().unwrap_or_else(|| PathBuf::from("/"));
        which::which_in(provider.binary(), Some(path), cwd).ok()
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
