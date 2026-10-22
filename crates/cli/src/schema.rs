//! Checks a JSON value against one of the checked-in schemas: the subset of JSON Schema they
//! use (`type`, `properties`, `required`, `additionalProperties: false`, `items`, `enum`,
//! `minimum`, `const`, and `$ref` into `$defs`). Anything else in a schema is an error, so a
//! schema can't silently say more than is checked.

use serde_json::{Map, Value};

/// Every way `value` breaks `schema` (the schema file's text), as `path: problem` lines.
pub fn violations(schema: &str, value: &Value) -> Vec<String> {
    let root: Value = match serde_json::from_str(schema) {
        Ok(root) => root,
        Err(err) => return vec![format!("the schema is not JSON: {err}")],
    };
    let mut found = Vec::new();
    check(&root, &root, value, "$", &mut found);
    found
}

const KNOWN: &[&str] = &[
    "$schema",
    "$id",
    "$defs",
    "$ref",
    "title",
    "description",
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "enum",
    "const",
    "minimum",
];

fn check(root: &Value, schema: &Value, value: &Value, path: &str, found: &mut Vec<String>) {
    let Some(schema) = schema.as_object() else {
        found.push(format!("{path}: the schema here is not an object"));
        return;
    };
    for key in schema.keys() {
        if !KNOWN.contains(&key.as_str()) {
            found.push(format!(
                "{path}: the schema uses {key:?}, which isn't checked"
            ));
        }
    }
    if let Some(reference) = schema.get("$ref") {
        match resolve(root, reference) {
            Some(target) => check(root, target, value, path, found),
            None => found.push(format!("{path}: unknown $ref {reference}")),
        }
        return;
    }
    if let Some(types) = schema.get("type") {
        let types: Vec<&str> = match types {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            _ => Vec::new(),
        };
        if !types.iter().any(|name| is_type(value, name)) {
            found.push(format!(
                "{path}: {} is not {}",
                kind(value),
                types.join(" or ")
            ));
            return;
        }
    }
    if let Some(Value::Array(allowed)) = schema.get("enum")
        && !allowed.contains(value)
    {
        found.push(format!(
            "{path}: {value} is not one of {}",
            Value::from(allowed.clone())
        ));
    }
    if let Some(expected) = schema.get("const")
        && expected != value
    {
        found.push(format!("{path}: {value} is not {expected}"));
    }
    if let (Some(minimum), Some(number)) = (
        schema.get("minimum").and_then(Value::as_f64),
        value.as_f64(),
    ) && number < minimum
    {
        found.push(format!("{path}: {number} is below {minimum}"));
    }
    if let Value::Object(object) = value {
        check_object(root, schema, object, path, found);
    }
    if let (Value::Array(items), Some(item_schema)) = (value, schema.get("items")) {
        for (index, item) in items.iter().enumerate() {
            check(root, item_schema, item, &format!("{path}[{index}]"), found);
        }
    }
}

fn check_object(
    root: &Value,
    schema: &Map<String, Value>,
    object: &Map<String, Value>,
    path: &str,
    found: &mut Vec<String>,
) {
    let properties = schema.get("properties").and_then(Value::as_object);
    if let Some(Value::Array(required)) = schema.get("required") {
        for name in required.iter().filter_map(Value::as_str) {
            if !object.contains_key(name) {
                found.push(format!("{path}: missing {name:?}"));
            }
        }
    }
    for (name, field) in object {
        let field_path = format!("{path}.{name}");
        match properties.and_then(|properties| properties.get(name)) {
            Some(field_schema) => check(root, field_schema, field, &field_path, found),
            None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                found.push(format!("{field_path}: not in the schema"));
            }
            None => {}
        }
    }
}

fn resolve<'a>(root: &'a Value, reference: &Value) -> Option<&'a Value> {
    let name = reference.as_str()?.strip_prefix("#/$defs/")?;
    root.get("$defs")?.get(name)
}

fn is_type(value: &Value, name: &str) -> bool {
    match name {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => false,
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = r##"{
        "type": "object",
        "required": ["a", "b"],
        "additionalProperties": false,
        "properties": {
            "a": {"type": "integer", "minimum": 0},
            "b": {"type": ["string", "null"], "enum": ["x", null]},
            "c": {"type": "array", "items": {"$ref": "#/$defs/item"}}
        },
        "$defs": {"item": {"type": "object", "required": ["k"], "properties": {"k": {"const": 1}}}}
    }"##;

    #[test]
    fn a_value_that_fits_has_no_violations() {
        assert!(violations(SCHEMA, &json!({"a": 1, "b": null, "c": [{"k": 1}]})).is_empty());
    }

    #[test]
    fn each_kind_of_misfit_is_reported() {
        let found = violations(SCHEMA, &json!({"a": -1, "b": "y", "c": [{}], "d": 0}));
        let found = found.join("\n");
        for problem in [
            "$.a: -1 is below 0",
            "$.b: \"y\" is not one of",
            "$.c[0]: missing \"k\"",
            "$.d: not in the schema",
        ] {
            assert!(found.contains(problem), "{problem} in:\n{found}");
        }
        assert!(
            violations(SCHEMA, &json!({"b": "x"}))
                .join("")
                .contains("missing \"a\"")
        );
        assert!(
            violations(SCHEMA, &json!([]))
                .join("")
                .contains("an array is not object")
        );
    }

    #[test]
    fn a_keyword_the_checker_doesnt_know_is_an_error() {
        let found = violations(r#"{"type": "string", "pattern": "x"}"#, &json!("x"));
        assert!(found.join("").contains("\"pattern\""));
    }
}
