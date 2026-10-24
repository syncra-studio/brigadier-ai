//! Settings › Plugins' actions as jobs: each answers at once with an id and runs in the
//! background, one at a time (they edit the same configs), and the page reads how each agent's
//! part goes ([`super::super::SessionManager::plugin_job`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use brigadier_registry::Agent;
use brigadier_registry::library::{Library, SecretSink};

use super::ops::{self, Ctx, PluginAddCheck, PluginJobView, PluginStep, Secrets};
use super::{PluginsView, provider};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// Every job of this run, by id.
static JOBS: LazyLock<Mutex<HashMap<String, PluginJobView>>> = LazyLock::new(Default::default);
/// Held while a job changes the agents' configs.
static RUNNING: Mutex<()> = Mutex::new(());
/// Whether each Library item's source has moved on, and when that was read.
static UPDATES: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> = LazyLock::new(Default::default);
const UPDATES_FRESH: Duration = Duration::from_secs(60 * 60);

/// The values of `brigadier secret`, kept in the Keychain by the `brigadier` CLI.
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

impl SecretSink for CliSecrets {
    fn store(
        &self,
        server: &str,
        name: &str,
        value: &brigadier_registry::Secret,
    ) -> Result<(), String> {
        use std::io::Write;
        let mut child = self
            .command(&["secret", "set", server, name])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|err| format!("Couldn't run brigadier: {err}."))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(value.expose().as_bytes())
                .map_err(|err| err.to_string())?;
        }
        let out = child.wait_with_output().map_err(|err| err.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }
}

impl Secrets for CliSecrets {
    fn load(&self, server: &str) -> Result<BTreeMap<String, String>, String> {
        let out = self
            .command(&["secret", "headers", server])
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("Couldn't run brigadier: {err}."))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_owned());
        }
        serde_json::from_slice(&out.stdout).map_err(|err| err.to_string())
    }

    fn forget(&self, server: &str) -> Result<(), String> {
        let out = self
            .command(&["secret", "remove", server])
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("Couldn't run brigadier: {err}."))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
        }
    }
}

/// One action, run as a job.
pub(crate) enum Action {
    Switch {
        key: String,
        project: Option<PathBuf>,
        on: bool,
        agent: Option<Agent>,
    },
    Add {
        text: String,
        name: Option<String>,
        project: Option<PathBuf>,
        shared: bool,
    },
    UseEverywhere {
        key: String,
    },
    Undo {
        key: String,
    },
    Remove {
        key: String,
    },
    Update {
        key: String,
    },
}

/// What the actions run with.
pub(crate) struct Setup {
    pub registry: Arc<brigadier_registry::Registry>,
    pub library: Library,
    pub cli: PathBuf,
    pub projects: Vec<PathBuf>,
}

/// Starts `action`; its job's id.
pub(crate) fn start(setup: Setup, action: Action) -> String {
    let id = format!("plugin-job-{}", uuid::Uuid::new_v4().simple());
    lock(&JOBS).insert(
        id.clone(),
        PluginJobView {
            id: id.clone(),
            agents: ops::waiting(),
            finished: false,
            error: None,
            warning: None,
            keys: Vec::new(),
        },
    );
    let job = id.clone();
    std::thread::spawn(move || {
        let _running = lock(&RUNNING);
        let roots = setup.registry.roots().clone();
        let env: Vec<(String, String)> = roots.vars.clone().into_iter().collect();
        let secrets = CliSecrets {
            cli: setup.cli.clone(),
            env: env.clone(),
        };
        let report = |agent: Agent, step: PluginStep| {
            if let Some(view) = lock(&JOBS).get_mut(&job) {
                let agent = provider(agent);
                if let Some(entry) = view.agents.iter_mut().find(|entry| entry.agent == agent) {
                    entry.step = step;
                }
            }
        };
        let ctx = Ctx {
            roots: &roots,
            env: &env,
            library: &setup.library,
            secrets: &secrets,
            projects: &setup.projects,
            report: &report,
        };
        let (result, warning): (Result<Vec<String>, String>, Option<String>) = match &action {
            Action::Switch {
                key,
                project,
                on,
                agent,
            } => (
                ops::switch(&ctx, key, project.as_deref(), *on, *agent).map(|_| vec![key.clone()]),
                None,
            ),
            Action::Add {
                text,
                name,
                project,
                shared,
            } => match ops::add(&ctx, text, name.as_deref(), project.as_deref(), *shared) {
                Ok((keys, warning)) => (Ok(keys), warning),
                Err(err) => (Err(err), None),
            },
            Action::UseEverywhere { key } => {
                (ops::use_everywhere(&ctx, key).map(|key| vec![key]), None)
            }
            Action::Undo { key } => (
                ops::undo_use_everywhere(&ctx, key).map(|()| Vec::new()),
                None,
            ),
            Action::Remove { key } => (ops::remove(&ctx, key).map(|()| Vec::new()), None),
            Action::Update { key } => {
                let done = ops::update(&ctx, key).map(|()| vec![key.clone()]);
                if let Some(name) = key.strip_prefix("lib:") {
                    lock(&UPDATES).remove(name);
                }
                (done, None)
            }
        };
        setup.registry.refresh();
        if let Some(view) = lock(&JOBS).get_mut(&job) {
            view.finished = true;
            view.warning = warning;
            match result {
                Ok(keys) => view.keys = keys,
                Err(err) => view.error = Some(err),
            }
            // An agent's part that never started didn't happen.
            for entry in &mut view.agents {
                if matches!(entry.step, PluginStep::Waiting | PluginStep::Working) {
                    entry.step = PluginStep::Skipped {
                        reason: String::new(),
                    };
                }
            }
        }
    });
    id
}

/// How job `id` goes.
pub(crate) fn job(id: &str) -> Option<PluginJobView> {
    lock(&JOBS).get(id).cloned()
}

/// What a paste would add.
pub(crate) fn check_add(home: &Path, text: &str, name: Option<&str>) -> PluginAddCheck {
    ops::check_add(home, text, name)
}

/// Marks each Library item of `view` that Use everywhere can undo, and whose source has moved
/// on (read in the background, at most hourly).
pub(crate) fn mark_library(view: &mut PluginsView, library: Library) {
    let library = Arc::new(library);
    let mut stale = Vec::new();
    for item in &mut view.items {
        let Some(name) = item.key.strip_prefix("lib:") else {
            continue;
        };
        item.can_undo = ops::can_undo(&library, name);
        let known = lock(&UPDATES).get(name).copied();
        item.update_available = known.is_some_and(|(_, waiting)| waiting);
        if known.is_none_or(|(at, _)| at.elapsed() > UPDATES_FRESH)
            && library.origin(name).is_some()
        {
            stale.push(name.to_owned());
        }
    }
    if stale.is_empty() {
        return;
    }
    for name in &stale {
        // Not read again while this read runs.
        lock(&UPDATES)
            .entry(name.clone())
            .and_modify(|(at, _)| *at = Instant::now())
            .or_insert((Instant::now(), false));
    }
    std::thread::spawn(move || {
        for name in stale {
            let waiting = ops::update_waiting(&library, &name);
            lock(&UPDATES).insert(name, (Instant::now(), waiting));
        }
    });
}
