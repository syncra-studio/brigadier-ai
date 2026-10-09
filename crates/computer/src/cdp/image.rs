//! Page images: the browser's PNG decoded, and resampled to the size an observation sends.

use crate::error::{CuError, CuResult, ErrorCode};
use crate::geom::Rect;
use crate::redact::Rgba;

pub fn decode_png(bytes: &[u8]) -> CuResult<Rgba> {
    let bad =
        |e: png::DecodingError| CuError::new(ErrorCode::Failed, format!("the page image: {e}"));
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = dec.read_info().map_err(bad)?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| CuError::new(ErrorCode::Failed, "the page image is too large"))?;
    let mut buf = vec![0; size];
    let info = reader.next_frame(&mut buf).map_err(bad)?;
    let (w, h) = (info.width, info.height);
    let px = (w * h) as usize;
    let data = match info.color_type {
        png::ColorType::Rgba => buf[..px * 4].to_vec(),
        png::ColorType::Rgb => buf[..px * 3]
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf[..px * 2]
            .chunks(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => buf[..px].iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => {
            return Err(CuError::new(ErrorCode::Failed, "an indexed page image"));
        }
    };
    Ok(Rgba {
        width: w,
        height: h,
        data,
    })
}

/// The part `crop` (source pixels) of `src`, averaged down or sampled up to `w` × `h`.
pub fn resample(src: &Rgba, crop: Rect, w: u32, h: u32) -> Rgba {
    let mut data = vec![0u8; (w * h * 4) as usize];
    let sx = crop.w / f64::from(w);
    let sy = crop.h / f64::from(h);
    for y in 0..h {
        let y0 = crop.y + f64::from(y) * sy;
        let y1 = (y0 + sy).max(y0 + 1.0);
        for x in 0..w {
            let x0 = crop.x + f64::from(x) * sx;
            let x1 = (x0 + sx).max(x0 + 1.0);
            let mut acc = [0u32; 4];
            let mut n = 0u32;
            let (ya, yb) = (
                y0.floor() as i64,
                (y1.ceil() as i64).max(y0.floor() as i64 + 1),
            );
            let (xa, xb) = (
                x0.floor() as i64,
                (x1.ceil() as i64).max(x0.floor() as i64 + 1),
            );
            for py in ya..yb {
                for px in xa..xb {
                    if px < 0 || py < 0 || px >= i64::from(src.width) || py >= i64::from(src.height)
                    {
                        continue;
                    }
                    let p = src.pixel(px as u32, py as u32);
                    for (a, v) in acc.iter_mut().zip(p) {
                        *a += u32::from(v);
                    }
                    n += 1;
                }
            }
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                data[i + c] = acc[c].checked_div(n).unwrap_or(0) as u8;
            }
        }
    }
    Rgba {
        width: w,
        height: h,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_two_x_image_averages_down_to_one_pixel_per_point() {
        let src = Rgba {
            width: 4,
            height: 2,
            data: [
                [0, 0, 0, 255],
                [255, 255, 255, 255],
                [10, 10, 10, 255],
                [10, 10, 10, 255],
            ]
            .iter()
            .cycle()
            .take(8)
            .flatten()
            .copied()
            .collect(),
        };
        let out = resample(&src, Rect::new(0.0, 0.0, 4.0, 2.0), 2, 1);
        assert_eq!(out.pixel(0, 0), [127, 127, 127, 255]);
        assert_eq!(out.pixel(1, 0), [10, 10, 10, 255]);
        let png = src.encode_png();
        assert_eq!(decode_png(&png).unwrap(), src);
    }
}
