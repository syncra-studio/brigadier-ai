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
    /// Whether an Open/Save panel owns the shortcut instead of the client's
    /// menu bar. Its native key equivalents must go through the driver.
    fn panel_owns_keys(&self, pid: i32, window_id: u32) -> bool;
    /// Makes `window_id` the app's main window, so the menu item acts on it.
    /// The app is not brought to the front.
    fn make_main(&self, pid: i32, window_id: u32);
    /// Presses the enabled menu item whose shortcut is `key` with
    /// `modifiers` (a [`menu_modifiers`] value), and returns its title.
    fn press(&self, pid: i32, key: &str, modifiers: i64) -> Option<String>;
    /// Held while the menu item is pressed: hands the user's front window
    /// back if the press raises one of the app's windows (an Open panel).
    fn guard_front(&self, _pid: i32) -> Option<Box<dyn Send>> {
        None
    }
    /// Selects all the text of the focused text field (in `window_id`, when
    /// given), or else of the first one in that window. Returns what was
    /// selected, for the answer.
    fn select_all_text(&self, pid: i32, window_id: Option<u32>) -> Option<String>;
    /// Presses `window_id`'s own close button through accessibility.
    /// `None` when it has none to press; else whether the window is gone
    /// from the window list afterwards.
    fn close_window(&self, _pid: i32, _window_id: u32) -> Option<bool> {
        None
    }
    /// Whether the Open/Save panel `window_id` shows a sheet within a short
    /// wait. `None` when it isn't such a panel.
    fn panel_sheet_opened(&self, _pid: i32, _window_id: u32) -> Option<bool> {
        None
    }
    /// Whether the Open/Save panel `window_id` shows a sheet right now.
    fn panel_sheet_shown(&self, _pid: i32, _window_id: u32) -> Option<bool> {
        None
    }
}

/// The answer when ⌘⇧G is asked of a panel that already shows a sheet: the
/// keys would go to that sheet, and a sheet read back afterwards proves nothing.
pub const GO_TO_FOLDER_SHEET_ALREADY_SHOWN: &str = "Nothing was sent: the Open/Save panel \
    already shows a sheet (Go to Folder, New Folder or another dialog), so Go to Folder can't \
    open on it. Read the panel: if that sheet is Go to Folder, type the path into it; otherwise \
    close it first (press_key escape), then send the shortcut again.";

/// The answer when ⌘⇧G reached a background Open/Save panel but no Go to
/// Folder sheet opened.
pub const GO_TO_FOLDER_NOT_OPENED: &str = "Go to Folder did not open: the shortcut was sent to \
    the Open/Save panel, but no Go to Folder sheet appeared within 600 ms. Read the panel: if it \
    shows another sheet or dialog, close that first. Otherwise pick the folder with the panel's \
    sidebar and file rows (click them), or open the folder with launch_app urls.";

/// The answer when the panel's Go to Folder sheet was read back.
pub const GO_TO_FOLDER_OPENED: &str = "Go to Folder opened on the Open/Save panel (confirmed: \
    its sheet is shown). Next: type_text the folder path with the same pid and window_id, then \
    press_key return; the panel then shows that folder. press_key escape closes the sheet.";

/// The confirmed Go to Folder answer.
fn go_to_folder_opened() -> ToolResult {
    use cua_driver_core::action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
        EvidenceKind, RequestedDelivery,
    };
    let result = ToolResult::text(GO_TO_FOLDER_OPENED);
    match ActionExecutionRecord::builder(
        ActionEffect::Confirmed,
        ActionTransport::MacosCgEventPid,
        RequestedDelivery::Background,
    )
    .evidence(ActionEvidence {
        kind: EvidenceKind::AccessibilityReadback,
        detail: "the panel shows its Go to Folder sheet".into(),
    })
    .actual_delivery(ActualDelivery::Background)
    .build()
    {
        Ok(record) => result.with_action_record(record),
        Err(_) => result,
    }
}

