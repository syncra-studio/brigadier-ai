//! MCP JSON-RPC 2.0 protocol types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Request ──────────────────────────────────────────────────────────────────

/// An incoming JSON-RPC 2.0 request or notification.
#[derive(Debug, Deserialize)]
pub struct Request {
    #[allow(dead_code)]
    pub jsonrpc: String,
    /// Absent on notifications.
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

impl Request {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }

    pub fn tool_call(&self) -> anyhow::Result<ToolCall> {
        let params = self
            .params
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing params"))?;
        let name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing tool name"))?
            .to_owned();
        let args = params
            .get("arguments")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        Ok(ToolCall { name, args })
    }
}

pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

// ── Response ─────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Response {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(flatten)]
    pub body: ResponseBody,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum ResponseBody {
    Result { result: Value },
    Error { error: RpcError },
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl Response {
    pub fn ok(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            body: ResponseBody::Result { result },
        }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            body: ResponseBody::Error {
                error: RpcError {
                    code,
                    message: message.into(),
                },
            },
        }
    }

    pub fn parse_error() -> Self {
        Self::error(Value::Null, -32700, "Parse error")
    }

    pub fn method_not_found(id: Value, method: &str) -> Self {
        Self::error(id, -32601, format!("Unknown method: {method}"))
    }
}

// ── Tool result content ───────────────────────────────────────────────────────

/// A single item in the `content` array of a `tools/call` result.
#[derive(Debug, Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Content {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
    Image {
        data: String, // base64-encoded PNG
        #[serde(rename = "mimeType")]
        mime_type: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        annotations: Option<Value>,
    },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Content::Text {
            text: text.into(),
            annotations: None,
        }
    }

    pub fn image_png(data_base64: String) -> Self {
        Content::Image {
            data: data_base64,
            mime_type: "image/png".into(),
            annotations: None,
        }
    }

    pub fn image_jpeg(data_base64: String) -> Self {
        Content::Image {
            data: data_base64,
            mime_type: "image/jpeg".into(),
            annotations: None,
        }
    }
}

/// The value placed in `result` for a `tools/call` response.
#[derive(Debug, Serialize, Default)]
pub struct ToolResult {
    pub content: Vec<Content>,
    #[serde(rename = "isError", skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
}

impl ToolResult {
    pub fn text(msg: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(msg)],
            ..Default::default()
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            content: vec![Content::text(msg)],
            is_error: Some(true),
            ..Default::default()
        }
    }

    pub fn with_structured(mut self, v: Value) -> Self {
        self.structured_content = Some(v);
        self
    }
}

// ── Initialize result ─────────────────────────────────────────────────────────

pub fn initialize_result() -> Value {
    serde_json::json!({
        "protocolVersion": "2025-06-18",
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "brigadier-cu", "version": env!("CARGO_PKG_VERSION") },
        "instructions": agent_instructions()
    })
}

/// MCP `instructions` (`InitializeResult.instructions`) sent to every
/// connecting client. Clients may add this to the system prompt on every
/// turn, so it stays short; the long-form workflow lives in the
/// `computer-use` skill that Brigadier gives its agents.
fn agent_instructions() -> String {
    r#"Computer Use: see and drive apps on this Mac in the background, without taking keyboard focus or moving the user's pointer. Prefer element_index (Accessibility tree) targets over pixel coordinates; they work on hidden and background windows.

Each turn:
0. start_session(session) once per run with a stable id you choose; pass the same `session` on every action; end_session(session) when done. Parallel runs use their own ids.
1. launch_app: idempotent, returns the pid and its windows in one call.
2. get_window_state(pid, window_id): fresh Accessibility snapshot with element indices.
3. click / type_text / press_key / set_value with an element_index from step 2. Batch several grounded steps with perform_actions.
4. get_window_state again to confirm the action landed.

Never bring a window to the front unless the user asked for it. If the `computer-use` skill is available, follow it for the full workflow.

Calls that target the user's Brigadier, terminal apps, or macOS security surfaces return target_not_allowed; do not retry them."#
        .to_owned()
}

#[cfg(test)]
mod image_mime_type_tests {
    use super::Content;

    /// Surface 7 contract: image parts serialize an explicit `mimeType` field
    /// (not `mime_type`, not `format`) so MCP consumers don't have to sniff
    /// magic bytes off the base64 PNG/JPEG to know what they're holding.
    /// This locks in the JSON shape — any rename or drop breaks here first.
    #[test]
    fn image_png_serializes_with_explicit_mime_type() {
        let c = Content::image_png("ZmFrZQ==".into());
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("image"));
        assert_eq!(
            v.get("mimeType").and_then(|t| t.as_str()),
            Some("image/png")
        );
        assert_eq!(v.get("data").and_then(|t| t.as_str()), Some("ZmFrZQ=="));
    }

    #[test]
    fn image_jpeg_serializes_with_explicit_mime_type() {
        let c = Content::image_jpeg("ZmFrZQ==".into());
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("image"));
        assert_eq!(
            v.get("mimeType").and_then(|t| t.as_str()),
            Some("image/jpeg")
        );
    }

    /// Text parts must not grow a mimeType field — the field belongs to
    /// image parts only. Guards against an accidental serde_json renamer
    /// that emits a shared shape across the enum.
    #[test]
    fn text_does_not_carry_mime_type() {
        let c = Content::text("hi");
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("text"));
        assert!(
            v.get("mimeType").is_none(),
            "text content must not carry mimeType"
        );
    }
}
