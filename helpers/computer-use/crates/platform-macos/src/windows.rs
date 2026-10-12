//! macOS window enumeration via CGWindowList APIs.
//!
//! Uses the C-level CGWindowListCopyWindowInfo API which returns a CFArray
//! of CFDictionary objects describing each window.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInfo {
    pub window_id: u32,
    pub pid: i32,
    pub app_name: String,
    pub title: String,
    pub bounds: WindowBounds,
    pub layer: i32,
    /// WindowServer compositing alpha. Used to ignore detectably transparent
    /// helper windows during pixel-delivery obstruction checks.
    #[serde(default = "default_window_alpha")]
    pub alpha: f64,
    pub z_index: usize,
    pub is_on_screen: bool,
    pub on_current_space: Option<bool>,
    pub space_ids: Option<Vec<u64>>,
}

fn default_window_alpha() -> f64 {
    1.0
}

// ── CGWindow option flags ─────────────────────────────────────────────────────
// Apple-canonical kCG* naming preserved to match the public Apple headers — the
// upper-case-globals lint would rename them to KCG_..., which would silently
// shadow the Apple-namespaced constant references in any future code that
// re-introduces them. Mirrors platform-windows::uia/windows_enum.rs which uses
// the same allow for UIA_* constants.
#[allow(non_upper_case_globals)]
const kCGWindowListExcludeDesktopElements: u32 = 16;
#[allow(non_upper_case_globals)]
const kCGWindowListOptionOnScreenOnly: u32 = 1;
#[allow(non_upper_case_globals)]
const kCGNullWindowID: u32 = 0;

// ── Internal CGWindowInfo parsing ─────────────────────────────────────────────
//
// We use `system_profiler` workaround via `CGWindowListCopyWindowInfo` which
// returns a plist-like structure. The simplest cross-compile-safe approach
// is to dump via `osascript` or use the Objective-C runtime.
//
// For the initial version we use the `core-foundation` crate + direct C linkage.

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relativeToWindow: u32,
    ) -> core_foundation::array::CFArrayRef;
}

/// Enumerate all windows (including off-screen).
pub fn all_windows() -> Vec<WindowInfo> {
    enumerate_windows(kCGWindowListExcludeDesktopElements)
}

/// Enumerate only on-screen windows.
pub fn visible_windows() -> Vec<WindowInfo> {
    enumerate_windows(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements)
}

/// Select the real frontmost layer-0 window that an otherwise-unpinned cursor
/// overlay should sit above. The caller supplies WindowServer's native
/// front-to-back ordering and the overlay owner's pid.
pub(crate) fn cursor_overlay_anchor_window(windows: &[WindowInfo], driver_pid: i32) -> Option<u32> {
    windows
        .iter()
        .find(|window| {
            window.pid != driver_pid
                && window.layer == 0
                && window.is_on_screen
                && window.alpha > 0.01
                && window.bounds.width > 1.0
                && window.bounds.height > 1.0
                && !is_window_server_owner_name(&window.app_name)
        })
        .map(|window| window.window_id)
}

fn enumerate_windows(options: u32) -> Vec<WindowInfo> {
    enumerate_windows_inner(options, true)
}

/// On-screen windows in WindowServer's native front-to-back order, including
/// non-zero layers. Public window listing deliberately remains layer-0-only.
fn hit_test_windows() -> Vec<WindowInfo> {
    enumerate_windows_inner(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        false,
    )
}

