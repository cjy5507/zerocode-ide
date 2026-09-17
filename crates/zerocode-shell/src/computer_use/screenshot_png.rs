//! A captured window as a bounded PNG.
//!
//! The macOS helper's `boundedPngData`: encode at each rung of the resize
//! ladder (`screenshot_resize_ladder`, whose first rung is the budget's long
//! edge) until the bytes fit, keeping the smallest rung if none does. Platform-neutral — a
//! Windows capture hands over RGBA and gets the same bytes-per-frame
//! discipline the helper keeps, and the arithmetic is tested here rather
//! than on a machine that can only be reached through CI.

use zerocode_core::computer_use_protocol::{MAX_SCREENSHOT_PNG_BYTES, screenshot_resize_ladder};

/// Straight RGBA, row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    #[must_use]
    pub(crate) fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        (pixels.len() == expected && width > 0 && height > 0).then_some(Self {
            width,
            height,
            pixels,
        })
    }

    /// Box-filter downscale to `scale` of the size (at least one pixel each
    /// way). Every source pixel counts once; edge cells average whatever
    /// falls in them.
    #[must_use]
    pub(super) fn downscaled(&self, scale: f64) -> Self {
        let width = ((f64::from(self.width) * scale).round() as u32).max(1);
        let height = ((f64::from(self.height) * scale).round() as u32).max(1);
        if width >= self.width && height >= self.height {
            return self.clone();
        }
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for y in 0..height {
            let sy0 = (y as usize * self.height as usize) / height as usize;
            let sy1 = (((y as usize + 1) * self.height as usize) / height as usize).max(sy0 + 1);
            for x in 0..width {
                let sx0 = (x as usize * self.width as usize) / width as usize;
                let sx1 = (((x as usize + 1) * self.width as usize) / width as usize).max(sx0 + 1);
                let mut sum = [0u64; 4];
                let mut count = 0u64;
                for sy in sy0..sy1.min(self.height as usize) {
                    for sx in sx0..sx1.min(self.width as usize) {
                        let at = (sy * self.width as usize + sx) * 4;
                        for (total, sample) in sum.iter_mut().zip(&self.pixels[at..at + 4]) {
                            *total += u64::from(*sample);
                        }
                        count += 1;
                    }
                }
                let at = (y as usize * width as usize + x as usize) * 4;
                for (cell, total) in pixels[at..at + 4].iter_mut().zip(sum) {
                    *cell = (total / count.max(1)) as u8;
                }
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }

    /// PNG bytes for this image as it is.
    pub(crate) fn encode(&self) -> Option<Vec<u8>> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, self.width, self.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().ok()?;
            writer.write_image_data(&self.pixels).ok()?;
            writer.finish().ok()?;
        }
        Some(bytes)
    }
}

/// A PNG within the budget, or the smallest rung when nothing fits.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) struct BoundedPng {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// `boundedPngData`: the ladder's rungs in order, each encoded once, the
/// first that fits kept — the smallest rung when none does.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(super) fn bounded_png(image: &RgbaImage) -> Option<BoundedPng> {
    let mut best = None;
    for scale in screenshot_resize_ladder(image.width, image.height) {
        let downscaled;
        let rung = if scale < 1.0 {
            downscaled = image.downscaled(scale);
            &downscaled
        } else {
            image
        };
        let Some(data) = rung.encode() else {
            break;
        };
        let fits = data.len() <= MAX_SCREENSHOT_PNG_BYTES;
        best = Some(BoundedPng {
            data,
            width: rung.width,
            height: rung.height,
        });
        if fits {
            break;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noise(width: u32, height: u32) -> RgbaImage {
        // Incompressible pixels, so the byte budget actually bites.
        let mut state = 0x9e37_79b9_u32;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width * height {
            for _ in 0..4 {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                pixels.push((state & 0xff) as u8);
            }
        }
        RgbaImage::new(width, height, pixels).unwrap()
    }

    #[test]
    fn a_small_image_is_encoded_as_it_is() {
        let image = RgbaImage::new(4, 3, vec![200u8; 4 * 3 * 4]).unwrap();
        let png = bounded_png(&image).unwrap();
        assert_eq!((png.width, png.height), (4, 3));
        assert!(png.data.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert!(RgbaImage::new(2, 2, vec![0; 3]).is_none());
        assert!(RgbaImage::new(0, 2, vec![]).is_none());
    }

    #[test]
    fn a_large_image_walks_the_ladder_until_it_fits() {
        let image = noise(1600, 1000);
        assert!(image.encode().unwrap().len() > MAX_SCREENSHOT_PNG_BYTES);
        let png = bounded_png(&image).unwrap();
        assert!(
            png.data.len() <= MAX_SCREENSHOT_PNG_BYTES,
            "{} bytes",
            png.data.len()
        );
        assert!(png.width < 1600 && png.height < 1000);
        assert_eq!(png.width * 1000 / png.height, 1600, "aspect kept");
    }

    /// A picture past the budget's long edge comes at that edge even when
    /// its full-resolution PNG would have fit: the full encode is never paid.
    #[test]
    fn a_large_picture_comes_at_the_budgets_long_edge_whatever_it_compresses_to() {
        let flat = RgbaImage::new(2560, 1600, vec![128u8; 2560 * 1600 * 4]).unwrap();
        assert!(
            flat.encode().unwrap().len() <= MAX_SCREENSHOT_PNG_BYTES,
            "it would have fit"
        );
        let png = bounded_png(&flat).unwrap();
        assert_eq!((png.width, png.height), (1280, 800));
    }

    #[test]
    fn downscaling_averages_the_cells_it_folds() {
        let mut pixels = Vec::new();
        for y in 0..2u8 {
            for x in 0..2u8 {
                pixels.extend_from_slice(&[x * 100, y * 100, 50, 255]);
            }
        }
        let image = RgbaImage::new(2, 2, pixels).unwrap();
        let half = image.downscaled(0.5);
        assert_eq!((half.width, half.height), (1, 1));
        assert_eq!(half.pixels, vec![50, 50, 50, 255]);
        assert_eq!(image.downscaled(2.0), image, "never upscaled");
        let tiny = image.downscaled(0.01);
        assert_eq!((tiny.width, tiny.height), (1, 1));
    }
}
