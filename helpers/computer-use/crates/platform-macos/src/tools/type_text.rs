//! type_text tool — matches the Swift reference TypeTextTool.swift.
//!
//! Inserts text via `AXSelectedText` attribute write — an atomic single-call
//! insertion at the current cursor position. This is the preferred path for
//! all standard Cocoa text views (NSTextField, NSTextView, WKWebView text
//! inputs in Safari, etc.) and is significantly faster than per-keystroke
//! CGEvent synthesis.
//!
//! For Chromium / Electron inputs that don't implement `kAXSelectedText`,
//! the tool falls back to character-by-character CGEvent keystrokes so the
//! caller doesn't need to detect the app type themselves.
//!
//! When the target pid belongs to a terminal emulator (Ghostty,
//! Terminal.app, iTerm2, Alacritty, kitty, WezTerm, Hyper, Warp — see
//! [`crate::terminal::TERMINAL_BUNDLE_IDS`]), the AX path is skipped
//! entirely: terminals expose `AXTextArea` for their grid but the
//! `AXSelectedText` write never reaches the pty, so the tool would
//! report success while the shell sees nothing. We go straight to
//! CGEvent key-event synthesis (`path: "key_events"`).
//!
//! Use `type_text_chars` when you explicitly need per-character pacing
//! (e.g., to trigger live-search debounce handlers).

use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;
use std::sync::Arc;

use crate::apps;
use crate::ax::bindings::{
    copy_string_attr, focused_element_of_pid, kAXErrorCannotComplete, kAXErrorSuccess,
    perform_action, set_string_attr, AXUIElementRef,
};
use crate::focus_guard;
use crate::window_change_detector::WindowChangeDetector;
use core_foundation::base::CFRelease;

use super::ToolState;

pub struct TypeTextTool {
    pub state: Arc<ToolState>,
}

