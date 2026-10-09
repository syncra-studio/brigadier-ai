//! Action records (§5): one per action, with who, what, where, how and what came of it. In
//! Phase 1 the development harness writes them as JSON lines; from Phase 2 they become
//! session events in the event store.

use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionResult, Effect, Expect, Rung, Status, Timings};
use crate::desktop::WindowInfo;
use crate::error::ErrorCode;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        // Typed text is not kept in the record: it may be anything the worker was given. A
        // value predicate usually repeats it, so its text goes too.
        let mut action = action.clone();
        if let Action::Type { text, .. } | Action::SetValue { text, .. } = &mut action {
            *text = chars(text);
        }
        if let Some(Expect::ValueEquals { text, .. } | Expect::ValueContains { text, .. }) =
            action.expect_mut()
        {
            *text = chars(text);
        }
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

fn chars(text: &str) -> String {
    format!("<{} chars>", text.chars().count())
}
