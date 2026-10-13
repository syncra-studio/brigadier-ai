//! What agents read: tool descriptions and results in Brigadier's words.
//!
//! The driver's own text sometimes suggests acting in front (a
//! `delivery_mode` of foreground, `bring_to_front`, a `foreground_fallback`
//! batch) or desktop input that moves the user's pointer. Brigadier refuses
//! all of those ([`crate::never_front`]), so the sentences that suggest them
//! are removed from every description and result, and the driver's name is
//! replaced by Brigadier's. Content an app shows (window trees, element
//! labels and values, page text) is never changed.

use serde_json::{Map, Value};

use crate::catalog;

/// Words that mark a sentence as suggesting the foreground.
const FRONT_WORDS: &[&str] = &[
    "foreground",
    "bring_to_front",
    "delivery_mode",
    "bring it to the front",
    "bring the app to the front",
    "bring the window to the front",
];

/// Words that mark a sentence as describing desktop input (the user's
/// pointer, desktop coordinates). Only used for tool descriptions, where
/// `get_desktop_state` and other plain mentions of the desktop stay.
const DESKTOP_INPUT_WORDS: &[&str] = &[
    "scope=desktop",
    "scope:\"desktop\"",
    "scope: \"desktop\"",
    "\"desktop\":",
    "kind \"desktop\"",
    "kind: \"desktop\"",
    "kind:\"desktop\"",
    "desktop scope",
    "desktop-scope",
    "desktop target",
    "real os pointer",
    "real pointer",
    "display_id",
    "coordinate_frame",
];

/// Arguments agents never send: Brigadier always acts in the background.
const HIDDEN_ARGS: &[&str] = &["delivery_mode", "dispatch", "foreground_fallback"];

/// Structured result fields that hold what an app shows, never rewritten.
const DATA_KEYS: &[&str] = &[
    "tree_markdown",
    "elements",
    "tree",
    "text",
    "value",
    "values",
    "title",
    "label",
    "name",
    "role",
    "html",
    "url",
    "urls",
    "result",
    "app_name",
    "apps",
    "windows",
    "items",
    "menu",
    "path",
    "selected_text",
    "screenshot",
    "image",
];

/// Structured fields that only exist to suggest acting in front.
const FRONT_KEYS: &[&str] = &["escalation", "foreground_hint", "delivery_fallback"];

fn mentions(text: &str, words: &[&str]) -> bool {
    let lower = text.to_ascii_lowercase();
    words.iter().any(|word| lower.contains(word))
}

/// Split `line` into sentences, each with its closing punctuation and the
/// space after it.
fn sentences(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = line.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        let ends = matches!(byte, b'.' | b'!' | b'?' | b';')
            && bytes.get(index + 1).is_some_and(|next| *next == b' ');
        if ends {
            out.push(&line[start..index + 2]);
            start = index + 2;
        }
    }
    if start < line.len() {
        out.push(&line[start..]);
    }
    out
}

/// Remove the sentences of `text` that mention any of `words`, line by line.
/// A line left empty is dropped.
fn drop_sentences(text: &str, words: &[&str]) -> String {
    if !mentions(text, words) {
        return text.to_owned();
    }
    let mut lines = Vec::new();
    for line in text.split('\n') {
        if !mentions(line, words) {
            lines.push(line.to_owned());
            continue;
        }
        if is_tree_row(line) {
            lines.push(line.to_owned());
            continue;
        }
        let parts = sentences(line);
        let mut kept = String::new();
        for (index, sentence) in parts.iter().enumerate() {
            if mentions(sentence, words) {
                continue;
            }
            // A clause cut short by a dropped one after its `;` ends there.
            let next_dropped = parts
                .get(index + 1)
                .is_some_and(|next| mentions(next, words));
            match sentence.trim_end().strip_suffix(';') {
                Some(clause) if next_dropped || index + 1 == parts.len() => {
                    kept.push_str(clause);
                    kept.push_str(". ");
                }
                _ => kept.push_str(sentence),
            }
        }
        let kept = kept.trim_end();
        if !kept.trim().is_empty() {
            lines.push(kept.to_owned());
        }
    }
    lines.join("\n")
}

/// A row of a window's element tree (`- [12] AXButton "…"`, or a diff row
/// `+ [3] …`): what the app shows, kept as it is.
fn is_tree_row(line: &str) -> bool {
    let trimmed = line
        .trim_start()
        .trim_start_matches(['-', '+', '~', '*', ' ']);
    trimmed.starts_with('[')
        && trimmed[1..]
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_digit())
}

/// Brigadier's names for the driver's.
pub fn rename(text: &str) -> String {
    if !text.contains("Cua") && !text.contains("cua-driver") && !text.contains("CUA") {
        return text.to_owned();
    }
    text.replace("Cua Driver", "Brigadier Computer Use")
        .replace("Cua driver", "Brigadier Computer Use")
        .replace("cua-driver", "brigadier-cu")
        .replace("Cua runtime", "computer use helper")
        .replace("Cua ", "Brigadier ")
}

/// A description or message in Brigadier's words, with every sentence that
/// suggests the foreground removed.
pub fn prose(text: &str) -> String {
    rename(&drop_sentences(text, FRONT_WORDS))
}

