//! Typing into a desktop web app's page (Electron, Chromium) while its window
//! stays behind the user's work.
//!
//! The driver refuses an element-targeted `type_text` into such a page in the
//! background: its accessibility insert isn't seen by the page, and key
//! events reach the page only while its window is the app's key window.
//! Around the driver's tool, Brigadier answers that refusal itself: it
//! focuses the element, makes its window the app's key window without
//! bringing the app forward ([`crate::key_window`]), sends the text as key
//! events to the app, and reads the field back. The refusal stands when the
//! app is the one the user is working in (its key window is the user's), or
//! when the element can't be found again.
//!
//! In a window on another Space than the user's, the driver's own keys
//! don't reach the page, so such a call goes straight to this route.
//!
//! A page whose window isn't being drawn (covered, or on another Space) gets
//! the keys but tells accessibility about its new text only when it next
//! handles an event, so the read-back asks the field to scroll itself into
//! view (nothing moves for a field already in view) until the text shows.
//! A browser window opened behind the user's work may take no keys at all;
//! when the field still doesn't show the text and the browser has a
//! debugging port, the page's focused field gets the text through it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cua_driver_core::action_record::{
    ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
    EvidenceKind, RequestedDelivery,
};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// How long the field gets to show the typed text.
const READ_BACK_WAIT: Duration = Duration::from_millis(600);
const READ_BACK_POLL: Duration = Duration::from_millis(20);

/// What typing into a page field did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Typed {
    /// The key events didn't show; the text went in through the browser's
    /// debugging port and the page's field holds it.
    Inserted { value: String },
    /// The field shows the text.
    Landed { value: String },
    /// The keys were sent but the field doesn't show the text.
    Unconfirmed,
    /// The element didn't take focus, so no keys were sent.
    FocusElsewhere,
}

/// Finds the page element a refused call addressed. Tests supply a fake.
#[async_trait]
pub trait PageFields: Send + Sync {
    /// `None` when the route doesn't apply (the app is in front, the element
    /// or its window can't be found): the driver's refusal then stands.
    fn find(&self, args: &Value) -> Option<Box<dyn PageField>>;

    /// Whether the target is a desktop web app's window on another Space
    /// than the user's, where the driver's own keys don't reach the page.
    fn on_other_space(&self, _args: &Value) -> bool {
        false
    }

    /// When the key events didn't show: the page's focused field through
    /// the browser's debugging port. [`Typed::Landed`] when it already holds
    /// `text`, [`Typed::Inserted`] when it does after inserting it, `None`
    /// when there's no port or the page's focus is on something else.
    async fn insert_over_debug_port(&self, _args: &Value, _text: &str) -> Option<Typed> {
        None
    }

    /// The app and window a `click` acts on, and whether it presses a text
    /// field inside a web page. `None` when they can't be told.
    fn click_target(&self, _args: &Value) -> Option<ClickTarget> {
        None
    }

    /// Whether this window's focus is on an editable field outside its web
    /// page: the browser's own address or search field. An unknown focus
    /// is `false`.
    fn focus_on_browser_chrome(&self, _pid: i32, _window_id: u32) -> bool {
        false
    }
}

/// What a `click` acts on. See [`PageFields::click_target`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClickTarget {
    pub pid: i32,
    pub window_id: u32,
    pub presses_page_field: bool,
}

/// The page field each app's last click pressed: its window and its token.
type ClickedFields = Arc<Mutex<HashMap<i32, (u32, String)>>>;

/// One page element found by [`PageFields::find`].
pub trait PageField: Send {
    /// Types `text` into the element. `None` when its window couldn't be
    /// made key; nothing was typed then.
    fn type_text(&self, text: &str) -> Option<Typed>;
}

/// Whether `result` is the driver's refusal of an element-targeted text
/// insert into web content, which pixel-targeted key events would reach.
pub fn is_web_content_refusal(result: &ToolResult) -> bool {
    result.is_error == Some(true)
        && result.structured_content.as_ref().is_some_and(|s| {
            s.get("code").and_then(Value::as_str) == Some("background_unavailable")
                && s.pointer("/escalation/recommended").and_then(Value::as_str) == Some("px")
        })
}

