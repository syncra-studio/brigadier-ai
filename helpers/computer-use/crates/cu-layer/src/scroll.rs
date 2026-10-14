//! Scrolling a text area stops at its end.
//!
//! The driver scrolls a native text area through its scroll-bar buttons, a
//! fixed number of presses. Around it, Brigadier reads where the text is
//! before each press: a text area that already shows its start or end
//! answers that there is nothing to scroll and sends nothing (so it also
//! works while another window covers it), a scroll stops as soon as the end
//! shows, a press the text doesn't follow gets a moment for a busy app, and
//! a scroll that couldn't get further says so instead of reporting every
//! press. Each press is one call to the driver's own scroll, so its window
//! checks still run before every press.

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// What a scroll through a text area's scroll-bar buttons did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeScroll {
    /// Presses that moved the text (or every press, when the position can't be read).
    pub steps: usize,
    /// The text reached its start or end, so the remaining presses were skipped.
    pub at_end: bool,
    /// The text stopped moving short of its end, even after waiting for the app.
    pub stalled: bool,
}

/// A press that leaves the text where it was, short of its end, usually means the app is
/// still laying the text out or is busy: wait this long and press again, up to
/// [`STALL_RETRIES`] times in a row.
const STALL_WAIT: std::time::Duration = std::time::Duration::from_millis(120);
const STALL_RETRIES: usize = 6;
const AMOUNT_MAX: u64 = 50;

/// The visible part of a text area as (first character, characters shown, all characters).
pub type TextPosition = (isize, isize, isize);

/// Whether a text view showing `(location, length, total)` can't scroll further that way.
pub fn text_view_at_end((location, length, total): TextPosition, reverse: bool) -> bool {
    if reverse {
        location <= 0
    } else {
        location + length >= total
    }
}

/// The answer for a scroll through a text area's scroll-bar buttons.
pub fn native_scroll_summary(
    direction: &str,
    by: &str,
    amount: usize,
    done: NativeScroll,
) -> String {
    let edge = if direction == "up" { "start" } else { "end" };
    if done.at_end && done.steps == 0 {
        format!(
            "✅ Nothing to scroll {direction}: the text area already shows its {edge}, so no \
             scroll was sent."
        )
    } else if done.stalled {
        format!(
            "⚠️ Scrolled native macOS control {direction} by {by} × {} of {amount} through AX: \
             the text stopped moving before its {edge}, even after waiting for the app. Take a \
             fresh get_window_state before scrolling again.",
            done.steps
        )
    } else if done.at_end && done.steps < amount {
        format!(
            "✅ Scrolled native macOS control {direction} by {by} × {} through AX and reached \
             the {edge} of the text; the other {} step(s) weren't needed.",
            done.steps,
            amount - done.steps
        )
    } else {
        format!("✅ Scrolled native macOS control {direction} by {by} × {amount} through AX.")
    }
}

/// Reads where a text area's text is. Tests supply a fake.
pub trait TextArea: Send + Sync {
    /// The text area an element-targeted `scroll` call addresses, if it is a
    /// native text area in a scroll area, as an opaque handle.
    fn find(&self, args: &Value) -> Option<Box<dyn TextHandle>>;
}

/// One text area found by [`TextArea::find`].
pub trait TextHandle: Send {
    fn position(&self) -> Option<TextPosition>;
}

/// The driver's `scroll` with the stop-at-end behaviour around it.
pub struct ScrollToEnd {
    inner: Box<dyn Tool>,
    text_area: Box<dyn TextArea>,
}

impl ScrollToEnd {
    pub fn new(inner: Box<dyn Tool>, text_area: Box<dyn TextArea>) -> Self {
        Self { inner, text_area }
    }
}

/// Replace the driver's `scroll` with [`ScrollToEnd`] around it. Call before
/// [`crate::guard::install`].
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, || Box::new(System));
}

/// [`install`] with the given text-area reader.
pub fn install_with(registry: &mut ToolRegistry, text_area: impl FnOnce() -> Box<dyn TextArea>) {
    registry.wrap("scroll", move |inner| {
        Box::new(ScrollToEnd::new(inner, text_area())) as Box<dyn Tool>
    });
}

