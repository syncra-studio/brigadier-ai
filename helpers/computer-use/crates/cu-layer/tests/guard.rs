//! The guard holds the same way for a single call and for a `run_steps`
//! step. Fake tools and fake apps: no permission grant, no real app.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cu_layer::lookup::Lookups;
use cu_layer::scope::{self, DispatchScope};
use cu_layer::target_policy::TargetIdentity;
use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};

/// pid 100 = the user's Brigadier, 102 = TextEdit, 103 = Terminal,
/// 106 = System Settings. Window 9000 belongs to Brigadier, 9001 to TextEdit.
struct FakeApps;

fn app(name: &str, bundle: &str, pid: i64) -> TargetIdentity {
    TargetIdentity {
        name: name.to_owned(),
        bundle_id: Some(bundle.to_owned()),
        pid: Some(pid),
    }
}

impl Lookups for FakeApps {
    fn app_for_pid(&self, pid: i64) -> Option<TargetIdentity> {
        match pid {
            100 => Some(app("Brigadier", "ai.brigadier.app", 100)),
            102 => Some(app("TextEdit", "com.apple.TextEdit", 102)),
            103 => Some(app("Terminal", "com.apple.Terminal", 103)),
            106 => Some(app("System Settings", "com.apple.systempreferences", 106)),
            _ => None,
        }
    }
    fn pid_for_window(&self, window_id: u64) -> Option<i64> {
        match window_id {
            9000 => Some(100),
            9001 => Some(102),
            _ => None,
        }
    }
    fn identify(&self, reference: &str) -> Option<(Option<String>, String)> {
        (reference == "TextEdit").then(|| (Some("com.apple.TextEdit".into()), "TextEdit".into()))
    }
    fn name_for_bundle(&self, _bundle_id: &str) -> Option<String> {
        None
    }
}

/// A stand-in driver tool that records its calls. `get_window_state`
/// answers with a snapshot of the pid and window it was asked about.
struct Probe {
    def: ToolDef,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

static NEXT_SNAPSHOT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x100);

#[async_trait]
impl Tool for Probe {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        self.calls
            .lock()
            .unwrap()
            .push((self.def.name.clone(), args.clone()));
        match self.def.name.as_str() {
            "get_window_state" => {
                let snapshot =
                    NEXT_SNAPSHOT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                ToolResult::text("- [1] AXButton \"Foreground\"").with_structured(json!({
                    "pid": args["pid"], "window_id": args["window_id"],
                    "snapshot_id": format!("s{snapshot:08x}"),
                    "tree_markdown": "- [1] AXButton \"Foreground\"",
                    "hint": "If a click is refused, retry with delivery_mode:\"foreground\". Read again after acting.",
                }))
            }
            "type_text" => ToolResult::error(
                "Nothing was typed. Re-send with delivery_mode:\"foreground\". The Cua Driver could not confirm it.",
            ),
            name => ToolResult::text(format!("{name} done")),
        }
    }
}

const TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "set_value",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
    "move_cursor",
    "get_window_state",
    "zoom",
    "invoke_menu",
    "launch_app",
    "bring_to_front",
    "clipboard_read",
    "list_windows",
];

