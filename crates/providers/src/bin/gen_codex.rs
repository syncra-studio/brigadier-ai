//! Regenerates the typed Codex app-server bindings in `src/codex/protocol.rs` from the installed
//! `codex`'s own JSON Schema.
//!
//! Usage: `cargo run -p brigadier-providers --features codegen --bin gen-codex [path/to/codex]`.
//!
//! Only the messages the adapter uses, and the types they reference, are generated. Unknown
//! fields are tolerated (newer Codex versions add fields all the time); the adapter compares the
//! running version with the one recorded here and warns when they differ.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value};

/// Types from the v2 bundle the adapter reads or writes.
const V2_ROOTS: &[&str] = &[
    "InitializeParams",
    "ThreadStartParams",
    "ThreadStartResponse",
    "ThreadResumeParams",
    "ThreadResumeResponse",
    "ThreadForkParams",
    "ThreadForkResponse",
    "ThreadDeleteParams",
    "ThreadArchiveParams",
    "ThreadUnarchiveParams",
    "TurnStartParams",
    "TurnStartResponse",
    "TurnSteerParams",
    "TurnSteerResponse",
    "TurnInterruptParams",
    "ThreadCompactStartParams",
    "ThreadCompactStartResponse",
    "ModelListParams",
    "ModelListResponse",
    "GetAccountRateLimitsResponse",
    "GetAccountParams",
    "GetAccountResponse",
    "ThreadStartedNotification",
    "TurnStartedNotification",
    "TurnCompletedNotification",
    "ItemStartedNotification",
    "ItemCompletedNotification",
    "AgentMessageDeltaNotification",
    "ReasoningSummaryTextDeltaNotification",
    "CommandExecutionOutputDeltaNotification",
    "ThreadTokenUsageUpdatedNotification",
    "AccountRateLimitsUpdatedNotification",
    "ErrorNotification",
];

/// Request params the adapter builds field by field; they get `Default`.
const DEFAULTED: &[&str] = &[
    "ThreadStartParams",
    "ThreadResumeParams",
    "ThreadForkParams",
    "TurnStartParams",
    "TurnSteerParams",
    "ModelListParams",
];

/// Server → client requests, each generated to its own file.
const REQUEST_FILES: &[&str] = &[
    "CommandExecutionRequestApprovalParams",
    "CommandExecutionRequestApprovalResponse",
    "FileChangeRequestApprovalParams",
    "FileChangeRequestApprovalResponse",
    "PermissionsRequestApprovalParams",
    "PermissionsRequestApprovalResponse",
];

type Error = Box<dyn std::error::Error>;

fn main() -> Result<(), Error> {
    let codex = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .map_or_else(|| which::which("codex"), Ok)?;
    let version = Command::new(&codex).arg("--version").output()?;
    let version = String::from_utf8(version.stdout)?;
    let version = brigadier_providers::cli::parse_version(&version)
        .ok_or("could not read the codex version")?;

    let schema_dir =
        std::env::temp_dir().join(format!("brigadier-codex-schema-{}", std::process::id()));
    let generated = generate(&codex, &version, &schema_dir);
    let _ = std::fs::remove_dir_all(&schema_dir);
    let source = generated?;

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/codex/protocol.rs");
    std::fs::write(&out, source)?;
    // Same formatting as the rest of the workspace, so `cargo fmt --check` stays clean.
    let formatted = Command::new("rustfmt")
        .args(["--edition", "2024"])
        .arg(&out)
        .status()?;
    if !formatted.success() {
        return Err(format!("rustfmt failed on {}", out.display()).into());
    }
    println!("wrote bindings for codex {version} to {}", out.display());
    Ok(())
}

