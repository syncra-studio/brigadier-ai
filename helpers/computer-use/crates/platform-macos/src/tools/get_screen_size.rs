use async_trait::async_trait;
use cu_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef},
};
use serde_json::Value;

pub struct GetScreenSizeTool;

static DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| ToolDef {
        // Matches `GetScreenSizeTool.swift` description verbatim.
        name: "get_screen_size".into(),
        description: "Return the logical size of the main display in points plus its backing \
            scale factor. Agents click in points; Retina displays have scale_factor 2.0. \
            Structured output also lists every active display as `displays` (id, uuid, \
            bounds, is_main, index) in the order `launch_display` selectors use. \
            Requires no TCC permissions.".into(),
        input_schema: serde_json::json!({"type":"object","properties":{},"additionalProperties":false}),
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })
}

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        match main_screen_size() {
            Some((w, h, scale)) => {
                // Matches Swift text format 1:1.
                ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
                    .with_structured(serde_json::json!({
                        "width": w, "height": h, "scale_factor": scale,
                        "displays": displays_json(),
                    }))
            }
            None => ToolResult::error("No main display detected."),
        }
    }
}

fn displays_json() -> Value {
    let displays = crate::displays::active_displays();
    let ordered = cu_core::display_placement::ordered_displays(&displays);
    Value::Array(
        ordered
            .iter()
            .enumerate()
            .map(|(index, d)| {
                serde_json::json!({
                    "index": index,
                    "id": d.id,
                    "uuid": d.uuid,
                    "is_main": d.is_main,
                    "bounds": {
                        "x": d.bounds.x, "y": d.bounds.y,
                        "width": d.bounds.width, "height": d.bounds.height,
                    },
                })
            })
            .collect(),
    )
}

/// Returns `(width_points, height_points, backing_scale_factor)` from
/// CoreGraphics — safe to call from any thread (no AppKit main-thread requirement).
///
/// The previous NSScreen-based implementation required `MainThreadMarker::new()`
/// which always returns `None` on async tokio threads, causing the tool to
/// return an error even when a display is attached.
pub(crate) fn main_screen_size() -> Option<(i64, i64, f64)> {
    use core_graphics::display::{CGDisplayBounds, CGMainDisplayID};

    // SAFETY: CGMainDisplayID / CGDisplayBounds are thread-safe CG APIs.
    let display_id = unsafe { CGMainDisplayID() };
    if display_id == 0 {
        return None;
    }
    let bounds = unsafe { CGDisplayBounds(display_id) };
    let w = bounds.size.width as i64;
    let h = bounds.size.height as i64;
    if w == 0 || h == 0 {
        return None;
    }

    let scale = get_backing_scale(display_id, w);
    Some((w, h, scale))
}

/// Backing scale from the display's current mode: physical pixel width over
/// logical point width.
///
/// `CGDisplayPixelsWide` cannot be the primary source: on HiDPI ("Retina")
/// modes it reports the mode's POINT width, so the old pixel/logical ratio
/// collapsed to 1.0 and every overlay raster (cursor, focus ring) drew at
/// half resolution on Retina displays.
pub(crate) fn get_backing_scale(display_id: u32, logical_w: i64) -> f64 {
    use core_graphics::display::CGDisplay;
    if let Some(mode) = CGDisplay::new(display_id).display_mode() {
        if let Some(scale) = scale_from_widths(mode.pixel_width() as f64, mode.width() as f64) {
            return scale;
        }
    }
    // Legacy framebuffer-width heuristic; correct on non-HiDPI modes.
    use core_graphics::display::CGDisplayPixelsWide;
    let pixel_w = unsafe { CGDisplayPixelsWide(display_id) } as f64;
    scale_from_widths(pixel_w, logical_w as f64).unwrap_or(1.0)
}

/// Pixels-per-point ratio rounded to the nearest 0.5 to absorb floating point
/// noise. `None` when either width is unusable.
fn scale_from_widths(pixel_w: f64, point_w: f64) -> Option<f64> {
    if pixel_w > 0.0 && point_w > 0.0 {
        Some(((pixel_w / point_w) * 2.0).round() / 2.0)
    } else {
        None
    }
}

#[cfg(test)]
mod backing_scale_tests {
    use super::scale_from_widths;

    /// Retina regression: the display mode reports 3024 physical pixels over
    /// 1512 points. The old CGDisplayPixelsWide-based ratio compared points to
    /// points and collapsed to 1.0, so the overlay rasterized at half
    /// resolution on every Retina display.
    #[test]
    fn retina_mode_widths_yield_2x() {
        assert_eq!(scale_from_widths(3024.0, 1512.0), Some(2.0));
        assert_eq!(scale_from_widths(1512.0, 1512.0), Some(1.0));
        // Scaled "More Space" style modes land on non-integer ratios.
        assert_eq!(scale_from_widths(3600.0, 2400.0), Some(1.5));
        assert_eq!(scale_from_widths(0.0, 1512.0), None);
        assert_eq!(scale_from_widths(3024.0, 0.0), None);
    }
}
