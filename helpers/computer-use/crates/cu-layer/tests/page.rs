//! Brigadier's `page` tool in the driver's registry, behind the guard. Fake
//! apps: no permission grant, no real app, nothing brought to the front.
#![cfg(target_os = "macos")]

use std::sync::Arc;

use cu_layer::lookup::Lookups;
use cu_layer::target_policy::TargetIdentity;
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::ToolRegistry;
use serde_json::{json, Value};

/// pid 100 = the user's Brigadier; window 9000 belongs to it.
struct FakeApps;

impl Lookups for FakeApps {
    fn app_for_pid(&self, pid: i64) -> Option<TargetIdentity> {
        (pid == 100).then(|| TargetIdentity {
            name: "Brigadier".to_owned(),
            bundle_id: Some("ai.brigadier.app".to_owned()),
            pid: Some(100),
        })
    }
    fn pid_for_window(&self, window_id: u64) -> Option<i64> {
        (window_id == 9000).then_some(100)
    }
    fn identify(&self, _reference: &str) -> Option<(Option<String>, String)> {
        None
    }
    fn name_for_bundle(&self, _bundle_id: &str) -> Option<String> {
        None
    }
}

fn registry() -> Arc<ToolRegistry> {
    let mut registry = platform_macos::register_tools();
    cu_layer::page::install(&mut registry);
    cu_layer::guard::install_with(&mut registry, Arc::new(FakeApps));
    let registry = Arc::new(registry);
    registry.init_self_weak();
    registry
}

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn code(result: &ToolResult) -> Option<String> {
    let structured = result.structured_content.as_ref()?;
    structured
        .get("code")
        .or_else(|| structured.pointer("/refusal/code"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[test]
fn page_takes_the_same_arguments_as_brigadiers_own_tool() {
    let registry = registry();
    let def = registry.get_def("page").expect("page is registered");
    let mut names: Vec<&str> = def.input_schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "action",
            "attributes",
            "bundle_id",
            "cdp_port",
            "css_selector",
            "javascript",
            "pid",
            "selector",
            "target_url_contains",
            "text",
            "user_has_confirmed_enabling",
            "window_id",
        ]
    );
    assert_eq!(def.input_schema["required"], json!(["action"]));
    assert_eq!(
        def.input_schema["properties"]["action"]["enum"],
        json!([
            "execute_javascript",
            "get_text",
            "query_dom",
            "click_element",
            "insert_text",
            "type_keystrokes",
            "enable_javascript_apple_events"
        ])
    );
    // Brigadier's tool, not the driver's.
    assert!(def
        .description
        .contains("- insert_text: Insert `text` at whatever currently holds DOM focus"));
    assert!(!def.description.contains("LEGACY_PAGE_MUTATIONS"));
}

#[tokio::test]
async fn page_on_a_refused_app_is_refused_without_running() {
    let registry = registry();
    for args in [
        json!({"action": "get_text", "pid": 100, "window_id": 9000}),
        json!({"action": "query_dom", "pid": 100, "window_id": 9000, "css_selector": "#out"}),
    ] {
        let result = registry.invoke("page", args.clone()).await;
        assert_eq!(result.is_error, Some(true), "{args}: {}", text(&result));
        assert_eq!(
            code(&result).as_deref(),
            Some("target_not_allowed"),
            "{args}: {}",
            text(&result)
        );
    }
}

/// The script actions are decided by Brigadier's rules, not by the driver's
/// unrestricted mode, which stays off.
#[tokio::test]
async fn page_scripts_are_decided_by_brigadiers_rules() {
    let registry = registry();
    for action in [
        "execute_javascript",
        "click_element",
        "insert_text",
        "type_keystrokes",
    ] {
        let args = json!({"action": action, "pid": 100, "window_id": 9000,
            "javascript": "1", "selector": "#a", "text": "x"});
        let result = registry.invoke("page", args).await;
        assert_eq!(
            code(&result).as_deref(),
            Some("target_not_allowed"),
            "{action}: {}",
            text(&result)
        );
    }
    let result = registry
        .invoke(
            "page",
            json!({"action": "execute_javascript", "window_id": 9000, "javascript": "1"}),
        )
        .await;
    assert_eq!(
        code(&result).as_deref(),
        Some("host_policy_refused"),
        "{}",
        text(&result)
    );
    let result = registry
        .invoke(
            "page",
            json!({"action": "enable_javascript_apple_events", "bundle_id": "ai.brigadier.app",
                "user_has_confirmed_enabling": true}),
        )
        .await;
    assert_eq!(
        code(&result).as_deref(),
        Some("target_not_allowed"),
        "{}",
        text(&result)
    );
}

/// Without Brigadier's page tool the driver still refuses its own script
/// actions: the hook covers only a tool the host replaced.
#[tokio::test]
async fn the_drivers_own_page_scripts_stay_refused() {
    let registry = Arc::new(platform_macos::register_tools());
    registry.init_self_weak();
    let result = registry
        .invoke(
            "page",
            json!({"action": "execute_javascript", "pid": 100, "window_id": 9000,
                "javascript": "1"}),
        )
        .await;
    assert_eq!(
        code(&result).as_deref(),
        Some("unbounded_operation_requires_unrestricted"),
        "{}",
        text(&result)
    );
}