/// [`prose`] for tool descriptions, which also lose desktop-input sentences
/// and every mention of a tool Brigadier doesn't offer.
fn description(text: &str) -> String {
    let text = drop_sentences(&drop_sentences(text, FRONT_WORDS), DESKTOP_INPUT_WORDS);
    rename(&drop_sentences(&text, &withheld_mentions()).replace(", drag,", ","))
}

/// How descriptions name tools agents don't get: the withheld ones in
/// backticks, and one the driver names but doesn't have.
fn withheld_mentions() -> Vec<&'static str> {
    static WORDS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    WORDS
        .get_or_init(|| {
            catalog::WITHHELD
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .chain(["type_text_chars".to_owned()])
                .collect()
        })
        .iter()
        .map(String::as_str)
        .collect()
}

/// Rewrite one tool result for agents.
pub fn result(
    mut result: cua_driver_core::protocol::ToolResult,
) -> cua_driver_core::protocol::ToolResult {
    use cua_driver_core::protocol::Content;
    for item in &mut result.content {
        if let Content::Text { text, .. } = item {
            *text = prose(text);
        }
    }
    result.content.retain(|item| match item {
        Content::Text { text, .. } => !text.trim().is_empty(),
        _ => true,
    });
    if let Some(structured) = result.structured_content.as_mut() {
        structured_prose(structured);
    }
    result
}

fn structured_prose(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.retain(|key, value| !is_front_field(key, value));
            for (key, value) in object.iter_mut() {
                if DATA_KEYS.contains(&key.as_str()) {
                    continue;
                }
                structured_prose(value);
            }
            object.retain(|_, value| !matches!(value, Value::String(text) if text.is_empty()));
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                structured_prose(item);
            }
            items.retain(|item| !matches!(item, Value::String(text) if text.is_empty()));
        }
        Value::String(text) => {
            let rewritten = prose(text);
            *text = if rewritten.trim().is_empty() {
                String::new()
            } else {
                rewritten
            };
        }
        _ => {}
    }
}

fn is_front_field(key: &str, value: &Value) -> bool {
    if FRONT_KEYS.contains(&key) {
        return true;
    }
    value
        .as_str()
        .is_some_and(|text| text.trim().eq_ignore_ascii_case("foreground"))
}

/// The driver's `tools/list` result as agents see it: only offered tools,
/// without the arguments and enum values that would act in front or on the
/// desktop, and with every description in Brigadier's words.
pub fn tools_list(mut list: Value) -> Value {
    // The driver's own authorization inventory: not for agents, and it names
    // tools Brigadier withholds.
    if let Some(object) = list.as_object_mut() {
        object.remove("enforcement_adapters");
    }
    if let Some(tools) = list.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .is_some_and(catalog::offered)
        });
        for tool in tools.iter_mut() {
            tool_entry(tool);
        }
    }
    list
}

fn tool_entry(tool: &mut Value) {
    let Some(object) = tool.as_object_mut() else {
        return;
    };
    if let Some(Value::String(text)) = object.get_mut("description") {
        *text = description(text);
    }
    if let Some(Value::Array(capabilities)) = object.get_mut("capabilities") {
        capabilities.retain(|capability| {
            capability
                .as_str()
                .is_none_or(|capability| !mentions(capability, FRONT_WORDS))
        });
    }
    for key in ["inputSchema", "outputSchema"] {
        if let Some(schema) = object.get_mut(key) {
            schema_prose(schema);
        }
    }
}

