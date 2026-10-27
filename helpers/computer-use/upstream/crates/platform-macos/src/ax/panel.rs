//! Exact AX resolution for out-of-process AppKit Open/Save panels.
use super::bindings::*;
use core_foundation::base::{CFRelease, CFTypeRef};
use std::time::{Duration, Instant};

fn panel_identifier(identifier: Option<&str>) -> bool {
    matches!(identifier, Some("open-panel" | "save-panel"))
}

fn panel_service_path(path: &str) -> bool {
    path.contains("/com.apple.appkit.xpc.openAndSavePanelService.xpc/")
        || path.ends_with("/OpenAndSavePanelService")
}

pub(crate) unsafe fn is_panel(element: AXUIElementRef) -> bool {
    if panel_identifier(copy_string_attr(element, "AXIdentifier").as_deref()) {
        return true;
    }
    let mut pid = 0;
    AXUIElementGetPid(element, &mut pid) == kAXErrorSuccess
        && crate::apps::path_of_pid(pid).is_some_and(|path| panel_service_path(&path))
}

/// Bounded traversal is shared with the fake XPC-panel graphs in the tests.
fn find_window_in_tree<E>(
    roots: Vec<E>,
    mut matches: impl FnMut(&E) -> bool,
    mut children: impl FnMut(&E) -> Vec<E>,
    mut within_budget: impl FnMut() -> bool,
) -> Option<E> {
    let mut pending: Vec<_> = roots.into_iter().map(|e| (e, 0)).collect();
    let mut visited = 0;
    while let Some((element, depth)) = pending.pop() {
        visited += 1;
        if visited > 256 || !within_budget() {
            return None;
        }
        if matches(&element) {
            return Some(element);
        }
        if depth < 5 {
            pending.extend(children(&element).into_iter().map(|e| (e, depth + 1)));
        }
    }
    None
}

unsafe fn owned(element: AXUIElementRef) -> super::snapshot::RetainedElement {
    let retained = super::snapshot::RetainedElement::retain(element as usize);
    CFRelease(element as CFTypeRef);
    retained
}

/// Follow only bounded AX links and require the exact CGWindowID; title or
/// geometry alone never establishes a relationship to a client's panel.
unsafe fn find_window(pid: i32, wid: u32, deadline: Instant) -> Option<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return None;
    }
    AXUIElementSetMessagingTimeout(app, 0.05);
    let mut roots = copy_ax_windows(app);
    roots.extend(copy_children(app));
    for name in ["AXFocusedWindow", "AXMainWindow"] {
        roots.extend(copy_element_attr(app, name));
    }
    CFRelease(app as CFTypeRef);
    let found = find_window_in_tree(
        roots.into_iter().map(|e| owned(e)).collect(),
        |element| {
            let element = element.as_ptr() as AXUIElementRef;
            AXUIElementSetMessagingTimeout(element, 0.05);
            copy_string_attr(element, "AXRole").as_deref() == Some("AXWindow")
                && ax_get_window_id(element) == Some(wid)
        },
        |element| {
            copy_children(element.as_ptr() as AXUIElementRef)
                .into_iter()
                .map(|e| owned(e))
                .collect()
        },
        || Instant::now() < deadline,
    )?;
    let ptr = found.as_ptr() as AXUIElementRef;
    AXUIElementSetMessagingTimeout(ptr, 0.0);
    core_foundation::base::CFRetain(ptr as CFTypeRef);
    Some(ptr)
}

