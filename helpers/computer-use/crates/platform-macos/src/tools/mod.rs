//! MCP tool implementations for macOS.

mod bring_to_front;
mod click;
mod double_click;
mod drag;
mod drag_cursor;
mod get_window_state;
mod hotkey;
mod kill_app;
mod launch_app;
mod list_apps;
mod list_windows;
mod press_key;
mod right_click;
mod scroll;
mod set_value;
mod type_text;
// `screenshot` / `screenshot_compat` modules removed in PR #1692 —
// `get_window_state` capture_mode:"vision" is the canonical screenshot
// path. The capture functions they wrapped (ScreenCaptureKit, CGWindow,
// etc.) live elsewhere under BrigadierCuCore::Capture and are reached
// through GetWindowStateTool.
mod check_permissions;
mod cursor_tools;
mod get_accessibility_tree;
mod get_config;
mod get_cursor_position;
mod get_desktop_state;
pub(crate) mod get_screen_size;
mod health_report;
mod move_cursor;
mod page;
mod set_config;
mod type_text_chars;
mod zoom;

/// Perform the prompt-capable live ScreenCaptureKit readiness check for an
/// embedding host's explicit onboarding flow.
///
/// This is deliberately not registered as an MCP tool. Only the daemon's
/// host-authenticated control request can reach it, so agents cannot raise
/// Tahoe's private-window-picker consent outside the host UI.
///
/// `Err` means macOS did not answer the probe in time (typically a pending
/// system dialog); that is unknown readiness, not a missing grant.
pub fn verify_screen_capture_ready() -> Result<bool, crate::capture_deadline::CaptureTimeout> {
    check_permissions::screen_recording_capturable()
}

use cu_core::tool::ToolRegistry;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{ax::cache::ElementCache, cursor::state::CursorRegistry};

/// Per-process zoom context — stores the padded crop origin and resize scale
/// from the most recent `zoom` call, so `click(from_zoom=true)` can translate
/// zoom-image pixel coordinates back to full-window coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ZoomContext {
    /// Padded crop X origin in full-window pixel space.
    pub origin_x: f64,
    /// Padded crop Y origin in full-window pixel space.
    pub origin_y: f64,
    /// Inverse resize scale: `cw / out_w` (1.0 = no downscale).
    pub scale_inv: f64,
}

impl ZoomContext {
    /// Translate a zoom-image coordinate `(px, py)` to full-window pixel coordinates.
    pub fn zoom_to_window(&self, px: f64, py: f64) -> (f64, f64) {
        (
            self.origin_x + px * self.scale_inv,
            self.origin_y + py * self.scale_inv,
        )
    }
}

/// Input delivery modality — the agent-selected rung of the best-effort-background
/// ladder, passed per call (never a stored/config setting).
///
/// - `Background` (default): post synthetic input to the pid without fronting.
/// - `Foreground`: briefly front the target window, act, then restore the prior
///   frontmost (see [`crate::input::skylight::with_foreground_assist`]). The
///   agent's vision-driven last resort — and the only way `click` reaches a
///   foreground rung. Orthogonal to addressing (`element_index` vs `x/y`, which
///   selects AX vs pixel).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DeliveryMode {
    #[default]
    Background,
    Foreground,
}

impl DeliveryMode {
    /// Parse the per-call `delivery_mode` argument. Anything other than an
    /// explicit case-insensitive `"foreground"` resolves to `Background` — the
    /// correct default, so an omitted/garbage value never silently fronts.
    pub fn parse(arg: Option<&str>) -> Self {
        match arg {
            Some(s) if s.eq_ignore_ascii_case("foreground") => Self::Foreground,
            _ => Self::Background,
        }
    }

    pub fn is_foreground(self) -> bool {
        matches!(self, Self::Foreground)
    }
}

/// Side-effect-free addressing result used by targeted-action preflights.
/// Resolving an element token reads only the snapshot registry; it does not
/// touch Accessibility, cursor, focus, or input state.
pub(crate) struct ActionAddress {
    pub element: bool,
    pub window_id: Option<u32>,
    pub has_xy: bool,
    pub partial_xy: bool,
}

