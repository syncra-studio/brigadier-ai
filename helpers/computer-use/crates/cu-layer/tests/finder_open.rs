//! Finder gestures that open a file open it through `launch_app` instead,
//! behind the user's work, with fake driver tools, a fake Finder and fake
//! nested calls: no permission grant, no real app.
// What it tests exists on macOS only.
#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cu_layer::finder_open::{Calls, Finder};
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

const FINDER: i32 = 10;
const OTHER_APP: i32 = 20;

#[derive(Default)]
struct Seen {
    /// Driver tools that ran, by name.
    driver: Vec<String>,
    /// Nested registry calls: tool and arguments.
    nested: Vec<(String, Value)>,
}

type Shared = Arc<Mutex<Seen>>;

struct FakeTool {
    def: ToolDef,
    seen: Shared,
}

#[async_trait]
impl Tool for FakeTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        self.seen.lock().unwrap().driver.push(self.def.name.clone());
        ToolResult::text("driver")
    }
}

/// Finder's window lists `a.txt` and the folder `sub`; the row the token
/// `s1:3` names is `a.txt`, `s1:4` is `sub`; both are selected.
struct FakeFinder;

impl Finder for FakeFinder {
    fn is_finder(&self, pid: i32) -> bool {
        pid == FINDER
    }
    fn element_files(&self, _pid: i32, args: &Value) -> Vec<PathBuf> {
        match args.get("element_token").and_then(Value::as_str) {
            Some("s1:3") => vec![PathBuf::from("/fx/a.txt")],
            Some("s1:4") => vec![PathBuf::from("/fx/sub")],
            _ => vec![],
        }
    }
    fn selected_files(&self, _pid: i32, _window_id: Option<u32>) -> Vec<PathBuf> {
        vec![PathBuf::from("/fx/a.txt"), PathBuf::from("/fx/sub")]
    }
    /// `b.txt`'s row is at (100, 80) in window 5; nothing else is a row.
    fn files_at(&self, _pid: i32, window_id: u32, args: &Value) -> Vec<PathBuf> {
        let at = |key: &str| args.get(key).and_then(Value::as_f64);
        if window_id == 5 && at("x") == Some(100.0) && at("y") == Some(80.0) {
            vec![PathBuf::from("/fx/b.txt")]
        } else {
            vec![]
        }
    }
    fn is_folder(&self, path: &Path) -> bool {
        path.extension().is_none()
    }
    fn default_app(&self, _path: &Path) -> Option<String> {
        Some("com.apple.TextEdit".to_owned())
    }
}

struct FakeCalls {
    seen: Shared,
    launch_fails: bool,
}

#[async_trait]
impl Calls for FakeCalls {
    async fn call(&self, tool: &str, args: Value) -> ToolResult {
        self.seen
            .lock()
            .unwrap()
            .nested
            .push((tool.to_owned(), args));
        match tool {
            "launch_app" if self.launch_fails => ToolResult::error("Launch failed: no"),
            "launch_app" => ToolResult::text(
                "TextEdit (pid 30) was already running.\n\nWindows:\n- \"a.txt\" [window_id: 7]",
            )
            .with_structured(json!({"pid": 30, "name": "TextEdit", "windows": [{"window_id": 7}]})),
            _ => ToolResult::text("clicked"),
        }
    }
}

