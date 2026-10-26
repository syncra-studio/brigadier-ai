//! Scrolling a text area stops at its end, with a fake driver scroll and a
//! fake text area: no permission grant, no real app.
// What it tests exists on macOS only.
#![cfg(target_os = "macos")]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cu_layer::scroll::{TextArea, TextHandle, TextPosition};
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// A text of `total` characters showing `shown` at a time; each press moves
/// it `per_press` characters unless the app is "busy" for some presses.
struct Text {
    location: isize,
    shown: isize,
    total: isize,
    per_press: isize,
    busy_presses: usize,
    presses: usize,
}

type Shared = Arc<Mutex<Text>>;

struct FakeScroll {
    def: ToolDef,
    text: Shared,
}

#[async_trait]
impl Tool for FakeScroll {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        assert_eq!(args["amount"], 1, "one press per call");
        let mut text = self.text.lock().unwrap();
        text.presses += 1;
        if text.busy_presses > 0 {
            text.busy_presses -= 1;
            return ToolResult::text("pressed");
        }
        let step = if args["direction"] == "up" {
            -text.per_press
        } else {
            text.per_press
        };
        text.location = (text.location + step).clamp(0, text.total - text.shown);
        ToolResult::text("pressed").with_structured(json!({"pid": 1}))
    }
}

struct FakeArea(Shared);
struct FakeHandle(Shared);

impl TextArea for FakeArea {
    fn find(&self, _args: &Value) -> Option<Box<dyn TextHandle>> {
        Some(Box::new(FakeHandle(self.0.clone())))
    }
}

impl TextHandle for FakeHandle {
    fn position(&self) -> Option<TextPosition> {
        let text = self.0.lock().unwrap();
        Some((text.location, text.shown, text.total))
    }
}

fn build(text: Text) -> (ToolRegistry, Shared) {
    let shared = Arc::new(Mutex::new(text));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeScroll {
        def: ToolDef {
            name: "scroll".into(),
            description: "fake".into(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        text: shared.clone(),
    }));
    let area = shared.clone();
    cu_layer::scroll::install_with(&mut registry, move || Box::new(FakeArea(area)));
    (registry, shared)
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

fn args(direction: &str, amount: u64) -> Value {
    json!({"pid": 1, "window_id": 2, "element_token": "s00000001:3",
           "direction": direction, "by": "page", "amount": amount})
}

#[tokio::test]
async fn a_scroll_stops_when_the_end_shows() {
    let (registry, text) = build(Text {
        location: 0,
        shown: 100,
        total: 1000,
        per_press: 100,
        busy_presses: 0,
        presses: 0,
    });
    let result = registry.invoke("scroll", args("down", 50)).await;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert_eq!(text.lock().unwrap().presses, 9);
    assert!(
        text_of(&result).contains("reached the end"),
        "{}",
        text_of(&result)
    );
    // The registry keeps its own structured shape for actions and carries
    // the summary there too.
    let structured = result.structured_content.unwrap();
    assert!(structured["summary"]
        .as_str()
        .unwrap()
        .contains("reached the end"));
}

#[tokio::test]
async fn a_text_that_already_shows_its_end_sends_nothing() {
    let (registry, text) = build(Text {
        location: 0,
        shown: 100,
        total: 100,
        per_press: 100,
        busy_presses: 0,
        presses: 0,
    });
    let result = registry.invoke("scroll", args("up", 3)).await;
    assert!(
        text_of(&result).starts_with("✅ Nothing to scroll up"),
        "{}",
        text_of(&result)
    );
    assert_eq!(text.lock().unwrap().presses, 0);
}

#[tokio::test]
async fn a_busy_app_gets_a_moment_and_a_stuck_one_is_reported() {
    let (registry, text) = build(Text {
        location: 0,
        shown: 100,
        total: 1000,
        per_press: 100,
        busy_presses: 2,
        presses: 0,
    });
    let result = registry.invoke("scroll", args("down", 3)).await;
    assert!(
        text_of(&result).starts_with("✅ Scrolled"),
        "{}",
        text_of(&result)
    );
    assert_eq!(text.lock().unwrap().location, 300, "three real steps");

    let (registry, _text) = build(Text {
        location: 0,
        shown: 100,
        total: 1000,
        per_press: 100,
        busy_presses: 100,
        presses: 0,
    });
    let result = registry.invoke("scroll", args("down", 3)).await;
    assert!(text_of(&result).starts_with("⚠️"), "{}", text_of(&result));
}

#[tokio::test]
async fn other_scrolls_go_to_the_driver_as_they_are() {
    let (registry, text) = build(Text {
        location: 0,
        shown: 100,
        total: 1000,
        per_press: 100,
        busy_presses: 0,
        presses: 0,
    });
    // No element target: the driver's own keystroke scroll (fake asserts one press).
    let result = registry
        .invoke(
            "scroll",
            json!({"pid": 1, "direction": "down", "amount": 1}),
        )
        .await;
    assert_eq!(text_of(&result), "pressed");
    assert_eq!(text.lock().unwrap().presses, 1);
}