pub(crate) fn preflight_action_address(
    args: &serde_json::Value,
    tool_name: &str,
) -> Result<ActionAddress, cu_core::protocol::ToolResult> {
    use cu_core::tool_args::ArgsExt;

    let pid = args.require_i32("pid")?;
    let window_id_arg = args
        .opt_u64("window_id")
        .and_then(|value| u32::try_from(value).ok());
    let resolved = cu_core::element_token::resolve_element_args(
        pid,
        args.opt_u64("element_index").map(|value| value as usize),
        args.opt_str("element_token").as_deref(),
        window_id_arg,
        tool_name,
    )?;
    let (element, window_id) = match resolved {
        cu_core::element_token::ResolvedElement::None => (false, window_id_arg),
        cu_core::element_token::ResolvedElement::Element { window_id, .. } => (true, window_id),
    };
    let has_x = args.get("x").is_some_and(serde_json::Value::is_number);
    let has_y = args.get("y").is_some_and(serde_json::Value::is_number);
    Ok(ActionAddress {
        element,
        window_id,
        has_xy: has_x && has_y,
        partial_xy: has_x != has_y,
    })
}

// ── Explicit "watchable" target fronting ─────────────────────────────────────
//
// A host may explicitly request visible computer use, where the app being
// driven stays frontmost. Embedding alone preserves the normal background
// delivery contract. The shared action choke point (`ToolRegistry::invoke` in
// cu-core) calls [`front_target_if_watchable`] before each targeted
// drive action; this module owns the activation + front-once/dedupe state so we
// never flicker focus by re-activating an already-frontmost target on every
// single action.

/// The (session, target_pid) most recently fronted by [`front_target_if_watchable`].
/// One driver process ⇒ one dedupe cell. `None` until the first front.
static LAST_FRONTED: std::sync::Mutex<Option<(Option<String>, i32)>> = std::sync::Mutex::new(None);

/// Outcome of the front-once/dedupe decision. Pure and unit-testable: given the
/// target, the current frontmost pid, and what we fronted last, decide whether
/// to issue an activation now and what to remember as last-fronted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FrontDecision {
    /// Issue an `activate` for the target now.
    pub front: bool,
    /// Value to store as the new last-fronted `(session, pid)`.
    pub new_last: Option<(Option<String>, i32)>,
}

/// Decide whether to bring `target_pid` to the front.
///
/// Rules (pure — no OS calls):
/// - If the target is ALREADY frontmost → skip (never flicker a frontmost app),
///   but still record it as last-fronted so later unchanged actions dedupe.
/// - Else if we already fronted this exact `(session, target_pid)` on the
///   previous action AND no *different* app has since grabbed the foreground
///   (frontmost is the target or simply unknown) → skip: the earlier activation
///   is still settling, re-issuing it just churns focus.
/// - Else → front it so the watching user sees the driven window.
pub(crate) fn decide_front(
    target_pid: i32,
    session: Option<&str>,
    frontmost_pid: Option<i32>,
    last_fronted: Option<&(Option<String>, i32)>,
) -> FrontDecision {
    let key = (session.map(str::to_owned), target_pid);
    let already_frontmost = frontmost_pid == Some(target_pid);
    let unchanged = last_fronted == Some(&key);
    // A *different* app currently holds the foreground (real focus steal), as
    // opposed to `None`/target (activation still settling).
    let stolen_by_other = matches!(frontmost_pid, Some(p) if p != target_pid);

    let front = if already_frontmost {
        false
    } else if unchanged && !stolen_by_other {
        false
    } else {
        true
    };
    FrontDecision {
        front,
        new_last: Some(key),
    }
}

/// Bring a driven target app to the foreground in explicit watchable mode so
/// the watching user sees the window being driven. Best-effort and deduped via
/// [`decide_front`]: a rejected/failed activation is only warned about, never an
/// error — this is called from the shared action choke point and must never
/// fail a tool call. No-op without host opt-in (the core caller already gates
/// on `watchable_front_mode()`, and this rechecks so a stray direct call stays
/// safe).
pub fn front_target_if_watchable(target_pid: i64, session: Option<&str>) {
    if !cu_core::watchable_front_mode() {
        return;
    }
    let Ok(pid) = i32::try_from(target_pid) else {
        return;
    };
    let frontmost = crate::apps::frontmost_pid();
    let mut last = LAST_FRONTED.lock().unwrap();
    let decision = decide_front(pid, session, frontmost, last.as_ref());
    if decision.front {
        let activated = crate::apps::activate_pid_all_windows(pid);
        if !activated {
            tracing::warn!(
                target: "platform_macos::tools::watchable_front",
                target_pid = pid,
                "watchable front: activation returned NO (app hidden, \
                 terminating, or denied) — proceeding with background drive"
            );
        }
    }
    *last = decision.new_last;
}

