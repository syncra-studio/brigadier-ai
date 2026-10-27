//! A ⌘ shortcut for an app in the background presses its menu item, with a
//! fake driver hotkey and a fake menu bar: no permission grant, no real app.
// What it tests exists on macOS only.
#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cu_layer::hotkey::MenuBar;
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

#[derive(Default)]
struct Seen {
    posted: usize,
    posted_args: Vec<Value>,
    panel: Option<(i32, u32)>,
    driver_refuses: bool,
    made_main: Vec<u32>,
    menu_pressed: usize,
    selected: usize,
}

type Shared = Arc<Mutex<Seen>>;

struct FakeHotkey {
    def: ToolDef,
    seen: Shared,
}

#[async_trait]
impl Tool for FakeHotkey {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let mut seen = self.seen.lock().unwrap();
        seen.posted += 1;
        seen.posted_args.push(args);
        if seen.driver_refuses {
            return ToolResult::error("panel window could not be resolved");
        }
        ToolResult::text("posted")
    }
}

/// pid 1 is a native app whose Edit menu has Undo (⌘Z) and a disabled
/// Select All; pid 2 is a Chromium app.
struct FakeMenuBar(Shared);

impl MenuBar for FakeMenuBar {
    fn chromium(&self, pid: i32) -> bool {
        pid == 2
    }
    fn panel_owns_keys(&self, pid: i32, window_id: u32) -> bool {
        self.0.lock().unwrap().panel == Some((pid, window_id))
    }
    fn make_main(&self, _pid: i32, window_id: u32) {
        self.0.lock().unwrap().made_main.push(window_id);
    }
    fn press(&self, _pid: i32, key: &str, modifiers: i64) -> Option<String> {
        self.0.lock().unwrap().menu_pressed += 1;
        match (key, modifiers) {
            ("z", 0) => Some("Undo".to_owned()),
            ("g", 1) => Some("Find Previous".to_owned()),
            _ => None,
        }
    }
    fn select_all_text(&self, _pid: i32, _window_id: Option<u32>) -> Option<String> {
        self.0.lock().unwrap().selected += 1;
        Some("all 5 characters of the AXTextArea".to_owned())
    }
}

fn build() -> (ToolRegistry, Shared) {
    let seen = Shared::default();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeHotkey {
        def: ToolDef {
            name: "hotkey".into(),
            description: "fake".into(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        seen: seen.clone(),
    }));
    let menu = seen.clone();
    cu_layer::hotkey::install_with(&mut registry, move || Box::new(FakeMenuBar(menu)));
    (registry, seen)
}

fn text_of(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn assert_forwarded(seen: &Seen, expected: Value) {
    assert_eq!(seen.posted_args.len(), 1);
    let mut forwarded = seen.posted_args[0].as_object().unwrap().clone();
    // The registry supplies its own transport context before the wrapper.
    forwarded.remove("_session_id");
    forwarded.remove("_transport_session_id");
    assert_eq!(Value::Object(forwarded), expected);
}

#[tokio::test]
async fn a_menu_shortcut_presses_its_menu_item_in_the_named_window() {
    let (registry, seen) = build();
    let result = registry
        .invoke(
            "hotkey",
            json!({"pid": 1, "window_id": 9, "keys": ["cmd", "z"]}),
        )
        .await;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert!(
        text_of(&result).contains("Pressed Undo (cmd+z) in the menu bar of pid 1."),
        "{}",
        text_of(&result)
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.posted, 0);
    assert_eq!(seen.made_main, [9]);
}

#[tokio::test]
async fn command_a_selects_the_text_when_select_all_cant_be_pressed() {
    let (registry, seen) = build();
    let result = registry
        .invoke("hotkey", json!({"pid": 1, "keys": ["cmd", "a"]}))
        .await;
    assert!(
        text_of(&result).contains("Selected all 5 characters of the AXTextArea (cmd+a) in pid 1."),
        "{}",
        text_of(&result)
    );
    assert_eq!(seen.lock().unwrap().posted, 0);
}

#[tokio::test]
async fn panel_shortcuts_bypass_the_client_menu_and_preserve_the_exact_target() {
    for keys in [
        json!(["cmd", "shift", "g"]),
        json!(["cmd", "z"]),
        json!(["cmd", "a"]),
    ] {
        let (registry, seen) = build();
        seen.lock().unwrap().panel = Some((1, 90));
        let args = json!({"pid": 1, "window_id": 90, "keys": keys});
        let result = registry.invoke("hotkey", args.clone()).await;
        assert_eq!(text_of(&result), "posted", "{args}");
        let seen = seen.lock().unwrap();
        assert_forwarded(&seen, args);
        assert_eq!(seen.posted, 1);
        assert!(seen.made_main.is_empty());
        assert_eq!(seen.menu_pressed, 0);
        assert_eq!(seen.selected, 0);
    }
}

#[tokio::test]
async fn panel_driver_refusal_never_falls_back_to_the_client_menu() {
    let (registry, seen) = build();
    {
        let mut seen = seen.lock().unwrap();
        seen.panel = Some((1, 90));
        seen.driver_refuses = true;
    }
    let args = json!({"pid": 1, "window_id": 90, "keys": ["cmd", "shift", "g"]});
    let result = registry.invoke("hotkey", args.clone()).await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(text_of(&result), "panel window could not be resolved");
    let seen = seen.lock().unwrap();
    assert_forwarded(&seen, args);
    assert!(seen.made_main.is_empty());
    assert_eq!(seen.menu_pressed, 0);
    assert_eq!(seen.selected, 0);
}

#[tokio::test]
async fn other_combos_go_to_the_driver() {
    for args in [
        // No menu item answers it.
        json!({"pid": 1, "keys": ["cmd", "b"]}),
        // Chromium pages handle shortcuts themselves.
        json!({"pid": 2, "keys": ["cmd", "z"]}),
        // Not a menu shortcut.
        json!({"pid": 1, "keys": ["ctrl", "a"]}),
        // A pixel target focuses that spot first.
        json!({"pid": 1, "keys": ["cmd", "v"], "x": 4, "y": 4}),
    ] {
        let (registry, seen) = build();
        let result = registry.invoke("hotkey", args.clone()).await;
        assert_eq!(text_of(&result), "posted", "{args}");
        assert_eq!(seen.lock().unwrap().posted, 1, "{args}");
        assert_eq!(seen.lock().unwrap().selected, 0, "{args}");
    }
}
