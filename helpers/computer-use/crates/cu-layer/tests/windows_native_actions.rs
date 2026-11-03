//! Exact native command identities are tested on every host.
#[path = "../../../upstream/crates/platform-windows/src/input/action_policy.rs"]
mod action_policy;

#[test]
fn synchronous_and_posted_messages_keep_distinct_transport_labels() {
    use cua_driver_core::action_record::{
        ActionEffect, ActionExecutionRecord, ActionRoute, ActionTransport,
    };
    for (path, transport, label) in [
        (
            "send_message",
            ActionTransport::WindowsSendMessage,
            "windows_send_message",
        ),
        (
            "SendMessageW",
            ActionTransport::WindowsSendMessage,
            "windows_send_message",
        ),
        (
            "SendMessageTimeoutW",
            ActionTransport::WindowsSendMessage,
            "windows_send_message",
        ),
        (
            "post_message",
            ActionTransport::WindowsPostMessage,
            "windows_post_message",
        ),
    ] {
        for tool in ["click", "type_text"] {
            let record = ActionExecutionRecord::from_legacy(
                tool,
                &serde_json::json!({}),
                &serde_json::json!({
                    "path": path, "effect": "unverifiable", "verified": false,
                }),
            )
            .unwrap();
            assert_eq!(record.transport, transport);
            assert_eq!(record.debug_json()["transport"], label);
            assert_eq!(record.transport.route(), ActionRoute::SyntheticEvents);
            assert_eq!(record.effect, ActionEffect::Unverifiable);
            assert!(record.evidence.is_empty());
        }
    }
}

#[test]
fn message_delivery_acknowledgement_cannot_confirm_an_effect() {
    use cua_driver_core::action_record::{ActionEffect, ActionExecutionRecord, ActionTransport};
    let record = ActionExecutionRecord::from_legacy(
        "click",
        &serde_json::json!({}),
        &serde_json::json!({
            "path": "send_message", "effect": "confirmed", "verified": true,
        }),
    )
    .unwrap();
    assert_eq!(record.transport, ActionTransport::WindowsSendMessage);
    assert_eq!(record.effect, ActionEffect::Unverifiable);
    assert!(record.evidence.is_empty());
}

#[cfg(target_os = "windows")]
#[test]
fn pointer_injection_fallback_refuses_before_any_desktop_access() {
    // Zero handles/coordinates deliberately provide no desktop target. The
    // safety refusal must happen before HWND lookup, device creation or input.
    for button in ["left", "right"] {
        let click = platform_windows::input::inject_click_screen(0, 0, 0, 1, button)
            .unwrap_err()
            .to_string();
        assert!(click.contains("background_unavailable"));
        assert!(click.contains("moving your pointer"));
        assert!(click.contains("No click was sent"));
        let drag = platform_windows::input::inject::inject_drag_screen(0, 0, 0, 1, 1, 8, button)
            .unwrap_err()
            .to_string();
        assert!(drag.contains("background_unavailable"));
        assert!(drag.contains("moving your pointer"));
        assert!(drag.contains("No drag was sent"));
    }
}