/// Fail closed when a background pixel event would be hit-tested onto a
/// different visible window. Cursor-overlay windows are filtered inside the
/// WindowServer hit-test because they share this process's pid.
pub(crate) fn pixel_obstruction_error(
    target_pid: i32,
    target_window_id: u32,
    screen_x: f64,
    screen_y: f64,
    point_name: Option<&str>,
) -> Option<cu_core::protocol::ToolResult> {
    let occluder =
        crate::windows::pixel_obstruction(target_pid, target_window_id, screen_x, screen_y)?;
    let app = if occluder.app_name.trim().is_empty() {
        format!("pid {}", occluder.pid)
    } else {
        occluder.app_name.clone()
    };
    let title_suffix = if occluder.title.trim().is_empty() {
        String::new()
    } else {
        format!(" (\"{}\")", occluder.title)
    };
    let point_suffix = point_name
        .map(|name| format!(" at the drag {name}"))
        .unwrap_or_default();
    Some(
        cu_core::protocol::ToolResult::error(format!(
            "Pixel dispatch blocked{point_suffix}: {app}{title_suffix} window {} is in front at \
             screen point ({screen_x:.0}, {screen_y:.0}). Retry with \
             delivery_mode:\"foreground\" or clear the obstruction.",
            occluder.window_id,
        ))
        .with_structured(serde_json::json!({
            "error": "obstructed",
            "code": "background_occluded",
            "occluding_app": app,
            "occluding_pid": occluder.pid,
            "occluding_window_id": occluder.window_id,
            "occluding_window_title": occluder.title,
            "screen_point": { "x": screen_x, "y": screen_y },
            "point": point_name,
            "hint": "retry with delivery_mode:\"foreground\" or clear the obstruction"
        })),
    )
}

/// px-focus for the keyboard family (type_text / press_key / hotkey): pixel-click
/// at (x,y) to establish real renderer focus before a keystroke — the *element px
/// action* form of a keyboard tool. Reuses ClickTool's exact coordinate
/// translation + delivery_mode so it lands on the same pixel a px-click would.
/// `Ok(())` on success; `Err(ToolResult)` short-circuits the caller.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn focus_by_pixel(
    state: &Arc<ToolState>,
    pid: i32,
    window_id: Option<u32>,
    x: f64,
    y: f64,
    foreground: bool,
    session: Option<String>,
    session_id: Option<String>,
    from_zoom: bool,
) -> Result<(), cu_core::protocol::ToolResult> {
    use cu_core::tool::Tool;
    let mut click_args = serde_json::json!({
        "pid": pid, "x": x, "y": y,
        "delivery_mode": if foreground { "foreground" } else { "background" },
        "action": if foreground { "press" } else { "focus" },
    });
    if let Some(wid) = window_id {
        click_args["window_id"] = serde_json::json!(wid);
    }
    if let Some(s) = session {
        click_args["session"] = serde_json::json!(s);
    }
    if let Some(s) = session_id {
        click_args["_session_id"] = serde_json::json!(s);
    }
    if from_zoom {
        click_args["from_zoom"] = serde_json::json!(true);
    }
    let focus = click::ClickTool::new(state.clone())
        .invoke(click_args)
        .await;
    if focus.is_error == Some(true) {
        // Preserve structured obstruction/background-unavailable details from
        // ClickTool so pixel-focused keyboard calls remain actionable.
        return Err(focus);
    }
    // Brief settle so the renderer registers focus before the keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    Ok(())
}

/// Thread-safe per-pid zoom context registry.
pub struct ZoomRegistry {
    inner: std::sync::Mutex<HashMap<i32, ZoomContext>>,
}

impl ZoomRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn set(&self, pid: i32, ctx: ZoomContext) {
        self.inner.lock().unwrap().insert(pid, ctx);
    }

    pub fn get(&self, pid: i32) -> Option<ZoomContext> {
        self.inner.lock().unwrap().get(&pid).copied()
    }
}

/// Tracks the per-pid ratio applied by `max_image_dimension` downscaling.
///
/// `ratio = original_dim / resized_dim` — multiply resized image coordinates
/// by this to recover original (native) window-local pixel coordinates.
/// Mirrors Swift's `ImageResizeRegistry`.
pub struct ResizeRegistry {
    inner: std::sync::Mutex<HashMap<i32, f64>>,
}

