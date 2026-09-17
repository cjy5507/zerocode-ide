//! QA's screen comparison (docs/design/computer-use-full-operator.md §4):
//! a baseline image against the screen (or another image), pixel by pixel,
//! with the table's tolerance; the answer is a share, a pass, and a diff
//! image a person can look at.

use serde_json::{Value, json};
use zerocode_core::computer_use::{
    COMPUTER_COMPARE_PIXEL_DELTA, OBSERVE_DIFF_CELL, OBSERVE_DIFF_RECTS, compare_verdict,
};
use zerocode_core::computer_use_protocol::render::Rect;

use super::ComputerUseError;
use super::screenshot_png::RgbaImage;
use zerocode_core::computer_use_protocol::error_code;

/// A PNG's width and height, read off its header without decoding it.
#[must_use]
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let info = decoder.read_info().ok()?;
    let header = info.info();
    Some((header.width, header.height))
}

/// PNG bytes as straight RGBA, whatever the file's own colour type.
pub(crate) fn decode_png(bytes: &[u8]) -> Option<RgbaImage> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let raw = &buffer[..info.buffer_size()];
    let pixels: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => raw.to_vec(),
        png::ColorType::Rgb => raw
            .chunks(3)
            .flat_map(|px| [px[0], px[1], px[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => raw
            .chunks(2)
            .flat_map(|px| [px[0], px[0], px[0], px[1]])
            .collect(),
        png::ColorType::Grayscale => raw.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    RgbaImage::new(info.width, info.height, pixels)
}

/// What differed, where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Diff {
    pub width: u32,
    pub height: u32,
    pub different: usize,
    pub mask: Vec<bool>,
}

/// Bring two images to one size: the larger is box-filtered down to the
/// smaller's scale, then both are cut to the common corner.
fn matched(a: &RgbaImage, b: &RgbaImage) -> (RgbaImage, RgbaImage) {
    let scale_a =
        (f64::from(b.width) / f64::from(a.width)).min(f64::from(b.height) / f64::from(a.height));
    let scale_b =
        (f64::from(a.width) / f64::from(b.width)).min(f64::from(a.height) / f64::from(b.height));
    let a = if scale_a < 1.0 {
        a.downscaled(scale_a)
    } else {
        a.clone()
    };
    let b = if scale_b < 1.0 {
        b.downscaled(scale_b)
    } else {
        b.clone()
    };
    (a, b)
}

/// Pixels whose any channel drifts past `delta` are different; the compared
/// area is the two images' common corner.
pub(super) fn diff(baseline: &RgbaImage, against: &RgbaImage, delta: u8) -> Diff {
    let (a, b) = matched(baseline, against);
    let width = a.width.min(b.width);
    let height = a.height.min(b.height);
    let mut mask = Vec::with_capacity(width as usize * height as usize);
    let mut different = 0;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let at_a = (y * a.width as usize + x) * 4;
            let at_b = (y * b.width as usize + x) * 4;
            let differs = a.pixels[at_a..at_a + 4]
                .iter()
                .zip(&b.pixels[at_b..at_b + 4])
                .any(|(&p, &q)| p.abs_diff(q) > delta);
            if differs {
                different += 1;
            }
            mask.push(differs);
        }
    }
    Diff {
        width,
        height,
        different,
        mask,
    }
}

/// One changed area, in the compared image's pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// What changed, as rectangles: the mask is coarsened to a grid of `cell`
/// pixels, touching changed cells are one area, and the largest `cap`
/// areas are answered — the way a person says "the top-right corner moved".
pub(crate) fn changed_regions(diff: &Diff, cell: u32, cap: usize) -> Vec<Region> {
    let cell = cell.max(1) as usize;
    let columns = (diff.width as usize).div_ceil(cell);
    let rows = (diff.height as usize).div_ceil(cell);
    if columns == 0 || rows == 0 {
        return Vec::new();
    }
    let mut hot = vec![false; columns * rows];
    for y in 0..diff.height as usize {
        for x in 0..diff.width as usize {
            if diff.mask[y * diff.width as usize + x] {
                hot[(y / cell) * columns + x / cell] = true;
            }
        }
    }
    hot_regions(&hot, columns, cell, (diff.width, diff.height), cap)
}