fn enumerate_windows_inner(options: u32, layer_zero_only: bool) -> Vec<WindowInfo> {
    use core_foundation::{
        array::CFArray,
        base::{CFGetTypeID, CFTypeRef, TCFType},
        boolean::CFBoolean,
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use std::os::raw::c_void;

    let raw_ref = unsafe { CGWindowListCopyWindowInfo(options, kCGNullWindowID) };
    if raw_ref.is_null() {
        return vec![];
    }

    let raw: CFArray<CFTypeRef> = unsafe { CFArray::wrap_under_create_rule(raw_ref as _) };
    let total = raw.len() as usize;
    let mut result = Vec::new();

    for (idx, item) in raw.iter().enumerate() {
        let item = *item;
        // Each item should be a CFDictionary.
        let dict_type = CFDictionary::<*const c_void, *const c_void>::type_id();
        if unsafe { CFGetTypeID(item) } != dict_type {
            continue;
        }

        let dict: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(item as _) };

        // Helper: get number from dict by key string.
        let get_num = |key: &str| -> i64 {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFNumber::type_id() {
                        CFNumber::wrap_under_get_rule(v as _).to_i64()
                    } else {
                        None
                    }
                })
                .unwrap_or(0)
        };

        let get_str = |key: &str| -> String {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFString::type_id() {
                        Some(CFString::wrap_under_get_rule(v as _).to_string())
                    } else {
                        None
                    }
                })
                .unwrap_or_default()
        };

        let get_bool = |key: &str| -> bool {
            let k = CFString::new(key);
            dict.find(k.as_concrete_TypeRef() as *const c_void)
                .map(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFBoolean::type_id() {
                        bool::from(CFBoolean::wrap_under_get_rule(v as _))
                    } else {
                        false
                    }
                })
                .unwrap_or(false)
        };

        let window_id = get_num("kCGWindowNumber") as u32;
        let pid = get_num("kCGWindowOwnerPID") as i32;
        let app_name = get_str("kCGWindowOwnerName");
        let title = get_str("kCGWindowName");
        let layer = get_num("kCGWindowLayer") as i32;
        let alpha = get_number(&dict, "kCGWindowAlpha");
        let is_on_screen = get_bool("kCGWindowIsOnscreen");

        // Only include layer-0 windows.
        if layer_zero_only && layer != 0 {
            continue;
        }

        // Parse bounds dict.
        let bounds = {
            let bk = CFString::new("kCGWindowBounds");
            dict.find(bk.as_concrete_TypeRef() as *const c_void)
                .and_then(|v| unsafe {
                    let v = *v;
                    if CFGetTypeID(v) == CFDictionary::<*const c_void, *const c_void>::type_id() {
                        let bd: CFDictionary<*const c_void, *const c_void> =
                            CFDictionary::wrap_under_get_rule(v as _);
                        let x = get_bounds_num(&bd, "X");
                        let y = get_bounds_num(&bd, "Y");
                        let w = get_bounds_num(&bd, "Width");
                        let h = get_bounds_num(&bd, "Height");
                        Some(WindowBounds {
                            x,
                            y,
                            width: w,
                            height: h,
                        })
                    } else {
                        None
                    }
                })
                .unwrap_or(WindowBounds {
                    x: 0.,
                    y: 0.,
                    width: 0.,
                    height: 0.,
                })
        };

        // z_index: CGWindowList front-to-back → assign reverse index.
        let z_index = total - idx;

        result.push(WindowInfo {
            window_id,
            pid,
            app_name,
            title,
            bounds,
            layer,
            alpha,
            z_index,
            is_on_screen,
            on_current_space: None,
            space_ids: None,
        });
    }

    result
}

/// Return the first real input-owning window at a screen point after excluding
/// windows owned by the driver itself (notably its cursor overlay) and
/// detectably transparent overlay helpers.
///
/// `windows` must be ordered front-to-back, as returned by
/// `CGWindowListCopyWindowInfo`. Kept as pure logic so obstruction behaviour is
/// regression-testable without posting GUI events.
fn frontmost_input_window_at_point(
    windows: &[WindowInfo],
    x: f64,
    y: f64,
    driver_pid: i32,
) -> Option<&WindowInfo> {
    let window_server_pid = windows
        .iter()
        .find(|window| is_window_server_owner_name(&window.app_name))
        .map(|window| window.pid);
    windows.iter().find(|window| {
        if window.pid == driver_pid
            || window_server_pid == Some(window.pid)
            || is_window_server_owner_name(&window.app_name)
            || window.window_id == 0
            || !window.is_on_screen
        {
            return false;
        }
        // A completely transparent window cannot own a meaningful visible
        // pixel. Translucent non-normal layers are typically annotation/cursor
        // overlays; skip them when WindowServer exposes that fact.
        if window.alpha <= 0.01 || (window.layer != 0 && window.alpha < 0.95) {
            return false;
        }
        let bounds = &window.bounds;
        bounds.width > 0.0
            && bounds.height > 0.0
            && x >= bounds.x
            && y >= bounds.y
            && x < bounds.x + bounds.width
            && y < bounds.y + bounds.height
    })
}

