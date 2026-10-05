//! Bottom-pane shells, independently keyed within each session's checkout, on a
//! pseudo terminal. Its output streams live to the connections that opened it, and a tail is
//! kept so a tab opened again shows what came before; none of it is stored. A shell ends when
//! its tab closes, its session is archived or deleted, or the daemon quits.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use brigadier_core::Error;
use brigadier_ipc::protocol::{TerminalInfo, TerminalOutput};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::broadcast;

/// The output kept for a tab opened again, in bytes of text.
const SCROLLBACK: usize = 256 * 1024;
/// Output chunks a connection may fall behind by before it misses some.
const FEED: usize = 1024;
const READ_CHUNK: usize = 16 * 1024;
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
    scrollback: Mutex<String>,
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
}

impl Terminals {
    pub fn new() -> Self {
        let (feed, _) = broadcast::channel(FEED);
        Self {
            live: Arc::new(Mutex::new(HashMap::new())),
            feed,
            next: AtomicU64::new(1),
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

        let failed =
            |err: anyhow::Error| Error::Invalid(format!("couldn't start a terminal: {err}"));
        let pair = native_pty_system().openpty(size).map_err(failed)?;
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
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(command).map_err(failed)?;
        drop(pair.slave);
        let reader = pair.master.try_clone_reader().map_err(failed)?;
        let writer = pair.master.take_writer().map_err(failed)?;

        let id = format!("terminal-{}", self.next.fetch_add(1, Ordering::Relaxed));
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
        // A new shell's first output already streams to the connection that subscribed
        // before opening it; sending it here too would show it twice.
        Ok(TerminalInfo {
            scrollback: String::new(),
            ..terminal.info()
        })
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

    /// How many terminals run.
    pub fn count(&self) -> usize {
        lock(&self.live).len()
    }

    /// Ends a terminal's shell; closing one that already ended is fine.
    pub fn close(&self, id: &str) {
        let terminal = lock(&self.live).remove(id);
        if let Some(terminal) = terminal {
            terminal.kill();
        }
    }

    /// Ends the conversation's terminal, if one runs.
    pub fn close_conversation(&self, conversation: &str) {
        let ids: Vec<String> = lock(&self.live)
            .values()
            .filter(|terminal| terminal.conversation == conversation)
            .map(|terminal| terminal.id.clone())
            .collect();
        for id in ids {
            self.close(&id);
        }
    }

    /// Ends every shell (the daemon is quitting).
    pub fn close_all(&self) {
        let terminals: Vec<Arc<Terminal>> = lock(&self.live).drain().map(|(_, t)| t).collect();
        for terminal in terminals {
            terminal.kill();
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
        {
            let mut scrollback = lock(&terminal.scrollback);
            scrollback.push_str(&data);
            if scrollback.len() > SCROLLBACK {
                let mut cut = scrollback.len() - SCROLLBACK;
                while !scrollback.is_char_boundary(cut) {
                    cut += 1;
                }
                scrollback.drain(..cut);
            }
        }
        // No receiver just means no tab is open.
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
}
