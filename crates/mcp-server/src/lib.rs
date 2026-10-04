//! The Brigadier MCP server: the orchestrator's and the workers' tools.
//!
//! Every CLI session Brigadier starts gets this server through `brigadierd mcp`, a stdio bridge
//! that hands the connection to the daemon, which calls [`serve`] with the session's grant.
//!
//! - The grant's role decides the tool list, once, when the connection opens: an orchestrator
//!   gets the orchestrator tools, a worker `ask_orchestrator` (unless it checks a change or
//!   plan in a gate) and `submit_report`. A gate grant or an unknown grant is refused at
//!   `initialize`, so the CLI gets no tools at all.
//! - Every `tools/call` goes through [`ToolHost::call`], which checks the grant again, so a
//!   revoked grant stops working on an open connection too.
//! - A call cancelled by the client, or cut off by the connection closing, drops the host's
//!   future.
//!
//! Tool input schemas are kept to the JSON Schema subset both Claude Code and Codex accept
//! (see [`schema`]).

mod catalog;
pub mod schema;

use std::sync::Arc;

use brigadier_core::tools::{Role, ToolHost};
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
    Implementation, InitializeRequestParams, InitializeResult, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::{RequestContext, ServerInitializeError};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

pub use catalog::tools_for;

/// How long a CLI may keep the tool list (a day): it is fixed for the life of a grant.
const TOOLS_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// The name the CLIs know the server by (their tools show up as `mcp__brigadier__<tool>`).
pub const SERVER_NAME: &str = "brigadier";

/// Why a connection ended other than by the client closing it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The grant is unknown, revoked, or belongs to the command gate.
    #[error("the grant does not allow Brigadier tools")]
    Refused,
    #[error("MCP handshake failed: {0}")]
    Handshake(String),
    #[error("the MCP service task failed: {0}")]
    Service(#[from] tokio::task::JoinError),
}

/// Serves one MCP connection (newline-delimited JSON-RPC, the stdio transport's framing) for
/// `grant` until the client closes it or `cancel` fires.
pub async fn serve<IO>(
    host: Arc<dyn ToolHost>,
    grant: String,
    io: IO,
    cancel: CancellationToken,
) -> Result<(), Error>
where
    IO: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let role = host.role(&grant);
    let refused = role.is_none();
    let server = BrigadierServer { host, grant, role };
    let running = match server.serve_with_ct(io, cancel).await {
        Ok(running) => running,
        Err(ServerInitializeError::InitializeFailed(_)) if refused => return Err(Error::Refused),
        Err(ServerInitializeError::ConnectionClosed(_)) | Err(ServerInitializeError::Cancelled) => {
            return Ok(());
        }
        Err(err) => return Err(Error::Handshake(err.to_string())),
    };
    running.waiting().await?;
    Ok(())
}

struct BrigadierServer {
    host: Arc<dyn ToolHost>,
    grant: String,
    /// The grant's role when the connection opened; `None` when it was refused.
    role: Option<Role>,
}

impl BrigadierServer {
    fn role(&self) -> Result<&Role, ErrorData> {
        self.role.as_ref().ok_or_else(refused)
    }
}

fn refused() -> ErrorData {
    ErrorData::invalid_request(
        "This Brigadier session is not allowed to use Brigadier tools (unknown or ended grant).",
        None,
    )
}

fn text_result(text: String, is_error: bool) -> CallToolResult {
    let content = vec![ContentBlock::text(text)];
    if is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    }
}

impl ServerHandler for BrigadierServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        self.role()?;
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        // Protocol 2026-07-28 requires both cache fields: Claude Code rejects the whole list
        // without them, and the session starts with no Brigadier tools. A role's tools never
        // change while its grant lives, and they are the grant's own.
        Ok(
            ListToolsResult::with_all_items(tools_for(self.role()?).to_vec())
                .with_ttl_ms(TOOLS_TTL_MS)
                .with_cache_scope(CacheScope::Private),
        )
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let role = self.role()?;
        let call = match catalog::parse_call(role, &request.name, request.arguments) {
            Ok(call) => call,
            Err(err) => return Ok(text_result(err.to_string(), true).into()),
        };
        tracing::debug!(tool = %request.name, "brigadier tool call");
        tokio::select! {
            reply = self.host.call(&self.grant, call) => {
                Ok(text_result(reply.text, reply.is_error).into())
            }
            _ = context.ct.cancelled() => {
                tracing::debug!(tool = %request.name, "brigadier tool call cancelled");
                Err(ErrorData::internal_error("the call was cancelled", None))
            }
        }
    }
}
