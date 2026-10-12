//! Connections from CLI sessions, which carry a grant instead of the UI token.
//!
//! [`serve_mcp`]: after `ClientFrame::Mcp`, the connection is raw MCP, served in-process by
//! `brigadier_mcp_server` against the session manager. It runs as a tracked connection, so an
//! orderly quit closes it. [`serve_hook`]: after `ClientFrame::Hook`, a Claude thread's output
//! hook sends one command output, which is stored for the thread, and gets one answer. Grants
//! are never logged.

use std::sync::Arc;

use std::time::Duration;

use brigadier_core::manager::{HookOutput, OUTPUT_MAX_BYTES};
use brigadier_core::tools::{Role, ToolHost};
use brigadier_ipc::RawStream;
use brigadier_ipc::protocol::{HookOutput as HookFrame, HookReply};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::server::Daemon;

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::Orchestrator { .. } => "orchestrator",
        Role::Worker { .. } => "worker",
        Role::BrainJob { .. } => "brain job",
        Role::Chat { .. } => "chat",
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
            | Role::Worker { .. }
            | Role::BrainJob { .. }
            | Role::Chat { .. }),
        ) => role,
        other => {
            let role = other.as_ref().map_or("unknown", role_name);
            tracing::warn!(role, "refused an MCP connection: not a tool grant");
            return;
        }
    };
    let role = role_name(&role);
    tracing::debug!(role, "MCP connection opened");
    match brigadier_mcp_server::serve(host, grant, stream, daemon.closing.child_token()).await {
        Ok(()) => tracing::debug!(role, "MCP connection closed"),
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
    let Ok(frame) = brigadier_ipc::encode_frame(&HookReply { replacement }) else {
        return;
    };
    if let Err(err) = stream.write_all(&frame).await {
        tracing::debug!(error = %err, "an output hook left before its answer");
    }
    let _ = stream.flush().await;
}
