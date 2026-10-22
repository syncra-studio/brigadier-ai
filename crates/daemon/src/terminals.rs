//! Bottom-pane shells, independently keyed within each conversation (or Home), on a
//! pseudo terminal. Its output streams live to the connections that opened it, and a tail is
//! kept so a tab opened again shows what came before; none of it is stored. A shell ends when
//! its tab closes, its conversation is archived or deleted, or the daemon quits.
//!
//! A worker's terminal ("Open in terminal") runs the worker's own CLI directly, not through a
//! shell, with exactly the environment the session manager gives it; the session manager hears
//! when it ends, and can end it and wait until its process is gone ([`TerminalHost`]).

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use brigadier_core::Error;
use brigadier_core::manager::{HostedTerminal, TerminalHost, TerminalSnapshot};
use brigadier_ipc::protocol::{TerminalInfo, TerminalOutput};
use brigadier_providers::TerminalCommand;
use brigadier_providers::login_mark::LoginMark;
use brigadier_sandbox::Platform;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, oneshot, watch};

/// The output kept for a tab opened again, in bytes of text.
const SCROLLBACK: usize = 256 * 1024;
/// Output chunks a connection may fall behind by before it misses some.
const FEED: usize = 1024;
const READ_CHUNK: usize = 16 * 1024;
/// How much of a terminal's latest output a snapshot carries, in bytes.
const SNAPSHOT_TAIL: usize = 4 * 1024;
/// How long an ended terminal's process has to be reaped after its tree was killed.
const TERMINATE_WAIT: Duration = Duration::from_secs(5);
/// How often, and how many times, an ended shell is checked for its exit code.
const REAP_POLL: Duration = Duration::from_millis(50);
const REAP_TRIES: u32 = 40;

type Result<T> = std::result::Result<T, Error>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub struct Terminals {
    live: Arc<Mutex<HashMap<String, Arc<Terminal>>>>,
    /// Closed tabs whose processes still have time to end; shutdown owns them too.
    lingering: Arc<Mutex<HashMap<String, Arc<Terminal>>>>,
    feed: broadcast::Sender<TerminalOutput>,
    next: AtomicU64,
    /// Ends a worker terminal's process tree.
    platform: Option<Arc<dyn Platform>>,
    /// Where the agents' own installers put them ([`CliEnv::installer_folder`]): on Windows
    /// a shell's PATH ends with those that exist, as Claude Code's installer doesn't add its
    /// folder to the user's Path, so `claude` there is the one Brigadier finds.
    ///
    /// [`CliEnv::installer_folder`]: brigadier_providers::CliEnv::installer_folder
    agent_folders: Vec<PathBuf>,
}

struct Terminal {
    id: String,
    conversation: String,
    session: Option<String>,
    shell: String,
    cwd: String,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Mutex<Box<dyn Child + Send + Sync>>,
    /// Held while output is kept and sent, so a reattach's snapshot and the feed agree.
    scrollback: Mutex<String>,
    /// Told when its process has ended (a worker's terminal).
    on_exit: Mutex<Option<oneshot::Sender<()>>>,
    /// Becomes true once its output ended and its process was reaped.
    ended: watch::Sender<bool>,
    pid: Option<u32>,
    account_sign_in: Mutex<Option<AccountSignIn>>,
}

#[derive(Clone)]
struct AccountSignIn {
    mark: Option<LoginMark>,
    before: Option<String>,
    started_at_ms: i64,
}

impl Terminal {
    fn info(&self) -> TerminalInfo {
        TerminalInfo {
            id: self.id.clone(),
            shell: self.shell.clone(),
            cwd: self.cwd.clone(),
            scrollback: lock(&self.scrollback).clone(),
        }
    }

    /// Whether a command runs in front of its shell: the pty's foreground process group is
    /// another than the shell's own. A worker's terminal runs its CLI, so it always is busy.
    fn busy(&self) -> Option<bool> {
        if self
            .session
            .as_deref()
            .is_some_and(|key| key.starts_with("worker:"))
        {
            return Some(true);
        }
        foreground_busy(lock(&self.master).as_ref(), self.pid)
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let scrollback = lock(&self.scrollback);
        let mut from = scrollback.len().saturating_sub(SNAPSHOT_TAIL);
        while !scrollback.is_char_boundary(from) {
            from += 1;
        }
        TerminalSnapshot {
            id: self.id.clone(),
            key: self.session.clone(),
            cwd: self.cwd.clone(),
            busy: self.busy(),
            tail: scrollback[from..].to_owned(),
        }
    }

    fn kill(&self) {
        if let Err(err) = lock(&self.child).kill() {
            tracing::debug!(terminal = %self.id, error = %err, "terminal shell already ended");
        }
    }

    /// Ends it: a worker's terminal with its whole process tree (what its CLI started must
    /// not go on in the task's checkout), any other with its shell.
    fn end(&self, platform: Option<&Arc<dyn Platform>>) {
        let worker = self
            .session
            .as_deref()
            .is_some_and(|key| key.starts_with("worker:"));
        match (platform, self.pid) {
            (Some(platform), Some(pid)) if worker => {
                if let Err(err) = platform.processes().kill_tree(pid) {
                    tracing::debug!(terminal = %self.id, error = %err, "could not kill a terminal's tree");
                    self.kill();
                }
            }
            _ => self.kill(),
        }
    }
}

