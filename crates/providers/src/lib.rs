//! Provider adapters: how Brigadier drives the CLIs installed on the machine.
//!
//! Every CLI sits behind [`Provider`] (discovery, auth, models, quota, starting sessions) and
//! [`ProviderSession`] (turns, steering, interrupts, approvals, shutdown). Sessions report what
//! happens as [`ProviderEvent`]s in one normalized vocabulary, whatever the CLI speaks.
//!
//! - [`claude`]: the user's own `claude` binary over stream-json stdio. Never the Agent SDK, and
//!   Brigadier never touches its OAuth tokens.
//! - [`codex`]: `codex app-server` over stdio JSON-RPC, with bindings generated from the
//!   installed version's schema.
//!
//! Whatever a session creates on disk or in the process table is reported to a [`Ledger`] the
//! moment it is known, so it can be removed exactly when the session is disposed of, or by the
//! crash sweep after the daemon died.

pub mod accounts;
#[cfg(all(test, unix))]
mod chat_shell_live_tests;
pub mod claude;
pub mod cli;
pub mod codex;
mod events;
pub mod fixtures;
pub mod history;
pub mod leftovers;
mod logging;
pub mod login_mark;
mod looked;
pub mod model;
pub mod policy;
pub mod process;
pub mod record;
pub mod redact;
pub mod session_file;
pub mod simulate;
mod time;
pub mod trust;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;

pub use looked::only_looks;
pub use model::*;
pub use redact::{REDACTED, Redactor, env_file_values};

/// A boxed future, so the traits stay object-safe.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0} is not installed (not found on the login shell's PATH)")]
    NotInstalled(ProviderKind),
    #[error("could not start the CLI: {0}")]
    Spawn(String),
    #[error("{0}")]
    Protocol(String),
    /// The CLI answered a request with an error.
    #[error("{0}")]
    Rejected(String),
    #[error("{0}")]
    Invalid(String),
    #[error("the session has ended")]
    Closed,
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("the cleanup ledger could not record an artifact: {0}")]
    Ledger(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Platform(#[from] brigadier_sandbox::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Records what a session creates, durably, before the session relies on it.
pub trait Ledger: Send + Sync {
    /// This session belongs to a tab: no dynamic diagnostics may reach machine logs.
    fn private_logs(&self) -> bool {
        false
    }

    fn record(&self, artifact: Artifact) -> BoxFuture<'_, Result<()>>;

    /// Whether any session's ledger still holds `artifact`, i.e. Brigadier created it and it
    /// has not been removed yet. A session sharing such a directory records it too, so the
    /// last one disposed of removes it.
    fn holds(&self, artifact: &Artifact) -> bool;
}

/// A CLI Brigadier can drive.
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Whether the CLI is installed and logged in, with guidance when it is not.
    fn status(&self) -> BoxFuture<'_, ProviderStatus>;

    /// The models the CLI offers right now.
    fn models(&self) -> BoxFuture<'_, Result<ModelCatalog>>;

    /// Remaining quota in each usage window.
    fn quota(&self) -> BoxFuture<'_, Result<QuotaSnapshot>>;

    /// Starts, resumes or forks a session. Everything it creates is recorded in `ledger`
    /// first; events stream on the returned receiver until the CLI exits, and
    /// [`ProviderEvent::Exited`] is always the last one.
    fn start(&self, spec: SessionSpec, ledger: Arc<dyn Ledger>) -> BoxFuture<'_, Result<Started>>;

    /// Removes the CLI-side artifacts of a finished session (transcripts, thread records).
    /// Processes are the caller's: see [`cleanup::end_process`]. Only what is listed is touched.
    fn remove(&self, artifacts: Vec<Artifact>) -> BoxFuture<'_, Result<()>>;

    /// A tab's cleanup must also suppress identifying diagnostics from control processes.
    fn remove_with_privacy(
        &self,
        artifacts: Vec<Artifact>,
        _private_logs: bool,
    ) -> BoxFuture<'_, Result<()>> {
        self.remove(artifacts)
    }

    /// A fresh parser over the CLI's raw output, for replaying recordings.
    fn replayer(&self) -> Box<dyn Replayer>;

    /// The command that continues `spec`'s session (an [`Origin::Resume`]) interactively in a
    /// terminal in `cwd`, with the same flags as its headless process. Nothing may run the
    /// session meanwhile: Claude has no lock of its own. A Codex thread is unarchived first.
    fn terminal(&self, spec: SessionSpec, cwd: PathBuf) -> BoxFuture<'_, Result<TerminalCommand>> {
        let _ = (spec, cwd);
        Box::pin(async { Err(Error::Invalid("this CLI cannot open in a terminal".into())) })
    }

    /// The folders the user's own sessions of this CLI ran in, most recent first, from the
    /// CLI's session files (read, never changed).
    fn past_folders(&self) -> BoxFuture<'_, Vec<history::PastFolder>> {
        Box::pin(async { Vec::new() })
    }

    /// Whether session `native_id` took `needle` as input, by its transcript: in a message
    /// sent or steered in, never in a tool's output or the model's own words (read, never
    /// changed).
    fn took_input(&self, native_id: &str, needle: &str) -> BoxFuture<'_, bool> {
        let _ = (native_id, needle);
        Box::pin(async { false })
    }
}

