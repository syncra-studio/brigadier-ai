//! click tool — matches the Swift reference ClickTool.swift.
//!
//! Two addressing modes:
//!
//! * **AX path** (`element_index` + `window_id`): performs AXAction on the cached
//!   element. Fires via AX RPC — the target app never needs to be frontmost.
//!   Extra behaviors vs. the naive dispatch:
//!   - AXTextField / AXTextArea: 800 ms post-click delay for WebKit DOM focus settle.
//!   - AXPopUpButton: appends the list of available options and redirects to set_value.
//!   - Advertised-action warning if the element didn't list the requested action.
//!
//! * **Pixel path** (`x`, `y`): synthesises CGEvent mouse clicks and posts them to
//!   the target pid.  `from_zoom=true` translates zoom-crop pixel coordinates back
//!   to full-window space using the most recent `zoom` context stored per-pid.

use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_action_names, copy_children, copy_string_attr, copy_stringified_attr,
    element_at_screen_position, element_screen_rect, kAXErrorSuccess, AXUIElementPerformAction,
    AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::{CFRelease, TCFType};

use super::ToolState;

pub struct ClickTool {
    state: Arc<ToolState>,
}

impl ClickTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "click".into(),
        description:
            "Click against a target pid. **Prefer `element_index` over pixel \
             coordinates** — element_index works on backgrounded / minimized / hidden / \
             off-Space windows, surfaces a stable handle that survives rebuilds, and tells \
             you what you're clicking via the cached element's role + label. Reach for \
             `x, y` only when the target is a canvas / video / WebGL / custom-drawn surface \
             that doesn't appear in the AX tree.\n\n\
             Two addressing modes:\n\n\
             - element_index + window_id (from last get_window_state): AX action path. \
               Works on backgrounded/hidden windows. No cursor move, no focus steal. \
               element_index cache is scoped per (pid, window_id) and is replaced by the \
               next snapshot of the same window — re-snapshot every turn before clicking.\n\n\
             - x, y (window-local screenshot pixels, top-left origin of the PNG returned \
               by get_window_state): CGEvent path. Synthesizes mouse events and posts to \
               pid. Before a background post with pid+window_id, the driver checks the \
               front-to-back WindowServer stack and returns error=`obstructed`, \
               code=`background_occluded` without posting if another app/window owns \
               that point. Use modifier for \
               cmd/shift/option/ctrl. Needs a visible on-screen window to anchor the \
               conversion.\n\n\
             button: \"left\" (default), \"right\", or \"middle\". Defaults to left so the \
             field is fully back-compat — omit it and you get the legacy left-click behaviour. \
             Pixel path: routes through the CGEvent left/right/middle mouse-button primitives. \
             AX path: \"right\" maps to AXShowMenu (same surface as the dedicated `right_click` \
             tool); \"middle\" has no AX equivalent and falls back to a pixel middle-click at the \
             element's center.\n\
             action: press (default), show_menu, pick, confirm, cancel, open.\n\
             from_zoom: set true after a zoom call to auto-translate zoom-image pixel \
             coordinates to full-window space."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            // `pid` is conditionally required — needed for window/element clicks
            // but omitted for windowless `scope:"desktop"` clicks — so it is NOT
            // in `required`; the code validates it with a clear error when needed.
            // (Keeps the contract consistent across platforms; see
            // cu_core::tool_schema.)
            "required": [],
            "properties": {
                "session": { "type": "string", "description": "Optional explicit session id for the agent cursor and per-session state. Embedded MCP calls may omit it to use BRIGADIER_CU_DEFAULT_SESSION (or embedded-<pid>); anonymous non-embedded calls remain cursor-less." },
                "pid":           { "type": "integer", "description": "Target process ID." },
                "window_id":     { "type": "integer", "description": "Target window ID. Required for element_index. Optional when element_token is supplied (the token carries it)." },
                "element_index": { "type": "integer", "description": "Element index from last get_window_state. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op." },
                "element_token": { "type": "string",  "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token` of the last get_window_state. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded — re-snapshot in that case." },
                "x":             { "type": "number",  "description": "X in screenshot pixels, read straight off the image you were handed — no scaling math needed. With pid+window_id (capture_scope=window): window-local pixels from the get_window_state PNG (top-left origin). Windowless (no pid/window_id, capture_scope=desktop): pixels from the get_desktop_state PNG (the native full-display image). Either way, the pixel you read IS the pixel that gets clicked; the driver undoes the Retina backing scale + any downscale internally." },
                "y":             { "type": "number",  "description": "Y in screenshot pixels (see x). Window-local from get_window_state, or full-display from get_desktop_state under capture_scope=desktop." },
                "action":        { "type": "string",  "description": "AX action: press, show_menu, pick, confirm, cancel, open." },
                "button":        {
                    "type": "string",
                    "enum": ["left", "right", "middle"],
                    "description": "Mouse button. Default: \"left\" — omit for legacy left-click behaviour. Pixel path uses the matching CGEvent primitive; AX path maps \"right\" to AXShowMenu and falls back to a pixel middle-click at the element's center for \"middle\"."
                },
                "count":         { "type": "integer", "description": "Click count (pixel path only). Default 1." },
                "modifier": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Modifier keys: cmd, shift, option/alt, ctrl."
                },
                "from_zoom": {
                    "type": "boolean",
                    "description": "When true, x and y are in the last zoom image for this pid; driver translates back to full-window coordinates."
                },
                "debug_image_out": {
                    "type": "string",
                    "description": "Optional file path. When set on a pixel-addressed click, captures a fresh screenshot, draws a red crosshair at (x, y), and writes the PNG. Use to verify coordinate spaces. Requires window_id; incompatible with from_zoom."
                },
                "delivery_mode": {
                    "type": "string",
                    "enum": ["background", "foreground"],
                    "description": "Best-effort-background ladder rung (default \"background\"). \"background\": perform the AX action or post the CGEvent without fronting; pixel dispatch with pid+window_id fails with structured error=\"obstructed\", code=\"background_occluded\" when a different visible window owns the screen point. \"foreground\": briefly front the window, act, let transient UI settle, then restore the prior frontmost app; foreground skips the obstruction check because fronting resolves Z order. Requires window_id. A click that is dispatched remains verified:false — confirm its effect via get_window_state."
                },
                "scope": {
                    "type": "string",
                    "enum": ["window", "desktop"],
                    "description": "Coordinate frame for a windowless screen-absolute click (default \"window\"). Pass \"desktop\" when sending x,y with NO pid/window_id — the coordinates are then true screen pixels (read from get_desktop_state with scope=\"desktop\"). Per-call; not a setting."
                }
            },
            "additionalProperties": false
        }),
        read_only:   false,
        destructive: true,
        idempotent:  false,
        open_world:  true,
    })
}

