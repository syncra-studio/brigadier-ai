//! Write-time redaction of tool arguments (invariant C5).
//!
//! Text an agent makes the machine type or inject is never stored in clear:
//! only a marker and its length survive. That covers `type_text`, `set_value`,
//! `page` insert/type/JavaScript, nested `run_steps` steps, the Codex
//! compat tools, and single printable keys (with no modifier other than
//! shift), which could otherwise spell a password one key at a time. Key
//! names, shortcuts with a command modifier, coordinates and element indices
//! stay in clear. Reserved `_`-prefixed daemon keys are dropped.

use serde_json::{json, Map, Value};

/// String fields whose value is replaced by `{redacted, length}` at any depth.
const TEXT_FIELDS: &[(&str, &str)] = &[
    ("text", "text"),
    ("value", "text"),
    ("javascript", "javascript"),
];

const COMMAND_MODIFIERS: &[&str] = &[
    "cmd", "command", "super", "meta", "ctrl", "control", "option", "alt", "fn",
];

/// User policy for what the log may keep (`activity_policy_set`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct RedactPolicy {
    /// Keep the first [`JAVASCRIPT_KEEP_CHARS`] characters of JavaScript
    /// passed to `page`. Typed text is never kept.
    #[serde(default)]
    pub store_javascript: bool,
}

/// Characters of JavaScript kept when `store_javascript` is on.
pub const JAVASCRIPT_KEEP_CHARS: usize = 4096;

/// Returns a copy of `args` that is safe to persist for `tool`.
pub fn redact_args(tool: &str, args: &Value) -> Value {
    redact_args_with(tool, args, RedactPolicy::default())
}

/// [`redact_args`] under a user policy.
pub fn redact_args_with(tool: &str, args: &Value, policy: RedactPolicy) -> Value {
    match args {
        Value::Object(map) => Value::Object(redact_object(tool, map, policy)),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| redact_args_with(tool, item, policy))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn redact_object(tool: &str, map: &Map<String, Value>, policy: RedactPolicy) -> Map<String, Value> {
    // A `run_steps` step (or any nested call) carries its own tool name:
    // `{tool, args}` (or `arguments`), or `{<tool>: args}`.
    if let Some(Value::String(step_tool)) = map.get("tool") {
        if map.contains_key("args") || map.contains_key("arguments") {
            let mut out = Map::new();
            for (key, value) in map {
                if key.starts_with('_') {
                    continue;
                }
                let tool = if key == "args" || key == "arguments" {
                    step_tool.as_str()
                } else {
                    tool
                };
                out.insert(key.clone(), redact_args_with(tool, value, policy));
            }
            return out;
        }
    }

    let printable_key = is_typed_key_call(tool, map);
    let mut out = Map::new();
    for (key, value) in map {
        if key.starts_with('_') {
            continue;
        }
        if key == "javascript" && policy.store_javascript {
            if let Value::String(text) = value {
                let kept: String = text.chars().take(JAVASCRIPT_KEEP_CHARS).collect();
                out.insert(key.clone(), Value::String(kept));
                continue;
            }
        }
        if let Some(marker) = text_marker(key) {
            if let Value::String(text) = value {
                out.insert(key.clone(), redacted(marker, text));
                continue;
            }
        }
        if printable_key && (key == "key" || key == "keys") {
            out.insert(key.clone(), json!({"redacted": "key"}));
            continue;
        }
        // `{<tool>: args}`: only the key tools redact by tool name.
        if (key == "press_key" || key == "hotkey") && value.is_object() {
            out.insert(key.clone(), redact_args_with(key, value, policy));
            continue;
        }
        out.insert(key.clone(), redact_args_with(tool, value, policy));
    }
    out
}

fn text_marker(key: &str) -> Option<&'static str> {
    TEXT_FIELDS
        .iter()
        .find(|(field, _)| *field == key)
        .map(|(_, marker)| *marker)
}

fn redacted(marker: &str, text: &str) -> Value {
    json!({"redacted": marker, "length": text.chars().count()})
}

/// Whether this call types one printable character (native `press_key`
/// `{key, modifiers}`, `hotkey` `{keys}`, or Codex `press_key` `{key:"shift+a"}`).
fn is_typed_key_call(tool: &str, map: &Map<String, Value>) -> bool {
    match tool {
        "press_key" => {
            let Some(Value::String(key)) = map.get("key") else {
                return false;
            };
            let mut parts: Vec<String> = split_combo(key);
            if let Some(Value::Array(modifiers)) = map.get("modifiers") {
                parts.extend(
                    modifiers
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_ascii_lowercase),
                );
            }
            combo_types_character(&parts)
        }
        "hotkey" => {
            let Some(Value::Array(keys)) = map.get("keys") else {
                return false;
            };
            let parts: Vec<String> = keys
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            combo_types_character(&parts)
        }
        _ => false,
    }
}

/// Splits `"shift+a"` into parts. A lone `"+"` is the plus key itself.
fn split_combo(key: &str) -> Vec<String> {
    if key.chars().count() == 1 {
        return vec![key.to_owned()];
    }
    let mut parts: Vec<String> = Vec::new();
    for piece in key.split('+') {
        if piece.is_empty() {
            // `ctrl++` or a trailing `+`: the plus key.
            if parts.last().map(String::as_str) != Some("+") {
                parts.push("+".to_owned());
            }
            continue;
        }
        parts.push(piece.to_owned());
    }
    parts
}

