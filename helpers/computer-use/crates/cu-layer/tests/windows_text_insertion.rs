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
    assert!(typing.contains("post_insert_text(text_target"));
}