#[async_trait]
impl Tool for ClickTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    fn dispatch_preflight(&self, args: &Value) -> Result<(), ToolResult> {
        click_dispatch_preflight(args)
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        let dispatch_gate = crate::dispatch_gate::NativeDispatchGate::for_args(&args);

        // ── Window-less screen-absolute branch (scope="desktop") ──────
        // x,y given with NO pid and NO window_id → the coordinates are TRUE
        // SCREEN pixels. This is the foreground, vision-driven desktop-scope
        // path, the macOS peer of the Windows WindowFromPoint click. Gate on the
        // effective scope: under "window" return a structured
        // `desktop_scope_disabled` error (same contract as Windows) rather than
        // silently treating window-local pixels as screen pixels.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if has_xy && !has_pid && !has_window_id {
            // `scope` is a per-call param now (default "window"); pass
            // scope="desktop" to enable screen-absolute clicks.
            let scope = args.str_or("scope", "window");
            if scope != "desktop" {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id, but scope is \"window\". \
                     Screen-absolute clicks require desktop scope. Pass scope=\"desktop\" \
                     (and use get_desktop_state with scope=\"desktop\" to read true \
                     screen pixels) first."
                        .to_string(),
                )
                .with_structured(serde_json::json!({
                    "code": "desktop_scope_disabled",
                    "scope": scope,
                    "suggestion": "pass scope=\"desktop\"",
                }));
            }
            let sx_shot = args
                .opt_f64("x")
                .or_else(|| args.opt_i64("x").map(|i| i as f64))
                .unwrap_or(0.0);
            let sy_shot = args
                .opt_f64("y")
                .or_else(|| args.opt_i64("y").map(|i| i as f64))
                .unwrap_or(0.0);
            // ── Desktop-screenshot pixels → logical screen points ──────────────
            // The vision invariant: the pixel an agent reads off the screenshot it
            // was handed is the pixel that gets clicked. `get_desktop_state`
            // returns the display at NATIVE pixels (e.g. 3024×1964 on a 2× Retina
            // display whose logical size is 1512×982), but everything below — the
            // window-under-point hit test (logical CGWindow bounds), the cursor
            // warp, and the CGEvent post — operates in LOGICAL screen points. So
            // x,y arrive in desktop-SCREENSHOT space (what the agent reads off the
            // PNG) and must be divided by the screenshot↔logical ratio, or a
            // center-pixel pick warps to the corner (off by the backing scale).
            //
            // Derive the ratio the same way `get_desktop_state` reports it: native
            // screenshot width / logical screen width. This is robust even when
            // CGDisplayPixelsWide under-reports the backing scale (it returns the
            // scaled-mode point width on some Retina configs → a bogus 1.0).
            let desktop_ratio = tokio::task::spawn_blocking(|| {
                let logical_w =
                    super::get_screen_size::main_screen_size().map(|(w, _, _)| w as f64);
                let shot_w = crate::capture::screenshot_display_bytes()
                    .ok()
                    .and_then(|png| crate::capture::png_dimensions(&png).ok())
                    .map(|(w, _)| w as f64);
                match (shot_w, logical_w) {
                    (Some(sw), Some(lw)) if lw > 0.0 && sw > lw => sw / lw,
                    _ => 1.0,
                }
            })
            .await
            .unwrap_or(1.0);
            let sx = sx_shot / desktop_ratio;
            let sy = sy_shot / desktop_ratio;
            let button = match args.str_or("button", "left").to_lowercase().as_str() {
                "right" => "right",
                "middle" => "middle",
                "" | "left" => "left",
                other => {
                    return ToolResult::error(format!(
                        "click: unknown button \"{other}\" — expected one of left, right, middle."
                    ))
                }
            }
            .to_string();
            let count = args.u64_or("count", 1) as usize;
            // Glide the session's agent cursor to the screen point for visibility.
            let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
            crate::cursor::overlay::animate_cursor_for_action(cursor_key.clone(), sx, sy, &args)
                .await;
            self.state
                .cursor_registry
                .update_position(&cursor_key, sx, sy);

            let btn = button.clone();
            let gate = dispatch_gate.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                // Desktop scope is explicitly foreground and vision-driven: post
                // at the global HID tap so WindowServer delivers to the window
                // actually visible at this point. PID-posting here would silently
                // turn the foreground contract back into background delivery.
                crate::input::mouse::click_at_xy_desktop_guarded(sx, sy, count, &btn, &gate)
            })
            .await;
            let button_label = match button.as_str() {
                "right" => "right-click",
                "middle" => "middle-click",
                _ => "click",
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent screen-absolute {button_label} at desktop-pixel \
                     ({sx_shot:.0},{sy_shot:.0}) → screen-point ({sx:.0},{sy:.0}) \
                     (desktop scope; not driver-verified)."
                ))
                .with_structured(serde_json::json!({ "path": "cgevent_hid", "verified": false, "effect": "unverifiable" })),
                Ok(Err(e)) => ToolResult::error(format!("desktop-scope click failed: {e}")),
                Err(e) => ToolResult::error(format!("task error: {e}")),
            };
        }

        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Resolve this action's cursor key so its click-pulse / glide land on
        // the calling session's cursor, not the shared "default" one.
        let cursor_key = super::cursor_tools::resolve_cursor_key(&args);

        // Surface 6: resolve element_token / element_index precedence
        // BEFORE the pixel-path fallback. Token wins on disagreement; a
        // stale token returns an explicit error instead of silently
        // falling back to the integer (Surface 6 hard constraint).
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cu_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id, _via_token) = match resolved {
            cu_core::element_token::ResolvedElement::None => (None, window_id_arg, false),
            cu_core::element_token::ResolvedElement::Element {
                window_id: wid,
                element_index: idx,
                via_token,
            } => (Some(idx), wid, via_token),
        };
        let x = args
            .opt_f64("x")
            .or_else(|| args.opt_i64("x").map(|i| i as f64));
        let y = args
            .opt_f64("y")
            .or_else(|| args.opt_i64("y").map(|i| i as f64));
        let action = args.str_or("action", "press");
        // Surface 5: optional `button` arg, default "left" preserves legacy behaviour.
        // Pixel path: routes to left/right/middle CGEvent primitives.
        // AX path: "right" delegates to AXShowMenu (same surface as right_click);
        // "middle" has no AX equivalent and falls back to a pixel middle-click
        // at the element's screen-space center.
        let button_str = args.str_or("button", "left").to_lowercase();
        // delivery_mode: per-call ladder rung. Foreground briefly activates the
        // target for both AX and pixel paths, then restores the prior app.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        // Reject unknown buttons explicitly so silent left-click fall-through can't
        // mask a typo. Keep "" → default left for old clients that never sent the field.
        if !matches!(button_str.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_str}\" — expected one of left, right, middle."
            ));
        }
        let button_str = if button_str.is_empty() {
            "left".to_string()
        } else {
            button_str
        };
        let count = args.u64_or("count", 1) as usize;
        let from_zoom = args.bool_or("from_zoom", false);
        let debug_image_out = args.opt_str("debug_image_out");
        let modifiers: Vec<String> = args.str_array("modifier");

        if let (Some(idx), Some(wid)) = (element_index, window_id) {
            // ── AX element path ────────────────────────────────────────────
            // Retain the element out of the cache so it can't be freed by a
            // concurrent get_window_state on the same (pid, window_id) while
            // this click is mid-flight (use-after-free → daemon crash). The
            // guard lives to the end of this method, past the AX action below.
            let element_guard = match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => e,
                None => {
                    return ToolResult::error(format!(
                        "Element index {idx} not found in cache for pid={pid} window_id={wid}. \
                     Call get_window_state first."
                    ))
                }
            };
            let element_ptr = element_guard.as_ptr();

            // Surface 5: button=right on the AX path → AXShowMenu (the same surface
            // the dedicated `right_click` tool dispatches). Threads through the
            // identical perform_ax_click code path with the action remapped.
            let effective_action = if button_str == "right" && action == "press" {
                "show_menu".to_string()
            } else {
                action.clone()
            };

            // Animate cursor to element center BEFORE firing AX action,
            // mirroring Swift's `performElementClick` → `animateAndWait(to:)`.
            let center_ptr = element_ptr;
            let center = tokio::task::spawn_blocking(move || unsafe {
                crate::ax::bindings::element_screen_center(center_ptr as AXUIElementRef)
            })
            .await
            .ok()
            .flatten();

            // Surface 5: button=middle on the AX path has no AX equivalent.
            // Fall back to a pixel middle-click at the element's screen-space center
            // so the request still produces a real middle-button event (browser tab
            // close, autoscroll, etc.). If we can't resolve a center, error rather
            // than silently degrade to AXPress.
            if button_str == "middle" {
                let (cx, cy) = match center {
                    Some(c) => c,
                    None => {
                        return ToolResult::error(
                            "click(button=middle) on element_index: could not resolve element \
                         center for the pixel-middle-click fallback. Pass x, y directly.",
                        )
                    }
                };
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
                crate::cursor::overlay::animate_cursor_for_action(
                    cursor_key.clone(),
                    cx,
                    cy,
                    &args,
                )
                .await;
                self.state
                    .cursor_registry
                    .update_position(&cursor_key, cx, cy);

                let mods_owned = modifiers.clone();
                let gate = dispatch_gate.clone();
                let result = tokio::task::spawn_blocking(move || {
                    let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    crate::input::mouse::middle_click_at_xy_guarded(pid, cx, cy, &m, &gate)
                })
                .await;
                return match result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Posted middle-click to pid {pid} at element [{idx}] center \
                         (background CGEvent; not driver-verified — confirm via screenshot)."
                    ))
                    .with_structured(serde_json::json!({ "path": "cgevent", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(e)) => ToolResult::error(format!("Middle-click failed: {e}")),
                    Err(e)     => ToolResult::error(format!("Task error: {e}")),
                };
            }

            if let Some((cx, cy)) = center {
                // Pin overlay above target window first.
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
                crate::cursor::overlay::animate_cursor_for_action(
                    cursor_key.clone(),
                    cx,
                    cy,
                    &args,
                )
                .await;
                // Keep the registry in sync with the overlay so
                // get_agent_cursor_state reports a truthful position even when
                // the click was dispatched via the AX path (no pixel coords).
                self.state
                    .cursor_registry
                    .update_position(&cursor_key, cx, cy);
            }

            // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
            // Capture prior frontmost, arm the wildcard suppressor in the
            // snapshot, then arm a targeted suppressor across the AX action
            // itself via FocusGuard. After the action returns, detect any
            // new-window / foreground side-effects and append a one-liner
            // suffix matching Swift's wording.
            let prior_front = apps::frontmost_pid();
            let foreground = delivery_mode.is_foreground();
            let snapshot = if foreground {
                WindowChangeDetector::snapshot_without_suppression_for_session(
                    prior_front,
                    &super::cursor_tools::resolve_cursor_key(&args),
                )
            } else {
                WindowChangeDetector::snapshot_for_session(
                    prior_front,
                    &super::cursor_tools::resolve_cursor_key(&args),
                )
            };

            // Run AX work on a blocking thread (can't block async executor).
            // Use `effective_action` so button=right rewrites press → show_menu.
            let action_clone = effective_action.clone();
            // Thread the resolved session cursor key into the blocking AX path
            // so its ShowFocusRect + ClickPulse land on THIS session's cursor,
            // not the shared "default" one (which would light the wrong cursor
            // and stomp default for a non-default session).
            let ck = cursor_key.clone();
            let gate = dispatch_gate.clone();
            let result = focus_guard::with_focus_suppressed(
                if foreground { None } else { Some(pid) },
                prior_front,
                "click.AXPress",
                || async move {
                    tokio::task::spawn_blocking(move || {
                        if foreground {
                            let mut outcome = None;
                            let fronted = crate::input::skylight::with_foreground_assist(
                                pid as libc::pid_t,
                                wid,
                                || {
                                    outcome = Some(perform_ax_click(
                                        element_ptr,
                                        idx,
                                        pid,
                                        wid,
                                        &action_clone,
                                        &ck,
                                        &gate,
                                        false,
                                    )?);
                                    std::thread::sleep(std::time::Duration::from_millis(150));
                                    Ok(())
                                },
                            )?;
                            let outcome = outcome.ok_or_else(|| {
                                anyhow::anyhow!("foreground AX click did not execute")
                            })?;
                            Ok((outcome, fronted))
                        } else {
                            perform_ax_click(
                                element_ptr,
                                idx,
                                pid,
                                wid,
                                &action_clone,
                                &ck,
                                &gate,
                                true,
                            )
                            .map(|outcome| (outcome, false))
                        }
                    })
                    .await
                },
            )
            .await;

            // Drop the wildcard lease + detect window/foreground side-effects.
            let changes = snapshot.detect_async_for_args(&args).await;

            match result {
                Ok(Ok(((mut msg, needs_webkit_delay, suspected_noop), fronted))) => {
                    // For text inputs, wait 800ms for WebKit DOM focus to settle
                    // before returning — matches the Swift reference behaviour.
                    if needs_webkit_delay {
                        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                    }
                    msg.push_str(&changes.result_suffix());
                    // AX dispatch went through, but AXPerformAction returning
                    // success does not confirm the on-screen effect (many elements
                    // no-op silently). A click is never driver-verifiable (no
                    // read-back) → verified:false stays for back-compat. The
                    // tri-state `effect` is the richer signal:
                    //   * suspected_noop — the element didn't advertise the action,
                    //     so the press likely did nothing → cross to vision/pixel.
                    //   * unverifiable — dispatched fine, driver just can't confirm;
                    //     the caller verifies via screenshot.
                    let mut structured = serde_json::json!({
                        "path": if fronted { "ax_fg" } else { "ax" },
                        "verified": false,
                        "effect": if suspected_noop { "suspected_noop" } else { "unverifiable" },
                    });
                    if suspected_noop {
                        structured["escalation"] = serde_json::json!({
                            "recommended": "px",
                            "reason": "element does not advertise this action — the \
                                       AX press likely no-op'd. Do an element px \
                                       action: click by pixel (x,y) off the \
                                       screenshot from get_window_state."
                        });
                    }
                    ToolResult::text(msg).with_structured(structured)
                }
                Ok(Err(e)) => match e.downcast_ref::<AxActionFailure>() {
                    Some(failure) => {
                        let mut result = ToolResult::error(format!("AX action failed: {failure}"));
                        result.structured_content = Some(serde_json::json!({
                            "path": "ax",
                            "verified": false,
                            "effect": failure.effect,
                            "error_code": failure.code,
                        }));
                        result
                    }
                    None => ToolResult::error(format!("AX action failed: {e}")),
                },
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut cx), Some(mut cy)) = (x, y) {
            // ── Pixel path ─────────────────────────────────────────────────

            // debug_image_out: capture fresh screenshot, overlay crosshair BEFORE
            // any coordinate translation (so it shows received coords in the same
            // space the caller was reasoning in).
            if let Some(ref dbg_path) = debug_image_out {
                if from_zoom {
                    return ToolResult::error(
                        "debug_image_out is incompatible with from_zoom — \
                         received (x, y) would be in zoom-crop space, not window-local.",
                    );
                }
                match window_id {
                    None => return ToolResult::error("debug_image_out requires window_id."),
                    Some(wid) => {
                        // Session-effective max dimension so debug_image_out
                        // matches the resize the calling session sees in
                        // get_window_state (precedence: session override > global).
                        let max_dim = self.state.session_config.effective_max_image_dimension(
                            args.opt_str("_session_id").as_deref(),
                            &self.state.config.read().unwrap(),
                        );
                        let dbg_path_c = dbg_path.clone();
                        let dbg_result = tokio::task::spawn_blocking(move || {
                            let png = crate::capture::screenshot_window_bytes(wid)?;
                            let png = crate::capture::resize_png_if_needed(&png, max_dim)?;
                            crate::capture::write_crosshair_png(&png, cx, cy, &dbg_path_c)
                        })
                        .await;
                        match dbg_result {
                            Err(e) => {
                                return ToolResult::error(format!(
                                    "debug_image_out task failed: {e}. Not dispatching click."
                                ))
                            }
                            Ok(Err(e)) => {
                                return ToolResult::error(format!(
                                    "debug_image_out write failed: {e}. Not dispatching click."
                                ))
                            }
                            Ok(Ok(())) => {}
                        }
                    }
                }
            }

            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(cx, cy);
                        cx = wx;
                        cy = wy;
                    }
                    None => {
                        return ToolResult::error(format!(
                            "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                        ))
                    }
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                // Coordinates are in the downscaled image space; scale back to native pixels.
                cx *= ratio;
                cy *= ratio;
            }

            // ── Window-local → screen coordinate translation ──────────────────
            // `click_at_xy` accepts screen-space coordinates (top-left origin).
            // Callers supply window-local screenshot pixels; we add the window's
            // screen-origin to produce the final screen position.
            //
            // Backing scale: screencapture captures at physical pixels, so on a
            // Retina display the screenshot is 2× the logical window size.
            // We detect the scale by comparing the live screenshot dimensions to
            // the window's logical bounds from WindowServer.
            //
            // win_local_x/y: window-local logical-pixel coords (= cx/scale, cy/scale)
            // needed for CGEventSetWindowLocation in the Chromium recipe.
            let (screen_x, screen_y, win_local_x, win_local_y) = if let Some(wid) = window_id {
                let result = tokio::task::spawn_blocking(move || {
                    let bounds = crate::windows::window_bounds_by_id(wid);
                    let scale: f64 = if let Some(ref b) = bounds {
                        // Detect Retina scale from the window screenshot.
                        // We take a tiny peek at the PNG dimensions to compare
                        // against the logical bounds.
                        if let Ok(png) = crate::capture::screenshot_window_bytes(wid) {
                            if png.len() >= 24 {
                                let pw =
                                    u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
                                let lw = b.width;
                                if lw > 0.0 && pw > lw {
                                    pw / lw
                                } else {
                                    1.0
                                }
                            } else {
                                1.0
                            }
                        } else {
                            1.0
                        }
                    } else {
                        1.0
                    };
                    (bounds, scale)
                })
                .await
                .unwrap_or((None, 1.0));
                if let (Some(b), scale) = result {
                    let wx = cx / scale;
                    let wy = cy / scale;
                    // Window-local screenshot pixel → GLOBAL top-left-origin
                    // screen point; this is the coordinate space the cursor
                    // feed emits in (see cu_core::cursor_feed).
                    let (gx, gy) =
                        cu_core::cursor_feed::window_local_to_global((b.x, b.y), scale, (cx, cy));
                    (gx, gy, wx, wy)
                } else {
                    // window_id not found — fall back to treating x,y as screen coords.
                    (cx, cy, cx, cy)
                }
            } else {
                // No window_id → treat x,y as screen coordinates (legacy behaviour).
                (cx, cy, cx, cy)
            };

            // A background PX action can still use an accessibility delivery
            // backend after resolving the requested screen point. This keeps
            // targeting (PX) orthogonal to delivery (AX) and avoids making a
            // Chromium/AppKit window key merely to satisfy first-mouse rules.
            if !delivery_mode.is_foreground()
                && window_id.is_some()
                && button_str == "left"
                && count == 1
                && modifiers.is_empty()
            {
                let focus_only = action == "focus";
                // The AX delivery backend must stay visually identical to the
                // CGEvent path: glide the branded cursor to the target before
                // pressing, so a background AX press never looks like the
                // action teleported. Mirrors the element-click path above
                // (PinAbove → animate → registry sync). Focus-only actions are
                // not clicks and stay motionless. If no AX element is found,
                // the CGEvent fallback below re-animates to the same point,
                // which is an instant no-op glide.
                if !focus_only {
                    if let Some(wid) = window_id {
                        crate::cursor::overlay::send_command(
                            cursor_key.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                        );
                    }
                    crate::cursor::overlay::animate_cursor_for_action(
                        cursor_key.clone(),
                        screen_x,
                        screen_y,
                        &args,
                    )
                    .await;
                    self.state
                        .cursor_registry
                        .update_position(&cursor_key, screen_x, screen_y);
                }
                let ax_result = tokio::task::spawn_blocking(move || unsafe {
                    let Some(element) = element_at_screen_position(pid, screen_x, screen_y) else {
                        return Ok::<bool, anyhow::Error>(false);
                    };
                    let delivered = if focus_only {
                        crate::input::ax_actions::focus_element(element as usize).is_ok()
                    } else {
                        let press = core_foundation::string::CFString::new("AXPress");
                        AXUIElementPerformAction(element, press.as_concrete_TypeRef())
                            == kAXErrorSuccess
                    };
                    CFRelease(element as _);
                    Ok(delivered)
                })
                .await;
                match ax_result {
                    Ok(Ok(true)) => {
                        let label = if focus_only { "focused" } else { "pressed" };
                        if !focus_only {
                            // The press landed — pulse the cursor at the click
                            // point exactly like the CGEvent and element paths.
                            crate::cursor::overlay::send_command(
                                cursor_key.clone(),
                                cursor_overlay::OverlayCommand::ClickPulse {
                                    x: screen_x,
                                    y: screen_y,
                                },
                            );
                        }
                        return ToolResult::text(format!(
                            "✅ PX hit-test {label} the background element via AX."
                        ))
                        .with_structured(serde_json::json!({
                            "path": "ax",
                            "verified": false,
                            "effect": "unverifiable"
                        }));
                    }
                    Ok(Ok(false)) if focus_only => {
                        return ToolResult::error(
                            "Background PX focus is unavailable at the requested point.".to_owned(),
                        )
                        .with_structured(serde_json::json!({
                            "code": "background_unavailable"
                        }));
                    }
                    Ok(Err(error)) if focus_only => {
                        return ToolResult::error(format!("Background PX focus failed: {error}"))
                            .with_structured(serde_json::json!({
                                "code": "background_unavailable"
                            }));
                    }
                    _ => {}
                }
            }

            // PID-posted CGEvents are still hit-tested by WindowServer at the
            // screen point. If another window owns that pixel, fail before
            // posting instead of claiming an unverifiable success. Foreground
            // mode intentionally skips this check because it fronts the target.
            if !delivery_mode.is_foreground() {
                if let Some(wid) = window_id {
                    if let Some(error) =
                        super::pixel_obstruction_error(pid, wid, screen_x, screen_y, None)
                    {
                        return error;
                    }
                }
            }

            // Pin the overlay above the target window BEFORE animating so
            // the cursor is already sandwiched correctly while it glides in.
            if let Some(wid) = window_id {
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
            }
            // Animate the visual cursor to the click point and wait for it to
            // arrive — mirrors Swift's `AgentCursor.shared.animateAndWait(to:)`.
            crate::cursor::overlay::animate_cursor_for_action(
                cursor_key.clone(),
                screen_x,
                screen_y,
                &args,
            )
            .await;
            // Keep the registry in sync with the overlay (see AX path above).
            self.state
                .cursor_registry
                .update_position(&cursor_key, screen_x, screen_y);
            // Show click-pulse on the agent cursor overlay.
            crate::cursor::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: screen_x,
                    y: screen_y,
                },
            );

            // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
            // A pixel click can land on a "Sign In" button that opens a sheet
            // or a Safari link that activates a new tab — same side-effect
            // shape as the AX path, so we wrap identically.
            let prior_front = apps::frontmost_pid();
            let snapshot = WindowChangeDetector::snapshot_for_session(
                prior_front,
                &super::cursor_tools::resolve_cursor_key(&args),
            );

            let mods_owned = modifiers.clone();
            // Surface 5: route to the right/middle CGEvent primitives when
            // button != left. Left-button path stays on the existing Chromium-
            // routed `click_at_xy_with_window_local` for back-compat.
            let button_kind = button_str.clone();
            // delivery_mode:foreground briefly fronts the window before clicking —
            // the explicit last resort for surfaces that drop background synthetic
            // clicks. Needs window_id to front; without one it degrades to
            // background (and is labelled as such).
            let fg = delivery_mode.is_foreground() && window_id.is_some();
            let gate = dispatch_gate.clone();

            let result = focus_guard::with_focus_suppressed(
                Some(pid),
                prior_front,
                "click.pixel",
                || async move {
                    tokio::task::spawn_blocking(move || {
                        let action_gate = gate.clone();
                        let do_click = move || -> anyhow::Result<()> {
                            let m: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                            match button_kind.as_str() {
                                "right" => {
                                    if let Some(wid) = window_id {
                                        return crate::input::mouse::right_click_at_xy_with_window_local_guarded(
                                            pid, screen_x, screen_y, win_local_x, win_local_y, wid, &m, &action_gate,
                                        );
                                    }
                                    crate::input::mouse::right_click_at_xy_guarded(
                                        pid, screen_x, screen_y, &m, &action_gate,
                                    )
                                }
                                "middle" => {
                                    if let Some(_wid) = window_id {
                                        return crate::input::mouse::middle_click_at_xy_with_window_local_guarded(
                                            pid, screen_x, screen_y, win_local_x, win_local_y, &m, &action_gate,
                                        );
                                    }
                                    crate::input::mouse::middle_click_at_xy_guarded(
                                        pid, screen_x, screen_y, &m, &action_gate,
                                    )
                                }
                                // "left" (default) or anything else — preserve legacy left-click path.
                                _ => {
                                    // When we know the window_id, pass the window-local coordinates so
                                    // `click_at_xy_with_window_local` can stamp `CGEventSetWindowLocation`
                                    // and Chromium-specific fields (f40, f51, f58, f91, f92) onto events
                                    // for better backgrounded-target delivery.
                                    if let Some(wid) = window_id {
                                        if fg {
                                            return crate::input::mouse::click_at_xy_with_window_local_guarded(
                                                pid, screen_x, screen_y,
                                                win_local_x, win_local_y,
                                                wid, count, &m, &action_gate,
                                            );
                                        }
                                        return crate::input::mouse::click_at_xy_chromium_guarded(
                                            pid, screen_x, screen_y,
                                            win_local_x, win_local_y,
                                            wid, count, &m, &action_gate,
                                        );
                                    }
                                    crate::input::mouse::click_at_xy_guarded(
                                        pid, screen_x, screen_y, count, &m, &action_gate,
                                    )
                                }
                            }
                        };
                        // Foreground rung: brief front → click → restore.
                        // Returns whether the window was ACTUALLY fronted, so the
                        // reported `path` honestly reflects the rung that ran.
                        match (fg, window_id) {
                            (true, Some(wid)) => crate::input::skylight::with_foreground_assist_guarded(
                                pid as libc::pid_t, wid, &gate, do_click,
                            ),
                            _ => do_click().map(|_| false),
                        }
                    })
                    .await
                },
            )
            .await;

            let changes = snapshot.detect_async_for_args(&args).await;

            let button_label = match button_str.as_str() {
                "right" => "right-click",
                "middle" => "middle-click",
                _ => "click",
            };
            match result {
                Ok(Ok(fronted)) => {
                    // `with_foreground_assist` returns `false` when the fronting SPIs
                    // were unavailable and it clicked WITHOUT activation — report the
                    // background path in that case so `path` reflects the rung that ran.
                    let (path, mode_label) = if fg && fronted {
                        ("cgevent_fg", "foreground CGEvent")
                    } else {
                        ("cgevent", "background CGEvent")
                    };
                    ToolResult::text(format!(
                        "✅ Posted {button_label} to pid {pid} ({mode_label}; \
                         not driver-verified — confirm via screenshot).{}",
                        changes.result_suffix()
                    ))
                    .with_structured(serde_json::json!({ "path": path, "verified": false, "effect": "unverifiable" }))
                }
                Ok(Err(e)) => ToolResult::error(format!("{button_label} failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            ToolResult::error(
                "Provide either (element_index + window_id) or (x + y). pid is always required.",
            )
        }
    }
}