impl ResizeRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Record that pid's screenshot was downscaled by `ratio`.
    pub fn set_ratio(&self, pid: i32, ratio: f64) {
        self.inner.lock().unwrap().insert(pid, ratio);
    }

    /// Remove the ratio entry (no active downscale).
    pub fn clear_ratio(&self, pid: i32) {
        self.inner.lock().unwrap().remove(&pid);
    }

    /// Returns the most recent ratio, or `None` if no downscale happened.
    pub fn ratio(&self, pid: i32) -> Option<f64> {
        self.inner.lock().unwrap().get(&pid).copied()
    }
}

/// Runtime-mutable driver configuration persisted across calls within a session.
///
/// `capture_mode` is per-call now (the `capture_mode` param). `capture_scope` is
/// re-introduced here as a GLOBAL setting: it gates `get_desktop_state` (a
/// full-display capture has no pid/window_id and therefore no per-call scope to
/// key on — the only coherent gate is a global one), matching the Windows/Linux
/// `DriverConfig.capture_scope`. Per-call tools (`click`/`scroll`) still accept a
/// per-call `scope`; this global is the baseline the no-args desktop-capture
/// tool reads. Default `"window"`.
pub struct DriverConfig {
    /// Max screenshot dimension (0 = no limit). Applied during screenshot/zoom.
    /// Default 1568 matches Swift's `BrigadierCuConfig.defaultMaxImageDimension` —
    /// the long edge is downscaled to this before encoding.
    pub max_image_dimension: u32,
    /// Capture scope: `"window"` (default) or `"desktop"`. Gates
    /// `get_desktop_state` (full-display capture requires `"desktop"`).
    pub capture_scope: String,
    /// Display that `launch_app` moves newly opened windows to. Default
    /// `["secondary"]`: off the main display when another is attached.
    pub launch_display: cu_core::display_placement::DisplayPolicy,
    /// Whether Codex-compatible `get_app_state` may inspect protected apps.
    /// Defaults to true so the app-oriented surface can reach every app.
    pub allow_unrestricted_app_state: bool,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            max_image_dimension: 1568,
            capture_scope: "window".to_owned(),
            launch_display: Default::default(),
            allow_unrestricted_app_state: true,
        }
    }
}

/// Path to the persistent JSON config file shared by the CLI and MCP session.
pub fn config_file_path() -> std::path::PathBuf {
    pip_preview::home::home().join("config.json")
}

/// Load `DriverConfig` from `~/.brigadier-cu/config.json`, falling back to
/// defaults for any missing or unrecognised keys.  Called at MCP startup so
/// that `brigadier-cu config set capture_mode vision` (CLI) carries over into
/// the next MCP session without requiring a per-call `set_config`.
/// Tool error for a screen capture that hit its deadline, carrying the
/// structured `capture_timeout` payload. `None` for every other error.
pub(crate) fn capture_timeout_result(
    context: &str,
    error: &anyhow::Error,
) -> Option<cu_core::protocol::ToolResult> {
    let timeout = crate::capture_deadline::CaptureTimeout::find(error)?;
    Some(
        cu_core::protocol::ToolResult::error(format!("{context}: {timeout}"))
            .with_structured(timeout.to_json()),
    )
}

pub fn load_driver_config() -> DriverConfig {
    let mut cfg = DriverConfig::default();
    let path = config_file_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return cfg, // no file yet — use defaults
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return cfg, // malformed file — use defaults
    };
    // `capture_mode` is per-call now; an old on-disk `capture_mode` key is
    // silently ignored. `capture_scope` IS a global setting again (gates
    // get_desktop_state) — load it, accepting only window|desktop.
    if let Some(v) = json.get("max_image_dimension").and_then(|v| v.as_u64()) {
        if let Ok(v32) = u32::try_from(v) {
            cfg.max_image_dimension = v32;
        }
    }
    if let Some(s) = json.get("capture_scope").and_then(|v| v.as_str()) {
        if s == "window" || s == "desktop" {
            cfg.capture_scope = s.to_owned();
        }
    }
    if let Some(v) = json
        .get("allow_unrestricted_app_state")
        .and_then(|v| v.as_bool())
    {
        cfg.allow_unrestricted_app_state = v;
    }
    if let Some(v) = json.get("launch_display") {
        match cu_core::display_placement::DisplayPolicy::from_json(v) {
            Ok(policy) => cfg.launch_display = policy,
            Err(e) => tracing::warn!("ignoring invalid launch_display in config: {e}"),
        }
    }
    cfg
}

