//! Brigadier's own plain commands (a worktree's setup script, a preview's helper): each runs
//! as it is, in its own process group, recorded under its owner in the cleanup ledger while it
//! runs, and is killed when it times out or its caller goes. Brigadier adds no sandbox: the
//! thread and workers run their commands with their CLI's own shell (G5).

use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::Artifact;
use brigadier_sandbox::Platform;
use tokio::io::AsyncReadExt;

use super::SessionManager;
use crate::{Error, Result};

/// How long a command runs when its caller doesn't say.
pub(crate) const RUN_TIMEOUT_DEFAULT: Duration = Duration::from_secs(600);
/// How long the output pipes may stay open after the command ended (something it started
/// elsewhere still holds them).
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// The shell commands run in.
pub(super) const SHELL: &str = "/bin/sh";

/// How a command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Ran {
    /// "exit 0", "exit 2", "killed by signal 9", "timed out after 600 s".
    pub status: String,
    /// Stdout and stderr together (stderr joined into stdout by the shell, so in order), at
    /// most [`super::OUTPUT_MAX_BYTES`].
    pub output: Vec<u8>,
}

impl SessionManager {
    /// What runs `command` through the shell in `workdir`, as it is.
    pub(super) fn run_spec(&self, workdir: &Path, command: &str) -> brigadier_sandbox::SpawnSpec {
        let shell = shell_command(command);
        let mut spec = self.runtime.cli_env().spec(Path::new(SHELL));
        spec.args = shell[1..].iter().map(Into::into).collect();
        spec.cwd = Some(workdir.to_owned());
        spec
    }
}

/// The shell command line for `command`, its stderr joined into its stdout so the output
/// keeps its order.
pub(super) fn shell_command(command: &str) -> Vec<String> {
    vec![
        SHELL.to_owned(),
        "-c".to_owned(),
        format!("exec 2>&1\n{command}"),
    ]
}

/// Kills a command's process tree unless it was reaped: a command whose caller is gone goes
/// with it.
struct Running {
    platform: Arc<dyn Platform>,
    pid: u32,
    reaped: bool,
}

impl Drop for Running {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.platform.processes().kill_tree(self.pid);
        }
    }
}

/// Tests: the next run by each owner here times out only once its gate is notified too, so a
/// test sees what the command did before its timeout, however slow the machine.
#[cfg(test)]
static TIMEOUT_GATES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Notify>>>,
> = std::sync::LazyLock::new(Default::default);

/// Tests: holds the timeout of `owner`'s next run until `gate` is notified.
#[cfg(all(test, unix))]
pub(crate) fn hold_timeout(owner: &str, gate: Arc<tokio::sync::Notify>) {
    TIMEOUT_GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(owner.to_owned(), gate);
}

/// Runs `spec` to its end, `timeout`, or `ended` (its caller went), whichever comes first,
/// recorded under `owner` in the cleanup ledger while it runs.
#[cfg(all(test, unix))]
pub(crate) async fn run_command(
    platform: Arc<dyn Platform>,
    spec: &brigadier_sandbox::SpawnSpec,
    timeout: Duration,
    ended: tokio_util::sync::CancellationToken,
    owner: &str,
    ledger: &Arc<crate::ledger::CleanupLedger>,
) -> Result<Ran> {
    run_command_streamed(platform, spec, timeout, ended, owner, ledger, None).await
}

/// A pipe chunk delivered to its caller as it comes (a setup script's live output).
pub(super) type CommandOutput =
    Arc<dyn Fn(Vec<u8>) -> brigadier_providers::BoxFuture<'static, ()> + Send + Sync>;

