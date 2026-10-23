//! Brigadier's terminals, as the daemon runs them: the session's shells (`terminal_run`) and
//! what `app_state` lists of them.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use brigadier_providers::TerminalCommand;
use tokio::sync::oneshot;

use super::SessionManager;
use crate::{Error, Result};

/// Where Brigadier's terminals run (the daemon's pseudo terminals).
pub trait TerminalHost: Send + Sync {
    /// Starts `command` in a new terminal of `conversation` under `key`, sized
    /// `cols` × `rows`; the program runs directly, not through a shell.
    fn start(
        &self,
        conversation: &str,
        key: &str,
        command: TerminalCommand,
        cols: u16,
        rows: u16,
    ) -> Result<HostedTerminal>;

    /// Ends the terminal's process with its tree and resolves once it has exited (at once
    /// when it already has).
    fn terminate(&self, id: &str) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

    /// `conversation`'s terminals now, oldest first (Q90's `app_state`).
    fn list(&self, _conversation: &str) -> Vec<TerminalSnapshot> {
        Vec::new()
    }

    /// Everything terminal `id` kept, if it is one of `conversation`'s (`read_terminal`).
    fn output(&self, _conversation: &str, _id: &str) -> Option<String> {
        None
    }

    /// The shell of `conversation`'s tab `key`, started in `cwd` unless it runs already
    /// (`terminal_run`): its id. The tab attaches to it when it shows.
    fn open_shell(&self, _conversation: &str, _key: &str, _cwd: &str) -> Result<String> {
        Err(Error::Invalid("terminals aren't available here".into()))
    }

    /// Types `data` into terminal `id`, if it is one of `conversation`'s (`terminal_run`).
    fn type_in(&self, _conversation: &str, _id: &str, _data: &str) -> Result<()> {
        Err(Error::Invalid("terminals aren't available here".into()))
    }
}

/// One of a conversation's terminals, as [`TerminalHost::list`] gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSnapshot {
    pub id: String,
    /// The tab it belongs to (`terminal:agent-…` for one an agent opened; `worker:…` for a
    /// worker's session opened in it); none for the original shell.
    pub key: Option<String>,
    pub cwd: String,
    /// Whether a command runs in front of its shell: the terminal's foreground process group
    /// isn't the shell's own. `None` where that can't be known.
    pub busy: Option<bool>,
    /// The end of what it kept, escape codes and all.
    pub tail: String,
}

/// A terminal a [`TerminalHost`] started.
pub struct HostedTerminal {
    pub id: String,
    pub pid: Option<u32>,
    /// The process's start time, which tells it apart from a later one with the same pid.
    pub started_at_ms: Option<f64>,
    /// Resolves when its process has ended.
    pub exited: oneshot::Receiver<()>,
}

impl SessionManager {
    /// Where terminals run; the daemon sets it once at start.
    pub fn set_terminal_host(&self, host: Arc<dyn TerminalHost>) {
        let _ = self.terminal_host.set(host);
    }
}