/// Persist a single key/value pair to `~/.brigadier-cu/config.json`.
/// Merges with any existing file contents so other keys are preserved.
/// Returns `Err` if the directory cannot be created or the file cannot be written.
pub fn write_driver_config_key(key: &str, value: &serde_json::Value) -> Result<(), String> {
    let path = config_file_path();
    let mut json: serde_json::Value = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    json[key] = value.clone();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Per-session config overrides layered over the global persisted `DriverConfig`.
///
/// The brigadier-cu daemon is one shared process: every `brigadier-cu mcp` proxy
/// connects to it and shares its `ToolState`. `DriverConfig` is therefore
/// multi-tenant AND persisted to disk — so without session scoping, session A's
/// `set_config capture_mode=vision` clobbers session B's value and flips the
/// on-disk default under everyone. These overrides fix that: a named MCP session
/// gets an in-memory, non-persisted override keyed by its `_session_id`; the
/// anonymous session (CLI / one-shot `call`) still writes the shared global +
/// disk. `None` fields mean "fall through to the global layer".
#[derive(Clone, Default)]
pub struct ConfigOverrides {
    pub max_image_dimension: Option<u32>,
}

/// Thread-safe map of `session_id` → `ConfigOverrides`, mirroring
/// `CursorRegistry`'s registry shape. Cleared per session on `session_end`.
pub struct SessionConfigRegistry {
    inner: std::sync::Mutex<HashMap<String, ConfigOverrides>>,
}

impl SessionConfigRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Merge `delta` into `session`'s overrides (only the `Some` fields of
    /// `delta` overwrite; existing overrides for unset fields are preserved).
    pub fn set(&self, session: &str, delta: ConfigOverrides) {
        // Write-boundary resurrection guard: keyed by session_id, so an
        // in-flight set_config that lands AFTER session_end (passed the dispatch
        // gate, then the proxy died and the reaper cleared this session's
        // overrides) must NOT re-create the entry — it would be invisible and
        // never reaped again. `fire_session_end` marks ENDED_SESSIONS *before*
        // running the config-clear hook, so this check is authoritative.
        if cu_core::session::is_session_ended(session) {
            return;
        }
        let mut map = self.inner.lock().unwrap();
        let entry = map.entry(session.to_owned()).or_default();
        if delta.max_image_dimension.is_some() {
            entry.max_image_dimension = delta.max_image_dimension;
        }
    }

    /// Resolve the effective `max_image_dimension` for `session`, layering its
    /// override over the global `DriverConfig`. `session = None` (anonymous)
    /// returns the global value verbatim.
    pub fn effective_max_image_dimension(
        &self,
        session: Option<&str>,
        global: &DriverConfig,
    ) -> u32 {
        let ov = session.and_then(|s| self.inner.lock().unwrap().get(s).cloned());
        match ov {
            Some(ov) => ov.max_image_dimension.unwrap_or(global.max_image_dimension),
            None => global.max_image_dimension,
        }
    }

    /// Drop `session`'s overrides. No-op for an unknown id (so `session_end`
    /// for an anonymous / never-set session is harmless).
    pub fn clear(&self, session: &str) {
        self.inner.lock().unwrap().remove(session);
    }
}

impl Default for SessionConfigRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared state passed to all tools.
pub struct ToolState {
    pub element_cache: Arc<ElementCache>,
    pub cursor_registry: Arc<CursorRegistry>,
    pub zoom_registry: Arc<ZoomRegistry>,
    pub resize_registry: Arc<ResizeRegistry>,
    /// Global, disk-persisted config — the base layer and the only one the
    /// anonymous session / CLI writes.
    pub config: Arc<std::sync::RwLock<DriverConfig>>,
    /// Per-MCP-session in-memory config overrides layered over `config`.
    pub session_config: Arc<SessionConfigRegistry>,
    /// Open CDP connections, one per port, reused across `insert_text` /
    /// `type_keystrokes` calls instead of reconnecting fresh every time —
    /// see `CdpSessionCache` for why (Chrome's "allow remote debugging"
    /// popup fires on every new connection, not once per session).
    pub cdp_sessions: Arc<crate::browser::CdpSessionCache>,
}

