//! Signing in to a remote MCP server with each agent CLI's own `mcp login`, every agent at
//! once (STRIP-DOWN-PLAN §9.4 X5): the CLI keeps the sign-in in its own store, so a plain
//! `claude` or `codex` in a terminal is signed in too. Brigadier keeps no token, so the agents
//! can't share one sign-in: each opens its own page.
//!
//! - Which agents need it, and under which name, comes from each CLI's own list (Claude names
//!   a plugin's server `plugin:<plugin>:<server>`): the one Settings showed, when it has it, as
//!   Claude's takes as long as its slowest server (30 s for one that hangs).
//! - Each login prints its sign-in page's address (`--no-browser`); Brigadier opens it in the
//!   browser and waits for the CLI to finish. One that needs a terminal runs in a PTY.
//! - A sign-in can be cancelled (its CLIs are stopped) and its pages opened again.
//!
//! The lists are also what Settings shows: which agent still needs a sign-in, read again at most
//! every [`LIST_FRESH`]. A CLI's own word that it signed in is taken at once, not read again.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use brigadier_providers::ProviderKind;
use brigadier_registry::Agent;
use brigadier_registry::adapter::{AgentAdapter, CliCall, Listed, adapters, plain_text};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How long a CLI's list is taken as it was.
pub(crate) const LIST_FRESH: Duration = Duration::from_secs(60);
/// The longest a list may take (Claude checks every server's health).
const LIST_TIMEOUT: Duration = Duration::from_secs(90);
/// The longest one agent's sign-in may wait for the user.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// How often a running CLI is asked whether it ended.
const POLL: Duration = Duration::from_millis(20);

/// Opens a link in the user's browser.
pub(crate) type Open = Arc<dyn Fn(&str) + Send + Sync>;

/// How a sign-in went, for every agent at once. It never carries a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum McpSignInStatus {
    /// Still going: an agent is waiting for the user in their browser, or starting.
    Pending,
    /// Every agent that needed it is signed in.
    SignedIn,
    /// The user cancelled it.
    Cancelled,
    /// Why, in a short plain sentence (the first agent's that failed).
    Failed { reason: String },
}

/// How a sign-in goes, agent by agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct McpSignInProgress {
    /// Each agent that lists the server, in order; empty while their lists are read.
    pub agents: Vec<AgentSignIn>,
    /// Nothing more will change.
    pub finished: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AgentSignIn {
    pub agent: ProviderKind,
    /// The server's name in that agent's own list.
    pub server: String,
    pub state: AgentSignInState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AgentSignInState {
    /// It had a sign-in already.
    AlreadySignedIn,
    /// Its sign-in starts next.
    Waiting,
    /// Its CLI is starting the sign-in.
    Starting,
    /// Its sign-in page is open in the browser.
    InBrowser,
    SignedIn,
    Cancelled,
    Failed {
        reason: String,
    },
}

impl McpSignInProgress {
    pub fn status(&self) -> McpSignInStatus {
        if !self.finished {
            return McpSignInStatus::Pending;
        }
        let states = || self.agents.iter().map(|agent| &agent.state);
        if let Some(reason) = states().find_map(|state| match state {
            AgentSignInState::Failed { reason } => Some(reason.clone()),
            _ => None,
        }) {
            return McpSignInStatus::Failed { reason };
        }
        if states().any(|state| *state == AgentSignInState::Cancelled) {
            return McpSignInStatus::Cancelled;
        }
        McpSignInStatus::SignedIn
    }
}

pub(crate) fn provider(agent: Agent) -> ProviderKind {
    match agent {
        Agent::Claude => ProviderKind::Claude,
        Agent::Codex => ProviderKind::Codex,
    }
}

/// How the CLIs are run: the environment they get (the login shell's, with each adapter's
/// home), and where each one's binary is.
#[derive(Clone)]
pub(crate) struct Clis {
    pub env: Vec<(String, String)>,
    pub binary: Arc<dyn Fn(Agent) -> Option<PathBuf> + Send + Sync>,
}

impl Clis {
    fn command(
        &self,
        adapter: &dyn AgentAdapter,
    ) -> Result<(PathBuf, Vec<(String, String)>), String> {
        let binary = (self.binary)(adapter.agent())
            .ok_or_else(|| format!("{} isn't installed.", adapter.label()))?;
        Ok((binary, self.env.clone()))
    }
}

/// The server a sign-in is for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Target {
    /// Its name, as configured.
    pub server: String,
    /// The plugin it comes with (its name), if any.
    pub plugin: Option<String>,
    /// The project whose servers the CLIs list (they run in its folder).
    pub project: Option<PathBuf>,
}

