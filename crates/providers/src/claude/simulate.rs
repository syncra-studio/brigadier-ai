//! Claude Code's side of [`crate::simulate`].

use serde_json::{Value, json};

/// A turn that ends because the subscription's usage window ran out ([`crate::simulate`]).
pub(super) fn usage_limit(now_s: i64) -> Vec<Value> {
    let resets_s = now_s + 2 * 3_600;
    let weekly_resets_s = now_s + 4 * 86_400;
    let session = uuid::Uuid::new_v4().to_string();
    let id = || uuid::Uuid::new_v4().to_string();
    let text = "You've hit your session limit · resets in 2h";
    vec![
        json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "text", "text": "Summarize the open pull requests."}]},
            "session_id": session, "parent_tool_use_id": null, "uuid": id(), "isReplay": true,
        }),
        json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "rejected",
                "resetsAt": resets_s,
                "rateLimitType": "five_hour",
                "utilization": 1.0,
                "overageStatus": "rejected",
                "overageDisabledReason": "org_level_disabled",
                "isUsingOverage": false,
                "unifiedWindows": {
                    "five_hour": {"utilization": 1.0, "resetsAt": resets_s},
                    "seven_day": {"utilization": 0.41, "resetsAt": weekly_resets_s},
                },
            },
            "uuid": id(), "session_id": session,
        }),
        json!({
            "type": "assistant",
            "message": {
                "id": id(), "type": "message", "role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": text}],
                "stop_reason": "stop_sequence", "stop_sequence": "",
                "usage": {"input_tokens": 0, "output_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0},
            },
            "parent_tool_use_id": null, "session_id": session, "uuid": id(),
            "error": "rate_limit",
        }),
        json!({
            "type": "result", "subtype": "success", "is_error": true,
            "duration_ms": 412, "duration_api_ms": 0, "num_turns": 1,
            "result": text, "stop_reason": "stop_sequence", "session_id": session,
            "total_cost_usd": 0,
            "usage": {"input_tokens": 0, "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 0},
            "permission_denials": [], "uuid": id(),
        }),
    ]
}
