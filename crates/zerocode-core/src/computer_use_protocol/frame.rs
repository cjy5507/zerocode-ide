//! A screenshot's frame: how a pixel in the picture and a point on the
//! screen name the same place (docs/design/computer-use-full-operator.md §1.2).
//!
//! A desktop look is the display scaled to the picture budget; the provider
//! answers the picture's `origin` (its top-left, in screen points) and its
//! `scale` (picture pixels per screen point). A model reads the picture and
//! speaks in its pixels; the CLI moves the hand in points. Every conversion
//! between the two reads this one type — the observer placing what changed,
//! and zo's `Computer` tool translating what its model said.

use serde_json::Value;

/// The point pairs a provider answers, as (x, y) keys: a rectangle's corner
/// and its centre.
pub const POINT_KEYS: [(&str, &str); 2] = [("x", "y"), ("centerX", "centerY")];
/// The lengths a provider answers beside a point.
pub const LENGTH_KEYS: [&str; 2] = ["width", "height"];
/// The key an answer names its coordinate space under.
pub const SPACE_KEY: &str = "coordinateSpace";
/// The space an answer is in once it is spoken in a picture's pixels.
pub const SHOT_SPACE: &str = "shot";
/// Hundredths of a point: finer than any display resolves, coarse enough that
/// a converted coordinate reads as a number rather than a float's noise.
const POINT_PRECISION: f64 = 100.0;

/// Where a picture sits on the screen and how many of its pixels make a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotFrame {
    origin: (f64, f64),
    scale: f64,
}

impl ShotFrame {
    /// A picture that is the screen itself: pixels are points.
    pub const UNIT: Self = Self {
        origin: (0.0, 0.0),
        scale: 1.0,
    };

    /// A frame, if the scale is a real positive number.
    #[must_use]
    pub fn new(origin: (f64, f64), scale: f64) -> Option<Self> {
        (scale.is_finite() && scale > 0.0 && origin.0.is_finite() && origin.1.is_finite())
            .then_some(Self { origin, scale })
    }

    /// The frame a look answered: `origin` beside the picture, and the
    /// picture's own `scale` (the answer's top-level `scale` when the
    /// picture does not carry one). An app's look sits at its window's
    /// corner (`snapshot.window`): the helper answers the window's picture
    /// beside the window it captured, and no origin.
    #[must_use]
    pub fn from_answer(answer: &Value) -> Option<Self> {
        let number = |pointer: &str| answer.pointer(pointer).and_then(Value::as_f64);
        let scale = number("/screenshot/scale").or_else(|| number("/scale"))?;
        let corner = |axis: &str| {
            number(&format!("/origin/{axis}"))
                .or_else(|| number(&format!("/snapshot/window/{axis}")))
                .unwrap_or(0.0)
        };
        Self::new((corner("x"), corner("y")), scale)
    }

    #[must_use]
    pub fn origin(&self) -> (f64, f64) {
        self.origin
    }

    #[must_use]
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// The screen point under a picture pixel.
    #[must_use]
    pub fn to_point(&self, pixel: [f64; 2]) -> [f64; 2] {
        [
            tidy(self.origin.0 + pixel[0] / self.scale),
            tidy(self.origin.1 + pixel[1] / self.scale),
        ]
    }

    /// The picture pixel over a screen point.
    #[must_use]
    pub fn to_pixel(&self, point: [f64; 2]) -> [f64; 2] {
        [
            tidy((point[0] - self.origin.0) * self.scale),
            tidy((point[1] - self.origin.1) * self.scale),
        ]
    }

    /// A length in the picture, in points.
    #[must_use]
    pub fn length_to_point(&self, pixels: f64) -> f64 {
        tidy(pixels / self.scale)
    }

    /// A length on the screen, in picture pixels.
    #[must_use]
    pub fn length_to_pixel(&self, points: f64) -> f64 {
        tidy(points * self.scale)
    }

    /// A picture rectangle `[x, y, width, height]`, on the screen.
    #[must_use]
    pub fn rect_to_point(&self, rect: [f64; 4]) -> [f64; 4] {
        let [x, y] = self.to_point([rect[0], rect[1]]);
        [
            x,
            y,
            self.length_to_point(rect[2]),
            self.length_to_point(rect[3]),
        ]
    }

    /// Speak an answer in this picture's pixels, in place: every object that
    /// carries a point pair (and the lengths beside it) is converted, and an
    /// object that named its space names the picture's. A picture's own
    /// size has no point beside it and is left as it is.
    pub fn pixelize(&self, value: &mut Value) {
        match value {
            Value::Array(items) => items.iter_mut().for_each(|item| self.pixelize(item)),
            Value::Object(object) => {
                let mut placed = false;
                for (x_key, y_key) in POINT_KEYS {
                    let pair = object
                        .get(x_key)
                        .and_then(Value::as_f64)
                        .zip(object.get(y_key).and_then(Value::as_f64));
                    if let Some((x, y)) = pair {
                        let [px, py] = self.to_pixel([x, y]);
                        object.insert(x_key.into(), px.into());
                        object.insert(y_key.into(), py.into());
                        placed = true;
                    }
                }
                for key in LENGTH_KEYS.into_iter().filter(|_| placed) {
                    if let Some(length) = object.get(key).and_then(Value::as_f64) {
                        object.insert(key.into(), self.length_to_pixel(length).into());
                    }
                }
                if object.contains_key(SPACE_KEY) {
                    object.insert(SPACE_KEY.into(), SHOT_SPACE.into());
                }
                object.values_mut().for_each(|child| self.pixelize(child));
            }
            _ => {}
        }
    }
}

