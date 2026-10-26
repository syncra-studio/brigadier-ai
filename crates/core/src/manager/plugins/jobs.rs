//! Settings › Plugins' actions as jobs: each answers at once with an id and runs in the
//! background, one at a time (they edit the same configs), and the page reads how each agent's
//! part goes ([`super::super::SessionManager::plugin_job`]).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use brigadier_registry::Agent;
use brigadier_registry::adapter::{output_within, wait_within};
use brigadier_registry::library::{Library, SecretSink};

use brigadier_registry::library::Entry;

use super::ops::{self, Ctx, PluginAddCheck, PluginJobView, PluginStep, Secrets};
use super::{provider, share};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

/// Every job of this run, by id.
static JOBS: LazyLock<Mutex<HashMap<String, PluginJobView>>> = LazyLock::new(Default::default);
/// Held while a job (or a sharing pass) changes the agents' configs.
static RUNNING: Mutex<()> = Mutex::new(());
/// A sharing pass runs, and another was asked for meanwhile.
static SYNC: Mutex<SyncState> = Mutex::new(SyncState {
    running: false,
    pending: None,
});

struct SyncState {
    running: bool,
    pending: Option<(Setup, bool)>,
}
/// Whether each Library item's source has moved on, and when that was read.
static UPDATES: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> = LazyLock::new(Default::default);
const UPDATES_FRESH: Duration = Duration::from_secs(60 * 60);
/// How long one `brigadier secret` call may take.
const SECRET_TIMEOUT: Duration = if cfg!(test) {
    Duration::from_secs(1)
} else {
    Duration::from_secs(30)
};
/// When a removed original waits for its confirming pass, how long until that pass.
const CONFIRM_AFTER: Duration = if cfg!(test) {
    Duration::from_millis(20)
} else {
    Duration::from_secs(15)
};

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
        // The deadline includes writing stdin: a CLI that never reads a large value must
        // not block the queue before wait_within can start.
        let value = value.clone();
        let (send, receive) = std::sync::mpsc::channel();
        if let Some(mut stdin) = child.stdin.take() {
            std::thread::spawn(move || {
                let _ = send.send(stdin.write_all(value.expose().as_bytes()));
            });
        }
        let out = wait_within(child, SECRET_TIMEOUT).map_err(|err| err.to_string())?;
        if out.status.success() {
            receive
                .recv_timeout(SECRET_TIMEOUT)
                .map_err(|_| "Couldn't save the value in the Keychain.".to_owned())?
                .map_err(|_| "Couldn't save the value in the Keychain.".to_owned())
        } else {
            Err("Brigadier couldn't save the value in the Keychain.".to_owned())
        }
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

    fn forget(&self, server: &str) -> Result<(), String> {
        let out = output_within(
            self.command(&["secret", "remove", server])
                .stdin(Stdio::null()),
            SECRET_TIMEOUT,
        )
        .map_err(|err| format!("Couldn't run brigadier: {err}."))?;
        if out.status.success() {
            Ok(())
        } else {
            Err("Brigadier couldn't remove the server's values from the Keychain.".to_owned())
        }
    }
}

/// One action, run as a job.
pub(crate) enum Action {
    Switch {
        key: String,
        on: bool,
    },
    /// Every project (`scope` `None`), or exactly these projects.
    SetScope {
        key: String,
        scope: Option<Vec<PathBuf>>,
    },
    /// Share with every agent.
    Share {
        key: String,
    },
    Add {
        text: String,
        name: Option<String>,
        scope: Option<Vec<PathBuf>>,
        shared: bool,
        options: ops::AddOptions,
    },
    Remove {
        key: String,
    },
    Update {
        key: String,
    },
    /// Brings another app's server (`other:<id>`) to every agent.
    Import {
        key: String,
        name: Option<String>,
        scope: Option<Vec<PathBuf>>,
    },
    /// A skill file was saved: its copies follow.
    FollowSkill {
        key: String,
        path: String,
    },
}

/// What the actions run with.
#[derive(Clone)]
pub(crate) struct Setup {
    pub registry: Arc<brigadier_registry::Registry>,
    pub library: Library,
    pub cli: PathBuf,
    pub projects: Vec<PathBuf>,
    pub claude_configs: Vec<PathBuf>,
    /// Each agent's CLI as sessions run it.
    pub binary: Arc<dyn Fn(Agent) -> Option<PathBuf> + Send + Sync>,
}

