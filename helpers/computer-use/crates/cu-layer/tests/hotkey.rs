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
    made_main: Vec<u32>,
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
    async fn invoke(&self, _args: Value) -> ToolResult {
        self.seen.lock().unwrap().posted += 1;
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
    fn make_main(&self, _pid: i32, window_id: u32) {
        self.0.lock().unwrap().made_main.push(window_id);
    }
    fn press(&self, _pid: i32, key: &str, modifiers: i64) -> Option<String> {
        (key == "z" && modifiers == 0).then(|| "Undo".to_owned())
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
