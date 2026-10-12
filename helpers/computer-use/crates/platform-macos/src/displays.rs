//! Active display enumeration and AX window moves for display placement.
//!
//! Policy parsing, display ordering, and frame math live in
//! `cu_core::display_placement`; this module only supplies the macOS
//! display list and performs the move through the Accessibility API, which
//! works on background windows without activating the app.

use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::display::{CGDirectDisplayID, CGDisplay, CGDisplayBounds, CGMainDisplayID};
use cu_core::display_placement::{DisplayInfo, Rect};
use serde_json::Value;

use crate::ax::bindings::{
    ax_get_window_id, copy_ax_windows, element_screen_rect, kAXErrorSuccess, kAXValueCGPointType,
    kAXValueCGSizeType, AXUIElementCreateApplication, AXUIElementRef, AXUIElementSetAttributeValue,
    AXValueRef,
};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGDisplayMirrorsDisplay(display: CGDirectDisplayID) -> CGDirectDisplayID;
}

#[link(name = "ColorSync", kind = "framework")]
extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: CGDirectDisplayID) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFUUIDCreateString(alloc: CFTypeRef, uuid: CFTypeRef) -> CFStringRef;
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXValueCreate(value_type: i32, value_ptr: *const std::ffi::c_void) -> AXValueRef;
}

/// Active displays that are not mirrors of another display. Bounds are global
/// points with a top-left origin, the same space as AX and CGWindow frames.
pub fn active_displays() -> Vec<DisplayInfo> {
    let ids = CGDisplay::active_displays().unwrap_or_default();
    // SAFETY: thread-safe CoreGraphics query.
    let main = unsafe { CGMainDisplayID() };
    ids.into_iter()
        .filter(|id| unsafe { CGDisplayMirrorsDisplay(*id) } == 0)
        .map(|id| {
            let b = unsafe { CGDisplayBounds(id) };
            let bounds = Rect {
                x: b.origin.x,
                y: b.origin.y,
                width: b.size.width,
                height: b.size.height,
            };
            // CoreGraphics does not report the menu bar or Dock; WindowServer
            // keeps titled windows below the menu bar when AX moves them.
            DisplayInfo {
                id,
                uuid: display_uuid(id),
                bounds,
                usable: bounds,
                is_main: id == main,
            }
        })
        .collect()
}

fn display_uuid(id: CGDirectDisplayID) -> Option<String> {
    unsafe {
        let uuid = CGDisplayCreateUUIDFromDisplayID(id);
        if uuid.is_null() {
            return None;
        }
        let s = CFUUIDCreateString(std::ptr::null(), uuid);
        CFRelease(uuid);
        if s.is_null() {
            return None;
        }
        Some(CFString::wrap_under_create_rule(s).to_string())
    }
}