/// Side-effect-free validation performed before explicit watchable mode may
/// front the target. Runtime cache/AX failures remain ordinary dispatch
/// failures, but malformed addressing and delivery requests cannot steal focus.
fn click_dispatch_preflight(args: &Value) -> Result<(), ToolResult> {
    use cu_core::tool_args::ArgsExt;

    cu_core::tool::validate_dispatch_args(def(), args)?;
    let has_pid = args.get("pid").is_some_and(|value| !value.is_null());
    let has_window_id = args.get("window_id").is_some_and(|value| !value.is_null());
    let has_x = args.get("x").is_some_and(Value::is_number);
    let has_y = args.get("y").is_some_and(Value::is_number);

    if has_x && has_y && !has_pid && !has_window_id {
        return if args.str_or("scope", "window") == "desktop" {
            Ok(())
        } else {
            Err(ToolResult::error(
                "click: x,y given with no pid/window_id, but scope is \"window\". \
                 Screen-absolute clicks require desktop scope. Pass scope=\"desktop\" \
                 (and use get_desktop_state with scope=\"desktop\" to read true \
                 screen pixels) first."
                    .to_owned(),
            )
            .with_structured(serde_json::json!({
                "code": "desktop_scope_disabled",
                "scope": "window",
                "suggestion": "pass scope=\"desktop\"",
            })))
        };
    }

    let address = super::preflight_action_address(args, "click")?;
    let has_element_target = address.element && address.window_id.is_some();
    if !has_element_target && !address.has_xy {
        return Err(ToolResult::error(
            "Provide either (element_index + window_id) or (x + y). pid is always required.",
        ));
    }
    if !has_element_target && args.opt_str("debug_image_out").is_some() {
        if args.bool_or("from_zoom", false) {
            return Err(ToolResult::error(
                "debug_image_out is incompatible with from_zoom — received (x, y) \
                 would be in zoom-crop space, not window-local.",
            ));
        }
        if address.window_id.is_none() {
            return Err(ToolResult::error("debug_image_out requires window_id."));
        }
    }
    Ok(())
}