struct Harness {
    registry: Arc<ToolRegistry>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Harness {
    fn new() -> Self {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut registry = ToolRegistry::new();
        for name in TOOLS {
            registry.register(Box::new(Probe {
                def: ToolDef {
                    name: (*name).into(),
                    description: "fake".into(),
                    input_schema: json!({"type": "object", "additionalProperties": true}),
                    read_only: false,
                    destructive: false,
                    idempotent: false,
                    open_world: false,
                },
                calls: calls.clone(),
            }));
        }
        registry.register_session_tools();
        cu_layer::guard::install_with(&mut registry, Arc::new(FakeApps));
        let registry = Arc::new(registry);
        registry.init_self_weak();
        Self { registry, calls }
    }

    async fn call(&self, tool: &str, args: Value) -> ToolResult {
        self.registry.invoke(tool, args).await
    }

    async fn step(&self, tool: &str, args: Value) -> ToolResult {
        self.registry
            .invoke(
                "run_steps",
                json!({"steps": [{"tool": tool, "args": args}]}),
            )
            .await
    }

    fn ran(&self, tool: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(name, _)| name == tool)
            .count()
    }
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

/// Refused directly with `expected`, and refused as a step with the same
/// message, and the tool never ran either way.
async fn refused_both_ways(harness: &Harness, tool: &str, args: Value, expected: &str) {
    let ran_before = harness.ran(tool);
    let direct = harness.call(tool, args.clone()).await;
    assert_eq!(
        direct.is_error,
        Some(true),
        "{tool} {args}: {}",
        text(&direct)
    );
    assert_eq!(
        code(&direct).as_deref(),
        Some(expected),
        "{tool} {args}: {}",
        text(&direct)
    );
    let message = text(&direct);
    let batched = harness.step(tool, args.clone()).await;
    assert_eq!(
        batched.is_error,
        Some(true),
        "step {tool} {args}: {}",
        text(&batched)
    );
    let batched_text = text(&batched);
    let first_line = message.lines().next().unwrap();
    assert!(
        batched_text.contains(first_line)
            || batched_text.contains(&first_line[..first_line.len().min(80)]),
        "step {tool} {args}: expected «{first_line}» in «{batched_text}»"
    );
    assert_eq!(harness.ran(tool), ran_before, "{tool} must not run");
}

#[tokio::test]
async fn a_refused_app_is_refused_directly_and_as_a_step() {
    let harness = Harness::new();
    refused_both_ways(
        &harness,
        "click",
        json!({"pid": 100, "x": 1, "y": 1}),
        "target_not_allowed",
    )
    .await;
    refused_both_ways(
        &harness,
        "type_text",
        json!({"pid": 102, "window_id": 9000, "text": "a"}),
        "target_not_allowed",
    )
    .await;
    refused_both_ways(
        &harness,
        "press_key",
        json!({"pid": 103, "key": "a"}),
        "target_not_allowed",
    )
    .await;
    refused_both_ways(
        &harness,
        "set_value",
        json!({"pid": 106, "value": "1"}),
        "target_not_allowed",
    )
    .await;
}

#[tokio::test]
async fn acting_in_front_or_on_the_desktop_is_refused_directly_and_as_a_step() {
    let harness = Harness::new();
    refused_both_ways(
        &harness,
        "click",
        json!({"pid": 102, "x": 1, "y": 1, "delivery_mode": "foreground"}),
        "foreground_refused",
    )
    .await;
    refused_both_ways(
        &harness,
        "drag",
        json!({"pid": 102, "from_x": 1, "from_y": 1, "to_x": 2, "to_y": 2}),
        "foreground_refused",
    )
    .await;
    refused_both_ways(
        &harness,
        "move_cursor",
        json!({"x": 1, "y": 1, "scope": "desktop"}),
        "foreground_refused",
    )
    .await;
    refused_both_ways(
        &harness,
        "click",
        json!({"target": {"kind": "desktop", "display_id": "primary"}, "x": 1, "y": 1}),
        "foreground_refused",
    )
    .await;
    let direct = harness.call("bring_to_front", json!({"pid": 102})).await;
    assert_eq!(code(&direct).as_deref(), Some("foreground_refused"));
    let fallback = harness
        .registry
        .invoke(
            "run_steps",
            json!({"pid": 102, "foreground_fallback": true, "steps": [{"tool": "click", "args": {"x": 1, "y": 1}}]}),
        )
        .await;
    assert_eq!(code(&fallback).as_deref(), Some("foreground_refused"));
    assert_eq!(harness.ran("click"), 0);
    assert_eq!(harness.ran("bring_to_front"), 0);
}

#[tokio::test]
async fn an_element_token_is_checked_against_the_app_it_came_from() {
    let harness = Harness::new();
    let read = harness
        .call("get_window_state", json!({"pid": 100, "window_id": 9000}))
        .await;
    assert_eq!(
        code(&read).as_deref(),
        Some("target_not_allowed"),
        "reading Brigadier is refused too"
    );

    // A read of TextEdit is allowed; its token works alone.
    let read = harness
        .call("get_window_state", json!({"pid": 102, "window_id": 9001}))
        .await;
    assert_ne!(read.is_error, Some(true), "{}", text(&read));
    let snapshot = read.structured_content.as_ref().unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let clicked = harness
        .call("click", json!({"element_token": format!("{snapshot}:1")}))
        .await;
    assert_ne!(clicked.is_error, Some(true), "{}", text(&clicked));

    // A token of a snapshot nobody read through the guard is refused.
    refused_both_ways(
        &harness,
        "click",
        json!({"element_token": "s7ffffff0:1"}),
        "target_unresolved",
    )
    .await;
}

#[tokio::test]
async fn a_token_of_a_refused_app_is_refused() {
    let harness = Harness::new();
    // An allowed read of the Terminal (unlocked for this session) mints a
    // token; without the unlock the same token is refused.
    let unlocked = DispatchScope {
        allowed_bundles: vec!["com.apple.terminal".into()],
        ..Default::default()
    };
    let read = scope::with_scope(
        unlocked,
        harness.call("get_window_state", json!({"pid": 103, "window_id": 1})),
    )
    .await;
    assert_ne!(read.is_error, Some(true), "{}", text(&read));
    let snapshot = read.structured_content.as_ref().unwrap()["snapshot_id"]
        .as_str()
        .unwrap()
        .to_owned();
    refused_both_ways(
        &harness,
        "click",
        json!({"element_token": format!("{snapshot}:1")}),
        "target_not_allowed",
    )
    .await;
}

#[tokio::test]
async fn an_app_the_user_stopped_is_refused_directly_and_as_a_step() {
    let harness = Harness::new();
    let noted = Arc::new(Mutex::new(Vec::new()));
    let log = noted.clone();
    let stopped = DispatchScope {
        on_target: Some(Arc::new(move |tool: &str, app: Option<&TargetIdentity>| {
            log.lock()
                .unwrap()
                .push((tool.to_owned(), app.map(|app| app.name.clone())));
            app.filter(|app| app.name == "TextEdit")
                .map(|_| "The user asked agents to stop using TextEdit.".to_owned())
        })),
        ..Default::default()
    };
    scope::with_scope(stopped, async {
        refused_both_ways(
            &harness,
            "click",
            json!({"pid": 102, "x": 1, "y": 1}),
            "stopped_by_user",
        )
        .await;
        let launch = harness
            .call("launch_app", json!({"name": "TextEdit"}))
            .await;
        assert_eq!(code(&launch).as_deref(), Some("stopped_by_user"));
    })
    .await;
    assert!(noted
        .lock()
        .unwrap()
        .iter()
        .any(|(tool, app)| tool == "click" && app.as_deref() == Some("TextEdit")));
}

#[tokio::test]
async fn tools_brigadier_does_not_offer_are_refused() {
    let harness = Harness::new();
    let result = harness.call("clipboard_read", json!({})).await;
    assert_eq!(code(&result).as_deref(), Some("tool_not_offered"));
    assert_eq!(harness.ran("clipboard_read"), 0);
}

#[tokio::test]
async fn results_never_suggest_the_foreground_and_keep_what_the_app_shows() {
    let harness = Harness::new();
    let read = harness
        .call("get_window_state", json!({"pid": 102, "window_id": 9001}))
        .await;
    let structured = read.structured_content.as_ref().unwrap();
    assert_eq!(structured["hint"], "Read again after acting.");
    assert_eq!(structured["tree_markdown"], "- [1] AXButton \"Foreground\"");
    assert_eq!(text(&read), "- [1] AXButton \"Foreground\"");

    let typed = harness
        .call("type_text", json!({"pid": 102, "text": "a"}))
        .await;
    assert_eq!(
        text(&typed),
        "Nothing was typed. The Brigadier Computer Use could not confirm it."
    );
    let batched = harness
        .step("type_text", json!({"pid": 102, "text": "a"}))
        .await;
    let batched = text(&batched).to_ascii_lowercase();
    assert!(!batched.contains("foreground"), "{batched}");
    assert!(!batched.contains("delivery_mode"), "{batched}");
}
