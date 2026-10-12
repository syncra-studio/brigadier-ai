use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use super::ToolState;

pub struct MoveCursorTool {
    state: Arc<ToolState>,
}

impl MoveCursorTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "move_cursor".into(),
        description: "Move the agent cursor overlay to (x, y). Does NOT move the real mouse \
            cursor — the user's cursor stays where it is. Useful for showing the agent's \
            attention without interrupting the user.".into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["x", "y"],
            "properties": {
                "session": { "type": "string", "description": "Optional explicit session id for the agent cursor and per-session state. Embedded MCP calls may omit it to use BRIGADIER_CU_DEFAULT_SESSION (or embedded-<pid>); anonymous non-embedded calls remain cursor-less." },
                "x": { "type": "number" },
                "y": { "type": "number" },
                "cursor_id": { "type": "string", "description": "Cursor instance to move. Default: 'default'." }
            },
            "additionalProperties": false
        }),
        // read-only: move_cursor only nudges the agent-cursor overlay, never the
        // target app — so it's safe to run concurrently. The `readOnlyHint` this
        // emits lets MCP clients (e.g. Claude Code's isConcurrencySafe) parallelize
        // cursor moves. (Mutating tools like click stay read_only:false on purpose.)
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        let x = match args.require_f64("x") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let y = match args.require_f64("y") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let cursor_id = super::cursor_tools::resolve_cursor_key(&args);
        if args.opt_str(cu_core::HOST_SESSION_ARG).is_some()
            && !super::cursor_tools::host_cursor_lifecycle_is_live(&args)
        {
            return ToolResult::text(format!(
                "Agent cursor '{cursor_id}' move ignored after its session ended."
            ));
        }
        if let Some(host_session) = args.opt_str(cu_core::HOST_SESSION_ARG) {
            if let Some(generation) = args.opt_str("session") {
                crate::cursor::overlay::begin_reusable_cursor_generation(host_session, generation);
            } else {
                crate::cursor::overlay::activate_reusable_cursor(host_session);
            }
        }

        // Unlike click/scroll/drag, move_cursor has no target window id to pin
        // above. Re-anchor the overlay above WindowServer's real frontmost
        // layer-0 window before animating so an explicit visual cursor move can
        // never remain hidden behind the app the user is looking at.
        let driver_pid = std::process::id() as i32;
        let anchor_window_id = tokio::task::spawn_blocking(move || {
            crate::windows::cursor_overlay_anchor_window(
                &crate::windows::visible_windows(),
                driver_pid,
            )
        })
        .await
        .unwrap_or(None);
        if let Some(window_id) = anchor_window_id {
            crate::cursor::overlay::send_command(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::PinAbove(window_id as u64),
            );
        }

        self.state.cursor_registry.update_position(&cursor_id, x, y);
        // Drive the DRAWN cursor via the same path as click's animation. A raw
        // `MoveTo` doesn't reliably bring a brand-new session cursor on-screen —
        // it sits at the off-screen sentinel until a click seeds it, so the
        // visible cursor wouldn't move (the reported position would, but the
        // overlay wouldn't). `animate_cursor_to` seeds the sentinel on-screen
        // then glides in, identical to `click`. No-op for an empty (anonymous)
        // key or when the overlay is disabled for this cursor.
        crate::cursor::overlay::animate_cursor_to(cursor_id.clone(), x, y).await;
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."
        ))
    }
}
