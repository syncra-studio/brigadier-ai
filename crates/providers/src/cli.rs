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
        Self { vars }
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
