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

#[test]
fn a_deadline_names_the_time_limit_and_how_much_was_typed() {
    use super::{deadline_reply_text, TYPING_ENDED, TYPING_NOT_STARTED, TYPING_STARTED};
    let text = "the quick brown fox jumps over the lazy dog";
    let reply = deadline_reply_text(TYPING_STARTED, 10, text, 70);
    assert!(reply.contains("70-second time limit"), "{reply}");
    assert!(reply.contains("Typed 10 of 43 characters"), "{reply}");
    assert!(reply.contains("remaining 33"), "{reply}");
    assert!(reply.contains("\"brown fox"), "{reply}");
    assert!(!reply.contains("accessibility provider"), "{reply}");

    let before = deadline_reply_text(TYPING_NOT_STARTED, 0, text, 30);
    assert!(before.contains("Nothing was typed"), "{before}");

    let after = deadline_reply_text(TYPING_ENDED, 43, text, 70);
    assert!(after.starts_with("All 43 characters were typed"), "{after}");

    // Surrogate pairs count as one character each.
    let emoji = deadline_reply_text(TYPING_STARTED, 1, "😀😀x", 30);
    assert!(emoji.contains("Typed 1 of 3 characters"), "{emoji}");
    assert!(emoji.contains("\"😀x\""), "{emoji}");
}

#[test]
fn the_insertion_deadline_grows_with_the_text_and_stays_bounded() {
    use super::type_text_insert_limit;
    assert_eq!(type_text_insert_limit(0, 30).as_secs(), 30);
    // The reported case: 309 characters 130 ms apart outlived a fixed 30 s.
    assert!(type_text_insert_limit(309, 130).as_secs() >= 85);
    assert_eq!(type_text_insert_limit(100_000, 200).as_secs(), 100);
}
