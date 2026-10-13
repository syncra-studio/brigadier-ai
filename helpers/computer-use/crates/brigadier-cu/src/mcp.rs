//! The helper's own MCP surface: the `initialize` answer agents get, the
//! in-process stdio server, and the argument check the proxy runs before it
//! starts the daemon.

use std::sync::Arc;

use cu_core::protocol::{Request, Response, ToolResult};
use cu_core::tool::{ToolDef, ToolRegistry};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

// ── Initialize result ─────────────────────────────────────────────────────────

pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "brigadier-cu", "version": env!("CARGO_PKG_VERSION") },
        "instructions": agent_instructions(host_manages_session())
    })
}

/// Whether the host that started this process already gives each connection its own session
/// (and with it the agent cursor): every call then carries that session, and the connection
/// closing ends it.
fn host_manages_session() -> bool {
    std::env::var(crate::env::DEFAULT_SESSION_ENV).is_ok_and(|value| !value.trim().is_empty())
}

/// MCP `instructions` (`InitializeResult.instructions`) sent to every
/// connecting client. Clients may add this to the system prompt on every
/// turn, so it stays short; the long-form workflow lives in the
/// `computer-use` skill that Brigadier gives its agents.
fn agent_instructions(host_manages_session: bool) -> String {
    let session_step = if host_manages_session {
        "0. Your session and agent cursor are already set up for you: don't call start_session or end_session, and leave out `session`."
    } else {
        "0. start_session(session) once per run with a stable id you choose; pass the same `session` on every action; end_session(session) when done. Parallel runs use their own ids."
    };
    r#"Computer Use: see and drive apps on this Mac in the background, without taking keyboard focus or moving the user's pointer. Prefer element_index (Accessibility tree) targets over pixel coordinates; they work on hidden and background windows.

Each turn:
SESSION_STEP
1. launch_app: idempotent, returns the pid and its windows in one call.
2. get_window_state(pid, window_id): fresh Accessibility snapshot with element indices.
3. click / type_text / press_key / set_value with an element_index from step 2. Batch several grounded steps with perform_actions.
4. get_window_state again to confirm the action landed.

Never bring a window to the front unless the user asked for it. If the `computer-use` skill is available, follow it for the full workflow.

Calls that target the user's Brigadier, terminal apps, or macOS security surfaces return target_not_allowed; do not retry them."#
        .replace("SESSION_STEP", session_step)
}

// ── In-process stdio server ───────────────────────────────────────────────────

