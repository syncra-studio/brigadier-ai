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
mod computer;
pub mod schema;

use std::sync::Arc;

use brigadier_core::tools::{Role, ToolHost, ToolReply};
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
/// The name of the computer-use server a worker's computer grant gets (`mcp__computer__<tool>`).
pub const COMPUTER_SERVER_NAME: &str = "computer";

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
    reply_result(ToolReply {
        is_error,
        ..ToolReply::ok(text)
    })
}

/// A reply as MCP content: its images first, as base64 image blocks, then its text.
fn reply_result(reply: ToolReply) -> CallToolResult {
    use base64::Engine as _;
    let mut content: Vec<ContentBlock> = reply
        .images
        .iter()
        .map(|image| {
            ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(&image.data),
                image.mime.clone(),
            )
        })
        .collect();
    content.push(ContentBlock::text(reply.text));
    let mut result = if reply.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    if let Some(us) = reply.engine_us {
        let mut meta = rmcp::model::JsonObject::new();
        meta.insert(
            "brigadier/engine_ms".into(),
            serde_json::json!(us as f64 / 1000.0),
        );
        result.meta = Some(rmcp::model::MetaObject(meta));
    }
    result
}

impl ServerHandler for BrigadierServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(
                match self.role {
                    Some(Role::Computer { .. }) => COMPUTER_SERVER_NAME,
                    _ => SERVER_NAME,
                },
                env!("CARGO_PKG_VERSION"),
            ),
        )
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
                Ok(reply_result(reply).into())
            }
            _ = context.ct.cancelled() => {
                tracing::debug!(tool = %request.name, "brigadier tool call cancelled");
                Err(ErrorData::internal_error("the call was cancelled", None))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_with_an_image_puts_it_first_as_base64() {
        let mut reply =
            ToolReply::ok("window w2 · image i1").with_image("image/png", vec![1, 2, 3]);
        reply.engine_us = Some(1_250);
        let result = serde_json::to_value(reply_result(reply)).unwrap();
        let content = result["content"].as_array().unwrap();
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["data"], "AQID");
        assert_eq!(content[0]["mimeType"], "image/png");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "window w2 · image i1");
        // The engine's time rides in `_meta`, out of the model's content.
        assert_eq!(result["_meta"]["brigadier/engine_ms"], 1.25);
    }
}