fn generate(codex: &Path, version: &str, schema_dir: &Path) -> Result<String, Error> {
    let status = Command::new(codex)
        .args(["app-server", "generate-json-schema", "--out"])
        .arg(schema_dir)
        .status()?;
    if !status.success() {
        return Err(format!("codex app-server generate-json-schema failed: {status}").into());
    }

    let bundle: Value = read_json(&schema_dir.join("codex_app_server_protocol.v2.schemas.json"))?;
    let mut definitions: BTreeMap<String, Value> = bundle
        .get("definitions")
        .and_then(Value::as_object)
        .ok_or("the v2 bundle has no definitions")?
        .iter()
        .map(|(name, schema)| (name.clone(), schema.clone()))
        .collect();

    let mut roots: Vec<String> = V2_ROOTS.iter().map(|name| (*name).to_owned()).collect();
    for name in REQUEST_FILES {
        let mut schema: Value = read_json(&schema_dir.join(format!("{name}.json")))?;
        let object = schema
            .as_object_mut()
            .ok_or("request schema is not an object")?;
        if let Some(Value::Object(own)) = object.remove("definitions") {
            for (def, body) in own {
                match definitions.get(&def) {
                    Some(existing) if *existing != body => {
                        return Err(format!("{name} redefines {def} differently").into());
                    }
                    Some(_) => {}
                    None => {
                        definitions.insert(def, body);
                    }
                }
            }
        }
        object.remove("$schema");
        definitions.insert((*name).to_owned(), schema);
        roots.push((*name).to_owned());
    }

    let mut wanted = BTreeSet::new();
    let mut queue = roots.clone();
    while let Some(name) = queue.pop() {
        if !wanted.insert(name.clone()) {
            continue;
        }
        let schema = definitions
            .get(&name)
            .ok_or_else(|| format!("{name} is not in the schema"))?;
        let mut refs = BTreeSet::new();
        collect_refs(schema, &mut refs);
        queue.extend(refs.into_iter().filter(|name| !wanted.contains(name)));
    }

    let mut selected = Map::new();
    for name in &wanted {
        selected.insert(name.clone(), definitions[name].clone());
    }
    let root = serde_json::json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "definitions": selected,
    });
    let root: schemars::schema::RootSchema = serde_json::from_value(root)?;

    let mut settings = typify::TypeSpaceSettings::default();
    settings.with_struct_builder(false);
    for name in DEFAULTED {
        settings.with_patch(
            name,
            typify::TypeSpacePatch::default().with_derive("Default"),
        );
    }
    let mut space = typify::TypeSpace::new(&settings);
    space.add_root_schema(root)?;
    let file: syn::File = syn::parse2(space.to_stream())?;
    let code = line_docs(&prettyplease::unparse(&file))
        // Newer Codex versions add fields; old bindings must still read their messages.
        .replace("#[serde(deny_unknown_fields)]\n", "");

    Ok(format!(
        "//! Codex app-server protocol types, generated by `gen-codex` from the JSON Schema of\n\
         //! codex-cli {version}. Do not edit; regenerate after upgrading Codex.\n\
         //!\n\
         //! {count} types: the messages the adapter uses and everything they reference.\n\
         \n\
         /// The codex-cli version these bindings were generated from.\n\
         pub const SCHEMA_VERSION: &str = \"{version}\";\n\
         \n\
         {code}",
        count = wanted.len(),
    ))
}

fn read_json(path: &Path) -> Result<Value, Error> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

fn collect_refs(value: &Value, refs: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if key == "$ref"
                    && let Some(name) = value
                        .as_str()
                        .and_then(|r| r.strip_prefix("#/definitions/"))
                {
                    refs.insert(name.to_owned());
                } else {
                    collect_refs(value, refs);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_refs(item, refs)),
        _ => {}
    }
}

/// Keep rustfmt's item indentation out of Markdown: indented block-doc prose becomes
/// a Rust code example when rustdoc reads it. Line docs preserve the intended paragraphs.
fn line_docs(source: &str) -> String {
    let mut output = String::new();
    let mut indent = None;
    for line in source.lines() {
        let text = if let Some(prefix) = indent {
            line.strip_prefix(prefix).unwrap_or(line)
        } else if let Some(text) = line.trim_start().strip_prefix("/**") {
            indent = Some(&line[..line.len() - line.trim_start().len()]);
            text
        } else {
            output.push_str(line);
            output.push('\n');
            continue;
        };
        let (text, ended) = match text.strip_suffix("*/") {
            Some(text) => (text, true),
            None => (text, false),
        };
        output.push_str(indent.unwrap());
        output.push_str("///");
        output.push_str(text);
        output.push('\n');
        if ended {
            indent = None;
        }
    }
    output
}
