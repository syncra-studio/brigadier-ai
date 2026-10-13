//! A quick look at which normal windows exist, for waiting on a new one.
//!
//! The driver's window list also asks the window server which Space each
//! window is on, one window at a time, which takes about 20 ms with a few
//! hundred windows open. Watching for a window to appear needs only the ids
//! and sizes, which one window-list call gives in a few milliseconds.

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

/// A normal (layer 0) window: its id, owner and size in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    pub id: u32,
    pub pid: i32,
    pub width: f64,
    pub height: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(
        option: u32,
        relative_to_window: u32,
    ) -> core_foundation::array::CFArrayRef;
}

/// `kCGWindowListExcludeDesktopElements`: every window, on screen or not,
/// without the desktop's own.
const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;

/// Every layer-0 window, or only `pid`'s.
pub fn windows(pid: Option<i32>) -> Vec<Window> {
    // SAFETY: the call returns a +1 array (or null), wrapped under the
    // create rule so it is released once.
    let raw = unsafe { CGWindowListCopyWindowInfo(EXCLUDE_DESKTOP_ELEMENTS, 0) };
    if raw.is_null() {
        return Vec::new();
    }
    let rows: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_create_rule(raw) };
    rows.iter()
        .filter_map(|row| {
            let number = |key: &'static str| {
                row.find(CFString::from_static_string(key))
                    .and_then(|value| value.downcast::<CFNumber>())
            };
            if number("kCGWindowLayer")?.to_i64()? != 0 {
                return None;
            }
            let owner = number("kCGWindowOwnerPID")?.to_i64()? as i32;
            if pid.is_some_and(|pid| pid != owner) {
                return None;
            }
            let id = number("kCGWindowNumber")?.to_i64()? as u32;
            let bounds = row
                .find(CFString::from_static_string("kCGWindowBounds"))
                .and_then(|value| value.downcast::<CFDictionary>())?;
            let side = |key: &'static str| {
                bounds
                    .find(CFString::from_static_string(key).as_CFTypeRef())
                    .map(|value| unsafe { CFType::wrap_under_get_rule(*value) })
                    .and_then(|value| value.downcast::<CFNumber>())
                    .and_then(|number| number.to_f64())
            };
            Some(Window {
                id,
                pid: owner,
                width: side("Width")?,
                height: side("Height")?,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_lists_the_same_windows_as_the_driver() {
        // A test process may have no window server session: nothing to compare.
        let ours: std::collections::HashSet<u32> = windows(None).iter().map(|w| w.id).collect();
        let driver: std::collections::HashSet<u32> = platform_macos::windows::all_windows()
            .iter()
            .map(|w| w.window_id)
            .collect();
        // Windows can open or close between the two calls.
        let common = ours.intersection(&driver).count();
        assert!(common + 5 >= driver.len(), "{common} of {}", driver.len());
    }
}
