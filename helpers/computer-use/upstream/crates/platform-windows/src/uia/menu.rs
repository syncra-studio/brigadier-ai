//! WinUI menu bars and detached MenuFlyout popups, scoped to one HWND.
use windows::core::Interface;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
};

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct PopupScan {
    owner: HWND,
    pid: u32,
    windows: Vec<HWND>,
}
unsafe extern "system" fn collect_popup(hwnd: HWND, data: LPARAM) -> BOOL {
    let scan = &mut *(data.0 as *mut PopupScan);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid != scan.pid || !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut cursor = hwnd;
    for _ in 0..16 {
        let Ok(owner) = GetWindow(cursor, GW_OWNER) else {
            break;
        };
        if owner == scan.owner {
            scan.windows.push(hwnd);
            break;
        }
        if owner.0.is_null() {
            break;
        }
        cursor = owner;
    }
    BOOL(1)
}
fn popups(hwnd: HWND) -> Vec<HWND> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    let mut scan = PopupScan {
        owner: hwnd,
        pid,
        windows: Vec::new(),
    };
    let _ = unsafe { EnumWindows(Some(collect_popup), LPARAM(&mut scan as *mut _ as isize)) };
    scan.windows
}

fn menu_ancestry(uia: &IUIAutomation, element: &IUIAutomationElement, top: bool) -> (bool, bool) {
    unsafe {
        let Ok(walker) = uia.ControlViewWalker() else {
            return (false, false);
        };
        let mut cursor = element.clone();
        let mut in_menu = false;
        for _ in 0..32 {
            let Ok(parent) = walker.GetParentElement(&cursor) else {
                break;
            };
            let kind = parent.CurrentControlType().ok();
            if kind == Some(UIA_DocumentControlTypeId) {
                return (false, true);
            }
            if kind == Some(UIA_MenuBarControlTypeId)
                || (!top && kind == Some(UIA_MenuControlTypeId))
            {
                in_menu = true;
            }
            if kind == Some(UIA_WindowControlTypeId) {
                break;
            }
            cursor = parent;
        }
        (in_menu, false)
    }
}

fn find(
    uia: &IUIAutomation,
    scope: &IUIAutomationElement,
    wanted: &str,
    top: bool,
    popup: bool,
) -> Result<Vec<IUIAutomationElement>, String> {
    unsafe {
        let condition = uia.CreateTrueCondition().map_err(|e| e.to_string())?;
        let all = scope
            .FindAll(TreeScope_Subtree, &condition)
            .map_err(|e| e.to_string())?;
        let count = all.Length().map_err(|e| e.to_string())?;
        if count > 5000 {
            return Err("menu tree exceeded 5000 elements; narrow the target window".into());
        }
        let mut matches = Vec::new();
        for i in 0..count {
            let element = all.GetElement(i).map_err(|e| e.to_string())?;
            if element
                .CurrentIsOffscreen()
                .map(|s| s.as_bool())
                .unwrap_or(true)
            {
                continue;
            }
            let kind = element.CurrentControlType().ok();
            let control = if kind == Some(UIA_MenuItemControlTypeId) {
                "MenuItem"
            } else if kind == Some(UIA_ButtonControlTypeId) {
                "Button"
            } else {
                continue;
            };
            let name = element
                .CurrentName()
                .map(|s| s.to_string())
                .unwrap_or_default();
            let (in_menu, in_document) = menu_ancestry(uia, &element, top);
            // An owned popup or a previously expanded item provides the menu scope
            // even when XAML omits Menu/MenuBar ancestry in its fragment tree.
            if crate::menu_path::candidate(control, &name, wanted, in_menu || popup, in_document) {
                matches.push(element);
            }
        }
        Ok(matches)
    }
}

