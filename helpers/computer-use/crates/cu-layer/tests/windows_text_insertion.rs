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
    assert!(typing.contains("keyboard::post_insert_text("));
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
