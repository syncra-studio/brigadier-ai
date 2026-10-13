//! A menu shortcut (⌘A, ⌘Z, ⇧⌘Z, ⌘B…) for an app in the background. A key
//! combo posted to an app that isn't active never reaches its menu bar, so
//! native apps ignore it. The shortcut's menu item is pressed through
//! accessibility instead, which is what the app does with the shortcut, and
//! nothing comes to the front.
//!
//! An app in the background has no key window, so AppKit disables the menu
//! items that act on one (Select All, Undo, Bold…). For ⌘A, the text field
//! is then selected through accessibility instead; the others can't be done
//! from the background and go to the driver's posted combo.
//!
//! Chromium and Electron apps keep the posted combo: their pages see key
//! events before the menu does, and may handle the shortcut themselves. So
//! do combos with a pixel or element target, which focus that spot first.
//!
//! [`install`] wraps the driver's `hotkey`; call it before
//! [`crate::guard::install`], so the guard checks the target first.

use async_trait::async_trait;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{ProtectedResourceOwnership, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

// AXMenuItemCmdModifiers: no bits set means ⌘ alone.
const SHIFT: i64 = 1;
const OPTION: i64 = 2;
const CONTROL: i64 = 4;
const NO_COMMAND: i64 = 8;

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

/// What the app's menu bar and text fields can do from the background.
pub trait MenuBar: Send + Sync {
    /// Whether the app embeds Chromium (Electron apps and Chromium browsers).
    fn chromium(&self, pid: i32) -> bool;
    /// Makes `window_id` the app's main window, so the menu item acts on it.
    /// The app is not brought to the front.
    fn make_main(&self, pid: i32, window_id: u32);
    /// Presses the enabled menu item whose shortcut is `key` with
    /// `modifiers` (a [`menu_modifiers`] value), and returns its title.
    fn press(&self, pid: i32, key: &str, modifiers: i64) -> Option<String>;
    /// Selects all the text of the focused text field (in `window_id`, when
    /// given), or else of the first one in that window. Returns what was
    /// selected, for the answer.
    fn select_all_text(&self, pid: i32, window_id: Option<u32>) -> Option<String>;
}

/// Puts Brigadier's menu shortcuts in front of the driver's `hotkey`.
#[cfg(target_os = "macos")]
pub fn install(registry: &mut ToolRegistry) {
    install_with(registry, || Box::new(System));
}

/// [`install`] with the given menu bar.
pub fn install_with(registry: &mut ToolRegistry, menu_bar: impl FnOnce() -> Box<dyn MenuBar>) {
    registry.wrap("hotkey", move |inner| {
        Box::new(MenuShortcut {
            inner,
            menu_bar: menu_bar(),
        }) as Box<dyn Tool>
    });
}

struct MenuShortcut {
    inner: Box<dyn Tool>,
    menu_bar: Box<dyn MenuBar>,
}

/// The pid, window, key and menu modifiers of a combo the menu bar may
/// answer, or `None` when it goes to the driver as it is.
fn menu_combo(args: &Value) -> Option<(i32, Option<u32>, String, i64, String)> {
    if ["x", "y", "element_token"]
        .iter()
        .any(|key| args.get(*key).is_some_and(|value| !value.is_null()))
    {
        return None;
    }
    let pid = i32::try_from(args.get("pid")?.as_i64()?).ok()?;
    let window_id = args
        .get("window_id")
        .and_then(Value::as_u64)
        .and_then(|id| u32::try_from(id).ok());
    let keys: Vec<&str> = args
        .get("keys")?
        .as_array()?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()?;
    let (key, modifiers) = keys.split_last()?;
    if key.chars().count() != 1 {
        return None;
    }
    let modifiers_value = menu_modifiers(modifiers)?;
    Some((
        pid,
        window_id,
        key.to_string(),
        modifiers_value,
        keys.join("+"),
    ))
}

#[async_trait]
impl Tool for MenuShortcut {
    fn def(&self) -> &ToolDef {
        self.inner.def()
    }

    fn has_independent_input_lane(&self, args: &Value) -> bool {
        self.inner.has_independent_input_lane(args)
    }

    async fn protected_resource_ownership(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> ProtectedResourceOwnership {
        self.inner
            .protected_resource_ownership(adapter_id, args)
            .await
    }

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        self.inner.protected_resource_scope(adapter_id, args).await
    }

    async fn validate_protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
        approved_scope: &Value,
    ) -> Result<(), String> {
        self.inner
            .validate_protected_resource_scope(adapter_id, args, approved_scope)
            .await
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some((pid, window_id, key, modifiers, display)) = menu_combo(&args) else {
            return self.inner.invoke(args).await;
        };
        if self.menu_bar.chromium(pid) {
            return self.inner.invoke(args).await;
        }
        if let Some(window_id) = window_id {
            self.menu_bar.make_main(pid, window_id);
        }
        if let Some(title) = self.menu_bar.press(pid, &key, modifiers) {
            return ToolResult::text(format!(
                "Pressed {title} ({display}) in the menu bar of pid {pid}."
            ))
            .with_structured(json!({}));
        }
        if key.eq_ignore_ascii_case("a") && modifiers == 0 {
            if let Some(what) = self.menu_bar.select_all_text(pid, window_id) {
                return ToolResult::text(format!("Selected {what} ({display}) in pid {pid}."))
                    .with_structured(json!({}));
            }
        }
        self.inner.invoke(args).await
    }
}