/// Where repaints fell (screen points), as the areas a diff answers: laid on
/// a grid of `OBSERVE_DIFF_CELL` points, touching cells one area, the
/// largest `OBSERVE_DIFF_RECTS` answered — in screen points.
pub(crate) fn rect_regions(rects: &[Rect]) -> Vec<Value> {
    let Some(first) = rects.first() else {
        return Vec::new();
    };
    let (x0, y0, x1, y1) = rects.iter().fold(
        (first.x, first.y, first.max_x(), first.max_y()),
        |(x0, y0, x1, y1), rect| {
            (
                x0.min(rect.x),
                y0.min(rect.y),
                x1.max(rect.max_x()),
                y1.max(rect.max_y()),
            )
        },
    );
    let width = (x1 - x0).ceil().max(1.0) as u32;
    let height = (y1 - y0).ceil().max(1.0) as u32;
    let cell = OBSERVE_DIFF_CELL.max(1);
    let columns = width.div_ceil(cell) as usize;
    let rows = height.div_ceil(cell) as usize;
    let size = f64::from(cell);
    let mut hot = vec![false; columns * rows];
    for rect in rects {
        let first_column = ((rect.x - x0) / size).floor() as usize;
        let first_row = ((rect.y - y0) / size).floor() as usize;
        let last_column =
            (((rect.max_x() - x0) / size).ceil() as usize).clamp(first_column + 1, columns);
        let last_row = (((rect.max_y() - y0) / size).ceil() as usize).clamp(first_row + 1, rows);
        for row in first_row.min(rows - 1)..last_row {
            for column in first_column.min(columns - 1)..last_column {
                hot[row * columns + column] = true;
            }
        }
    }
    hot_regions(
        &hot,
        columns,
        cell as usize,
        (width, height),
        OBSERVE_DIFF_RECTS,
    )
    .into_iter()
    .map(|region| {
        json!({
            "x": x0 + f64::from(region.x),
            "y": y0 + f64::from(region.y),
            "width": region.width,
            "height": region.height,
        })
    })
    .collect()
}

