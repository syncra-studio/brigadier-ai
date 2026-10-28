//! Semantic clicks and bounded effect readback; never equate dispatch with success.
use crate::input_outcome::{
    observed_change, pattern_order, summary, window_closed, ClickPattern, ControlState,
};
use cua_driver_core::action_record::{
    ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
    EvidenceKind, RequestedDelivery,
};
use cua_driver_core::protocol::ToolResult;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use windows::core::Interface;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetDlgCtrlID, GetDlgItem, GetParent, GetWindowRect, IsWindow,
    IsWindowEnabled, SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_CLOSE, WM_COMMAND,
    PostMessageW,
};

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
    present_before: bool,
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
            present_before: unsafe { IsWindow(HWND(hwnd as *mut _)) }.as_bool(),
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
            closed = window_closed(self.present_before, unsafe { IsWindow(hwnd) }.as_bool());
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
            "path": match transport { ActionTransport::WindowsPostMessage => "post_message", ActionTransport::WindowsTargetedInjection => "pixel", _ => "ax" },
            "verified": confirmed, "effect": if confirmed { "confirmed" } else { "unverifiable" }
        })).with_action_record(record)
    }
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..n.max(0) as usize])
}

/// Bounded synchronous dispatch keeps the background guard alive while the
/// target handles the command. A timeout is never retried: it may have run.
fn send_command(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> windows::core::Result<()> {
    let mut result = 0usize;
    let sent = unsafe { SendMessageTimeoutW(hwnd, message, wparam, lparam, SMTO_ABORTIFHUNG, 1000, Some(&mut result)) };
    if sent.0 == 0 { return Err(windows::core::Error::from_win32()); }
    Ok(())
}

/// Exact IDOK control in a native common dialog. Do this before Invoke:
/// UIA can reject the disabled dialog's button with UIA_E_ELEMENTNOTENABLED.
fn common_dialog_button(hwnd: HWND, element: &IUIAutomationElement) -> Option<HWND> {
    unsafe {
        if element.CurrentControlType().ok()? != UIA_ButtonControlTypeId { return None; }
        let button = GetDlgItem(hwnd, 1).ok()?;
        if !crate::input::action_policy::common_dialog_accept(&class_name(hwnd), &class_name(button), GetDlgCtrlID(button))
            || GetParent(button).ok()? != hwnd || !IsWindowEnabled(button).as_bool() { return None; }
        let native = element.CurrentNativeWindowHandle().ok();
        if native == Some(button) { return Some(button); }
        if element.CurrentAutomationId().ok()?.to_string() != "1" { return None; }
        let r = element.CurrentBoundingRectangle().ok()?;
        let mut b = windows::Win32::Foundation::RECT::default();
        GetWindowRect(button, &mut b).ok()?;
        let x = r.left + (r.right-r.left)/2;
        let y = r.top + (r.bottom-r.top)/2;
        (x >= b.left && x < b.right && y >= b.top && y < b.bottom).then_some(button)
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
                return crate::input_outcome::close_button_identity(true, true, &name, &id);
            }
            if parent.CurrentControlType().ok() == Some(UIA_WindowControlTypeId) {
                break;
            }
            current = parent;
        }
        false
    }
}

/// Cancellation is checked after provider lookups and immediately before each
/// mutation: a late provider return must not send input after the caller timed out.
pub(crate) fn ensure_active(cancelled: &AtomicBool) -> windows::core::Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(windows::core::Error::from_hresult(windows::core::HRESULT(
            0x80004004_u32 as i32,
        )))
    } else {
        Ok(())
    }
}

pub(crate) fn deadline_reason(error: super::windows_enum::UiaDeadlineError) -> &'static str {
    use super::windows_enum::UiaDeadlineError;
    match error {
        UiaDeadlineError::Timeout => {
            "the app's accessibility provider did not respond before the deadline"
        }
        UiaDeadlineError::Busy => "an earlier accessibility provider call is still running",
        UiaDeadlineError::Unavailable => {
            "the accessibility worker could not start or stopped unexpectedly"
        }
    }
}

pub(crate) fn retained_click(
    hwnd: u64,
    retained: super::snapshot::RetainedElement,
    expand_only: bool,
) -> anyhow::Result<Option<ToolResult>> {
    super::windows_enum::run_uia_with_deadline_cancelable(
        "semantic-click", Duration::from_secs(4), "no fallback input",
        move |cancelled| try_click(hwnd, retained.as_ptr(), expand_only, &cancelled),
    ).map_err(|error| anyhow::anyhow!("Click not confirmed because {}. No fallback click was sent; read fresh state before retrying.", deadline_reason(error)))?
}