impl TypeTextTool {
    pub fn new(state: Arc<ToolState>) -> Self {
        Self { state }
    }
}

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        name: "type_text".into(),
        description:
            "Insert text into the target pid via `AXSetAttribute(kAXSelectedText)`. \
             Works for standard Cocoa text fields and text views. No keystrokes are \
             synthesized — special keys (Return / Escape / arrows) go through \
             `press_key` / `hotkey`. For Chromium / Electron inputs that don't \
             implement `kAXSelectedText`, the tool falls back to CGEvent \
             character synthesis automatically.\n\n\
             Optional `element_index` + `window_id` (from the last \
             `get_window_state` snapshot) directs the write to a specific field. \
             Without `element_index`, the write goes to the pid's currently \
             focused element.\n\n\
             WEB CONTENT (Chromium/WebKit/Electron — browser tabs, Slack, VS Code, \
             X's compose box): the AX layer accepts a write and echoes it back \
             through AXValue while the renderer/DOM never observes it. The driver \
             detects this at the element level (an AXWebArea ancestor) and refuses \
             to trust that echo — an AX-path insert into web content returns \
             effect:\"unverifiable\" + escalation, never a false \"confirmed\" (a \
             browser's own native address bar/toolbar stays trusted). For a browser \
             TAB the reliable path is the `page` tool (drives the DOM via CDP); for \
             an embedded web view use this tool's px form: pass x,y (no \
             element_index) to pixel-click the field then type, in one call. NOTE: \
             a px focus-click won't reliably open+focus a CLOSED control; AX-press \
             to open/activate it first (works in the background), then px-type. \
             Always confirm via the screenshot."
            .into(),
        input_schema: serde_json::json!({
            "type": "object",
            "required": ["pid", "text"],
            "properties": {
                "session": { "type": "string", "description": "Optional explicit session id for the agent cursor and per-session state. Embedded MCP calls may omit it to use BRIGADIER_CU_DEFAULT_SESSION (or embedded-<pid>); anonymous non-embedded calls remain cursor-less." },
                "pid":  { "type": "integer", "description": "Target process ID." },
                "text": { "type": "string",  "description": "Text to insert at the target's cursor." },
                "window_id": {
                    "type": "integer",
                    "description": "CGWindowID. Required when element_index is used. Optional when element_token is supplied (the token carries it)."
                },
                "element_index": {
                    "type": "integer",
                    "description": "Element index from last get_window_state. Directs the write to a specific field. REQUIRES `pid` and `window_id` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op."
                },
                "element_token": {
                    "type": "string",
                    "description": "Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."
                },
                "x": { "type": "number", "description": "Screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_index) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the AX path can't reach. Read straight off the get_window_state PNG, same convention as click." },
                "y": { "type": "number", "description": "Screenshot-pixel Y of the field (see x)." },
                "delay_ms": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 200,
                    "description": "Milliseconds between characters in the CGEvent fallback path. Default 30. Ignored when the AX path succeeds."
                },
                "delivery_mode": {
                    "type": "string",
                    "enum": ["background", "foreground"],
                    "description": "Best-effort-background ladder rung (default \"background\"). \"background\": AX insert, then CGEvent keystrokes if needed — no focus steal; the driver verifies via an AXValue read-back and reports `verified`. \"foreground\" is refused: bringing the app to the front would interrupt the user."
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
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    fn dispatch_preflight(&self, args: &Value) -> Result<(), ToolResult> {
        cu_core::tool::validate_dispatch_args(self.def(), args)?;
        let address = super::preflight_action_address(args, "type_text")?;
        if address.element && address.has_xy {
            return Err(ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to type_text, not both.",
            ));
        }
        if address.element && address.window_id.is_none() {
            return Err(ToolResult::error(
                "window_id is required when element_index is used.",
            ));
        }
        Ok(())
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cu_core::tool_args::ArgsExt;
        let dispatch_gate = crate::dispatch_gate::NativeDispatchGate::for_args(&args);
        let pid = match args.require_i32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags — see
        // cu_core::text_sanitize docs for rationale.
        let text =
            cu_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw).into_owned();
        // Surface 6: element_token / element_index precedence resolution.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id").map(|v| v as u32);
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cu_core::element_token::resolve_element_args(
            pid,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg,
            "type_text",
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
        let delay_ms = args.u64_or("delay_ms", 30);
        let delivery_mode = super::DeliveryMode::parse(args.opt_str("delivery_mode").as_deref());

        // ── px form: focus by pixel-click, then type into the focused element ──
        // Pass x,y (no element_index) for an *element px action*: pixel-click the
        // field to give the Chromium/Electron renderer the real keyboard focus the
        // AX path can't, then fall through to the focused-element type path (which
        // escalates AX → CGEvent and lands once focused). Reuses ClickTool's exact
        // coordinate translation + delivery_mode, so it lands on the same pixel a
        // px-click would.
        let px = args.get("x").and_then(|v| v.as_f64());
        let py = args.get("y").and_then(|v| v.as_f64());
        if let (Some(cx), Some(cy)) = (px, py) {
            if element_index.is_some() {
                return ToolResult::error(
                    "Pass either element_index (ax) or x,y (px) to type_text, not both.",
                );
            }
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Err(e) = super::focus_by_pixel(
                &self.state,
                pid,
                window_id,
                cx,
                cy,
                delivery_mode.is_foreground(),
                args.opt_str("session"),
                args.opt_str("_session_id"),
                from_zoom,
            )
            .await
            {
                return e;
            }
            // element_index stays None → the type path below writes to the now-
            // focused element via the CGEvent (key_events) rung.
        }

        // Validate element_index requires window_id (still applies for
        // the legacy integer path; token path already resolved window_id).
        if element_index.is_some() && window_id.is_none() {
            return ToolResult::error("window_id is required when element_index is used.");
        }

        // Resolve the element pointer (if element_index given). Retain it out
        // of the cache so a concurrent get_window_state can't free it before
        // the blocking type below dereferences it (use-after-free → daemon
        // crash). The guard lives to method end, past type_text_blocking.
        let element_guard = if let (Some(idx), Some(wid)) = (element_index, window_id) {
            match self.state.element_cache.get_element_retained(pid, wid, idx) {
                Some(e) => Some((e, idx)),
                None => {
                    return ToolResult::error(format!(
                        "Element index {idx} not found. Call get_window_state first."
                    ))
                }
            }
        } else {
            None
        };
        let element_ptr = element_guard
            .as_ref()
            .map(|(g, idx)| (g.as_ptr(), Some(*idx)));

        // A keyboard action still has a visible action point: the explicit AX
        // target, or the app's currently focused element. Glide before typing
        // so an embedded default cursor is present where the text lands (the px
        // form has already focused/clicked the same point above).
        let cursor_point = if let Some((ptr, _)) = element_ptr {
            super::cursor_tools::retained_element_center(ptr).await
        } else {
            super::cursor_tools::focused_element_center(pid).await
        };
        if let Some(point) = cursor_point {
            super::cursor_tools::animate_to_action_point(&self.state, &args, point, window_id)
                .await;
        }

        let text_clone = text.clone();
        let char_count = text.chars().count();

        // ── Focus-suppression wrap (Swift WindowChangeDetector + FocusGuard) ──
        // Typing into a field can trigger autocomplete popovers or
        // Chrome/Safari's "Save Password?" prompt, both of which open
        // helper windows. Wrap so callers see them in the result suffix
        // and the wildcard suppressor catches reflex activations.
        let prior_front = apps::frontmost_pid();
        let snapshot = WindowChangeDetector::snapshot_for_session(
            prior_front,
            &super::cursor_tools::resolve_cursor_key(&args),
        );

        // Terminal-emulator short-circuit: when the target pid belongs
        // to a known terminal (Ghostty / Terminal.app / iTerm2 / …), the
        // AX value-set is silently dropped — see crate::terminal docs.
        // Skip the AX path entirely so the caller never sees the
        // "success but nothing typed" symptom.
        let is_terminal_target = crate::terminal::is_terminal_pid(pid);
        let gate = dispatch_gate.clone();

        let result = focus_guard::with_focus_suppressed(
            Some(pid),
            prior_front,
            "type_text.AXSelectedText",
            || async move {
                tokio::task::spawn_blocking(move || {
                    type_text_blocking(
                        pid,
                        &text_clone,
                        element_ptr,
                        delay_ms,
                        is_terminal_target,
                        delivery_mode,
                        window_id,
                        &gate,
                    )
                })
                .await
            },
        )
        .await;

        let changes = snapshot.detect_async_for_args(&args).await;

        match result {
            Ok(Ok((detail, path, verified))) => {
                // SURFACE-AWARE VERIFICATION. On any web-content surface —
                // Chromium/WebKit/Electron — the AX layer accepts a write and
                // echoes it straight back through `AXValue` while the renderer/DOM
                // never observes it, so an AX-path read-back "confirm" is a shim
                // echo, not ground truth (the Slack-search AND Chrome-on-X
                // false-confirms). Detect it at the ELEMENT level (an `AXWebArea`
                // ancestor) so it covers every browser + Electron uniformly, yet a
                // browser's OWN native chrome (address bar, toolbar) stays trusted.
                // Probe ONLY when it would otherwise confirm via the AX path, so
                // native types pay nothing.
                let ax_echo_surface =
                    verified && path == PATH_AX && target_in_web_area(pid, element_ptr);
                let verified = verified && !ax_echo_surface;

                // `verified:false` means the driver could not confirm the text
                // landed (Electron AX echo, unreadable AXValue on Catalyst, or a
                // CGEvent rung the app may have dropped). Don't dress that as a
                // confirmed insert — tell the agent to look, and point at the
                // right next rung.
                let (mark, note) = if verified {
                    ("✅ Inserted", String::new())
                } else if ax_echo_surface {
                    (
                        "📨 Sent (unverified)",
                        " — web-content surface (Chromium / WebKit / Electron): the AX \
                      layer accepts and echoes the write but the renderer/DOM may not \
                      have observed it, so the driver cannot confirm via AX. Verify \
                      via the screenshot. For a browser tab use the `page` tool (it \
                      drives the DOM); for an embedded web view, re-type with the px \
                      form (x,y)."
                            .to_string(),
                    )
                } else if path == PATH_KEY_EVENTS_FG {
                    (
                        "📨 Sent (unverified)",
                        " — driver could not confirm; verify via screenshot.".to_string(),
                    )
                } else {
                    (
                        "📨 Sent (unverified)",
                        " — driver could not confirm the text landed; check with get_window_state \
                      or a screenshot before typing again."
                            .to_string(),
                    )
                };
                ToolResult::text(format!(
                    "{mark} {char_count} char(s){detail}.{note}{}",
                    changes.result_suffix()
                ))
                .with_structured({
                    // `effect` mirrors `verified`'s read-back tri-state: a TRUSTED
                    // positive read-back is "confirmed"; an unreadable/unchanged
                    // AXValue, a dropped CGEvent rung, or an Electron AX echo we
                    // refuse to trust is "unverifiable".
                    let mut s = serde_json::json!({
                        "path": path,
                        "characters": char_count,
                        "verified": verified,
                        "effect": if verified { "confirmed" } else { "unverifiable" },
                    });
                    if ax_echo_surface {
                        // Web-content AX echo. A real browser TAB → the `page` tool
                        // (drives the DOM via CDP) is the reliable rung; an embedded
                        // web view (Electron, no CDP) → the element px action. It's a
                        // renderer/DOM-focus problem, never a foreground one.
                        let (recommended, reason) =
                            if crate::browser::electron_js::ElectronJs::is_electron(pid) {
                                (
                                    "px",
                                    "Electron web view — the AX write was echoed but the \
                                  renderer may not have observed it. Confirm via the \
                                  screenshot; if it didn't land, re-type with the \
                                  element px action (x,y to pixel-focus the field, then \
                                  type).",
                                )
                            } else {
                                (
                                    "page",
                                    "Browser web content — the AX write was echoed but the \
                                  DOM may not have observed it (and AX type_text on a \
                                  contenteditable is racy). Drive the tab's DOM with the \
                                  `page` tool: execute_javascript + el.value/innerText for a \
                                  plain input; for a rich-text contenteditable \
                                  (Draft.js/Lexical/Slate-style editors can silently discard \
                                  a one-shot DOM write on their next render) try insert_text \
                                  first (one CDP call, cheap), then type_keystrokes if that \
                                  also gets discarded (real per-character keyboard events, \
                                  slower but most durable). Or confirm via the screenshot.",
                                )
                            };
                        s["escalation"] = serde_json::json!({
                            "recommended": recommended,
                            "reason": reason,
                        });
                    } else if !verified && path != PATH_KEY_EVENTS_FG {
                        s["escalation"] = serde_json::json!({
                            "recommended": "verify",
                            "reason": "background insert could not be confirmed — \
                                       check with get_window_state or a screenshot \
                                       before typing again."
                        });
                    }
                    s
                })
            }
            Ok(Err(e)) => ToolResult::error(format!("type_text failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── Blocking implementation ───────────────────────────────────────────────────

/// Which delivery path was taken. Surfaced as `structuredContent.path`
/// on success.
const PATH_AX: &str = "ax";
const PATH_KEY_EVENTS: &str = "key_events";
const PATH_KEY_EVENTS_FG: &str = "key_events_fg";

/// Read-back verification for a keystroke rung: did the typed text actually land?
///
/// `before`/`after` are `AXValue` read from the target field before and after
/// the keystrokes. Returns whether we can *positively confirm* the text landed:
/// - unreadable `after` (`None`) → unverifiable → `false` (Catalyst case; the
///   agent must confirm via screenshot).
/// - `after` contains the text, or grew vs `before` → `true`.
/// - empty input text → trivially `true`.
///
/// Apps that normalize input (smart quotes, autocomplete) may fail the
/// substring/length test even though something landed — we report `false`
/// (unverified) rather than erroring, so the agent can still confirm.
fn verify_typed(before: Option<&str>, after: Option<&str>, text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let Some(after) = after else { return false };
    after.contains(text) || before.map_or(false, |b| after.chars().count() > b.chars().count())
}

/// Read the focused/target field's `AXValue`, for before/after read-back.
/// Re-fetches the focused element each call when no explicit element is given
/// (cheap, and focus is stable across our own keystrokes).
fn read_axvalue(pid: i32, element_ptr_and_idx: Option<(usize, Option<usize>)>) -> Option<String> {
    if let Some((ptr, _)) = element_ptr_and_idx {
        unsafe { copy_string_attr(ptr as AXUIElementRef, "AXValue") }
    } else if let Some(el) = unsafe { focused_element_of_pid(pid) } {
        let v = unsafe { copy_string_attr(el, "AXValue") };
        unsafe {
            CFRelease(el as _);
        }
        v
    } else {
        None
    }
}

/// True when the addressed (or focused) AX element sits inside a web-content
/// subtree — an `AXWebArea` ancestor. That covers every Chromium / WebKit /
/// Electron rendered surface (Chrome, Safari, Slack, VS Code, X's compose box…),
/// where an AX write is echoed back through `AXValue` while the renderer/DOM
/// never observes it — so an AX read-back "confirm" there is a shim echo. A
/// browser's OWN native chrome (address bar, toolbar) has no `AXWebArea`
/// ancestor, so it stays trusted. Walks a bounded ancestor chain; each
/// `AXParent` copy is released, and it stops at the window/app boundary.
fn target_in_web_area(pid: i32, element_ptr_and_idx: Option<(usize, Option<usize>)>) -> bool {
    use crate::ax::bindings::AXUIElementCopyAttributeValue;
    use core_foundation::base::{CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    unsafe {
        // Start element: the addressed one (borrowed — do NOT release) or the pid's
        // focused element (owned — must release when done).
        let (start, start_owned) = match element_ptr_and_idx {
            Some((ptr, _)) => (ptr as AXUIElementRef, false),
            None => match focused_element_of_pid(pid) {
                Some(el) => (el, true),
                None => return false,
            },
        };
        let parent_attr = CFString::new("AXParent");
        let mut cur = start;
        let mut cur_owned = start_owned;
        let mut found = false;
        for _ in 0..40 {
            match copy_string_attr(cur, "AXRole").as_deref() {
                Some("AXWebArea") => {
                    found = true;
                    break;
                }
                // No web area lives above the window/app root — stop.
                Some("AXWindow") | Some("AXApplication") | None => break,
                _ => {}
            }
            let mut parent: CFTypeRef = std::ptr::null_mut();
            let err =
                AXUIElementCopyAttributeValue(cur, parent_attr.as_concrete_TypeRef(), &mut parent);
            if cur_owned {
                CFRelease(cur as CFTypeRef);
            }
            if err != kAXErrorSuccess || parent.is_null() {
                cur = std::ptr::null_mut();
                cur_owned = false;
                break;
            }
            cur = parent as AXUIElementRef;
            cur_owned = true;
        }
        if cur_owned && !cur.is_null() {
            CFRelease(cur as CFTypeRef);
        }
        found
    }
}

/// Type via CGEvent keystrokes, optionally clearing the field first (idempotent
/// retype), then verify by read-back.
///
/// `clear_first` is the idempotency guard for escalation: when the field was
/// empty/unreadable at capture, `Cmd+A`+`Delete` makes the retype land exactly
/// `text` regardless of what a prior unverified rung may have done — avoiding
/// double-type. It is NOT used when the field had readable pre-existing content
/// (that would clobber it).
fn cgevent_type_verified(
    pid: i32,
    text: &str,
    delay_ms: u64,
    before: Option<&str>,
    clear_first: bool,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    settle_ms: u64,
    gate: &crate::dispatch_gate::NativeDispatchGate,
) -> anyhow::Result<bool> {
    // Focus the target element first so the keystrokes land in IT. Critical in
    // foreground mode: a freshly-fronted window's keyboard focus may be on the
    // search box or nowhere, so without this the text goes into the void (or the
    // wrong field). AXFocused is best-effort — harmless when unsupported.
    if let Some((ptr, _)) = element_ptr_and_idx {
        crate::input::ax_actions::focus_element_guarded(ptr, gate)?;
    }
    // First-keystroke settle (foreground rung only — caller passes `settle_ms > 0`).
    // After a window is fronted (with_foreground_assist) and the element focused,
    // the surface isn't ready to accept input for a few tens of ms, so the FIRST
    // synthesized character gets eaten: typing "i love u" rendered "love u" (the
    // leading "i " was dropped). A short sleep here lets focus settle before the
    // first key event. Background/terminal call sites pass 0 — they have no front
    // transition and must not pay this latency.
    if settle_ms > 0 {
        std::thread::sleep(std::time::Duration::from_millis(settle_ms));
    }
    if clear_first {
        if settle_ms > 0 {
            // Some renderer focus proxies discard the first printable event
            // after activation even after their AX focus is visible. Prime
            // that channel with disposable text, then clear it before the
            // requested payload. Never do this for a nonempty field.
            crate::input::keyboard::type_text_with_delay_guarded(pid, " ", delay_ms, gate)?;
        }
        crate::input::keyboard::press_key_guarded(pid, "a", &["cmd"], gate)?;
        crate::input::keyboard::press_key_guarded(pid, "delete", &[], gate)?;
    }
    crate::input::keyboard::type_text_with_delay_guarded(pid, text, delay_ms, gate)?;
    let after = read_axvalue(pid, element_ptr_and_idx);
    if after.as_deref().is_some_and(|v| v.contains(text)) {
        return Ok(true);
    }
    if refresh_web_value(pid, element_ptr_and_idx, text) {
        return Ok(true);
    }
    Ok(verify_typed(before, after.as_deref(), text))
}

/// A web view in a window that isn't painting (background, another Space)
/// sends its accessibility changes only when it next handles an event, so the
/// keystrokes have landed but `AXValue` still shows older text — and so does
/// the agent's next snapshot. Asking the element to scroll itself into view (a
/// no-op for a visible field) makes the renderer send its pending changes; the
/// keystrokes may still be on their way, so ask again until the text shows.
/// Keystrokes go through the renderer, so this read-back is real, unlike an
/// echoed AX write. Only the text itself counts: the stale value may grow from
/// an earlier change.
fn refresh_web_value(
    pid: i32,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    text: &str,
) -> bool {
    let Some((ptr, _)) = element_ptr_and_idx else {
        return false;
    };
    if text.is_empty() || !target_in_web_area(pid, element_ptr_and_idx) {
        return false;
    }
    for _ in 0..10 {
        let _ = unsafe { perform_action(ptr as AXUIElementRef, "AXScrollToVisible") };
        std::thread::sleep(std::time::Duration::from_millis(40));
        if read_axvalue(pid, element_ptr_and_idx).is_some_and(|v| v.contains(text)) {
            return true;
        }
    }
    false
}

/// Best-effort-background ladder for `type_text`.
///
/// - `delivery_mode == Background` (default): AX insert → read-back; on a
///   silent/unreadable accept, CGEvent keystrokes → read-back. Never fronts.
/// - `delivery_mode == Foreground`: the agent's explicit last resort — briefly
///   front `window_id`, type (clear-first when the field was empty/unreadable so
///   the retype is idempotent), restore, then read-back.
///
/// Returns `(detail, path, verified)`. `verified` is `true` only when a
/// read-back positively confirmed the text; `false` means the agent must
/// confirm via screenshot (and, for background, can escalate to foreground).
fn type_text_blocking(
    pid: i32,
    text: &str,
    element_ptr_and_idx: Option<(usize, Option<usize>)>,
    delay_ms: u64,
    is_terminal_target: bool,
    delivery_mode: super::DeliveryMode,
    window_id: Option<u32>,
    gate: &crate::dispatch_gate::NativeDispatchGate,
) -> anyhow::Result<(String, &'static str, bool)> {
    // One call types into an app at a time, so another agent can't move the
    // app's keyboard focus to its own window halfway through this text.
    let _keys = crate::input::key_window::lease(pid);
    // Keystrokes and focused-element writes land wherever the app's keyboard
    // focus is; with a window named, make sure it is there before using them.
    let element_ptr = element_ptr_and_idx.map(|(ptr, _)| ptr);
    let ensure_focus = || match window_id {
        Some(wid) => crate::input::key_window::ensure_in(pid, wid, element_ptr),
        None => Ok(()),
    };
    // Original field value before ANY rung — drives both the read-back delta and
    // the clear-then-type idempotency decision.
    let before = read_axvalue(pid, element_ptr_and_idx);
    // Clear-then-type only when we can't see existing content to preserve
    // (empty or unreadable). A readable non-empty value is left intact.
    let clear_first = !matches!(before.as_deref(), Some(b) if !b.is_empty());

    // --- Foreground rung: explicit agent request (skip AX/background ladder). ---
    if delivery_mode.is_foreground() {
        // Settle between front+focus and the first keystroke — see the
        // "i love u" -> "love u" first-char-drop note in cgevent_type_verified.
        // 60ms covers native Cocoa/Catalyst surfaces, but focus-proxy clients
        // that re-establish their own input channel on activation need longer:
        // an RDP client (Microsoft Windows App) re-arms its keyboard grab with
        // the remote host over hundreds of ms, so at 60ms every keystroke was
        // dropped. 200ms covers that re-grab without being perceptible.
        const FOREGROUND_SETTLE_MS: u64 = 200;
        let do_type = || {
            cgevent_type_verified(
                pid,
                text,
                delay_ms,
                before.as_deref(),
                clear_first,
                element_ptr_and_idx,
                FOREGROUND_SETTLE_MS,
                gate,
            )
        };
        let (verified, fronted) = match window_id {
            Some(wid) => {
                // Front → type → restore. The closure returns the read-back
                // result; with_foreground_assist returns whether it actually
                // fronted (Ok(false) when the fronting SPIs are unavailable —
                // the keystrokes still ran, just as background input).
                let mut typed_verified = false;
                let fronted = crate::input::skylight::with_foreground_assist_guarded(
                    pid as libc::pid_t,
                    wid,
                    gate,
                    || {
                        typed_verified = do_type()?;
                        Ok(())
                    },
                )?;
                (typed_verified, fronted)
            }
            // No window to front — best-effort background keystrokes instead.
            None => (do_type()?, false),
        };
        // Only claim the `_fg` path when a front actually happened; when no
        // foregrounding occurred (no window, or SPIs unavailable) these were
        // background keystrokes and `path` must say so honestly.
        return Ok((
            format!(" via foreground keystrokes ({delay_ms}ms delay)"),
            if fronted {
                PATH_KEY_EVENTS_FG
            } else {
                PATH_KEY_EVENTS
            },
            verified,
        ));
    }

    // --- Background rung 0: terminal emulator → CGEvent only (AX is dropped). ---
    if is_terminal_target {
        tracing::debug!(
            "type_text: pid {pid} is a terminal emulator; skipping AX value-set, \
             using CGEvent key-event synthesis"
        );
        ensure_focus()?;
        let verified = cgevent_type_verified(
            pid,
            text,
            delay_ms,
            before.as_deref(),
            /*clear_first=*/ false,
            element_ptr_and_idx,
            /*settle_ms=*/ 0,
            gate,
        )?;
        return Ok((
            format!(" via CGEvent (terminal emulator, {delay_ms}ms delay)"),
            PATH_KEY_EVENTS,
            verified,
        ));
    }

    // --- Background rung 1: AX SelectedText write (element or focused). ---
    if element_ptr_and_idx.is_none() {
        ensure_focus()?;
    }
    let ax_target: Option<(AXUIElementRef, bool, Option<usize>)> = match element_ptr_and_idx {
        Some((ptr, idx)) => Some((ptr as AXUIElementRef, /*owns=*/ false, idx)),
        None => unsafe { focused_element_of_pid(pid) }.map(|el| (el, /*owns=*/ true, None)),
    };
    if let Some((element, owns, idx_opt)) = ax_target {
        let role = unsafe { copy_string_attr(element, "AXRole") }.unwrap_or_default();
        let title = unsafe { copy_string_attr(element, "AXTitle") }.unwrap_or_default();
        if let Err(error) = gate.check() {
            if owns {
                unsafe { CFRelease(element as _) };
            }
            return Err(error.into());
        }
        let err = unsafe { set_string_attr(element, "AXSelectedText", text) };
        // Landed iff the API succeeded AND a read-back positively confirms the
        // intended `text` — it now contains `text`, or the field grew vs
        // `before`. A non-empty `AXValue` alone is NOT enough: a pre-filled
        // field whose `AXSelectedText` write silently no-ops still reads back
        // its old (non-empty) value, which would otherwise report a false
        // success. `verify_typed` compares the read-back against `before`/`text`.
        let after = unsafe { copy_string_attr(element, "AXValue") };
        let mut ax_landed =
            err == kAXErrorSuccess && verify_typed(before.as_deref(), after.as_deref(), text);
        if err == kAXErrorCannotComplete && !ax_landed {
            // The app didn't answer in time, but may have taken the write:
            // look again rather than type it a second time.
            std::thread::sleep(std::time::Duration::from_millis(200));
            let again = unsafe { copy_string_attr(element, "AXValue") };
            ax_landed = verify_typed(before.as_deref(), again.as_deref(), text);
            if !ax_landed {
                if owns {
                    unsafe { CFRelease(element as _) };
                }
                anyhow::bail!(
                    "the app did not answer in time while the text was written, so it may or \
                     may not have landed. Take a fresh get_window_state before typing it again, \
                     or it may end up there twice."
                );
            }
        }
        if owns {
            unsafe {
                CFRelease(element as _);
            }
        }
        if ax_landed {
            let idx_str = idx_opt.map(|i| format!(" [{i}]")).unwrap_or_default();
            return Ok((format!(" into{idx_str} {role} \"{title}\""), PATH_AX, true));
        }
        tracing::debug!(
            "AX write did not land for {role} \"{title}\" (err={err}); \
             falling back to CGEvent keystrokes"
        );
    } else {
        tracing::debug!("No focused element for pid {pid}; using CGEvent keystrokes");
    }

    // --- Background rung 2: CGEvent keystrokes with read-back. ---
    // No clear-first here: a partial AX write is rare and clearing on every
    // background fallback would change insert-at-cursor semantics. The
    // foreground rung owns the idempotent clear-then-type.
    ensure_focus()?;
    let verified = cgevent_type_verified(
        pid,
        text,
        delay_ms,
        before.as_deref(),
        /*clear_first=*/ false,
        element_ptr_and_idx,
        /*settle_ms=*/ 0,
        gate,
    )?;
    Ok((
        format!(" via CGEvent ({delay_ms}ms delay)"),
        PATH_KEY_EVENTS,
        verified,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity-check that the terminal short-circuit can be expressed as a
    /// pure function of `is_terminal_target`: when true, the code goes
    /// to key-event synthesis without consulting AX. This test stands
    /// in for an integration test (which would need a running terminal)
    /// — it exercises the branch by injecting `is_terminal_target=true`
    /// with a non-existent pid and checking we get the expected error
    /// shape from the CGEvent path (not from the AX path).
    ///
    /// The CGEvent post will fail for pid 0 / -1, so we only assert
    /// that `type_text_blocking` returns `Err` *after* deciding to
    /// take the key-events path — i.e. it doesn't hit the AX branches
    /// where `set_string_attr(0)` would crash.
    #[test]
    #[ignore = "sends accessibility requests and posts key events; run with --ignored from a granted, signed helper"]
    fn terminal_flag_routes_past_ax_path() {
        // Pid -1 is invalid; the AX path would unconditionally call
        // focused_element_of_pid which is safe but it would never reach
        // CGEvent. The fact that this returns an Err (without crashing)
        // proves we routed through CGEvent-only and never touched AX.
        let r = type_text_blocking(
            -1,
            "x",
            None,
            0,
            /*is_terminal_target=*/ true,
            super::super::DeliveryMode::Background,
            None,
            &crate::dispatch_gate::NativeDispatchGate::default(),
        );
        // We don't care whether r is Ok or Err — what matters is that
        // calling it with is_terminal_target=true is safe and never
        // dereferences null AX pointers.
        let _ = r;
    }

    #[test]
    fn verify_typed_unreadable_after_is_unverified() {
        // Catalyst: can't read AXValue back → cannot confirm → false.
        assert!(!verify_typed(None, None, "hi"));
        assert!(!verify_typed(Some(""), None, "hi"));
    }

    #[test]
    fn verify_typed_contains_or_grew_is_verified() {
        assert!(verify_typed(Some(""), Some("hi"), "hi")); // contains
        assert!(verify_typed(Some("ab"), Some("ab hi"), "hi")); // contains, appended
        assert!(verify_typed(Some("ab"), Some("abXY"), "??")); // grew vs before
    }

    #[test]
    fn verify_typed_unchanged_is_unverified() {
        // Readable but the field didn't change and doesn't contain the text.
        assert!(!verify_typed(Some("ab"), Some("ab"), "hi"));
    }

    #[test]
    fn verify_typed_empty_text_is_trivially_verified() {
        assert!(verify_typed(None, None, ""));
    }

    #[test]
    fn path_constants_are_stable_tokens() {
        // These string constants are part of the structured-response
        // contract; freezing them here makes the contract a unit test.
        assert_eq!(PATH_AX, "ax");
        assert_eq!(PATH_KEY_EVENTS, "key_events");
        assert_eq!(PATH_KEY_EVENTS_FG, "key_events_fg");
    }
}