pub(super) fn invoke(hwnd: u64, path: &[String]) -> Result<(), String> {
    if crate::input::delivery::is_wpf_target_window(hwnd) {
        return Err("Menu invocation refused: this app's WPF automation handlers may activate its window. Use a background keyboard shortcut or ask the user to choose the menu item.".into());
    }
    let path = path.to_vec();
    super::windows_enum::run_uia_with_deadline_cancelable(
        super::windows_enum::UiaTarget::Window(hwnd),
        "menu-path", std::time::Duration::from_secs(4), "no fallback input",
        move |cancelled| invoke_inner(hwnd, &path, &cancelled),
    ).map_err(|error| format!("Menu action not confirmed because {}. No fallback input was sent; read fresh state before retrying.", super::click::deadline_reason(error)))?
}
fn invoke_inner(
    hwnd: u64,
    path: &[String],
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<(), String> {
    if path.is_empty() || path.len() > 16 {
        return Err("menu path must contain 1 to 16 segments".into());
    }
    let _com = Apartment(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
    let hwnd = HWND(hwnd as *mut _);
    let uia: IUIAutomation =
        unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| e.to_string())?;
    let root = unsafe { uia.ElementFromHandle(hwnd) }.map_err(|e| e.to_string())?;
    let prior_popups = popups(hwnd);
    let _no_activate = crate::input::NoActivateGuard::arm(hwnd);
    let mut expanded: Vec<IUIAutomationElement> = Vec::new();
    let mut opened: Vec<IUIAutomationExpandCollapsePattern> = Vec::new();
    let result = (|| {
        for (depth, wanted) in path.iter().enumerate() {
            let mut found = Vec::new();
            for attempt in 0..9 {
                // Prefer the current expanded item's own descendants. WinUI may
                // instead attach its popup to the window or an owned popup HWND.
                if let Some(parent) = expanded.last() {
                    found = find(&uia, parent, wanted, false, true)?;
                }
                if found.is_empty() {
                    found = find(&uia, &root, wanted, depth == 0, false)?;
                }
                if found.is_empty() && depth > 0 {
                    for popup in popups(hwnd)
                        .into_iter()
                        .filter(|p| !prior_popups.contains(p))
                    {
                        let scope =
                            unsafe { uia.ElementFromHandle(popup) }.map_err(|e| e.to_string())?;
                        found.extend(find(&uia, &scope, wanted, false, true)?);
                    }
                }
                if !found.is_empty() || attempt == 8 || depth == 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let target = match found.as_slice() {
                [element] => element,
                [] => return Err(format!("menu path segment {} ({wanted}) was not found in the addressed window or its opened popup", depth + 1)),
                _ => return Err(format!("menu path segment {} ({wanted}) is ambiguous", depth + 1)),
            };
            if !unsafe { target.CurrentIsEnabled() }
                .map_err(|e| e.to_string())?
                .as_bool()
            {
                return Err(format!(
                    "menu path segment {} ({wanted}) is disabled",
                    depth + 1
                ));
            }
            if depth + 1 < path.len() {
                let pattern: IUIAutomationExpandCollapsePattern =
                    unsafe { target.GetCurrentPattern(UIA_ExpandCollapsePatternId) }
                        .and_then(|p| p.cast())
                        .map_err(|e| {
                            format!("{wanted} has no usable ExpandCollapse pattern: {e}")
                        })?;
                let already_open = unsafe { pattern.CurrentExpandCollapseState() }.ok()
                    == Some(ExpandCollapseState_Expanded);
                if !already_open {
                    // Remember before dispatch: even a provider returning an error
                    // may have opened its popup and needs cleanup.
                    opened.push(pattern.clone());
                    super::fg_bypass::run_with_uwp_bypass(hwnd.0 as isize, || unsafe {
                        super::click::ensure_active(cancelled)?;
                        pattern.Expand()
                    })
                    .map_err(|e| format!("could not expand {wanted}: {e}"))?;
                }
                expanded.push(target.clone());
            } else {
                super::fg_bypass::run_with_uwp_bypass(hwnd.0 as isize, || unsafe {
                    if let Ok(p) = target.GetCurrentPattern(UIA_InvokePatternId) {
                        {
                            let p = p.cast::<IUIAutomationInvokePattern>()?;
                            super::click::ensure_active(cancelled)?;
                            p.Invoke()
                        }
                    } else {
                        {
                            let p = target
                                .GetCurrentPattern(UIA_SelectionItemPatternId)?
                                .cast::<IUIAutomationSelectionItemPattern>()?;
                            super::click::ensure_active(cancelled)?;
                            p.Select()
                        }
                    }
                })
                .map_err(|e| {
                    format!("menu action for {wanted} failed: {e}; its effect is not confirmed")
                })?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        let mut cleanup_failed = false;
        for pattern in opened.iter().rev() {
            let collapsed = super::fg_bypass::run_with_uwp_bypass(hwnd.0 as isize, || unsafe {
                super::click::ensure_active(cancelled)?;
                pattern.Collapse()
            });
            cleanup_failed |= collapsed.is_err()
                || unsafe { pattern.CurrentExpandCollapseState() }.ok()
                    != Some(ExpandCollapseState_Collapsed);
        }
        return Err(format!(
            "{error}. {}",
            if cleanup_failed {
                "Could not confirm all menus opened by this call collapsed; read fresh state."
            } else {
                if opened.is_empty() {
                    "No menu was opened by this call."
                } else {
                    "Menus opened by this call are collapsed."
                }
            }
        ));
    }
    Ok(())
}

/// Search only advertised menu containers, never invoke a menu header to probe it.
/// Runs inside the accelerator worker's existing COM apartment/deadline.
pub(super) fn search_accelerator(
    uia: &IUIAutomation,
    root: &IUIAutomationElement,
    hwnd: isize,
    cancelled: &std::sync::atomic::AtomicBool,
    mut scan: impl FnMut(&IUIAutomationElement) -> anyhow::Result<bool>,
) -> anyhow::Result<bool> {
    super::click::ensure_active(cancelled)?;
    let hwnd = HWND(hwnd as *mut _);
    let _no_activate = crate::input::NoActivateGuard::arm(hwnd);
    let mut visited = Vec::new();
    search_menu_level(
        uia,
        root,
        hwnd,
        cancelled,
        &mut scan,
        &mut visited,
        0,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn search_menu_level(
    uia: &IUIAutomation,
    scope: &IUIAutomationElement,
    hwnd: HWND,
    cancelled: &std::sync::atomic::AtomicBool,
    scan: &mut impl FnMut(&IUIAutomationElement) -> anyhow::Result<bool>,
    visited: &mut Vec<IUIAutomationElement>,
    depth: usize,
    menu_scope: bool,
) -> anyhow::Result<bool> {
    anyhow::ensure!(
        depth < 16 && visited.len() < 32,
        "accelerator menu search limit reached"
    );
    super::click::ensure_active(cancelled)?;
    let condition = unsafe { uia.CreateTrueCondition() }?;
    let all = unsafe { scope.FindAll(TreeScope_Subtree, &condition) }?;
    let count = unsafe { all.Length() }?;
    anyhow::ensure!(
        count <= 5000,
        "accelerator menu tree exceeded 5000 elements"
    );
    for i in 0..count {
        super::click::ensure_active(cancelled)?;
        let element = unsafe { all.GetElement(i) }?;
        let kind = unsafe { element.CurrentControlType() }.ok();
        if (kind != Some(UIA_MenuItemControlTypeId) && kind != Some(UIA_ButtonControlTypeId))
            || unsafe { element.CurrentIsOffscreen() }
                .map(|v| v.as_bool())
                .unwrap_or(true)
            || !unsafe { element.CurrentIsEnabled() }
                .map(|v| v.as_bool())
                .unwrap_or(false)
        {
            continue;
        }
        let (in_menu, in_document) = menu_ancestry(uia, &element, depth == 0);
        if in_document || !(in_menu || menu_scope) {
            continue;
        }
        // The window fallback finds attached flyouts, but must not switch to
        // another menu-bar header while this branch is still expanded.
        if depth > 0 && !menu_scope && menu_ancestry(uia, &element, true).0 {
            continue;
        }
        if visited.iter().any(|prior| unsafe {
            uia.CompareElements(prior, &element)
                .map(|v| v.as_bool())
                .unwrap_or(false)
        }) {
            continue;
        }
        let Ok(pattern) = (unsafe { element.GetCurrentPattern(UIA_ExpandCollapsePatternId) })
            .and_then(|p| p.cast::<IUIAutomationExpandCollapsePattern>())
        else {
            continue;
        };
        let state = unsafe { pattern.CurrentExpandCollapseState() }?;
        if state == ExpandCollapseState_LeafNode {
            continue;
        }
        anyhow::ensure!(visited.len() < 32, "accelerator menu search limit reached");
        visited.push(element.clone());
        let prior_popups = popups(hwnd);
        let saw_popup = std::cell::Cell::new(false);
        let opened = state != ExpandCollapseState_Expanded;
        // Remember the pattern before Expand: a failed provider call may still open it.
        let result = (|| -> anyhow::Result<bool> {
            if opened {
                super::fg_bypass::run_with_uwp_bypass(hwnd.0 as isize, || unsafe {
                    super::click::ensure_active(cancelled)?;
                    pattern.Expand()
                })?;
            }
            // WinUI attaches MenuFlyout children asynchronously, sometimes to an
            // owned popup HWND rather than the expanded element's fragment.
            for attempt in 0..4 {
                super::click::ensure_active(cancelled)?;
                let mut scopes = vec![element.clone(), scope.clone()];
                for popup in popups(hwnd)
                    .into_iter()
                    .filter(|p| !prior_popups.contains(p))
                {
                    saw_popup.set(true);
                    scopes.push(unsafe { uia.ElementFromHandle(popup) }?);
                }
                for child in &scopes {
                    if scan(child)? {
                        return Ok(true);
                    }
                }
                if attempt == 3 {
                    for (index, child) in scopes.iter().enumerate() {
                        if search_menu_level(
                            uia,
                            child,
                            hwnd,
                            cancelled,
                            scan,
                            visited,
                            depth + 1,
                            index != 1 || menu_scope,
                        )? {
                            return Ok(true);
                        }
                    }
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
            Ok(false)
        })();
        // On success too: only collapse menus this call opened. Never send
        // cleanup after timeout/user cancellation, just like invoke_menu.
        if opened {
            let cleanup = super::fg_bypass::run_with_uwp_bypass(hwnd.0 as isize, || unsafe {
                super::click::ensure_active(cancelled)?;
                if pattern.CurrentExpandCollapseState()? != ExpandCollapseState_Collapsed {
                    pattern.Collapse()?;
                }
                Ok::<(), windows::core::Error>(())
            });
            // A matched shortcut that closed its own window (Ctrl+Shift+W)
            // takes the menu with it: there is nothing left to collapse.
            if matches!(result, Ok(true))
                && !unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindow(hwnd) }.as_bool()
            {
                return Ok(true);
            }
            // A matched command closes its menu flyout itself, and a dialog it
            // opens (Notepad's "Save changes?") can make the menu header's
            // provider fail: the flyout's own popup window closing confirms it.
            let unconfirmed = cleanup.is_err()
                || unsafe { pattern.CurrentExpandCollapseState() }.ok()
                    != Some(ExpandCollapseState_Collapsed);
            if matches!(result, Ok(true)) && unconfirmed && saw_popup.get() {
                for _ in 0..8 {
                    if popups(hwnd).iter().all(|p| prior_popups.contains(p)) {
                        return Ok(true);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
            }
            if let Err(error) = cleanup {
                if matches!(result, Ok(true)) {
                    anyhow::bail!("the menu command for this shortcut was invoked, but closing its menu could not be confirmed ({error}); read fresh state before retrying");
                }
                anyhow::bail!("accelerator menu search result {result:?}; could not confirm menu cleanup: {error}; read fresh state before retrying");
            }
            anyhow::ensure!(
                unsafe { pattern.CurrentExpandCollapseState() }.ok()
                    == Some(ExpandCollapseState_Collapsed),
                "{}could not confirm the menu it opened collapsed; read fresh state before retrying",
                if matches!(result, Ok(true)) { "the menu command for this shortcut was invoked, but " } else { "" }
            );
        }
        if result? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn cancelled_menu_search_never_scans_or_expands() {
        use std::sync::atomic::AtomicBool;
        use windows::Win32::System::Com::*;
        let _com = super::Apartment(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok());
        let uia: super::IUIAutomation =
            unsafe { CoCreateInstance(&super::CUIAutomation, None, CLSCTX_INPROC_SERVER) }.unwrap();
        let root = unsafe { uia.GetRootElement() }.unwrap();
        // Enter the worker directly: cancellation must be checked before even
        // querying a provider, much less opening a menu or invoking a match.
        let result = super::search_menu_level(
            &uia,
            &root,
            super::HWND::default(),
            &AtomicBool::new(true),
            &mut |_| panic!("cancelled search attempted accelerator scan"),
            &mut Vec::new(),
            0,
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn an_invalid_window_cannot_invoke_a_menu() {
        assert!(super::invoke(0, &["File".into(), "Save as".into()]).is_err());
        assert!(super::invoke(0, &[]).is_err());
    }
}
