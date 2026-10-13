//! Window / display screenshot using CoreGraphics, with a CLI fallback.
//!
//! CoreGraphics captures a single window by CGWindowID in-process. The
//! `screencapture` fallback remains for windows that WindowServer refuses to
//! materialize (and for older OS/TCC edge cases), but the normal path avoids a
//! subprocess and temporary PNG file on every state refresh.
//!
//! `screencapture -x <file>` captures the full main display.
//!
//! For production use, the ImageIO/CGWindowListCreateImageFromArray path
//! would give lower overhead (no subprocess + temp file), but the subprocess
//! approach is simpler to implement correctly and is reliable across OS versions.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use core_graphics::{
    base::kCGImageAlphaPremultipliedLast,
    color_space::CGColorSpace,
    context::CGContext,
    geometry::{CGPoint, CGRect, CGSize, CG_ZERO_RECT},
    window::{
        create_image, kCGWindowImageBestResolution, kCGWindowImageBoundsIgnoreFraming,
        kCGWindowListOptionIncludingWindow,
    },
};
use image::{codecs::png::PngEncoder, ColorType, ImageEncoder};
use std::process::Command;

use crate::capture_deadline::{self, CaptureOperation, CaptureTimeout};

/// Capture a window by its `window_id` (CGWindowID).
/// Returns raw PNG bytes or an error.
///
/// Each attempt is bounded by [`capture_deadline::SCREENSHOT_BUDGET`]. A
/// timeout returns [`capture_deadline::CaptureTimeout`] at once: the
/// `screencapture` fallback talks to the same blocked capture service, so
/// trying it would only double the wait.
pub fn screenshot_window_bytes(window_id: u32) -> anyhow::Result<Vec<u8>> {
    bounded_with_fallback(
        CaptureOperation::WindowImage,
        move || screenshot_window_bytes_core_graphics(window_id),
        move || screenshot_window_bytes_screencapture(window_id),
    )
}

fn bounded_with_fallback(
    operation: CaptureOperation,
    primary: impl FnOnce() -> anyhow::Result<Vec<u8>> + Send + 'static,
    fallback: impl FnOnce() -> anyhow::Result<Vec<u8>> + Send + 'static,
) -> anyhow::Result<Vec<u8>> {
    if crate::permissions::tcc_off_by_env() {
        anyhow::bail!("a test process never captures the screen");
    }
    match capture_deadline::run(operation, capture_deadline::SCREENSHOT_BUDGET, primary) {
        Ok(bytes) => Ok(bytes),
        Err(error) if CaptureTimeout::find(&error).is_some() => Err(error),
        Err(_) => capture_deadline::run(operation, capture_deadline::SCREENSHOT_BUDGET, fallback),
    }
}

fn screenshot_window_bytes_core_graphics(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let image = create_image(
        CG_ZERO_RECT,
        kCGWindowListOptionIncludingWindow,
        window_id,
        kCGWindowImageBestResolution | kCGWindowImageBoundsIgnoreFraming,
    )
    .ok_or_else(|| anyhow::anyhow!("CoreGraphics returned no image for window {window_id}"))?;
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        anyhow::bail!("CoreGraphics returned an empty image for window {window_id}");
    }

    // Render into a known RGBA bitmap instead of relying on the source
    // CGImage's platform-dependent byte order/premultiplication flags.
    let color_space = CGColorSpace::create_device_rgb();
    let mut context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &color_space,
        kCGImageAlphaPremultipliedLast,
    );
    context.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        ),
        &image,
    );
    let pixels = context.data().to_vec();
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        &pixels,
        width as u32,
        height as u32,
        ColorType::Rgba8.into(),
    )?;
    Ok(png)
}

fn screenshot_window_bytes_screencapture(window_id: u32) -> anyhow::Result<Vec<u8>> {
    let tmp_path = format!("/tmp/brigadier-cu-capture-{}.png", window_id);

    let output = Command::new("/usr/sbin/screencapture")
        .args([
            "-l",
            &window_id.to_string(),
            "-x", // no sound
            "-o", // no shadow
            &tmp_path,
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for window {window_id} with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for window {window_id} with status {}: {stderr}",
            output.status
        );
    }

    let bytes = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for window {window_id}");
    }
    Ok(bytes)
}

/// Capture a window by its `window_id` (CGWindowID).
/// Returns (base64-encoded PNG, width, height) or an error.
pub fn screenshot_window(window_id: u32) -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_window_bytes(window_id)?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

/// Capture the full main display.
/// Returns raw PNG bytes or an error.
/// Bounded like [`screenshot_window_bytes`].
pub fn screenshot_display_bytes() -> anyhow::Result<Vec<u8>> {
    bounded_with_fallback(
        CaptureOperation::DisplayImage,
        screenshot_display_bytes_core_graphics,
        screenshot_display_bytes_screencapture,
    )
}

