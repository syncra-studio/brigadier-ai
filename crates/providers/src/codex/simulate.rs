//! Codex's side of [`crate::simulate`].

use serde_json::{Value, json};

/// A turn that ends because the subscription's usage window ran out ([`crate::simulate`]).
pub(super) fn usage_limit(now_s: i64) -> Vec<Value> {
    let resets_s = now_s + 2 * 3_600;
    let weekly_resets_s = now_s + 4 * 86_400;
    let session = uuid::Uuid::new_v4().to_string();
    let id = || uuid::Uuid::new_v4().to_string();
    let turn = id();
    let error = json!({
        "message": "You've hit your usage limit. Visit https://chatgpt.com/codex/settings/usage to purchase more credits or try again in 2 hours.",
        "codexErrorInfo": "usageLimitExceeded",
        "additionalDetails": null,
    });
    let limits = json!({
        "limitId": "codex", "limitName": null, "normalModelSlug": null,
        "primary": {"usedPercent": 100, "windowDurationMins": 300, "resetsAt": resets_s},
        "secondary": {"usedPercent": 41, "windowDurationMins": 10_080, "resetsAt": weekly_resets_s},
        "credits": {"hasCredits": false, "unlimited": false, "balance": "0"},
        "individualLimit": null, "spendControlReached": false, "planType": "pro",
        "rateLimitReachedType": "rate_limit_reached",
    });
    vec![
        json!({"method": "turn/started", "params": {"threadId": session, "turn": {
            "id": turn, "items": [], "itemsView": "notLoaded", "status": "inProgress",
            "error": null, "startedAt": now_s, "completedAt": null, "durationMs": null,
        }}}),
        json!({"method": "item/completed", "params": {"threadId": session, "turnId": turn, "completedAtMs": now_s * 1000, "item": {
            "type": "userMessage", "id": id(), "clientId": null,
            "content": [{"type": "text", "text": "Summarize the open pull requests.", "text_elements": []}],
        }}}),
        json!({"method": "account/rateLimits/updated", "params": {"rateLimits": limits}}),
        json!({"method": "error", "params": {
            "error": error, "willRetry": false, "threadId": session, "turnId": turn,
        }}),
        json!({"method": "turn/completed", "params": {"threadId": session, "turn": {
            "id": turn, "items": [], "itemsView": "notLoaded", "status": "failed",
            "error": error, "startedAt": now_s, "completedAt": now_s + 1, "durationMs": 640,
        }}}),
    ]
}
