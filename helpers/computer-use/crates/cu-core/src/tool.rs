//! Tool trait and registry.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    pip_hook,
    protocol::{Content, ToolResult},
    recording::{now_ms, screenshot_for, RecordingSession},
    recording_tools::{
        init_replay_registry, GetRecordingStateTool, ReplayTrajectoryTool, StartRecordingTool,
        StopRecordingTool,
    },
    tool_args::ArgsExt,
};

/// MCP `tools/list` capability-vocabulary version. Bumped on BREAKING
/// changes only (renaming a capability token, removing a capability
/// claim from a tool). Additive changes — new capability tokens, new
/// tools, new tools that newly claim an existing token — keep the
/// version. Agent clients read this to gate
/// strict-vs-tolerant capability matching. See
/// `default_capabilities_for` for the live vocabulary.
pub const CAPABILITY_VERSION: &str = "1";

/// Metadata for a single tool.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

impl ToolDef {
    pub fn to_list_entry(&self) -> Value {
        // `capabilities` is always emitted (even when empty) so consumers
        // can rely on the key existing. Additive only — old consumers
        // that ignore the field keep working unchanged.
        //
        // Capabilities are resolved from the centralised
        // `default_capabilities_for` name → tokens map. Keeping the
        // mapping in one place — rather than scattered across every
        // per-platform tool literal — means adding a new capability
        // claim is a one-file change, and sibling PRs touching
        // individual tool files don't conflict with this surface.
        let caps = default_capabilities_for(&self.name);
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            },
            "capabilities": caps,
        })
    }

    /// MCP tool entry matching Codex Computer Use's public wire contract.
    /// The native profile keeps its additive capability metadata, while this
    /// compatibility entry deliberately contains only the built-in fields.
    pub fn to_codex_computer_use_list_entry(&self) -> Value {
        serde_json::json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.idempotent,
                "openWorldHint": self.open_world,
            },
        })
    }
}

/// Centralised tool name → capability tokens map. Lookup is by name so
/// platform-specific tool modules don't have to declare their own
/// capabilities — keeps the additive-only contract tight and avoids
/// merge collisions with sibling agents touching the same tool files.
///
/// ### Vocabulary
/// Capability strings are dotted-namespace tokens. The canonical set
/// is (extend additively as new tools / surfaces ship — never rename
/// without bumping `CAPABILITY_VERSION`):
///
/// - `input.pointer.click`, `input.pointer.click.left`,
///   `input.pointer.click.right`, `input.pointer.click.double`,
///   `input.pointer.drag`, `input.pointer.scroll`,
///   `input.pointer.move`, `input.pointer.button` (raw down/up)
/// - `input.keyboard.type`, `input.keyboard.hotkey`,
///   `input.keyboard.press`
/// - `screen.capture`, `screen.capture.window`,
///   `screen.capture.region`, `screen.dimensions`,
///   `screen.cursor.position`
/// - `accessibility.tree`, `accessibility.tree.structured`,
///   `accessibility.tree.bounded`, `accessibility.window_state`,
///   `accessibility.element_tokens` (Surface 6 — tool accepts the
///   opaque `element_token` arg alongside the integer `element_index`)
/// - `app.launch`, `app.list`, `app.kill`, `window.list`,
///   `window.activate`, `window.debug_info`
/// - `system.permissions.tcc`,
///   `system.permissions.tcc.accessibility`,
///   `system.permissions.tcc.screen_recording`
/// - `system.config.read`, `system.config.write`
/// - `session.lifecycle.start`, `session.lifecycle.end`
/// - `agent_cursor.move`, `agent_cursor.set_enabled`,
///   `agent_cursor.set_motion`, `agent_cursor.set_style`,
///   `agent_cursor.state`
/// - `recording.start`, `recording.stop`, `recording.state`,
///   `recording.replay`, `recording.install_dependency`
/// - `page.action`
/// - `driver.update_check`, `driver.probe`
///
/// Tools with no entry get `[]` — that's fine, it just means
/// downstream consumers fall back to matching by tool name for them.
pub fn default_capabilities_for(tool_name: &str) -> Vec<String> {
    let caps: &[&str] = match tool_name {
        // ── input.pointer ────────────────────────────────────────────
        //
        // Surface 6: tools that accept the opaque `element_token` arg
        // (in addition to the integer `element_index`) claim the
        // `accessibility.element_tokens` token so consumers can branch
        // on its presence
        // for each tool; the capability token removes that coupling.
        "click" => &[
            "input.pointer.click",
            "input.pointer.click.left",
            "accessibility.element_tokens",
        ],
        "double_click" => &[
            "input.pointer.click",
            "input.pointer.click.left",
            "input.pointer.click.double",
            "accessibility.element_tokens",
        ],
        "right_click" => &[
            "input.pointer.click",
            "input.pointer.click.right",
            "accessibility.element_tokens",
        ],
        "drag" => &["input.pointer.drag"],
        "mouse_drag" => &["input.pointer.drag"],
        "parallel_mouse_drag" => &["input.pointer.drag"],
        "mouse_button_down" => &["input.pointer.button"],
        "mouse_button_up" => &["input.pointer.button"],
        "scroll" => &["input.pointer.scroll", "accessibility.element_tokens"],
        "move_cursor" => &[
            // Visual overlay move, not a real OS pointer move on
            // macOS/Windows — see SKILL.md. Surfaced as
            // `agent_cursor.move` because the canonical name on the
            // overlay side is "agent cursor"; `input.pointer.move` is
            // intentionally omitted to avoid claiming we shift the
            // real cursor.
            "agent_cursor.move",
        ],

        // ── input.keyboard ───────────────────────────────────────────
        // `type_text` claims `terminal_safe` because every platform
        // implementation detects terminal-emulator targets (bundle id
        // on macOS, WM_CLASS / process name on Linux, window class on
        // Windows) and routes past the accessibility-text channel to
        // key-event synthesis — bypassing the silent-drop that
        // otherwise affects Ghostty / iTerm2 / Terminal.app / Windows
        // Terminal / mintty / GVim, etc. See the per-platform
        // `terminal` module for the matched list and the structured
        // `path: "ax" | "key_events"` field on the response.
        "type_text" => &[
            "input.keyboard.type",
            "input.keyboard.type.terminal_safe",
            "accessibility.element_tokens",
        ],
        // `type_text_chars` is a deprecated alias resolved at invoke
        // time on macOS/Windows. On Linux it's still registered (see
        // platform-linux/impl_.rs). The Linux implementation runs
        // XSendEvent per-character without the terminal short-circuit,
        // so we deliberately do NOT claim `terminal_safe` here — the
        // contract is intentionally narrower than `type_text`'s. It
        // still accepts `element_token`, hence the tokens claim.
        "type_text_chars" => &["input.keyboard.type", "accessibility.element_tokens"],
        "press_key" => &["input.keyboard.press", "accessibility.element_tokens"],
        "hotkey" => &["input.keyboard.hotkey"],
        "set_value" => &[
            // Bulk-set an editable field's value — semantically a
            // typing surface, even though the implementation skips
            // per-key events.
            "input.keyboard.type",
            "accessibility.element_tokens",
        ],

        // ── screen / capture ─────────────────────────────────────────
        // Note: the regular `screenshot` tool was removed from the
        // surface in PR #1692 — get_window_state's vision capture mode
        // is the canonical screenshot path. `zoom` returns a JPEG of
        // a window region, so it claims screen.capture.region.
        "zoom" => &[
            "screen.capture",
            "screen.capture.window",
            "screen.capture.region",
        ],
        "get_screen_size" => &["screen.dimensions"],
        "get_desktop_state" => &["screen.capture", "screen.dimensions"],
        "get_cursor_position" => &["screen.cursor.position"],

        // ── accessibility / window state ─────────────────────────────
        "get_accessibility_tree" => &["accessibility.tree", "accessibility.tree.structured"],
        "get_window_state" => &[
            "accessibility.window_state",
            "accessibility.tree",
            "accessibility.tree.structured",
            "accessibility.tree.bounded",
            // Surface 6: emits `element_token` on every structured
            // element entry — paired with the existing integer
            // `element_index`.
            "accessibility.element_tokens",
            // capture_mode:"vision" returns a window screenshot — see
            // platform-{macos,windows,linux}/src/tools/get_window_state.rs.
            "screen.capture",
            "screen.capture.window",
        ],

        // ── apps / windows ───────────────────────────────────────────
        "launch_app" => &["app.launch"],
        "list_apps" => &["app.list"],
        "kill_app" => &["app.kill"],
        "list_windows" => &["window.list"],
        "bring_to_front" => &["window.activate"],
        "debug_window_info" => &["window.debug_info"],

        // ── permissions / config ─────────────────────────────────────
        // The macOS TCC tokens are claimed even on Windows/Linux —
        // `check_permissions` on those platforms still reports the
        // same accessibility/screen_recording booleans (mapped to the
        // platform's own permission model), so the capability surface
        // stays platform-agnostic.
        "check_permissions" => &[
            "system.permissions.tcc",
            "system.permissions.tcc.accessibility",
            "system.permissions.tcc.screen_recording",
        ],
        "get_config" => &["system.config.read"],
        "set_config" => &["system.config.write"],

        // ── sessions ─────────────────────────────────────────────────
        "start_session" => &["session.lifecycle.start"],
        "end_session" => &["session.lifecycle.end"],

        // ── agent cursor ─────────────────────────────────────────────
        "set_agent_cursor_enabled" => &["agent_cursor.set_enabled"],
        "set_agent_cursor_motion" => &["agent_cursor.set_motion"],
        "set_agent_cursor_style" => &["agent_cursor.set_style"],
        "get_agent_cursor_state" => &["agent_cursor.state"],

        // ── recording / replay ───────────────────────────────────────
        "start_recording" => &["recording.start"],
        "stop_recording" => &["recording.stop"],
        "get_recording_state" => &["recording.state"],
        "replay_trajectory" => &["recording.replay"],

        // ── cross-platform page ──────────────────────────────────────
        "page" => &["page.action"],

        // ── driver self-service ──────────────────────────────────────
        "probe" => &["driver.probe"],

        // ── unsupported_platform stub & anything else ────────────────
        _ => &[],
    };
    caps.iter().map(|s| (*s).to_owned()).collect()
}