impl Default for ToolState {
    fn default() -> Self {
        Self {
            element_cache: Arc::new(ElementCache::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            zoom_registry: Arc::new(ZoomRegistry::new()),
            resize_registry: Arc::new(ResizeRegistry::new()),
            // Load persisted config from ~/.brigadier-cu/config.json so that
            // `brigadier-cu config set` changes carry over into MCP sessions.
            config: Arc::new(std::sync::RwLock::new(load_driver_config())),
            session_config: Arc::new(SessionConfigRegistry::new()),
            cdp_sessions: Arc::new(crate::browser::CdpSessionCache::new()),
        }
    }
}

/// Register all macOS tools into the registry. `compat=true` swaps the
/// regular `screenshot` tool for the Claude Code computer-use compat
/// variant — same name, stricter args, window-scoped JPEG @ 85% + a text
/// note telling the caller to use pixel-addressed tools.
pub fn register_all(registry: &mut ToolRegistry, compat: bool) {
    registry.set_target_app_resolver(|pid| {
        i32::try_from(pid)
            .ok()
            .and_then(crate::apps::get_app_name_for_pid)
    });
    // Explicit "watchable" fronting: the core action choke point calls this
    // before each targeted drive action only after host opt-in, so ordinary
    // embedded and daemon sessions preserve background delivery semantics.
    registry.set_target_front_hook(front_target_if_watchable);
    // Native target guard: refuse the user's Brigadier, other terminals, the
    // driver, and security surfaces in every profile, not only Codex
    // compatibility. See cu_core::target_policy.
    registry.set_target_policy_hook(crate::target_guard::enforce_native);
    let state = Arc::new(ToolState::default());
    // Share the element cache with the recording-hook layer so it can
    // resolve element_index → window-local screenshot coords for click.png.
    crate::recording_hooks::set_element_cache(state.element_cache.clone());

    // Drop a disconnecting session's config overrides + owned cursor on
    // `session_end`. The daemon fans the session id out to this hook;
    // recording ownership is handled separately on the core RecordingSession.
    {
        let session_config = state.session_config.clone();
        let cursor_registry = state.cursor_registry.clone();
        cu_core::session::register_session_end_hook(move |session_id| {
            session_config.clear(session_id);
            // Per-session agent cursor: the session_id is the cursor key when
            // the caller gave no explicit cursor_id, so dropping it here both
            // prunes the metadata registry and stops the overlay painting that
            // session's cursor. Both paths guard "default" so the anonymous /
            // one-shot cursor survives. Anonymous sessions that never created a
            // cursor are a harmless no-op.
            cursor_registry.remove(session_id);
            crate::cursor::overlay::remove_cursor(session_id.to_owned());
            // A managed Brigadier proxy generation owns only its short-lived
            // lifecycle state. Its actions render under the stable host key,
            // which must be hidden (not removed) so the next generation can
            // reuse the same cursor without leaving a visible remnant.
            if let Some(host_session) = cu_core::host_session_id_from_proxy(session_id) {
                crate::cursor::overlay::end_reusable_cursor_generation(
                    host_session.to_owned(),
                    session_id,
                );
            }
            // Process-global cursor feed: hide only if the ending session still
            // owns the host-rendered cursor, so another active session remains
            // visible. No-op unless BRIGADIER_CU_STATE_DIR is set.
            cu_core::cursor_feed::emit_hidden_if_owned(session_id);
        });
    }

    registry.register(Box::new(list_apps::ListAppsTool));
    registry.register(Box::new(list_windows::ListWindowsTool));
    registry.register(Box::new(get_window_state::GetWindowStateTool::new(
        state.clone(),
    )));
    registry.register(Box::new(launch_app::LaunchAppTool));
    registry.register(Box::new(kill_app::KillAppTool));
    registry.register(Box::new(bring_to_front::BringToFrontTool));
    registry.register(Box::new(click::ClickTool::new(state.clone())));
    registry.register(Box::new(double_click::DoubleClickTool::new(state.clone())));
    registry.register(Box::new(right_click::RightClickTool::new(state.clone())));
    registry.register(Box::new(drag::DragTool::new(state.clone())));
    registry.register(Box::new(type_text::TypeTextTool::new(state.clone())));
    registry.register(Box::new(press_key::PressKeyTool::new(state.clone())));
    registry.register(Box::new(hotkey::HotkeyTool::new(state.clone())));
    registry.register(Box::new(set_value::SetValueTool::new(state.clone())));
    registry.register(Box::new(scroll::ScrollTool::new(state.clone())));
    // The standalone `screenshot` tool was removed (#1692). The pixel-grounding
    // screenshot the Claude Code computer-use compat loop relies on now comes
    // from `get_window_state` (which always returns BOTH the tree AND a
    // screenshot — perception is mode-agnostic; `capture_mode` is deprecated/
    // ignored) for a window, or `get_desktop_state` for the whole screen.
    // `compat` no longer gates a tool swap here — the flag's live purpose is to
    // register the MCP server under the `cu-computer-use` name, which is what
    // triggers Claude Code's computer-use beta-tool injection (see cli.rs).
    let _ = compat;
    registry.register(Box::new(get_screen_size::GetScreenSizeTool));
    registry.register(Box::new(get_desktop_state::GetDesktopStateTool));
    registry.register(Box::new(get_cursor_position::GetCursorPositionTool));
    registry.register(Box::new(move_cursor::MoveCursorTool::new(state.clone())));
    registry.register(Box::new(cursor_tools::SetAgentCursorEnabledTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::SetAgentCursorMotionTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::SetAgentCursorStyleTool::new(
        state.clone(),
    )));
    registry.register(Box::new(cursor_tools::GetAgentCursorStateTool::new(
        state.clone(),
    )));
    registry.register(Box::new(check_permissions::CheckPermissionsTool));
    // `health_report` — single-call end-to-end diagnostics. Stable
    // schema_version="1" contract aimed at downstream consumers who must
    // not have to know brigadier-cu internals. Provider is platform-specific; tool plumbing is in
    // `cu_core::health_report`.
    registry.register(Box::new(cu_core::health_report::HealthReportTool::new(
        Arc::new(health_report::MacosHealthProvider),
    )));
    registry.register(Box::new(get_config::GetConfigTool::new(state.clone())));
    registry.register(Box::new(set_config::SetConfigTool::new(state.clone())));
    registry.register(Box::new(
        get_accessibility_tree::GetAccessibilityTreeTool::new(state.clone()),
    ));
    registry.register(Box::new(zoom::ZoomTool {
        state: state.clone(),
    }));
    // `type_text_chars` is intentionally NOT registered — Swift treats it as
    // a deprecated alias for `type_text` resolved at invoke time in
    // mcp-server's `ToolRegistry::invoke`. Keeping it out of the registry
    // means it doesn't show up in `tools/list` either, matching Swift's
    // ToolRegistry.swift (`type_text_chars` not in `handlers`) and the
    // platform-windows::build_registry which uses the same convention.
    // Touch the struct so it stays in this crate for the alias resolver.
    let _: &type_text_chars::TypeTextCharsTool =
        &type_text_chars::TypeTextCharsTool::new(state.clone());
    // Cross-platform `page` tool definition lives in mcp-server; macOS plugs in
    // its Apple-Events / CDP / AX-tree backend here.
    registry.register(Box::new(cu_core::page::PageTool::new(Arc::new(
        page::MacOsPageBackend::new(state.clone()),
    ))));
    // Recording / replay + session-lifecycle tools are platform-independent.
    registry.register_recording_tools();
    registry.register_session_tools();
}

#[cfg(test)]
mod watchable_front_tests {
    //! Pure front-once/dedupe decision for explicit watchable fronting.
    //! Injects the frontmost pid + last-fronted state so no live WindowServer
    //! is needed.
    use super::{decide_front, FrontDecision};