/// Serve MCP over stdio against `registry`: JSON-RPC lines from stdin,
/// answers to stdout, until stdin reaches EOF or an I/O error.
pub async fn run(registry: Arc<ToolRegistry>) -> anyhow::Result<()> {
    // One stdio process is one embedded MCP session. Keep its implicit cursor
    // under the same session_end lifecycle as declared sessions, including I/O
    // errors: the guard fires cleanup when this function returns or unwinds.
    let _embedded_session = EmbeddedSessionGuard(cu_core::embedded_default_session_id());
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut writer = tokio::io::BufWriter::new(tokio::io::stdout());
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
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
            // Notifications are silently dropped.
            Ok(req) if req.is_notification() => continue,
            Ok(req) => {
                let id = req.id.clone().unwrap_or(Value::Null);
                handle_request(req, id, &registry).await
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

impl Drop for EmbeddedSessionGuard {
    fn drop(&mut self) {
        if let Some(session_id) = self.0 {
            cu_core::session::fire_session_end(session_id);
        }
    }
}

async fn handle_request(req: Request, id: Value, registry: &Arc<ToolRegistry>) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),
        "tools/list" => Response::ok(id, registry.tools_list()),
        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                let mut args = call.args;
                // The model never sets the target scope; this process's env does.
                crate::scope_args::scope_target_args(
                    &mut args,
                    &cu_core::target_policy::allowed_from_env(),
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

// ── Dispatch argument check ───────────────────────────────────────────────────

/// Validate the JSON-Schema vocabulary used by driver tool definitions at the
/// embedded dispatch boundary. This intentionally covers only the vocabulary
/// emitted by our hand-written schemas: object properties, required fields,
/// primitive types, array item types/cardinality, enums, and numeric bounds.
///
/// MCP clients normally validate this schema themselves, but the driver cannot
/// rely on that for focus safety: direct CLI/daemon callers can reach the
/// registry with malformed JSON. Internal daemon metadata keys (`_...`) are
/// ignored because they are injected after client-side schema validation.
/// So is `session` on a tool that doesn't take one: agents are told to pass
/// their session on every call, and a tool that has no use for it ignores it.
pub fn validate_dispatch_args(def: &ToolDef, args: &Value) -> Result<(), ToolResult> {
    let Some(object) = args.as_object() else {
        return Err(dispatch_validation_error(
            &def.name,
            "arguments must be an object",
        ));
    };
    let schema = &def.input_schema;
    let properties = schema.get("properties").and_then(Value::as_object);

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for field in required.iter().filter_map(Value::as_str) {
            if !object.get(field).is_some_and(|value| !value.is_null()) {
                return Err(dispatch_validation_error(
                    &def.name,
                    &format!("missing required field `{field}`"),
                ));
            }
        }
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        if let Some(properties) = properties {
            if let Some(field) = object.keys().find(|field| {
                !field.starts_with('_') && *field != "session" && !properties.contains_key(*field)
            }) {
                return Err(dispatch_validation_error(
                    &def.name,
                    &format!("unknown field `{field}`"),
                ));
            }
        }
    }

    let Some(properties) = properties else {
        return Ok(());
    };
    for (field, value) in object {
        if field.starts_with('_') || value.is_null() {
            continue;
        }
        let Some(field_schema) = properties.get(field) else {
            continue;
        };
        validate_dispatch_value(&def.name, field, value, field_schema)?;
    }
    Ok(())
}

fn validate_dispatch_value(
    tool_name: &str,
    field: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), ToolResult> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let valid = match expected {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !valid {
            return Err(dispatch_validation_error(
                tool_name,
                &format!("field `{field}` must be {expected}"),
            ));
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(dispatch_validation_error(
                tool_name,
                &format!("field `{field}` has an unsupported value"),
            ));
        }
    }

    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must be at least {minimum}"),
                ));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must be at most {maximum}"),
                ));
            }
        }
    }

    if let (Some(values), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for item in values {
            validate_dispatch_value(tool_name, field, item, item_schema)?;
        }
    }
    if let Some(values) = value.as_array() {
        if let Some(min_items) = schema.get("minItems").and_then(Value::as_u64) {
            if values.len() < min_items as usize {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must contain at least {min_items} items"),
                ));
            }
        }
        if let Some(max_items) = schema.get("maxItems").and_then(Value::as_u64) {
            if values.len() > max_items as usize {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must contain at most {max_items} items"),
                ));
            }
        }
    }
    Ok(())
}

fn dispatch_validation_error(tool_name: &str, detail: &str) -> ToolResult {
    ToolResult::error(format!("Invalid {tool_name} arguments: {detail}.")).with_structured(
        serde_json::json!({
            "code": "invalid_arguments",
            "tool": tool_name,
            "detail": detail,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_the_host_manages_takes_no_session_calls() {
        let hosted = agent_instructions(true);
        assert!(
            hosted.contains("don't call start_session or end_session"),
            "{hosted}"
        );
        assert!(!hosted.contains("SESSION_STEP"));
        let standalone = agent_instructions(false);
        assert!(
            standalone.contains("start_session(session) once per run"),
            "{standalone}"
        );
    }

    #[test]
    fn session_and_private_args_pass_a_closed_schema() {
        let def = ToolDef {
            name: "probe".to_owned(),
            description: String::new(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": { "window_id": { "type": "integer" } },
                "additionalProperties": false,
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        };
        assert!(validate_dispatch_args(&def, &serde_json::json!({"session": "run-1"})).is_ok());
        assert!(validate_dispatch_args(&def, &serde_json::json!({"_host_session": "a"})).is_ok());
        assert!(validate_dispatch_args(&def, &serde_json::json!({"windw": 1})).is_err());
        assert!(validate_dispatch_args(&def, &serde_json::json!({"window_id": "1"})).is_err());
    }
}