impl Terminals {
    pub fn new() -> Self {
        let (feed, _) = broadcast::channel(FEED);
        Self {
            live: Arc::new(Mutex::new(HashMap::new())),
            lingering: Arc::new(Mutex::new(HashMap::new())),
            feed,
            next: AtomicU64::new(1),
            platform: None,
            agent_folders: Vec::new(),
        }
    }

    /// Terminals that can end a worker terminal's whole process tree.
    pub fn with_platform(platform: Arc<dyn Platform>) -> Self {
        Self {
            platform: Some(platform),
            ..Self::new()
        }
    }

    /// Shells on Windows also find the agents in `folders`, their installers' own.
    pub fn with_agent_folders(self, folders: Vec<PathBuf>) -> Self {
        Self {
            agent_folders: folders,
            ..self
        }
    }

    /// Every terminal's output; a connection forwards the ones it opened.
    pub fn subscribe(&self) -> broadcast::Receiver<TerminalOutput> {
        self.feed.subscribe()
    }

    /// The running terminal of `conversation`'s `session`, if any.
    pub fn running(&self, conversation: &str, session: &str) -> Option<String> {
        lock(&self.live)
            .values()
            .find(|terminal| {
                terminal.conversation == conversation
                    && terminal.session.as_deref() == Some(session)
            })
            .map(|terminal| terminal.id.clone())
    }

    /// Opens an independent shell while keeping archive/delete ownership on the conversation.
    pub fn open_session(
        &self,
        conversation: &str,
        session: Option<&str>,
        cwd: String,
        cols: u16,
        rows: u16,
    ) -> Result<TerminalInfo> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let running = lock(&self.live)
            .values()
            .find(|terminal| {
                terminal.conversation == conversation && terminal.session.as_deref() == session
            })
            .cloned();
        if let Some(terminal) = running {
            if let Err(err) = lock(&terminal.master).resize(size) {
                tracing::debug!(terminal = %terminal.id, error = %err, "resize failed");
            }
            return Ok(terminal.info());
        }

