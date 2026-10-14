//! Checks against the vendored driver itself: which tools it registers, what
//! agents see of them, and the driver text that suggests the foreground.
//! Run with `UPDATE_SNAPSHOTS=1` after moving to a newer driver release, then
//! review the snapshot diff (see upstream/UPSTREAM.md).
#![cfg(target_os = "macos")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use cu_layer::catalog;
use serde_json::Value;

fn snapshot(name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        expected == actual,
        "{} changed; review it and re-run with UPDATE_SNAPSHOTS=1.\n--- now ---\n{actual}",
        path.display()
    );
}

/// The registry as the helper builds it, as agents get it.
fn brigadier_registry() -> std::sync::Arc<cua_driver_core::tool::ToolRegistry> {
    cu_layer::registry::build(cu_layer::registry::Options {
        cursor: cursor_overlay::CursorConfig {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    })
    .unwrap()
}

#[test]
fn every_driver_tool_is_classified() {
    let registry = platform_macos::register_tools();
    let registered: BTreeSet<&str> = registry.tool_names().collect();
    let withheld: BTreeSet<&str> = catalog::WITHHELD.iter().map(|(name, _)| *name).collect();
    let offered: BTreeSet<&str> = catalog::OFFERED.iter().copied().collect();
    let unclassified: Vec<_> = registered
        .iter()
        .filter(|name| !offered.contains(*name) && !withheld.contains(*name))
        .collect();
    assert!(
        unclassified.is_empty(),
        "new driver tools to review, then offer or withhold in catalog.rs: {unclassified:?}"
    );
    let gone: Vec<_> = offered
        .iter()
        .filter(|name| !registered.contains(*name))
        .collect();
    assert!(
        gone.is_empty(),
        "offered tools the driver no longer has: {gone:?}"
    );
}

/// Text typed into a document stays one undo step: no key edit is added
/// after the insert, which ⌘Z would undo instead of the agent's text.
#[test]
fn typed_text_is_not_followed_by_a_key_edit() {
    let _registry = brigadier_registry();
    assert!(!platform_macos::document_edit::key_edit_after_insert());
}

/// What agents see: tool names and their arguments, and no word of the
/// foreground anywhere in the list.
#[test]
fn agents_see_only_background_tools() {
    let registry = brigadier_registry();
    let list = cu_layer::text::tools_list(registry.tools_list());
    let rendered = serde_json::to_string(&list).unwrap().to_ascii_lowercase();
    for word in [
        "foreground",
        "bring_to_front",
        "delivery_mode",
        "cua driver",
        "cua-driver",
    ] {
        assert!(!rendered.contains(word), "tools/list still says {word:?}");
    }
    let mut lines = String::new();
    for tool in list["tools"].as_array().unwrap() {
        let mut args: Vec<&str> = tool["inputSchema"]["properties"]
            .as_object()
            .map(|properties| properties.keys().map(String::as_str).collect())
            .unwrap_or_default();
        args.sort_unstable();
        lines.push_str(&format!(
            "{}({})\n",
            tool["name"].as_str().unwrap(),
            args.join(", ")
        ));
    }
    snapshot("tools-list.txt", &lines);
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Every line of the driver's source with a string that suggests acting in
/// front, counted per file. A newer driver that adds such text changes the
/// snapshot, so its wording is reviewed against the result rewrite.
#[test]
fn driver_text_that_suggests_the_foreground_is_reviewed() {
    let upstream = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../upstream/crates");
    let mut files = Vec::new();
    for krate in ["cua-driver-core", "platform-macos"] {
        rust_files(&upstream.join(krate).join("src"), &mut files);
    }
    files.sort();
    let mut lines = String::new();
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        let count = source
            .lines()
            .filter(|line| {
                let trimmed = line.trim_start();
                !trimmed.starts_with("//")
                    && line.contains('"')
                    && ["foreground", "bring_to_front"]
                        .iter()
                        .any(|word| line.to_ascii_lowercase().contains(word))
            })
            .count();
        if count > 0 {
            let relative = file.strip_prefix(&upstream).unwrap();
            lines.push_str(&format!("{count:4} {}\n", relative.display()));
        }
    }
    snapshot("foreground-text.txt", &lines);
}

#[test]
fn listed_tools_have_object_schemas() {
    let registry = brigadier_registry();
    let list = cu_layer::text::tools_list(registry.tools_list());
    for tool in list["tools"].as_array().unwrap() {
        assert_eq!(
            tool["inputSchema"]["type"],
            Value::from("object"),
            "{}",
            tool["name"]
        );
    }
}

#[test]
#[ignore]
fn dump_rewritten_tools_list() {
    let registry = brigadier_registry();
    let list = cu_layer::text::tools_list(registry.tools_list());
    std::fs::write(
        std::env::var("DUMP").unwrap(),
        serde_json::to_string_pretty(&list).unwrap(),
    )
    .unwrap();
}
