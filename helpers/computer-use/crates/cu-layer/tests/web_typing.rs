//! Typing into a desktop web app's page in the background, with a fake driver
//! `type_text` and fake page fields: no permission grant, no real app.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cu_layer::web_typing::{PageField, PageFields, Typed};
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// What the fake driver answers, and what the fake page did.
#[derive(Default)]
struct Page {
    /// The driver's answer: its web-content refusal (`None`: it typed).
    refusal: Option<(&'static str, &'static str)>,
    /// What typing into the field gives (`None`: its window can't be made key).
    typed: Option<Typed>,
    /// Whether the element can be found again.
    found: bool,
    finds: usize,
    keys: Vec<String>,
}

type Shared = Arc<Mutex<Page>>;

struct FakeTypeText {
    def: ToolDef,
    page: Shared,
}

#[async_trait]
impl Tool for FakeTypeText {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        match self.page.lock().unwrap().refusal {
            None => ToolResult::text("✅ Inserted by the driver"),
            Some((code, advice)) => ToolResult::error(format!("Background input refused ({code})"))
                .with_structured(json!({
                    "code": code,
                    "effect": "refused",
                    "pid": 1,
                    "window_id": 2,
                    "escalation": {"recommended": advice},
                })),
        }
    }
}

struct FakeFields(Shared);
struct FakeField(Shared);

impl PageFields for FakeFields {
    fn find(&self, _args: &Value) -> Option<Box<dyn PageField>> {
        let mut page = self.0.lock().unwrap();
        page.finds += 1;
        page.found
            .then(|| Box::new(FakeField(self.0.clone())) as Box<dyn PageField>)
    }
}

impl PageField for FakeField {
    fn type_text(&self, text: &str) -> Option<Typed> {
        let mut page = self.0.lock().unwrap();
        let typed = page.typed.clone();
        if matches!(typed, Some(Typed::Landed { .. } | Typed::Unconfirmed)) {
            page.keys.push(text.to_owned());
        }
        typed
    }
}

fn build(page: Page) -> (ToolRegistry, Shared) {
    let shared = Arc::new(Mutex::new(page));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(FakeTypeText {
        def: ToolDef {
            name: "type_text".into(),
            description: "fake".into(),
            input_schema: json!({"type": "object", "additionalProperties": true}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        page: shared.clone(),
    }));
    let fields = shared.clone();
    cu_layer::web_typing::install_with(&mut registry, move || Box::new(FakeFields(fields)));
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

/// Each test types into its own app: the registry lets one text input run
/// per app at a time.
fn args(pid: i32) -> Value {
    json!({"pid": pid, "window_id": 2, "element_token": "s00000001:5", "text": "zzquery"})
}

const WEB_REFUSAL: Option<(&str, &str)> = Some(("background_unavailable", "px"));

#[tokio::test]
async fn a_refused_page_field_gets_the_text_as_key_events_and_shows_it() {
    let (registry, page) = build(Page {
        refusal: WEB_REFUSAL,
        typed: Some(Typed::Landed {
            value: "zzquery".into(),
        }),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(1)).await;
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert!(
        text_of(&result).starts_with("✅ Typed 7 char(s)"),
        "{}",
        text_of(&result)
    );
    assert!(text_of(&result).contains("stayed in the background"));
    assert_eq!(page.lock().unwrap().keys, ["zzquery"]);
}

#[tokio::test]
async fn keys_the_field_doesnt_show_are_reported_unverified() {
    let (registry, _) = build(Page {
        refusal: WEB_REFUSAL,
        typed: Some(Typed::Unconfirmed),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(2)).await;
    assert_ne!(result.is_error, Some(true));
    assert!(
        text_of(&result).contains("(unverified)"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn no_keys_are_sent_when_the_field_doesnt_take_focus() {
    let (registry, page) = build(Page {
        refusal: WEB_REFUSAL,
        typed: Some(Typed::FocusElsewhere),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(3)).await;
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result).contains("didn't take keyboard focus"));
    assert!(page.lock().unwrap().keys.is_empty());
}

#[tokio::test]
async fn the_refusal_stands_when_the_field_cant_be_used() {
    for (found, typed) in [(false, None), (true, None)] {
        let (registry, page) = build(Page {
            refusal: WEB_REFUSAL,
            typed,
            found,
            ..Page::default()
        });
        let result = registry.invoke("type_text", args(4)).await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            text_of(&result).contains("Background input refused (background_unavailable)"),
            "{}",
            text_of(&result)
        );
        assert!(page.lock().unwrap().keys.is_empty());
    }
}

#[tokio::test]
async fn other_answers_pass_through_untouched() {
    for refusal in [None, Some(("browser_field_needs_keystrokes", "foreground"))] {
        let (registry, page) = build(Page {
            refusal,
            typed: Some(Typed::Landed {
                value: "zzquery".into(),
            }),
            found: true,
            ..Page::default()
        });
        let result = registry.invoke("type_text", args(5)).await;
        assert_eq!(result.is_error, refusal.map(|_| true));
        assert_eq!(page.lock().unwrap().finds, 0);
    }
}
