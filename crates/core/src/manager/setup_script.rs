//! A new worktree's tracked setup script (`.brigadier/setup`) is plumbing: the engine runs it
//! once, before the session's first turn or the worker's first command, under that session's
//! own access and only in a folder the user trusts, then tells the session how it went. A
//! failure is a fact the session hears, not a gate (#326). Where Brigadier can't hold the
//! command to the session's access (Windows; a sandboxed session off macOS without Codex),
//! the session is told to run it itself, as before.

use std::path::Path;
use std::time::Duration;

use brigadier_providers::Access;
use brigadier_sandbox::Protection;

use super::SessionManager;
use super::prompts::{SETUP_LINE, SETUP_SCRIPT};
use super::run::{CommandOutput, RUN_TIMEOUT_DEFAULT, run_command_streamed, shell_command};
use crate::model::ConversationId;

/// How long the setup script may run.
const SETUP_TIMEOUT: Duration = RUN_TIMEOUT_DEFAULT;
/// The most of its output's end the session is shown.
const TAIL_LINES: usize = 20;
const TAIL_BYTES: usize = 2000;
/// The most of its output's end kept as it runs, the tail's lines picked from.
const KEPT_BYTES: usize = 4 * TAIL_BYTES;

impl SessionManager {
    /// Runs the setup script in the new `worktree` of conversation `id` as `access` allows
    /// (`protection` kept at Full access), its process recorded under `owner` while it runs,
    /// with `env`, until it ends or `cancel` (the user's Stop). Returns what the session is
    /// told.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_setup_script(
        &self,
        id: &ConversationId,
        owner: &str,
        worktree: &Path,
        access: &Access,
        protection: Option<&Protection>,
        env: &[(String, String)],
        cancel: tokio_util::sync::CancellationToken,
    ) -> String {
        if cfg!(windows) {
            return SETUP_LINE.into();
        }
        // A script that isn't marked executable runs with the shell.
        let command = format!(
            "if [ -x ./{SETUP_SCRIPT} ]; then exec ./{SETUP_SCRIPT}; else exec /bin/sh ./{SETUP_SCRIPT}; fi"
        );
        let spec = match access {
            Access::Full => {
                self.run_spec(access, worktree, &command)
                    .and_then(|spec| match protection {
                        Some(protection) => self
                            .runtime
                            .platform()
                            .sandbox()
                            .protect(spec, protection)
                            .map_err(|err| crate::Error::Invalid(err.to_string())),
                        None => Ok(spec),
                    })
            }
            _ if cfg!(target_os = "macos") => {
                self.seatbelt_spec(access, worktree, &shell_command(&command)[1..])
            }
            _ => self.run_spec(access, worktree, &command),
        };
        let mut spec = match spec {
            Ok(spec) => spec,
            Err(err) => {
                tracing::warn!(worktree = %worktree.display(), %err, "can't run the setup script here; the session runs it");
                return SETUP_LINE.into();
            }
        };
        brigadier_providers::cli::apply_session_env(&mut spec, env, &[]);
        let started = std::time::Instant::now();
        // The end of what it printed, however much that is.
        let end: std::sync::Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
        let keep: CommandOutput = {
            let end = end.clone();
            std::sync::Arc::new(move |chunk: Vec<u8>| {
                let mut end = end.lock().unwrap_or_else(|p| p.into_inner());
                end.extend_from_slice(&chunk);
                let over = end.len().saturating_sub(KEPT_BYTES);
                end.drain(..over);
                Box::pin(async {})
            })
        };
        let ran = run_command_streamed(
            self.runtime.platform().clone(),
            &spec,
            SETUP_TIMEOUT,
            cancel,
            owner,
            self.runtime.ledger(),
            Some(keep),
        )
        .await;
        let status = match ran {
            Ok(ran) => ran.status,
            Err(err) => err.to_string(),
        };
        tracing::info!(worktree = %worktree.display(), %status, ms = started.elapsed().as_millis() as u64, "ran the setup script");
        let output = std::mem::take(&mut *end.lock().unwrap_or_else(|p| p.into_inner()));
        let output = self.hide_secrets(id, output).await;
        setup_ran_line(&status, &tail(&String::from_utf8_lossy(&output)))
    }
}

/// The end of a command's output: its last [`TAIL_LINES`] lines, at most [`TAIL_BYTES`].
fn tail(output: &str) -> String {
    let lines: Vec<&str> = output.trim_end().lines().collect();
    let mut kept = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    if kept.len() > TAIL_BYTES {
        let mut from = kept.len() - TAIL_BYTES;
        while !kept.is_char_boundary(from) {
            from += 1;
        }
        kept = kept[from..].to_owned();
    }
    kept
}

/// What the session is told once the engine ran its new worktree's setup script.
pub(crate) fn setup_ran_line(status: &str, tail: &str) -> String {
    let how = if status == "exit 0" {
        format!(
            "Brigadier ran this repository's setup script `{SETUP_SCRIPT}` in your new worktree before you started ({status}); don't run it again."
        )
    } else {
        format!(
            "Brigadier ran this repository's setup script `{SETUP_SCRIPT}` in your new worktree before you started, and it failed ({status}). The worktree is otherwise ready: deal with what the failure means for your work."
        )
    };
    if tail.trim().is_empty() {
        format!("{how} It printed nothing.")
    } else {
        format!("{how} The end of its output:\n```\n{tail}\n```")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_last_lines_within_its_bytes() {
        let many: String = (1..=30).map(|n| format!("line {n}\n")).collect();
        let kept = tail(&many);
        assert!(
            kept.starts_with("line 11\n") && kept.ends_with("line 30"),
            "{kept}"
        );
        let long = "é".repeat(3000);
        assert!(tail(&long).len() <= TAIL_BYTES);
        assert_eq!(tail("\n\n"), "");
    }
}
