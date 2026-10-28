use super::{changed_contains_send_message_result, post_message_readback_observed};

#[test]
fn observes_only_a_changed_value_containing_the_requested_text() {
    assert!(post_message_readback_observed(
        Some("prefix"),
        Some("prefixhello"),
        "hello"
    ));
    assert!(!post_message_readback_observed(None, None, "hello"));
    assert!(!post_message_readback_observed(
        Some("hello"),
        Some("hello"),
        "hello"
    ));
    assert!(!post_message_readback_observed(
        Some(""),
        Some("h"),
        "hello"
    ));
    assert!(!post_message_readback_observed(
        Some("before"),
        Some("different"),
        "hello"
    ));
}

#[test]
fn changed_contains_remains_unverifiable_without_retry_escalation() {
    let result = changed_contains_send_message_result(5);
    assert_eq!(result["path"], "send_message");
    assert_eq!(result["effect"], "unverifiable");
    assert_eq!(result["verify"], "changed_contains");
    assert_eq!(result["verified"], false);
    assert!(result.get("escalation").is_none());

    let record = cua_driver_core::action_record::ActionExecutionRecord::from_legacy(
        "type_text",
        &serde_json::json!({"delivery_mode": "background"}),
        &result,
    )
    .expect("changed-and-contains SendMessage result should normalize");
    assert_eq!(
        record.transport,
        cua_driver_core::action_record::ActionTransport::WindowsSendMessage
    );
    let public = serde_json::to_value(record.public_result().expect("valid ActionResult"))
        .expect("serialize ActionResult");
    assert_eq!(public["effect"], "unverifiable");
    assert!(public.get("escalation").is_none());
    assert!(public.get("evidence").is_none());
}

#[test]
fn native_combo_writes_report_the_synchronous_message_transport() {
    use crate::win32::combo::{Combo, ComboWrite};
    let result = super::win32_combo_set_value_result(
        0,
        "chosen",
        Combo {
            hwnd: 0,
            editable: false,
        },
        ComboWrite::Applied {
            selected: Some(0),
            unchanged: false,
            readback_text: Some("chosen".into()),
            readback_index: Some(0),
        },
        &serde_json::json!({}),
    );
    let record = result.action_record.unwrap();
    assert_eq!(
        record.transport,
        cua_driver_core::action_record::ActionTransport::WindowsSendMessage
    );
    assert_eq!(record.debug_json()["transport"], "windows_send_message");
}
