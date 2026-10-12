//! Mirrors posted native drag events without planning an independent animation.

use super::cursor_tools;
use crate::cursor::CursorRegistry;
use core_graphics::event::{CGEvent, CGEventType};
use cursor_overlay::OverlayCommand;
use serde_json::Value;
use std::sync::Arc;

pub(super) struct DragCursor {
    registry: Arc<CursorRegistry>,
    key: String,
    request_session: Option<String>,
}

impl DragCursor {
    pub(super) fn new(registry: Arc<CursorRegistry>, args: &Value) -> Self {
        Self {
            registry,
            key: cursor_tools::resolve_cursor_key(args),
            request_session: args
                .get("session")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }
    }

    pub(super) fn observe(&self, event: &CGEvent) {
        if self.key.is_empty()
            || self
                .request_session
                .as_deref()
                .is_some_and(cu_core::session::is_session_ended)
        {
            return;
        }
        let pressed = match event.get_type() {
            CGEventType::LeftMouseDown
            | CGEventType::RightMouseDown
            | CGEventType::OtherMouseDown
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged => true,
            CGEventType::MouseMoved
            | CGEventType::LeftMouseUp
            | CGEventType::RightMouseUp
            | CGEventType::OtherMouseUp => false,
            _ => return,
        };
        let point = event.location();
        // Samples already carry the real gesture pacing. Snapping each sample
        // avoids trailing the divider or continuing toward an unposted target
        // after cancellation. The renderer retains its click-through ordering.
        crate::cursor::overlay::send_command(
            self.key.clone(),
            OverlayCommand::DragTo {
                x: point.x,
                y: point.y,
                pressed,
            },
        );
        crate::cursor::overlay::publish_cursor_position(&self.key, point.x, point.y, true);
        self.registry.update_position(&self.key, point.x, point.y);
        if matches!(
            event.get_type(),
            CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp
        ) {
            // This barrier runs only after native capture has been released.
            cu_core::cursor_feed::flush();
        }
    }
}
