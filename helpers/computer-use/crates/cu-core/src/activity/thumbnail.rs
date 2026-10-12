//! Thumbnail geometry for the timeline filmstrip.

/// Default long edge of a stored thumbnail, in pixels.
pub const THUMBNAIL_LONG_EDGE: u32 = 320;

/// JPEG quality used for thumbnails.
pub const THUMBNAIL_JPEG_QUALITY: u8 = 70;

/// Scales `(width, height)` so the long edge is at most `long_edge`, keeping
/// the aspect ratio. Never upscales, never returns a zero dimension, and
/// returns `None` for an empty source.
pub fn thumbnail_dimensions(width: u32, height: u32, long_edge: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 || long_edge == 0 {
        return None;
    }
    let source_long = width.max(height);
    if source_long <= long_edge {
        return Some((width, height));
    }
    let scale = |side: u32| -> u32 {
        let scaled = (u64::from(side) * u64::from(long_edge) + u64::from(source_long) / 2)
            / u64::from(source_long);
        (scaled as u32).max(1)
    };
    if width >= height {
        Some((long_edge, scale(height)))
    } else {
        Some((scale(width), long_edge))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_long_edge_and_keeps_aspect() {
        assert_eq!(thumbnail_dimensions(2560, 1600, 320), Some((320, 200)));
        assert_eq!(thumbnail_dimensions(1600, 2560, 320), Some((200, 320)));
        assert_eq!(thumbnail_dimensions(1000, 1000, 320), Some((320, 320)));
    }

    #[test]
    fn never_upscales_or_collapses() {
        assert_eq!(thumbnail_dimensions(200, 100, 320), Some((200, 100)));
        assert_eq!(thumbnail_dimensions(10_000, 3, 320), Some((320, 1)));
        assert_eq!(thumbnail_dimensions(0, 100, 320), None);
        assert_eq!(thumbnail_dimensions(100, 100, 0), None);
    }
}