struct Run {
    target: Target,
    progress: McpSignInProgress,
    /// The pages waiting in the browser, each agent's, to open again.
    links: Vec<(ProviderKind, String)>,
    open: Open,
    cancel: Arc<AtomicBool>,
}

impl Run {
    fn open_again(&self) {
        for (_, link) in &self.links {
            (self.open)(link);
        }
    }
}

type ListKey = (Agent, Option<PathBuf>);

#[derive(Default)]
pub(crate) struct McpLogins {
    runs: Mutex<HashMap<String, Run>>,
    /// Told whenever a sign-in moves on.
    changed: Condvar,
    /// Each CLI's last list, by project.
    lists: Mutex<HashMap<ListKey, (Instant, Vec<Listed>)>>,
    /// When each CLI said it signed in to a server, by the name it lists: a list read from
    /// before then doesn't undo it. Taken with `lists` held.
    signed_in: Mutex<HashMap<(Agent, String), Instant>>,
    /// Lists being read in the background.
    reading: Mutex<HashSet<ListKey>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|p| p.into_inner())
}

impl McpLogins {
    /// Starts signing every agent that needs it in to `target`, all at once, in the background
    /// (blocking threads): its id at once. One already going for the same server is kept: its
    /// pages open again and its id comes back.
    pub(crate) fn start(self: &Arc<Self>, clis: Clis, target: Target, open: Open) -> String {
        {
            let runs = lock(&self.runs);
            if let Some((id, run)) = runs
                .iter()
                .find(|(_, run)| run.target == target && !run.progress.finished)
            {
                run.open_again();
                return id.clone();
            }
        }
        let id = format!("mcp-sign-in-{}", uuid::Uuid::new_v4().simple());
        let cancel = Arc::new(AtomicBool::new(false));
        lock(&self.runs).insert(
            id.clone(),
            Run {
                target: target.clone(),
                progress: McpSignInProgress {
                    agents: Vec::new(),
                    finished: false,
                },
                links: Vec::new(),
                open,
                cancel: cancel.clone(),
            },
        );
        let (me, run_id) = (self.clone(), id.clone());
        std::thread::spawn(move || me.sign_in(&run_id, &clis, &target, &cancel));
        id
    }

    pub(crate) fn progress(&self, id: &str) -> Option<McpSignInProgress> {
        lock(&self.runs).get(id).map(|run| run.progress.clone())
    }