fn combo_types_character(parts: &[String]) -> bool {
    let mut printable = 0usize;
    for part in parts {
        let lower = part.to_ascii_lowercase();
        if COMMAND_MODIFIERS.contains(&lower.as_str()) {
            return false;
        }
        if lower == "shift" {
            continue;
        }
        let mut chars = part.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => printable += 1,
            _ => return false, // a key name such as `return`, `tab`, `space`
        }
    }
    printable == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_string(value: &Value, needle: &str) -> bool {
        match value {
            Value::String(s) => s.contains(needle),
            Value::Array(items) => items.iter().any(|v| contains_string(v, needle)),
            Value::Object(map) => map
                .iter()
                .any(|(k, v)| k.contains(needle) || contains_string(v, needle)),
            _ => false,
        }
    }

    #[test]
    fn type_text_is_length_only() {
        let out = redact_args(
            "type_text",
            &json!({"pid": 7, "text": "hunter22", "_session_id": "x"}),
        );
        assert_eq!(
            out,
            json!({"pid": 7, "text": {"redacted": "text", "length": 8}})
        );
    }

    #[test]
    fn set_value_and_codex_tools_are_redacted() {
        let out = redact_args("set_value", &json!({"element_index": 3, "value": "secret"}));
        assert_eq!(out["value"], json!({"redacted": "text", "length": 6}));
        let out = redact_args("type_text", &json!({"app": "Notes", "text": "påss"}));
        assert_eq!(out["text"], json!({"redacted": "text", "length": 4}));
        assert_eq!(out["app"], json!("Notes"));
    }

    #[test]
    fn page_insert_and_javascript_are_redacted() {
        let out = redact_args("page", &json!({"action": "insert_text", "text": "pw"}));
        assert_eq!(out["text"]["redacted"], json!("text"));
        let out = redact_args(
            "page",
            &json!({"action": "execute_javascript", "javascript": "el.value='pw'"}),
        );
        assert_eq!(
            out["javascript"],
            json!({"redacted": "javascript", "length": 13})
        );
        assert_eq!(out["action"], json!("execute_javascript"));
    }

    #[test]
    fn javascript_policy_keeps_a_prefix_but_never_typed_text() {
        let policy = RedactPolicy {
            store_javascript: true,
        };
        let long = "x".repeat(JAVASCRIPT_KEEP_CHARS + 10);
        let out = redact_args_with("page", &json!({"javascript": long, "text": "pw"}), policy);
        assert_eq!(
            out["javascript"].as_str().unwrap().len(),
            JAVASCRIPT_KEEP_CHARS
        );
        assert_eq!(out["text"]["redacted"], json!("text"));
    }

    #[test]
    fn run_steps_steps_use_their_own_tool() {
        let args = json!({"steps": [
            {"tool": "type_text", "args": {"text": "abc", "pid": 1}},
            {"tool": "press_key", "args": {"key": "x", "pid": 1}},
            {"tool": "press_key", "arguments": {"key": "return", "pid": 1}},
            {"tool": "click", "args": {"x": 10, "y": 20}},
            {"press_key": {"key": "y"}, "expect": [{"text": "Thanks"}]},
            {"hotkey": {"keys": ["cmd", "z"]}},
            {"set_value": {"role": "textfield", "value": "ada@example.com"}}
        ], "observe": {}});
        let out = redact_args("run_steps", &args);
        let steps = &out["steps"];
        assert_eq!(steps[0]["args"]["text"]["length"], json!(3));
        assert_eq!(steps[1]["args"]["key"], json!({"redacted": "key"}));
        assert_eq!(steps[2]["arguments"]["key"], json!("return"));
        assert_eq!(steps[3]["args"], json!({"x": 10, "y": 20}));
        assert_eq!(steps[4]["press_key"]["key"], json!({"redacted": "key"}));
        assert_eq!(steps[5]["hotkey"]["keys"], json!(["cmd", "z"]));
        assert_eq!(steps[6]["set_value"]["value"]["length"], json!(15));
        assert!(!contains_string(&out, "ada@example.com"));
    }

    #[test]
    fn printable_keys_are_redacted_and_shortcuts_are_not() {
        let cases = [
            (json!({"key": "a"}), true),
            (json!({"key": "A", "modifiers": ["shift"]}), true),
            (json!({"key": "c", "modifiers": ["cmd"]}), false),
            (json!({"key": "return"}), false),
            (json!({"key": "shift+a"}), true),
            (json!({"key": "ctrl+a"}), false),
            (json!({"key": "+"}), true),
            (json!({"key": "ctrl++"}), false),
        ];
        for (args, masked) in cases {
            let out = redact_args("press_key", &args);
            assert_eq!(out["key"] == json!({"redacted": "key"}), masked, "{args}");
        }
        let out = redact_args("hotkey", &json!({"keys": ["cmd", "c"]}));
        assert_eq!(out["keys"], json!(["cmd", "c"]));
        let out = redact_args("hotkey", &json!({"keys": ["shift", "q"]}));
        assert_eq!(out["keys"], json!({"redacted": "key"}));
    }

    #[test]
    fn no_typed_text_survives_random_inputs() {
        // Seeded generator: printable secrets placed in every redacted field
        // at random depths must never appear in the output.
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..500 {
            let secret = format!("S3cr{:x}t{}", next(), round);
            let field = ["text", "value", "javascript"][(next() % 3) as usize];
            let tool =
                ["type_text", "set_value", "page", "run_steps", "click"][(next() % 5) as usize];
            let mut inner = Map::new();
            inner.insert(field.to_owned(), Value::String(secret.clone()));
            inner.insert("pid".to_owned(), json!(next() % 1000));
            let mut args = Value::Object(inner);
            for _ in 0..(next() % 3) {
                args = json!({"actions": [{"tool": tool, "arguments": args}], "note": [1, 2]});
            }
            let out = redact_args(tool, &args);
            assert!(!contains_string(&out, &secret), "leaked {secret} in {out}");
        }
    }
}