#[async_trait]
impl Tool for ScrollToEnd {
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
        let direction = args
            .get("direction")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if !matches!(direction.as_str(), "up" | "down") || args.get("element_token").is_none() {
            return self.inner.invoke(args).await;
        }
        let Some(text) = self.text_area.find(&args) else {
            return self.inner.invoke(args).await;
        };
        let Some(mut position) = text.position() else {
            return self.inner.invoke(args).await;
        };
        let reverse = direction == "up";
        let by = args
            .get("by")
            .and_then(Value::as_str)
            .unwrap_or("line")
            .to_owned();
        let amount = args
            .get("amount")
            .and_then(Value::as_u64)
            .unwrap_or(3)
            .clamp(1, AMOUNT_MAX) as usize;

        let mut done = NativeScroll {
            steps: 0,
            at_end: false,
            stalled: false,
        };
        let mut last = None;
        let mut stalls = 0;
        let mut one_press = args.clone();
        one_press["amount"] = json!(1);
        while done.steps < amount {
            if text_view_at_end(position, reverse) {
                done.at_end = true;
                break;
            }
            let result = self.inner.invoke(one_press.clone()).await;
            if result.is_error == Some(true) {
                if done.steps == 0 && stalls == 0 {
                    return result;
                }
                last = Some(result);
                break;
            }
            last = Some(result);
            let unmoved = |now: Option<TextPosition>| {
                now.is_some_and(|now| now.0 == position.0 && !text_view_at_end(now, reverse))
            };
            let mut now = text.position();
            if unmoved(now) {
                // Give a busy app a moment; press again only if the text still hasn't moved,
                // so a late update never turns into an extra step.
                tokio::time::sleep(STALL_WAIT).await;
                now = text.position();
            }
            if unmoved(now) {
                stalls += 1;
                if stalls > STALL_RETRIES {
                    done.stalled = true;
                    break;
                }
            } else {
                stalls = 0;
                done.steps += 1;
            }
            match now {
                Some(now) => position = now,
                // The position can't be read any more: count the press and go on blind.
                None => continue,
            }
        }
        if !done.stalled && text_view_at_end(position, reverse) {
            done.at_end = true;
        }

        // The registry keeps its own structured shape for actions; the
        // answer's text says how far the scroll went.
        let structured = last
            .as_ref()
            .and_then(|result| result.structured_content.clone())
            .unwrap_or_else(|| json!({}));
        let summary = native_scroll_summary(&direction, &by, amount, done);
        match last {
            Some(result) if result.is_error == Some(true) => {
                let mut result = result;
                result.content.insert(
                    0,
                    cua_driver_core::protocol::Content::text(format!(
                        "Scrolled {} step(s) {direction} before this:",
                        done.steps
                    )),
                );
                result
            }
            _ => ToolResult::text(summary).with_structured(structured),
        }
    }
}

/// The real text areas, read through Accessibility from the driver's
/// current snapshot.
pub struct System;

#[cfg(target_os = "macos")]
impl TextArea for System {
    fn find(&self, args: &Value) -> Option<Box<dyn TextHandle>> {
        use cua_driver_core::element_token::ResolvedElement;
        use platform_macos::ax::bindings::{copy_element_attr, copy_string_attr, AXUIElementRef};
        use platform_macos::ax::snapshot::AxSnapshot;

        let token = args.get("element_token").and_then(Value::as_str)?;
        let pid = args
            .get("pid")
            .and_then(Value::as_i64)
            .or_else(|| crate::tokens::owner(token).map(|(pid, _)| pid))?;
        let store = cua_driver_core::snapshot_store::current_runtime_store::<AxSnapshot>()?;
        let ResolvedElement::Element { element, .. } =
            store.resolve(i32::try_from(pid).ok()?, args).ok()?
        else {
            return None;
        };
        let pointer = element.as_ptr() as AXUIElementRef;
        unsafe {
            if copy_string_attr(pointer, "AXRole").as_deref() != Some("AXTextArea") {
                return None;
            }
            let parent = copy_element_attr(pointer, "AXParent")?;
            let in_scroll_area =
                copy_string_attr(parent, "AXRole").as_deref() == Some("AXScrollArea");
            core_foundation::base::CFRelease(parent as core_foundation::base::CFTypeRef);
            if !in_scroll_area {
                return None;
            }
        }
        Some(Box::new(AxTextArea(element)))
    }
}