fn tidy(value: f64) -> f64 {
    (value * POINT_PRECISION).round() / POINT_PRECISION
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// This machine's main display, measured: a 1512-point-wide screen in a
    /// 1280-pixel picture.
    fn laptop() -> ShotFrame {
        ShotFrame::new((0.0, 0.0), 1280.0 / 1512.0).expect("a frame")
    }

    #[test]
    fn a_pixel_the_model_names_lands_on_the_point_under_it() {
        // The model saw a 1280x831 picture and said [1000, 500]; the point
        // under that pixel is 18% further out, not the same number.
        assert_eq!(laptop().to_point([1000.0, 500.0]), [1181.25, 590.63]);
        assert_eq!(laptop().to_point([0.0, 0.0]), [0.0, 0.0]);
        assert_eq!(laptop().to_point([1280.0, 831.0]), [1512.0, 981.62]);
    }

    #[test]
    fn a_display_left_of_the_main_one_keeps_its_origin() {
        let left = ShotFrame::new((-2560.0, -98.0), 0.5).expect("a frame");
        assert_eq!(left.to_point([100.0, 40.0]), [-2360.0, -18.0]);
        assert_eq!(left.to_pixel([-2360.0, -18.0]), [100.0, 40.0]);
    }

    #[test]
    fn point_and_pixel_round_trip_within_a_hundredth() {
        let frame = laptop();
        for pixel in [[3.0, 7.0], [640.5, 415.5], [1279.0, 830.0]] {
            let back = frame.to_pixel(frame.to_point(pixel));
            assert!(
                (back[0] - pixel[0]).abs() <= 0.01 && (back[1] - pixel[1]).abs() <= 0.01,
                "{pixel:?} -> {back:?}"
            );
        }
    }

    #[test]
    fn a_rectangle_is_placed_by_its_corner_and_stretched_by_its_lengths() {
        assert_eq!(
            laptop().rect_to_point([128.0, 64.0, 256.0, 32.0]),
            [151.2, 75.6, 302.4, 37.8]
        );
    }

    #[test]
    fn a_frame_needs_a_real_scale() {
        assert!(ShotFrame::new((0.0, 0.0), 0.0).is_none());
        assert!(ShotFrame::new((0.0, 0.0), f64::NAN).is_none());
        assert!(ShotFrame::new((f64::INFINITY, 0.0), 1.0).is_none());
    }

    #[test]
    fn the_frame_is_read_off_a_desktop_look() {
        let look = json!({
            "screenshot": { "width": 1280, "height": 831, "scale": 0.5 },
            "origin": { "x": -2560.0, "y": -98.0 }
        });
        assert_eq!(
            ShotFrame::from_answer(&look),
            ShotFrame::new((-2560.0, -98.0), 0.5)
        );
        let observed =
            json!({ "screenshot": { "width": 10 }, "origin": { "x": 0, "y": 0 }, "scale": 2.0 });
        assert_eq!(
            ShotFrame::from_answer(&observed),
            ShotFrame::new((0.0, 0.0), 2.0)
        );
        assert_eq!(ShotFrame::from_answer(&json!({ "lines": [] })), None);
    }

    /// An app's look sits at its window's corner: the helper answers the
    /// window's picture beside the window it captured, and no `origin`.
    #[test]
    fn an_apps_look_sits_at_its_window() {
        let look = json!({
            "snapshot": { "window": { "id": 7, "x": 100, "y": 50, "width": 800, "height": 600 } },
            "screenshot": { "width": 1600, "height": 1200, "scale": 2.0 }
        });
        assert_eq!(
            ShotFrame::from_answer(&look),
            ShotFrame::new((100.0, 50.0), 2.0)
        );
        let said = json!({
            "snapshot": { "window": { "x": 100, "y": 50 } },
            "screenshot": { "scale": 2.0 },
            "origin": { "x": 1.0, "y": 2.0 }
        });
        assert_eq!(
            ShotFrame::from_answer(&said),
            ShotFrame::new((1.0, 2.0), 2.0),
            "an origin the answer says wins"
        );
    }

    #[test]
    fn an_answer_in_points_is_spoken_in_the_pictures_pixels() {
        let frame = ShotFrame::new((0.0, 0.0), 0.5).expect("a frame");
        let mut answer = json!({
            "coordinateSpace": "screen",
            "matches": [{ "x": 100.0, "y": 40.0, "width": 60.0, "height": 20.0,
                          "centerX": 130.0, "centerY": 50.0, "text": "OK", "confidence": 1 }],
            "screenshot": { "width": 1280, "height": 831, "scale": 0.5 },
            "cursor": { "x": 10, "y": 20 }
        });
        frame.pixelize(&mut answer);
        assert_eq!(answer["coordinateSpace"], "shot");
        assert_eq!(
            answer["matches"][0],
            json!({ "x": 50.0, "y": 20.0, "width": 30.0, "height": 10.0,
            "centerX": 65.0, "centerY": 25.0, "text": "OK", "confidence": 1 })
        );
        assert_eq!(
            answer["screenshot"],
            json!({ "width": 1280, "height": 831, "scale": 0.5 }),
            "the picture's own size is pixels already"
        );
        assert_eq!(answer["cursor"], json!({ "x": 5.0, "y": 10.0 }));
    }
}