        // Reattaching above does not need the original folder to still exist.
        if !std::path::Path::new(&cwd).is_absolute() || !std::path::Path::new(&cwd).is_dir() {
            return Err(Error::Invalid(
                "terminal folder is not an existing absolute directory".into(),
            ));
        }
        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        if cfg!(unix) {
            // A login shell, so the user's PATH and profile apply as in their own terminal.
            command.arg("-l");
        }
        command.cwd(&cwd);
        // The daemon's own settings are not the user's.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("BRIGADIER_") {
                command.env_remove(key);
            }
        }
        // The builder's PATH is the system's and the user's from the registry, read now.
        if cfg!(windows)
            && let Some(path) = with_folders(command.get_env("PATH"), &self.agent_folders)
        {
            command.env("PATH", path);
        }
        let terminal = self.spawn(conversation, session, shell, cwd, command, size, None)?;
        // A new shell's first output already streams to the connection that subscribed
        // before opening it; sending it here too would show it twice.
        Ok(TerminalInfo {
            scrollback: String::new(),
            ..terminal.info()
        })
    }

    /// Starts `command` on a new pseudo terminal and pumps its output.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        conversation: &str,
        session: Option<&str>,
        shell: String,
        cwd: String,
        mut command: CommandBuilder,
        size: PtySize,
        on_exit: Option<oneshot::Sender<()>>,
    ) -> Result<Arc<Terminal>> {
        let failed =
            |err: anyhow::Error| Error::Invalid(format!("couldn't start a terminal: {err}"));
        let pair = native_pty_system().openpty(size).map_err(failed)?;
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(command).map_err(failed)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(failed)?;
        let writer = pair.master.take_writer().map_err(failed)?;

        let id = format!("terminal-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let pid = child.process_id();
        let terminal = Arc::new(Terminal {
            id: id.clone(),
            conversation: conversation.to_owned(),
            session: session.map(str::to_owned),
            shell,
            cwd,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child: Mutex::new(child),
            scrollback: Mutex::new(String::new()),
            on_exit: Mutex::new(on_exit),
            ended: watch::Sender::new(false),
            pid,
            account_sign_in: Mutex::new(None),
        });
        lock(&self.live).insert(id.clone(), terminal.clone());
        let live = self.live.clone();
        let lingering = self.lingering.clone();
        let feed = self.feed.clone();
        let pump = terminal.clone();
        let started = std::thread::Builder::new()
            .name(format!("{id} output"))
            .spawn(move || pump_output(&pump, reader, &feed, &live, &lingering));
        if let Err(err) = started {
            lock(&self.live).remove(&id);
            terminal.kill();
            return Err(Error::Invalid(format!("couldn't start a terminal: {err}")));
        }
        tracing::info!(terminal = %id, conversation, "terminal started");
        Ok(terminal)
    }

    /// A worker's terminal: `command` run directly (no shell), with exactly its environment.
    pub fn start_command(
        &self,
        conversation: &str,
        key: &str,
        command: TerminalCommand,
        cols: u16,
        rows: u16,
    ) -> Result<HostedTerminal> {
        let size = PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        };
        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.args);
        builder.cwd(&command.cwd);
        builder.env_clear();
        for (key, value) in &command.env {
            builder.env(key, value);
        }
        let name = command
            .program
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (exit_tx, exited) = oneshot::channel();
        let terminal = self.spawn(
            conversation,
            Some(key),
            name,
            command.cwd.display().to_string(),
            builder,
            size,
            Some(exit_tx),
        )?;
        let started_at_ms = match (&self.platform, terminal.pid) {
            (Some(platform), Some(pid)) => platform.processes().start_time_ms(pid).ok(),
            _ => None,
        };
        Ok(HostedTerminal {
            id: terminal.id.clone(),
            pid: terminal.pid,
            started_at_ms,
            exited,
        })
    }

    /// A running terminal resized to `cols` × `rows`, and everything it showed so far. Its
    /// output already queued in `feed` (this connection's subscription) is dropped, so
    /// nothing shows twice; what came for other terminals meanwhile is returned to forward.
    pub fn attach(
        &self,
        id: &str,
        feed: &mut broadcast::Receiver<TerminalOutput>,
        cols: u16,
        rows: u16,
    ) -> Result<(TerminalInfo, Vec<TerminalOutput>)> {
        let terminal = self.get(id)?;
        if let Err(err) = lock(&terminal.master).resize(PtySize {
            rows: rows.max(1),
            cols: cols.max(1),
            pixel_width: 0,
            pixel_height: 0,
        }) {
            tracing::debug!(terminal = %id, error = %err, "resize failed");
        }
        // No output is kept or sent while this holds the scrollback.
        let scrollback = lock(&terminal.scrollback);
        let mut others = Vec::new();
        loop {
            match feed.try_recv() {
                Ok(output) => {
                    let ours = match &output {
                        TerminalOutput::Data { terminal_id, .. }
                        | TerminalOutput::Exited { terminal_id, .. } => terminal_id == id,
                    };
                    if !ours {
                        others.push(output);
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
        let info = TerminalInfo {
            id: terminal.id.clone(),
            shell: terminal.shell.clone(),
            cwd: terminal.cwd.clone(),
            scrollback: scrollback.clone(),
        };
        Ok((info, others))
    }

    fn get(&self, id: &str) -> Result<Arc<Terminal>> {
        lock(&self.live)
            .get(id)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("terminal {id}")))
    }

    pub fn write(&self, id: &str, data: &str) -> Result<()> {
        let terminal = self.get(id)?;
        let mut writer = lock(&terminal.writer);
        writer
            .write_all(data.as_bytes())
            .and_then(|()| writer.flush())
            .map_err(|err| Error::Invalid(format!("couldn't write to the terminal: {err}")))
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<()> {
        let terminal = self.get(id)?;
        lock(&terminal.master)
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|err| Error::Invalid(format!("couldn't resize the terminal: {err}")))
    }

    /// Forgets the saved output but the line in progress (the prompt), as the view's clear does.
    pub fn clear(&self, id: &str) -> Result<()> {
        let terminal = self.get(id)?;
        let mut scrollback = lock(&terminal.scrollback);
        let keep = scrollback.rfind('\n').map_or(0, |at| at + 1);
        scrollback.drain(..keep);
        Ok(())
    }

    /// How many terminals run.
    pub fn count(&self) -> usize {
        lock(&self.live).len()
    }

    /// The key a terminal was started under (an extra account's sign-in: `account:<id>`).
    pub fn session_of(&self, id: &str) -> Option<String> {
        lock(&self.live).get(id)?.session.clone()
    }

    /// Remembers where this terminal's login stood before its own sign-in started.
    pub fn watch_account_sign_in(
        &self,
        id: &str,
        mark: Option<LoginMark>,
        before: Option<String>,
        started_at_ms: i64,
    ) {
        if let Ok(terminal) = self.get(id) {
            *lock(&terminal.account_sign_in) = Some(AccountSignIn {
                mark,
                before,
                started_at_ms,
            });
        }
    }

    /// A cached signed-in account is not evidence this sign-in completed: its login must
    /// have changed, and a check since this terminal started must have read as signed in.
    pub async fn close_account_sign_in(
        &self,
        id: &str,
        checked_at_ms: Option<i64>,
        signed_in: bool,
        grace: Duration,
    ) {
        let sign_in = self
            .get(id)
            .ok()
            .and_then(|terminal| lock(&terminal.account_sign_in).clone());
        let completed = match sign_in {
            Some(AccountSignIn {
                mark: Some(mark),
                before: Some(before),
                started_at_ms,
            }) if signed_in && checked_at_ms.is_some_and(|at| at > started_at_ms) => {
                mark.read().await.is_some_and(|now| now != before)
            }
            _ => false,
        };
        if completed {
            self.close_after(id, grace);
        } else {
            self.close(id);
        }
    }

    /// Lets a terminal go as [`Self::close`] does, but gives its process `grace` to end by
    /// itself first: a sign-in that is done may still be saving what it saves.
    fn close_after(&self, id: &str, grace: Duration) {
        let terminal = {
            // The move shares close_all's lock order, so shutdown cannot miss a process
            // between the visible and lingering sets.
            let mut live = lock(&self.live);
            let Some(terminal) = live.remove(id) else {
                return;
            };
            lock(&self.lingering).insert(id.to_owned(), terminal.clone());
            terminal
        };
        let platform = self.platform.clone();
        let mut ended = terminal.ended.subscribe();
        tokio::spawn(async move {
            if tokio::time::timeout(grace, ended.wait_for(|ended| *ended))
                .await
                .is_err()
            {
                terminal.end(platform.as_ref());
            }
        });
    }

    /// Ends a terminal's shell; closing one that already ended is fine.
    pub fn close(&self, id: &str) {
        let terminal = {
            let mut live = lock(&self.live);
            live.remove(id).or_else(|| lock(&self.lingering).remove(id))
        };
        if let Some(terminal) = terminal {
            terminal.end(self.platform.as_ref());
        }
    }

    /// Ends the conversation's terminals. They are let go of at once, so a shell opened for it
    /// afterwards (it was restored) is a new one; ending them happens on its own thread, so
    /// archiving and deleting don't wait for it.
    pub fn close_conversation(&self, conversation: &str) {
        let ended: Vec<Arc<Terminal>> = {
            let mut live = lock(&self.live);
            let ids: Vec<String> = live
                .values()
                .filter(|terminal| terminal.conversation == conversation)
                .map(|terminal| terminal.id.clone())
                .collect();
            ids.iter().filter_map(|id| live.remove(id)).collect()
        };
        if ended.is_empty() {
            return;
        }
        let killed = std::thread::Builder::new()
            .name("terminal close".into())
            .spawn({
                let (ended, platform) = (ended.clone(), self.platform.clone());
                move || {
                    ended
                        .iter()
                        .for_each(|terminal| terminal.end(platform.as_ref()))
                }
            });
        if killed.is_err() {
            ended
                .iter()
                .for_each(|terminal| terminal.end(self.platform.as_ref()));
        }
    }

    /// Ends every shell (the daemon is quitting).
    pub fn close_all(&self) {
        let terminals: Vec<Arc<Terminal>> = {
            let mut live = lock(&self.live);
            let mut lingering = lock(&self.lingering);
            live.drain()
                .chain(lingering.drain())
                .map(|(_, t)| t)
                .collect()
        };
        for terminal in terminals {
            terminal.end(self.platform.as_ref());
        }
    }
}

/// Reads a shell's output until it ends: keeps its tail and sends it to the connections, then
/// says how it ended.
fn pump_output(
    terminal: &Terminal,
    mut reader: Box<dyn Read + Send>,
    feed: &broadcast::Sender<TerminalOutput>,
    live: &Mutex<HashMap<String, Arc<Terminal>>>,
    lingering: &Mutex<HashMap<String, Arc<Terminal>>>,
) {
    let mut buffer = vec![0; READ_CHUNK];
    let mut decoder = Utf8Stream::default();
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let data = decoder.push(&buffer[..read]);
        if data.is_empty() {
            continue;
        }
        let mut scrollback = lock(&terminal.scrollback);
        scrollback.push_str(&data);
        if scrollback.len() > SCROLLBACK {
            let mut cut = scrollback.len() - SCROLLBACK;
            while !scrollback.is_char_boundary(cut) {
                cut += 1;
            }
            scrollback.drain(..cut);
        }
        // Sent under the scrollback's lock: a reattach sees it either kept or queued. No
        // receiver just means no tab is open.
        let _ = feed.send(TerminalOutput::Data {
            terminal_id: terminal.id.clone(),
            data,
        });
    }
    let code = reap(terminal);
    {
        let mut live = lock(live);
        if live
            .get(&terminal.id)
            .is_some_and(|current| std::ptr::eq(Arc::as_ptr(current), terminal))
        {
            live.remove(&terminal.id);
        }
        lock(lingering).remove(&terminal.id);
    }
    tracing::info!(terminal = %terminal.id, ?code, "terminal ended");
    let _ = feed.send(TerminalOutput::Exited {
        terminal_id: terminal.id.clone(),
        code,
    });
    terminal.ended.send_replace(true);
    if let Some(on_exit) = lock(&terminal.on_exit).take() {
        let _ = on_exit.send(());
    }
}