// ── AX click implementation (blocking) ───────────────────────────────────────

/// Returns `(summary_text, needs_webkit_delay, suspected_noop)`.
///
/// `suspected_noop` is true when the element did not advertise the action we
/// dispatched — AXUIElementPerformAction returns success regardless, so this is
/// the driver's only signal that the press likely did nothing. The caller turns
/// it into `effect: "suspected_noop"` + an escalation hint so the agent crosses
/// to the vision/pixel path instead of trusting a hollow success.
/// How a failed AX action is reported to the agent (code, effect, guidance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AxActionFailure {
    pub code: &'static str,
    /// `unknown`: the press may have landed; `not_landed`: it did not.
    pub effect: &'static str,
    pub message: String,
}

impl std::fmt::Display for AxActionFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AxActionFailure {}

/// A menu item the app shows dimmed right now: pressing it would do nothing.
fn dimmed_menu_item(
    role: &str,
    enabled: Option<&str>,
    idx: usize,
    title: &str,
) -> Option<AxActionFailure> {
    (role == "AXMenuItem" && enabled == Some("false")).then(|| AxActionFailure {
        code: "menu_item_dimmed",
        effect: "not_landed",
        message: format!(
            "[{idx}] AXMenuItem \"{title}\" is dimmed in the app's menu right now, so pressing \
             it would do nothing and it was not pressed. Apps dim a command that doesn't apply \
             at the moment, often while the app is in the background or nothing it acts on is \
             selected. Select what it should act on first, or press it with delivery_mode \
             \"foreground\"."
        ),
    })
}

