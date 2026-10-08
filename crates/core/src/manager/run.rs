//! A thread's `run` and `run_unsandboxed` tools (THREAD-PLAN.md Q4; a Claude thread's too,
//! the user's ruling of 2026-10-07). A Claude thread's `run` is sandboxed by Brigadier's own
//! Seatbelt profile with the thread's folders ([`SessionManager::seatbelt_spec`]); under
//! Approve for me its `run_unsandboxed` is decided by Brigadier's reviewer
//! (`super::escalation`), under Ask for approval by the user, as Codex's is.
//!
//! Codex's hooks stay off, so its built-in shell's output reaches the model untrimmed. `run`
//! is the shell the thread uses for builds, tests and logs instead: Brigadier owns the command,
//! keeps its whole output (stdout and stderr together, in order) and returns the digest of any
//! result over [`TRIM_ABOVE`] bytes, a failure's too.
//!
//! It keeps the thread's access exactly (docs/evidence/2026-10-07-thread-phase2-contracts.md
//! §5): at Full access the command runs as it is; otherwise through `codex sandbox` under the
//! permission profile the thread's own session has, built by the same code
//! ([`brigadier_providers::codex::sandbox_args`]).
//!
//! Leaving the sandbox is `run_unsandboxed`, under Ask for approval only. Codex asks about each
//! call first (its per-tool `approval_mode` is `prompt`), which reaches the user as a card; and
//! the call runs only a command approved that way, once ([`RunPasses`]). The tool's grant alone
//! is not enough: a sandboxed command can read another process's environment (`sysctl
//! KERN_PROCARGS2` is allowed in Codex's sandbox, checked live on 0.160.1) and so the grant of
//! the thread's MCP bridge, and it may reach the daemon's socket. Under Approve for me Codex's
//! auto-reviewer settles such a call inside Codex, with nothing Brigadier could tie the call to,
//! so that level has no `run_unsandboxed`: the thread widens the sandbox from its own shell
//! (`with_additional_permissions`, never unsandboxed, see the Codex provider), which the
//! auto-reviewer settles (its output untrimmed, as all of that shell's is).
//!
//! Every command leads its own process group, recorded under the thread's cleanup owner while
//! it runs, and is killed when it times out, when its call is abandoned and when the thread's
//! CLI ends (hibernation and stop end the owner's processes too). It is short-lived: what it
//! leaves running in its group is ended with it (a preview is the way to keep a server).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::{Access, Artifact, ProviderKind};
use brigadier_sandbox::Platform;
use tokio::io::AsyncReadExt;

use super::SessionManager;
use crate::digest::TRIM_ABOVE;
use crate::model::{ConversationId, PermissionLevel};
use crate::tools::RunTools;
use crate::work::OutputSource;
use crate::{Error, Result};

/// How long a command runs when the call doesn't say.
pub(crate) const RUN_TIMEOUT_DEFAULT: Duration = Duration::from_secs(600);
/// The longest a command may run (the thread's MCP calls wait as long).
pub(crate) const RUN_TIMEOUT_MAX: Duration = Duration::from_secs(1800);
/// How long the output pipes may stay open after the command ended (something it started
/// elsewhere still holds them).
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// The shell commands run in.
const SHELL: &str = "/bin/sh";

/// How long an approved `run_unsandboxed` command waits for its call.
const PASS_TTL: Duration = Duration::from_secs(120);

/// The command tools a thread on `provider` at `permission` has. Both vendors run builds,
/// tests, logs and long listings through `run`. A Codex thread leaves its sandbox through
/// `run_unsandboxed` under Ask for approval only (under Approve for me its own shell asks
/// Codex's auto-reviewer); a Claude thread has it at both sandboxed levels: Claude's auto mode
/// declines every command that leaves its sandbox, so under Approve for me Brigadier's
/// reviewer decides (`super::escalation`). Full access has no sandbox to leave. Not on
/// Windows, which has no `/bin/sh` and no sandbox `run` can keep.
pub(crate) fn run_tools(provider: ProviderKind, permission: PermissionLevel) -> RunTools {
    match (provider, permission) {
        _ if cfg!(windows) => RunTools::None,
        (_, PermissionLevel::FullAccess) => RunTools::Run,
        (_, PermissionLevel::AskForApproval) => RunTools::WithEscalation,
        (ProviderKind::Claude, PermissionLevel::ApproveForMe) => RunTools::WithEscalation,
        (ProviderKind::Codex, PermissionLevel::ApproveForMe) => RunTools::Run,
    }
}

/// The `run_unsandboxed` commands approved and not yet run, per conversation: each runs once,
/// in the working directory it was approved for, within [`PASS_TTL`] of its approval.
#[derive(Default)]
pub(crate) struct RunPasses {
    inner: std::sync::Mutex<std::collections::HashMap<ConversationId, Vec<Pass>>>,
}