impl TerminalHost for Terminals {
    fn start(
        &self,
        conversation: &str,
        key: &str,
        command: TerminalCommand,
        cols: u16,
        rows: u16,
    ) -> brigadier_core::Result<HostedTerminal> {
        self.start_command(conversation, key, command, cols, rows)
    }

    fn terminate(
        &self,
        id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        let terminal = self.get(id).ok();
        let platform = self.platform.clone();
        let id = id.to_owned();
        Box::pin(async move {
            let Some(terminal) = terminal else {
                return;
            };
            let mut ended = terminal.ended.subscribe();
            terminal.end(platform.as_ref());
            if tokio::time::timeout(TERMINATE_WAIT, ended.wait_for(|ended| *ended))
                .await
                .is_err()
            {
                tracing::warn!(terminal = %id, "a terminal's process did not end in time");
            }
        })
    }

    fn list(&self, conversation: &str) -> Vec<TerminalSnapshot> {
        let mut terminals: Vec<Arc<Terminal>> = lock(&self.live)
            .values()
            .filter(|terminal| terminal.conversation == conversation)
            .cloned()
            .collect();
        // Started in order: their ids count up.
        terminals.sort_by_key(|terminal| {
            terminal
                .id
                .trim_start_matches("terminal-")
                .parse::<u64>()
                .unwrap_or(u64::MAX)
        });
        terminals
            .iter()
            .map(|terminal| terminal.snapshot())
            .collect()
    }

    fn output(&self, conversation: &str, id: &str) -> Option<String> {
        let terminal = self.get(id).ok()?;
        (terminal.conversation == conversation).then(|| lock(&terminal.scrollback).clone())
    }

    fn open_shell(
        &self,
        conversation: &str,
        key: &str,
        cwd: &str,
    ) -> brigadier_core::Result<String> {
        // Sized as a tab starts; the tab resizes it when it shows.
        self.open_session(conversation, Some(key), cwd.to_owned(), 120, 32)
            .map(|terminal| terminal.id)
    }