/// A callable tool handler. Object-safe — uses `Box<dyn Tool>`.
#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> &ToolDef;

    /// Validate everything that must be known before a targeted action may
    /// have observable dispatch side effects (for example, fronting its app).
    ///
    /// The registry calls this at the embedded action choke point immediately
    /// before the front hook and `invoke`. The default validates the advertised
    /// input-schema subset used by driver tools; tools with semantic delivery
    /// constraints (such as macOS foreground-only drag) extend it and return a
    /// structured rejection here. Implementations must be side-effect free.
    fn dispatch_preflight(&self, args: &Value) -> Result<(), ToolResult> {
        validate_dispatch_args(self.def(), args)
    }

    async fn invoke(&self, args: Value) -> ToolResult;
}

/// Validate the JSON-Schema vocabulary used by driver tool definitions at the
/// embedded dispatch boundary. This intentionally covers only the vocabulary
/// emitted by our hand-written schemas: object properties, required fields,
/// primitive types, array item types/cardinality, enums, and numeric bounds.
///
/// MCP clients normally validate this schema themselves, but the driver cannot
/// rely on that for focus safety: direct CLI/daemon callers can reach the
/// registry with malformed JSON. Internal daemon metadata keys (`_...`) are
/// ignored because they are injected after client-side schema validation.
/// So is `session` on a tool that doesn't take one: agents are told to pass
/// their session on every call, and a tool that has no use for it ignores it.
pub fn validate_dispatch_args(def: &ToolDef, args: &Value) -> Result<(), ToolResult> {
    let Some(object) = args.as_object() else {
        return Err(dispatch_validation_error(
            &def.name,
            "arguments must be an object",
        ));
    };
    let schema = &def.input_schema;
    let properties = schema.get("properties").and_then(Value::as_object);

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for field in required.iter().filter_map(Value::as_str) {
            if !object.get(field).is_some_and(|value| !value.is_null()) {
                return Err(dispatch_validation_error(
                    &def.name,
                    &format!("missing required field `{field}`"),
                ));
            }
        }
    }

    if schema.get("additionalProperties").and_then(Value::as_bool) == Some(false) {
        if let Some(properties) = properties {
            if let Some(field) = object.keys().find(|field| {
                !field.starts_with('_') && *field != "session" && !properties.contains_key(*field)
            }) {
                return Err(dispatch_validation_error(
                    &def.name,
                    &format!("unknown field `{field}`"),
                ));
            }
        }
    }

    let Some(properties) = properties else {
        return Ok(());
    };
    for (field, value) in object {
        if field.starts_with('_') || value.is_null() {
            continue;
        }
        let Some(field_schema) = properties.get(field) else {
            continue;
        };
        validate_dispatch_value(&def.name, field, value, field_schema)?;
    }
    Ok(())
}

fn validate_dispatch_value(
    tool_name: &str,
    field: &str,
    value: &Value,
    schema: &Value,
) -> Result<(), ToolResult> {
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let valid = match expected {
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            _ => true,
        };
        if !valid {
            return Err(dispatch_validation_error(
                tool_name,
                &format!("field `{field}` must be {expected}"),
            ));
        }
    }

    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            return Err(dispatch_validation_error(
                tool_name,
                &format!("field `{field}` has an unsupported value"),
            ));
        }
    }

    if let Some(number) = value.as_f64() {
        if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64) {
            if number < minimum {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must be at least {minimum}"),
                ));
            }
        }
        if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64) {
            if number > maximum {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must be at most {maximum}"),
                ));
            }
        }
    }

    if let (Some(values), Some(item_schema)) = (value.as_array(), schema.get("items")) {
        for item in values {
            validate_dispatch_value(tool_name, field, item, item_schema)?;
        }
    }
    if let Some(values) = value.as_array() {
        if let Some(min_items) = schema.get("minItems").and_then(Value::as_u64) {
            if values.len() < min_items as usize {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must contain at least {min_items} items"),
                ));
            }
        }
        if let Some(max_items) = schema.get("maxItems").and_then(Value::as_u64) {
            if values.len() > max_items as usize {
                return Err(dispatch_validation_error(
                    tool_name,
                    &format!("field `{field}` must contain at most {max_items} items"),
                ));
            }
        }
    }
    Ok(())
}