/// The answer for a typing done here.
pub fn typed_result(typed: &Typed, text: &str, args: &Value) -> ToolResult {
    let count = text.chars().count();
    let mut structured = json!({
        "route": "key_events_background_key_window",
        "characters": count,
    });
    for key in ["pid", "window_id"] {
        if let Some(value) = args.get(key) {
            structured[key] = value.clone();
        }
    }
    let record = |effect| {
        ActionExecutionRecord::builder(
            effect,
            ActionTransport::MacosCgEventPid,
            RequestedDelivery::Background,
        )
    };
    let delivered = |effect| {
        record(effect)
            .actual_delivery(ActualDelivery::Background)
            .delivered_count(count as u32)
    };
    match typed {
        Typed::Landed { value } => {
            structured["verified"] = json!(true);
            structured["value"] = json!(value);
            let result = ToolResult::text(format!(
                "✅ Typed {count} char(s) into the page's field as key events; it now shows \
                 {value:?}. The app stayed in the background."
            ))
            .with_structured(structured);
            match delivered(ActionEffect::Confirmed)
                .evidence(ActionEvidence {
                    kind: EvidenceKind::AccessibilityReadback,
                    detail: "the field's value shows the typed text".into(),
                })
                .build()
            {
                Ok(record) => result.with_action_record(record),
                Err(_) => result,
            }
        }
        Typed::Inserted { value } => {
            structured["route"] = json!("debugging_port_insert_text");
            structured["verified"] = json!(true);
            structured["value"] = json!(value);
            let result = ToolResult::text(format!(
                "✅ Typed {count} char(s) into the page's field through the browser's \
                 debugging port (its key events didn't reach the page); it now shows \
                 {value:?}. The app stayed in the background."
            ))
            .with_structured(structured);
            match ActionExecutionRecord::builder(
                ActionEffect::Confirmed,
                ActionTransport::BrowserCdpInputKey,
                RequestedDelivery::Background,
            )
            .actual_delivery(ActualDelivery::Background)
            .delivered_count(count as u32)
            .evidence(ActionEvidence {
                kind: EvidenceKind::BrowserReadback,
                detail: "the page's focused field holds the typed text".into(),
            })
            .build()
            {
                Ok(record) => result.with_action_record(record),
                Err(_) => result,
            }
        }
        Typed::Unconfirmed => {
            structured["verified"] = json!(false);
            let result = ToolResult::text(format!(
                "📨 Sent {count} char(s) as key events to the page's field (unverified): the \
                 field doesn't show them yet. Read the window again before typing more."
            ))
            .with_structured(structured);
            match delivered(ActionEffect::Unverifiable).build() {
                Ok(record) => result.with_action_record(record),
                Err(_) => result,
            }
        }
        Typed::FocusElsewhere => {
            structured["code"] = json!("focus_elsewhere");
            structured["effect"] = json!("refused");
            let result = ToolResult::error(
                "Nothing was typed: the page's field didn't take keyboard focus, so key events \
                 would have gone to another element of the page. Click the field first, then \
                 type again.",
            )
            .with_structured(structured);
            match record(ActionEffect::Refused)
                .refusal("focus_elsewhere", None)
                .build()
            {
                Ok(record) => result.with_action_record(record),
                Err(_) => result,
            }
        }
    }
}

/// The driver's `type_text` with page typing around its refusal.
pub struct TypeIntoPages {
    inner: Box<dyn Tool>,
    fields: Box<dyn PageFields>,
    clicked: ClickedFields,
}

impl TypeIntoPages {
    pub fn new(inner: Box<dyn Tool>, fields: Box<dyn PageFields>) -> Self {
        Self {
            inner,
            fields,
            clicked: ClickedFields::default(),
        }
    }
}

/// The driver's `click`, noting when it pressed a text field inside a web
/// page. In a background Safari window the press focuses the page's field but
/// not the app's, so text typed without a target would go to the address bar.
struct NoteClickedFields {
    inner: Box<dyn Tool>,
    fields: Box<dyn PageFields>,
    clicked: ClickedFields,
}

#[async_trait]
impl Tool for NoteClickedFields {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let target = self.fields.click_target(&args);
        let token = args.get("element_token").and_then(Value::as_str);
        let token = token.map(str::to_owned);
        let result = self.inner.invoke(args).await;
        if let (Some(target), false) = (target, result.is_error == Some(true)) {
            let mut clicked = self
                .clicked
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match token.filter(|_| target.presses_page_field) {
                Some(token) => clicked.insert(target.pid, (target.window_id, token)),
                None => clicked.remove(&target.pid),
            };
        }
        result
    }
}

/// Replace the driver's `type_text` with [`TypeIntoPages`] around it. Call
/// before [`crate::guard::install`].
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, || Box::new(System));
}

/// [`install`] with the given page fields.
pub fn install_with(registry: &mut ToolRegistry, fields: impl Fn() -> Box<dyn PageFields>) {
    let clicked = ClickedFields::default();
    registry.wrap("click", |inner| {
        Box::new(NoteClickedFields {
            inner,
            fields: fields(),
            clicked: clicked.clone(),
        }) as Box<dyn Tool>
    });
    registry.wrap("type_text", |inner| {
        Box::new(TypeIntoPages {
            inner,
            fields: fields(),
            clicked,
        }) as Box<dyn Tool>
    });
}

