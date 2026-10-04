//! Connections from CLI sessions, which carry a grant instead of the UI token.
//!
//! [`serve_mcp`]: after `ClientFrame::Mcp`, the connection is raw MCP, served in-process by
//! `brigadier_mcp_server` against the session manager. It runs as a tracked connection, so an
//! orderly quit closes it. Grants are never logged.

use std::sync::Arc;

use brigadier_core::tools::{Role, ToolHost};
use brigadier_ipc::RawStream;

use crate::server::Daemon;

fn role_name(role: &Role) -> &'static str {
    match role {
        Role::Orchestrator { .. } => "orchestrator",
        Role::Worker { .. } => "worker",
        Role::BrainJob { .. } => "brain job",
        Role::Chat { .. } => "chat",
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
