//! The arithmetic a synthetic pointer needs, kept away from the calls that
//! use it so a machine without the platform can still prove it.

use zerocode_core::computer_use_protocol::render::Rect;

/// A screen point as `SendInput` wants it with `MOUSEEVENTF_ABSOLUTE |
/// MOUSEEVENTF_VIRTUALDESK`: 0..=65535 across the whole virtual desktop, so
/// a monitor left of or above the primary one (negative coordinates) is as
/// reachable as any other.
///
/// Windows turns the normalized value back into a pixel by truncation —
/// `pixel = floor(n * extent / 65536)` — so the value for a pixel is the
/// one at the CENTRE of that pixel's cell, `(offset + 0.5) * 65536 / extent`
/// truncated, never the rounded left edge: rounding puts pixel 1 of a 3840
/// wide desktop at 17, which maps back to pixel 0. Every pixel of every
/// desktop width round-trips (the test below walks them). Off-desktop
/// points clamp, because a point off the desktop is not an error worth
/// failing a click over — the fence around the click catches a wrong
/// recipient.
#[must_use]
pub(super) fn virtual_desk_coordinate(x: f64, y: f64, virtual_screen: &Rect) -> (i32, i32) {
    let scale = |value: f64, origin: f64, extent: f64| -> i32 {
        if extent <= 0.0 {
            return 0;
        }
        let normalized = ((value - origin) + 0.5) * 65536.0 / extent;
        normalized.clamp(0.0, 65535.0).floor() as i32
    };
    (
        scale(x, virtual_screen.x, virtual_screen.width),
        scale(y, virtual_screen.y, virtual_screen.height),
    )
}

/// One `WHEEL_DELTA` notch scrolls three lines by default, and the helper's
/// page is twelve lines: four notches per page, signed the way Windows
/// signs a wheel (positive is up / left-to-right-negative for horizontal).
#[must_use]
pub(super) fn wheel_delta(pages: f64, towards_start: bool) -> i32 {
    const WHEEL_DELTA: f64 = 120.0;
    let notches = (4.0 * pages).round().max(1.0);
    let magnitude = (notches * WHEEL_DELTA).min(f64::from(i32::MAX / 2)) as i32;
    if towards_start { magnitude } else { -magnitude }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What `SendInput` does with a normalized coordinate, per the
    /// documented mapping: the pixel is the truncated product.
    fn pixel_of(normalized: i32, extent: f64) -> i64 {
        (f64::from(normalized) * extent / 65536.0).floor() as i64
    }

    #[test]
    fn points_map_across_the_virtual_desktop_including_negative_monitors() {
        // A second monitor to the left: the desktop starts at -1920.
        let desktop = Rect::new(-1920.0, 0.0, 3840.0, 1080.0);
        assert_eq!(virtual_desk_coordinate(-1920.0, 0.0, &desktop), (8, 30));
        assert_eq!(
            virtual_desk_coordinate(0.0, 540.0, &desktop),
            (32776, 32798)
        );
        assert_eq!(
            virtual_desk_coordinate(1919.0, 1079.0, &desktop),
            (65527, 65505)
        );
        // Off the desktop clamps instead of wrapping.
        assert_eq!(virtual_desk_coordinate(-5000.0, -5.0, &desktop), (0, 0));
        assert_eq!(
            virtual_desk_coordinate(9000.0, 9000.0, &desktop),
            (65535, 65535)
        );
        assert_eq!(
            virtual_desk_coordinate(3.0, 3.0, &Rect::new(0.0, 0.0, 0.0, 0.0)),
            (0, 0)
        );
    }

    /// L5: every pixel of every common desktop width, at a zero and a
    /// negative origin, comes back as itself after Windows truncates —
    /// including pixel 1 of a 3840-wide desktop, which rounding lost.
    #[test]
    fn every_pixel_round_trips_through_the_normalized_coordinate() {
        for extent in [800.0, 1366.0, 1920.0, 2560.0, 3840.0, 5120.0, 7680.0] {
            for origin in [0.0, -1920.0, -2560.0] {
                let desktop = Rect::new(origin, origin, extent, extent);
                for pixel in 0..(extent as i64) {
                    let (nx, ny) = virtual_desk_coordinate(
                        origin + pixel as f64,
                        origin + pixel as f64,
                        &desktop,
                    );
                    assert_eq!(
                        pixel_of(nx, extent),
                        pixel,
                        "x pixel {pixel} of {extent} at origin {origin} → {nx}"
                    );
                    assert_eq!(pixel_of(ny, extent), pixel);
                }
            }
        }
        // The old rounding: pixel 1 of 3840 was 17, which truncates to 0.
        assert_eq!(pixel_of(17, 3840.0), 0);
        assert_eq!(
            pixel_of(
                virtual_desk_coordinate(1.0, 0.0, &Rect::new(0.0, 0.0, 3840.0, 1.0)).0,
                3840.0
            ),
            1
        );
    }

    #[test]
    fn a_page_is_four_notches_signed_like_windows() {
        assert_eq!(wheel_delta(1.0, false), -480);
        assert_eq!(wheel_delta(1.0, true), 480);
        assert_eq!(wheel_delta(0.5, false), -240);
        assert_eq!(wheel_delta(0.01, true), 120, "never less than one notch");
        assert_eq!(wheel_delta(2.5, false), -1200);
    }
}