fn dispatch_validation_error(tool_name: &str, detail: &str) -> ToolResult {
    ToolResult::error(format!("Invalid {tool_name} arguments: {detail}.")).with_structured(
        serde_json::json!({
            "code": "invalid_arguments",
            "tool": tool_name,
            "detail": detail,
        }),
    )
}

/// Thread-safe collection of all registered tools.
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn Tool>>,
    /// Ordered list of tool names for `tools/list`.
    order: Vec<String>,
    /// Shared recording session — auto-records each non-read-only tool call.
    pub recording: Arc<RecordingSession>,
    /// Optional embedded-host state file, enabled by BRIGADIER_CU_STATE_DIR.
    state_file: Option<crate::session_state::StateFile>,
    /// Platform hook for resolving a human-readable target application name.
    target_app_resolver: Option<fn(i64) -> Option<String>>,
    /// Platform hook that brings a driven target app to the foreground so the
    /// watching user sees the window being driven. Only invoked after the host
    /// explicitly opts into watchable foregrounding, from the shared action
    /// choke point, for targeted action tools.
    /// The hook is best-effort and must never fail a tool call; it owns its own
    /// front-once/dedupe state. Called with `(target_pid, session)`. May remain
    /// `None` on platforms that do not support visible foregrounding.
    target_front_hook: Option<fn(i64, Option<&str>)>,
    /// Platform target guard (see [`crate::target_policy`]). Called with the
    /// tool name, its args, and the trusted allow list before every call; an
    /// `Err` is returned to the caller and the tool does not run.
    target_policy_hook: Option<TargetPolicyHook>,
}

