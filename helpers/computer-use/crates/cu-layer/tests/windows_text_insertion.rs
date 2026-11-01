//! Portable regression coverage for Windows caret insertion decisions.
#[path = "../../../upstream/crates/platform-windows/src/input/text_policy.rs"]
mod text_policy;

#[test]
fn type_text_never_calls_whole_document_set_value() {
    let source = include_str!("../../../upstream/crates/platform-windows/src/tools/impl_.rs");
    let typing = source
        .split("impl Tool for TypeTextTool")
        .nth(1)
        .unwrap()
        .split("fn post_message_readback_observed")
        .next()
        .unwrap();
    assert!(!typing.contains(".SetValue("));
    assert!(!typing.contains(".SetFocus("));
    assert!(typing.contains("keyboard::post_insert_text_watched("));
}

#[test]
fn background_typing_watches_for_the_users_input_between_characters() {
    let source = include_str!("../../../upstream/crates/platform-windows/src/tools/impl_.rs");
    let typing = source
        .split("impl Tool for TypeTextTool")
        .nth(1)
        .unwrap()
        .split("fn post_message_readback_observed")
        .next()
        .unwrap();
    // Every per-character insertion goes through the watched loop, the
    // monitor is armed before it, and a stop is reported as such.
    assert!(!typing.contains("keyboard::post_insert_text_cancelable("));
    assert!(typing.contains("UserInputMonitor::start("));
    assert!(typing.contains("monitor.check()"));
    assert!(typing.contains("InsertStop::Unmonitored"));
    assert!(typing.contains("downcast_ref::<crate::input::text_policy::InsertStopped>()"));
    let watch = include_str!(
        "../../../upstream/crates/platform-windows/src/input/foreground_watch.rs"
    );
    // Pointer motion counts: the mouse hook notes input before filtering to buttons.
    let mouse = watch.split("fn mouse_event(").nth(1).unwrap();
    let noted = mouse.find("note_user_input(").unwrap();
    let buttons = mouse.find("WM_LBUTTONDOWN").unwrap();
    assert!(noted < buttons);
    assert!(mouse[..buttons].contains("LLMHF_INJECTED"));
}

#[cfg(target_os = "windows")]
use platform_windows::input;
#[cfg(target_os = "windows")]
#[path = "../../../upstream/crates/platform-windows/src/input/native_text_tests.rs"]
mod native_text_tests;

#[test]
fn elevated_processes_use_the_exact_read_and_input_refusal() {
    let message =
        cua_driver_core::window_state_view::windows_integrity_message(0x2000, 0x3000).unwrap();
    assert_eq!(message, "This window runs as administrator and cannot be read or controlled here.\nHint: open the app without administrator privileges, or restart the computer-use helper as administrator.");
}