/// The command runner with an optional observer of its pipes. Reading and final output
/// retention still use one runner.
pub(super) async fn run_command_streamed(
    platform: Arc<dyn Platform>,
    spec: &brigadier_sandbox::SpawnSpec,
    timeout: Duration,
    ended: tokio_util::sync::CancellationToken,
    owner: &str,
    ledger: &Arc<crate::ledger::CleanupLedger>,
    output: Option<CommandOutput>,
) -> Result<Ran> {
    let mut command = tokio::process::Command::from(platform.processes().piped_command(spec));
    command.stdin(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|err| Error::Invalid(format!("could not start the command: {err}")))?;
    let pid = child
        .id()
        .ok_or_else(|| Error::Invalid("the command ended before it started".into()))?;
    let mut running = Running {
        platform: platform.clone(),
        pid,
        reaped: false,
    };
    let artifact = Artifact::Process {
        pid,
        started_at_ms: platform.processes().start_time_ms(pid).ok(),
    };
    if let Err(err) = ledger.record(owner, artifact.clone()).await {
        tracing::warn!(owner, error = %err, "could not record a run command");
    }
    let stdout = child
        .stdout
        .take()
        .map(|pipe| read_capped(pipe, output.clone()));
    let stderr = child.stderr.take().map(|pipe| read_capped(pipe, output));
    let stopped = |why: String| {
        let _ = platform.processes().kill_tree(pid);
        why
    };
    #[cfg(test)]
    let gate = TIMEOUT_GATES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .remove(owner);
    let timed_out = async {
        tokio::time::sleep(timeout).await;
        #[cfg(test)]
        if let Some(gate) = gate {
            gate.notified().await;
        }
    };
    let status = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => exit_status(status),
            Err(err) => stopped(format!("lost the command: {err}")),
        },
        () = timed_out => {
            stopped(format!("timed out after {} s", timeout.as_secs()))
        }
        () = ended.cancelled() => stopped("stopped: its caller went away".into()),
    };
    // Reaped (or killed and reaped now); what it left in its group goes too.
    let _ = child.wait().await;
    running.reaped = true;
    let _ = platform.processes().kill_group(pid);
    let mut output = drain(stdout).await;
    let stderr = drain(stderr).await;
    if !stderr.is_empty() {
        // What the shell itself wrote (the command's stderr is in stdout).
        if !output.is_empty() && !output.ends_with(b"\n") {
            output.push(b'\n');
        }
        output.extend_from_slice(&stderr);
    }
    ledger.forget(owner, artifact).await;
    Ok(Ran { status, output })
}

/// What a pipe's reader has kept so far, and how many bytes past the cap it dropped.
type Kept = Arc<std::sync::Mutex<(Vec<u8>, u64)>>;

/// Reads a pipe to its end, keeping at most [`super::OUTPUT_MAX_BYTES`].
fn read_capped(
    mut pipe: impl tokio::io::AsyncRead + Unpin + Send + 'static,
    output: Option<CommandOutput>,
) -> (Kept, tokio::task::JoinHandle<()>) {
    let kept = Kept::default();
    let into = kept.clone();
    let reader = tokio::spawn(async move {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    {
                        let mut kept = into.lock().unwrap_or_else(|p| p.into_inner());
                        let room = super::OUTPUT_MAX_BYTES.saturating_sub(kept.0.len());
                        let take = read.min(room);
                        kept.0.extend_from_slice(&buffer[..take]);
                        kept.1 += (read - take) as u64;
                    }
                    if let Some(output) = &output {
                        output(buffer[..read].to_vec()).await;
                    }
                }
            }
        }
    });
    (kept, reader)
}

/// What a reader kept, once its pipe closed or [`DRAIN_GRACE`] passed (something the command
/// started elsewhere may hold the pipe open).
async fn drain(reader: Option<(Kept, tokio::task::JoinHandle<()>)>) -> Vec<u8> {
    let Some((kept, mut reader)) = reader else {
        return Vec::new();
    };
    if tokio::time::timeout(DRAIN_GRACE, &mut reader)
        .await
        .is_err()
    {
        reader.abort();
    }
    let (mut bytes, dropped) = std::mem::take(&mut *kept.lock().unwrap_or_else(|p| p.into_inner()));
    if dropped > 0 {
        bytes.extend_from_slice(format!("\n[… {dropped} more bytes not kept]\n").as_bytes());
    }
    bytes
}

pub(super) fn exit_status(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("killed by signal {signal}");
        }
    }
    "ended".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_joins_stdout_in_the_shell() {
        assert_eq!(
            shell_command("make test"),
            ["/bin/sh", "-c", "exec 2>&1\nmake test"]
        );
    }
}
