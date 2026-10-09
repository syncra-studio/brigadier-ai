//! One window's pixels through ScreenCaptureKit, at an exact scale, covered or not; a window on
//! another Space through the window server's own capture.

use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use block2::RcBlock;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality,
};
use objc2_foundation::NSError;
use objc2_screen_capture_kit::{
    SCContentFilter, SCScreenshotManager, SCShareableContent, SCStreamConfiguration, SCWindow,
};

use crate::error::{CuResult, ErrorCode, err};
use crate::geom::{ImageTransform, Rect};
use crate::redact::Rgba;

use super::private::Private;

const TIMEOUT: Duration = Duration::from_secs(5);
/// The pauses before a failed capture is asked for again.
const RETRY_PAUSES: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(250)];

/// The shareable windows, cached: listing them costs tens of milliseconds (§2).
#[derive(Default)]
pub struct Shareable {
    content: Option<Retained<SCShareableContent>>,
    /// Set by a capture that failed: the window may be gone or the list stale.
    stale: Rc<Cell<bool>>,
}

fn fetch_content() -> CuResult<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel::<Option<Retained<SCShareableContent>>>();
    let block = RcBlock::new(
        move |content: *mut SCShareableContent, _error: *mut NSError| {
            // SAFETY: the handler's content is either null or a live object we retain.
            let c = unsafe { Retained::retain(content) };
            let _ = tx.send(c);
        },
    );
    // SAFETY: the block lives until the handler ran (we wait for it below).
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
            true, false, &block,
        );
    }
    match rx.recv_timeout(TIMEOUT) {
        Ok(Some(c)) => Ok(c),
        Ok(None) => err(
            ErrorCode::PermissionMissing,
            "the Screen Recording permission is missing",
        ),
        Err(_) => err(ErrorCode::Failed, "the window list didn't arrive"),
    }
}

impl Shareable {
    fn window(&mut self, id: u32) -> CuResult<Retained<SCWindow>> {
        if self.stale.replace(false) {
            self.content = None;
        }
        for refresh in [false, true] {
            if refresh || self.content.is_none() {
                self.content = Some(fetch_content()?);
            }
            if let Some(c) = &self.content {
                // SAFETY: plain getters on a live object.
                let found = unsafe { c.windows() }
                    .iter()
                    .find(|w| unsafe { w.windowID() } == id);
                if let Some(w) = found {
                    return Ok(w);
                }
            }
        }
        err(
            ErrorCode::NoSuchTarget,
            format!("window {id} can't be captured"),
        )
    }

    /// Captures `crop` (window points) of the window at `scale` pixels per point.
    pub fn capture(
        &mut self,
        id: u32,
        frame: Rect,
        crop: Rect,
        scale: f64,
        max_side: u32,
    ) -> CuResult<(Rgba, ImageTransform)> {
        self.begin(id, frame, crop, scale, max_side)?()
    }

    /// Starts the capture `capture` makes; the returned wait gives its image.
    pub fn begin(
        &mut self,
        id: u32,
        frame: Rect,
        crop: Rect,
        scale: f64,
        max_side: u32,
    ) -> CuResult<impl FnOnce() -> CuResult<(Rgba, ImageTransform)> + 'static> {
        let win = self.window(id)?;
        let t = ImageTransform::fit(id, frame, crop, scale, max_side);
        // SAFETY: plain object creation and setters.
        let (filter, config) = unsafe {
            let filter =
                SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), &win);
            let config = SCStreamConfiguration::new();
            config.setWidth(t.width as usize);
            config.setHeight(t.height as usize);
            config.setSourceRect(CGRect::new(
                CGPoint::new(crop.x, crop.y),
                CGSize::new(crop.w, crop.h),
            ));
            config.setShowsCursor(false);
            config.setIgnoreShadowsSingleWindow(true);
            config.setScalesToFit(true);
            (filter, config)
        };
        let (tx, rx) = mpsc::channel::<Result<CFRetained<CGImage>, String>>();
        let block = RcBlock::new(move |img: *mut CGImage, error: *mut NSError| {
            // SAFETY: the handler's image and error are null or live; retaining keeps the image
            // past the handler.
            let img = NonNull::new(img).map(|p| unsafe { CFRetained::retain(p) });
            let why = || {
                // SAFETY: a live error's description is a plain getter.
                unsafe { error.as_ref() }
                    .map(|e| e.localizedDescription().to_string())
                    .unwrap_or_default()
            };
            let _ = tx.send(img.ok_or_else(why));
        });
        let submit =
            move |filter: &SCContentFilter,
                  config: &SCStreamConfiguration,
                  block: &RcBlock<dyn Fn(*mut CGImage, *mut NSError)>| {
                // SAFETY: the API copies the handler; the wait below also keeps ours until it ran or
                // the capture is given up, and a send after that goes nowhere.
                unsafe {
                    SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(
                        filter,
                        config,
                        Some(block),
                    );
                }
            };
        submit(&filter, &config, &block);
        let stale = self.stale.clone();
        Ok(move || {
            let mut got = rx.recv_timeout(TIMEOUT);
            // The system now and then fails to start a capture ("Failed to start stream due to
            // audio/video capture failure", measured 2026-10-09: 2 of 12 runs of 10 boards), and
            // the same request a moment later succeeds.
            for pause in RETRY_PAUSES {
                if !matches!(got, Ok(Err(_))) {
                    break;
                }
                std::thread::sleep(pause);
                submit(&filter, &config, &block);
                got = rx.recv_timeout(TIMEOUT);
            }
            drop(block);
            let img = match got {
                Ok(Ok(i)) => i,
                Ok(Err(why)) => {
                    // Drop the cached list before the next capture.
                    stale.set(true);
                    return err(ErrorCode::Failed, format!("the capture failed: {why}"));
                }
                Err(_) => return err(ErrorCode::Failed, "the capture timed out"),
            };
            Ok((to_rgba(&img, t.width, t.height)?, t))
        })
    }
}