/// One approved command: the command line, its `workdir` argument as approved, and when.
type Pass = (String, Option<String>, std::time::Instant);

impl RunPasses {
    /// `command` was approved for the thread of `id`, to run in `workdir` (the call's own
    /// argument, `None` for the default).
    pub(crate) fn grant(&self, id: &ConversationId, command: &str, workdir: Option<&str>) {
        let mut passes = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        passes.entry(id.clone()).or_default().push((
            command.to_owned(),
            workdir.map(str::to_owned),
            std::time::Instant::now(),
        ));
    }

    /// Takes the approval of `command` in `workdir` for the thread of `id`, if it has a fresh
    /// one: the same command in another directory can run another script.
    pub(crate) fn take(&self, id: &ConversationId, command: &str, workdir: Option<&str>) -> bool {
        let mut passes = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let Some(list) = passes.get_mut(id) else {
            return false;
        };
        list.retain(|(_, _, at)| at.elapsed() < PASS_TTL);
        let found = list
            .iter()
            .position(|(approved, dir, _)| approved == command && dir.as_deref() == workdir);
        if let Some(index) = found {
            list.remove(index);
        }
        if list.is_empty() {
            passes.remove(id);
        }
        found.is_some()
    }
}

/// The tool name of a thread's escalated command, as its approval requests carry it.
pub(crate) const RUN_UNSANDBOXED: &str = "run_unsandboxed";

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
    /// `run` (`unsandboxed`: `run_unsandboxed`, which Codex let through): runs `command` for
    /// the thread of `id` and returns what the model gets.
    pub(crate) async fn run_tool(
        &self,
        id: &ConversationId,
        command: &str,
        workdir: Option<&str>,
        timeout_secs: Option<u64>,
        unsandboxed: bool,
    ) -> Result<String> {
        if command.trim().is_empty() {
            return Err(Error::Invalid("`command` is empty".into()));
        }
        let cli = self
            .conv(id)?
            .live_cli()
            .await
            .ok_or_else(|| Error::Invalid("the session's thread has ended".into()))?;
        // A Claude thread at Approve for me asks Brigadier's reviewer once its folder is known.
        let reviewed = unsandboxed
            && cli.provider == ProviderKind::Claude
            && self.permission(id) == PermissionLevel::ApproveForMe;
        if unsandboxed && !reviewed && !self.run_passes.take(id, command, workdir) {
            return Err(Error::Invalid(
                "run_unsandboxed runs only a command the user approved when it was asked, \
                 once; nothing approved this one"
                    .into(),
            ));
        }
        let launch = cli
            .launch
            .as_ref()
            .ok_or_else(|| Error::Invalid("only a session's thread runs commands".into()))?;
        let scratch = self.owned_dir("orch", &id.0);
        let workspace = launch
            .workspace
            .as_ref()
            .map(|workspace| workspace.path.clone());
        let workdir = run_workdir(workdir, workspace.as_deref(), &scratch)?;
        if reviewed {
            self.decide_escalation(id, command, &workdir, workspace.as_deref())
                .await?;
        }
        let timeout = timeout_secs
            .map_or(RUN_TIMEOUT_DEFAULT, Duration::from_secs)
            .clamp(Duration::from_secs(1), RUN_TIMEOUT_MAX);
        let access = if unsandboxed {
            Access::Full
        } else {
            launch.access.clone()
        };
        // A Claude thread's sandbox is Brigadier's own Seatbelt profile with its folders;
        // Codex's is `codex sandbox` with its session's profile.
        let mut spec = match cli.provider {
            ProviderKind::Claude if access != Access::Full => {
                self.seatbelt_spec(&access, &workdir, &shell_command(command)[1..])?
            }
            _ => self.run_spec(&access, &workdir, command)?,
        };
        brigadier_providers::cli::apply_session_env(
            &mut spec,
            &SessionManager::thread_env(&scratch, cli.provider),
            &[],
        );
        let ran = run_command(
            self.runtime.platform().clone(),
            &spec,
            timeout,
            cli.ended.clone(),
            &cli.owner,
            self.runtime.ledger(),
        )
        .await?;
        if ran.output.len() <= TRIM_ABOVE {
            let output = self.hide_secrets(id, ran.output).await;
            return Ok(format!(
                "[{}]\n{}",
                ran.status,
                String::from_utf8_lossy(&output)
            ));
        }
        let (_, digest) = self
            .store_output(id, OutputSource::Run, &ran.status, ran.output, true)
            .await?;
        Ok(digest)
    }

    /// What runs `command` in `workdir` held to `access`: the shell itself at full access,
    /// else `codex sandbox` with the session's profile around it.
    pub(super) fn run_spec(
        &self,
        access: &Access,
        workdir: &Path,
        command: &str,
    ) -> Result<brigadier_sandbox::SpawnSpec> {
        let shell = shell_command(command);
        let env = self.runtime.cli_env();
        let mut spec = match brigadier_providers::codex::sandbox_args(access, workdir, &shell) {
            None if *access == Access::Full => {
                let mut spec = env.spec(Path::new(SHELL));
                spec.args = shell[1..].iter().map(Into::into).collect();
                spec
            }
            None => {
                return Err(Error::Invalid(
                    "this session's access has no sandbox run can keep".into(),
                ));
            }
            Some(args) => {
                let codex = env
                    .resolve(ProviderKind::Codex)
                    .ok_or_else(|| Error::Invalid("Codex is not installed".into()))?;
                let mut spec = env.spec(&codex);
                spec.args = args.into_iter().map(Into::into).collect();
                spec
            }
        };
        spec.cwd = Some(workdir.to_owned());
        Ok(spec)
    }

    /// What runs the shell with `shell_args` in `workdir` held to `access` under Brigadier's
    /// own Seatbelt profile (where Codex's sandbox isn't the one to keep): the access's
    /// writable folders (`workdir` among them when the access writes its working directory),
    /// network and denied reads. Full access needs no sandbox: [`Self::run_spec`] runs it.
    pub(super) fn seatbelt_spec(
        &self,
        access: &Access,
        workdir: &Path,
        shell_args: &[String],
    ) -> Result<brigadier_sandbox::SpawnSpec> {
        let mut spec = self.runtime.cli_env().spec(Path::new(SHELL));
        spec.args = shell_args.iter().map(Into::into).collect();
        spec.cwd = Some(workdir.to_owned());
        let (writable_roots, network, deny_read) = match access {
            Access::Scoped {
                write_cwd,
                writable_roots,
                network,
                deny_read,
                ..
            } => {
                let mut roots = writable_roots.clone();
                if *write_cwd {
                    roots.push(workdir.to_owned());
                }
                (roots, *network, deny_read.clone())
            }
            Access::Workspace { extra_roots } => {
                let mut roots = extra_roots.clone();
                roots.push(workdir.to_owned());
                (roots, true, Vec::new())
            }
            Access::ReadOnly | Access::Full => (Vec::new(), false, Vec::new()),
        };
        let writable_roots = writable_roots
            .into_iter()
            .filter(|root| root.exists())
            .collect();
        self.runtime
            .platform()
            .sandbox()
            .confine(
                spec,
                &brigadier_sandbox::SandboxPolicy {
                    writable_roots,
                    network,
                    deny_read,
                },
            )
            .map_err(|err| Error::Invalid(err.to_string()))
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

/// The folder a command runs in: the workspace by default, else `requested` (relative to the
/// workspace), which must be inside the workspace or the thread's scratch folder.
pub(super) fn run_workdir(
    requested: Option<&str>,
    workspace: Option<&Path>,
    scratch: &Path,
) -> Result<PathBuf> {
    let base = workspace.unwrap_or(scratch);
    let wanted = match requested.map(str::trim).filter(|dir| !dir.is_empty()) {
        None => base.to_owned(),
        Some(dir) if Path::new(dir).is_absolute() => PathBuf::from(dir),
        Some(dir) => base.join(dir),
    };
    let real = wanted
        .canonicalize()
        .map_err(|err| Error::Invalid(format!("workdir {}: {err}", wanted.display())))?;
    let inside = |root: &Path| root.canonicalize().is_ok_and(|root| real.starts_with(root));
    if workspace.is_some_and(inside) || inside(scratch) {
        Ok(real)
    } else {
        Err(Error::Invalid(format!(
            "workdir {} is outside the workspace and your scratch folder",
            wanted.display()
        )))
    }
}

/// Kills a command's process tree unless it was reaped: a `run` call that is abandoned (its
/// MCP call cancelled) takes its command with it.
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

/// Runs `spec` to its end, `timeout`, or `ended` (the thread's CLI ended), whichever comes
/// first, recorded under `owner` in the cleanup ledger while it runs.
pub(crate) async fn run_command(
    platform: Arc<dyn Platform>,
    spec: &brigadier_sandbox::SpawnSpec,
    timeout: Duration,
    ended: tokio_util::sync::CancellationToken,
    owner: &str,
    ledger: &Arc<crate::ledger::CleanupLedger>,
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
    let stdout = child.stdout.take().map(read_capped);
    let stderr = child.stderr.take().map(read_capped);
    let stopped = |why: String| {
        let _ = platform.processes().kill_tree(pid);
        why
    };
    let status = tokio::select! {
        status = child.wait() => match status {
            Ok(status) => exit_status(status),
            Err(err) => stopped(format!("lost the command: {err}")),
        },
        () = tokio::time::sleep(timeout) => {
            stopped(format!("timed out after {} s", timeout.as_secs()))
        }
        () = ended.cancelled() => stopped("stopped: the thread's session ended".into()),
    };
    // Reaped (or killed and reaped now); what it left in its group goes too.
    let _ = child.wait().await;
    running.reaped = true;
    let _ = platform.processes().kill_group(pid);
    let mut output = drain(stdout).await;
    let stderr = drain(stderr).await;
    if !stderr.is_empty() {
        // `codex sandbox`'s own messages (the command's stderr is in stdout).
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
) -> (Kept, tokio::task::JoinHandle<()>) {
    let kept = Kept::default();
    let into = kept.clone();
    let reader = tokio::spawn(async move {
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            match pipe.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let mut kept = into.lock().unwrap_or_else(|p| p.into_inner());
                    let room = super::OUTPUT_MAX_BYTES.saturating_sub(kept.0.len());
                    let take = read.min(room);
                    kept.0.extend_from_slice(&buffer[..take]);
                    kept.1 += (read - take) as u64;
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
    fn every_thread_runs_and_leaves_its_sandbox_where_something_decides() {
        if cfg!(windows) {
            return;
        }
        assert_eq!(
            run_tools(ProviderKind::Claude, PermissionLevel::FullAccess),
            RunTools::Run
        );
        // Brigadier's reviewer decides at Approve for me, the user at Ask.
        assert_eq!(
            run_tools(ProviderKind::Claude, PermissionLevel::ApproveForMe),
            RunTools::WithEscalation
        );
        assert_eq!(
            run_tools(ProviderKind::Claude, PermissionLevel::AskForApproval),
            RunTools::WithEscalation
        );
        assert_eq!(
            run_tools(ProviderKind::Codex, PermissionLevel::FullAccess),
            RunTools::Run
        );
        // Approve for me leaves the sandbox with its own shell (see the module docs).
        assert_eq!(
            run_tools(ProviderKind::Codex, PermissionLevel::ApproveForMe),
            RunTools::Run
        );
        assert_eq!(
            run_tools(ProviderKind::Codex, PermissionLevel::AskForApproval),
            RunTools::WithEscalation
        );
    }

    #[test]
    fn a_command_runs_in_the_workspace_or_the_scratch_folder_only() {
        let root = std::env::temp_dir().join(format!("brigadier-workdir-{}", uuid::Uuid::new_v4()));
        let (workspace, scratch) = (root.join("ws"), root.join("orch"));
        std::fs::create_dir_all(workspace.join("crate")).unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        let real = |path: &Path| path.canonicalize().unwrap();
        assert_eq!(
            run_workdir(None, Some(&workspace), &scratch).unwrap(),
            real(&workspace)
        );
        assert_eq!(
            run_workdir(Some("crate"), Some(&workspace), &scratch).unwrap(),
            real(&workspace.join("crate"))
        );
        assert_eq!(
            run_workdir(Some(scratch.to_str().unwrap()), Some(&workspace), &scratch).unwrap(),
            real(&scratch)
        );
        assert_eq!(run_workdir(None, None, &scratch).unwrap(), real(&scratch));
        for outside in ["..", "/", "crate/../../orch/../.."] {
            assert!(
                run_workdir(Some(outside), Some(&workspace), &scratch).is_err(),
                "{outside}"
            );
        }
        assert!(run_workdir(Some("missing"), Some(&workspace), &scratch).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_approved_command_runs_once_and_only_in_its_conversation() {
        let passes = RunPasses::default();
        let (a, b) = (ConversationId("a".into()), ConversationId("b".into()));
        passes.grant(&a, "curl https://example.com", None);
        assert!(!passes.take(&b, "curl https://example.com", None));
        assert!(!passes.take(&a, "curl https://example.org", None));
        assert!(passes.take(&a, "curl https://example.com", None));
        assert!(!passes.take(&a, "curl https://example.com", None), "once");
    }

    #[test]
    fn an_approved_command_runs_only_in_the_directory_it_was_approved_for() {
        let passes = RunPasses::default();
        let a = ConversationId("a".into());
        passes.grant(&a, "sh script.sh", Some("app"));
        assert!(!passes.take(&a, "sh script.sh", None));
        assert!(!passes.take(&a, "sh script.sh", Some("other")));
        assert!(passes.take(&a, "sh script.sh", Some("app")));
    }

    #[test]
    fn stderr_joins_stdout_in_the_shell() {
        assert_eq!(
            shell_command("make test"),
            ["/bin/sh", "-c", "exec 2>&1\nmake test"]
        );
    }
}