/// Touching hot cells of a grid `columns` wide, each `cell` units over an
/// image of `size`, as areas — the largest `cap` of them.
fn hot_regions(
    hot: &[bool],
    columns: usize,
    cell: usize,
    size: (u32, u32),
    cap: usize,
) -> Vec<Region> {
    let rows = hot.len() / columns.max(1);
    let mut seen = vec![false; columns * rows];
    let mut regions = Vec::new();
    for start in 0..columns * rows {
        if !hot[start] || seen[start] {
            continue;
        }
        let (mut min_c, mut min_r, mut max_c, mut max_r) = (columns, rows, 0, 0);
        let mut stack = vec![start];
        seen[start] = true;
        while let Some(at) = stack.pop() {
            let (col, row) = (at % columns, at / columns);
            min_c = min_c.min(col);
            max_c = max_c.max(col);
            min_r = min_r.min(row);
            max_r = max_r.max(row);
            let neighbours = [
                (col > 0).then(|| at - 1),
                (col + 1 < columns).then(|| at + 1),
                (row > 0).then(|| at - columns),
                (row + 1 < rows).then(|| at + columns),
            ];
            for next in neighbours.into_iter().flatten() {
                if hot[next] && !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        let x = (min_c * cell) as u32;
        let y = (min_r * cell) as u32;
        let right = (((max_c + 1) * cell) as u32).min(size.0);
        let bottom = (((max_r + 1) * cell) as u32).min(size.1);
        regions.push(Region {
            x,
            y,
            width: right - x,
            height: bottom - y,
        });
    }
    regions.sort_by_key(|region| {
        std::cmp::Reverse(u64::from(region.width) * u64::from(region.height))
    });
    regions.truncate(cap);
    regions
}

/// The diff as a picture: the screen, dimmed, with every differing pixel red.
pub(super) fn diff_png(against: &RgbaImage, diff: &Diff) -> Option<Vec<u8>> {
    let (_, shown) = matched(against, against);
    let mut pixels = Vec::with_capacity(diff.width as usize * diff.height as usize * 4);
    for y in 0..diff.height as usize {
        for x in 0..diff.width as usize {
            let at = (y * shown.width as usize + x) * 4;
            let differs = diff.mask[y * diff.width as usize + x];
            if differs {
                pixels.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                let px = &shown.pixels[at..at + 4];
                pixels.extend_from_slice(&[px[0] / 2 + 64, px[1] / 2 + 64, px[2] / 2 + 64, 255]);
            }
        }
    }
    RgbaImage::new(diff.width, diff.height, pixels)?.encode()
}

/// The comparison, from bytes to the answer's fields and the diff picture.
pub(crate) fn compare(
    baseline: &[u8],
    against: &[u8],
    max_diff: Option<f64>,
) -> Result<(Value, Vec<u8>), ComputerUseError> {
    let baseline = decode_png(baseline).ok_or_else(|| {
        ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            "the baseline is not a PNG this window can read",
        )
    })?;
    let current = decode_png(against).ok_or_else(|| {
        ComputerUseError::new(
            error_code::INVALID_ARGUMENT,
            "the image to compare against is not a PNG this window can read",
        )
    })?;
    let report = diff(&baseline, &current, COMPUTER_COMPARE_PIXEL_DELTA);
    let total = report.width as usize * report.height as usize;
    let (ratio, pass) = compare_verdict(report.different, total, max_diff);
    let picture = diff_png(&current, &report).ok_or_else(|| {
        ComputerUseError::new(
            error_code::SCREENSHOT_FAILED,
            "encoding the diff picture failed",
        )
    })?;
    Ok((
        serde_json::json!({
            "pass": pass,
            "diffRatio": ratio,
            "differentPixels": report.different,
            "totalPixels": total,
            "width": report.width,
            "height": report.height,
            "maxDiff": max_diff.unwrap_or(zerocode_core::computer_use::COMPUTER_COMPARE_MAX_DIFF),
            "pixelDelta": COMPUTER_COMPARE_PIXEL_DELTA,
            "baselineSize": [baseline.width, baseline.height],
            "againstSize": [current.width, current.height],
        }),
        picture,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&paint(x, y));
            }
        }
        RgbaImage::new(width, height, pixels).expect("image")
    }

    #[test]
    fn identical_pictures_pass_and_one_changed_pixel_in_four_fails_the_bar() {
        let same = picture(2, 2, |_, _| [10, 20, 30, 255]);
        let png = same.encode().expect("png");
        let (answer, diff) = compare(&png, &png, None).expect("compare");
        assert_eq!(answer["pass"], true);
        assert_eq!(answer["differentPixels"], 0);
        assert!(decode_png(&diff).is_some(), "the diff is a picture");

        let changed = picture(2, 2, |x, y| {
            if (x, y) == (1, 1) {
                [200, 20, 30, 255]
            } else {
                [10, 20, 30, 255]
            }
        });
        let (answer, _) = compare(&png, &changed.encode().unwrap(), None).expect("compare");
        assert_eq!(answer["pass"], false);
        assert_eq!(answer["differentPixels"], 1);
        assert_eq!(answer["diffRatio"], 0.25);
        let (lenient, _) = compare(&png, &changed.encode().unwrap(), Some(0.5)).expect("compare");
        assert_eq!(lenient["pass"], true, "the caller's bar");
    }

    #[test]
    fn changed_areas_are_the_touching_cells_largest_first_and_capped() {
        let before = picture(96, 48, |_, _| [0, 0, 0, 255]);
        // Two blobs: a 30×20 one at the top-left and a 10×10 one at the bottom-right.
        let after = picture(96, 48, |x, y| {
            if (x < 30 && y < 20) || (x >= 80 && y >= 36) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        let report = diff(&before, &after, 0);
        let regions = changed_regions(&report, 8, 10);
        assert_eq!(regions.len(), 2, "{regions:?}");
        assert_eq!(
            regions[0],
            Region {
                x: 0,
                y: 0,
                width: 32,
                height: 24
            },
            "the larger first, on cell bounds"
        );
        assert_eq!(
            regions[1],
            Region {
                x: 80,
                y: 32,
                width: 16,
                height: 16
            },
            "clipped to the image"
        );
        assert_eq!(changed_regions(&report, 8, 1).len(), 1, "the cap");
        let nothing = diff(&before, &before, 0);
        assert!(changed_regions(&nothing, 8, 10).is_empty());
    }

    #[test]
    fn a_drift_under_the_table_is_not_a_difference_and_sizes_are_matched() {
        let base = picture(4, 4, |_, _| [100, 100, 100, 255]);
        let drifted = picture(4, 4, |_, _| {
            [100 + COMPUTER_COMPARE_PIXEL_DELTA, 100, 100, 255]
        });
        let report = diff(&base, &drifted, COMPUTER_COMPARE_PIXEL_DELTA);
        assert_eq!(report.different, 0, "at the delta is not past it");
        let bigger = picture(8, 8, |_, _| [100, 100, 100, 255]);
        let report = diff(&base, &bigger, COMPUTER_COMPARE_PIXEL_DELTA);
        assert_eq!(
            (report.width, report.height),
            (4, 4),
            "the larger comes down to the smaller"
        );
        assert_eq!(report.different, 0);
        assert!(decode_png(b"not a png").is_none());
        let grey = {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0, 255]).unwrap();
            writer.finish().unwrap();
            bytes
        };
        let decoded = decode_png(&grey).expect("grey reads");
        assert_eq!(decoded.pixels, vec![0, 0, 0, 255, 255, 255, 255, 255]);
    }
}