/// Maps an `AXUIElementPerformAction` error to what the agent should do.
/// A press is not idempotent, so an error that leaves the effect unknown is
/// never retried here: the agent must observe before it acts again.
pub(crate) fn classify_ax_action_error(action: &str, err: i32) -> AxActionFailure {
    match err {
        // kAXErrorCannotComplete: the app did not answer in time. The action
        // may still have been delivered.
        -25204 => AxActionFailure {
            code: "ax_cannot_complete",
            effect: "unknown",
            message: format!(
                "{action} returned -25204 (the app did not answer in time). The action may \
                 or may not have landed: observe the window (get_window_state) before you \
                 retry, or a retry can act twice."
            ),
        },
        // kAXErrorActionUnsupported
        -25205 => AxActionFailure {
            code: "ax_action_unsupported",
            effect: "not_landed",
            message: format!(
                "{action} returned -25205: the element does not support this action. Use a \
                 pixel click (x,y) from the get_window_state screenshot."
            ),
        },
        // kAXErrorInvalidUIElement
        -25202 => AxActionFailure {
            code: "ax_element_invalid",
            effect: "not_landed",
            message: format!(
                "{action} returned -25202: the element is gone. Call get_window_state again \
                 and use the new element_index."
            ),
        },
        other => AxActionFailure {
            code: "ax_error",
            effect: "unknown",
            message: format!("{action} returned {other}. Observe the window before you retry."),
        },
    }
}

