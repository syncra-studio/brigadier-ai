//! Async MCP stdio server loop.

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::protocol::{initialize_result, Request, Response};
use crate::tool::ToolRegistry;

/// Run the MCP server, reading JSON-RPC lines from stdin and writing
/// responses to stdout. Exits when stdin reaches EOF or a fatal I/O
/// error occurs.
pub async fn run(registry: Arc<ToolRegistry>) -> anyhow::Result<()> {
    run_with_initialize_result_and_tool_profile(registry, initialize_result(), false).await
}

/// Run the stdio server with a profile-specific MCP initialize envelope.
pub async fn run_with_initialize_result(
    registry: Arc<ToolRegistry>,
    initialize: serde_json::Value,
) -> anyhow::Result<()> {
    run_with_initialize_result_and_tool_profile(registry, initialize, false).await
}

async fn run_with_initialize_result_and_tool_profile(
    registry: Arc<ToolRegistry>,
    initialize: serde_json::Value,
    codex_computer_use_compat: bool,
) -> anyhow::Result<()> {
    // One stdio process is one embedded MCP session. Keep its implicit cursor
    // under the same session_end lifecycle as declared sessions, including I/O
    // errors: the guard fires cleanup when this function returns or unwinds.
    let _embedded_session = EmbeddedSessionGuard::new();
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            // EOF
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ request");

        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Response::parse_error()
            }
            Ok(req) if req.is_notification() => {
                // Notifications are silently dropped.
                continue;
            }
            Ok(req) => {
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                handle_request_with_initialize_result(
                    req,
                    id,
                    &registry,
                    &initialize,
                    codex_computer_use_compat,
                )
                .await
            }
        };

        let serialized = serde_json::to_string(&response)
            .unwrap_or_else(|e| format!(r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#));
        debug!(raw = %serialized, "← response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

struct EmbeddedSessionGuard(Option<&'static str>);

impl EmbeddedSessionGuard {
    fn new() -> Self {
        Self(crate::embedded_default_session_id())
    }
}

impl Drop for EmbeddedSessionGuard {
    fn drop(&mut self) {
        if let Some(session_id) = self.0 {
            crate::session::fire_session_end(session_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EmbeddedSessionGuard;

    #[test]
    fn embedded_stdio_guard_ends_its_default_session_on_drop() {
        let session_id = "embedded-stdio-guard-test-7f9c";
        crate::session::revive_session(session_id);
        assert!(!crate::session::is_session_ended(session_id));
        drop(EmbeddedSessionGuard(Some(session_id)));
        assert!(crate::session::is_session_ended(session_id));
    }
}

/// Dispatch one MCP JSON-RPC request against the registry (initialize /
/// tools/list / tools/call). Shared by the stdio loop above and the
/// daemon's HTTP transport (`brigadier-cu`'s `mcp_http`) so both speak the
/// exact same MCP semantics.
pub async fn handle_request(
    req: Request,
    id: serde_json::Value,
    registry: &Arc<ToolRegistry>,
) -> Response {
    handle_request_with_initialize_result(req, id, registry, &initialize_result(), false).await
}

async fn handle_request_with_initialize_result(
    req: Request,
    id: serde_json::Value,
    registry: &Arc<ToolRegistry>,
    initialize: &serde_json::Value,
    codex_computer_use_compat: bool,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize.clone()),

        "tools/list" => Response::ok(
            id,
            if codex_computer_use_compat {
                registry.codex_computer_use_tools_list()
            } else {
                registry.tools_list()
            },
        ),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                let mut args = call.args;
                // The model never sets the target scope; this process's env does.
                crate::target_policy::scope_target_args(
                    &mut args,
                    &crate::target_policy::allowed_from_env(),
                );
                let result = registry.invoke(&call.name, args).await;
                match serde_json::to_value(result) {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::error(id, -32603, format!("Serialize error: {e}")),
                }
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}
