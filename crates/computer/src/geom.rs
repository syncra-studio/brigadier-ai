//! Points, rectangles and the transform that maps an image's pixels back to the desktop.
//!
//! Every image the engine hands out records exactly how it was made (§4.3 of
//! docs/COMPUTER-USE-PLAN.md): which window, where that window was, which part of it was
//! captured and at what scale. A point a model reads off the image goes back through that
//! record and nothing else, so there is no fixed "halve it for Retina" rule anywhere.

use serde::{Deserialize, Serialize};

/// A point in points (not pixels), in whatever space the field's name says.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A rectangle with its origin at the top left, y growing downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    pub fn center(&self) -> Point {
        Point::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.x + self.w && p.y < self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = (self.x + self.w).min(o.x + o.w);
        let y1 = (self.y + self.h).min(o.y + o.h);
        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }

    /// The same rectangle relative to `origin`.
    pub fn relative_to(&self, origin: Point) -> Rect {
        Rect::new(self.x - origin.x, self.y - origin.y, self.w, self.h)
    }
}

/// How one image was made from one window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageTransform {
    /// The window the image shows.
    pub window: u32,
    /// The window's frame on the desktop when the image was taken, in global points.
    pub window_frame: Rect,
    /// The part of the window the image covers, in window points (top left of the frame).
    pub crop: Rect,
    /// Image pixels per window point, the same on both axes.
    pub scale: f64,
    /// The image's size in pixels.
    pub width: u32,
    pub height: u32,
}

/// Why an image point can't be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// The point lies outside the image.
    OutsideImage,
    /// The window changed size since the image was taken: what the image shows is gone.
    StaleGeometry,
}

impl ImageTransform {
    /// The output size and scale for capturing `crop` (window points) so that the image is at
    /// most `max_side` pixels on each side and at most `pixels_per_point` pixels per point.
    pub fn fit(
        window: u32,
        window_frame: Rect,
        crop: Rect,
        pixels_per_point: f64,
        max_side: u32,
    ) -> Self {
        let longest = crop.w.max(crop.h).max(1.0);
        let scale = pixels_per_point.min(f64::from(max_side) / longest);
        Self {
            window,
            window_frame,
            crop,
            scale,
            width: ((crop.w * scale).round() as u32).max(1),
            height: ((crop.h * scale).round() as u32).max(1),
        }
    }

    /// Maps a pixel of this image to a point in the window (top left of the frame).
    /// `current_frame` is the window's frame now: a resize since the capture makes every point
    /// on the image stale, while a move doesn't (window points don't move with the window).
    pub fn to_window(&self, px: f64, py: f64, current_frame: Rect) -> Result<Point, MapError> {
        if (current_frame.w - self.window_frame.w).abs() > 0.5
            || (current_frame.h - self.window_frame.h).abs() > 0.5
        {
            return Err(MapError::StaleGeometry);
        }
        if px < 0.0 || py < 0.0 || px >= f64::from(self.width) || py >= f64::from(self.height) {
            return Err(MapError::OutsideImage);
        }
        Ok(Point::new(
            self.crop.x + px / self.scale,
            self.crop.y + py / self.scale,
        ))
    }

    /// Maps a window point to this image's pixels (to draw a predicted point, or to redact).
    pub fn to_image(&self, p: Point) -> Point {
        Point::new(
            (p.x - self.crop.x) * self.scale,
            (p.y - self.crop.y) * self.scale,
        )
    }

    /// The part of this image named by a pixel rectangle, in window points, clipped to the image.
    pub fn region_to_window(&self, region: Rect) -> Rect {
        let clipped = region.intersect(&Rect::new(
            0.0,
            0.0,
            f64::from(self.width),
            f64::from(self.height),
        ));
        Rect::new(
            self.crop.x + clipped.x / self.scale,
            self.crop.y + clipped.y / self.scale,
            clipped.w / self.scale,
            clipped.h / self.scale,
        )
    }
}

/// The estimated cost of an image for the worker's provider, labelled as an estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
}

/// Visual tokens of a `w`×`h` image: Claude counts 28 px tiles, Codex's models 32 px patches
/// times 1.2 (§11 of the plan).
pub fn image_tokens(provider: Provider, w: u32, h: u32) -> u32 {
    match provider {
        Provider::Claude => w.div_ceil(28) * h.div_ceil(28),
        Provider::Codex => (f64::from(w.div_ceil(32) * h.div_ceil(32)) * 1.2).ceil() as u32,
    }
}

/// The most pixels per side an image may have: Claude's limit once a request holds more than
/// 20 images.
pub const MAX_IMAGE_SIDE: u32 = 2000;
/// The most Claude visual tokens one image may cost.
pub const MAX_IMAGE_TOKENS: u32 = 4784;
/// The most pixels per side of a zoom crop.
pub const MAX_ZOOM_SIDE: u32 = 1024;

