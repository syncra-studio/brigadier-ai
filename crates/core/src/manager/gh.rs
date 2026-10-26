//! The user's own `gh`, run with exactly their environment: the pull request row's lookups
//! and Send feedback's issues go through it.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

/// How long one `gh` call may take before it counts as a failed call.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The user's `gh`, run with exactly the user's environment.
#[derive(Clone)]
pub(crate) struct Gh {
    pub program: PathBuf,
    pub vars: Vec<(OsString, OsString)>,
}

/// What one `gh` call printed and how it exited.
#[derive(Debug, Clone)]
pub(crate) struct GhOutput {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl GhOutput {
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Why `gh` gave no output at all.
#[derive(Debug)]
pub(crate) enum RunError {
    /// It could not be started: missing, or not runnable.
    Spawn(std::io::Error),
    /// It did not finish within [`CALL_TIMEOUT`].
    Timeout,
}

impl std::fmt::Debug for Gh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Gh")
            .field("program", &self.program)
            .finish_non_exhaustive()
    }
}

impl Gh {
    /// Resolve a named identity privately. Credentials never enter args or diagnostics.
    pub(crate) async fn for_account(&self, host: &str, account: &str) -> Option<Self> {
        let mut named = self.clone();
        named.vars.retain(|(k, _)| {
            !matches!(
                k.to_str(),
                Some(
                    "GH_TOKEN"
                        | "GITHUB_TOKEN"
                        | "GH_ENTERPRISE_TOKEN"
                        | "GITHUB_ENTERPRISE_TOKEN"
                        | "GH_HOST"
                        | "GH_REPO"
                        | "GH_DEBUG"
                )
            )
        });
        let output = named
            .output(&["auth", "token", "--hostname", host, "--user", account])
            .await
            .ok()?;
        if !output.success() || output.stdout.trim().is_empty() {
            return None;
        }
        let key = if host == "github.com" {
            "GH_TOKEN"
        } else {
            "GH_ENTERPRISE_TOKEN"
        };
        named.vars.push((key.into(), output.stdout.trim().into()));
        Some(named)
    }

    /// Runs `gh` with `args`. An error means `gh` could not be started (or never finished);
    /// a non-zero exit is an `Ok` with its code.
    pub(crate) async fn output(&self, args: &[&str]) -> std::result::Result<GhOutput, RunError> {
        self.run(args, None).await
    }

    /// [`Self::output`] with `input` on its standard input (an issue's body for `--body-file -`).
    pub(crate) async fn output_with_input(
        &self,
        args: &[&str],
        input: &str,
    ) -> std::result::Result<GhOutput, RunError> {
        self.run(args, Some(input)).await
    }

    /// Runs `gh` with `args`, `input` on its standard input (else none). An error means `gh`
    /// could not be started (or never finished); a non-zero exit is an `Ok` with its code.
    async fn run(
        &self,
        args: &[&str],
        input: Option<&str>,
    ) -> std::result::Result<GhOutput, RunError> {
        let mut tries = 0;
        let mut child = loop {
            let mut command = tokio::process::Command::new(&self.program);
            command
                .args(args)
                .env_clear()
                .envs(self.vars.iter().map(|(key, value)| (key, value)))
                .stdin(if input.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            match command.spawn() {
                Ok(child) => break child,
                // A script just written by another thread can still be open for writing in a
                // forked child for a moment; that clears on its own.
                Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 10 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(err) => return Err(RunError::Spawn(err)),
            }
        };
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            use tokio::io::AsyncWriteExt;
            let written = async {
                stdin.write_all(input.as_bytes()).await?;
                stdin.shutdown().await
            };
            // Dropping `stdin` closes it, so `gh` reads to the end.
            match tokio::time::timeout(CALL_TIMEOUT, written).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => return Err(RunError::Spawn(err)),
                Err(_) => return Err(RunError::Timeout),
            }
        }
        let output = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| RunError::Timeout)?
            .map_err(RunError::Spawn)?;
        Ok(GhOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

impl super::SessionManager {
    /// The user's `gh`, from their login PATH, with their environment.
    pub(crate) fn gh(&self) -> Option<Gh> {
        #[cfg(test)]
        if let Some(gh) = self.gh_override.lock().unwrap().clone() {
            return Some(gh);
        }
        let env = self.runtime.cli_env();
        Some(Gh {
            program: env.which("gh")?,
            vars: env.vars(),
        })
    }
}