fn build(launch_fails: bool) -> (ToolRegistry, Shared) {
    let seen = Shared::default();
    let mut registry = ToolRegistry::new();
    for name in cu_layer::finder_open::TOOLS {
        registry.register(Box::new(FakeTool {
            def: ToolDef {
                name: (*name).into(),
                description: "fake".into(),
                input_schema: json!({"type": "object", "additionalProperties": true}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            seen: seen.clone(),
        }));
    }
    cu_layer::finder_open::install_with(
        &mut registry,
        Arc::new(FakeFinder),
        Arc::new(FakeCalls {
            seen: seen.clone(),
            launch_fails,
        }),
    );
    (registry, seen)
}

fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn launches(seen: &Shared) -> Vec<Value> {
    seen.lock()
        .unwrap()
        .nested
        .iter()
        .filter(|(tool, _)| tool == "launch_app")
        .map(|(_, args)| args.clone())
        .collect()
}

#[tokio::test]
async fn opening_a_file_row_in_finder_opens_it_behind_instead() {
    let (registry, seen) = build(false);
    let result = registry
        .invoke(
            "click",
            json!({"pid": FINDER, "element_token": "s1:3", "action": "open", "session": "s"}),
        )
        .await;
    assert_ne!(result.is_error, Some(true));
    assert!(
        seen.lock().unwrap().driver.is_empty(),
        "Finder must not open it"
    );
    let mut launches = launches(&seen);
    // The registry gives the session its runtime-private name before the
    // wrapper sees it; the nested launch keeps that name.
    let session = launches[0]["session"].take();
    assert!(
        session.as_str().is_some_and(|name| name.ends_with(":s")),
        "{session}"
    );
    assert_eq!(
        launches,
        vec![json!({"bundle_id": "com.apple.TextEdit", "urls": ["/fx/a.txt"], "session": null})]
    );
    let text = text_of(&result);
    assert!(
        text.contains("Opened a.txt in TextEdit behind the user's work"),
        "{text}"
    );
    assert!(text.contains("[window_id: 7]"), "{text}");
    assert!(text.contains("TextEdit (pid 30)"), "{text}");
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["effect"], "confirmed", "{structured}");
    assert_eq!(structured["delivery"]["mode"], "background", "{structured}");
}

#[tokio::test]
async fn a_folder_or_another_app_goes_to_the_driver() {
    let (registry, seen) = build(false);
    registry
        .invoke(
            "double_click",
            json!({"pid": FINDER, "element_token": "s1:4"}),
        )
        .await;
    registry
        .invoke(
            "double_click",
            json!({"pid": OTHER_APP, "element_token": "s1:3"}),
        )
        .await;
    registry
        .invoke("click", json!({"pid": FINDER, "element_token": "s1:3"}))
        .await;
    assert!(launches(&seen).is_empty());
    assert_eq!(
        seen.lock().unwrap().driver,
        ["double_click", "double_click", "click"]
    );
}

#[tokio::test]
async fn open_keys_and_menus_open_the_selected_files_only() {
    for (tool, args) in [
        ("hotkey", json!({"pid": FINDER, "keys": ["cmd", "o"]})),
        (
            "press_key",
            json!({"pid": FINDER, "key": "down", "modifiers": ["cmd"]}),
        ),
        (
            "invoke_menu",
            json!({"pid": FINDER, "path": ["File", "Open"]}),
        ),
    ] {
        let (registry, seen) = build(false);
        registry.invoke(tool, args).await;
        assert!(seen.lock().unwrap().driver.is_empty(), "{tool}");
        assert_eq!(
            launches(&seen),
            vec![json!({"bundle_id": "com.apple.TextEdit", "urls": ["/fx/a.txt"]})],
            "{tool}"
        );
    }
    let (registry, seen) = build(false);
    registry
        .invoke(
            "invoke_menu",
            json!({"pid": FINDER, "path": ["File", "Open With", "Preview"]}),
        )
        .await;
    assert_eq!(
        launches(&seen),
        vec![json!({"name": "Preview", "urls": ["/fx/a.txt"]})]
    );
}

#[tokio::test]
async fn a_pixel_double_click_selects_then_opens_behind() {
    let (registry, seen) = build(false);
    registry
        .invoke(
            "double_click",
            json!({"pid": FINDER, "window_id": 5, "x": 40, "y": 60}),
        )
        .await;
    let seen = seen.lock().unwrap();
    // The selecting click is the driver's own, inside the double-click's
    // turn: a nested click through the registry would wait for that turn.
    assert_eq!(seen.driver, ["click"]);
    let tools: Vec<&str> = seen.nested.iter().map(|(tool, _)| tool.as_str()).collect();
    assert_eq!(tools, ["launch_app"]);
}

#[tokio::test]
async fn a_pixel_double_click_on_a_row_opens_its_file_behind_without_clicking() {
    for (tool, args) in [
        (
            "double_click",
            json!({"pid": FINDER, "window_id": 5, "x": 100, "y": 80}),
        ),
        (
            "click",
            json!({"pid": FINDER, "window_id": 5, "x": 100, "y": 80, "count": 2}),
        ),
    ] {
        let (registry, seen) = build(false);
        let result = registry.invoke(tool, args).await;
        assert_ne!(result.is_error, Some(true), "{tool}");
        assert!(
            seen.lock().unwrap().driver.is_empty(),
            "{tool}: no click reaches Finder"
        );
        assert_eq!(
            launches(&seen),
            vec![json!({"bundle_id": "com.apple.TextEdit", "urls": ["/fx/b.txt"]})],
            "{tool}"
        );
        assert!(
            text_of(&result).contains("Opened b.txt in TextEdit"),
            "{tool}"
        );
    }
}

#[tokio::test]
async fn a_modified_pixel_double_click_is_left_to_the_driver() {
    for extra in [
        json!({"modifier": ["ctrl"]}),
        json!({"action": "hover"}),
        json!({"button": "right"}),
        json!({"debug_image_out": "/tmp/x.png"}),
    ] {
        let (registry, seen) = build(false);
        let mut args = json!({"pid": FINDER, "window_id": 5, "x": 100, "y": 80, "count": 2});
        args.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        registry.invoke("click", args).await;
        let seen = seen.lock().unwrap();
        assert_eq!(
            seen.driver,
            ["click"],
            "{extra}: the driver's click checks it first"
        );
        let opened: Vec<_> = seen
            .nested
            .iter()
            .flat_map(|(_, args)| args["urls"].as_array().cloned().unwrap_or_default())
            .collect();
        assert!(!opened.contains(&json!("/fx/b.txt")), "{extra}: {opened:?}");
    }
}

/// A `selected_files` that never reports anything selected, as the real
/// driver didn't after a background pixel click in a live probe.
struct NothingSelectedFinder;

impl Finder for NothingSelectedFinder {
    fn is_finder(&self, pid: i32) -> bool {
        pid == FINDER
    }
    fn element_files(&self, _pid: i32, _args: &Value) -> Vec<PathBuf> {
        vec![]
    }
    fn selected_files(&self, _pid: i32, _window_id: Option<u32>) -> Vec<PathBuf> {
        vec![]
    }
    fn files_at(&self, _pid: i32, _window_id: u32, _args: &Value) -> Vec<PathBuf> {
        vec![]
    }
    fn is_folder(&self, path: &Path) -> bool {
        path.extension().is_none()
    }
    fn default_app(&self, _path: &Path) -> Option<String> {
        Some("com.apple.TextEdit".to_owned())
    }
}

#[tokio::test]
async fn a_pixel_double_click_that_selects_nothing_is_refused_not_silently_dropped() {
    let seen = Shared::default();
    let mut registry = ToolRegistry::new();
    for name in cu_layer::finder_open::TOOLS {
        registry.register(Box::new(FakeTool {
            def: ToolDef {
                name: (*name).into(),
                description: "fake".into(),
                input_schema: json!({"type": "object", "additionalProperties": true}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            seen: seen.clone(),
        }));
    }
    cu_layer::finder_open::install_with(
        &mut registry,
        Arc::new(NothingSelectedFinder),
        Arc::new(FakeCalls {
            seen: seen.clone(),
            launch_fails: false,
        }),
    );
    let result = registry
        .invoke(
            "double_click",
            json!({"pid": FINDER, "window_id": 5, "x": 40, "y": 60}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(text.contains("element_token"), "{text}");
    assert!(launches(&seen).is_empty());
    assert_eq!(
        seen.lock().unwrap().driver,
        ["click"],
        "the real double_click tool must never run: posting it as a raw background \
         CGEvent can silently open nothing"
    );
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["code"], "pixel_open_unresolved", "{structured}");
}

#[tokio::test]
async fn a_failed_open_says_so() {
    let (registry, _) = build(true);
    let result = registry
        .invoke(
            "click",
            json!({"pid": FINDER, "element_token": "s1:3", "action": "open"}),
        )
        .await;
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result).contains("Launch failed"));
}