    fn type_in(&self, conversation: &str, id: &str, data: &str) -> brigadier_core::Result<()> {
        if self.get(id)?.conversation != conversation {
            return Err(Error::NotFound(format!("terminal {id}")));
        }
        self.write(id, data)
    }
}

#[cfg(unix)]
fn foreground_busy(master: &(dyn MasterPty + Send), pid: Option<u32>) -> Option<bool> {
    let leader = master.process_group_leader()?;
    let pid = pid?;
    Some(u32::try_from(leader).ok()? != pid)
}

/// Windows' pseudo console doesn't say what runs in front.
#[cfg(not(unix))]
fn foreground_busy(_master: &(dyn MasterPty + Send), _pid: Option<u32>) -> Option<bool> {
    None
}

/// The ended shell's exit code. Its output closed, so it is exiting; one that lingers past
/// `REAP_TRIES` polls is killed.
fn reap(terminal: &Terminal) -> Option<u32> {
    for _ in 0..REAP_TRIES {
        match lock(&terminal.child).try_wait() {
            Ok(Some(status)) => return Some(status.exit_code()),
            Ok(None) => std::thread::sleep(REAP_POLL),
            Err(_) => return None,
        }
    }
    terminal.kill();
    None
}

/// `path` with each of `folders` that exists and isn't on it yet added at its end, in order;
/// `None` when none is added. `path` itself is kept as it is, first. Folders compare as
/// Windows does, in any case and with or without a trailing separator.
fn with_folders(path: Option<&OsStr>, folders: &[PathBuf]) -> Option<OsString> {
    let key = |dir: &Path| {
        dir.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    let mut seen: Vec<String> = path
        .map(|path| std::env::split_paths(path).map(|dir| key(&dir)).collect())
        .unwrap_or_default();
    let mut added = Vec::new();
    for folder in folders {
        if folder.is_dir() && !seen.contains(&key(folder)) {
            seen.push(key(folder));
            added.push(folder);
        }
    }
    if added.is_empty() {
        return None;
    }
    let added = std::env::join_paths(added).ok()?;
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Some(added);
    };
    let mut joined = path.to_owned();
    joined.push(if cfg!(windows) { ";" } else { ":" });
    joined.push(added);
    Some(joined)
}

/// The user's shell: `$SHELL` on macOS and Linux, PowerShell on Windows.
fn default_shell() -> String {
    if cfg!(windows) {
        return "powershell.exe".into();
    }
    std::env::var("SHELL")
        .ok()
        .filter(|shell| shell.starts_with('/'))
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/sh".into()
            }
        })
}