fn is_window_server_owner_name(app_name: &str) -> bool {
    matches!(app_name.trim(), "Window Server" | "WindowServer")
}

/// Resolve an occluding window for a background pixel dispatch.
///
/// Returns `Some(window)` only when the first input-owning window at `(x, y)`
/// is not the exact requested `(pid, window_id)`. Foreground delivery skips
/// this check because fronting establishes the requested Z order.
pub fn pixel_obstruction(
    target_pid: i32,
    target_window_id: u32,
    x: f64,
    y: f64,
) -> Option<WindowInfo> {
    let windows = hit_test_windows();
    let driver_pid = std::process::id() as i32;
    pixel_obstruction_in_stack(&windows, target_pid, target_window_id, x, y, driver_pid)
}

fn pixel_obstruction_in_stack(
    windows: &[WindowInfo],
    target_pid: i32,
    target_window_id: u32,
    x: f64,
    y: f64,
    driver_pid: i32,
) -> Option<WindowInfo> {
    let owner = frontmost_input_window_at_point(windows, x, y, driver_pid)?;
    (owner.pid != target_pid || owner.window_id != target_window_id).then(|| owner.clone())
}

fn get_bounds_num(
    dict: &core_foundation::dictionary::CFDictionary<
        *const std::os::raw::c_void,
        *const std::os::raw::c_void,
    >,
    key: &str,
) -> f64 {
    use core_foundation::{
        base::{CFGetTypeID, TCFType},
        number::CFNumber,
        string::CFString,
    };
    use std::os::raw::c_void;

    let k = CFString::new(key);
    dict.find(k.as_concrete_TypeRef() as *const c_void)
        .and_then(|v| unsafe {
            let v = *v;
            if CFGetTypeID(v) == CFNumber::type_id() {
                CFNumber::wrap_under_get_rule(v as _).to_f64()
            } else {
                None
            }
        })
        .unwrap_or(0.0)
}

fn get_number(
    dict: &core_foundation::dictionary::CFDictionary<
        *const std::os::raw::c_void,
        *const std::os::raw::c_void,
    >,
    key: &str,
) -> f64 {
    get_bounds_num(dict, key)
}

/// Look up a window's bounds by its CGWindowID.
///
/// Returns `None` if the window is not currently known to WindowServer
/// (e.g. it was closed or the window_id is stale).
pub fn window_bounds_by_id(window_id: u32) -> Option<WindowBounds> {
    all_windows()
        .into_iter()
        .find(|w| w.window_id == window_id)
        .map(|w| w.bounds)
}

