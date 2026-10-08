//! Redaction of images before they leave the engine (§4.3): secure fields are painted over in
//! the pixels themselves, so no unredacted copy exists anywhere downstream.

use crate::geom::{ImageTransform, Rect};

/// An RGBA image, 8 bits per channel, rows packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Fills a pixel rectangle (clipped to the image) with one colour.
    pub fn fill(&mut self, r: Rect, rgba: [u8; 4]) {
        let x0 = r.x.floor().max(0.0) as u32;
        let y0 = r.y.floor().max(0.0) as u32;
        let x1 = ((r.x + r.w).ceil().max(0.0) as u32).min(self.width);
        let y1 = ((r.y + r.h).ceil().max(0.0) as u32).min(self.height);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = ((y * self.width + x) * 4) as usize;
                self.data[i..i + 4].copy_from_slice(&rgba);
            }
        }
    }

    pub fn encode_png(&self) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, self.width, self.height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.set_compression(png::Compression::Fast);
            // Writing into a Vec can't fail, and the buffer's size matches the header.
            if let Ok(mut w) = enc.write_header() {
                let _ = w.write_image_data(&self.data);
            }
        }
        out
    }
}

/// Paints every secure field (window points) black in an image made with `t`.
pub fn redact(img: &mut Rgba, t: &ImageTransform, secure_fields: &[Rect]) {
    for f in secure_fields {
        let a = t.to_image(crate::geom::Point::new(f.x, f.y));
        img.fill(
            Rect::new(a.x, a.y, f.w * t.scale, f.h * t.scale),
            [0, 0, 0, 255],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secure_field_is_painted_over_at_the_image_scale() {
        let t = ImageTransform::fit(
            1,
            Rect::new(0.0, 0.0, 100.0, 50.0),
            Rect::new(10.0, 0.0, 50.0, 50.0),
            2.0,
            2000,
        );
        let mut img = Rgba {
            width: t.width,
            height: t.height,
            data: vec![255; (t.width * t.height * 4) as usize],
        };
        // A field at window points 20..30 × 5..10 is image pixels 20..40 × 10..20.
        redact(&mut img, &t, &[Rect::new(20.0, 5.0, 10.0, 5.0)]);
        assert_eq!(img.pixel(20, 10), [0, 0, 0, 255]);
        assert_eq!(img.pixel(39, 19), [0, 0, 0, 255]);
        assert_eq!(img.pixel(40, 19), [255; 4]);
        assert_eq!(img.pixel(19, 10), [255; 4]);
        assert!(img.encode_png().starts_with(b"\x89PNG"));
    }
}
