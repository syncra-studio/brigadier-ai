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
    /// The driver typed but none of the text went in.
    incomplete: bool,
    /// The target window is on another Space than the user's.
    other_space: bool,
    /// What the browser's debugging port gives when the keys didn't show.
    debug_port: Option<Typed>,
    /// Whether the element can be found again.
    found: bool,
    finds: usize,
    /// How often the driver's tool ran.
    driver_calls: usize,
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
        let mut page = self.page.lock().unwrap();
        page.driver_calls += 1;
        if page.incomplete {
            return ToolResult::error(
                "type_text incomplete: delivered 0 of 7 character(s); retry only the remaining suffix",
            )
            .with_structured(json!({
                "code": "type_text_incomplete",
                "effect": "partial",
                "delivered_chars": 0,
                "retryable": true,
            }));
        }
        match page.refusal {
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

#[async_trait]
impl PageFields for FakeFields {
    fn find(&self, _args: &Value) -> Option<Box<dyn PageField>> {
        let mut page = self.0.lock().unwrap();
        page.finds += 1;
        page.found
            .then(|| Box::new(FakeField(self.0.clone())) as Box<dyn PageField>)
    }

    fn on_other_space(&self, _args: &Value) -> bool {
        self.0.lock().unwrap().other_space
    }

    async fn insert_over_debug_port(&self, _args: &Value, _text: &str) -> Option<Typed> {
        self.0.lock().unwrap().debug_port.clone()
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

fn code_of(result: &ToolResult) -> Option<String> {
    result
        .structured_content
        .as_ref()?
        .get("code")?
        .as_str()
        .map(str::to_owned)
}

#[tokio::test]
async fn a_page_on_another_space_is_typed_into_through_its_key_window_without_the_drivers_keys() {
    let (registry, page) = build(Page {
        incomplete: true,
        other_space: true,
        typed: Some(Typed::Landed {
            value: "zzquery".into(),
        }),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(6)).await;
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{text}");
    assert!(text.starts_with("✅ Typed 7 char(s)"), "{text}");
    let page = page.lock().unwrap();
    assert_eq!(page.keys, ["zzquery"]);
    assert_eq!(page.driver_calls, 0);
}

#[tokio::test]
async fn a_page_on_another_space_whose_field_cant_be_used_gets_the_drivers_answer() {
    let (registry, page) = build(Page {
        incomplete: true,
        other_space: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(11)).await;
    assert_eq!(code_of(&result).as_deref(), Some("type_text_incomplete"));
    assert_eq!(page.lock().unwrap().driver_calls, 1);
}

#[tokio::test]
async fn the_drivers_answer_stands_when_its_keys_reached_nothing_on_the_users_space() {
    let (registry, page) = build(Page {
        incomplete: true,
        found: true,
        typed: Some(Typed::Landed {
            value: "zzquery".into(),
        }),
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(8)).await;
    assert_eq!(code_of(&result).as_deref(), Some("type_text_incomplete"));
    assert_eq!(page.lock().unwrap().finds, 0);
}

#[tokio::test]
async fn keys_a_browser_page_didnt_take_go_in_through_its_debugging_port() {
    let (registry, page) = build(Page {
        refusal: WEB_REFUSAL,
        typed: Some(Typed::Unconfirmed),
        debug_port: Some(Typed::Inserted {
            value: "zzquery".into(),
        }),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(9)).await;
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{text}");
    assert!(
        text.contains("through the browser's debugging port"),
        "{text}"
    );
    assert_eq!(page.lock().unwrap().keys, ["zzquery"]);
}

#[tokio::test]
async fn keys_that_landed_late_are_not_inserted_again() {
    let (registry, _) = build(Page {
        refusal: WEB_REFUSAL,
        typed: Some(Typed::Unconfirmed),
        debug_port: Some(Typed::Landed {
            value: "zzquery".into(),
        }),
        found: true,
        ..Page::default()
    });
    let result = registry.invoke("type_text", args(10)).await;
    let text = text_of(&result);
    assert!(
        text.starts_with("✅ Typed 7 char(s) into the page's field as key events"),
        "{text}"
    );
}

/// A stand-in page: `el(tag, {id, value, text, label, attrs, ce})` makes an
/// element, `add(...)` puts it in the page, `focus(e)` focuses it.
const FAKE_PAGE: &str = r#"
function el(tag, o) {
  o = o || {}; const attrs = o.attrs || {};
  return { tagName: tag, id: o.id || '', value: o.value, textContent: o.text || '',
           isContentEditable: !!o.ce, labels: o.label ? [{ textContent: o.label }] : [],
           getAttribute: k => (k in attrs ? attrs[k] : null) };
}
var all = [];
var document = {
  activeElement: null,
  querySelectorAll(sel) {
    if (sel === '[id]') return all.filter(x => x.id);
    return all.filter(x => x.tagName === 'INPUT' || x.tagName === 'TEXTAREA' || x.isContentEditable);
  },
};
function add(e) { all.push(e); return e; }
function focus(e) { document.activeElement = e; }
"#;

/// What the probe answers on a page set up by `page` (JavaScript).
fn probe(page: &str, dom_id: &str, names: &[&str], text: &str) -> String {
    let names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    let script = cu_layer::web_typing::field_probe(dom_id, &names, text);
    let runtime = rquickjs::Runtime::new().unwrap();
    let context = rquickjs::Context::full(&runtime).unwrap();
    context.with(|ctx| {
        ctx.eval::<(), _>(format!("{FAKE_PAGE}\n{page}")).unwrap();
        ctx.eval::<String, _>(script).unwrap()
    })
}

#[test]
fn the_field_with_the_targets_dom_id_is_used() {
    let page = "add(el('INPUT', {id: 'q'})); focus(add(el('INPUT', {id: 'name', value: ''})));";
    assert_eq!(probe(page, "name", &[], "zz"), "missing");
    let typed = "focus(add(el('INPUT', {id: 'name', value: 'a zz b'})));";
    assert_eq!(probe(typed, "name", &[], "zz"), "there:a zz b");
}

#[test]
fn a_focused_field_with_another_dom_id_gets_nothing() {
    let page = "add(el('INPUT', {id: 'name'})); focus(add(el('INPUT', {id: 'q', value: ''})));";
    assert_eq!(probe(page, "name", &["Name"], "zz"), "none");
}

#[test]
fn a_dom_id_used_twice_in_the_page_ties_nothing() {
    let page = "add(el('INPUT', {id: 'name'})); focus(add(el('INPUT', {id: 'name', value: ''})));";
    assert_eq!(probe(page, "name", &[], "zz"), "none");
}

#[test]
fn without_a_dom_id_the_one_field_with_the_targets_name_is_used() {
    let page = "add(el('INPUT', {attrs: {'aria-label': 'Search'}})); \
                focus(add(el('INPUT', {value: '', label: 'Name '})));";
    assert_eq!(probe(page, "", &["Name"], "zz"), "missing");
    let placeholder = "focus(add(el('TEXTAREA', {value: '', attrs: {placeholder: 'Message'}})));";
    assert_eq!(
        probe(placeholder, "", &["Note", "Message"], "zz"),
        "missing"
    );
}

#[test]
fn without_a_dom_id_any_other_focused_field_gets_nothing() {
    // The page's focus is on a field the agent didn't target.
    let unnamed =
        "add(el('INPUT', {attrs: {'aria-label': 'Name'}})); focus(add(el('INPUT', {value: ''})));";
    assert_eq!(probe(unnamed, "", &["Name"], "zz"), "none");
    let other = "add(el('INPUT', {attrs: {'aria-label': 'Name'}})); \
                 focus(add(el('INPUT', {value: '', attrs: {'aria-label': 'Email'}})));";
    assert_eq!(probe(other, "", &["Name"], "zz"), "none");
    // Nothing on the target to match by.
    let any = "focus(add(el('INPUT', {value: '', attrs: {'aria-label': 'Name'}})));";
    assert_eq!(probe(any, "", &[], "zz"), "none");
}

#[test]
fn without_a_dom_id_a_name_two_fields_share_ties_nothing() {
    let page = "add(el('INPUT', {attrs: {'aria-label': 'Search'}})); \
                focus(add(el('INPUT', {value: '', attrs: {'aria-label': 'Search'}})));";
    assert_eq!(probe(page, "", &["Search"], "zz"), "none");
}

#[test]
fn focus_on_something_that_isnt_a_field_gets_nothing() {
    let page = "add(el('INPUT', {id: 'name'})); focus(add(el('BUTTON', {id: 'name'})));";
    assert_eq!(probe(page, "name", &[], "zz"), "none");
}

#[test]
fn keys_that_landed_in_an_editable_element_are_read_from_its_text() {
    let page = "focus(add(el('DIV', {id: 'doc', ce: true, text: 'zz here'})));";
    assert_eq!(probe(page, "doc", &[], "zz"), "there:zz here");
}