/// None means no supported semantic pattern. A failed supported action is terminal.
fn try_click(
    hwnd: u64,
    ptr: usize,
    expand_only: bool,
    cancelled: &AtomicBool,
) -> anyhow::Result<Option<ToolResult>> {
    if crate::input::delivery::is_wpf_target_window(hwnd) {
        // WPF peers may call Focus during semantic activation. Preserve the
        // existing background coordinate route instead of entering that handler.
        return Ok(None);
    }
    let element = ManuallyDrop::new(unsafe { IUIAutomationElement::from_raw(ptr as *mut _) });
    let readback = Readback::new(hwnd, Some(ptr));
    if !expand_only {
        if let Some(button) = common_dialog_button(HWND(hwnd as *mut _), &element) {
            let result = super::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                ensure_active(cancelled)?;
                send_command(HWND(hwnd as *mut _), WM_COMMAND, WPARAM(1), LPARAM(button.0 as isize))
            });
            if let Err(error) = result {
                anyhow::bail!("Common-dialog accept command was attempted: {error}; effect not confirmed. No second click was sent. Read fresh state before retrying.");
            }
            return Ok(Some(readback.finish(Some(ptr), "Common-dialog IDOK WM_COMMAND (dialog closure does not prove file contents)", ActionTransport::WindowsPostMessage)));
        }
    }
    let order = pattern_order(titlebar_close(&element), expand_only);
    let dispatch = || -> windows::core::Result<Option<(&str, ActionTransport)>> {
        unsafe {
            for action in order {
                let id = match action {
                    ClickPattern::WindowClose => UIA_WindowPatternId,
                    ClickPattern::Invoke => UIA_InvokePatternId,
                    ClickPattern::Toggle => UIA_TogglePatternId,
                    ClickPattern::Select => UIA_SelectionItemPatternId,
                    ClickPattern::Expand => UIA_ExpandCollapsePatternId,
                };
                let root;
                let source: &IUIAutomationElement = if *action == ClickPattern::WindowClose {
                    root = automation()?.ElementFromHandle(HWND(hwnd as *mut _))?;
                    &root
                } else {
                    &element
                };
                let pattern = source.GetCurrentPattern(id);
                // Non-client Close has a precise WM_CLOSE fallback. Ordinary
                // client mouse messages do not implement a title-bar hit.
                if *action == ClickPattern::WindowClose && pattern.is_err() {
                    ensure_active(cancelled)?;
                    PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0))?;
                    return Ok(Some((
                        "PostMessage WM_CLOSE",
                        ActionTransport::WindowsPostMessage,
                    )));
                }
                let Ok(pattern) = pattern else {
                    continue;
                };
                let (route, transport) = match action {
                    ClickPattern::WindowClose => {
                        let p = pattern.cast::<IUIAutomationWindowPattern>()?;
                        ensure_active(cancelled)?;
                        p.Close()?;
                        ("WindowPattern.Close", ActionTransport::WindowsUiaInvoke)
                    }
                    ClickPattern::Invoke => {
                        let p = pattern.cast::<IUIAutomationInvokePattern>()?;
                        ensure_active(cancelled)?;
                        p.Invoke()?;
                        ("UIA Invoke", ActionTransport::WindowsUiaInvoke)
                    }
                    ClickPattern::Toggle => {
                        let p = pattern.cast::<IUIAutomationTogglePattern>()?;
                        ensure_active(cancelled)?;
                        p.Toggle()?;
                        ("UIA Toggle", ActionTransport::WindowsUiaToggle)
                    }
                    ClickPattern::Select => {
                        let p = pattern.cast::<IUIAutomationSelectionItemPattern>()?;
                        ensure_active(cancelled)?;
                        p.Select()?;
                        (
                            "UIA SelectionItem.Select",
                            ActionTransport::WindowsUiaSelection,
                        )
                    }
                    ClickPattern::Expand => {
                        let p = pattern.cast::<IUIAutomationExpandCollapsePattern>()?;
                        ensure_active(cancelled)?;
                        p.Expand()?;
                        (
                            "UIA ExpandCollapse.Expand",
                            ActionTransport::WindowsUiaExpandCollapse,
                        )
                    }
                };
                return Ok(Some((route, transport)));
            }
            Ok(None)
        }
    };
    let result = super::fg_bypass::run_with_uwp_bypass(hwnd as isize, dispatch)
        .map_err(|error| anyhow::anyhow!("UI Automation action failed: {error}; effect not confirmed. No second click was sent. Read fresh state before retrying."))?;
    Ok(result.map(|(route, transport)| readback.finish(Some(ptr), route, transport)))
}

/// Search inside the addressed window, never ElementFromPoint on the user's
/// desktop: an occluding app must not become the click target.
pub(crate) fn at_point(hwnd: u64, x: i32, y: i32) -> anyhow::Result<Option<ToolResult>> {
    match super::windows_enum::run_uia_with_deadline_cancelable(
        "semantic-point-click", Duration::from_secs(4), "no fallback input",
        move |cancelled| at_point_inner(hwnd, x, y, &cancelled),
    ) {
        Ok(result) => result,
        Err(error) => Ok(Some(ToolResult::error(format!("Click not confirmed because {}. No fallback click was sent; read fresh state before retrying.", deadline_reason(error))))),
    }
}
fn at_point_inner(
    hwnd: u64,
    x: i32,
    y: i32,
    cancelled: &AtomicBool,
) -> anyhow::Result<Option<ToolResult>> {
    let _com = ComApartment::new();
    unsafe {
        let uia = automation()?;
        let root = uia.ElementFromHandle(HWND(hwnd as *mut _))?;
        let all = root.FindAll(TreeScope_Subtree, &uia.CreateTrueCondition()?)?;
        let mut candidates = Vec::new();
        let count = all.Length()?;
        if count > 5000 {
            anyhow::bail!("UIA click tree exceeded 5000 elements; no input was sent");
        }
        for i in 0..count {
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
            match try_click(hwnd, element.as_raw() as usize, false, cancelled) {
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
    fn an_already_missing_window_is_never_a_confirmed_close() {
        let result =
            Readback::new(0, None).finish(None, "Close", ActionTransport::WindowsPostMessage);
        assert_eq!(result.structured_content.unwrap()["verified"], false);
    }
    #[test]
    fn cancellation_prevents_late_mutation() {
        let cancelled = AtomicBool::new(true);
        assert!(ensure_active(&cancelled).is_err());
        cancelled.store(false, Ordering::Release);
        assert!(ensure_active(&cancelled).is_ok());
    }
    #[test]
    fn invalid_window_is_not_a_successful_semantic_click() {
        let result = at_point(0, 0, 0);
        assert!(result.is_err() || result.unwrap().is_some_and(|r| r.is_error == Some(true)));
    }
}
