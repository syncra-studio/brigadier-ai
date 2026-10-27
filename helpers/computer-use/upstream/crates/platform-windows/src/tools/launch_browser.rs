//! Verify the visible web document, rather than returning a browser startup window.
use windows::core::Interface;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationLegacyIAccessiblePattern,
    TreeScope_Subtree, UIA_ControlTypePropertyId, UIA_DocumentControlTypeId,
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

pub(super) unsafe fn matching_document(
    hwnd: u64,
    requested: &str,
    exact: bool,
) -> anyhow::Result<(IUIAutomation, IUIAutomationElement)> {
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
    let root = automation.ElementFromHandle(HWND(hwnd as *mut _))?;
    let condition = automation.CreatePropertyCondition(
        UIA_ControlTypePropertyId,
        &UIA_DocumentControlTypeId.0.into(),
    )?;
    let documents = root.FindAll(TreeScope_Subtree, &condition)?;
    let mut matching = None;
    for index in 0..documents.Length()? {
        let document = documents.GetElement(index)?;
        if document
            .CurrentIsOffscreen()
            .map(|off| off.as_bool())
            .unwrap_or(true)
        {
            continue;
        }
        if document_url(&document)
            .is_some_and(|url| super::browser_target::url_matches(&url, requested, exact))
        {
            if matching.is_some() {
                anyhow::bail!("More than one visible document matches {requested:?}; use a more specific URL. No other page was read.");
            }
            matching = Some(document);
        }
    }
    matching.map(|doc| (automation, doc)).ok_or_else(|| anyhow::anyhow!(
        "Windows UI Automation does not expose a visible tab matching {requested:?} in this window. The tab may be inactive, absent, or its URL unavailable. No other page was read. Select that tab in the named window and retry."
    ))
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