/// The real menu bar, through Accessibility.
#[cfg(target_os = "macos")]
pub struct System;

#[cfg(target_os = "macos")]
impl MenuBar for System {
    fn chromium(&self, pid: i32) -> bool {
        system::chromium_family(pid)
    }

    fn make_main(&self, pid: i32, window_id: u32) {
        unsafe { system::make_main(pid, window_id) }
    }

    fn press(&self, pid: i32, key: &str, modifiers: i64) -> Option<String> {
        unsafe { system::press(pid, key, modifiers) }
    }

    fn select_all_text(&self, pid: i32, window_id: Option<u32>) -> Option<String> {
        unsafe { system::select_all_text(pid, window_id) }
    }
}

/// Whether `pid`'s app embeds Chromium (Electron apps and Chromium browsers).
#[cfg(target_os = "macos")]
pub fn chromium_family(pid: i32) -> bool {
    system::chromium_family(pid)
}

#[cfg(target_os = "macos")]
mod system {
    use core_foundation::base::{CFRange, CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use platform_macos::ax::bindings::{
        ax_get_window_id, copy_ax_windows, copy_bool_attr, copy_children, copy_element_attr,
        copy_number_attr, copy_string_attr, kAXErrorSuccess, kAXValueCFRangeType, perform_action,
        set_bool_attr_true, AXUIElementCreateApplication, AXUIElementRef,
        AXUIElementSetAttributeValue, AXValueCreate,
    };

    use super::shortcut_matches;

    /// Menus are small; this only bounds a pathological tree.
    const MAX_ITEMS: usize = 4000;

    pub unsafe fn make_main(pid: i32, window_id: u32) {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return;
        }
        for window in copy_ax_windows(app) {
            if ax_get_window_id(window) == Some(window_id) {
                set_bool_attr_true(window, "AXMain");
            }
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
    }

    pub unsafe fn press(pid: i32, key: &str, modifiers: i64) -> Option<String> {
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

    pub unsafe fn select_all_text(pid: i32, window_id: Option<u32>) -> Option<String> {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return None;
        }
        let mut target = copy_element_attr(app, "AXFocusedUIElement").filter(|&focused| {
            let keep = is_text(focused)
                && window_id.is_none_or(|id| {
                    copy_element_attr(focused, "AXWindow").is_some_and(|window| {
                        let same = ax_get_window_id(window) == Some(id);
                        CFRelease(window as CFTypeRef);
                        same
                    })
                });
            if !keep {
                CFRelease(focused as CFTypeRef);
            }
            keep
        });
        if target.is_none() {
            if let Some(id) = window_id {
                for window in copy_ax_windows(app) {
                    if target.is_none() && ax_get_window_id(window) == Some(id) {
                        target = first_text(window);
                    }
                    CFRelease(window as CFTypeRef);
                }
            }
        }
        CFRelease(app as CFTypeRef);
        let element = target?;
        let length = copy_number_attr(element, "AXNumberOfCharacters").unwrap_or(0.0) as isize;
        let done = set_range(element, "AXSelectedTextRange", CFRange::init(0, length));
        let role = copy_string_attr(element, "AXRole").unwrap_or_default();
        CFRelease(element as CFTypeRef);
        done.then(|| format!("all {length} characters of the {role}"))
    }

    unsafe fn set_range(element: AXUIElementRef, attr_name: &str, range: CFRange) -> bool {
        let value = AXValueCreate(
            kAXValueCFRangeType,
            &range as *const CFRange as *const std::ffi::c_void,
        );
        if value.is_null() {
            return false;
        }
        let attr = CFString::new(attr_name);
        let error =
            AXUIElementSetAttributeValue(element, attr.as_concrete_TypeRef(), value as CFTypeRef);
        CFRelease(value as CFTypeRef);
        error == kAXErrorSuccess
    }

    /// An editable text element: it has a text selection to set.
    unsafe fn is_text(element: AXUIElementRef) -> bool {
        matches!(
            copy_string_attr(element, "AXRole").as_deref(),
            Some("AXTextArea" | "AXTextField" | "AXSearchField" | "AXComboBox")
        ) && copy_number_attr(element, "AXNumberOfCharacters").is_some()
    }

    /// The first text element under `root`, retained.
    unsafe fn first_text(root: AXUIElementRef) -> Option<AXUIElementRef> {
        let mut pending = copy_children(root);
        let mut found = None;
        let mut seen = 0;
        while let Some(element) = pending.pop() {
            seen += 1;
            if found.is_none() && seen <= MAX_ITEMS && is_text(element) {
                found = Some(element);
                continue;
            }
            if found.is_none() && seen <= MAX_ITEMS {
                pending.extend(copy_children(element));
            }
            CFRelease(element as CFTypeRef);
        }
        found
    }

    unsafe fn press_if_shortcut(
        element: AXUIElementRef,
        key: &str,
        modifiers: i64,
    ) -> Option<String> {
        if copy_string_attr(element, "AXRole").as_deref() != Some("AXMenuItem") {
            return None;
        }
        let item_char = copy_string_attr(element, "AXMenuItemCmdChar")?;
        let item_modifiers =
            copy_number_attr(element, "AXMenuItemCmdModifiers").unwrap_or(0.0) as i64;
        if !shortcut_matches(&item_char, item_modifiers, key, modifiers)
            || copy_bool_attr(element, "AXEnabled") != Some(true)
        {
            return None;
        }
        let title = copy_string_attr(element, "AXTitle").unwrap_or_default();
        (perform_action(element, "AXPress") == kAXErrorSuccess).then_some(title)
    }

    /// Whether `pid`'s app embeds Chromium (Electron apps and Chromium browsers).
    pub fn chromium_family(pid: i32) -> bool {
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
        #[test]
        fn this_test_binary_is_not_chromium() {
            assert!(!super::chromium_family(std::process::id() as i32));
        }
    }
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
    fn only_a_plain_command_combo_for_an_app_goes_to_the_menu_bar() {
        assert_eq!(
            menu_combo(&json!({"pid": 7, "window_id": 3, "keys": ["cmd", "shift", "z"]})),
            Some((7, Some(3), "z".into(), SHIFT, "cmd+shift+z".into()))
        );
        for args in [
            json!({"pid": 7, "keys": ["ctrl", "a"]}),
            json!({"pid": 7, "keys": ["cmd", "return"]}),
            json!({"keys": ["cmd", "a"]}),
            json!({"pid": 7, "keys": ["cmd", "v"], "x": 10, "y": 10}),
            json!({"pid": 7, "keys": ["cmd", "a"], "element_token": "s00000001:4"}),
        ] {
            assert_eq!(menu_combo(&args), None, "{args}");
        }
    }
}