fn screenshot_display_bytes_core_graphics() -> anyhow::Result<Vec<u8>> {
    // A zero window id plus the on-screen list asks WindowServer for the main
    // display image without starting a helper process. The display capture is
    // primarily used by native desktop tools; compatibility state uses the
    // window path above.
    let image = create_image(CG_ZERO_RECT, 1u32 | 16u32, 0, kCGWindowImageBestResolution)
        .ok_or_else(|| anyhow::anyhow!("CoreGraphics returned no display image"))?;
    let width = image.width() as usize;
    let height = image.height() as usize;
    if width == 0 || height == 0 {
        anyhow::bail!("CoreGraphics returned an empty display image");
    }
    let color_space = CGColorSpace::create_device_rgb();
    let mut context = CGContext::create_bitmap_context(
        None,
        width,
        height,
        8,
        width * 4,
        &color_space,
        kCGImageAlphaPremultipliedLast,
    );
    context.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        ),
        &image,
    );
    let pixels = context.data().to_vec();
    let mut png = Vec::new();
    PngEncoder::new(&mut png).write_image(
        &pixels,
        width as u32,
        height as u32,
        ColorType::Rgba8.into(),
    )?;
    Ok(png)
}

fn screenshot_display_bytes_screencapture() -> anyhow::Result<Vec<u8>> {
    // Use a pid-unique path so concurrent brigadier-cu processes don't step on each other.
    let tmp_path = format!("/tmp/brigadier-cu-display-{}.png", std::process::id());

    let output = Command::new("/usr/sbin/screencapture")
        .args(["-x", &*tmp_path])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if stderr.is_empty() {
            anyhow::bail!(
                "screencapture failed for main display with status {}",
                output.status
            );
        }
        anyhow::bail!(
            "screencapture failed for main display with status {}: {stderr}",
            output.status
        );
    }

    let bytes = std::fs::read(&tmp_path)?;
    let _ = std::fs::remove_file(&tmp_path);

    if bytes.is_empty() {
        anyhow::bail!("screencapture produced empty output for main display");
    }
    Ok(bytes)
}

/// Capture the main display and return (base64-encoded PNG, width, height).
pub fn screenshot_display() -> anyhow::Result<(String, u32, u32)> {
    let bytes = screenshot_display_bytes()?;
    let (w, h) = png_dimensions(&bytes)?;
    let b64 = BASE64.encode(&bytes);
    Ok((b64, w, h))
}

// PNG/JPEG/resize/crosshair helpers — re-exports of the shared
// `cu_core::image_utils` module. The previous file-local copies were
// near-identical to the Windows and Linux versions; the dedup-audit
// (2026-05) moved them all to one place. See
// `BRIGADIER_CU_DEDUP_AUDIT.md` for the audit trail.

/// Convert raw PNG bytes to JPEG at the given quality (1-95).
pub fn png_bytes_to_jpeg(png_bytes: &[u8], quality: u8) -> anyhow::Result<Vec<u8>> {
    cu_core::image_utils::png_bytes_to_jpeg(png_bytes, quality)
}

/// Downscale `png_bytes` so neither dimension exceeds `max_dim`.
/// If `max_dim == 0` or the image already fits, returns the original
/// bytes unchanged.
pub fn resize_png_if_needed(png_bytes: &[u8], max_dim: u32) -> anyhow::Result<Vec<u8>> {
    cu_core::image_utils::resize_png_if_needed(png_bytes, max_dim)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and write to
/// `path`. Used by `click`'s `debug_image_out` param to verify
/// coordinate spaces. The crosshair uses top-left-origin coords
/// matching the click tool's convention.
pub fn write_crosshair_png(png_bytes: &[u8], cx: f64, cy: f64, path: &str) -> anyhow::Result<()> {
    cu_core::image_utils::write_crosshair_png(png_bytes, cx, cy, path)
}

/// Draw a red crosshair at pixel (cx, cy) on a PNG image and return the
/// modified PNG bytes. Used by recording's click-marker callback to
/// produce click.png.
pub fn crosshair_png_bytes(png_bytes: &[u8], cx: f64, cy: f64) -> anyhow::Result<Vec<u8>> {
    cu_core::image_utils::crosshair_png_bytes(png_bytes, cx, cy)
}

/// Parse width and height from a PNG file's IHDR chunk.
pub fn png_dimensions(data: &[u8]) -> anyhow::Result<(u32, u32)> {
    cu_core::image_utils::png_dimensions(data)
}
