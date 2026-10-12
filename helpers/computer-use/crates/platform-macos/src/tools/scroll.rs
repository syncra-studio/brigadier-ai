use async_trait::async_trait;
use core_foundation::base::{CFRelease, CFTypeRef};
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_children, copy_element_attr, copy_number_attr, copy_range_attr, copy_string_attr,
    copy_stringified_attr, element_at_screen_position, element_screen_center,
    focused_element_of_pid, kAXErrorSuccess, perform_action, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;

use super::ToolState;

/// Per-notch pixel step for the wheel path. `page` rolls a screenful-ish chunk,
/// `line` a few text lines — tuned to feel like a real wheel notch. Runtime
/// tuning deferred (host is screen-recording); centralized here for one-line edits.
const WHEEL_STEP_LINE_PX: i32 = 120;
const WHEEL_STEP_PAGE_PX: i32 = 600;

/// Resolved pixel-wheel target in screen space, plus optional window-local
/// stamp + window id for backgrounded delivery.
struct WheelTarget {
    screen_x: f64,
    screen_y: f64,
    win_local: Option<(f64, f64)>,
    wid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq)]
enum ScrollMetric {
    Number(f64),
    Range { location: isize, length: isize },
    Text(String),
}

impl ScrollMetric {
    fn json(&self) -> serde_json::Value {
        match self {
            Self::Number(value) => serde_json::json!(value),
            Self::Range { location, length } => {
                serde_json::json!({ "location": location, "length": length })
            }
            Self::Text(value) => serde_json::json!(value),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ScrollObservation {
    attribute: String,
    metric: ScrollMetric,
}

/// Read a cheap scroll-position signal from an element or its first scroll-area
/// ancestor. The caller owns/retains `element` for the duration of this call.
unsafe fn read_scroll_observation(
    element: AXUIElementRef,
    direction: &str,
) -> Option<ScrollObservation> {
    if element.is_null() {
        return None;
    }
    core_foundation::base::CFRetain(element as CFTypeRef);
    let mut current = element;
    let mut value_fallback = None;

    for depth in 0..12 {
        if let Some(range) = copy_range_attr(current, "AXVisibleCharacterRange") {
            CFRelease(current as CFTypeRef);
            return Some(ScrollObservation {
                attribute: "AXVisibleCharacterRange".to_owned(),
                metric: ScrollMetric::Range {
                    location: range.location,
                    length: range.length,
                },
            });
        }

        let role = copy_string_attr(current, "AXRole").unwrap_or_default();
        if role == "AXScrollArea" {
            let scrollbar_attr = if matches!(direction, "left" | "right") {
                "AXHorizontalScrollBar"
            } else {
                "AXVerticalScrollBar"
            };
            if let Some(scrollbar) = copy_element_attr(current, scrollbar_attr) {
                let metric = copy_number_attr(scrollbar, "AXValue")
                    .map(ScrollMetric::Number)
                    .or_else(|| {
                        copy_stringified_attr(scrollbar, "AXValue")
                            .map(crate::ax::tree::truncate_ax_value)
                            .map(ScrollMetric::Text)
                    });
                CFRelease(scrollbar as CFTypeRef);
                if let Some(metric) = metric {
                    CFRelease(current as CFTypeRef);
                    return Some(ScrollObservation {
                        attribute: format!("{scrollbar_attr}.AXValue"),
                        metric,
                    });
                }
            }
        }

        if depth == 0 {
            value_fallback = copy_number_attr(current, "AXValue")
                .map(ScrollMetric::Number)
                .or_else(|| {
                    copy_stringified_attr(current, "AXValue")
                        .filter(|value| !value.trim().is_empty())
                        .map(crate::ax::tree::truncate_ax_value)
                        .map(ScrollMetric::Text)
                })
                .map(|metric| ScrollObservation {
                    attribute: "AXValue".to_owned(),
                    metric,
                });
        }

        let parent = copy_element_attr(current, "AXParent");
        CFRelease(current as CFTypeRef);
        let Some(parent) = parent else {
            return value_fallback;
        };
        current = parent;
    }
    CFRelease(current as CFTypeRef);
    value_fallback
}

unsafe fn read_scroll_observation_at_point(
    pid: i32,
    screen_x: f64,
    screen_y: f64,
    direction: &str,
) -> Option<ScrollObservation> {
    let element = element_at_screen_position(pid, screen_x, screen_y)?;
    let observation = read_scroll_observation(element, direction);
    CFRelease(element as CFTypeRef);
    observation
}

async fn observe_scroll_target(
    pid: i32,
    element_ptr: Option<usize>,
    screen_x: f64,
    screen_y: f64,
    direction: String,
) -> Option<ScrollObservation> {
    tokio::task::spawn_blocking(move || unsafe {
        match element_ptr {
            Some(ptr) => read_scroll_observation(ptr as AXUIElementRef, &direction),
            None => read_scroll_observation_at_point(pid, screen_x, screen_y, &direction),
        }
    })
    .await
    .ok()
    .flatten()
}

async fn observe_focused_scroll_target(pid: i32, direction: String) -> Option<ScrollObservation> {
    tokio::task::spawn_blocking(move || unsafe {
        let element = focused_element_of_pid(pid)?;
        let observation = read_scroll_observation(element, &direction);
        CFRelease(element as CFTypeRef);
        observation
    })
    .await
    .ok()
    .flatten()
}

fn scroll_verification(
    before: Option<ScrollObservation>,
    after: Option<ScrollObservation>,
) -> serde_json::Value {
    let (verified, delta, effect) = match (&before, &after) {
        (Some(before), Some(after)) if before.attribute == after.attribute => {
            let delta = match (&before.metric, &after.metric) {
                (ScrollMetric::Number(before), ScrollMetric::Number(after)) => {
                    serde_json::json!(after - before)
                }
                (
                    ScrollMetric::Range {
                        location: before_location,
                        length: before_length,
                    },
                    ScrollMetric::Range {
                        location: after_location,
                        length: after_length,
                    },
                ) => serde_json::json!({
                    "location": after_location - before_location,
                    "length": after_length - before_length,
                }),
                (ScrollMetric::Text(before), ScrollMetric::Text(after)) => {
                    serde_json::json!({ "changed": before != after })
                }
                _ => serde_json::Value::Null,
            };
            let verified = before.metric != after.metric;
            (
                verified,
                delta,
                if verified {
                    "verified"
                } else {
                    "no_observed_change"
                },
            )
        }
        _ => (false, serde_json::Value::Null, "unobservable"),
    };
    serde_json::json!({
        "verified": verified,
        "effect": effect,
        "observed_delta": delta,
        "scroll_observation": {
            "attribute": before.as_ref().map(|value| value.attribute.as_str())
                .or_else(|| after.as_ref().map(|value| value.attribute.as_str())),
            "before": before.as_ref().map(|value| value.metric.json()),
            "after": after.as_ref().map(|value| value.metric.json()),
        }
    })
}

pub struct ScrollTool {
    state: Arc<ToolState>,
}

impl ScrollTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// Wheel notches or keystroke repetitions accepted in one call. The input
/// schema advertises this range and every delivery path enforces it (ported
/// from upstream the upstream driver 990a2d1f1: an unclamped `amount: 1100` spent 117 s
/// in the keystroke loop).
const AMOUNT_MIN: u64 = 1;
const AMOUNT_MAX: u64 = 50;

fn clamp_amount(requested: u64) -> usize {
    requested.clamp(AMOUNT_MIN, AMOUNT_MAX) as usize
}

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "scroll".into(),
        description: "Scroll the target pid. Two paths, picked by how you address the scroll:\n\n\
            • **Targeted wheel path** — when you pass a target, either \
            `element_index`/`element_token` (preferred) or window-local `x, y` pixels: \
            the driver synthesizes a real mouse-wheel event (CGEventCreateScrollWheelEvent) \
            at that screen point. The renderer hit-tests the wheel at the \
            cursor, so the scroll lands on whatever element is under the point — exactly \
            like physically rolling the wheel over it. This is the ONLY way to scroll a \
            nested `overflow:auto` region (e.g. a scrollable <div> with no tabindex): such \
            regions never take keyboard focus, so the keystroke path below no-ops on them. \
            Use this for inner/nested scrollers in web views. Background wheel dispatch \
            with pid+window_id is refused with error=`obstructed`, \
            code=`background_occluded` when another window owns the point. After \
            dispatch the driver reads AXValue, visible range, or \
            the first scroll-area ancestor and returns `verified` plus `observed_delta` \
            whenever the app exposes a cheap scroll metric.\n\n\
            • **Keystroke path (focused region)** — when you pass NO target (just pid + \
            direction): synthesizes PageDown/PageUp (by='page') or Down/Up arrows \
            (by='line'); horizontal uses Left/Right arrows. Drives the focused / page \
            scroller only.\n\n\
            Mapping: by='page' → larger step; by='line' → smaller step; amount = number of \
            wheel notches (targeted path) or keystroke repetitions (keystroke path).".into(),
        input_schema: serde_json::json!({
            "type": "object",
            // `pid` conditionally required (validated in code), not pinned in the
            // schema — keeps the contract consistent across platforms.
            "required": ["direction"],
            "properties": {
                "session": { "type": "string", "description": "Optional explicit session id for the agent cursor and per-session state. Embedded MCP calls may omit it to use BRIGADIER_CU_DEFAULT_SESSION (or embedded-<pid>); anonymous non-embedded calls remain cursor-less." },
                "pid": { "type": "integer" },
                "direction": {
                    "type": "string",
                    "enum": ["up", "down", "left", "right"],
                    "description": "Scroll direction."
                },
                "by": {
                    "type": "string",
                    "enum": ["line", "page"],
                    "description": "Scroll granularity. Default: line."
                },
                "amount": {
                    "type": "integer",
                    "minimum": AMOUNT_MIN,
                    "maximum": AMOUNT_MAX,
                    "description": "Pixel-wheel path: number of wheel notches. Keystroke path: number of keystroke repetitions. Larger requests are clamped to the maximum. Default: 3."
                },
                "window_id": { "type": "integer" },
                "element_index": { "type": "integer", "description": "Element from last get_window_state. Routes through the pixel-wheel path AT this element's center — use it to scroll a nested overflow region you located in the AX tree." },
                "element_token": { "type": "string", "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded. Routes through the pixel-wheel path at the element's center." },
                "x": { "type": "number", "description": "Window-local screenshot X (top-left origin of the PNG from get_window_state). With `y`, routes through the pixel-wheel path at this point — use for a scrollable surface that isn't in the AX tree. Requires window_id to anchor the window→screen conversion." },
                "y": { "type": "number", "description": "Window-local screenshot Y. See `x`." },
                "delivery_mode": cu_core::tool_schema::delivery_mode_schema_with(
                    "Default background. Background targeted-wheel dispatch with pid+window_id checks WindowServer Z order and returns structured error=\"obstructed\", code=\"background_occluded\" without posting when another window owns the target point. Foreground fronts the target, skips that check, scrolls, then restores the prior app. Successful scrolls report verified plus observed_delta when AX exposes a readable scroll metric."
                )
            },
            "additionalProperties": false
        }),
        read_only: false,
        destructive: false,
        idempotent: false,
        open_world: true,
    })
}

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    fn dispatch_preflight(&self, args: &Value) -> Result<(), ToolResult> {
        cu_core::tool::validate_dispatch_args(self.def(), args)?;
        let address = super::preflight_action_address(args, "scroll")?;
        if address.element && address.window_id.is_none() {
            return Err(ToolResult::error(
                "window_id is required when element_index is used.",
            ));
        }
        if address.has_xy && address.window_id.is_none() {
            return Err(ToolResult::error(
                "window_id is required when scrolling by window-local x,y pixels.",
            ));
        }
        use cu_core::tool_args::ArgsExt;
        let pid = args.require_i32("pid")?;
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        if !delivery_mode.is_foreground() && crate::browser::ElectronJs::is_electron(pid) {
            return Err(ToolResult::error(
                "Background scroll is unavailable for Electron/Chromium windows on macOS."
                    .to_owned(),
            )
            .with_structured(serde_json::json!({ "code": "background_unavailable" })));
        }
        Ok(())
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let dispatch_gate = crate::dispatch_gate::NativeDispatchGate::for_args(&args);
        // delivery_mode: foreground briefly fronts the window before the
        // pixel-wheel dispatch (the explicit last resort for surfaces that drop
        // background CGEvents). Only the pixel-wheel path honors it; the
        // keystroke path is background-by-design and untouched.
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());
        if !delivery_mode.is_foreground() && crate::browser::ElectronJs::is_electron(pid) {
            return ToolResult::error(
                "Background scroll is unavailable for Electron/Chromium windows on macOS."
                    .to_owned(),
            )
            .with_structured(serde_json::json!({ "code": "background_unavailable" }));
        }
        let direction = match args.require_str("direction") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let by = args.str_or("by", "line");
        let amount = clamp_amount(args.u64_or("amount", 3));
        // Surface 6: element_token / element_index precedence.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cu_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "scroll",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (element_index, window_id) = match resolved {
            cu_core::element_token::ResolvedElement::None => (None, window_id_arg),
            cu_core::element_token::ResolvedElement::Element {
                window_id: wid,
                element_index: idx,
                via_token: _,
            } => (Some(idx), wid),
        };

        // Resolve the pre-focus element pointer (if requested) outside
        // the suppression closure — only the focus_element() write itself
        // needs to run under suppression, the cache lookup does not.
        // Retain out of the cache so a concurrent get_window_state can't free
        // the element before the suppressed focus below dereferences it
        // (use-after-free → daemon crash). Guard lives to method end.
        let pre_focus_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            self.state.element_cache.get_element_retained(pid, wid, idx)
        } else {
            None
        };
        let pre_focus_ptr: Option<usize> = pre_focus_guard.as_ref().map(|g| g.as_ptr());

        // An element target was explicitly requested (element_index/element_token)
        // but couldn't be retained from the cache — the snapshot is stale. Fail
        // loudly instead of silently falling through to the keystroke/page
        // scroller below, which would scroll the wrong thing.
        if let Some(idx) = element_index {
            if pre_focus_guard.is_none() {
                return ToolResult::error(format!(
                    "Element index {idx} not found. Call get_window_state first."
                ));
            }
        }

        // An element-addressed scroll may complete through the native AX
        // scrollbar path below, before the targeted-wheel code gets a chance
        // to place the cursor. Glide now so every element scroll is visible at
        // its action point; the wheel path may refine the point after
        // AXScrollToVisible changes the element frame.
        if let Some(element_ptr) = pre_focus_ptr {
            if let Some(point) = super::cursor_tools::retained_element_center(element_ptr).await {
                super::cursor_tools::animate_to_action_point(&self.state, &args, point, window_id)
                    .await;
            }
        }

        // AppKit exposes vertical scroll-bar buttons beneath the text area's
        // AXScrollArea parent. Pressing those controls is a true
        // background-safe scroll: no activation, z-order change, or cursor move.
        if matches!(direction.as_str(), "up" | "down") {
            if let (Some(index), Some(wid)) = (element_index, window_id) {
                let native_element_guard = self
                    .state
                    .element_cache
                    .get_element_retained(pid, wid, index);
                let direction_for_ax = direction.clone();
                let by_for_ax = by.clone();
                let foreground = delivery_mode.is_foreground();
                let ax_result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
                    let Some(element_guard) = native_element_guard else {
                        return Ok((false, false, None, None));
                    };
                    let element = element_guard.as_ptr() as AXUIElementRef;
                    let before = unsafe { read_scroll_observation(element, &direction_for_ax) };
                    if foreground {
                        let mut delivered = false;
                        let fronted = crate::input::skylight::with_foreground_assist(
                            pid as libc::pid_t,
                            wid,
                            || {
                                delivered = unsafe {
                                    scroll_native_text_area(
                                        element,
                                        &direction_for_ax,
                                        &by_for_ax,
                                        amount,
                                    )
                                };
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                Ok(())
                            },
                        )?;
                        let after = unsafe { read_scroll_observation(element, &direction_for_ax) };
                        Ok((delivered, fronted, before, after))
                    } else {
                        let delivered = unsafe {
                            scroll_native_text_area(element, &direction_for_ax, &by_for_ax, amount)
                        };
                        std::thread::sleep(std::time::Duration::from_millis(80));
                        let after = unsafe { read_scroll_observation(element, &direction_for_ax) };
                        Ok((delivered, false, before, after))
                    }
                })
                .await;
                match ax_result {
                    Ok(Ok((true, fronted, before, after))) => {
                        let mut structured = scroll_verification(before, after);
                        structured["path"] =
                            serde_json::json!(if fronted { "ax_fg" } else { "ax" });
                        return ToolResult::text(format!(
                        "✅ Scrolled native macOS control {direction} by {by} × {amount} through AX."
                    ))
                    .with_structured(structured);
                    }
                    Ok(Ok((false, _, _, _))) => {}
                    Ok(Err(error)) => {
                        return ToolResult::error(format!("Native AX scroll failed: {error}"));
                    }
                    Err(error) => {
                        return ToolResult::error(format!("Native AX scroll task failed: {error}"));
                    }
                }
            }
        }

        // ── Targeted wheel path ─────────────────────────────────────────────
        // A target — element (preferred) OR window-local x,y — routes the scroll
        // through a synthesized mouse-wheel event at that screen point, so the
        // renderer's hit-test delivers it to whatever element is under the
        // cursor. This is the ONLY way to scroll a nested overflow:auto region
        // that never takes keyboard focus (the keystroke path below no-ops on
        // it). No user-facing flag: presence of a target IS the switch.
        let x_arg = args
            .opt_f64("x")
            .or_else(|| args.opt_i64("x").map(|v| v as f64));
        let y_arg = args
            .opt_f64("y")
            .or_else(|| args.opt_i64("y").map(|v| v as f64));

        // Per-notch step + direction→delta mapping (sign convention lives
        // here; the mouse primitive stays sign-agnostic). macOS: +y reveals
        // content ABOVE, -y reveals BELOW; +x reveals LEFT, -x reveals RIGHT.
        let step = if by == "page" {
            WHEEL_STEP_PAGE_PX
        } else {
            WHEEL_STEP_LINE_PX
        };
        let (delta_y, delta_x): (i32, i32) = match direction.as_str() {
            "down" => (-step, 0),
            "up" => (step, 0),
            "right" => (0, -step),
            "left" => (0, step),
            _ => (-step, 0),
        };

        // Resolve a screen-space wheel target, if a target was supplied.
        let wheel_target: Option<WheelTarget> = if let Some(element_ptr) = pre_focus_ptr {
            // Element path: wheel at the element's screen-space center. Both AX
            // coordinates and window bounds are logical top-left points, so no
            // Retina scaling is needed here.
            let wid = window_id;
            let gate = dispatch_gate.clone();
            tokio::task::spawn_blocking(move || {
                // Web content can be present in AX while its frame is below
                // the outer page viewport. Ask the accessibility hierarchy to
                // reveal the target before taking the screen-space center;
                // otherwise the wheel is posted outside the rendered window
                // and nested overflow regions never receive it.
                unsafe {
                    if gate.check().is_ok() {
                        let _ = crate::ax::bindings::perform_action(
                            element_ptr as AXUIElementRef,
                            "AXScrollToVisible",
                        );
                    } else {
                        return None;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(40));
                let center = unsafe { element_screen_center(element_ptr as AXUIElementRef) };
                center.map(|(cx, cy)| {
                    let win_local = wid
                        .and_then(crate::windows::window_bounds_by_id)
                        .map(|b| (cx - b.x, cy - b.y));
                    WheelTarget {
                        screen_x: cx,
                        screen_y: cy,
                        win_local,
                        wid,
                    }
                })
            })
            .await
            .ok()
            .flatten()
        } else if let (Some(mut cx), Some(mut cy)) = (x_arg, y_arg) {
            // Targeted x,y are window-local screenshot pixels and REQUIRE a
            // window_id to anchor the window→screen conversion (schema contract).
            // Without one, refuse rather than scrolling at screen-absolute coords.
            if window_id.is_none() {
                return ToolResult::error(
                    "window_id is required when scrolling by window-local x,y pixels.".to_string(),
                );
            }
            // Pixel path: x,y are window-local screenshot pixels. Mirror the
            // click pixel path — undo any session downscale, then add the
            // window origin and divide out the Retina backing scale.
            if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                cx *= ratio;
                cy *= ratio;
            }
            let wid = window_id;
            tokio::task::spawn_blocking(move || {
                if let Some(wid) = wid {
                    let bounds = crate::windows::window_bounds_by_id(wid);
                    let scale: f64 = if let Some(ref b) = bounds {
                        if let Ok(png) = crate::capture::screenshot_window_bytes(wid) {
                            if png.len() >= 24 {
                                let pw =
                                    u32::from_be_bytes([png[16], png[17], png[18], png[19]]) as f64;
                                if b.width > 0.0 && pw > b.width {
                                    pw / b.width
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
                    if let Some(b) = bounds {
                        let (wx, wy) = (cx / scale, cy / scale);
                        return WheelTarget {
                            screen_x: b.x + wx,
                            screen_y: b.y + wy,
                            win_local: Some((wx, wy)),
                            wid: Some(wid),
                        };
                    }
                }
                // No window_id → treat x,y as screen coordinates.
                WheelTarget {
                    screen_x: cx,
                    screen_y: cy,
                    win_local: None,
                    wid: None,
                }
            })
            .await
            .ok()
        } else {
            None
        };

        if let Some(target) = wheel_target {
            if !delivery_mode.is_foreground() {
                if let Some(wid) = target.wid {
                    if let Some(error) = super::pixel_obstruction_error(
                        pid,
                        wid,
                        target.screen_x,
                        target.screen_y,
                        None,
                    ) {
                        return error;
                    }
                }
            }
            let cursor_key = super::cursor_tools::resolve_cursor_key(&args);
            // Pin + glide the agent-cursor overlay to the target for visibility
            // (overlay only — does NOT move the hardware cursor). Mirrors click.
            if let Some(wid) = target.wid {
                crate::cursor::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(wid as u64),
                );
            }
            crate::cursor::overlay::animate_cursor_for_action(
                cursor_key.clone(),
                target.screen_x,
                target.screen_y,
                &args,
            )
            .await;
            self.state.cursor_registry.update_position(
                &cursor_key,
                target.screen_x,
                target.screen_y,
            );

            let prior_front = apps::frontmost_pid();
            let snapshot = WindowChangeDetector::snapshot_for_session(
                prior_front,
                &super::cursor_tools::resolve_cursor_key(&args),
            );

            let WheelTarget {
                screen_x,
                screen_y,
                win_local,
                wid,
            } = target;
            let probe_ptr = pre_focus_ptr;
            let before =
                observe_scroll_target(pid, probe_ptr, screen_x, screen_y, direction.clone()).await;
            let amount_ticks = amount;
            let fg = delivery_mode.is_foreground() && wid.is_some();
            let gate = dispatch_gate.clone();
            let result = focus_guard::with_focus_suppressed(
                Some(pid),
                prior_front,
                "scroll.CGScrollWheel",
                || async move {
                    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                        let action_gate = gate.clone();
                        let do_it = move || -> anyhow::Result<()> {
                            crate::input::mouse::scroll_wheel_at_xy_guarded(
                                pid,
                                screen_x,
                                screen_y,
                                win_local,
                                wid,
                                delta_y,
                                delta_x,
                                amount_ticks,
                                &action_gate,
                            )
                        };
                        // Foreground rung: brief front → wheel → restore prior frontmost.
                        match (fg, wid) {
                            (true, Some(w)) => {
                                crate::input::skylight::with_foreground_assist_guarded(
                                    pid as libc::pid_t,
                                    w,
                                    &gate,
                                    do_it,
                                )?;
                                Ok(())
                            }
                            _ => do_it(),
                        }
                    })
                    .await
                },
            )
            .await;

            let changes = snapshot.detect_async_for_args(&args).await;
            let mode_label = if fg {
                " (delivery_mode:foreground)"
            } else {
                ""
            };
            return match result {
                Ok(Ok(())) => {
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                    let after = observe_scroll_target(
                        pid,
                        probe_ptr,
                        screen_x,
                        screen_y,
                        direction.clone(),
                    )
                    .await;
                    let mut structured = scroll_verification(before, after);
                    structured["path"] =
                        serde_json::json!(if fg { "cgevent_fg" } else { "cgevent" });
                    let verified = structured["verified"].as_bool().unwrap_or(false);
                    let delta = &structured["observed_delta"];
                    ToolResult::text(format!(
                        "✅ Sent {direction} scroll by {by} × {amount} via pixel wheel at \
                         ({screen_x:.0}, {screen_y:.0}){mode_label}; verified={verified}, \
                         observed_delta={delta}.{}",
                        changes.result_suffix()
                    ))
                    .with_structured(structured)
                }
                Ok(Err(e)) => ToolResult::error(format!("Wheel scroll failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let key = match (by.as_str(), direction.as_str()) {
            ("page", "down") | (_, "down") if by == "page" => "pagedown",
            ("page", "up") | (_, "up") if by == "page" => "pageup",
            ("line", "down") | (_, "down") => "down",
            ("line", "up") | (_, "up") => "up",
            (_, "left") => "left",
            (_, "right") => "right",
            _ => "down",
        };
        let key = key.to_owned();

        // The untargeted keystroke path acts on the currently focused region.
        // When AX exposes its bounds, make that implicit action point visible.
        if pre_focus_ptr.is_none() {
            if let Some(point) = super::cursor_tools::focused_element_center(pid).await {
                super::cursor_tools::animate_to_action_point(&self.state, &args, point, window_id)
                    .await;
            }
        }

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Scroll keystrokes (PageDown / arrow) into search-box autocomplete
        // can spawn floating helper windows; rare but real. Wrap for parity
        // with the other action tools.
        //
        // The AX focus_element() pre-write also runs inside the closure so
        // any reflex activations it triggers are caught by both the wildcard
        // snapshot suppressor and the targeted FocusGuard lease.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot_for_session(
            prior_front,
            &super::cursor_tools::resolve_cursor_key(&args),
        );
        let focus_gate = dispatch_gate.clone();
        let key_gate = dispatch_gate.clone();
        let before = if window_id.is_some() {
            observe_focused_scroll_target(pid, direction.clone()).await
        } else {
            None
        };

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "scroll.CGEvent",
            || async move {
                // Pre-focus the element under suppression so its
                // side-effects are captured by the snapshot + lease.
                if let Some(element_ptr) = pre_focus_ptr {
                    let _ = tokio::task::spawn_blocking(move || {
                        crate::input::ax_actions::focus_element_guarded(element_ptr, &focus_gate)
                    })
                    .await;
                    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                }

                tokio::task::spawn_blocking(move || {
                    for _ in 0..amount {
                        if let Err(e) =
                            crate::input::keyboard::press_key_guarded(pid, &key, &[], &key_gate)
                        {
                            return Err(e);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Ok(())
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async_for_args(&args).await;

        match result {
            Ok(Ok(())) => {
                let after = if window_id.is_some() {
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                    observe_focused_scroll_target(pid, direction.clone()).await
                } else {
                    None
                };
                let mut structured = scroll_verification(before, after);
                structured["path"] = serde_json::json!("key_events");
                let verified = structured["verified"].as_bool().unwrap_or(false);
                let delta = &structured["observed_delta"];
                ToolResult::text(format!(
                    "✅ Sent {direction} scroll by {by} × {amount} via keystroke; \
                     verified={verified}, observed_delta={delta}.{}",
                    changes.result_suffix()
                ))
                .with_structured(structured)
            }
            Ok(Err(e)) => ToolResult::error(format!("Scroll failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

unsafe fn scroll_native_text_area(
    element: AXUIElementRef,
    direction: &str,
    by: &str,
    amount: usize,
) -> bool {
    if copy_string_attr(element, "AXRole").as_deref() != Some("AXTextArea") {
        return false;
    }
    let Some(scroll_area) = copy_element_attr(element, "AXParent") else {
        return false;
    };
    if copy_string_attr(scroll_area, "AXRole").as_deref() != Some("AXScrollArea") {
        CFRelease(scroll_area as CFTypeRef);
        return false;
    }
    let mut buttons = Vec::new();
    collect_ax_buttons(scroll_area, 0, &mut buttons);
    CFRelease(scroll_area as CFTypeRef);
    if buttons.is_empty() {
        return false;
    }

    let reverse = direction == "up";
    let base = if by == "page" && buttons.len() >= 4 {
        2
    } else {
        0
    };
    let index = base + usize::from(reverse);
    let mut delivered = false;
    if let Some(target) = buttons.get(index).copied() {
        for _ in 0..amount.max(1) {
            if perform_action(target, "AXPress") != kAXErrorSuccess {
                break;
            }
            delivered = true;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    for button in buttons {
        CFRelease(button as CFTypeRef);
    }
    delivered
}

unsafe fn collect_ax_buttons(
    element: AXUIElementRef,
    depth: usize,
    buttons: &mut Vec<AXUIElementRef>,
) {
    if depth >= 4 || buttons.len() >= 4 {
        return;
    }
    for child in copy_children(element) {
        if buttons.len() >= 4 {
            CFRelease(child as CFTypeRef);
        } else if copy_string_attr(child, "AXRole").as_deref() == Some("AXButton") {
            buttons.push(child);
        } else {
            collect_ax_buttons(child, depth + 1, buttons);
            CFRelease(child as CFTypeRef);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(attribute: &str, metric: ScrollMetric) -> ScrollObservation {
        ScrollObservation {
            attribute: attribute.to_owned(),
            metric,
        }
    }

    #[test]
    fn verification_reports_numeric_delta() {
        let result = scroll_verification(
            Some(observation(
                "AXVerticalScrollBar.AXValue",
                ScrollMetric::Number(0.25),
            )),
            Some(observation(
                "AXVerticalScrollBar.AXValue",
                ScrollMetric::Number(0.75),
            )),
        );
        assert_eq!(result["verified"], true);
        assert_eq!(result["observed_delta"], 0.5);
    }

    #[test]
    fn verification_reports_visible_range_delta() {
        let result = scroll_verification(
            Some(observation(
                "AXVisibleCharacterRange",
                ScrollMetric::Range {
                    location: 10,
                    length: 40,
                },
            )),
            Some(observation(
                "AXVisibleCharacterRange",
                ScrollMetric::Range {
                    location: 30,
                    length: 35,
                },
            )),
        );
        assert_eq!(result["verified"], true);
        assert_eq!(result["observed_delta"]["location"], 20);
        assert_eq!(result["observed_delta"]["length"], -5);
    }

    #[test]
    fn verification_is_explicit_when_no_ax_metric_is_available() {
        let result = scroll_verification(None, None);
        assert_eq!(result["verified"], false);
        assert!(result["observed_delta"].is_null());
        assert_eq!(result["effect"], "unobservable");
    }
}

#[cfg(test)]
mod amount_tests {
    use super::*;

    #[test]
    fn amount_is_clamped_to_the_advertised_range() {
        let amount = &def().input_schema["properties"]["amount"];
        assert_eq!(amount["minimum"], serde_json::json!(AMOUNT_MIN));
        assert_eq!(amount["maximum"], serde_json::json!(AMOUNT_MAX));
        assert_eq!(clamp_amount(1100), AMOUNT_MAX as usize);
        assert_eq!(clamp_amount(0), AMOUNT_MIN as usize);
        assert_eq!(clamp_amount(3), 3);
    }
}
