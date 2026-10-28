//! Verify the visible web document, rather than returning a browser startup window.
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationLegacyIAccessiblePattern,
    TreeScope_Subtree, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_LegacyIAccessiblePatternId, UIA_ValueValuePropertyId,
};

pub(super) struct Apartment(bool);
impl Apartment {
    pub(super) fn new() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub(super) unsafe fn document_url(document: &IUIAutomationElement) -> Option<String> {
    let value = document
        .GetCurrentPropertyValue(UIA_ValueValuePropertyId)
        .ok();
    let value = value
        .and_then(|value| windows::core::BSTR::try_from(&value).ok())
        .map(|s| s.to_string());
    if value
        .as_deref()
        .is_some_and(super::browser_target::is_web_url)
    {
        return value;
    }
    let pattern: IUIAutomationLegacyIAccessiblePattern = document
        .GetCurrentPattern(UIA_LegacyIAccessiblePatternId)
        .ok()?
        .cast()
        .ok()?;
    let value = pattern.CurrentValue().ok()?.to_string();
    super::browser_target::is_web_url(&value).then_some(value)
}

/// Only browser chrome can supply fallback URL evidence. A web page's own
/// Edit controls must never impersonate an omnibox.
unsafe fn inside_document(automation: &IUIAutomation, element: &IUIAutomationElement, root: &IUIAutomationElement) -> bool {
    let Ok(walker) = automation.ControlViewWalker() else { return true; };
    let mut current = element.clone();
    for _ in 0..128 {
        let Ok(parent) = walker.GetParentElement(&current) else { return true; };
        if automation.CompareElements(&parent, root).map(|same| same.as_bool()).unwrap_or(false) {
            return false;
        }
        if parent.CurrentControlType().ok() == Some(UIA_DocumentControlTypeId) { return true; }
        current = parent;
    }
    true
}

unsafe fn address_bar_url(automation: &IUIAutomation, root: &IUIAutomationElement) -> Option<String> {
    let condition = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &UIA_EditControlTypeId.0.into()).ok()?;
    let edits = root.FindAll(TreeScope_Subtree, &condition).ok()?;
    let mut result = None;
    for index in 0..edits.Length().ok()? {
        let edit = edits.GetElement(index).ok()?;
        if edit.CurrentIsOffscreen().map(|off| off.as_bool()).unwrap_or(true) || inside_document(automation, &edit, root) { continue; }
        let id = edit.CurrentAutomationId().ok().map(|s| s.to_string()).unwrap_or_default();
        let class = edit.CurrentClassName().ok().map(|s| s.to_string()).unwrap_or_default();
        if !super::browser_target::is_omnibox(&id, &class) { continue; }
        // Edge often exposes the address on its Edit, not its Document. The
        // value may omit https://, so document_url's scheme filter cannot apply.
        let value = edit.GetCurrentPropertyValue(UIA_ValueValuePropertyId).ok()
            .and_then(|v| windows::core::BSTR::try_from(&v).ok()).map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                let pattern: IUIAutomationLegacyIAccessiblePattern = edit.GetCurrentPattern(UIA_LegacyIAccessiblePatternId).ok()?.cast().ok()?;
                pattern.CurrentValue().ok().map(|s| s.to_string())
            })?;
        if result.replace(value).is_some() { return None; }
    }
    result
}

pub(super) unsafe fn matching_document(
    hwnd: u64,
    requested: &str,
    exact: bool,
) -> anyhow::Result<(IUIAutomation, IUIAutomationElement, String)> {
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
    let root = automation.ElementFromHandle(HWND(hwnd as *mut _))?;
    let condition = automation.CreatePropertyCondition(UIA_ControlTypePropertyId, &UIA_DocumentControlTypeId.0.into())?;
    let documents = root.FindAll(TreeScope_Subtree, &condition)?;
    let mut visible = Vec::new();
    let mut matching = None;
    for index in 0..documents.Length()? {
        let document = documents.GetElement(index)?;
        if document.CurrentIsOffscreen().map(|off| off.as_bool()).unwrap_or(true) || inside_document(&automation, &document, &root) { continue; }
        let url = document_url(&document);
        if url.as_deref().is_some_and(|url| super::browser_target::url_matches(url, requested, exact)) {
            if matching.is_some() {
                anyhow::bail!("More than one visible document matches {requested:?}; use a more specific URL. No other page was read.");
            }
            matching = Some((document.clone(), url.clone().unwrap_or_default()));
        }
        visible.push((document, url));
    }
    if let Some((doc, url)) = matching { return Ok((automation, doc, url)); }
    let browser = crate::win32::window_owner_pid(hwnd).is_some_and(|pid| {
        crate::win32::list_processes().iter().any(|p| p.pid == pid &&
            super::browser_target::named_browser_executable(&p.name).is_some())
    });
    if browser {
        if let Some(bar) = address_bar_url(&automation, &root) {
            if let Some((document, url)) = visible.first() {
                if super::browser_target::omnibox_binds_document(visible.len(), url.as_deref(), &bar, requested, exact) {
                    return Ok((automation, document.clone(), bar));
                }
            }
        }
    }
    anyhow::bail!("Windows UI Automation does not expose a uniquely visible document matching {requested:?} in window {hwnd}. Its document URL and address bar could not confirm the target. No other page was read. Select that tab in the named window and retry.")
}

pub(super) async fn window_showing_url(
    executable: &str,
    url: &str,
    timeout: std::time::Duration,
) -> anyhow::Result<Option<crate::win32::WindowInfo>> {
    let executable = super::launch_policy::executable_name(executable);
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        crate::uia::windows_enum::run_uia_with_deadline_cancelable(
            "browser URL verification",
            timeout,
            "no unverified browser window",
            move |cancelled| {
                let _com = Apartment::new();
                for process in crate::win32::list_processes() {
                    if super::launch_policy::executable_name(&process.name) != executable {
                        continue;
                    }
                    for window in crate::win32::list_windows_win32_first(process.pid) {
                        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                            return None;
                        }
                        if unsafe { matching_document(window.hwnd, &url, true) }.is_ok() {
                            return Some(window);
                        }
                    }
                }
                None
            },
        )
        .map_err(|error| {
            anyhow::anyhow!(
                "Browser URL verification stopped because {}",
                crate::uia::click::deadline_reason(error)
            )
        })
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_window_is_not_a_verified_browser_page() {
        let _com = Apartment::new();
        assert!(unsafe { matching_document(0, "example.com", false) }.is_err());
    }
}