fn service_pids() -> Vec<i32> {
    extern "C" {
        fn proc_listallpids(buffer: *mut std::ffi::c_void, size: i32) -> i32;
    }
    let count = unsafe { proc_listallpids(std::ptr::null_mut(), 0) };
    if count <= 0 {
        return Vec::new();
    }
    let mut pids = vec![0i32; count as usize + 64];
    let size = (pids.len() * std::mem::size_of::<i32>()) as i32;
    let written = unsafe { proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(written.max(0) as usize);
    pids.into_iter()
        .filter(|&pid| crate::apps::path_of_pid(pid).is_some_and(|p| panel_service_path(&p)))
        .take(16)
        .collect()
}

/// An unlisted window may be a client-owned Open/Save panel whose AX server
/// is the AppKit XPC panel service. Do not confine that search to off-Space
/// windows: these panels are routinely visible on the current Space too.
///
/// # Safety
/// Returns a +1 AXWindow for exactly `wid`; caller must release it.
pub unsafe fn copy_unlisted_window(pid: i32, wid: u32) -> Option<AXUIElementRef> {
    if !matches!(
        crate::windows::resolve_window_owner(pid, wid),
        crate::windows::WindowOwner::SamePid
    ) {
        return None;
    }
    let deadline = Instant::now() + Duration::from_millis(350);
    if let Some(window) = find_window(pid, wid, Instant::now() + Duration::from_millis(50)) {
        return Some(window);
    }
    for service in service_pids() {
        if Instant::now() >= deadline {
            break;
        }
        if let Some(window) = find_window(service, wid, deadline) {
            return Some(window);
        }
    }
    None
}

/// Keyboard events must use the AX panel server, rather than the client's
/// document/menu. Only a positively identified exact Open/Save panel qualifies.
pub fn keyboard_pid(pid: i32, wid: u32) -> Option<i32> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        AXUIElementSetMessagingTimeout(app, 0.1);
        let windows = copy_ax_windows_including(app, pid, wid);
        CFRelease(app as CFTypeRef);
        let mut target = None;
        for window in windows {
            if ax_get_window_id(window) == Some(wid) && is_panel(window) {
                let mut server = 0;
                if AXUIElementGetPid(window, &mut server) == kAXErrorSuccess && server > 0 {
                    target = Some(server);
                }
            }
            CFRelease(window as CFTypeRef);
        }
        target
    }
}

/// Open/Save panels own their key equivalents (notably Cmd-Shift-G). The
/// client app's identically bound Find Previous menu must never intercept them.
pub fn blocks_app_menu(pid: i32, wid: u32) -> bool {
    if keyboard_pid(pid, wid).is_some() {
        return true;
    }
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        AXUIElementSetMessagingTimeout(app, 0.1);
        let focused = copy_element_attr(app, "AXFocusedWindow");
        CFRelease(app as CFTypeRef);
        focused.is_some_and(|window| {
            let panel = is_panel(window);
            CFRelease(window as CFTypeRef);
            panel
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regression_remote_panel_lookup_never_substitutes_a_sibling() {
        // Client roots expose a document and an XPC container whose remote
        // child is the exact panel. A matching title on another window cannot
        // qualify: only the requested globally unique CGWindowID does.
        let graph = [
            ("AXApplication", None, vec![1, 2]),
            ("AXWindow", Some(40), vec![]),
            ("AXGroup", None, vec![3]),
            ("AXWindow", Some(90), vec![]),
        ];
        let lookup = |wid| {
            find_window_in_tree(
                vec![0],
                |&e| graph[e].0 == "AXWindow" && graph[e].1 == Some(wid),
                |&e| graph[e].2.clone(),
                || true,
            )
        };
        assert_eq!(lookup(90), Some(3));
        assert_eq!(lookup(40), Some(1));
        assert_eq!(lookup(99), None);
        assert_eq!(
            find_window_in_tree(vec![0], |_| false, |_| vec![0], || true),
            None
        );
        assert_eq!(
            find_window_in_tree(vec![0], |_| true, |_| vec![], || false),
            None
        );
    }

    #[test]
    fn regression_panel_identity_and_service_are_specific() {
        assert!(panel_identifier(Some("open-panel")));
        assert!(panel_identifier(Some("save-panel")));
        assert!(!panel_identifier(Some("document")));
        assert!(!panel_identifier(None));
        assert!(panel_service_path("/System/Library/Frameworks/AppKit.framework/Versions/C/XPCServices/com.apple.appkit.xpc.openAndSavePanelService.xpc/Contents/MacOS/com.apple.appkit.xpc.openAndSavePanelService"));
        assert!(!panel_service_path(
            "/Applications/Example.app/Contents/MacOS/Example"
        ));
    }
}
