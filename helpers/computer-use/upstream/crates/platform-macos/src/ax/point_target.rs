//! Window-scoped accessibility hit testing for background pixel presses.
use super::bindings::*;
use core_foundation::base::{CFRelease, CFTypeRef};
use std::time::{Duration, Instant};

fn contains(frame: [f64; 4], x: f64, y: f64) -> bool {
    frame.iter().all(|v| v.is_finite())
        && x.is_finite()
        && y.is_finite()
        && frame[2] > 0.0
        && frame[3] > 0.0
        && x >= frame[0]
        && y >= frame[1]
        && x < frame[0] + frame[2]
        && y < frame[1] + frame[3]
}

fn unique_deepest(paths: &[Vec<usize>]) -> Option<usize> {
    let deepest = paths
        .iter()
        .enumerate()
        .max_by_key(|(_, path)| path.len())?
        .0;
    paths
        .iter()
        .all(|path| paths[deepest].starts_with(path))
        .then_some(deepest)
}

/// Returns +1 only for an advertised AXPress in this exact window. Unlike a
/// process hit test, the geometry fallback works when its Space is inactive.
///
/// # Safety
/// The caller must release the returned element.
pub unsafe fn press_target(pid: i32, wid: u32, x: f64, y: f64) -> Option<AXUIElementRef> {
    if let Some(mut element) = element_at_screen_position(pid, x, y) {
        for _ in 0..12 {
            AXUIElementSetMessagingTimeout(element, 0.05);
            if crate::ax::exact_target::element_window_id(element) != Some(wid) {
                break;
            }
            if copy_action_names(element).iter().any(|a| a == "AXPress")
                && copy_bool_attr(element, "AXEnabled") != Some(false)
                && element_screen_rect(element).is_some_and(|rect| contains(rect, x, y))
            {
                AXUIElementSetMessagingTimeout(element, 0.0);
                return Some(element);
            }
            let parent = copy_element_attr(element, "AXParent");
            CFRelease(element as CFTypeRef);
            element = match parent {
                Some(parent) => parent,
                None => return geometry_target(pid, wid, x, y),
            };
        }
        CFRelease(element as CFTypeRef);
    }
    geometry_target(pid, wid, x, y)
}

unsafe fn geometry_target(pid: i32, wid: u32, x: f64, y: f64) -> Option<AXUIElementRef> {
    let app = AXUIElementCreateApplication(pid);
    if app.is_null() {
        return None;
    }
    let windows = copy_ax_windows_including(app, pid, wid);
    CFRelease(app as CFTypeRef);
    let mut pending = Vec::new();
    let mut next_node = 0usize;
    for window in windows {
        if ax_get_window_id(window) == Some(wid) {
            pending.push((window, vec![next_node]));
            next_node += 1;
        } else {
            CFRelease(window as CFTypeRef);
        }
    }
    let deadline = Instant::now() + Duration::from_millis(300);
    let mut visited = 0;
    let mut candidates: Vec<(AXUIElementRef, Vec<usize>)> = Vec::new();
    while let Some((element, path)) = pending.pop() {
        visited += 1;
        if visited > 1500 || Instant::now() >= deadline {
            CFRelease(element as CFTypeRef);
            for (element, _) in pending {
                CFRelease(element as CFTypeRef);
            }
            for (element, _) in candidates {
                CFRelease(element as CFTypeRef);
            }
            return None;
        }
        AXUIElementSetMessagingTimeout(element, 0.05);
        let inside = element_screen_rect(element).map(|rect| contains(rect, x, y));
        // Never cross into another top-level window through a remote AX link.
        let same_window = copy_string_attr(element, "AXRole").as_deref() != Some("AXWindow")
            || ax_get_window_id(element) == Some(wid);
        if same_window && inside != Some(false) {
            if path.len() < 40 {
                for child in copy_children(element) {
                    let mut child_path = path.clone();
                    child_path.push(next_node);
                    next_node += 1;
                    pending.push((child, child_path));
                }
            }
            if inside == Some(true)
                && copy_bool_attr(element, "AXEnabled") != Some(false)
                && copy_action_names(element).iter().any(|a| a == "AXPress")
            {
                core_foundation::base::CFRetain(element as CFTypeRef);
                candidates.push((element, path));
            }
        }
        CFRelease(element as CFTypeRef);
    }
    let selected = unique_deepest(
        &candidates
            .iter()
            .map(|(_, path)| path.clone())
            .collect::<Vec<_>>(),
    );
    let mut found = None;
    for (index, (element, _)) in candidates.into_iter().enumerate() {
        if selected == Some(index) {
            AXUIElementSetMessagingTimeout(element, 0.0);
            found = Some(element);
        } else {
            CFRelease(element as CFTypeRef);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn regression_pixel_press_chooses_a_child_but_never_an_overlapping_sibling() {
        assert_eq!(
            unique_deepest(&[vec![0], vec![0, 1], vec![0, 1, 2]]),
            Some(2)
        );
        assert_eq!(unique_deepest(&[vec![0, 1], vec![0, 2, 3]]), None);
        assert_eq!(unique_deepest(&[vec![0, 1], vec![0, 1]]), Some(1));
        assert_eq!(unique_deepest(&[]), None);
    }

    #[test]
    fn regression_pixel_geometry_uses_exact_point_and_rejects_bad_frames() {
        assert!(contains([100.0, 200.0, 50.0, 40.0], 125.0, 220.0));
        assert!(!contains([100.0, 200.0, 50.0, 40.0], 150.0, 220.0));
        assert!(!contains([0.0, 0.0, -1.0, 40.0], 0.0, 0.0));
        assert!(!contains([0.0, 0.0, 10.0, 10.0], f64::NAN, 0.0));
        assert!(!contains([f64::NAN, 0.0, 10.0, 10.0], 0.0, 0.0));
    }
}