/// Signature of the platform target guard.
pub type TargetPolicyHook = fn(&str, &Value, &[String]) -> Result<(), ToolResult>;

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            order: Vec::new(),
            recording: Arc::new(RecordingSession::new()),
            state_file: crate::session_state::StateFile::from_env(),
            target_app_resolver: Some(crate::session_state::resolve_process_name),
            target_front_hook: None,
            target_policy_hook: None,
        }
    }

    /// Install the platform target guard. See [`crate::target_policy`].
    pub fn set_target_policy_hook(&mut self, hook: TargetPolicyHook) {
        self.target_policy_hook = Some(hook);
    }

    pub fn set_target_app_resolver(&mut self, resolver: fn(i64) -> Option<String>) {
        self.target_app_resolver = Some(resolver);
    }

    /// Install the platform hook that fronts a driven target in watchable mode.
    /// See [`ToolRegistry::target_front_hook`].
    pub fn set_target_front_hook(&mut self, hook: fn(i64, Option<&str>)) {
        self.target_front_hook = Some(hook);
    }

    /// Installs the Brigadier host's in-memory key for authenticating state files.
    pub fn set_state_authentication_key(&self, key: Vec<u8>) -> bool {
        self.state_file
            .as_ref()
            .is_some_and(|state_file| state_file.set_authentication_key(key))
    }

    #[cfg(test)]
    fn set_state_file_for_test(&mut self, state_file: crate::session_state::StateFile) {
        self.state_file = Some(state_file);
    }

    /// Best-effort explicit cleanup for long-running servers whose entry point
    /// exits the process before ordinary Rust destructors can run.
    pub fn remove_state_file(&self) {
        if let Some(state_file) = &self.state_file {
            if let Err(error) = state_file.remove() {
                eprintln!("[brigadier-cu] warning: failed to remove state file: {error}");
            }
        }
        // Clean up the embedded-mode agent-cursor feed on the same shutdown
        // paths (stdio EOF / clean exit). No-op unless the feed is enabled.
        crate::cursor_feed::remove();
    }

    /// Best-effort cleanup for one ended daemon/proxy session.
    pub fn remove_session_state_file(&self, session_id: &str) {
        if let Some(state_file) = &self.state_file {
            if let Err(error) = state_file.remove_session(session_id) {
                eprintln!(
                    "[brigadier-cu] warning: failed to remove state for session \
                     {session_id}: {error}"
                );
            }
        }
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        let name = tool.def().name.clone();
        self.order.push(name.clone());
        self.tools.insert(name, tool);
    }

    /// Register the four platform-independent recording/replay tools.
    /// Call this after all platform tools have been registered.
    pub fn register_recording_tools(&mut self) {
        let session = self.recording.clone();
        self.register(Box::new(StartRecordingTool::new(session.clone())));
        self.register(Box::new(StopRecordingTool::new(session.clone())));
        self.register(Box::new(GetRecordingStateTool::new(session)));
        self.register(Box::new(ReplayTrajectoryTool));
    }

    /// Register the platform-independent session-lifecycle tools
    /// (`start_session` / `end_session`). Call alongside
    /// `register_recording_tools` from each platform's `register_all`.
    pub fn register_session_tools(&mut self) {
        use crate::session_tools::{EndSessionTool, StartSessionTool};
        self.register(Box::new(StartSessionTool));
        self.register(Box::new(EndSessionTool));
    }

    /// Wire up the replay tool's weak self-reference.
    /// Call this once, immediately after `Arc::new(registry)`.
    pub fn init_self_weak(self: &Arc<Self>) {
        init_replay_registry(Arc::downgrade(self));
    }

    pub fn tools_list(&self) -> Value {
        let list: Vec<Value> = self
            .order
            .iter()
            .filter_map(|n| self.tools.get(n))
            .map(|t| t.def().to_list_entry())
            .collect();
        // `capability_version` is the contract version for the
        // capability tokens claimed by each tool entry. Bumped on
        // BREAKING vocabulary changes only; additive changes (new
        // tokens, new tools, new claims) keep the version. See
        // `CAPABILITY_VERSION` for the policy.
        //
        // `schema_version` is the contract version for the rest of
        // the tools/list entry shape (name/description/inputSchema/
        // annotations/capabilities). Pinned at "1" today — bumped on
        // a BREAKING change to that shape, NOT when we add a new
        // optional field (those stay backward-compatible).
        //
        // Both fields are additive: existing consumers that read only
        // `tools` keep working unchanged.
        serde_json::json!({
            "tools": list,
            "capability_version": CAPABILITY_VERSION,
            "schema_version": "1",
        })
    }

    /// Exact tools/list envelope for Codex Computer Use compatibility.
    pub fn codex_computer_use_tools_list(&self) -> Value {
        let tools: Vec<Value> = self
            .order
            .iter()
            .filter_map(|name| self.tools.get(name))
            .map(|tool| tool.def().to_codex_computer_use_list_entry())
            .collect();
        serde_json::json!({"tools": tools})
    }

    /// Iterate over (name, &ToolDef) in registration order.
    pub fn iter_defs(&self) -> impl Iterator<Item = (&str, &ToolDef)> {
        self.order
            .iter()
            .filter_map(move |n| self.tools.get(n).map(|t| (n.as_str(), t.def())))
    }

    /// Get a tool's ToolDef by name, or None if unknown.
    pub fn get_def(&self, name: &str) -> Option<&ToolDef> {
        self.tools.get(name).map(|t| t.def())
    }

    /// List all tool names in registration order.
    pub fn tool_names(&self) -> impl Iterator<Item = &str> {
        self.order.iter().map(|s| s.as_str())
    }

    /// Invoke a tool by name and (if recording is enabled) write its result to disk.
    pub async fn invoke(&self, name: &str, mut args: Value) -> ToolResult {
        // The trusted allow list never reaches a tool; trusted entry points
        // put it here with `target_policy::scope_target_args`.
        let allowed_targets = crate::target_policy::take_allowed(&mut args);
        // Capture start time for recording timestamps.
        let start_ms = now_ms();

        // Deprecated alias: `type_text_chars` → `type_text`.  Swift's
        // ToolRegistry.swift keeps the same alias (with stderr warning) for
        // backwards compatibility with older clients that still emit
        // the old name.  Aliased name is intentionally not registered, so it
        // never appears in tools/list.
        let resolved_name: &str = match name {
            "type_text_chars" => {
                eprintln!("[brigadier-cu] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                "type_text"
            }
            other => other,
        };

        let Some(tool) = self.tools.get(resolved_name) else {
            return ToolResult::error(format!("Unknown tool: {name}"));
        };

        // Target guard: a refused target is side-effect free (no recording
        // turn, no fronting, no dispatch).
        if let Some(hook) = self.target_policy_hook {
            if let Err(refusal) = hook(resolved_name, &args, &allowed_targets) {
                return refusal;
            }
        }

        // Reserve and capture the turn before dispatch so recorded evidence
        // shows the application immediately before the action changed it.
        let should_record = !tool.def().read_only
            && !matches!(
                resolved_name,
                "start_recording" | "stop_recording" | "get_recording_state" | "replay_trajectory"
            );
        let pending_turn = should_record
            .then(|| self.recording.begin_turn(resolved_name, &args, start_ms))
            .flatten();

        // A rejected action must be side-effect free. In particular, explicit
        // watchable mode must not front a target until schema + tool-specific
        // delivery preflight has accepted the call. This is intentionally the
        // final boundary before invoke so accepted actions remain visible.
        if let Err(error) = preflight_and_front_target(
            tool.as_ref(),
            resolved_name,
            &args,
            crate::watchable_front_mode(),
            self.target_front_hook,
        ) {
            if let Some(pending_turn) = pending_turn {
                let result_text = error
                    .content
                    .iter()
                    .find_map(|content| match content {
                        Content::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .unwrap_or("");
                self.recording.finish_turn(pending_turn, result_text);
            }
            return error;
        }

        let result = tool.invoke(args.clone()).await;

        // Embedded-host process state is action-scoped: update it only after a
        // successful call to one of the target-driving tools, and only when the
        // call names a target pid/window. State I/O is observability-only and
        // must never turn a successful tool action into an error.
        const STATE_ACTION_TOOLS: &[&str] = &[
            "click",
            "double_click",
            "right_click",
            "type_text",
            "press_key",
            "hotkey",
            "scroll",
            "drag",
            "set_value",
            "get_window_state",
            // Codex compatibility surface. Its tools address the target by app
            // NAME and resolve the pid internally, so the fallback below reads
            // the resolved identity from the successful result instead.
            "get_app_state",
            "perform_secondary_action",
            "select_text",
        ];
        if result.is_error != Some(true) && STATE_ACTION_TOOLS.contains(&resolved_name) {
            // Native tools carry the target pid/window in their args. The
            // Codex compatibility tools carry only an app name there; their
            // successful results report the resolved "pid"/"window_id" in
            // structuredContent. Without this fallback Codex sessions never
            // produce the authenticated state files that power the embedding
            // host's session controls (menu bar target, focus, stop).
            let structured = result.structured_content.as_ref();
            // Codex-compat results nest the resolved identity under "app"
            // (see codex_compat::state_result), so check both shapes.
            let target_pid = args
                .get("pid")
                .and_then(Value::as_i64)
                .or_else(|| structured?.get("pid")?.as_i64())
                .or_else(|| structured?.get("app")?.get("pid")?.as_i64());
            let target_window = args
                .get("window_id")
                .and_then(Value::as_u64)
                .or_else(|| structured?.get("window_id")?.as_u64());
            if let Some(state_file) = self
                .state_file
                .as_ref()
                .filter(|_| target_pid.is_some() || target_window.is_some())
            {
                let target_app = target_pid
                    .and_then(|pid| self.target_app_resolver.and_then(|resolver| resolver(pid)));
                let mut state_args = args.clone();
                if let Some(pid) = target_pid {
                    state_args["pid"] = serde_json::json!(pid);
                }
                if let Some(window_id) = target_window {
                    state_args["window_id"] = serde_json::json!(window_id);
                }
                if state_args
                    .get("window_id")
                    .and_then(Value::as_u64)
                    .is_none()
                {
                    if let (Some(pid), Some(token)) = (
                        target_pid.and_then(|pid| i32::try_from(pid).ok()),
                        state_args.get("element_token").and_then(Value::as_str),
                    ) {
                        if let Ok((window_id, _)) =
                            crate::element_token::global().resolve(pid, token)
                        {
                            state_args["window_id"] = serde_json::json!(window_id);
                        }
                    }
                }
                if let Err(error) = state_file.update(&state_args, target_app) {
                    eprintln!("[brigadier-cu] warning: failed to update state file: {error}");
                }
            }
        }
        // Use the original name for downstream code paths below so the
        // exit-code matching and recording paths keep treating the alias
        // as a distinct call site.
        let name = resolved_name;

        // Record non-read-only, non-recording tool calls. The recording-
        // control tools themselves are excluded so the recorded turn
        // stream stays the actual user-action sequence (not the meta
        // start/stop frames).
        if let Some(pending_turn) = pending_turn {
            let result_text = result
                .content
                .iter()
                .find_map(|c| {
                    if let Content::Text { text, .. } = c {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .unwrap_or("");
            self.recording.finish_turn(pending_turn, result_text);
        }

        // Experimental PiP push — only when --experimental-pip is on argv
        // (otherwise `pip_enabled()` is false and we skip the screenshot
        // entirely to avoid wasted capture work). We push for the same set
        // of action tools the recording pipeline cares about (non-read-only,
        // not the recording-control meta-tools) so the live view matches
        // what the recorder would have captured for the turn.
        if pip_hook::pip_enabled() && should_record {
            let window_id = args.opt_u64("window_id");
            let pid = args.opt_i64("pid");
            if let Some(png_bytes) = screenshot_for(window_id, pid) {
                let label = synthesize_action_label(name, &args);
                pip_hook::push_pip_frame(pip_hook::PipHookFrame {
                    png_bytes,
                    action_label: label,
                    timestamp_ms: now_ms(),
                });
            }
        }

        result
    }
}

/// Shared watchable-action boundary: validate first, then front exactly once.
/// Kept separate from [`ToolRegistry::invoke`] so focus ordering can be tested
/// without mutating the process-wide watchable-mode environment.
fn preflight_and_front_target(
    tool: &dyn Tool,
    name: &str,
    args: &Value,
    watchable_front: bool,
    hook: Option<fn(i64, Option<&str>)>,
) -> Result<(), ToolResult> {
    if !watchable_front || !action_targets_window(name, args) {
        return Ok(());
    }

    tool.dispatch_preflight(args)?;
    if let (Some(hook), Some(target_pid)) = (hook, args.get("pid").and_then(Value::as_i64)) {
        let session = args
            .get("session")
            .and_then(Value::as_str)
            .or_else(|| args.get("_session_id").and_then(Value::as_str));
        hook(target_pid, session);
    }
    Ok(())
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// The action tools that DRIVE a specific target window (as opposed to merely
/// inspecting it). In explicit watchable mode the driver fronts the target
/// before these so the watching user sees the driven window.
/// `get_window_state` is a perception tool and is fronted only when it carries
/// an explicit `action` (it does not today — so it stays a pure read, never
/// stealing focus on inspection). No mode fronts unless
/// `BRIGADIER_CU_WATCHABLE_FRONT=1`.
const FRONT_ACTION_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "type_text",
    "press_key",
    "hotkey",
    "scroll",
    "drag",
    "set_value",
];

/// Whether a call is a targeted drive action that should front its target in
/// watchable mode. `get_window_state` fronts only with an explicit non-null
/// `action` arg so ordinary perception never steals focus.
fn action_targets_window(name: &str, args: &Value) -> bool {
    if FRONT_ACTION_TOOLS.contains(&name) {
        return true;
    }
    if name == "get_window_state" {
        return args.get("action").map(|v| !v.is_null()).unwrap_or(false);
    }
    false
}

/// Build a short, human-friendly label for the PiP overlay from the
/// tool name + raw args. Kept under ~60 chars so the macOS NSTextField
/// has room without truncation at default geometry.
fn synthesize_action_label(tool_name: &str, args: &Value) -> String {
    let arg = |k: &str| -> Option<String> {
        args.get(k).map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
    };
    let summary = match tool_name {
        "click" | "double_click" | "right_click" => {
            if let Some(idx) = args.opt_u64("element_index") {
                format!("element_index={idx}")
            } else if let (Some(x), Some(y)) = (args.opt_f64("x"), args.opt_f64("y")) {
                format!("({x:.0}, {y:.0})")
            } else {
                "".into()
            }
        }
        "type_text" => {
            let text = arg("text").unwrap_or_default();
            let trimmed: String = text.chars().take(40).collect();
            if text.chars().count() > 40 {
                format!("\"{trimmed}…\"")
            } else {
                format!("\"{trimmed}\"")
            }
        }
        "press_key" | "hotkey" => arg("key").or_else(|| arg("keys")).unwrap_or_default(),
        "scroll" => format!(
            "dx={} dy={}",
            arg("dx").unwrap_or_else(|| "0".into()),
            arg("dy").unwrap_or_else(|| "0".into())
        ),
        "drag" => "drag".into(),
        "set_value" => arg("value").unwrap_or_default(),
        "launch_app" => arg("bundle_id").or_else(|| arg("name")).unwrap_or_default(),
        _ => String::new(),
    };
    if summary.is_empty() {
        tool_name.to_owned()
    } else {
        format!("{tool_name}: {summary}")
    }
}

#[cfg(test)]
mod target_policy_hook_tests {
    //! The registry runs the platform target guard before every tool.
    use super::*;
    use crate::target_policy::ALLOWED_TARGET_BUNDLE_IDS_ARG;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static DISPATCHED: AtomicUsize = AtomicUsize::new(0);
    static SEEN_ARGS: Mutex<Option<Value>> = Mutex::new(None);
    static SEEN_ALLOWED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct CountingTool(ToolDef);

    #[async_trait::async_trait]
    impl Tool for CountingTool {
        fn def(&self) -> &ToolDef {
            &self.0
        }
        async fn invoke(&self, args: Value) -> ToolResult {
            DISPATCHED.fetch_add(1, Ordering::SeqCst);
            *SEEN_ARGS.lock().unwrap() = Some(args);
            ToolResult::text("dispatched")
        }
    }

    /// pid 100 is the user's Brigadier; everything else is another app.
    fn guard(tool: &str, args: &Value, allowed: &[String]) -> Result<(), ToolResult> {
        *SEEN_ALLOWED.lock().unwrap() = allowed.to_vec();
        let app_for_pid = |pid: i64| {
            (pid == 100).then(|| crate::target_policy::TargetIdentity {
                name: "Brigadier".to_owned(),
                bundle_id: Some("ai.brigadier.app".to_owned()),
                pid: Some(pid),
            })
        };
        let pid_for_window = |_window: u64| -> Option<i64> { None };
        crate::target_policy::enforce(
            tool,
            args,
            allowed,
            &crate::target_policy::TargetResolver {
                app_for_pid: &app_for_pid,
                pid_for_window: &pid_for_window,
            },
        )
    }

    #[tokio::test]
    async fn the_registry_refuses_a_brigadier_target_before_dispatch_and_strips_the_scope() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CountingTool(ToolDef {
            name: "click".to_owned(),
            description: "test".to_owned(),
            input_schema: json!({"type": "object", "properties": {"pid": {"type": "integer"}}}),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        })));
        registry.set_target_policy_hook(guard);

        let refused = registry.invoke("click", json!({"pid": 100})).await;
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(
            refused.structured_content.unwrap()["code"],
            "target_not_allowed"
        );
        assert_eq!(
            DISPATCHED.load(Ordering::SeqCst),
            0,
            "a refused call must not run"
        );

        let ok = registry
            .invoke(
                "click",
                json!({"pid": 7, ALLOWED_TARGET_BUNDLE_IDS_ARG: ["com.example.app"]}),
            )
            .await;
        assert_ne!(ok.is_error, Some(true));
        assert_eq!(DISPATCHED.load(Ordering::SeqCst), 1);
        assert_eq!(
            *SEEN_ALLOWED.lock().unwrap(),
            vec!["com.example.app".to_owned()]
        );
        let seen = SEEN_ARGS.lock().unwrap().clone().unwrap();
        assert!(
            seen.get(ALLOWED_TARGET_BUNDLE_IDS_ARG).is_none(),
            "the private scope argument must not reach the tool"
        );
    }
}

#[cfg(test)]
mod front_gate_tests {
    //! Which calls trigger explicit watchable fronting at the choke point.
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static FRONT_CALLS: AtomicUsize = AtomicUsize::new(0);

    fn count_front(_pid: i64, _session: Option<&str>) {
        FRONT_CALLS.fetch_add(1, Ordering::SeqCst);
    }

    struct PreflightTool {
        def: ToolDef,
        foreground_drag_only: bool,
    }

    #[async_trait::async_trait]
    impl Tool for PreflightTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }

        fn dispatch_preflight(&self, args: &Value) -> Result<(), ToolResult> {
            validate_dispatch_args(self.def(), args)?;
            if self.foreground_drag_only
                && args.get("delivery_mode").and_then(Value::as_str) != Some("foreground")
            {
                return Err(ToolResult::error("background drag rejected"));
            }
            Ok(())
        }

        async fn invoke(&self, _args: Value) -> ToolResult {
            ToolResult::text("dispatched")
        }
    }

    fn action_tool(name: &str, required: &[&str], foreground_drag_only: bool) -> PreflightTool {
        PreflightTool {
            def: ToolDef {
                name: name.to_owned(),
                description: "test action".to_owned(),
                input_schema: json!({
                    "type": "object",
                    "required": required,
                    "properties": {
                        "pid": { "type": "integer" },
                        "window_id": { "type": "integer" },
                        "from_x": { "type": "number" },
                        "from_y": { "type": "number" },
                        "to_x": { "type": "number" },
                        "to_y": { "type": "number" },
                        "x": { "type": "number" },
                        "y": { "type": "number" },
                        "delivery_mode": {
                            "type": "string",
                            "enum": ["background", "foreground"]
                        },
                        "keys": {
                            "type": "array",
                            "items": { "type": "string" },
                            "minItems": 2
                        },
                    },
                    "additionalProperties": false
                }),
                read_only: false,
                destructive: true,
                idempotent: false,
                open_world: true,
            },
            foreground_drag_only,
        }
    }

    #[test]
    fn drive_action_tools_front() {
        for name in [
            "click",
            "double_click",
            "right_click",
            "type_text",
            "press_key",
            "hotkey",
            "scroll",
            "drag",
            "set_value",
        ] {
            assert!(
                action_targets_window(name, &json!({"pid": 42})),
                "{name} should front"
            );
        }
    }

    #[test]
    fn perception_and_unknown_tools_do_not_front() {
        // get_window_state is perception: no fronting unless it carries an
        // explicit action (it does not today).
        assert!(!action_targets_window(
            "get_window_state",
            &json!({"pid": 42})
        ));
        assert!(!action_targets_window(
            "get_window_state",
            &json!({"pid": 42, "action": null})
        ));
        assert!(action_targets_window(
            "get_window_state",
            &json!({"pid": 42, "action": "press"})
        ));
        // Read-only / non-drive tools never front.
        assert!(!action_targets_window("list_windows", &json!({})));
        assert!(!action_targets_window(
            "launch_app",
            &json!({"bundle_id": "x"})
        ));
    }

    #[test]
    fn rejected_actions_never_front_and_valid_action_fronts_once() {
        FRONT_CALLS.store(0, Ordering::SeqCst);
        let drag = action_tool(
            "drag",
            &["pid", "window_id", "from_x", "from_y", "to_x", "to_y"],
            true,
        );
        let click = action_tool("click", &["pid", "x", "y"], false);

        for rejected in [
            json!({
                "pid": 42, "window_id": 9,
                "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4,
                "delivery_mode": "background"
            }),
            json!({
                "pid": 42,
                "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4,
                "delivery_mode": "foreground"
            }),
            json!({
                "pid": 42, "window_id": "not-a-window",
                "from_x": 1, "from_y": 2, "to_x": 3, "to_y": 4,
                "delivery_mode": "foreground"
            }),
        ] {
            assert!(
                preflight_and_front_target(&drag, "drag", &rejected, true, Some(count_front))
                    .is_err()
            );
        }
        assert!(preflight_and_front_target(
            &click,
            "click",
            &json!({"pid": "not-a-pid", "x": 1, "y": 2}),
            true,
            Some(count_front),
        )
        .is_err());
        assert_eq!(FRONT_CALLS.load(Ordering::SeqCst), 0);

        preflight_and_front_target(
            &click,
            "click",
            &json!({"pid": 42, "x": 1, "y": 2}),
            false,
            Some(count_front),
        )
        .expect("background delivery without host opt-in should pass without fronting");
        assert_eq!(FRONT_CALLS.load(Ordering::SeqCst), 0);

        preflight_and_front_target(
            &click,
            "click",
            &json!({"pid": 42, "x": 1, "y": 2}),
            true,
            Some(count_front),
        )
        .expect("valid click should pass dispatch preflight");
        assert_eq!(FRONT_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn malformed_hotkey_below_min_items_never_fronts() {
        static HOTKEY_FRONT_CALLS: AtomicUsize = AtomicUsize::new(0);
        fn count_hotkey_front(_pid: i64, _session: Option<&str>) {
            HOTKEY_FRONT_CALLS.fetch_add(1, Ordering::SeqCst);
        }

        HOTKEY_FRONT_CALLS.store(0, Ordering::SeqCst);
        let hotkey = action_tool("hotkey", &["pid", "keys"], false);
        let result = preflight_and_front_target(
            &hotkey,
            "hotkey",
            &json!({"pid": 42, "keys": ["cmd"]}),
            true,
            Some(count_hotkey_front),
        );

        assert!(result.is_err());
        assert_eq!(HOTKEY_FRONT_CALLS.load(Ordering::SeqCst), 0);
    }
}

#[cfg(test)]
mod capability_tests {
    //! Unit tests for the per-tool `capabilities` array and the
    //! top-level `capability_version` exposed in `tools/list`.
    //! These belong in cu-core because they cover the shape
    //! of the registry response — no platform code involved.
    use super::*;

    struct SuccessfulTargetTool {
        def: ToolDef,
    }

    #[async_trait::async_trait]
    impl Tool for SuccessfulTargetTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }

        async fn invoke(&self, _args: Value) -> ToolResult {
            ToolResult::text("ok")
        }
    }

    #[tokio::test]
    async fn successful_click_actions_update_embedded_process_state() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.set_state_file_for_test(crate::session_state::StateFile::new(
            dir.path().to_owned(),
            5151,
        ));
        registry.set_target_app_resolver(|pid| Some(format!("Target-{pid}")));
        for name in ["click", "double_click", "right_click"] {
            registry.register(Box::new(SuccessfulTargetTool {
                def: dummy_def(name),
            }));
        }

        for (index, name) in ["click", "double_click", "right_click"]
            .into_iter()
            .enumerate()
        {
            let pid = 42 + index as i64;
            let window_id = 99 + index as u64;
            let session = format!("embedded-{name}");
            let target_app = format!("Target-{pid}");
            let result = registry
                .invoke(
                    name,
                    serde_json::json!({
                        "session": session,
                        "pid": pid,
                        "window_id": window_id,
                    }),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
            let path = registry
                .state_file
                .as_ref()
                .unwrap()
                .path_for_session(Some(&session));
            let state: crate::session_state::DriverProcessState =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(state.session.as_deref(), Some(session.as_str()));
            assert_eq!(state.target_app.as_deref(), Some(target_app.as_str()));
            assert_eq!(state.target_pid, Some(pid));
            assert_eq!(state.target_window_id, Some(window_id));
        }
    }

    struct StructuredTargetTool {
        def: ToolDef,
        pid: i64,
        window_id: u64,
    }

    #[async_trait::async_trait]
    impl Tool for StructuredTargetTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }

        async fn invoke(&self, _args: Value) -> ToolResult {
            ToolResult {
                content: vec![Content::text("ok")],
                is_error: None,
                structured_content: Some(serde_json::json!({
                    "pid": self.pid,
                    "window_id": self.window_id,
                })),
            }
        }
    }

    /// Codex-surface tools address the target by app NAME; the resolved
    /// pid/window arrive only in the successful result's structuredContent.
    /// State must still be written, or Codex sessions never surface in the
    /// embedding host's session controls.
    #[tokio::test]
    async fn app_named_actions_update_state_from_structured_result() {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.set_state_file_for_test(crate::session_state::StateFile::new(
            dir.path().to_owned(),
            6161,
        ));
        registry.set_target_app_resolver(|pid| Some(format!("Target-{pid}")));
        for name in ["click", "get_app_state", "perform_secondary_action"] {
            registry.register(Box::new(StructuredTargetTool {
                def: dummy_def(name),
                pid: 4242,
                window_id: 777,
            }));
        }

        for name in ["click", "get_app_state", "perform_secondary_action"] {
            let session = format!("codex-{name}");
            let result = registry
                .invoke(
                    name,
                    serde_json::json!({
                        "session": session,
                        "app": "Calculator",
                    }),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
            let path = registry
                .state_file
                .as_ref()
                .unwrap()
                .path_for_session(Some(&session));
            let state: crate::session_state::DriverProcessState =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(state.session.as_deref(), Some(session.as_str()));
            assert_eq!(state.target_app.as_deref(), Some("Target-4242"));
            assert_eq!(state.target_pid, Some(4242));
            assert_eq!(state.target_window_id, Some(777));
        }
    }

    /// The real codex-compat results nest the resolved identity under "app"
    /// (codex_compat::state_result) and expose no top-level pid/window_id.
    /// This is the exact shape of the Calculator regression: actions ran,
    /// cursor files appeared, but no session state was ever written.
    #[tokio::test]
    async fn app_named_actions_update_state_from_nested_app_identity() {
        struct NestedAppTool {
            def: ToolDef,
        }

        #[async_trait::async_trait]
        impl Tool for NestedAppTool {
            fn def(&self) -> &ToolDef {
                &self.def
            }

            async fn invoke(&self, _args: Value) -> ToolResult {
                ToolResult {
                    content: vec![Content::text("ok")],
                    is_error: None,
                    structured_content: Some(serde_json::json!({
                        "app": {
                            "name": "Calculator",
                            "bundle_id": "com.apple.calculator",
                            "pid": 40930,
                        },
                        "window_title": "Calculator",
                    })),
                }
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let mut registry = ToolRegistry::new();
        registry.set_state_file_for_test(crate::session_state::StateFile::new(
            dir.path().to_owned(),
            6262,
        ));
        registry.set_target_app_resolver(|pid| Some(format!("Target-{pid}")));
        for name in ["click", "get_app_state", "type_text", "select_text"] {
            registry.register(Box::new(NestedAppTool {
                def: dummy_def(name),
            }));
        }

        for name in ["click", "get_app_state", "type_text", "select_text"] {
            let session = format!("codex-nested-{name}");
            let result = registry
                .invoke(
                    name,
                    serde_json::json!({
                        "session": session,
                        "app": "Calculator",
                        "element_index": "6",
                    }),
                )
                .await;
            assert_ne!(result.is_error, Some(true));
            let path = registry
                .state_file
                .as_ref()
                .unwrap()
                .path_for_session(Some(&session));
            let state: crate::session_state::DriverProcessState =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(state.session.as_deref(), Some(session.as_str()));
            assert_eq!(state.target_app.as_deref(), Some("Target-40930"));
            assert_eq!(state.target_pid, Some(40930));
            assert_eq!(state.target_window_id, None);
        }
    }

    #[tokio::test]
    async fn malformed_state_dir_never_fails_a_successful_action() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("plain-file");
        std::fs::write(&not_a_dir, b"occupied").unwrap();
        let mut registry = ToolRegistry::new();
        registry.set_state_file_for_test(crate::session_state::StateFile::new(not_a_dir, 5252));
        registry.register(Box::new(SuccessfulTargetTool {
            def: dummy_def("click"),
        }));

        let result = registry
            .invoke("click", serde_json::json!({"pid": 42}))
            .await;
        assert_ne!(result.is_error, Some(true));
    }

    /// Tools whose `default_capabilities_for` mapping must NOT be
    /// empty. Mirrors the documented vocabulary above. Lives here
    /// rather than in an integration test because adding a new tool
    /// without a capability claim should fail at unit-test time, not
    /// only when someone runs the platform-specific integration
    /// suite.
    const TOOLS_REQUIRING_CAPABILITIES: &[&str] = &[
        // pointer
        "click",
        "double_click",
        "right_click",
        "drag",
        "scroll",
        "move_cursor",
        "mouse_button_down",
        "mouse_button_up",
        "mouse_drag",
        "parallel_mouse_drag",
        // keyboard
        "type_text",
        "type_text_chars",
        "press_key",
        "hotkey",
        "set_value",
        // screen
        "zoom",
        "get_screen_size",
        "get_desktop_state",
        "get_cursor_position",
        // accessibility
        "get_accessibility_tree",
        "get_window_state",
        // app / window
        "launch_app",
        "list_apps",
        "kill_app",
        "list_windows",
        "bring_to_front",
        "debug_window_info",
        // permissions / config
        "check_permissions",
        "get_config",
        "set_config",
        // sessions
        "start_session",
        "end_session",
        // agent cursor
        "set_agent_cursor_enabled",
        "set_agent_cursor_motion",
        "set_agent_cursor_style",
        "get_agent_cursor_state",
        // recording / replay
        "start_recording",
        "stop_recording",
        "get_recording_state",
        "replay_trajectory",
        // misc
        "page",
        "probe",
    ];

    /// All capability tokens in the canonical vocabulary. Any token
    /// produced by `default_capabilities_for` MUST be in this set —
    /// catches typos and accidental ad-hoc extensions that would
    /// silently break consumers that match by token.
    const CANONICAL_VOCABULARY: &[&str] = &[
        // pointer
        "input.pointer.click",
        "input.pointer.click.left",
        "input.pointer.click.right",
        "input.pointer.click.double",
        "input.pointer.drag",
        "input.pointer.scroll",
        "input.pointer.move",
        "input.pointer.button",
        // keyboard
        "input.keyboard.type",
        "input.keyboard.type.terminal_safe",
        "input.keyboard.hotkey",
        "input.keyboard.press",
        // screen
        "screen.capture",
        "screen.capture.window",
        "screen.capture.region",
        "screen.dimensions",
        "screen.cursor.position",
        // accessibility
        "accessibility.tree",
        "accessibility.tree.structured",
        "accessibility.tree.bounded",
        "accessibility.window_state",
        // Surface 6 — claimed by tools that accept the opaque
        // `element_token` arg + get_window_state which emits them.
        "accessibility.element_tokens",
        // app / window
        "app.launch",
        "app.list",
        "app.kill",
        "window.list",
        "window.activate",
        "window.debug_info",
        // permissions
        "system.permissions.tcc",
        "system.permissions.tcc.accessibility",
        "system.permissions.tcc.screen_recording",
        // config
        "system.config.read",
        "system.config.write",
        // sessions
        "session.lifecycle.start",
        "session.lifecycle.end",
        // agent cursor
        "agent_cursor.move",
        "agent_cursor.set_enabled",
        "agent_cursor.set_motion",
        "agent_cursor.set_style",
        "agent_cursor.state",
        // recording
        "recording.start",
        "recording.stop",
        "recording.state",
        "recording.replay",
        "recording.install_dependency",
        // page
        "page.action",
        // driver self
        "driver.update_check",
        "driver.probe",
    ];

    #[test]
    fn every_known_tool_has_at_least_one_capability() {
        for name in TOOLS_REQUIRING_CAPABILITIES {
            let caps = default_capabilities_for(name);
            assert!(
                !caps.is_empty(),
                "tool {name:?} must claim at least one capability — \
                 add it to default_capabilities_for() or remove it \
                 from TOOLS_REQUIRING_CAPABILITIES"
            );
        }
    }

    #[test]
    fn every_claimed_capability_is_in_the_canonical_vocabulary() {
        let vocab: std::collections::HashSet<&str> = CANONICAL_VOCABULARY.iter().copied().collect();
        for name in TOOLS_REQUIRING_CAPABILITIES {
            for cap in default_capabilities_for(name) {
                assert!(
                    vocab.contains(cap.as_str()),
                    "tool {name:?} claims unknown capability {cap:?} — \
                     either add {cap:?} to CANONICAL_VOCABULARY or fix \
                     the typo in default_capabilities_for()"
                );
            }
        }
    }

    #[test]
    fn a_session_is_accepted_by_a_tool_that_takes_none_but_other_strays_are_not() {
        let def = ToolDef {
            name: "list_apps".into(),
            description: String::new(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false,
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        };
        assert!(validate_dispatch_args(&def, &serde_json::json!({"session": "run-1"})).is_ok());
        assert!(validate_dispatch_args(&def, &serde_json::json!({"windw": 1})).is_err());
    }

    #[test]
    fn capability_version_is_string_one() {
        // Bumping this constant in a non-breaking PR is an error —
        // the version is the contract version, not the build version.
        // Pinned to "1" until we ship a BREAKING vocabulary change.
        assert_eq!(CAPABILITY_VERSION, "1");
    }

    #[test]
    fn unknown_tools_get_empty_capabilities() {
        // Tools without a mapping (typically internal/stub tools like
        // `unsupported_platform`) return `[]`. Consumers fall back to
        // name-matching for those, which is fine — they were never
        // load-bearing for capability routing.
        assert!(default_capabilities_for("unsupported_platform").is_empty());
        assert!(default_capabilities_for("totally_made_up_tool").is_empty());
    }

    fn dummy_def(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("{name} (test)"),
            input_schema: serde_json::json!({"type":"object"}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        }
    }

    /// Surface 6: every tool that accepts the opaque `element_token`
    /// arg must claim the `accessibility.element_tokens` capability so
    /// Agent clients can branch on the capability
    /// token rather than coupling to tool names. Same set as the
    /// per-platform schema additions in this PR — keep the two lists
    /// in sync when a new element-targeting tool ships.
    #[test]
    fn every_token_accepting_tool_claims_element_tokens_capability() {
        const TOKEN_TOOLS: &[&str] = &[
            "click",
            "double_click",
            "right_click",
            "scroll",
            "type_text",
            "type_text_chars",
            "press_key",
            "set_value",
            // get_window_state emits the tokens — same capability
            // claim, from the other side of the contract.
            "get_window_state",
        ];
        for name in TOKEN_TOOLS {
            let caps = default_capabilities_for(name);
            assert!(
                caps.iter().any(|c| c == "accessibility.element_tokens"),
                "tool {name:?} accepts element_token but is missing the \
                 accessibility.element_tokens capability claim — add it \
                 in default_capabilities_for()"
            );
        }
    }

    #[test]
    fn to_list_entry_includes_capabilities_array_for_a_known_tool() {
        let def = dummy_def("click");
        let entry = def.to_list_entry();
        let caps = entry
            .get("capabilities")
            .and_then(|v| v.as_array())
            .expect("capabilities must be an array");
        assert!(
            !caps.is_empty(),
            "click must claim at least one capability via default_capabilities_for"
        );
        // Specifically: click claims the `input.pointer.click.left`
        // family — that's the contract clients are
        // expected to dispatch on once this surface is wired up.
        let cap_strs: Vec<&str> = caps.iter().filter_map(|v| v.as_str()).collect();
        assert!(
            cap_strs.contains(&"input.pointer.click"),
            "click missing input.pointer.click: {cap_strs:?}"
        );
        assert!(
            cap_strs.contains(&"input.pointer.click.left"),
            "click missing input.pointer.click.left: {cap_strs:?}"
        );
    }

    #[test]
    fn to_list_entry_includes_empty_capabilities_array_for_unknown_tool() {
        // Even when no capabilities are claimed, the field is still
        // present — consumers can rely on the key existing.
        let def = dummy_def("totally_made_up_tool");
        let entry = def.to_list_entry();
        let caps = entry
            .get("capabilities")
            .and_then(|v| v.as_array())
            .expect("capabilities must be present even if empty");
        assert!(caps.is_empty());
    }

    #[test]
    fn to_list_entry_preserves_existing_fields() {
        // Regression guard for the additive-only contract: every
        // pre-existing key in the response must still be there.
        let def = ToolDef {
            name: "click".into(),
            description: "Click an element.".into(),
            input_schema: serde_json::json!({"type":"object","properties":{}}),
            read_only: false,
            destructive: true,
            idempotent: false,
            open_world: true,
        };
        let entry = def.to_list_entry();
        // Keys old consumers (older drivers)
        // already read — must still be present.
        assert_eq!(entry["name"], "click");
        assert_eq!(entry["description"], "Click an element.");
        assert!(entry["inputSchema"].is_object());
        assert_eq!(entry["annotations"]["readOnlyHint"], false);
        assert_eq!(entry["annotations"]["destructiveHint"], true);
        assert_eq!(entry["annotations"]["idempotentHint"], false);
        assert_eq!(entry["annotations"]["openWorldHint"], true);
        // New key — the whole point of this PR.
        assert!(entry["capabilities"].is_array());
    }

    #[test]
    fn type_text_claims_terminal_safe_capability() {
        // The terminal-emulator fallback shipped per platform must be
        // discoverable as a capability so consumers can pick `type_text`
        // confidently over `type_text_chars` (whose Linux implementation
        // is the bare per-char XSendEvent path, with no terminal
        // short-circuit). Freezing the token name here makes a
        // future-PR rename a hard test failure.
        let caps = default_capabilities_for("type_text");
        let cap_strs: Vec<&str> = caps.iter().map(String::as_str).collect();
        assert!(
            cap_strs.contains(&"input.keyboard.type"),
            "type_text must keep the base capability: {cap_strs:?}"
        );
        assert!(
            cap_strs.contains(&"input.keyboard.type.terminal_safe"),
            "type_text must claim terminal_safe (PR additive surface): {cap_strs:?}"
        );
    }

    #[test]
    fn type_text_chars_does_not_claim_terminal_safe() {
        // The contract is intentionally narrower: type_text_chars on
        // Linux uses a per-character XSendEvent path that does not
        // route past the AT-SPI/value channel on terminals. Tightening
        // this gate prevents a future drive-by edit from over-claiming.
        let caps = default_capabilities_for("type_text_chars");
        let cap_strs: Vec<&str> = caps.iter().map(String::as_str).collect();
        assert!(
            !cap_strs.contains(&"input.keyboard.type.terminal_safe"),
            "type_text_chars must NOT claim terminal_safe: {cap_strs:?}"
        );
    }

    #[test]
    fn tools_list_top_level_envelope_has_capability_and_schema_versions() {
        // An empty registry still emits both version fields so
        // consumers don't have to special-case the bootstrap window
        // between server start and first tool register.
        let reg = ToolRegistry::new();
        let v = reg.tools_list();
        assert_eq!(v["capability_version"], "1");
        assert_eq!(v["schema_version"], "1");
        assert!(v["tools"].is_array(), "tools array must still be present");
        assert_eq!(v["tools"].as_array().unwrap().len(), 0);
    }
}
