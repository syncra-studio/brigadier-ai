//! Tool input schemas both CLIs accept.
//!
//! The schemas come from the argument types in `brigadier_core::tools` (field doc comments are
//! the descriptions), then are flattened into the plain JSON Schema subset that Claude Code and
//! Codex both handle: no `$ref`/`$defs`, no `oneOf`/`anyOf`, one `type` per property, no
//! non-standard `format`s. Optional fields are simply left out of `required`.

use rmcp::model::JsonObject;
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};

/// The input schema of a tool taking `T` as its arguments.
pub fn input_schema<T: JsonSchema>() -> JsonObject {
    let generator = SchemaSettings::draft07()
        .with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        })
        .into_generator();
    let mut schema = generator.into_root_schema_for::<T>().to_value();
    simplify(&mut schema);
    let mut object = match schema {
        Value::Object(object) => object,
        _ => Map::new(),
    };
    // The tool description says what the tool does; the struct's doc comment would repeat it.
    object.remove("title");
    object.remove("description");
    object.remove("definitions");
    object.remove("$defs");
    object
        .entry("properties")
        .or_insert_with(|| Value::Object(Map::new()));
    object
}

/// The schema of a tool without arguments.
pub fn no_arguments() -> JsonObject {
    let mut object = Map::new();
    object.insert("type".into(), "object".into());
    object.insert("properties".into(), Value::Object(Map::new()));
    object.insert("additionalProperties".into(), false.into());
    object
}

fn simplify(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for child in object.values_mut() {
                simplify(child);
            }
            simplify_object(object);
        }
        Value::Array(items) => items.iter_mut().for_each(simplify),
        _ => {}
    }
}

fn simplify_object(object: &mut Map<String, Value>) {
    // `Option<T>`: `"type": ["string", "null"]` → `"type": "string"`.
    if let Some(Value::Array(types)) = object.get("type") {
        let types: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
        if let [only] = types.as_slice() {
            object.insert("type".into(), only.clone());
        }
    }
    // `Option<Enum>`: `anyOf: [<schema>, {"type": "null"}]` → `<schema>`.
    for key in ["anyOf", "oneOf"] {
        let Some(Value::Array(variants)) = object.get(key) else {
            continue;
        };
        let variants: Vec<Value> = variants
            .iter()
            .filter(|variant| variant.get("type") != Some(&Value::from("null")))
            .cloned()
            .collect();
        object.remove(key);
        match variants.as_slice() {
            [Value::Object(only)] => merge(object, only),
            _ => {
                if let Some(merged) = string_enum(&variants) {
                    merge(object, &merged);
                } else {
                    object.insert(key.into(), Value::Array(variants));
                }
            }
        }
    }
    // A unit enum with a single variant.
    if let Some(constant) = object.remove("const") {
        object.insert("enum".into(), Value::Array(vec![constant]));
    }
    // `Option<Enum>` can also come out as `"enum": [..., null]`.
    if let Some(Value::Array(values)) = object.get_mut("enum") {
        values.retain(|value| !value.is_null());
    }
    // Doc comments are wrapped at 100 columns; the wrapping is not part of the text.
    if let Some(Value::String(description)) = object.get_mut("description") {
        *description = unwrap_lines(description);
    }
    if object.contains_key("enum") && !object.contains_key("type") {
        object.insert("type".into(), "string".into());
    }
    if object.get("default") == Some(&Value::Null) {
        object.remove("default");
    }
    // `uint32`, `uint64`, …: `minimum` already says what matters.
    if object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format.starts_with("int") || format.starts_with("uint"))
    {
        object.remove("format");
    }
}

/// `oneOf` over documented unit variants (`{"const": "scout", "description": "…"}`) as one
/// string enum whose description lists the variants.
fn string_enum(variants: &[Value]) -> Option<Map<String, Value>> {
    let mut values = Vec::with_capacity(variants.len());
    let mut lines = Vec::with_capacity(variants.len());
    for variant in variants {
        let variant = variant.as_object()?;
        let value = variant
            .get("const")
            .or_else(|| match variant.get("enum") {
                Some(Value::Array(values)) if values.len() == 1 => values.first(),
                _ => None,
            })?
            .as_str()?;
        values.push(Value::from(value));
        match variant.get("description").and_then(Value::as_str) {
            Some(description) => lines.push(format!("`{value}`: {}", unwrap_lines(description))),
            None => lines.push(format!("`{value}`")),
        }
    }
    let mut merged = Map::new();
    merged.insert("type".into(), "string".into());
    merged.insert("enum".into(), Value::Array(values));
    merged.insert("description".into(), lines.join(" ").into());
    Some(merged)
}

/// Copies `from` into `into`; a description on both is joined, the outer one first.
fn merge(into: &mut Map<String, Value>, from: &Map<String, Value>) {
    for (key, value) in from {
        match (key.as_str(), into.get(key)) {
            ("description", Some(Value::String(outer))) => {
                if let Value::String(inner) = value {
                    let joined = format!("{outer} {inner}");
                    into.insert(key.clone(), joined.into());
                }
            }
            (_, Some(_)) => {}
            _ => {
                into.insert(key.clone(), value.clone());
            }
        }
    }
}

/// Joins lines broken by comment wrapping; blank lines (paragraphs) stay.
fn unwrap_lines(text: &str) -> String {
    text.split("\n\n")
        .map(|paragraph| {
            paragraph
                .lines()
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}