/// Turns a byte stream into text without splitting a character across two chunks.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        loop {
            match std::str::from_utf8(&self.pending) {
                Ok(valid) => {
                    text.push_str(valid);
                    self.pending.clear();
                    return text;
                }
                Err(err) => {
                    let valid = err.valid_up_to();
                    text.push_str(&String::from_utf8_lossy(&self.pending[..valid]));
                    match err.error_len() {
                        // A character cut at the end: keep its start for the next chunk.
                        None => {
                            self.pending.drain(..valid);
                            return text;
                        }
                        Some(bad) => {
                            text.push(char::REPLACEMENT_CHARACTER);
                            self.pending.drain(..valid + bad);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::Terminals;
    #[cfg(unix)]
    use super::{
        Arc, Duration, HostedTerminal, LoginMark, Terminal, TerminalCommand, TerminalHost, Write,
        lock,
    };

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

    /// Whether this process can't open a terminal: the sandbox around a session's full checks
    /// refuses pseudo-terminals (#521), so these tests run where none is (CI, a terminal, a
    /// Full-access session's checks); says so in the test's output.
    fn no_terminal_here() -> bool {
        let refused = brigadier_sandbox::confined()
            && super::native_pty_system()
                .openpty(super::PtySize::default())
                .is_err();
        if refused {
            eprintln!("skipped: the sandbox this test runs in opens no terminal");
        }
        refused
    }

    #[test]
    fn independent_sessions_reattach_and_close_with_their_conversation() {
        if no_terminal_here() {
            return;
        }
        let terminals = Terminals::new();
        let cwd = std::env::temp_dir().display().to_string();
        let first = terminals
            .open_session("pane-test", Some("first"), cwd.clone(), 80, 24)
            .unwrap();
        let second = terminals
            .open_session("pane-test", Some("second"), cwd.clone(), 80, 24)
            .unwrap();
        let other = terminals
            .open_session("other-chat", Some("first"), cwd.clone(), 80, 24)
            .unwrap();
        let reattached = terminals
            .open_session("pane-test", Some("first"), cwd, 100, 30)
            .unwrap();
        assert_ne!(first.id, second.id);
        assert_ne!(first.id, other.id);
        assert_eq!(first.id, reattached.id);
        assert_eq!(terminals.count(), 3);
        terminals.close_conversation("pane-test");
        assert_eq!(terminals.count(), 1);
        assert!(terminals.get(&other.id).is_ok());
        terminals.close_all();
        assert_eq!(terminals.count(), 0);
    }

    /// Polls `check` for up to ten seconds.
    fn eventually(mut check: impl FnMut() -> bool) -> bool {
        for _ in 0..200 {
            if check() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    }

    /// An agent's folder joins PATH at its end when it exists and isn't there yet; what PATH
    /// holds stays as it is, first.
    #[test]
    fn agent_folders_that_exist_join_the_end_of_path() {
        use std::ffi::OsString;

        use super::with_folders;

        let home = Temp(
            std::env::temp_dir().join(format!("brig-agent-folders-{} a b", std::process::id())),
        );
        let (claude, codex, gone) = (
            home.join("claude bin"),
            home.join("codex"),
            home.join("gone"),
        );
        std::fs::create_dir_all(&claude).unwrap();
        std::fs::create_dir_all(&codex).unwrap();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let path = OsString::from(format!("{}{sep}{sep}/usr/bin", codex.display()));
        let joined = |dirs: &[&Path]| std::env::join_paths(dirs).unwrap();

        assert_eq!(
            with_folders(Some(&path), &[claude.clone(), codex.clone(), gone.clone()]),
            Some(OsString::from(format!(
                "{}{sep}{}",
                path.to_string_lossy(),
                joined(&[&claude]).to_string_lossy()
            ))),
        );
        assert_eq!(
            with_folders(Some(&path), &[codex.clone(), gone.clone()]),
            None
        );
        assert_eq!(with_folders(Some(&path), &[]), None);
        assert_eq!(
            with_folders(None, &[claude.clone(), codex.clone(), claude.clone()]),
            Some(joined(&[&claude, &codex]))
        );
        assert_eq!(
            with_folders(Some(&OsString::new()), std::slice::from_ref(&codex)),
            Some(joined(&[&codex]))
        );
    }

    /// Windows finds a folder on PATH in any case, with or without a trailing backslash.
    #[cfg(windows)]
    #[test]
    fn an_agent_folder_already_on_path_in_another_case_is_not_added_again() {
        use std::ffi::OsString;

        use super::with_folders;

        let home =
            Temp(std::env::temp_dir().join(format!("brig-agent-case-{}", std::process::id())));
        let bin = home.join(r".local\bin");
        std::fs::create_dir_all(&bin).unwrap();
        let upper = bin.display().to_string().to_uppercase();
        let path = OsString::from(format!(r"C:\Windows\System32;{upper}\"));
        assert_eq!(with_folders(Some(&path), &[bin]), None);
    }

    /// On Windows a Brigadier shell runs an agent from its installer's folder even when the
    /// user's Path lacks it, as Claude Code's installer leaves it.
    #[cfg(windows)]
    #[test]
    fn a_windows_shell_finds_a_program_in_an_agent_folder() {
        use brigadier_core::manager::TerminalHost;

        let home =
            Temp(std::env::temp_dir().join(format!("brig-agent-shell-{} a b", std::process::id())));
        let bin = home.join(r".local\bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(
            bin.join("brigadier-agent-probe.cmd"),
            "@echo agent folder on PATH\r\n",
        )
        .unwrap();
        let terminals = Terminals::new().with_agent_folders(vec![bin]);
        let cwd = std::env::temp_dir().display().to_string();
        let shell = terminals
            .open_session("agent-folders", Some("tab"), cwd, 120, 30)
            .unwrap();
        // ConPTY asks where the cursor is and waits for the answer, as a terminal gives it.
        assert!(eventually(|| terminals
            .output("agent-folders", &shell.id)
            .is_some_and(|text| text.contains("\u{1b}[6n"))));
        terminals.write(&shell.id, "\u{1b}[1;1R").unwrap();
        terminals
            .write(&shell.id, "brigadier-agent-probe\r")
            .unwrap();
        let found = eventually(|| {
            terminals
                .output("agent-folders", &shell.id)
                .is_some_and(|text| text.contains("agent folder on PATH"))
        });
        let output = terminals.output("agent-folders", &shell.id);
        terminals.close_all();
        assert!(found, "{output:?}");
    }

    /// An idle shell reads idle, a command in front of it reads busy, and only the
    /// conversation's own terminals are listed and read (Q90's `app_state`/`read_terminal`).
    #[cfg(unix)]
    #[test]
    fn a_shell_reads_idle_until_a_command_runs_in_front_of_it() {
        if no_terminal_here() {
            return;
        }
        use brigadier_core::manager::TerminalHost;

        let terminals = Terminals::new();
        let cwd = std::env::temp_dir().display().to_string();
        let shell = terminals
            .open_session("busy-test", Some("terminal:tab"), cwd.clone(), 80, 24)
            .unwrap();
        terminals
            .open_session("other-busy-test", Some("terminal:tab"), cwd, 80, 24)
            .unwrap();
        let busy = || terminals.list("busy-test")[0].busy;
        assert!(eventually(|| busy() == Some(false)), "{:?}", busy());
        terminals
            .write(&shell.id, "echo started-$((40+2)); sleep 30\n")
            .unwrap();
        assert!(eventually(|| busy() == Some(true)), "{:?}", busy());
        assert!(eventually(|| terminals.list("busy-test")[0]
            .tail
            .contains("started-42")));
        let listed = terminals.list("busy-test");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, shell.id);
        assert_eq!(listed[0].key.as_deref(), Some("terminal:tab"));
        // Ctrl-C ends the command: the shell is in front again.
        terminals.write(&shell.id, "\u{3}").unwrap();
        assert!(eventually(|| busy() == Some(false)), "{:?}", busy());
        assert!(
            terminals
                .output("busy-test", &shell.id)
                .is_some_and(|text| text.contains("started-42"))
        );
        assert_eq!(terminals.output("other-busy-test", &shell.id), None);
        terminals.close_all();
    }

    /// `terminal_run`'s shell: one per tab key, the same when asked again, typed into only
    /// for its own conversation.
    #[cfg(unix)]
    #[test]
    fn an_agents_tab_shell_is_opened_once_and_typed_into_only_for_its_conversation() {
        if no_terminal_here() {
            return;
        }
        use brigadier_core::manager::TerminalHost;

        let terminals = Terminals::new();
        let cwd = std::env::temp_dir().display().to_string();
        let id = terminals
            .open_shell("run-test", "terminal:agent-1", &cwd)
            .unwrap();
        assert_eq!(
            terminals
                .open_shell("run-test", "terminal:agent-1", &cwd)
                .unwrap(),
            id
        );
        assert!(
            terminals
                .type_in("other-run-test", &id, "echo nope\r")
                .is_err()
        );
        terminals
            .type_in("run-test", &id, "echo typed-$((40+2))\r")
            .unwrap();
        assert!(eventually(|| terminals
            .output("run-test", &id)
            .is_some_and(|text| text.contains("typed-42"))));
        let listed = terminals.list("run-test");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key.as_deref(), Some("terminal:agent-1"));
        terminals.close_all();
    }

    #[cfg(unix)]
    #[test]
    fn reattach_survives_a_removed_folder_but_new_shells_require_one() {
        if no_terminal_here() {
            return;
        }
        let folder =
            Temp(std::env::temp_dir().join(format!("brig-removed-cwd-{}", std::process::id())));
        std::fs::create_dir_all(&*folder).unwrap();
        let cwd = folder.display().to_string();
        let terminals = Terminals::new();
        for session in [Some("tab"), None] {
            let first = terminals
                .open_session("removed-cwd", session, cwd.clone(), 80, 24)
                .unwrap();
            std::fs::remove_dir(&*folder).unwrap();
            let reattached = terminals
                .open_session("removed-cwd", session, cwd.clone(), 100, 30)
                .unwrap();
            assert_eq!(first.id, reattached.id);
            terminals.close_all();
            assert!(
                terminals
                    .open_session("removed-cwd", session, cwd.clone(), 80, 24)
                    .is_err()
            );
            std::fs::create_dir(&*folder).unwrap();
        }
    }

    /// A worker's terminal runs its program directly with exactly the environment given; a
    /// reattach shows what it printed once; ending it kills its whole tree and waits.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_worker_terminal_runs_its_command_and_ends_with_its_tree() {
        if no_terminal_here() {
            return;
        }
        use brigadier_core::manager::TerminalHost;
        use brigadier_providers::TerminalCommand;
        use std::time::Duration;

        let data =
            Temp(std::env::temp_dir().join(format!("brig-terminals-{}", std::process::id())));
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(data.to_path_buf()),
        })
        .unwrap();
        let terminals = Terminals::with_platform(platform);
        let mut feed = terminals.subscribe();
        let command = TerminalCommand {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "echo \"[$WORKER_GRANT][$HOME]\"; sleep 600 & echo \"child:$!\"; wait".into(),
            ],
            cwd: std::env::temp_dir(),
            env: vec![("WORKER_GRANT".into(), "g-1".into())],
        };
        let started =
            TerminalHost::start(&terminals, "worker-test", "worker:t1", command, 80, 24).unwrap();
        assert!(started.pid.is_some());
        assert!(
            started.started_at_ms.is_some(),
            "its start time tells it apart"
        );
        let (info, child) = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (info, _) = terminals.attach(&started.id, &mut feed, 100, 30).unwrap();
                if let Some(child) = info
                    .scrollback
                    .split("child:")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|pid| pid.parse::<u32>().ok())
                {
                    return (info, child);
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the command printed");
        assert!(info.scrollback.contains("[g-1][]"), "{}", info.scrollback);
        assert_eq!(info.shell, "sh");
        // Nothing of it is left queued: the snapshot held it.
        assert!(feed.try_recv().is_err());

        TerminalHost::terminate(&terminals, &started.id).await;
        assert!(started.exited.await.is_ok(), "told of its end");
        let alive = std::process::Command::new("kill")
            .args(["-0", &child.to_string()])
            .status()
            .unwrap()
            .success();
        assert!(!alive, "its child went with it");
        // Ending one that already ended returns at once.
        tokio::time::timeout(
            Duration::from_secs(1),
            TerminalHost::terminate(&terminals, &started.id),
        )
        .await
        .unwrap();
    }

    /// A fake account CLI waits for its browser callback (a line on stdin), then writes its
    /// final file and exits. Its stored login starts signed in, as on "Sign in again".
    #[cfg(unix)]
    async fn account_sign_in_terminal() -> (Temp, Terminals, HostedTerminal, Arc<Terminal>) {
        let home = Temp(std::env::temp_dir().join(format!(
            "20261019-2257-onboarding-redesign-w08-account-{}",
            uuid::Uuid::new_v4()
        )));
        std::fs::create_dir_all(&*home).unwrap();
        let stored = home.join("auth.json");
        std::fs::write(&stored, "old login").unwrap();
        let mark = LoginMark::file(&stored);
        let before = mark.read().await;
        let terminals = Terminals::new();
        let hosted = terminals
            .start_command(
                "account:test",
                "account:test",
                TerminalCommand {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        "read callback; printf finished > finished".into(),
                    ],
                    cwd: home.to_path_buf(),
                    env: Vec::new(),
                },
                80,
                24,
            )
            .unwrap();
        terminals.watch_account_sign_in(&hosted.id, Some(mark), before, 100);
        let terminal = terminals.get(&hosted.id).unwrap();
        (home, terminals, hosted, terminal)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_sign_in_again_stops_the_cli_without_a_new_stored_login() {
        if no_terminal_here() {
            return;
        }
        // Even a new signed-in status cannot turn the account's unchanged old login into
        // completion of this attempt. A changed login also needs a fresh signed-in check.
        for (changed, checked_at, signed_in) in [
            (false, Some(99), true),
            (false, Some(101), true),
            (true, Some(99), true),
            (true, None, true),
            (true, Some(101), false),
        ] {
            let (home, terminals, hosted, terminal) = account_sign_in_terminal().await;
            if changed {
                std::fs::write(home.join("auth.json"), "new stored login").unwrap();
            }
            terminals
                .close_account_sign_in(&hosted.id, checked_at, signed_in, Duration::from_secs(20))
                .await;
            tokio::time::timeout(Duration::from_secs(2), hosted.exited)
                .await
                .expect("cancelled CLI stopped immediately, without its grace period")
                .unwrap();
            assert!(lock(&terminal.child).try_wait().unwrap().is_some());
            assert!(lock(&terminals.lingering).is_empty());
            assert!(
                !home.join("finished").exists(),
                "cancelled callback completed"
            );
            terminals.close_all();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_after_a_new_stored_login_lets_the_cli_finish_by_itself() {
        if no_terminal_here() {
            return;
        }
        let (home, terminals, hosted, terminal) = account_sign_in_terminal().await;
        std::fs::write(home.join("auth.json"), "new stored login").unwrap();
        terminals
            .close_account_sign_in(&hosted.id, Some(101), true, Duration::from_secs(20))
            .await;
        assert!(lock(&terminal.child).try_wait().unwrap().is_none());
        assert_eq!(terminals.count(), 0);
        assert!(terminals.running("account:test", "account:test").is_none());
        assert!(terminals.list("account:test").is_empty());
        assert!(terminals.output("account:test", &hosted.id).is_none());
        assert!(
            terminals
                .attach(&hosted.id, &mut terminals.subscribe(), 80, 24)
                .is_err()
        );
        assert!(lock(&terminals.lingering).contains_key(&hosted.id));
        // The hidden CLI can finish its own last writes and exit successfully.
        lock(&terminal.writer).write_all(b"callback\n").unwrap();
        tokio::time::timeout(Duration::from_secs(2), hosted.exited)
            .await
            .expect("completed CLI ended by itself")
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(home.join("finished")).unwrap(),
            "finished"
        );
        assert!(lock(&terminal.child).try_wait().unwrap().unwrap().success());
        assert!(lock(&terminals.lingering).is_empty());
        terminals.close_all();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn daemon_shutdown_stops_a_lingering_account_sign_in() {
        if no_terminal_here() {
            return;
        }
        let (home, terminals, hosted, terminal) = account_sign_in_terminal().await;
        std::fs::write(home.join("auth.json"), "new stored login").unwrap();
        terminals
            .close_account_sign_in(&hosted.id, Some(101), true, Duration::from_secs(20))
            .await;
        assert!(lock(&terminals.lingering).contains_key(&hosted.id));
        assert!(lock(&terminal.child).try_wait().unwrap().is_none());
        terminals.close_all();
        tokio::time::timeout(Duration::from_secs(2), hosted.exited)
            .await
            .expect("shutdown stopped the CLI without waiting for its grace period")
            .unwrap();
        assert!(lock(&terminal.child).try_wait().unwrap().is_some());
        assert!(lock(&terminals.lingering).is_empty());
        assert!(!home.join("finished").exists());
    }

    /// The user closing a worker's tab ends its whole tree too, a child that ignores the
    /// hang-up included.
    #[cfg(unix)]
    #[tokio::test]
    async fn closing_a_worker_tab_ends_its_tree() {
        if no_terminal_here() {
            return;
        }
        use brigadier_core::manager::TerminalHost;
        use brigadier_providers::TerminalCommand;
        use std::time::Duration;

        let data =
            Temp(std::env::temp_dir().join(format!("brig-terminals-close-{}", std::process::id())));
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(data.to_path_buf()),
        })
        .unwrap();
        let terminals = Terminals::with_platform(platform);
        let mut feed = terminals.subscribe();
        let command = TerminalCommand {
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "(trap '' HUP; exec sleep 600) & echo \"child:$!\"; wait".into(),
            ],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        };
        let started =
            TerminalHost::start(&terminals, "worker-test", "worker:t2", command, 80, 24).unwrap();
        let child = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let (info, _) = terminals.attach(&started.id, &mut feed, 100, 30).unwrap();
                if let Some(child) = info
                    .scrollback
                    .split("child:")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|pid| pid.parse::<u32>().ok())
                {
                    return child;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await
        .expect("the command printed");

        terminals.close(&started.id);
        tokio::time::timeout(Duration::from_secs(10), started.exited)
            .await
            .expect("told of its end")
            .unwrap();
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", &child.to_string()])
                .status()
                .unwrap()
                .success()
        };
        let gone = tokio::time::timeout(Duration::from_secs(5), async {
            while alive() {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
        .await;
        if gone.is_err() {
            let _ = std::process::Command::new("kill")
                .args(["-9", &child.to_string()])
                .status();
            panic!("its child outlived the closed tab");
        }
    }
}