/// Runs `work` with an action's context for `setup`, each agent's part told to `report`.
pub(crate) fn with_ctx<T>(
    setup: &Setup,
    report: &(dyn Fn(Agent, PluginStep) + Sync),
    work: impl FnOnce(&Ctx<'_>) -> T,
) -> T {
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
        report,
        binary: &*setup.binary,
    };
    work(&ctx)
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
        let report = |agent: Agent, step: PluginStep| {
            if let Some(view) = lock(&JOBS).get_mut(&job) {
                let agent = provider(agent);
                if let Some(entry) = view.agents.iter_mut().find(|entry| entry.agent == agent) {
                    entry.step = step;
                }
            }
        };
        let (result, warning) = with_ctx(&setup, &report, |ctx| run(ctx, &action));
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

/// Carries out `action`: the keys it made or changed, and what the user should know.
fn run(ctx: &Ctx<'_>, action: &Action) -> (Result<Vec<String>, String>, Option<String>) {
    let cli = |adapter: &dyn brigadier_registry::adapter::AgentAdapter,
               steps: &[brigadier_registry::adapter::Step]| ctx.run(adapter, steps);
    let done = |key: &str| vec![key.to_owned()];
    match action {
        Action::Switch { key, on } => {
            (share::switch(ctx, key, *on, &cli).map(|()| done(key)), None)
        }
        Action::SetScope { key, scope } => (
            share::set_scope(ctx, key, scope.as_deref(), &cli).map(|()| done(key)),
            None,
        ),
        Action::Share { key } => (share::share_again(ctx, key, &cli).map(|()| done(key)), None),
        Action::Add {
            text,
            name,
            scope,
            shared,
            options,
        } => match ops::add(
            ctx,
            text,
            name.as_deref(),
            scope.as_deref(),
            *shared,
            options,
            &cli,
        ) {
            Ok((keys, warning)) => (Ok(keys), warning),
            Err(err) => (Err(err), None),
        },
        Action::Remove { key } => (share::remove(ctx, key, &cli).map(|()| Vec::new()), None),
        Action::Update { key } => {
            let done = share::library_of(ctx, key).and_then(|name| {
                let updated = ops::update(ctx, &name);
                lock(&UPDATES).remove(&name);
                updated
            });
            (done.map(|()| vec![key.clone()]), None)
        }
        Action::Import { key, name, scope } => (
            share::import(ctx, key, name.as_deref(), scope.as_deref(), &cli).map(|key| vec![key]),
            None,
        ),
        Action::FollowSkill { key, path } => (
            share::follow_skill(ctx, key, path, &cli).map(|()| done(key)),
            None,
        ),
    }
}

/// Asks for a sharing pass over every item: now, or once the running one ends (passes asked
/// for meanwhile are one). `retry`: try again what failed with the same inputs.
pub(crate) fn request_sync(mut setup: Setup, retry: bool) {
    {
        let mut state = lock(&SYNC);
        if state.running {
            let retry = retry || state.pending.as_ref().is_some_and(|(_, retry)| *retry);
            state.pending = Some((setup, retry));
            return;
        }
        state.running = true;
    }
    std::thread::spawn(move || {
        let mut retry = retry;
        loop {
            {
                let _running = lock(&RUNNING);
                let result = with_ctx(&setup, &|_, _| {}, |ctx| {
                    share::sync(ctx, None, retry, &|adapter, steps| ctx.run(adapter, steps))
                });
                if let Err(error) = result {
                    tracing::warn!(%error, "sharing the plugins with every agent");
                }
            }
            setup.registry.refresh();
            // A removed original waits for a second clean read: that pass comes by itself, not
            // only with the next change (review #11).
            let confirming = share::confirming(&setup.library);
            if confirming && lock(&SYNC).pending.is_none() {
                std::thread::sleep(CONFIRM_AFTER);
            }
            let mut state = lock(&SYNC);
            if let Some((latest, requested_retry)) = state.pending.take() {
                setup = latest;
                retry = requested_retry;
                continue;
            }
            if confirming {
                retry = false;
                continue;
            }
            state.running = false;
            break;
        }
    });
}

/// How job `id` goes.
pub(crate) fn job(id: &str) -> Option<PluginJobView> {
    lock(&JOBS).get(id).cloned()
}

/// What a paste would add.
pub(crate) fn check_add(home: &Path, text: &str, name: Option<&str>) -> PluginAddCheck {
    ops::check_add(home, text, name)
}

/// The Library items whose source has moved on, as last read; a stale or unread one is read
/// again in the background (at most hourly).
pub(crate) fn updates_waiting(library: Library, entries: &[Entry]) -> Vec<String> {
    let library = Arc::new(library);
    let mut stale = Vec::new();
    let mut waiting = Vec::new();
    for entry in entries {
        let name = &entry.name;
        let known = lock(&UPDATES).get(name).copied();
        if known.is_some_and(|(_, waits)| waits) {
            waiting.push(name.clone());
        }
        if known.is_none_or(|(at, _)| at.elapsed() > UPDATES_FRESH)
            && library.origin(name).is_some()
        {
            stale.push(name.clone());
        }
    }
    if stale.is_empty() {
        return waiting;
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
            let waits = ops::update_waiting(&library, &name);
            lock(&UPDATES).insert(name, (Instant::now(), waits));
        }
    });
    waiting
}