#[async_trait]
impl Tool for TypeIntoPages {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(text) = args.get("text").and_then(Value::as_str).map(str::to_owned) else {
            return self.inner.invoke(args).await;
        };
        if let Some(refusal) = self.refusal_for_focus_on_browser_chrome(&args) {
            return refusal;
        }
        // The driver's keys don't reach a page on another Space: go straight
        // to the key window.
        if self.fields.on_other_space(&args) {
            if let Some(typed) = self.type_into_page(&args, &text).await {
                return typed;
            }
            return self.inner.invoke(args).await;
        }
        let result = self.inner.invoke(args.clone()).await;
        if !is_web_content_refusal(&result) {
            return result;
        }
        self.type_into_page(&args, &text).await.unwrap_or(result)
    }
}

impl TypeIntoPages {
    /// The answer to text without a target while the page field the last
    /// click pressed didn't get the app's focus, which stayed on the browser's
    /// own field. A focus that has moved on ends the watch.
    fn refusal_for_focus_on_browser_chrome(&self, args: &Value) -> Option<ToolResult> {
        if ["element_token", "element_index", "x", "y"]
            .iter()
            .any(|key| args.get(key).is_some())
        {
            return None;
        }
        let pid = i32::try_from(args.get("pid")?.as_i64()?).ok()?;
        let mut clicked = self
            .clicked
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (window_id, token) = clicked.get(&pid)?.clone();
        if args
            .get("window_id")
            .and_then(Value::as_u64)
            .is_some_and(|asked| asked != u64::from(window_id))
        {
            return None;
        }
        if !self.fields.focus_on_browser_chrome(pid, window_id) {
            clicked.remove(&pid);
            return None;
        }
        Some(
            ToolResult::error(format!(
                "Nothing was typed: the page field you clicked didn't get the app's focus, which \
                 is still on the browser's own address or search field. Pass element_token \
                 \"{token}\" to type into that field, or use the page tool. To type into the \
                 browser's own field, click it first."
            ))
            .with_structured(json!({
                "code": "focus_on_browser_chrome",
                "delivered": false,
                "clicked_element_token": token,
            })),
        )
    }

    /// Types into the page's field as key events, then through the
    /// browser's debugging port if the keys don't show. `None` when the
    /// field can't be used.
    async fn type_into_page(&self, args: &Value, text: &str) -> Option<ToolResult> {
        let field = self.fields.find(args)?;
        let typing = text.to_owned();
        let typed = tokio::task::spawn_blocking(move || field.type_text(&typing))
            .await
            .ok()??;
        let typed = match typed {
            Typed::Unconfirmed => self
                .fields
                .insert_over_debug_port(args, text)
                .await
                .unwrap_or(Typed::Unconfirmed),
            typed => typed,
        };
        Some(typed_result(&typed, text, args))
    }
}

/// The real page fields, found through the driver's current snapshot.
pub struct System;

#[cfg(target_os = "macos")]
#[async_trait]
impl PageFields for System {
    fn find(&self, args: &Value) -> Option<Box<dyn PageField>> {
        use cua_driver_core::element_token::ResolvedElement;
        use platform_macos::ax::snapshot::AxSnapshot;

        let (pid, window_id) = target(args)?;
        // The app the user works in keeps its own key window.
        if platform_macos::input::skylight::front_process_pid() == Some(pid) {
            return None;
        }
        let store = cua_driver_core::snapshot_store::current_runtime_store::<AxSnapshot>()?;
        let ResolvedElement::Element { element, .. } = store.resolve(pid, args).ok()? else {
            return None;
        };
        Some(Box::new(AxPageField {
            pid,
            window_id,
            element,
        }))
    }

    fn on_other_space(&self, args: &Value) -> bool {
        target(args).is_some_and(|(pid, window_id)| {
            crate::hotkey::chromium_family(pid)
                && platform_macos::windows::window_on_current_space_by_id(window_id) == Some(false)
        })
    }

