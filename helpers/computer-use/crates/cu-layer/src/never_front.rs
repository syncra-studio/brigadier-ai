//! Agents never bring an app to the front and never move the user's pointer.
//!
//! The driver can act in front as a last resort (`delivery_mode:
//! "foreground"`, `bring_to_front`, `run_steps` with `foreground_fallback`)
//! and can send input in desktop coordinates, which moves the real pointer.
//! Each of these would interrupt the user, so the guard refuses them before
//! the tool runs, for a single call and for every `run_steps` step alike.

use cua_driver_core::protocol::ToolResult;
use serde_json::{json, Value};

/// The refusal agents get. It suggests only ways that work in the background.
pub const FOREGROUND_REFUSED: &str = "Not done: this needs the app in front, which would \
    interrupt the user. Find a way that works with the app in the background (another element, \
    set_value, the app's menu bar items, or the page tool in a browser), or tell the user what \
    they need to do themselves.";

/// The refusal code in `structuredContent`.
pub const FOREGROUND_REFUSED_CODE: &str = "foreground_refused";

/// Tools that always need the app in front or the user's pointer.
const ALWAYS_FRONT: &[&str] = &["bring_to_front", "drag"];

/// Fields a `run_steps` step may carry besides its one action.
const STEP_KEYS: &[&str] = &["tool", "args", "wait_for", "expect", "timeout_ms"];

/// The typed refusal.
pub fn refusal() -> ToolResult {
    ToolResult::error(FOREGROUND_REFUSED).with_structured(json!({
        "status": "refused",
        "code": FOREGROUND_REFUSED_CODE,
        "refusal": { "code": FOREGROUND_REFUSED_CODE, "message": FOREGROUND_REFUSED },
    }))
}

/// `Some(refusal)` when `tool` called with `args` would bring an app to the
/// front or move the user's pointer.
pub fn check(tool: &str, args: &Value) -> Option<ToolResult> {
    fronts(tool, args).then(refusal)
}

fn fronts(tool: &str, args: &Value) -> bool {
    if ALWAYS_FRONT.contains(&tool) || asks_foreground(args) || desktop_scoped(args) {
        return true;
    }
    if tool == "run_steps" {
        if args.get("foreground_fallback").is_some_and(truthy) {
            return true;
        }
        return steps(args).any(|(step_tool, step_args)| {
            let step_tool = step_tool.unwrap_or_default();
            step_tool.to_ascii_lowercase().contains("drag") || fronts(step_tool, step_args)
        });
    }
    false
}

/// `delivery_mode` (or its old name `dispatch`) set to foreground, in any case.
fn asks_foreground(args: &Value) -> bool {
    ["delivery_mode", "dispatch"].iter().any(|key| {
        args.get(key)
            .and_then(Value::as_str)
            .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("foreground"))
    })
}

/// Input in desktop coordinates: `scope:"desktop"`, `coordinate_frame:
/// "desktop"` (what the registry turns a desktop scope with a pid into) or a
/// `target` of kind desktop.
fn desktop_scoped(args: &Value) -> bool {
    let is_desktop = |value: Option<&Value>| {
        value
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("desktop"))
    };
    is_desktop(args.get("scope"))
        || is_desktop(args.get("coordinate_frame"))
        || is_desktop(args.get("target").and_then(|target| target.get("kind")))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(value) => *value,
        Value::String(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "true" | "1" | "yes"
        ),
        Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        _ => false,
    }
}

/// The steps of a `run_steps` call as `(tool, args)`, read as the batch
/// reads them: `{"tool": name, "args": {...}}` or the shorthand
/// `{name: {...}}`. A step whose tool can't be read yields `None` and the
/// step itself as args.
fn steps(args: &Value) -> impl Iterator<Item = (Option<&str>, &Value)> {
    args.get("steps")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|step| {
            if let Some(tool) = step.get("tool").and_then(Value::as_str) {
                return (Some(tool), step.get("args").unwrap_or(step));
            }
            step.as_object()
                .and_then(|object| {
                    object
                        .iter()
                        .find(|(key, value)| !STEP_KEYS.contains(&key.as_str()) && !value.is_null())
                })
                .map(|(key, value)| (Some(key.as_str()), value))
                .unwrap_or((None, step))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(tool: &str, args: Value) -> bool {
        let Some(result) = check(tool, &args) else {
            return false;
        };
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            result.structured_content.unwrap()["code"],
            FOREGROUND_REFUSED_CODE
        );
        true
    }

    #[test]
    fn bringing_an_app_forward_or_dragging_is_refused() {
        assert!(refused("bring_to_front", json!({"pid": 1})));
        assert!(refused("drag", json!({"pid": 1, "window_id": 2})));
        for mode in ["foreground", "Foreground", " FOREGROUND "] {
            assert!(refused("click", json!({"pid": 1, "delivery_mode": mode})));
            assert!(refused("type_text", json!({"pid": 1, "dispatch": mode})));
        }
        assert!(!refused(
            "click",
            json!({"pid": 1, "delivery_mode": "background"})
        ));
        assert!(!refused(
            "click",
            json!({"pid": 1, "element_token": "s00000001:3"})
        ));
    }

    #[test]
    fn desktop_input_is_refused() {
        assert!(refused(
            "move_cursor",
            json!({"x": 1, "y": 1, "scope": "desktop"})
        ));
        assert!(refused(
            "click",
            json!({"pid": 1, "x": 1, "y": 1, "coordinate_frame": "desktop"})
        ));
        assert!(refused(
            "click",
            json!({"target": {"kind": "desktop", "display_id": "primary"}, "x": 1, "y": 1})
        ));
        assert!(!refused("move_cursor", json!({"x": 1, "y": 1})));
        assert!(!refused(
            "click",
            json!({"target": {"kind": "window", "pid": 1, "window_id": 2}, "x": 1, "y": 1})
        ));
    }

    #[test]
    fn a_batch_that_would_front_is_refused_before_it_starts() {
        assert!(refused(
            "run_steps",
            json!({"pid": 1, "foreground_fallback": true, "steps": []})
        ));
        assert!(refused(
            "run_steps",
            json!({"pid": 1, "delivery_mode": "foreground", "steps": []})
        ));
        for step in [
            json!({"tool": "click", "args": {"delivery_mode": "foreground"}}),
            json!({"click": {"delivery_mode": "foreground"}}),
            json!({"drag": {"from_x": 1, "from_y": 1, "to_x": 2, "to_y": 2}}),
            json!({"tool": "drag", "args": {}}),
            json!({"move_cursor": {"x": 1, "y": 1, "scope": "desktop"}}),
            json!({"tool": "unknown", "delivery_mode": "foreground"}),
            json!({"mouse_drag": {"from_x": 1}}),
        ] {
            assert!(
                refused("run_steps", json!({"pid": 1, "steps": [step.clone()]})),
                "{step}"
            );
        }
        assert!(!refused(
            "run_steps",
            json!({"pid": 1, "foreground_fallback": false, "steps": [
                {"click": {"element_token": "s00000001:3"}},
                {"tool": "type_text", "args": {"text": "hi"}},
            ]})
        ));
    }
}