fn schema_prose(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if let Some(Value::Object(properties)) = object.get_mut("properties") {
                hide_args(properties);
            }
            if let Some(Value::Array(required)) = object.get_mut("required") {
                required.retain(|name| {
                    name.as_str()
                        .is_none_or(|name| !HIDDEN_ARGS.contains(&name) && name != "display_id")
                });
            }
            if let Some(Value::Array(values)) = object.get_mut("enum") {
                values.retain(|value| {
                    value.as_str().is_none_or(|value| {
                        !mentions(value, FRONT_WORDS) && !value.eq_ignore_ascii_case("desktop")
                    })
                });
                // A list of tools (run_steps' step tools) offers only Brigadier's.
                let names_tools = values
                    .iter()
                    .any(|value| value.as_str().is_some_and(|name| name == "click"));
                if names_tools {
                    values.retain(|value| {
                        value.as_str().is_none_or(|name| {
                            !catalog::WITHHELD
                                .iter()
                                .any(|(withheld, _)| *withheld == name)
                        })
                    });
                }
            }
            for (key, value) in object.iter_mut() {
                match (key.as_str(), value) {
                    ("description", Value::String(text)) => *text = description(text),
                    (_, value) => schema_prose(value),
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(schema_prose),
        _ => {}
    }
}

fn hide_args(properties: &mut Map<String, Value>) {
    for name in HIDDEN_ARGS {
        properties.remove(*name);
    }
    properties.remove("display_id");
    // Only the window forms of these stay, so say just that.
    if let Some(Value::Object(scope)) = properties.get_mut("scope") {
        scope.insert(
            "description".to_owned(),
            Value::from("Always \"window\" (the default): coordinates are the window's."),
        );
    }
    if let Some(Value::Object(target)) = properties.get_mut("target") {
        target.insert(
            "description".to_owned(),
            Value::from("An exact window: kind \"window\" with pid and window_id."),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::protocol::{Content, ToolResult};
    use serde_json::json;

    #[test]
    fn foreground_suggestions_are_removed_and_the_rest_kept() {
        let text = "Typed 3 characters. If the field stays empty, re-call with \
                    delivery_mode:\"foreground\". Check the screenshot.";
        assert_eq!(prose(text), "Typed 3 characters. Check the screenshot.");
        assert_eq!(
            prose("Re-send with delivery_mode:\"foreground\"."),
            "",
            "a line that only suggests the foreground goes away"
        );
        assert_eq!(
            prose("Clicked.\nUse bring_to_front first.\nDone."),
            "Clicked.\nDone."
        );
        assert_eq!(
            prose("It moves there; with delivery_mode it fronts. Then reads."),
            "It moves there. Then reads."
        );
        assert_eq!(
            prose("At most 64 steps; foreground is off."),
            "At most 64 steps."
        );
    }

    #[test]
    fn what_an_app_shows_is_never_changed() {
        let tree = "- [3] AXButton \"Foreground colour\"\n+ [4] AXStaticText \"bring_to_front\"";
        assert_eq!(prose(tree), tree);
        let mut structured = json!({
            "tree_markdown": "Foreground. delivery_mode",
            "elements": [{"label": "Foreground"}],
            "hint": "Act with delivery_mode:\"foreground\". Or read again.",
            "escalation": {"recommended": "foreground", "reason": "x"},
            "advice": "foreground",
            "message": "Cua Driver refused it.",
        });
        structured_prose(&mut structured);
        assert_eq!(structured["tree_markdown"], "Foreground. delivery_mode");
        assert_eq!(structured["elements"][0]["label"], "Foreground");
        assert_eq!(structured["hint"], "Or read again.");
        assert!(structured.get("escalation").is_none());
        assert!(structured.get("advice").is_none());
        assert_eq!(structured["message"], "Brigadier Computer Use refused it.");
    }

    #[test]
    fn a_result_keeps_its_images_and_loses_empty_text() {
        let original = ToolResult {
            content: vec![
                Content::text("Use delivery_mode:\"foreground\"."),
                Content::image_png("AAAA".into()),
                Content::text("Done."),
            ],
            is_error: Some(true),
            ..Default::default()
        };
        let rewritten = result(original);
        assert_eq!(rewritten.content.len(), 2);
        assert_eq!(rewritten.is_error, Some(true));
    }

    #[test]
    fn tools_list_offers_only_background_tools_and_arguments() {
        let list = json!({
            "tools": [
                {
                    "name": "click",
                    "description": "Click an element. Use delivery_mode:\"foreground\" as a last resort. With scope=desktop, x,y are screen pixels.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "delivery_mode": {"type": "string", "enum": ["background", "foreground"]},
                            "scope": {"type": "string", "enum": ["window", "desktop"],
                                      "description": "Default \"window\". \"desktop\": x,y are true screen pixels."},
                            "target": {"type": "object", "properties": {
                                "kind": {"enum": ["window", "desktop"]},
                                "display_id": {"type": "string"}
                            }},
                            "element_token": {"type": "string"}
                        },
                        "required": ["delivery_mode"]
                    }
                },
                {"name": "bring_to_front", "description": "x", "inputSchema": {}},
                {"name": "run_steps", "description": "Batch. Each tool is one of click, drag, scroll and type_text. To paste, call `clipboard_write` first.", "inputSchema": {"properties": {
                    "foreground_fallback": {"type": "boolean"}, "steps": {"type": "array", "items": {
                        "properties": {"tool": {"enum": ["click", "drag", "scroll"]}}}}}}}
            ]
        });
        let list = tools_list(list);
        let tools = list["tools"].as_array().unwrap();
        let names: Vec<_> = tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["click", "run_steps"]);
        let click = &tools[0];
        assert_eq!(click["description"], "Click an element.");
        let properties = &click["inputSchema"]["properties"];
        assert!(properties.get("delivery_mode").is_none());
        assert_eq!(properties["scope"]["enum"], json!(["window"]));
        assert!(properties["scope"]["description"]
            .as_str()
            .unwrap()
            .starts_with("Always \"window\""));
        assert_eq!(
            properties["target"]["properties"]["kind"]["enum"],
            json!(["window"])
        );
        assert!(properties["target"]["properties"]
            .get("display_id")
            .is_none());
        assert_eq!(click["inputSchema"]["required"], json!([]));
        assert!(tools[1]["inputSchema"]["properties"]
            .get("foreground_fallback")
            .is_none());
        assert_eq!(
            tools[1]["description"],
            "Batch. Each tool is one of click, scroll and type_text."
        );
        assert_eq!(
            tools[1]["inputSchema"]["properties"]["steps"]["items"]["properties"]["tool"]["enum"],
            json!(["click", "scroll"])
        );
    }
}