    /// Sign-in `id` once it finished, the moment it does, or as it is after `timeout`.
    pub(crate) fn finished(&self, id: &str, timeout: Duration) -> Option<McpSignInProgress> {
        let deadline = Instant::now() + timeout;
        let mut runs = lock(&self.runs);
        loop {
            let progress = runs.get(id)?.progress.clone();
            let left = deadline.saturating_duration_since(Instant::now());
            if progress.finished || left.is_zero() {
                return Some(progress);
            }
            runs = match self.changed.wait_timeout(runs, left) {
                Ok((runs, _)) => runs,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }

    /// The sign-in going for `target`, if one is.
    #[cfg(all(test, unix))]
    pub(crate) fn pending(&self, target: &Target) -> Option<String> {
        lock(&self.runs)
            .iter()
            .find(|(_, run)| run.target == *target && !run.progress.finished)
            .map(|(id, _)| id.clone())
    }

    /// Stops sign-in `id`: the agents signing in are stopped and the ones not started skipped.
    pub(crate) fn cancel(&self, id: &str) -> Result<(), String> {
        let runs = lock(&self.runs);
        let run = runs.get(id).ok_or_else(|| format!("no sign-in `{id}`"))?;
        run.cancel.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Opens sign-in `id`'s waiting pages again.
    pub(crate) fn reopen(&self, id: &str) -> Result<(), String> {
        let runs = lock(&self.runs);
        let run = runs.get(id).ok_or_else(|| format!("no sign-in `{id}`"))?;
        if run.links.is_empty() || run.progress.finished {
            return Err("No sign-in page is waiting.".to_owned());
        }
        run.open_again();
        Ok(())
    }

    fn update(&self, id: &str, change: impl FnOnce(&mut Run)) {
        if let Some(run) = lock(&self.runs).get_mut(id) {
            change(run);
        }
        self.changed.notify_all();
    }

    fn set_state(&self, id: &str, agent: ProviderKind, state: AgentSignInState) {
        self.update(id, |run| {
            if let Some(step) = run.progress.agents.iter_mut().find(|a| a.agent == agent) {
                step.state = state;
            }
        });
    }

    /// The whole sign-in, blocking: every agent's at once.
    fn sign_in(&self, id: &str, clis: &Clis, target: &Target, cancel: &AtomicBool) {
        std::thread::scope(|scope| {
            for adapter in adapters() {
                scope.spawn(move || self.sign_agent_in(id, clis, target, cancel, adapter));
            }
        });
        self.update(id, |run| run.progress.finished = true);
    }

    /// One agent's part of sign-in `id`: none when its CLI isn't installed (Settings shows no
    /// sign-in of it) or its list hasn't the server.
    fn sign_agent_in(
        &self,
        id: &str,
        clis: &Clis,
        target: &Target,
        cancel: &AtomicBool,
        adapter: &dyn AgentAdapter,
    ) {
        if (clis.binary)(adapter.agent()).is_none() {
            return;
        }
        let agent = provider(adapter.agent());
        let step = match self.entry(clis, adapter, target) {
            Ok(None) => return,
            Ok(Some(entry)) => AgentSignIn {
                agent,
                server: entry.name,
                state: if entry.needs_sign_in {
                    AgentSignInState::Waiting
                } else {
                    AgentSignInState::AlreadySignedIn
                },
            },
            Err(reason) => AgentSignIn {
                agent,
                server: target.server.clone(),
                state: AgentSignInState::Failed { reason },
            },
        };
        let waiting = step.state == AgentSignInState::Waiting;
        let server = step.server.clone();
        // In the agents' own order, whichever list came in first.
        let rank = |agent: ProviderKind| {
            adapters()
                .iter()
                .position(|adapter| provider(adapter.agent()) == agent)
        };
        self.update(id, |run| {
            let agents = &mut run.progress.agents;
            let at = agents
                .iter()
                .position(|other| rank(other.agent) > rank(agent))
                .unwrap_or(agents.len());
            agents.insert(at, step);
        });
        if !waiting {
            return;
        }
        if cancel.load(Ordering::SeqCst) {
            self.set_state(id, agent, AgentSignInState::Cancelled);
            return;
        }
        self.set_state(id, agent, AgentSignInState::Starting);
        let call = adapter.login(&server, target.project.as_deref());
        let opened = |link: &str| {
            let mut open = None;
            self.update(id, |run| {
                run.links.push((agent, link.to_owned()));
                if let Some(step) = run.progress.agents.iter_mut().find(|a| a.agent == agent) {
                    step.state = AgentSignInState::InBrowser;
                }
                open = Some(run.open.clone());
            });
            if let Some(open) = open {
                open(link);
            }
        };
        let state = match clis
            .command(adapter)
            .and_then(|(binary, env)| login(&binary, &call, &env, &opened, cancel))
        {
            Ok(()) => {
                self.signed(adapter.agent(), &server);
                AgentSignInState::SignedIn
            }
            Err(_) if cancel.load(Ordering::SeqCst) => AgentSignInState::Cancelled,
            Err(reason) => AgentSignInState::Failed { reason },
        };
        self.update(id, |run| run.links.retain(|(other, _)| *other != agent));
        self.set_state(id, agent, state);
    }

    /// How `adapter`'s CLI lists `target`, if it does: from the list Settings showed while it
    /// is fresh, or however old when it asks for a sign-in (one more page at worst), else read
    /// now.
    fn entry(
        &self,
        clis: &Clis,
        adapter: &dyn AgentAdapter,
        target: &Target,
    ) -> Result<Option<Listed>, String> {
        let project = target.project.as_deref();
        let find = |listed: &[Listed]| {
            listed
                .iter()
                .find(|entry| {
                    adapter.is_server(&entry.name, &target.server, target.plugin.as_deref())
                })
                .cloned()
        };
        let key = (adapter.agent(), project.map(Path::to_owned));
        if let Some((at, listed)) = lock(&self.lists).get(&key) {
            let entry = find(listed);
            if at.elapsed() <= LIST_FRESH || entry.as_ref().is_some_and(|e| e.needs_sign_in) {
                return Ok(entry);
            }
        }
        Ok(find(&self.list(clis, adapter, project)?))
    }

    /// `agent`'s CLI said it signed in to the server it lists as `name`: every list of it
    /// says so from now on.
    fn signed(&self, agent: Agent, name: &str) {
        let mut lists = lock(&self.lists);
        lock(&self.signed_in).insert((agent, name.to_owned()), Instant::now());
        for (_, listed) in lists
            .iter_mut()
            .filter_map(|((of, _), list)| (*of == agent).then_some(list))
        {
            for entry in listed.iter_mut().filter(|entry| entry.name == name) {
                entry.needs_sign_in = false;
            }
        }
    }

    /// Reads `adapter`'s CLI's own list for `project` now, and keeps it.
    pub(crate) fn list(
        &self,
        clis: &Clis,
        adapter: &dyn AgentAdapter,
        project: Option<&Path>,
    ) -> Result<Vec<Listed>, String> {
        let call = adapter.list_servers(project);
        let (binary, env) = clis.command(adapter)?;
        let started = Instant::now();
        let output = capture(&binary, &call, &env, LIST_TIMEOUT)?;
        let mut listed = adapter.parse_servers(&output);
        let mut lists = lock(&self.lists);
        let signed_in = lock(&self.signed_in);
        for entry in &mut listed {
            if signed_in
                .get(&(adapter.agent(), entry.name.clone()))
                .is_some_and(|at| *at > started)
            {
                entry.needs_sign_in = false;
            }
        }
        lists.insert(
            (adapter.agent(), project.map(Path::to_owned)),
            (Instant::now(), listed.clone()),
        );
        Ok(listed)
    }

    /// `agent`'s last list for `project`, as read; one older than [`LIST_FRESH`] (or none) is
    /// read again in the background.
    pub(crate) fn listed(
        self: &Arc<Self>,
        clis: &Clis,
        agent: Agent,
        project: Option<&Path>,
    ) -> Option<Vec<Listed>> {
        let key = (agent, project.map(Path::to_owned));
        let known = lock(&self.lists).get(&key).cloned();
        if known
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > LIST_FRESH)
            && lock(&self.reading).insert(key.clone())
        {
            let (me, clis) = (self.clone(), clis.clone());
            std::thread::spawn(move || {
                let adapter = brigadier_registry::adapter::adapter(key.0);
                if let Err(err) = me.list(&clis, adapter, key.1.as_deref()) {
                    tracing::debug!(agent = adapter.label(), error = %err, "listing MCP servers");
                }
                lock(&me.reading).remove(&key);
            });
        }
        known.map(|(_, listed)| listed)
    }

    /// Signs every agent that lists `target` out of it, with its CLI's own `mcp logout`.
    pub(crate) fn sign_out(&self, clis: &Clis, target: &Target) -> Result<(), String> {
        for adapter in adapters() {
            let listed = self.list(clis, adapter, target.project.as_deref())?;
            let plugin = target.plugin.as_deref();
            for entry in listed
                .iter()
                .filter(|entry| adapter.is_server(&entry.name, &target.server, plugin))
            {
                let call = adapter.logout(&entry.name, target.project.as_deref());
                let (binary, env) = clis.command(adapter)?;
                capture(&binary, &call, &env, LIST_TIMEOUT)?;
            }
            self.list(clis, adapter, target.project.as_deref())?;
        }
        Ok(())
    }
}

/// The address of a sign-in page in a CLI's output: an `http(s)` link carrying an OAuth
/// request (`code_challenge` or `response_type`), else none.
pub(crate) fn sign_in_link(output: &str) -> Option<String> {
    let text = plain_text(output);
    let mut links = text
        .match_indices("http")
        .map(|(start, _)| &text[start..])
        .filter(|rest| rest.starts_with("http://") || rest.starts_with("https://"))
        .map(|rest| {
            let end = rest
                .find(|c: char| c.is_whitespace() || c.is_control())
                .unwrap_or(rest.len());
            &rest[..end]
        });
    links
        .find(|link| link.contains("code_challenge=") || link.contains("response_type="))
        .map(str::to_owned)
}

/// The last line of `output` worth showing, for a failure.
fn last_words(output: &str) -> Option<String> {
    plain_text(output)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| {
            line.rsplit_once("Error: ")
                .map_or(line, |(_, words)| words)
                .trim()
                .to_owned()
        })
}

fn base_command(binary: &Path, call: &CliCall, env: &[(String, String)]) -> std::process::Command {
    let mut command = std::process::Command::new(binary);
    command
        .args(&call.args)
        .env_clear()
        .envs(env.iter().map(|(key, value)| (key, value)));
    if let Some(dir) = &call.cwd {
        command.current_dir(dir);
    }
    command
}

/// Runs `call` to its end with no input; its standard output, or its own words on why it
/// failed.
fn capture(
    binary: &Path,
    call: &CliCall,
    env: &[(String, String)],
    timeout: Duration,
) -> Result<String, String> {
    use std::process::Stdio;
    let mut child = base_command(binary, call, env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("couldn't run {}: {err}", call.program))?;
    let out = read_all(child.stdout.take());
    let err = read_all(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{} didn't answer in time.", call.program));
            }
        }
    };
    let (out, err) = (
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
    );
    if status.success() {
        Ok(out)
    } else {
        Err(last_words(&err)
            .or_else(|| last_words(&out))
            .unwrap_or_else(|| format!("{} failed ({status}).", call.program)))
    }
}