#[cfg(not(target_os = "macos"))]
impl TextArea for System {
    fn find(&self, _args: &Value) -> Option<Box<dyn TextHandle>> {
        None
    }
}

#[cfg(target_os = "macos")]
struct AxTextArea(platform_macos::ax::snapshot::RetainedElement);

#[cfg(target_os = "macos")]
impl TextHandle for AxTextArea {
    fn position(&self) -> Option<TextPosition> {
        use platform_macos::ax::bindings::{copy_number_attr, AXUIElementRef};
        let element = self.0.as_ptr() as AXUIElementRef;
        unsafe {
            let range = copy_range_attr(element, "AXVisibleCharacterRange")?;
            let total = copy_number_attr(element, "AXNumberOfCharacters")?;
            Some((range.location, range.length, total as isize))
        }
    }
}

/// Copy a `CFRange` attribute (`AXVisibleCharacterRange`).
#[cfg(target_os = "macos")]
unsafe fn copy_range_attr(
    element: platform_macos::ax::bindings::AXUIElementRef,
    attr_name: &str,
) -> Option<core_foundation::base::CFRange> {
    use core_foundation::base::{CFGetTypeID, CFRelease, CFTypeID, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use platform_macos::ax::bindings::{
        kAXErrorSuccess, kAXValueCFRangeType, AXUIElementCopyAttributeValue, AXValueGetType,
        AXValueGetValue, AXValueRef,
    };
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXValueGetTypeID() -> CFTypeID;
    }
    let attr = CFString::new(attr_name);
    let mut value: CFTypeRef = std::ptr::null();
    let err = AXUIElementCopyAttributeValue(element, attr.as_concrete_TypeRef(), &mut value);
    if err != kAXErrorSuccess || value.is_null() {
        return None;
    }
    if CFGetTypeID(value) != AXValueGetTypeID()
        || AXValueGetType(value as AXValueRef) != kAXValueCFRangeType
    {
        CFRelease(value);
        return None;
    }
    let mut range = core_foundation::base::CFRange::init(0, 0);
    let ok = AXValueGetValue(
        value as AXValueRef,
        kAXValueCFRangeType,
        &mut range as *mut _ as *mut std::ffi::c_void,
    );
    CFRelease(value);
    ok.then_some(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_view_is_at_its_end_only_when_its_last_character_shows() {
        assert!(text_view_at_end((7830, 869, 8699), false));
        assert!(!text_view_at_end((1653, 899, 8699), false));
        assert!(text_view_at_end((0, 870, 8699), true));
        assert!(!text_view_at_end((29, 870, 8699), true));
    }

    #[test]
    fn a_scroll_that_stopped_short_of_the_end_says_so() {
        let stalled = NativeScroll {
            steps: 2,
            at_end: false,
            stalled: true,
        };
        let text = native_scroll_summary("down", "page", 50, stalled);
        assert!(text.starts_with("⚠️"), "{text}");
        assert!(text.contains("× 2 of 50"), "{text}");
        let ended = NativeScroll {
            steps: 11,
            at_end: true,
            stalled: false,
        };
        let text = native_scroll_summary("down", "page", 50, ended);
        assert!(
            text.starts_with("✅") && text.contains("reached the end"),
            "{text}"
        );
        let full = NativeScroll {
            steps: 3,
            at_end: false,
            stalled: false,
        };
        assert!(native_scroll_summary("up", "line", 3, full).starts_with("✅ Scrolled"));
        let nothing = NativeScroll {
            steps: 0,
            at_end: true,
            stalled: false,
        };
        let text = native_scroll_summary("up", "line", 2, nothing);
        assert!(
            text.starts_with("✅ Nothing to scroll up") && text.contains("its start"),
            "{text}"
        );
    }
}
