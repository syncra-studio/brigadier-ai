//! Connections from CLI sessions, which carry a grant instead of the UI token.
//!
//! [`serve_mcp`]: after `ClientFrame::Mcp`, the connection is raw MCP, served in-process by
//! `brigadier_mcp_server` against the session manager. It runs as a tracked connection, so an
//! orderly quit closes it. [`serve_hook`]: after `ClientFrame::Hook`, a Claude thread's output
//! hook sends one command output, which is stored for the thread, and gets one answer.
//! [`serve_cli`]: after `ClientFrame::Cli`, a session's `brigadier` command sends one call and
//! gets one answer. Grants are never logged.

use tracing::Instrument;

use std::sync::Arc;

use std::time::Duration;

use brigadier_cli::protocol::{CliCall, CliReply, ErrorCode};
use brigadier_core::manager::{HookOutput, OUTPUT_MAX_BYTES};
use brigadier_core::tools::{Role, ToolHost};
use brigadier_ipc::protocol::{HookOutput as HookFrame, HookReply};
use brigadier_ipc::{Connection, RawStream};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::server::Daemon;

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::Orchestrator { .. } => "orchestrator",
        Role::NoteFork { .. } => "note fork",
        Role::Worker { .. } => "worker",
        Role::BrainJob { .. } => "brain job",
        Role::Chat { .. } => "chat",
        Role::TabChat { .. } => "tab chat",
        Role::OutputHook { .. } => "output hook",
    }
}

/// Serves the Brigadier MCP tools on `stream` for an orchestrator, worker, Brain job or Chat
/// grant; any other grant closes the connection at once.
pub async fn serve_mcp(daemon: Arc<Daemon>, grant: String, stream: RawStream) {
    let host: Arc<dyn ToolHost> = daemon.sessions.clone();
    let role = match host.role(&grant) {
        Some(
            role @ (Role::Orchestrator { .. }
            | Role::NoteFork { .. }
            | Role::Worker { .. }
            | Role::BrainJob { .. }
            | Role::Chat { .. }
            | Role::TabChat { .. }),
        ) => role,
        other => {
            let role = other.as_ref().map_or("unknown", role_name);
            tracing::warn!(role, "refused an MCP connection: not a tool grant");
            return;
        }
    };
    let private_logs = matches!(role, Role::TabChat { .. });
    let role = role_name(&role);
    tracing::debug!(role, "MCP connection opened");
    let span = if private_logs {
        tracing::info_span!("tab_chat_private")
    } else {
        tracing::Span::none()
    };
    match brigadier_mcp_server::serve(host, grant, stream, daemon.closing.child_token())
        .instrument(span)
        .await
    {
        Ok(()) => tracing::debug!(role, "MCP connection closed"),
        Err(_) if private_logs => tracing::warn!(role = "tab chat", "MCP connection failed"),
        Err(err) => tracing::warn!(role, error = %err, "MCP connection failed"),
    }
}

/// How long a hook may take to send its output.
const HOOK_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Serves a Claude thread's output hook: reads the output that follows its frame, has it
/// stored for the thread, and answers with what the model gets instead (its digest, or
/// nothing). Any other grant, or more output than Brigadier keeps, closes the connection.
pub async fn serve_hook(
    daemon: Arc<Daemon>,
    grant: String,
    output: HookFrame,
    mut stream: RawStream,
) {
    if !matches!(daemon.sessions.role(&grant), Some(Role::OutputHook { .. })) {
        tracing::warn!("refused an output hook: not a hook grant");
        return;
    }
    // A file tool's call: no output follows, and the answer is its folders' rules.
    if let Some(path) = &output.touched {
        let context = daemon.sessions.hook_rules(&grant, path).await.flatten();
        answer_hook(
            &mut stream,
            &HookReply {
                replacement: None,
                context,
            },
        )
        .await;
        return;
    }
    let Some(len) = usize::try_from(output.bytes)
        .ok()
        .filter(|len| *len <= OUTPUT_MAX_BYTES + 1024)
    else {
        tracing::warn!(
            bytes = output.bytes,
            "refused an output hook: too much output"
        );
        return;
    };
    let mut bytes = vec![0u8; len];
    match tokio::time::timeout(HOOK_READ_TIMEOUT, stream.read_exact(&mut bytes)).await {
        Ok(Ok(_)) => {}
        Ok(Err(err)) => {
            tracing::warn!(error = %err, "an output hook's output was cut short");
            return;
        }
        Err(_) => {
            tracing::warn!("an output hook took too long to send its output");
            return;
        }
    }
    let kind = if output.excerpt {
        HookOutput::Excerpt
    } else {
        HookOutput::Full
    };
    let replacement = match daemon
        .sessions
        .hook_output(&grant, kind, &output.status, bytes)
        .await
    {
        None => return,
        Some(Ok(replacement)) => replacement,
        Some(Err(err)) => {
            tracing::warn!(error = %err, "could not store a thread's command output");
            None
        }
    };
    answer_hook(
        &mut stream,
        &HookReply {
            replacement,
            context: None,
        },
    )
    .await;
}

/// Sends a hook its answer; a hook that left before it is no matter.
async fn answer_hook(stream: &mut RawStream, reply: &HookReply) {
    let Ok(frame) = brigadier_ipc::encode_frame(reply) else {
        return;
    };
    if let Err(err) = stream.write_all(&frame).await {
        tracing::debug!(error = %err, "an output hook left before its answer");
    }
    let _ = stream.flush().await;
}

/// Answers one `brigadier` call of a session: its grant must be the live grant of the session it
/// names, or the connection closes without a word (the command says it was refused).
pub async fn serve_cli(
    daemon: Arc<Daemon>,
    session: String,
    grant: String,
    connection: Connection,
) {
    let Some(caller) = daemon.sessions.cli_caller(&session, &grant) else {
        tracing::warn!("refused a brigadier call: not a live grant of the session it names");
        return;
    };
    let Connection {
        mut reader,
        mut writer,
    } = connection;
    let call = match tokio::time::timeout(
        brigadier_ipc::transport::AUTH_TIMEOUT,
        reader.read::<serde_json::Value>(),
    )
    .await
    {
        Ok(Ok(Some(call))) => call,
        Ok(Ok(None)) => return,
        Ok(Err(err)) => {
            tracing::debug!(error = %err, "a brigadier call could not be read");
            return;
        }
        Err(_) => {
            tracing::debug!("a brigadier call came too late");
            return;
        }
    };
    let reply = match serde_json::from_value::<CliCall>(call) {
        Ok(call) => {
            tokio::select! {
                reply = brigadier_core::heap(|| daemon.sessions.cli_call(&caller, call)) => reply,
                () = daemon.closing.cancelled() => {
                    CliReply::error(ErrorCode::Unreachable, "Brigadier is quitting")
                }
                // `brigadier` sends nothing after its call: anything read means it hung up (a
                // `brigadier wait` stopped), and its call ends unanswered, so a wait takes no
                // event nobody reads.
                _ = reader.read::<serde_json::Value>() => {
                    tracing::debug!("a brigadier caller hung up before its answer");
                    return;
                }
            }
        }
        // A newer `brigadier` than this app, most likely.
        Err(err) => CliReply::error(
            ErrorCode::Version,
            format!(
                "Brigadier doesn't know this request ({err}): use the brigadier that came with the app"
            ),
        ),
    };
    if let Err(err) = writer.write(&reply).await {
        tracing::debug!(error = %err, "a brigadier answer could not be sent");
    }
}