fn read_all(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// A running login: its output as it comes, and a way to end it.
trait Login {
    /// `Some(success)` once it ended.
    fn ended(&mut self) -> Option<bool>;
    fn kill(&mut self);
}

struct Piped(std::process::Child);

impl Login for Piped {
    fn ended(&mut self) -> Option<bool> {
        match self.0.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }
    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Pty {
    child: Box<dyn portable_pty::Child + Send + Sync>,
    /// Kept open while it runs: closing it would hang up on the CLI.
    _master: Box<dyn portable_pty::MasterPty + Send>,
}

impl Login for Pty {
    fn ended(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.success()),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }
    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Sends what `stream` prints to `sink`, a chunk at a time, until it ends.
fn forward(mut stream: impl Read + Send + 'static, sink: mpsc::Sender<String>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stream.read(&mut buffer) {
            if read == 0
                || sink
                    .send(String::from_utf8_lossy(&buffer[..read]).into_owned())
                    .is_err()
            {
                break;
            }
        }
    });
}

/// Starts `call`: in a PTY when it needs a terminal, else with its input left open (Codex waits
/// for the browser's callback only while it can still be pasted one).
fn spawn_login(
    binary: &Path,
    call: &CliCall,
    env: &[(String, String)],
    sink: mpsc::Sender<String>,
) -> Result<Box<dyn Login>, String> {
    let failed = |err: &dyn std::fmt::Display| format!("couldn't run {}: {err}", call.program);
    if call.needs_terminal {
        let system = portable_pty::native_pty_system();
        let pair = system
            .openpty(portable_pty::PtySize {
                rows: 24,
                // Wide, so nothing it prints is cut.
                cols: 1000,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| failed(&err))?;
        let mut command = portable_pty::CommandBuilder::new(binary);
        command.args(&call.args);
        command.env_clear();
        for (key, value) in env {
            command.env(key, value);
        }
        if let Some(dir) = &call.cwd {
            command.cwd(dir);
        }
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|err| failed(&err))?;
        drop(pair.slave);
        forward(
            pair.master.try_clone_reader().map_err(|err| failed(&err))?,
            sink,
        );
        Ok(Box::new(Pty {
            child,
            _master: pair.master,
        }))
    } else {
        use std::process::Stdio;
        let mut child = base_command(binary, call, env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| failed(&err))?;
        if let Some(out) = child.stdout.take() {
            forward(out, sink.clone());
        }
        if let Some(err) = child.stderr.take() {
            forward(err, sink);
        }
        Ok(Box::new(Piped(child)))
    }
}