fn perform_ax_click(
    element_ptr: usize,
    idx: usize,
    pid: i32,
    window_id: u32,
    action_str: &str,
    cursor_key: &str,
    gate: &crate::dispatch_gate::NativeDispatchGate,
    in_background: bool,
) -> anyhow::Result<(String, bool, bool)> {
    let ax_action = map_action(action_str);
    let element = element_ptr as AXUIElementRef;

    // A dimmed menu item does nothing when pressed, yet the press still
    // reports success. In the background the app's own dimming is current;
    // brought to the front, it re-checks its menus only afterwards.
    if in_background && ax_action == "AXPress" {
        let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let enabled = unsafe { copy_stringified_attr(element, "AXEnabled") };
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        if let Some(failure) = dimmed_menu_item(&role, enabled.as_deref(), idx, &title) {
            return Err(failure.into());
        }
    }

    // Capture advertised actions BEFORE dispatching so we can detect silent no-ops
    // (AX returns success even when the element doesn't advertise the action).
    let advertised = unsafe { copy_action_names(element) };

    gate.check()?;
    let err = unsafe { crate::ax::bindings::perform_action(element, ax_action) };
    if err != crate::ax::bindings::kAXErrorSuccess {
        return Err(classify_ax_action_error(ax_action, err).into());
    }

    let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
    let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();

    let mut summary = format!("✅ Performed {ax_action} on [{idx}] {role} \"{title}\".");

    // AXPopUpButton: list available options, redirect to set_value.
    if role == "AXPopUpButton" {
        let children = unsafe { copy_children(element) };
        if !children.is_empty() {
            let options: Vec<String> = children
                .iter()
                .filter_map(|&child| {
                    let t = unsafe { copy_string_attr(child, "AXTitle") }.unwrap_or_default();
                    let v = unsafe { copy_string_attr(child, "AXValue") }.unwrap_or_default();
                    if t.is_empty() && v.is_empty() {
                        return None;
                    }
                    Some(if v.is_empty() || v == t {
                        format!("\"{t}\"")
                    } else {
                        format!("\"{t}\" (value: {v})")
                    })
                })
                .collect();
            for &child in &children {
                unsafe {
                    CFRelease(child as _);
                }
            }

            if !options.is_empty() {
                let opt_list = options.join(", ");
                summary.push_str(
                    "\n\n⚠️ This is a popup/select button. The native macOS menu closes \
                     immediately when the window is in the background. Do NOT use click \
                     again — instead, use:\n  set_value(pid, window_id, element_index, value)\n\
                     Available options: [",
                );
                summary.push_str(&opt_list);
                summary.push(']');
            }
        }
    }

    // Advertised-action warning: non-fatal but surfaces likely no-ops. Also the
    // machine-readable `suspected_noop` signal returned to the caller.
    let suspected_noop = !advertised.contains(&ax_action.to_string());
    if suspected_noop {
        let adv_list = if advertised.is_empty() {
            "none".into()
        } else {
            advertised.join(", ")
        };
        summary.push_str(&format!(
            "\n⚠️ Element does not advertise {ax_action} (actions: {adv_list}). \
             Action may have been a no-op."
        ));
    }

    // WebKit DOM focus settle: 800 ms for text inputs (returned to async caller).
    let needs_webkit_delay =
        ax_action == "AXPress" && (role == "AXTextField" || role == "AXTextArea");

    // Show focus-rect highlight around the element (matches Swift showFocusRect).
    // Also move the cursor to the element center so the glide animation plays.
    if let Some(rect) = unsafe { element_screen_rect(element) } {
        // Drive THIS session's cursor (threaded in via `cursor_key`), matching
        // the keyed glide already played in the invoke body above. The keyed
        // glide already played on the session's cursor in the invoke body.
        crate::cursor::overlay::send_command(
            cursor_key.to_owned(),
            cursor_overlay::OverlayCommand::ShowFocusRect(Some(rect)),
        );
        // Animate cursor to element center.
        let cx = rect[0] + rect[2] / 2.0;
        let cy = rect[1] + rect[3] / 2.0;
        crate::cursor::overlay::send_command(
            cursor_key.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: cx, y: cy },
        );
    }
    let _ = pid;
    let _ = window_id; // used by caller context

    Ok((summary, needs_webkit_delay, suspected_noop))
}