/// Captures `crop` (window points) of a window that isn't on screen, as `Shareable::capture` does
/// for one that is: the window server's copy of it, cropped and scaled to the transform's size.
pub fn capture_offscreen(
    id: u32,
    frame: Rect,
    crop: Rect,
    scale: f64,
    max_side: u32,
) -> CuResult<(Rgba, ImageTransform)> {
    let t = ImageTransform::fit(id, frame, crop, scale, max_side);
    let Some(img) = Private::get().capture_window(id) else {
        return err(ErrorCode::Failed, "the capture failed");
    };
    let k = CGImage::width(Some(&img)) as f64 / frame.w.max(1.0);
    let src = CGRect::new(
        CGPoint::new(crop.x * k, crop.y * k),
        CGSize::new(crop.w * k, crop.h * k),
    );
    let Some(part) = CGImage::with_image_in_rect(Some(&img), src) else {
        return err(ErrorCode::Failed, "the capture had no pixels");
    };
    let (w, h) = (t.width as usize, t.height as usize);
    let mut data = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb();
    // BGRA in memory, as ScreenCaptureKit's captures are.
    let info = CGImageAlphaInfo::PremultipliedFirst.0 | CGImageByteOrderInfo::Order32Little.0;
    // SAFETY: `data` holds `h` rows of `w * 4` bytes and outlives the context.
    let ctx = unsafe {
        CGBitmapContextCreate(
            data.as_mut_ptr().cast(),
            w,
            h,
            8,
            w * 4,
            space.as_deref(),
            info,
        )
    };
    let Some(ctx) = ctx else {
        return err(ErrorCode::Failed, "the capture couldn't be scaled");
    };
    CGContext::set_interpolation_quality(Some(&ctx), CGInterpolationQuality::High);
    CGContext::draw_image(
        Some(&ctx),
        CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(w as f64, h as f64)),
        Some(&part),
    );
    drop(ctx);
    for px in data.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    Ok((
        Rgba {
            width: t.width,
            height: t.height,
            data,
        },
        t,
    ))
}

/// Copies a BGRA capture into packed RGBA of exactly `w`×`h`.
fn to_rgba(img: &CGImage, w: u32, h: u32) -> CuResult<Rgba> {
    let iw = CGImage::width(Some(img)) as u32;
    let ih = CGImage::height(Some(img)) as u32;
    let bpr = CGImage::bytes_per_row(Some(img));
    let bpp = CGImage::bits_per_pixel(Some(img));
    if bpp != 32 {
        return err(
            ErrorCode::Failed,
            format!("unexpected capture format ({bpp} bits a pixel)"),
        );
    }
    let provider = CGImage::data_provider(Some(img));
    let Some(data) = CGDataProvider::data(provider.as_deref()) else {
        return err(ErrorCode::Failed, "the capture had no pixels");
    };
    // SAFETY: the bytes stay alive while `data` does.
    let bytes = unsafe { data.as_bytes_unchecked() };
    let (w, h) = (w.min(iw), h.min(ih));
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        let row = &bytes[y * bpr..];
        for x in 0..w as usize {
            let s = &row[x * 4..x * 4 + 4];
            let d = (y * w as usize + x) * 4;
            // BGRA in memory (little-endian, alpha first) to RGBA.
            out[d] = s[2];
            out[d + 1] = s[1];
            out[d + 2] = s[0];
            out[d + 3] = s[3];
        }
    }
    Ok(Rgba {
        width: w,
        height: h,
        data: out,
    })
}