    fn last(session: Option<&str>, pid: i32) -> (Option<String>, i32) {
        (session.map(str::to_owned), pid)
    }

    #[test]
    fn fronts_when_target_not_frontmost_and_no_history() {
        // First action on a target that is behind the host: bring it forward.
        let d = decide_front(42, Some("s"), Some(99), None);
        assert_eq!(
            d,
            FrontDecision {
                front: true,
                new_last: Some(last(Some("s"), 42))
            }
        );
    }

    #[test]
    fn skips_when_target_already_frontmost() {
        // Never flicker a frontmost app — but still record it as last-fronted.
        let d = decide_front(42, Some("s"), Some(42), None);
        assert_eq!(
            d,
            FrontDecision {
                front: false,
                new_last: Some(last(Some("s"), 42))
            }
        );
    }

    #[test]
    fn dedupes_unchanged_target_while_activation_settles() {
        // We fronted (s,42) last action; frontmost hasn't updated yet (unknown).
        // No *other* app stole focus, so don't re-issue the activation.
        let prev = last(Some("s"), 42);
        let d = decide_front(42, Some("s"), None, Some(&prev));
        assert_eq!(
            d,
            FrontDecision {
                front: false,
                new_last: Some(prev)
            }
        );
    }

    #[test]
    fn refronts_unchanged_target_when_another_app_stole_focus() {
        // Same target as last action, but a DIFFERENT app is now frontmost —
        // a real focus steal — so bring the target back for the watcher.
        let prev = last(Some("s"), 42);
        let d = decide_front(42, Some("s"), Some(77), Some(&prev));
        assert_eq!(
            d,
            FrontDecision {
                front: true,
                new_last: Some(last(Some("s"), 42))
            }
        );
    }