/// Runs a CLI's `mcp login` to its end: `opened` gets its sign-in page once it prints one.
fn login(
    binary: &Path,
    call: &CliCall,
    env: &[(String, String)],
    opened: &dyn Fn(&str),
    cancel: &AtomicBool,
) -> Result<(), String> {
    let (sink, source) = mpsc::channel();
    let mut running = spawn_login(binary, call, env, sink)?;
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut output = String::new();
    let mut link = None;
    let ended = loop {
        match source.recv_timeout(POLL) {
            Ok(chunk) => output.push_str(&chunk),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(POLL),
        }
        if link.is_none()
            && let Some(found) = sign_in_link(&output)
        {
            opened(&found);
            link = Some(found);
        }
        // Asked every time, so a success counts the moment the CLI ends.
        if let Some(success) = running.ended() {
            break success;
        }
        if cancel.load(Ordering::SeqCst) {
            running.kill();
            return Err("Cancelled.".to_owned());
        }
        if Instant::now() > deadline {
            running.kill();
            return Err("The sign-in took too long.".to_owned());
        }
    };
    if ended {
        return Ok(());
    }
    // What it printed last, to say why.
    while let Ok(chunk) = source.recv_timeout(Duration::from_millis(200)) {
        output.push_str(&chunk);
    }
    Err(last_words(&output).unwrap_or_else(|| format!("{} couldn't sign in.", call.program)))
}

#[cfg(test)]
#[path = "mcp_login_tests.rs"]
mod tests;
