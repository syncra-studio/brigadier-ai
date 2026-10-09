//! Action records (§5): one per action, with who, what, where, how and what came of it. In
//! Phase 1 the development harness writes them as JSON lines; from Phase 2 they become
//! session events in the event store.

use serde::{Deserialize, Serialize};

use crate::action::{Action, ActionResult, Effect, Expect, Rung, Status, Timings};
use crate::desktop::WindowInfo;
use crate::error::ErrorCode;
use crate::geom::{Point, Rect};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionRecord {
    /// Milliseconds since the Unix epoch.
    pub at_ms: u64,
    pub worker: String,
    /// The action's place in its batch, from 0.
    #[serde(default)]
    pub index: usize,
    pub pid: i32,
    pub window: u32,
    pub window_title: String,
    /// The app's name.
    #[serde(default)]
    pub app: String,
    /// What it aimed at, in words, when it named something: `button "Save"`, a menu path, a
    /// key chord. A point has none: `point` says where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub action: Action,
    pub status: Status,
    pub rung: Option<Rung>,
    pub effect: Option<Effect>,
    pub error: Option<ErrorCode>,
    /// The error's detail. Left out where it could repeat a value the action read back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub timings: Timings,
    /// The user's frontmost app, its key window and the cursor were the same after.
    pub user_focus_kept: bool,
    /// Where the action aimed, in window points, and the element's box when it named one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_box: Option<Rect>,
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
        let mut kept_out = Vec::new();
        if let Action::Type { text, .. } | Action::SetValue { text, .. } = &mut action {
            kept_out.push(std::mem::replace(text, chars(text)));
        }
        if let Some(Expect::ValueEquals { text, .. } | Expect::ValueContains { text, .. }) =
            action.expect_mut()
        {
            kept_out.push(std::mem::replace(text, chars(text)));
        }
        Self {
            at_ms,
            worker: worker.to_owned(),
            index: r.index,
            pid: w.pid,
            window: w.id,
            window_title: w.title.clone(),
            app: String::new(),
            target: None,
            action,
            status: r.status,
            rung: r.delivered,
            effect: r.effect,
            error: r.error.as_ref().map(|e| e.code),
            detail: r
                .error
                .as_ref()
                .filter(|e| e.code != ErrorCode::NotSettable)
                .map(|e| without(&e.detail, &kept_out)),
            timings: r.timings,
            user_focus_kept,
            point: None,
            element_box: None,
        }
    }
}

fn chars(text: &str) -> String {
    format!("<{} chars>", text.chars().count())
}

/// The error's detail with any of the action's kept-out texts replaced by their length: an error
/// may quote what it was given (`"abc" is not a number`).
fn without(detail: &str, kept_out: &[String]) -> String {
    let mut d = detail.to_owned();
    for t in kept_out.iter().filter(|t| !t.is_empty()) {
        d = d
            .replace(&format!("{t:?}"), &chars(t))
            .replace(t.as_str(), &chars(t));
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_error_quoting_the_given_text_keeps_only_its_length() {
        let kept_out = vec!["s3cret".to_owned()];
        assert_eq!(
            without("\"s3cret\" is not a number", &kept_out),
            "<6 chars> is not a number"
        );
        assert_eq!(
            without("no s3cret here, s3cret", &kept_out),
            "no <6 chars> here, <6 chars>"
        );
        assert_eq!(without("the field is gone", &[]), "the field is gone");
    }
}
