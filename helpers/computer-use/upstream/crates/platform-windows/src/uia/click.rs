//! Semantic clicks and bounded effect readback; never equate dispatch with success.
use crate::input_outcome::{observed_change, summary, ControlState};
use cua_driver_core::action_record::{
    ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
    EvidenceKind, RequestedDelivery,
};
use cua_driver_core::protocol::ToolResult;
use std::mem::ManuallyDrop;
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

struct ComApartment(bool);
impl ComApartment {
    fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn automation() -> windows::core::Result<IUIAutomation> {
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
}

fn state(element: &IUIAutomationElement) -> ControlState {
    unsafe {
        ControlState {
            toggle: element
                .GetCurrentPattern(UIA_TogglePatternId)
                .ok()
                .and_then(|p| p.cast::<IUIAutomationTogglePattern>().ok())
                .and_then(|p| p.CurrentToggleState().ok())
                .map(|s| s.0),
            selected: element
                .GetCurrentPattern(UIA_SelectionItemPatternId)
                .ok()
                .and_then(|p| p.cast::<IUIAutomationSelectionItemPattern>().ok())
                .and_then(|p| p.CurrentIsSelected().ok())
                .map(|s| s.as_bool()),
            expanded: element
                .GetCurrentPattern(UIA_ExpandCollapsePatternId)
                .ok()
                .and_then(|p| p.cast::<IUIAutomationExpandCollapsePattern>().ok())
                .and_then(|p| p.CurrentExpandCollapseState().ok())
                .map(|s| s.0),
            value: element
                .GetCurrentPattern(UIA_ValuePatternId)
                .ok()
                .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
                .and_then(|p| p.CurrentValue().ok())
                .map(|s| s.to_string()),
        }
    }
}

pub(crate) struct Readback {
    hwnd: u64,
    _com: ComApartment,
    before: ControlState,
}
impl Readback {
    pub(crate) fn new(hwnd: u64, ptr: Option<usize>) -> Self {
        let com = ComApartment::new();
        let before = ptr
            .map(|ptr| {
                let element =
                    ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(ptr as *mut _) });
                state(&element)
            })
            .unwrap_or_default();
        Self {
            hwnd,
            _com: com,
            before,
        }
    }
    pub(crate) fn finish(
        self,
        ptr: Option<usize>,
        route: &str,
        transport: ActionTransport,
    ) -> ToolResult {
        let hwnd = HWND(self.hwnd as *mut _);
        let mut closed = false;
        let mut changed = false;
        for attempt in 0..9 {
            closed = !unsafe { IsWindow(hwnd) }.as_bool();
            if !closed {
                changed = ptr.is_some_and(|ptr| {
                    let element =
                        ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(ptr as *mut _) });
                    observed_change(&self.before, &state(&element))
                });
            }
            if closed || changed || attempt == 8 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let confirmed = closed || changed;
        let effect = if confirmed {
            ActionEffect::Confirmed
        } else {
            ActionEffect::Unverifiable
        };
        let mut record =
            ActionExecutionRecord::new(effect, transport, RequestedDelivery::Background);
        record.actual_delivery = Some(ActualDelivery::Background);
        if confirmed {
            record.evidence.push(ActionEvidence {
                kind: if closed {
                    EvidenceKind::WindowChange
                } else {
                    EvidenceKind::AccessibilityReadback
                },
                detail: if closed {
                    "Target window/dialog closed"
                } else {
                    "Readable control state changed"
                }
                .into(),
            });
        }
        ToolResult::text(summary(route, closed, changed)).with_structured(serde_json::json!({
            "path": if transport == ActionTransport::WindowsPostMessage { "post_message" } else { "ax" },
            "verified": confirmed, "effect": if confirmed { "confirmed" } else { "unverifiable" }
        })).with_action_record(record)
    }
}

/// Only a Close button under a TitleBar qualifies; Close Tab must stay an Invoke.
fn titlebar_close(element: &IUIAutomationElement) -> bool {
    unsafe {
        if element.CurrentControlType().ok() != Some(UIA_ButtonControlTypeId) {
            return false;
        }
        let name = element
            .CurrentName()
            .map(|s| s.to_string())
            .unwrap_or_default();
        let id = element
            .CurrentAutomationId()
            .map(|s| s.to_string())
            .unwrap_or_default();
        if !name.eq_ignore_ascii_case("close") && !id.eq_ignore_ascii_case("close") {
            return false;
        }
        let Ok(uia) = automation() else {
            return false;
        };
        let Ok(walker) = uia.ControlViewWalker() else {
            return false;
        };
        let mut current = element.clone();
        for _ in 0..4 {
            let Ok(parent) = walker.GetParentElement(&current) else {
                break;
            };
            if parent.CurrentControlType().ok() == Some(UIA_TitleBarControlTypeId) {
                return true;
            }
            if parent.CurrentControlType().ok() == Some(UIA_WindowControlTypeId) {
                break;
            }
            current = parent;
        }
        false
    }
}

