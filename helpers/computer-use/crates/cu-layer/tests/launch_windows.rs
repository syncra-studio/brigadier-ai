//! Run the Windows launch decisions on every host without opening applications.
#[path = "../../../upstream/crates/platform-windows/src/tools/launch_policy.rs"]
mod launch_policy;

#[path = "../../../upstream/crates/platform-windows/src/tools/browser_target.rs"]
mod browser_target;

#[cfg(target_os = "windows")]
mod windows_dispatch {
    use cua_driver_core::protocol::{Content, ToolResult};
    use serde_json::json;

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

    #[tokio::test]
    async fn registered_page_never_ignores_url_hints() {
        let registry = platform_windows::register_tools_with_cursor(Default::default(), false);
        for action in ["get_text", "query_dom"] {
            let result = registry
                .invoke(
                    "page",
                    json!({
                        "action": action,
                        "pid": std::process::id(),
                        "window_id": 0,
                        "target_url_contains": "example.com"
                    }),
                )
                .await;
            assert_eq!(result.is_error, Some(true));
            assert!(
                text(&result).contains("valid window_id is required"),
                "{}",
                text(&result)
            );
        }
    }

    #[tokio::test]
    async fn registered_read_refuses_an_unbound_cdp_port() {
        let registry = platform_windows::register_tools_with_cursor(Default::default(), false);
        let result = registry
            .invoke(
                "page",
                json!({
                    "action": "get_text",
                    "pid": std::process::id(),
                    "window_id": 0,
                    "cdp_port": 9222,
                    "target_url_contains": "example.com"
                }),
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        assert!(
            text(&result).contains("cannot bind the supplied CDP port"),
            "{}",
            text(&result)
        );
    }
}