    fn click_target(&self, args: &Value) -> Option<ClickTarget> {
        use cua_driver_core::element_token::ResolvedElement;
        use platform_macos::ax::bindings::{copy_string_attr, AXUIElementRef};
        use platform_macos::ax::snapshot::AxSnapshot;

        let (pid, window_id) = target(args)?;
        let press = args
            .get("action")
            .and_then(Value::as_str)
            .is_none_or(|action| action == "press");
        let presses_page_field = press
            && args.get("element_token").is_some()
            && cua_driver_core::snapshot_store::current_runtime_store::<AxSnapshot>()
                .and_then(|store| store.resolve(pid, args).ok())
                .is_some_and(|resolved| {
                    let ResolvedElement::Element { element, .. } = resolved else {
                        return false;
                    };
                    let pointer = element.as_ptr() as AXUIElementRef;
                    let role = unsafe { copy_string_attr(pointer, "AXRole") };
                    is_field_role(role.as_deref()) && unsafe { inside_web_area(pointer) }
                });
        Some(ClickTarget {
            pid,
            window_id,
            presses_page_field,
        })
    }

    fn focus_on_browser_chrome(&self, pid: i32, window_id: u32) -> bool {
        use core_foundation::base::{CFRelease, CFTypeRef};
        use platform_macos::ax::bindings::copy_string_attr;

        unsafe {
            let Some(focused) =
                platform_macos::ax::exact_target::focused_element_in_window(pid, window_id)
            else {
                return false;
            };
            let chrome = is_field_role(copy_string_attr(focused, "AXRole").as_deref())
                && !inside_web_area(focused);
            CFRelease(focused as CFTypeRef);
            chrome
        }
    }

    async fn insert_over_debug_port(&self, args: &Value, text: &str) -> Option<Typed> {
        use cua_driver_core::element_token::ResolvedElement;
        use platform_macos::ax::bindings::{copy_string_attr, AXUIElementRef};
        use platform_macos::ax::snapshot::AxSnapshot;

        if text.is_empty() {
            return None;
        }
        let (pid, window_id) = target(args)?;
        let port = crate::page::cdp_port_for_pid(pid).await?;
        // What ties the page's focused element to this field: its DOM id,
        // or else its accessible names.
        let (dom_id, names) = {
            let store = cua_driver_core::snapshot_store::current_runtime_store::<AxSnapshot>()?;
            let ResolvedElement::Element { element, .. } = store.resolve(pid, args).ok()? else {
                return None;
            };
            let pointer = element.as_ptr() as AXUIElementRef;
            let read = |attr| unsafe { copy_string_attr(pointer, attr) };
            let names: Vec<String> = ["AXTitle", "AXDescription", "AXPlaceholderValue"]
                .into_iter()
                .filter_map(read)
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect();
            (read("AXDOMIdentifier").unwrap_or_default(), names)
        };
        let title = tokio::task::spawn_blocking(move || {
            crate::window_list::windows(Some(pid))
                .into_iter()
                .find(|window| window.id == window_id)
                .map(|window| window.title)
        })
        .await
        .ok()
        .flatten();
        let sessions = crate::page::shared_cdp_sessions();
        let probe = field_probe(&dom_id, &names, text);
        let read = || async {
            sessions
                .evaluate_in_window(&probe, port, title.as_deref())
                .await
                .ok()
        };
        let state = read().await?;
        if let Some(value) = state.strip_prefix("there:") {
            return Some(Typed::Landed {
                value: value.to_owned(),
            });
        }
        if state != "missing" {
            return None;
        }
        sessions
            .insert_text_in_window(text, port, title.as_deref())
            .await
            .ok()?;
        read()
            .await?
            .strip_prefix("there:")
            .map(|value| Typed::Inserted {
                value: value.to_owned(),
            })
    }
}

/// JavaScript answering `there:<value>` when the page's focused editable
/// element is the field and holds `text`, `missing` when it is the field
/// without the text, and `none` when the field can't be told apart for sure:
/// the focus is elsewhere, or the page doesn't tie the focused element to
/// the field. It is the field when it has `dom_id` and no other element has
/// that id, or, without a DOM id, when its accessible name (label, ARIA
/// label, placeholder or title) is one of `names` and no other editable
/// element of the page has that name. Text goes in only for `missing`.
pub fn field_probe(dom_id: &str, names: &[String], text: &str) -> String {
    let id = serde_json::to_string(dom_id).unwrap_or_else(|_| "\"\"".into());
    let names = serde_json::to_string(names).unwrap_or_else(|_| "[]".into());
    let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        "(() => {{ \
         const editable = x => !!x && (x.isContentEditable || x.tagName === 'INPUT' || x.tagName === 'TEXTAREA'); \
         const e = document.activeElement; if (!editable(e)) return 'none'; \
         const id = {id}; const names = {names}; \
         const nameOf = x => String(x.getAttribute('aria-label') || (x.labels && x.labels[0] && x.labels[0].textContent) \
           || x.getAttribute('placeholder') || x.getAttribute('title') || '').trim(); \
         let same = false; \
         if (id) {{ same = e.id === id && Array.from(document.querySelectorAll('[id]')).filter(x => x.id === id).length === 1; }} \
         else if (names.length) {{ const n = nameOf(e); \
           same = n !== '' && names.includes(n) && Array.from(document.querySelectorAll('input, textarea, [contenteditable]')) \
             .filter(x => editable(x) && nameOf(x) === n).length === 1; }} \
         if (!same) return 'none'; \
         const v = (e.value !== undefined ? e.value : e.textContent) || ''; \
         return v.includes({text}) ? 'there:' + v : 'missing'; }})()"
    )
}