/// None means no supported semantic pattern. A failed supported action is terminal:
/// falling back could dispatch the same command twice after a provider error.
pub(crate) fn try_click(
    hwnd: u64,
    ptr: usize,
    expand_only: bool,
) -> anyhow::Result<Option<ToolResult>> {
    let element = ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(ptr as *mut _) });
    let readback = Readback::new(hwnd, Some(ptr));
    let dispatch = || -> windows::core::Result<Option<(&str, ActionTransport)>> {
        unsafe {
            if !expand_only && titlebar_close(&element) {
                let root = automation()?.ElementFromHandle(HWND(hwnd as *mut _))?;
                if let Ok(p) = root.GetCurrentPattern(UIA_WindowPatternId) {
                    p.cast::<IUIAutomationWindowPattern>()?.Close()?;
                    return Ok(Some((
                        "WindowPattern.Close",
                        ActionTransport::WindowsUiaInvoke,
                    )));
                }
            }
            if !expand_only {
                if let Ok(p) = element.GetCurrentPattern(UIA_InvokePatternId) {
                    p.cast::<IUIAutomationInvokePattern>()?.Invoke()?;
                    return Ok(Some(("UIA Invoke", ActionTransport::WindowsUiaInvoke)));
                }
                if let Ok(p) = element.GetCurrentPattern(UIA_TogglePatternId) {
                    p.cast::<IUIAutomationTogglePattern>()?.Toggle()?;
                    return Ok(Some(("UIA Toggle", ActionTransport::WindowsUiaToggle)));
                }
                if let Ok(p) = element.GetCurrentPattern(UIA_SelectionItemPatternId) {
                    p.cast::<IUIAutomationSelectionItemPattern>()?.Select()?;
                    return Ok(Some((
                        "UIA SelectionItem.Select",
                        ActionTransport::WindowsUiaSelection,
                    )));
                }
            }
            if let Ok(p) = element.GetCurrentPattern(UIA_ExpandCollapsePatternId) {
                p.cast::<IUIAutomationExpandCollapsePattern>()?.Expand()?;
                return Ok(Some((
                    "UIA ExpandCollapse.Expand",
                    ActionTransport::WindowsUiaExpandCollapse,
                )));
            }
            Ok(None)
        }
    };
    let result = super::fg_bypass::run_with_uwp_bypass(hwnd as isize, dispatch)
        .map_err(|error| anyhow::anyhow!("UI Automation action failed: {error}; no second click was sent. Read fresh state before retrying."))?;
    Ok(result.map(|(route, transport)| readback.finish(Some(ptr), route, transport)))
}

/// Search inside the addressed window, never ElementFromPoint on the user's
/// desktop: an occluding app must not become the click target.
pub(crate) fn at_point(hwnd: u64, x: i32, y: i32) -> anyhow::Result<Option<ToolResult>> {
    let _com = ComApartment::new();
    unsafe {
        let uia = automation()?;
        let root = uia.ElementFromHandle(HWND(hwnd as *mut _))?;
        let all = root.FindAll(TreeScope_Subtree, &uia.CreateTrueCondition()?)?;
        let mut candidates = Vec::new();
        for i in 0..all.Length()?.min(5000) {
            let element = all.GetElement(i)?;
            let Ok(rect) = element.CurrentBoundingRectangle() else {
                continue;
            };
            if x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom {
                let area = i64::from(rect.right - rect.left) * i64::from(rect.bottom - rect.top);
                candidates.push((area, element));
            }
        }
        candidates.sort_by_key(|(area, _)| *area);
        for (_, element) in candidates {
            match try_click(hwnd, element.as_raw() as usize, false) {
                Ok(Some(result)) => return Ok(Some(result)),
                Ok(None) => {}
                Err(error) => return Ok(Some(ToolResult::error(error.to_string()))),
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_window_is_not_a_successful_semantic_click() {
        assert!(at_point(0, 0, 0).is_err());
    }
}
