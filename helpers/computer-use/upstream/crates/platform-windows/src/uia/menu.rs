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

#[cfg(test)]
mod tests {
    #[test]
    fn an_invalid_window_cannot_invoke_a_menu() {
        assert!(super::invoke(0, &["File".into(), "Save as".into()]).is_err());
        assert!(super::invoke(0, &[]).is_err());
    }
}