/// A role that takes typed text in a page or in a browser's own bar.
#[cfg(target_os = "macos")]
fn is_field_role(role: Option<&str>) -> bool {
    matches!(
        role,
        Some("AXTextField" | "AXTextArea" | "AXComboBox" | "AXSearchField")
    )
}

/// Whether `element` sits inside a page's `AXWebArea`. Unknown is `false`.
///
/// # Safety
///
/// `element` must be valid; it stays owned by the caller.
#[cfg(target_os = "macos")]
unsafe fn inside_web_area(element: platform_macos::ax::bindings::AXUIElementRef) -> bool {
    use core_foundation::base::{CFRelease, CFTypeRef};
    use platform_macos::ax::bindings::{copy_element_attr, copy_string_attr};

    /// Deeper than any page field; only bounds a malformed tree.
    const MAX_DEPTH: usize = 64;

    let mut current = copy_element_attr(element, "AXParent");
    for _ in 0..MAX_DEPTH {
        let Some(parent) = current else {
            return false;
        };
        let found = copy_string_attr(parent, "AXRole").as_deref() == Some("AXWebArea");
        current = (!found)
            .then(|| copy_element_attr(parent, "AXParent"))
            .flatten();
        CFRelease(parent as CFTypeRef);
        if found {
            return true;
        }
    }
    if let Some(parent) = current {
        CFRelease(parent as CFTypeRef);
    }
    false
}

/// The target's app and window, from the arguments or the element token.
#[cfg(target_os = "macos")]
fn target(args: &Value) -> Option<(i32, u32)> {
    let owner = args
        .get("element_token")
        .and_then(Value::as_str)
        .and_then(crate::tokens::owner);
    let pid = args
        .get("pid")
        .and_then(Value::as_i64)
        .or(owner.map(|(pid, _)| pid))?;
    let window_id = args
        .get("window_id")
        .and_then(Value::as_u64)
        .or(owner.and_then(|(_, window)| window))?;
    Some((i32::try_from(pid).ok()?, u32::try_from(window_id).ok()?))
}

#[cfg(not(target_os = "macos"))]
impl PageFields for System {
    fn find(&self, _args: &Value) -> Option<Box<dyn PageField>> {
        None
    }
}

#[cfg(target_os = "macos")]
struct AxPageField {
    pid: i32,
    window_id: u32,
    element: platform_macos::ax::snapshot::RetainedElement,
}

#[cfg(target_os = "macos")]
impl PageField for AxPageField {
    fn type_text(&self, text: &str) -> Option<Typed> {
        use platform_macos::ax::bindings::{
            copy_bool_attr, copy_string_attr, perform_action, AXUIElementRef,
        };
        use platform_macos::input::{ax_actions, keyboard};

        let pointer = self.element.as_ptr() as AXUIElementRef;
        let value = || unsafe { copy_string_attr(pointer, "AXValue") }.unwrap_or_default();

        let _ = ax_actions::focus_element(pointer as usize);
        if !crate::key_window::make_key_in_background(self.pid, self.window_id) {
            return None;
        }
        std::thread::sleep(crate::key_window::SETTLE);
        // A background app names no focused element of its own; the
        // element itself says whether it has the page's focus. One that
        // can't tell gets the keys, and the read-back decides.
        if unsafe { copy_bool_attr(pointer, "AXFocused") } == Some(false) {
            return Some(Typed::FocusElsewhere);
        }
        let before = value();
        keyboard::type_text(self.pid, text).ok()?;
        let deadline = Instant::now() + READ_BACK_WAIT;
        loop {
            let now = value();
            if now != before && now.contains(text) {
                return Some(Typed::Landed { value: now });
            }
            if Instant::now() >= deadline {
                return Some(Typed::Unconfirmed);
            }
            // A page that isn't being drawn sends its pending text changes
            // when it handles this.
            let _ = unsafe { perform_action(pointer, "AXScrollToVisible") };
            std::thread::sleep(READ_BACK_POLL);
        }
    }
}
