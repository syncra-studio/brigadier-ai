//! Bottom-pane shells, independently keyed within each conversation (or Home), on a
//! pseudo terminal. Its output streams live to the connections that opened it, and a tail is
//! kept so a tab opened again shows what came before; none of it is stored. A shell ends when
//! its tab closes, its conversation is archived or deleted, or the daemon quits.
//!
//! A worker's terminal ("Open in terminal") runs the worker's own CLI directly, not through a
//! shell, with exactly the environment the session manager gives it; the session manager hears
//! when it ends, and can end it and wait until its process is gone ([`TerminalHost`]).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use brigadier_core::Error;
use brigadier_core::manager::{HostedTerminal, TerminalHost};
use brigadier_ipc::protocol::{TerminalInfo, TerminalOutput};
use brigadier_providers::TerminalCommand;
use brigadier_sandbox::Platform;
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::{broadcast, oneshot, watch};

/// The output kept for a tab opened again, in bytes of text.
const SCROLLBACK: usize = 256 * 1024;
/// Output chunks a connection may fall behind by before it misses some.
const FEED: usize = 1024;
const READ_CHUNK: usize = 16 * 1024;
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
    feed: broadcast::Sender<TerminalOutput>,
    next: AtomicU64,
    /// Ends a worker terminal's process tree.
    platform: Option<Arc<dyn Platform>>,
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
            feed,
            next: AtomicU64::new(1),
            platform: None,
        }
    }

    /// Terminals that can end a worker terminal's whole process tree.
    pub fn with_platform(platform: Arc<dyn Platform>) -> Self {
        Self {
            platform: Some(platform),
            ..Self::new()
        }
    }

    /// Every terminal's output; a connection forwards the ones it opened.
    pub fn subscribe(&self) -> broadcast::Receiver<TerminalOutput> {
        self.feed.subscribe()
    }

    /// The conversation's running terminal resized to `cols` × `rows`, or a new shell in
    /// `cwd`: interactive, or running only the command `run` and ending with it.
    pub fn open(
        &self,
        conversation: &str,
        cwd: String,
        cols: u16,
        rows: u16,
        run: Option<&str>,
    ) -> Result<TerminalInfo> {
        self.open_inner(conversation, None, cwd, cols, rows, run)
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
        self.open_inner(conversation, session, cwd, cols, rows, None)
    }

    fn open_inner(
        &self,
        conversation: &str,
        session: Option<&str>,
        cwd: String,
        cols: u16,
        rows: u16,
        run: Option<&str>,
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

        let shell = default_shell();
        let mut command = CommandBuilder::new(&shell);
        if cfg!(unix) {
            // A login shell, so the user's PATH and profile apply as in their own terminal.
            command.arg("-l");
        }
        if let Some(run) = run {
            if cfg!(windows) {
                command.args(["-NoLogo", "-Command", run]);
            } else {
                command.args(["-c", run]);
            }
        }
        command.cwd(&cwd);
        // The daemon's own settings are not the user's.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("BRIGADIER_") {
                command.env_remove(key);
            }
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
        });
        lock(&self.live).insert(id.clone(), terminal.clone());
        let live = self.live.clone();
        let feed = self.feed.clone();
        let pump = terminal.clone();
        let started = std::thread::Builder::new()
            .name(format!("{id} output"))
            .spawn(move || pump_output(&pump, reader, &feed, &live));
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

    /// Ends a terminal's shell; closing one that already ended is fine.
    pub fn close(&self, id: &str) {
        let terminal = lock(&self.live).remove(id);
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
        let terminals: Vec<Arc<Terminal>> = lock(&self.live).drain().map(|(_, t)| t).collect();
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
    use super::Terminals;

    #[test]
    fn independent_sessions_reattach_and_close_with_their_conversation() {
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

    /// A worker's terminal runs its program directly with exactly the environment given; a
    /// reattach shows what it printed once; ending it kills its whole tree and waits.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_worker_terminal_runs_its_command_and_ends_with_its_tree() {
        use brigadier_core::manager::TerminalHost;
        use brigadier_providers::TerminalCommand;
        use std::time::Duration;

        let data = std::env::temp_dir().join(format!("brig-terminals-{}", std::process::id()));
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(data.clone()),
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
        let _ = std::fs::remove_dir_all(data);
    }

    /// The user closing a worker's tab ends its whole tree too, a child that ignores the
    /// hang-up included.
    #[cfg(unix)]
    #[tokio::test]
    async fn closing_a_worker_tab_ends_its_tree() {
        use brigadier_core::manager::TerminalHost;
        use brigadier_providers::TerminalCommand;
        use std::time::Duration;

        let data =
            std::env::temp_dir().join(format!("brig-terminals-close-{}", std::process::id()));
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(data.clone()),
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
        let _ = std::fs::remove_dir_all(data);
    }
}