    #[test]
    fn fronts_when_target_changed_even_if_not_frontmost() {
        // Last action fronted a different target; the new target is not
        // frontmost → front it.
        let prev = last(Some("s"), 7);
        let d = decide_front(42, Some("s"), Some(7), Some(&prev));
        assert_eq!(
            d,
            FrontDecision {
                front: true,
                new_last: Some(last(Some("s"), 42))
            }
        );
    }

    #[test]
    fn distinct_sessions_do_not_dedupe_each_other() {
        // Same pid, different session key ⇒ treated as changed; front when the
        // target is not already frontmost.
        let prev = last(Some("a"), 42);
        let d = decide_front(42, Some("b"), None, Some(&prev));
        assert_eq!(
            d,
            FrontDecision {
                front: true,
                new_last: Some(last(Some("b"), 42))
            }
        );
    }
}

#[cfg(test)]
mod session_config_guard_tests {
    use super::*;
    use cu_core::session::fire_session_end;

    fn overrides(max_dim: u32) -> ConfigOverrides {
        ConfigOverrides {
            max_image_dimension: Some(max_dim),
        }
    }

    #[test]
    fn ended_session_config_set_is_noop() {
        // THE FIX (config side): an ended session id keys the overrides map, so
        // an in-flight set_config after session_end must not re-create the entry
        // the reaper's clear hook removed. effective then falls back to global.
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-ended-Q9R8S7";
        fire_session_end(sid);
        assert!(cu_core::session::is_session_ended(sid));

        reg.set(sid, overrides(800));
        let dim = reg.effective_max_image_dimension(Some(sid), &global);
        assert_eq!(
            dim, global.max_image_dimension,
            "ended session must not get an override entry"
        );
    }

    #[test]
    fn live_session_config_set_takes_effect() {
        let reg = SessionConfigRegistry::new();
        let global = DriverConfig::default();
        let sid = "wb-config-live-T1U2V3";
        assert!(!cu_core::session::is_session_ended(sid));
        reg.set(sid, overrides(800));
        let dim = reg.effective_max_image_dimension(Some(sid), &global);
        assert_eq!(dim, 800, "live session override must apply");
    }
}

// RecordingSession lives in cu-core, but its `start()` pulls in the
// macOS cursor sampler (CoreGraphics), so the start-guard test runs here in
// platform-macos where build.rs links the frameworks — the core crate's test
// binary has no CoreGraphics linkage.
#[cfg(test)]
mod recording_start_guard_tests {
    use cu_core::recording::RecordingSession;
    use cu_core::session::fire_session_end;

    #[test]
    fn start_refuses_for_ended_session_owner() {
        // THE FIX (recording side): an in-flight start_recording owned by a
        // session that already ended would leak an ffmpeg/SCStream process owned
        // by a dead session that is never reaped. start() must refuse.
        let rec = RecordingSession::new();
        let sid = "wb-recording-ended-W4X5Y6";
        fire_session_end(sid);
        assert!(cu_core::session::is_session_ended(sid));

        let dir = std::env::temp_dir().join("wb-rec-ended");
        let err = rec.start(dir.to_str().unwrap(), false, Some(sid));
        assert!(err.is_err(), "start for an ended session owner must error");
        assert!(
            !rec.current_state().enabled,
            "no recording may start for a dead session"
        );
    }

    #[test]
    fn start_succeeds_for_live_session_owner() {
        let rec = RecordingSession::new();
        let sid = "wb-recording-live-Z7A8B9";
        assert!(!cu_core::session::is_session_ended(sid));
        let dir = std::env::temp_dir().join("wb-rec-live");
        // record_video=false avoids spawning ffmpeg in the test.
        let ok = rec.start(dir.to_str().unwrap(), false, Some(sid));
        assert!(ok.is_ok(), "start for a live session owner must succeed");
        assert!(rec.current_state().enabled);
        let _ = rec.stop_owner(Some(sid));
    }

    #[test]
    fn start_succeeds_for_anonymous_owner() {
        // owner = None (CLI one-shot / legacy shim) is never gated.
        let rec = RecordingSession::new();
        let dir = std::env::temp_dir().join("wb-rec-anon");
        let ok = rec.start(dir.to_str().unwrap(), false, None);
        assert!(ok.is_ok(), "anonymous start must never be gated");
        assert!(rec.current_state().enabled);
        let _ = rec.stop_owner(None);
    }
}