/// The scale (pixels per point, at most `preferred`) at which a `w`×`h` point region fits both
/// the side limit and the token limit.
pub fn fitting_scale(w: f64, h: f64, preferred: f64, max_side: u32) -> f64 {
    let mut scale = preferred.min(f64::from(max_side) / w.max(h).max(1.0));
    // Shrink until the Claude estimate fits; a few steps at most.
    for _ in 0..64 {
        let tokens = image_tokens(
            Provider::Claude,
            (w * scale).round() as u32,
            (h * scale).round() as u32,
        );
        if tokens <= MAX_IMAGE_TOKENS {
            break;
        }
        scale *= 0.97;
    }
    scale
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Rect = Rect::new(200.0, 585.0, 640.0, 332.0);

    #[test]
    fn one_pixel_per_point_maps_straight_through() {
        let t = ImageTransform::fit(
            7,
            FRAME,
            Rect::new(0.0, 0.0, 640.0, 332.0),
            1.0,
            MAX_IMAGE_SIDE,
        );
        assert_eq!((t.width, t.height, t.scale), (640, 332, 1.0));
        assert_eq!(t.to_window(70.0, 68.0, FRAME), Ok(Point::new(70.0, 68.0)));
    }

    #[test]
    fn retina_pixels_halve_back_to_points() {
        let t = ImageTransform::fit(
            7,
            FRAME,
            Rect::new(0.0, 0.0, 640.0, 332.0),
            2.0,
            MAX_IMAGE_SIDE,
        );
        assert_eq!((t.width, t.height), (1280, 664));
        assert_eq!(t.to_window(141.0, 137.0, FRAME), Ok(Point::new(70.5, 68.5)));
        assert_eq!(t.to_image(Point::new(70.5, 68.5)), Point::new(141.0, 137.0));
    }

    #[test]
    fn a_scaled_down_image_maps_back_exactly() {
        let frame = Rect::new(0.0, 0.0, 3000.0, 1500.0);
        let t = ImageTransform::fit(
            7,
            frame,
            Rect::new(0.0, 0.0, 3000.0, 1500.0),
            1.0,
            MAX_IMAGE_SIDE,
        );
        assert_eq!((t.width, t.height), (2000, 1000));
        let p = t.to_window(1000.0, 500.0, frame).unwrap();
        assert!((p.x - 1500.0).abs() < 1e-9 && (p.y - 750.0).abs() < 1e-9);
    }

    #[test]
    fn a_zoom_crop_maps_through_its_own_offset_and_scale() {
        let full = ImageTransform::fit(
            7,
            FRAME,
            Rect::new(0.0, 0.0, 640.0, 332.0),
            1.0,
            MAX_IMAGE_SIDE,
        );
        // The model zooms on pixels 360..500 × 52..202 of the full image.
        let crop = full.region_to_window(Rect::new(360.0, 52.0, 140.0, 150.0));
        let zoom = ImageTransform::fit(7, FRAME, crop, 2.0, MAX_ZOOM_SIDE);
        assert_eq!((zoom.width, zoom.height), (280, 300));
        // Pixel (80, 136) of the zoom is 40, 68 points into the crop.
        assert_eq!(
            zoom.to_window(80.0, 136.0, FRAME),
            Ok(Point::new(400.0, 120.0))
        );
    }

    #[test]
    fn a_large_zoom_is_bounded() {
        let crop = Rect::new(0.0, 0.0, 900.0, 300.0);
        let zoom = ImageTransform::fit(7, FRAME, crop, 2.0, MAX_ZOOM_SIDE);
        assert_eq!(zoom.width, MAX_ZOOM_SIDE);
        let p = zoom.to_window(1023.0, 0.0, FRAME).unwrap();
        assert!(p.x < 900.0);
    }

    #[test]
    fn a_resized_window_makes_the_image_stale_but_a_moved_one_does_not() {
        let t = ImageTransform::fit(
            7,
            FRAME,
            Rect::new(0.0, 0.0, 640.0, 332.0),
            1.0,
            MAX_IMAGE_SIDE,
        );
        let moved = Rect::new(10.0, 10.0, 640.0, 332.0);
        assert!(t.to_window(5.0, 5.0, moved).is_ok());
        let resized = Rect::new(200.0, 585.0, 700.0, 332.0);
        assert_eq!(t.to_window(5.0, 5.0, resized), Err(MapError::StaleGeometry));
        assert_eq!(t.to_window(640.0, 5.0, FRAME), Err(MapError::OutsideImage));
    }

    #[test]
    fn token_estimates_follow_each_provider() {
        assert_eq!(image_tokens(Provider::Claude, 1280, 720), 1196);
        assert_eq!(image_tokens(Provider::Claude, 1920, 1080), 2691);
        assert_eq!(image_tokens(Provider::Codex, 1280, 720), 1104);
        let s = fitting_scale(1728.0, 1117.0, 2.0, MAX_IMAGE_SIDE);
        let tokens = image_tokens(
            Provider::Claude,
            (1728.0 * s).round() as u32,
            (1117.0 * s).round() as u32,
        );
        assert!(tokens <= MAX_IMAGE_TOKENS, "{tokens}");
    }
}