/// How long placement waits for the first window the launch created. Only
/// spent when the policy resolves to a display, and returns as soon as one
/// appears.
pub const PLACEMENT_WINDOW_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Move windows the launch created onto the policy's display. Returns the
/// `placement` report, or `None` when the policy resolves to no display (single
/// display, `none`) or no new window appeared. Replaces `windows` with the
/// pid's current windows so the response matches the new geometry.
pub fn place_new_windows(
    pid: i32,
    windows: &mut Vec<crate::windows::WindowInfo>,
    preexisting: &std::collections::HashSet<u32>,
    policy: &cu_core::display_placement::DisplayPolicy,
    wait: std::time::Duration,
) -> Option<Value> {
    use cu_core::display_placement::{display_containing, place_rect};
    let displays = crate::displays::active_displays();
    let target = policy.resolve(&displays)?;
    let deadline = std::time::Instant::now() + wait;
    let fresh: Vec<usize> = loop {
        let fresh: Vec<usize> = (0..windows.len())
            .filter(|&i| !preexisting.contains(&windows[i].window_id))
            .collect();
        if !fresh.is_empty() {
            break fresh;
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        *windows = current_windows_for_pid(pid);
    };
    let mut moved = Vec::new();
    let mut failed = Vec::new();
    for i in fresh {
        let w = &mut windows[i];
        let Some(frame) = window_frame(pid, w.window_id) else {
            failed.push(serde_json::json!({ "window_id": w.window_id, "error": "no AX window" }));
            continue;
        };
        let source = display_containing(&frame, &displays);
        if source.map(|d| d.id) == Some(target.id) {
            continue;
        }
        let next = place_rect(&frame, source, target);
        let resize = next.width < frame.width || next.height < frame.height;
        match set_window_frame(pid, w.window_id, &next, resize) {
            Ok(()) => {
                // CGWindowList lags AX by a few hundred ms; AX reads back
                // the applied frame synchronously.
                if let Some(f) = window_frame(pid, w.window_id) {
                    w.bounds.x = f.x;
                    w.bounds.y = f.y;
                    w.bounds.width = f.width;
                    w.bounds.height = f.height;
                }
                moved.push(w.window_id);
            }
            Err(e) => failed.push(serde_json::json!({
                "window_id": w.window_id, "error": e.to_string(),
            })),
        }
    }
    Some(serde_json::json!({
        "display_id": target.id,
        "display_uuid": target.uuid,
        "moved": moved,
        "failed": failed,
    }))
}

/// Layer-0 windows of `pid` with a real size.
pub fn current_windows_for_pid(pid: i32) -> Vec<crate::windows::WindowInfo> {
    crate::windows::all_windows()
        .into_iter()
        .filter(|w| w.pid == pid && w.layer == 0)
        .filter(|w| w.bounds.width > 1.0 && w.bounds.height > 1.0)
        .collect()
}

/// Current AX frame of `window_id` owned by `pid`.
pub fn window_frame(pid: i32, window_id: u32) -> Option<Rect> {
    with_ax_window(pid, window_id, |w| unsafe {
        element_screen_rect(w).map(|[x, y, width, height]| Rect {
            x,
            y,
            width,
            height,
        })
    })
    .flatten()
}

/// Move and (when needed) resize a window through AX. Position is written
/// before and after the size so a window crossing displays with different
/// sizes is not clamped against its old display.
pub fn set_window_frame(
    pid: i32,
    window_id: u32,
    frame: &Rect,
    resize: bool,
) -> anyhow::Result<()> {
    let result = with_ax_window(pid, window_id, |w| unsafe {
        let mut ok = set_point(w, frame.x, frame.y);
        if resize {
            ok &= set_size(w, frame.width, frame.height);
            ok &= set_point(w, frame.x, frame.y);
        }
        ok
    });
    match result {
        Some(true) => Ok(()),
        Some(false) => anyhow::bail!("AX rejected the frame change for window {window_id}"),
        None => anyhow::bail!("window {window_id} is not in pid {pid}'s AXWindows"),
    }
}

fn with_ax_window<T>(pid: i32, window_id: u32, f: impl FnOnce(AXUIElementRef) -> T) -> Option<T> {
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let windows = copy_ax_windows(app);
        CFRelease(app as CFTypeRef);
        let mut out = None;
        let mut f = Some(f);
        for w in &windows {
            if out.is_none() && ax_get_window_id(*w) == Some(window_id) {
                out = f.take().map(|f| f(*w));
            }
        }
        for w in windows {
            CFRelease(w as CFTypeRef);
        }
        out
    }
}

unsafe fn set_point(element: AXUIElementRef, x: f64, y: f64) -> bool {
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    let p = CGPoint { x, y };
    set_ax_value(
        element,
        "AXPosition",
        kAXValueCGPointType,
        &p as *const _ as *const _,
    )
}

unsafe fn set_size(element: AXUIElementRef, w: f64, h: f64) -> bool {
    #[repr(C)]
    struct CGSize {
        w: f64,
        h: f64,
    }
    let s = CGSize { w, h };
    set_ax_value(
        element,
        "AXSize",
        kAXValueCGSizeType,
        &s as *const _ as *const _,
    )
}

unsafe fn set_ax_value(
    element: AXUIElementRef,
    attr: &str,
    ty: i32,
    ptr: *const std::ffi::c_void,
) -> bool {
    let value = AXValueCreate(ty, ptr);
    if value.is_null() {
        return false;
    }
    let attr = CFString::new(attr);
    let err = AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), value as CFTypeRef);
    CFRelease(value as CFTypeRef);
    err == kAXErrorSuccess
}
