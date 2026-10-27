//! Dependencies used only to undo the retired writer's recorded changes.
use brigadier_registry::adapter::output_within;
use brigadier_registry::library::Library;
use brigadier_registry::{Agent, Roots};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
const SECRET_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(30)
};
/// What an action works with.
pub(crate) struct Ctx<'a> {
    pub roots: &'a Roots,
    /// The environment the CLIs run with: the login shell's.
    pub env: &'a [(String, String)],
    pub library: &'a Library,
    pub secrets: &'a dyn Secrets,
    /// The user's projects (folders): where an item may be on or off by itself.
    pub projects: &'a [PathBuf],
    pub claude_configs: &'a [PathBuf],
    /// Each agent's CLI as sessions run it (the user's choice included); `None`: its name on
    /// PATH.
    pub binary: &'a (dyn Fn(Agent) -> Option<PathBuf> + Sync),
}

/// Where values (env, headers) are kept: the Keychain, through `brigadier secret`.
pub(crate) trait Secrets: Sync {
    /// Every value kept for `server`.
    fn load(&self, server: &str) -> Result<BTreeMap<String, String>, String>;
}

struct CliSecrets {
    cli: PathBuf,
    env: Vec<(String, String)>,
}

impl CliSecrets {
    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.cli);
        command
            .args(args)
            .envs(self.env.iter().map(|(key, value)| (key, value)))
            .stderr(Stdio::piped());
        command
    }
}

impl Secrets for CliSecrets {
    fn load(&self, server: &str) -> Result<BTreeMap<String, String>, String> {
        let out = output_within(
            self.command(&["secret", "headers", server])
                .stdin(Stdio::null()),
            SECRET_TIMEOUT,
        )
        .map_err(|err| format!("Couldn't run brigadier: {err}."))?;
        if !out.status.success() {
            return Err(
                "Brigadier couldn't read the server's values from the Keychain.".to_owned(),
            );
        }
        serde_json::from_slice(&out.stdout).map_err(|_| {
            "Brigadier couldn't read the server's values from the Keychain.".to_owned()
        })
    }
}

pub(crate) struct Setup {
    pub registry: Arc<brigadier_registry::Registry>,
    pub library: Library,
    pub cli: PathBuf,
    pub projects: Vec<PathBuf>,
    pub claude_configs: Vec<PathBuf>,
    /// Each agent's CLI as sessions run it.
    pub binary: Arc<dyn Fn(Agent) -> Option<PathBuf> + Send + Sync>,
}

/// Runs the one-time cleanup with its effective homes and read-only secret access.
pub(crate) fn with_ctx<T>(setup: &Setup, work: impl FnOnce(&Ctx<'_>) -> T) -> T {
    let roots = setup.registry.roots().clone();
    let env: Vec<(String, String)> = roots.vars.clone().into_iter().collect();
    let secrets = CliSecrets {
        cli: setup.cli.clone(),
        env: env.clone(),
    };
    let ctx = Ctx {
        roots: &roots,
        env: &env,
        library: &setup.library,
        secrets: &secrets,
        projects: &setup.projects,
        claude_configs: &setup.claude_configs,
        binary: &*setup.binary,
    };
    work(&ctx)
}