/// A CLI session continued in a terminal: the resolved binary (never a name looked up on a
/// terminal's PATH, where a wrapper may stand in), its arguments, and exactly its environment.
#[derive(Debug, Clone)]
pub struct TerminalCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
}

/// A started session and its event stream.
pub struct Started {
    pub session: Arc<dyn ProviderSession>,
    pub events: mpsc::Receiver<ProviderEvent>,
}

/// A live CLI session.
pub trait ProviderSession: Send + Sync {
    /// The CLI's own id for the session (Claude session id, Codex thread id).
    fn native_id(&self) -> String;

    /// Starts a turn with the user's message.
    fn send(&self, input: TurnInput) -> BoxFuture<'_, Result<()>>;

    /// Adds a message to the turn that is running now.
    fn steer(&self, input: TurnInput) -> BoxFuture<'_, Result<()>>;

    /// Stops the running turn.
    fn interrupt(&self) -> BoxFuture<'_, Result<()>>;

    /// Whether this CLI's version can [`compact`](Self::compact) its conversation.
    fn can_compact(&self) -> bool {
        false
    }

    /// Compacts the conversation now, in a turn of its own that replies nothing: it reports
    /// [`ProviderEvent::CompactionStarted`], then [`ProviderEvent::CompactionEnded`].
    fn compact(&self) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err(Error::Invalid("this CLI cannot compact its context".into())) })
    }

    /// Stops one of the CLI's background tasks ([`ProviderEvent::BackgroundTask`]) by its id.
    fn stop_task(&self, _task_id: String) -> BoxFuture<'_, Result<()>> {
        Box::pin(async {
            Err(Error::Invalid(
                "this CLI cannot stop a background task".into(),
            ))
        })
    }

    /// Answers an [`ProviderEvent::ApprovalRequested`].
    fn answer(&self, approval_id: String, decision: ApprovalDecision) -> BoxFuture<'_, Result<()>>;

    /// Ends the CLI process and reaps it, killing its process group if it does not exit in
    /// time. Bounded; safe to call more than once.
    fn close(&self) -> BoxFuture<'_, ()>;

    fn is_running(&self) -> bool;
}

/// Turns a recorded session back into events.
pub trait Replayer: Send {
    /// Takes recorded lines in both directions: some output only reads right knowing what
    /// Brigadier sent (Claude reports an interrupted turn as an error).
    fn feed(&mut self, dir: record::Direction, line: &str) -> Vec<ProviderEvent>;
}

pub mod cleanup {
    //! Ending processes recorded in a cleanup ledger.

    use brigadier_sandbox::Platform;

    /// Kills a recorded CLI process group, if that process is still the one recorded (its
    /// start time matches, so a reused pid is never touched). Returns whether it was running.
    pub fn end_process(platform: &dyn Platform, pid: u32, started_at_ms: Option<f64>) -> bool {
        let processes = platform.processes();
        if !processes.is_alive(pid) {
            return false;
        }
        if let Some(recorded) = started_at_ms {
            match processes.start_time_ms(pid) {
                // Start times are reported to the millisecond or better.
                Ok(actual) if (actual - recorded).abs() < 1_000.0 => {}
                _ => return false,
            }
        }
        if let Err(err) = processes.kill_tree(pid) {
            tracing::warn!(pid, error = %err, "could not end a recorded CLI process");
        }
        true
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

/// A tool input or result as an event shows it: a result is cut on the way out, once it is
/// redacted and kept whole (`events::Events::send`, [`KeptOutput`]).
pub const OUTPUT_CLIP: usize = 8 * 1024;
/// The most of one result kept: what a command prints beyond it is counted, not kept.
pub const OUTPUT_KEEP_MAX: usize = 64 * 1024 * 1024;

/// The file a Claude tool result's preview says the whole output was saved to (its
/// `tool-results/`), if it names one.
pub fn saved_output_file(text: &str) -> Option<String> {
    claude::parse::saved_file(text)
}

/// A file change's diff as shown: clipped on the way out, after it is redacted
/// (`events::Events::send`), so a secret that crosses the cut is still recognised whole and never
/// kept in part.
pub(crate) const DIFF_CLIP: usize = 8 * 1024;
/// How much of a diff a parser keeps for redaction to see, well past [`DIFF_CLIP`]: a secret
/// that crosses this cut lies in the part [`DIFF_CLIP`] drops.
pub(crate) const DIFF_RAW_CLIP: usize = 4 * DIFF_CLIP;

/// `text` cut to at most `max` bytes on a character boundary, marked when cut.
pub(crate) fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} more bytes]", &text[..end], text.len() - end)
}

/// Encodes an image's bytes for the provider and IPC media envelopes.
pub fn image_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