/// Select the best window_id for a pid.
pub fn resolve_main_window_id(pid: i32) -> anyhow::Result<u32> {
    let windows = all_windows();
    let pid_windows: Vec<&WindowInfo> = windows.iter().filter(|w| w.pid == pid).collect();
    if pid_windows.is_empty() {
        anyhow::bail!("pid {pid} has no windows");
    }
    let mut on_screen: Vec<&&WindowInfo> = pid_windows.iter().filter(|w| w.is_on_screen).collect();
    if !on_screen.is_empty() {
        on_screen.sort_by(|a, b| b.z_index.cmp(&a.z_index));
        return Ok(on_screen[0].window_id);
    }
    let largest = pid_windows.iter().max_by(|a, b| {
        let area_a = a.bounds.width * a.bounds.height;
        let area_b = b.bounds.width * b.bounds.height;
        area_a
            .partial_cmp(&area_b)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(largest.unwrap().window_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_without_alpha_defaults_to_opaque() {
        let info: WindowInfo = serde_json::from_value(serde_json::json!({
            "window_id": 1,
            "pid": 2,
            "app_name": "Notes",
            "title": "Note",
            "bounds": {"x": 0.0, "y": 0.0, "width": 100.0, "height": 100.0},
            "layer": 0,
            "z_index": 0,
            "is_on_screen": true,
            "on_current_space": true,
            "space_ids": null
        }))
        .unwrap();
        assert_eq!(info.alpha, 1.0);
    }

    fn window(window_id: u32, pid: i32, app_name: &str, layer: i32, alpha: f64) -> WindowInfo {
        WindowInfo {
            window_id,
            pid,
            app_name: app_name.to_owned(),
            title: format!("{app_name} window"),
            bounds: WindowBounds {
                x: 10.0,
                y: 20.0,
                width: 200.0,
                height: 100.0,
            },
            layer,
            alpha,
            z_index: 1,
            is_on_screen: true,
            on_current_space: Some(true),
            space_ids: None,
        }
    }

    #[test]
    fn hit_test_reports_frontmost_occluder() {
        let windows = vec![
            window(8, 200, "Notes", 0, 1.0),
            window(9, 300, "Calculator", 0, 1.0),
        ];
        let obstruction = pixel_obstruction_in_stack(&windows, 300, 9, 50.0, 50.0, 100).unwrap();
        assert_eq!(obstruction.window_id, 8);
        assert_eq!(obstruction.app_name, "Notes");
    }

    #[test]
    fn hit_test_allows_exact_target_window() {
        let windows = vec![
            window(9, 300, "Calculator", 0, 1.0),
            window(8, 200, "Notes", 0, 1.0),
        ];
        assert!(pixel_obstruction_in_stack(&windows, 300, 9, 50.0, 50.0, 100).is_none());
    }

    #[test]
    fn driver_cursor_overlay_never_obstructs_pixel_hit_test() {
        let driver_pid = 100;
        let windows = vec![
            // The real cursor overlay is layer 0 and spans the screen.
            window(7, driver_pid, "brigadier-cu", 0, 1.0),
            window(9, 300, "Calculator", 0, 1.0),
        ];
        let owner = frontmost_input_window_at_point(&windows, 50.0, 50.0, driver_pid).unwrap();
        assert_eq!(owner.window_id, 9, "driver-owned overlay must be skipped");
    }

    #[test]
    fn unpinned_cursor_anchors_above_frontmost_real_window() {
        let driver_pid = 100;
        let windows = vec![
            // The overlay itself is first in WindowServer's front-to-back list.
            window(7, driver_pid, "Brigadier Computer Use", 0, 1.0),
            window(8, 200, "Messages", 0, 1.0),
            window(9, 300, "Calculator", 0, 1.0),
        ];

        assert_eq!(
            cursor_overlay_anchor_window(&windows, driver_pid),
            Some(8),
            "move_cursor must raise the overlay above the real frontmost app, not leave it hidden behind that app",
        );
    }

    #[test]
    fn transparent_non_normal_overlay_is_skipped() {
        let windows = vec![
            window(7, 200, "Screen annotation", 25, 0.5),
            window(9, 300, "Calculator", 0, 1.0),
        ];
        let owner = frontmost_input_window_at_point(&windows, 50.0, 50.0, 100).unwrap();
        assert_eq!(owner.window_id, 9);
    }

    #[test]
    fn window_server_windows_and_owner_pid_are_skipped() {
        let windows = vec![
            window(1, 88, "Window Server", 101, 1.0),
            window(2, 88, "Hardware Cursor", 101, 1.0),
            window(9, 300, "Calculator", 0, 1.0),
        ];
        let owner = frontmost_input_window_at_point(&windows, 50.0, 50.0, 100).unwrap();
        assert_eq!(owner.window_id, 9);
    }
}
