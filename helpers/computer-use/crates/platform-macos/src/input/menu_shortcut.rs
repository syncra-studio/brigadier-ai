//! A menu shortcut (⌘A, ⌘Z, ⇧⌘Z, ⌘B…) for an app in the background. A key
//! combo posted to an app that isn't active never reaches its menu bar, so
//! native apps ignore it. The shortcut's menu item is pressed through
//! accessibility instead, which is what the app does with the shortcut, and
//! nothing comes to the front.
//!
//! Chromium and Electron apps keep the posted combo: their pages see key
//! events before the menu does, and may handle the shortcut themselves.

use core_foundation::base::{CFRelease, CFTypeRef};

use crate::ax::bindings::{
    copy_children, copy_element_attr, copy_number_attr, copy_string_attr, copy_stringified_attr,
    perform_action, AXUIElementCreateApplication, AXUIElementRef,
};

// AXMenuItemCmdModifiers: no bits set means ⌘ alone.
const SHIFT: i64 = 1;
const OPTION: i64 = 2;
const CONTROL: i64 = 4;
const NO_COMMAND: i64 = 8;

/// Menus are small; this only bounds a pathological tree.
const MAX_ITEMS: usize = 4000;

/// A menu item's modifier value for a combo's modifiers, or `None` when the
/// combo has no ⌘ (or a modifier menus can't show).
pub fn menu_modifiers(modifiers: &[&str]) -> Option<i64> {
    let mut command = false;
    let mut value = 0;
    for modifier in modifiers {
        match modifier.to_ascii_lowercase().as_str() {
            "cmd" | "command" => command = true,
            "shift" => value |= SHIFT,
            "option" | "opt" | "alt" => value |= OPTION,
            "ctrl" | "control" => value |= CONTROL,
            _ => return None,
        }
    }
    command.then_some(value)
}

/// Whether a menu item whose shortcut is `item_char` with `item_modifiers`
/// answers `key` with `modifiers`.
pub fn shortcut_matches(item_char: &str, item_modifiers: i64, key: &str, modifiers: i64) -> bool {
    item_modifiers & NO_COMMAND == 0
        && item_modifiers == modifiers
        && key.chars().count() == 1
        && item_char.eq_ignore_ascii_case(key)
}

/// Presses the enabled menu item of `pid` whose shortcut is the combo, and
/// returns its title. `None` when the app has no such item, or is a Chromium
/// app (see the module notes).
pub fn press(pid: i32, key: &str, modifiers: &[&str]) -> Option<String> {
    let modifiers = menu_modifiers(modifiers)?;
    if key.chars().count() != 1 || chromium_family(pid) {
        return None;
    }
    unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let menu_bar = copy_element_attr(app, "AXMenuBar");
        CFRelease(app as CFTypeRef);
        let menu_bar = menu_bar?;
        let mut pending = copy_children(menu_bar);
        CFRelease(menu_bar as CFTypeRef);
        let mut seen = 0;
        let mut pressed = None;
        while let Some(element) = pending.pop() {
            seen += 1;
            if pressed.is_none() && seen <= MAX_ITEMS {
                pressed = press_if_shortcut(element, key, modifiers);
                if pressed.is_none() {
                    pending.extend(copy_children(element));
                }
            }
            CFRelease(element as CFTypeRef);
        }
        pressed
    }
}

unsafe fn press_if_shortcut(element: AXUIElementRef, key: &str, modifiers: i64) -> Option<String> {
    if copy_string_attr(element, "AXRole").as_deref() != Some("AXMenuItem") {
        return None;
    }
    let item_char = copy_string_attr(element, "AXMenuItemCmdChar")?;
    let item_modifiers = copy_number_attr(element, "AXMenuItemCmdModifiers").unwrap_or(0.0) as i64;
    if !shortcut_matches(&item_char, item_modifiers, key, modifiers)
        || copy_stringified_attr(element, "AXEnabled").as_deref() != Some("true")
    {
        return None;
    }
    let title = copy_string_attr(element, "AXTitle").unwrap_or_default();
    (perform_action(element, "AXPress") == 0).then_some(title)
}

/// Whether `pid`'s app embeds Chromium (Electron apps and Chromium browsers).
fn chromium_family(pid: i32) -> bool {
    use objc2_app_kit::NSRunningApplication;
    let Some(bundle) = (unsafe {
        NSRunningApplication::runningApplicationWithProcessIdentifier(pid)
            .and_then(|app| app.bundleURL())
            .and_then(|url| url.path())
            .map(|path| path.to_string())
    }) else {
        return false;
    };
    let Ok(frameworks) =
        std::fs::read_dir(std::path::Path::new(&bundle).join("Contents/Frameworks"))
    else {
        return false;
    };
    frameworks.flatten().any(|entry| {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        name.ends_with(".framework")
            && (name.contains("electron") || name.contains("chrom") || name.contains("edge"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_combo_needs_command_and_menu_modifiers() {
        assert_eq!(menu_modifiers(&["cmd"]), Some(0));
        assert_eq!(menu_modifiers(&["command", "shift"]), Some(SHIFT));
        assert_eq!(
            menu_modifiers(&["cmd", "option", "ctrl"]),
            Some(OPTION | CONTROL)
        );
        assert_eq!(menu_modifiers(&["shift"]), None);
        assert_eq!(menu_modifiers(&["cmd", "fn"]), None);
    }

    #[test]
    fn a_shortcut_matches_its_key_and_exact_modifiers() {
        assert!(shortcut_matches("A", 0, "a", 0));
        assert!(shortcut_matches("Z", SHIFT, "z", SHIFT));
        assert!(!shortcut_matches("Z", SHIFT, "z", 0));
        assert!(!shortcut_matches("Z", 0, "z", SHIFT));
        assert!(!shortcut_matches("A", NO_COMMAND, "a", 0));
        assert!(!shortcut_matches("A", 0, "return", 0));
    }

    #[test]
    fn textedit_is_not_chromium() {
        assert!(!chromium_family(std::process::id() as i32));
    }
}
