//! Action records (§5): one per action, with who, what, where, how and what came of it. In
//! Phase 1 the development harness writes them as JSON lines; from Phase 2 they become
//! session events in the event store.

use serde::Serialize;

use crate::action::{Action, ActionResult, Effect, Rung, Status, Timings};
use crate::desktop::WindowInfo;
use crate::error::ErrorCode;

#[derive(Debug, Clone, Serialize)]
pub struct ActionRecord {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub worker: String,
    pub pid: i32,
    pub window: u32,
    pub window_title: String,
    pub action: Action,
    pub status: Status,
    pub rung: Option<Rung>,
    pub effect: Option<Effect>,
    pub error: Option<ErrorCode>,
    pub timings: Timings,
    /// The user's frontmost app, its key window and the cursor were the same after.
    pub user_focus_kept: bool,
}

impl ActionRecord {
    pub fn new(
        worker: &str,
        w: &WindowInfo,
        action: &Action,
        r: &ActionResult,
        user_focus_kept: bool,
    ) -> Self {
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        // Typed text is not kept in the record: it may be anything the worker was given.
        let action = match action {
            Action::Type {
                r#ref,
                expect,
                text,
            } => Action::Type {
                text: format!("<{} chars>", text.chars().count()),
                r#ref: r#ref.clone(),
                expect: expect.clone(),
            },
            Action::SetValue {
                r#ref,
                expect,
                text,
            } => Action::SetValue {
                text: format!("<{} chars>", text.chars().count()),
                r#ref: r#ref.clone(),
                expect: expect.clone(),
            },
            other => other.clone(),
        };
        Self {
            at_ms,
            worker: worker.to_owned(),
            pid: w.pid,
            window: w.id,
            window_title: w.title.clone(),
            action,
            status: r.status,
            rung: r.delivered,
            effect: r.effect,
            error: r.error.as_ref().map(|e| e.code),
            timings: r.timings,
            user_focus_kept,
        }
    }
}