fn map_action(action: &str) -> &'static str {
    match action.to_lowercase().as_str() {
        "press" | "click" => "AXPress",
        "show_menu" | "right_click" => "AXShowMenu",
        "pick" => "AXPick",
        "confirm" => "AXConfirm",
        "cancel" => "AXCancel",
        "open" => "AXOpen",
        _ => "AXPress",
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[test]
    fn ax_cannot_complete_reports_an_unknown_effect_and_asks_to_observe() {
        let failure = super::classify_ax_action_error("AXPress", -25204);
        assert_eq!(failure.code, "ax_cannot_complete");
        assert_eq!(failure.effect, "unknown");
        assert!(failure.message.contains("observe"), "{}", failure.message);
        assert!(failure.message.contains("AXPress"), "{}", failure.message);
    }

    #[test]
    fn a_dimmed_menu_item_is_not_pressed_and_says_so() {
        let failure = super::dimmed_menu_item("AXMenuItem", Some("false"), 223, "Bold").unwrap();
        assert_eq!(failure.effect, "not_landed");
        assert!(
            failure.message.contains("\"Bold\" is dimmed"),
            "{}",
            failure.message
        );
        assert!(super::dimmed_menu_item("AXMenuItem", Some("true"), 1, "Bold").is_none());
        assert!(super::dimmed_menu_item("AXMenuItem", None, 1, "Bold").is_none());
        assert!(super::dimmed_menu_item("AXButton", Some("false"), 1, "OK").is_none());
    }

    #[test]
    fn unsupported_and_invalid_elements_did_not_land() {
        let unsupported = super::classify_ax_action_error("AXPress", -25205);
        assert_eq!(
            (unsupported.code, unsupported.effect),
            ("ax_action_unsupported", "not_landed")
        );
        let invalid = super::classify_ax_action_error("AXPress", -25202);
        assert_eq!(
            (invalid.code, invalid.effect),
            ("ax_element_invalid", "not_landed")
        );
        assert!(
            invalid.message.contains("get_window_state"),
            "{}",
            invalid.message
        );
        let other = super::classify_ax_action_error("AXPress", -25200);
        assert_eq!((other.code, other.effect), ("ax_error", "unknown"));
    }

    use super::*;

    /// Surface 5: schema must advertise the new `button` field with the three
    /// canonical values and default to "left". Agent clients
    /// consumers branch on this enum being present.
    #[test]
    fn schema_advertises_button_enum() {
        let d = def();
        let props = d.input_schema.get("properties").expect("properties");
        let button = props.get("button").expect("button field present");
        let kind = button.get("type").and_then(|v| v.as_str());
        assert_eq!(kind, Some("string"));
        let enum_vals: Vec<&str> = button
            .get("enum")
            .and_then(|v| v.as_array())
            .expect("button.enum present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(enum_vals.contains(&"left"));
        assert!(enum_vals.contains(&"right"));
        assert!(enum_vals.contains(&"middle"));
    }

    /// Surface 5 hard constraint: the tool description must mention the
    /// `button` argument and the "left" default so MCP introspection (which
    /// pipes description into LLM prompts) carries the back-compat note.
    #[test]
    fn description_mentions_button_default() {
        let d = def();
        let desc = d.description.to_ascii_lowercase();
        assert!(
            desc.contains("button"),
            "description should mention button arg"
        );
        assert!(
            desc.contains("left"),
            "description should mention left default"
        );
        assert!(
            desc.contains("middle"),
            "description should mention middle button"
        );
    }

    /// Existing default behaviour preserved: no `button` field on the call →
    /// resolves to "left" inside invoke. We can't drive the AX path without a
    /// live macOS Window Server, but we CAN check the same arg-parsing logic
    /// the invoke uses produces "left" for empty / absent input.
    #[test]
    fn button_defaults_to_left_when_absent() {
        use cu_core::tool_args::ArgsExt;
        let args = serde_json::json!({ "pid": 1234 });
        let button_str_raw = args.str_or("button", "left").to_lowercase();
        let resolved = if button_str_raw.is_empty() {
            "left".to_string()
        } else {
            button_str_raw
        };
        assert_eq!(resolved, "left");
    }

    /// Round-trip the three canonical values through the same parse the invoke
    /// uses, so any future refactor that changes str_or semantics breaks here
    /// before it breaks consumers.
    #[test]
    fn button_round_trips_right_and_middle() {
        use cu_core::tool_args::ArgsExt;
        for v in ["left", "right", "middle"] {
            let args = serde_json::json!({ "pid": 1234, "button": v });
            let s = args.str_or("button", "left").to_lowercase();
            assert_eq!(s, v);
        }
    }
}