/// The answer for ⌘W done with the window's close button.
fn closed_window_result(display: &str, pid: i32, window_id: u32, gone: bool) -> ToolResult {
    use cua_driver_core::action_record::{
        ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
        EvidenceKind, RequestedDelivery,
    };
    let (text, record) = if gone {
        (
            format!(
                "Closed window {window_id} of pid {pid} ({display}) with its close button: the \
                 window is gone from the window list."
            ),
            ActionExecutionRecord::builder(
                ActionEffect::Confirmed,
                ActionTransport::MacosAxAction,
                RequestedDelivery::Background,
            )
            .evidence(ActionEvidence {
                kind: EvidenceKind::WindowChange,
                detail: "the window left the window list".into(),
            }),
        )
    } else {
        (
            format!(
                "Pressed the close button of window {window_id} of pid {pid} ({display}), but the \
                 window is still open: the app may be asking to save. Read the window to see."
            ),
            ActionExecutionRecord::builder(
                ActionEffect::Unverifiable,
                ActionTransport::MacosAxAction,
                RequestedDelivery::Background,
            ),
        )
    };
    let result = ToolResult::text(text);
    match record.actual_delivery(ActualDelivery::Background).build() {
        Ok(record) => result.with_action_record(record),
        Err(_) => result,
    }
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
        if let Some(window_id) =
            window_id.filter(|window_id| self.menu_bar.panel_owns_keys(pid, *window_id))
        {
            let go_to_folder = key.eq_ignore_ascii_case("g") && modifiers == SHIFT;
            if go_to_folder && self.menu_bar.panel_sheet_shown(pid, window_id) == Some(true) {
                return ToolResult::error(GO_TO_FOLDER_SHEET_ALREADY_SHOWN).with_structured(
                    json!({
                        "status": "failed",
                        "effect": "failed",
                        "reason": "panel_sheet_already_shown",
                    }),
                );
            }
            let result = self.inner.invoke(args).await;
            if go_to_folder && result.is_error != Some(true) {
                match self.menu_bar.panel_sheet_opened(pid, window_id) {
                    Some(true) => return go_to_folder_opened(),
                    Some(false) => {
                        return ToolResult::error(GO_TO_FOLDER_NOT_OPENED).with_structured(json!({
                            "status": "failed",
                            "effect": "failed",
                            "reason": "go_to_folder_sheet_not_shown",
                        }))
                    }
                    None => {}
                }
            }
            return result;
        }
        let pressed = {
            let _front = self.menu_bar.guard_front(pid);
            if let Some(window_id) = window_id {
                self.menu_bar.make_main(pid, window_id);
            }
            self.menu_bar.press(pid, &key, modifiers)
        };
        if let Some(title) = pressed {
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
        // Close Window is disabled while the app is in the background (Finder),
        // and the posted combo never reaches it. The window's own close button
        // does the same and works from the background.
        if let (true, 0, Some(window_id)) = (key.eq_ignore_ascii_case("w"), modifiers, window_id) {
            if let Some(gone) = self.menu_bar.close_window(pid, window_id) {
                return closed_window_result(&display, pid, window_id, gone);
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

    fn panel_owns_keys(&self, pid: i32, window_id: u32) -> bool {
        platform_macos::ax::panel::blocks_app_menu(pid, window_id)
    }

    fn make_main(&self, pid: i32, window_id: u32) {
        unsafe { system::make_main(pid, window_id) }
    }

    fn press(&self, pid: i32, key: &str, modifiers: i64) -> Option<String> {
        unsafe { system::press(pid, key, modifiers) }
    }

    fn guard_front(&self, pid: i32) -> Option<Box<dyn Send>> {
        platform_macos::key_focus_guard::protect(pid).map(|lease| Box::new(lease) as Box<dyn Send>)
    }

    fn select_all_text(&self, pid: i32, window_id: Option<u32>) -> Option<String> {
        unsafe { system::select_all_text(pid, window_id) }
    }

    fn panel_sheet_shown(&self, pid: i32, window_id: u32) -> Option<bool> {
        platform_macos::ax::panel::panel_has_sheet(pid, window_id)
    }

    fn panel_sheet_opened(&self, pid: i32, window_id: u32) -> Option<bool> {
        // A sheet the panel opens shows within a few run-loop turns.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
        loop {
            // A read can fail while the panel is busy presenting its sheet.
            let shown = platform_macos::ax::panel::panel_has_sheet(pid, window_id);
            if shown == Some(true) || std::time::Instant::now() >= deadline {
                return shown;
            }
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }

    fn close_window(&self, pid: i32, window_id: u32) -> Option<bool> {
        if !unsafe { system::press_close_button(pid, window_id) } {
            return None;
        }
        // The window leaves the window list once the app has closed it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
        loop {
            let open = crate::window_list::windows(Some(pid))
                .iter()
                .any(|w| w.id == window_id);
            if !open {
                return Some(true);
            }
            if std::time::Instant::now() >= deadline {
                return Some(false);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
        ax_get_window_id, copy_ax_windows, copy_ax_windows_including, copy_bool_attr,
        copy_children, copy_element_attr, copy_number_attr, copy_string_attr, kAXErrorSuccess,
        kAXValueCFRangeType, perform_action, set_bool_attr_true, AXUIElementCreateApplication,
        AXUIElementRef, AXUIElementSetAttributeValue, AXValueCreate,
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

    /// Press the window's enabled AXCloseButton. `true` when AX accepted it.
    pub unsafe fn press_close_button(pid: i32, window_id: u32) -> bool {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return false;
        }
        let mut pressed = false;
        for window in copy_ax_windows_including(app, pid, window_id) {
            if !pressed && ax_get_window_id(window) == Some(window_id) {
                if let Some(button) = copy_element_attr(window, "AXCloseButton") {
                    pressed = copy_bool_attr(button, "AXEnabled") != Some(false)
                        && perform_action(button, "AXPress") == kAXErrorSuccess;
                    CFRelease(button as CFTypeRef);
                }
            }
            CFRelease(window as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
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
        // A window on another Space than the user's is missing from the
        // app's window list; the lookup that includes it finds it anyway.
        if target.is_none() {
            if let Some(id) = window_id {
                for window in copy_ax_windows_including(app, pid, id) {
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
