//! The eyes (docs/design/computer-use-full-operator.md §7.1): one look that
//! merges what the helper knows — the accessibility tree when an app is
//! named, the frame, the text the pixels carry with `--ocr` — and, with
//! `--diff`, what changed since the last look, as rectangles in screen
//! points. The last frames are kept here, one per viewer and place looked at
//! (an app's window, or a desktop display and region), a bounded table of
//! them, so a look at another app never counts as "what changed" in this one
//! — and an agent that names itself (`--viewer`) is never told "nothing
//! changed" against a frame another agent looked at.

use std::sync::Mutex;

use serde_json::{Map, Value};
use zerocode_core::computer_use::{
    OBSERVE_DIFF_CELL, OBSERVE_DIFF_PIXEL_DELTA, OBSERVE_DIFF_RECTS, OBSERVE_FRAMES_MAX,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::ELEMENT_FRAMES_KEY;

use super::ComputerUseError;
use super::compare::{changed_regions, decode_png, diff};
use super::screenshot_png::RgbaImage;

/// The previous look's frame: its PNG and where it sat on the screen, so a
/// change can be placed there.
struct LastFrame {
    png: Vec<u8>,
    frame: ShotFrame,
}

/// A bounded table kept per key, oldest key first, never more than its cap —
/// the last frames, one per place (`OBSERVE_FRAMES_MAX`), and the marked
/// looks' numbers, one per look (`MARK_LOOKS_KEPT`).
#[derive(Debug)]
pub(crate) struct Kept<T> {
    held: Vec<(String, T)>,
    cap: usize,
}

impl<T> Kept<T> {
    pub(crate) const fn new(cap: usize) -> Self {
        Self {
            held: Vec::new(),
            cap,
        }
    }

    /// What is kept for `key`, taken out.
    pub(super) fn take(&mut self, key: &str) -> Option<T> {
        let at = self.held.iter().position(|(held, _)| held == key)?;
        Some(self.held.remove(at).1)
    }

    pub(crate) fn get(&self, key: &str) -> Option<&T> {
        self.held
            .iter()
            .find(|(held, _)| held == key)
            .map(|(_, value)| value)
    }

    /// Oldest first.
    pub(super) fn iter(&self) -> impl DoubleEndedIterator<Item = &(String, T)> {
        self.held.iter()
    }

    /// Remember `value` for `key`, replacing that key's older one and
    /// forgetting the oldest key once the table is full.
    pub(crate) fn keep(&mut self, key: String, value: T) {
        self.held.retain(|(held, _)| held != &key);
        self.held.push((key, value));
        while self.held.len() > self.cap {
            self.held.remove(0);
        }
    }
}

static LAST: Mutex<Kept<LastFrame>> = Mutex::new(Kept::new(OBSERVE_FRAMES_MAX));

/// Which place a look is of: an app's window (by id, index or the front one)
/// or a desktop display and region — the key its last frame is kept under.
fn frame_key(params: &Map<String, Value>) -> String {
    let text = |key: &str| {
        params
            .get(key)
            .map(Value::to_string)
            .map(|raw| raw.trim_matches('"').to_string())
    };
    let place = if let Some(app) = text("app") {
        let window = if let Some(id) = text("windowId") {
            format!("id:{id}")
        } else if let Some(index) = text("windowIndex") {
            format!("index:{index}")
        } else {
            "front".to_string()
        };
        format!("app:{app}/window:{window}")
    } else {
        let display = text("display").unwrap_or_else(|| "main".to_string());
        let region = text("region").unwrap_or_else(|| "full".to_string());
        format!("desktop:{display}/region:{region}")
    };
    match text("viewer") {
        Some(viewer) => format!("viewer:{viewer}/{place}"),
        None => place,
    }
}

fn take_last(key: &str) -> Option<LastFrame> {
    LAST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take(key)
}

fn keep(key: String, png: Vec<u8>, frame: ShotFrame) {
    LAST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keep(key, LastFrame { png, frame });
}

/// Where a frame sits on the screen and how many pixels make a point: an
/// app's picture sits at its window, a desktop look says its own origin —
/// the one `ShotFrame` reading of an answer.
fn placement(answer: &Value) -> ShotFrame {
    ShotFrame::from_answer(answer).unwrap_or(ShotFrame::UNIT)
}

/// What changed between the last frame and this one, in screen points,
/// with the share of pixels that differ. `None` when there was no last frame.
pub(crate) fn changes(
    previous: Option<&[u8]>,
    current: &[u8],
    frame: ShotFrame,
) -> Option<(Vec<Value>, f64)> {
    changes_against(previous, &decode_png(current)?, frame)
}

/// `changes`, with this frame decoded once already.
fn changes_against(
    previous: Option<&[u8]>,
    after: &RgbaImage,
    frame: ShotFrame,
) -> Option<(Vec<Value>, f64)> {
    let before = decode_png(previous?)?;
    let report = diff(&before, after, OBSERVE_DIFF_PIXEL_DELTA);
    let total = (report.width as usize * report.height as usize).max(1);
    #[allow(clippy::cast_precision_loss)]
    let share = report.different as f64 / total as f64;
    let regions = changed_regions(&report, OBSERVE_DIFF_CELL, OBSERVE_DIFF_RECTS)
        .into_iter()
        .map(|region| {
            let [x, y, width, height] = frame.rect_to_point([
                f64::from(region.x),
                f64::from(region.y),
                f64::from(region.width),
                f64::from(region.height),
            ]);
            serde_json::json!({ "x": x, "y": y, "width": width, "height": height })
        })
        .collect();
    Some((regions, share))
}

/// A desktop `screenshot` is a look too: its frame becomes the viewer's last
/// one at its place, so a later `--diff` says what changed since the caller
/// last saw the screen, whichever verb showed it. A full-resolution shot is
/// not the picture `observe` takes there, so it is not the one to diff.
pub fn remember(params: &Map<String, Value>, answer: &Value, png: Vec<u8>) {
    if params.contains_key("app") || params.get("fullRes").and_then(Value::as_bool) == Some(true) {
        return;
    }
    keep(frame_key(params), png, placement(answer));
}

/// The most a look's line in the session's `looks.jsonl` may weigh: its
/// numbers come to about 250 bytes (t-15517); a line past this is not
/// written.
pub(crate) const LOOK_LINE_MAX_BYTES: usize = 512;
/// The longest word a look's line keeps — which road the look took; every
/// other field is a number.
pub(crate) const LOOK_LINE_WORD_MAX: usize = 16;

/// The lines the looks on this thread left, taken out.
#[cfg(test)]
fn drain_looks() -> Vec<String> {
    Vec::new()
}

/// One look, through the helper.
pub fn observe(params: &Map<String, Value>) -> Result<Value, ComputerUseError> {
    observe_with(
        params,
        &super::eye::MEMORY,
        &mut super::call,
        &mut std::thread::sleep,
    )
}

/// One look, through `call` (the helper, or a test's script). With
/// `settle`, first wait for what the last act did to finish painting — on
/// the eye's repaints, or, without the eye, by looking until the table's
/// settle the way a look after an act always did.
pub(super) fn observe_with(
    params: &Map<String, Value>,
    memory: &super::eye::Memory,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
    pause: &mut dyn FnMut(std::time::Duration),
) -> Result<Value, ComputerUseError> {
    if params.get("settle").and_then(Value::as_bool) != Some(true) {
        return look(params, memory, call);
    }
    let display = params.get("display").and_then(Value::as_u64);
    if let Some(settled) = super::eye::settle(memory, display, call, pause) {
        let mut answer = look(params, memory, call)?;
        answer["settle"] = settled.answer();
        return Ok(answer);
    }
    let asked_diff = params.get("diff").and_then(Value::as_bool) == Some(true);
    let mut diffed = params.clone();
    diffed.insert("diff".into(), Value::Bool(true));
    let started = std::time::Instant::now();
    let mut looks = 0_u32;
    let mut answer = zerocode_core::computer_use::look_until_settled(
        || {
            looks += 1;
            look(&diffed, memory, call)
        },
        |seen| {
            seen.get("changed")
                .and_then(Value::as_array)
                .map(|regions| !regions.is_empty())
        },
        || started.elapsed(),
        pause,
    )?;
    if !asked_diff {
        answer["changed"] = Value::Null;
        answer["changedShare"] = Value::Null;
    }
    answer["settle"] = serde_json::json!({
        // The fallback waits for a visible response, not a proven quiet
        // interval. Lost stream history must not become a success claim.
        "settled": false,
        "waitedMs": u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        "looks": looks,
    });
    Ok(answer)
}

/// One look: the app's window through its tree, or the desktop — the eye's
/// newest frame when the eye is open, a capture otherwise: always for a
/// region, which is looked at closer than the eye sees, and for a marked
/// look, whose badges must sit on pixels taken after the windows were
/// listed (the eye's newest frame can be older than that list).
fn look(
    params: &Map<String, Value>,
    memory: &super::eye::Memory,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
) -> Result<Value, ComputerUseError> {
    let app = params.contains_key("app");
    let marks = params.get("marks").and_then(Value::as_bool) == Some(true);
    let key = frame_key(params);
    let mut ask = Map::new();
    for key in ["app", "windowId", "windowIndex", "display", "region"] {
        if let Some(value) = params.get(key) {
            ask.insert(key.to_string(), value.clone());
        }
    }
    // A marked desktop look lists the windows before its picture: the window
    // it marks is chosen from what the picture shows, and must stand still.
    let before = (marks && !app).then(|| super::marks::desktop_windows(call));
    let frame = if app {
        let mut look = ask.clone();
        if marks {
            look.insert(ELEMENT_FRAMES_KEY.into(), Value::Bool(true));
        }
        call("getAppState", Value::Object(look))?
    } else if let Some(seen) = (!ask.contains_key("region") && !marks)
        .then(|| super::eye::frame(memory, ask.get("display").and_then(Value::as_u64), call))
        .flatten()
    {
        seen
    } else {
        call("screenshotDesktop", Value::Object(ask.clone()))?
    };
    let png = super::screenshot_png(&frame).ok_or_else(|| {
        ComputerUseError::new(
            error_code::SCREENSHOT_FAILED,
            "the helper answered without a frame",
        )
    })?;
    // Decoded once, for the diff and for the marks alike.
    let clean = decode_png(&png);
    let placed = placement(&frame);
    let text = if params.get("ocr").and_then(Value::as_bool) == Some(true) {
        let mut read = ask.clone();
        read.insert("ocr".into(), Value::Bool(true));
        let read = super::eye::reading_with(memory, &Value::Object(read), call);
        Some(call("readText", read)?)
    } else {
        None
    };
    let (changed, share) = if params.get("diff").and_then(Value::as_bool) == Some(true) {
        // A diff is only honest against a look at the same place: a frame of
        // another window or another scale is not "what changed".
        let previous = take_last(&key).filter(|held| held.frame == placed);
        match clean.as_ref().and_then(|after| {
            changes_against(
                previous.as_ref().map(|held| held.png.as_slice()),
                after,
                placed,
            )
        }) {
            Some((regions, share)) => (Value::Array(regions), Some(share)),
            None => (Value::Null, None),
        }
    } else {
        (Value::Null, None)
    };
    let fingerprint = marks.then(|| super::marks::fingerprint(&png));
    // The diff's baseline is always the clean frame, never the marked one.
    keep(key.clone(), png, placed);
    let mut answer = serde_json::json!({
        "screenshot": frame.get("screenshot").cloned().unwrap_or(Value::Null),
        "origin": { "x": placed.origin().0, "y": placed.origin().1 },
        "scale": placed.scale(),
        "tree": frame.get("snapshot").map(|snapshot| serde_json::json!({
            "id": snapshot.get("id").cloned().unwrap_or(Value::Null),
            "window": snapshot.get("window").cloned().unwrap_or(Value::Null),
            "text": snapshot.get("treeText").cloned().unwrap_or(Value::Null),
            "elementCount": snapshot.get("elementCount").cloned().unwrap_or(Value::Null),
        })).unwrap_or(Value::Null),
        "text": text.as_ref().and_then(|read| read.get("lines").cloned()).unwrap_or(Value::Null),
        "changed": changed,
        "changedShare": share,
    });
    if let Some(share) = share {
        let stuck = super::state::note_look(
            share > 0.0,
            super::evidence::session_dir(crate::now_epoch_ms()).as_deref(),
        );
        answer["stuck"] = Value::Bool(stuck);
    }
    if marks {
        answer["marks"] = match (&clean, fingerprint) {
            (Some(clean), Some(fingerprint)) => {
                let picture = super::marks::Picture {
                    clean,
                    fingerprint,
                    placed,
                    place: &key,
                };
                let marked = super::marks::mark_look(params, &frame, &picture, before, call);
                if let Some(png) = &marked.png {
                    super::marks::put_picture(&mut answer, png);
                }
                marked.answer
            }
            _ => {
                serde_json::json!({ "unavailable": "screenshot_failed: the frame could not be read" })
            }
        };
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer_use::screenshot_png::RgbaImage;

    #[test]
    fn a_fallback_picture_does_not_claim_quiet_after_stream_history_was_lost() {
        use base64::Engine as _;
        let shot = png(4, 4, |_, _| [9, 9, 9, 255]);
        let frame = serde_json::json!({
            "screenshot": { "data": base64::engine::general_purpose::STANDARD.encode(&shot), "width": 4, "height": 4, "scale": 1.0 },
            "origin": { "x": 0, "y": 0 }
        });
        let mut polls = 0;
        let mut call = |method: &str, _: Value| match method {
            "eyeChanges" => {
                polls += 1;
                Ok(
                    serde_json::json!({ "streaming": true, "streamId": "eye", "seq": polls,
                    "nowMs": 1_000 + polls * 100, "whole": polls == 1, "changes": [] }),
                )
            }
            "listAllWindows" => Ok(serde_json::json!({ "windows": [] })),
            "eyeFrame" => Ok(frame.clone()),
            other => panic!("unexpected {other}"),
        };
        let params = serde_json::json!({ "settle": true, "viewer": "lost-history-fallback" });
        let answer = observe_with(
            params.as_object().unwrap(),
            &super::super::eye::Memory::new(),
            &mut call,
            &mut |_| panic!("the first fallback frame has no previous comparison"),
        )
        .unwrap();
        assert_eq!(answer["settle"]["settled"], false);
        assert_eq!(answer["screenshot"]["width"], 4);
        assert_eq!(answer["settle"]["looks"], 1);
    }

    /// A marked desktop look lists the windows, then takes its picture: the
    /// eye's newest frame may predate the list, so the badges would sit on
    /// another moment's pixels. It captures; a plain look reads the eye.
    #[test]
    fn a_marked_desktop_look_captures_and_a_plain_one_reads_the_eye() {
        use base64::Engine as _;
        let shot = png(4, 4, |_, _| [9, 9, 9, 255]);
        let answer = serde_json::json!({
            "screenshot": { "data": base64::engine::general_purpose::STANDARD.encode(&shot), "width": 4, "height": 4, "scale": 1.0 },
            "origin": { "x": 0, "y": 0 },
        });
        for (marks, looked) in [(true, "screenshotDesktop"), (false, "eyeFrame")] {
            let mut asked = Vec::new();
            let mut call = |method: &str, _: Value| -> Result<Value, ComputerUseError> {
                asked.push(method.to_string());
                match method {
                    "screenshotDesktop" | "eyeFrame" => Ok(answer.clone()),
                    "listAllWindows" => Ok(serde_json::json!({ "windows": [] })),
                    other => panic!("unexpected {other}"),
                }
            };
            let mut params = Map::new();
            params.insert("viewer".into(), format!("marked-{marks}").into());
            params.insert("marks".into(), marks.into());
            let memory = super::super::eye::Memory::new();
            observe_with(&params, &memory, &mut call, &mut |_| {
                panic!("no settle asked")
            })
            .unwrap();
            assert!(
                asked.contains(&looked.to_string()),
                "marks {marks}: {asked:?}"
            );
            assert!(
                !asked.contains(
                    &if marks {
                        "eyeFrame"
                    } else {
                        "screenshotDesktop"
                    }
                    .to_string()
                ),
                "{asked:?}"
            );
        }
    }

    fn png(width: u32, height: u32, paint: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut pixels = Vec::new();
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&paint(x, y));
            }
        }
        RgbaImage::new(width, height, pixels)
            .unwrap()
            .encode()
            .unwrap()
    }

    #[test]
    fn a_look_is_keyed_by_its_app_and_window_or_by_its_desktop_place() {
        let key = |json: &str| {
            frame_key(
                serde_json::from_str::<Value>(json)
                    .unwrap()
                    .as_object()
                    .unwrap(),
            )
        };
        assert_eq!(key(r#"{"app":"Safari"}"#), "app:Safari/window:front");
        assert_eq!(
            key(r#"{"app":"Safari","windowId":42}"#),
            "app:Safari/window:id:42"
        );
        assert_eq!(
            key(r#"{"app":"Safari","windowIndex":1}"#),
            "app:Safari/window:index:1"
        );
        assert_ne!(key(r#"{"app":"Safari"}"#), key(r#"{"app":"Finder"}"#));
        assert_eq!(key("{}"), "desktop:main/region:full");
        assert_eq!(
            key(r#"{"display":1,"region":"0,0,100,100"}"#),
            "desktop:1/region:0,0,100,100"
        );
        assert_ne!(key("{}"), key(r#"{"app":"Safari"}"#));
        assert_eq!(
            key(r#"{"viewer":"zo-1"}"#),
            "viewer:zo-1/desktop:main/region:full",
            "a named viewer keeps its own last look"
        );
        assert_ne!(key(r#"{"viewer":"zo-1"}"#), key(r#"{"viewer":"zo-2"}"#));
    }

    #[test]
    fn frames_are_kept_per_key_and_the_oldest_key_leaves_first() {
        let mut frames = Kept::new(OBSERVE_FRAMES_MAX);
        let held = |tag: u8| LastFrame {
            png: vec![tag],
            frame: ShotFrame::new((0.0, 0.0), 1.0).unwrap(),
        };
        frames.keep("a".into(), held(1));
        frames.keep("b".into(), held(2));
        assert_eq!(frames.take("a").map(|f| f.png), Some(vec![1]));
        assert!(frames.take("a").is_none(), "taking consumes the frame");
        assert_eq!(frames.take("b").map(|f| f.png), Some(vec![2]));
        for tag in 0..=OBSERVE_FRAMES_MAX {
            frames.keep(format!("k{tag}"), held(u8::try_from(tag).unwrap()));
        }
        assert!(
            frames.take("k0").is_none(),
            "the oldest key leaves past the table's cap"
        );
        assert!(frames.take(&format!("k{OBSERVE_FRAMES_MAX}")).is_some());
        frames.keep("same".into(), held(7));
        frames.keep("same".into(), held(8));
        assert_eq!(
            frames.take("same").map(|f| f.png),
            Some(vec![8]),
            "a key keeps only its latest frame"
        );
    }

    #[test]
    fn what_changed_is_placed_on_the_screen_by_origin_and_scale() {
        let before = png(48, 48, |_, _| [0, 0, 0, 255]);
        let after = png(48, 48, |x, y| {
            if x >= 24 && y >= 24 {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 255]
            }
        });
        let frame = |origin, scale| ShotFrame::new(origin, scale).unwrap();
        let (regions, share) =
            changes(Some(&before), &after, frame((100.0, 200.0), 2.0)).expect("a diff");
        assert!((share - 0.25).abs() < 1e-9, "{share}");
        assert_eq!(regions.len(), 1);
        assert_eq!(
            regions[0]["x"], 112.0,
            "24 px at scale 2 is 12 points past the origin"
        );
        assert_eq!(regions[0]["y"], 212.0);
        assert_eq!(regions[0]["width"], 12.0);
        assert!(
            changes(None, &after, frame((0.0, 0.0), 1.0)).is_none(),
            "no last frame, no diff"
        );
        assert!(changes(Some(b"not png"), &after, frame((0.0, 0.0), 1.0)).is_none());
    }

    // t-15517: a look that answers "nothing changed" when the act did change
    // the screen. Synthetic frames only — `Drawn` is a display these tests
    // paint, `desk` a helper that answers from it, `act` what the window
    // notes when an act is answered.

    /// Two pixels a point: a Retina display at its own resolution.
    const FULL_SCALE: f64 = 2.0;
    /// The built-in display's width in points: its look is these points
    /// brought to the ladder's first rung, about 0.85 pixels a point.
    const BUILT_IN_POINTS_WIDE: f64 = 1512.0;

    /// A display drawn at its own resolution before and after an act, and
    /// the picture a desktop look answers at its size.
    struct Drawn {
        full: [Vec<[u8; 4]>; 2],
        full_size: (u32, u32),
        look: [Vec<u8>; 2],
        look_size: (u32, u32),
        look_scale: f64,
        /// Where the act repainted, in points: the eye's repaint rect.
        repainted: [f64; 4],
    }

    fn drawn(
        points: (u32, u32),
        paint: impl Fn(bool, u32, u32) -> [u8; 4],
        repainted: [f64; 4],
    ) -> Drawn {
        let look_scale =
            zerocode_core::computer_use_protocol::SCREENSHOT_RESIZE_START_PX / BUILT_IN_POINTS_WIDE;
        let sized = |scale: f64| {
            (
                (f64::from(points.0) * scale).round() as u32,
                (f64::from(points.1) * scale).round() as u32,
            )
        };
        let (full_size, look_size) = (sized(FULL_SCALE), sized(look_scale));
        let full = [false, true].map(|after| {
            (0..full_size.1)
                .flat_map(|y| (0..full_size.0).map(move |x| (x, y)))
                .map(|(x, y)| paint(after, x, y))
                .collect::<Vec<_>>()
        });
        let look =
            [0_usize, 1].map(|at| encoded(&shrink(&full[at], full_size, look_size), look_size));
        Drawn {
            full,
            full_size,
            look,
            look_size,
            look_scale,
            repainted,
        }
    }

    fn encoded(pixels: &[[u8; 4]], (width, height): (u32, u32)) -> Vec<u8> {
        RgbaImage::new(width, height, pixels.iter().flatten().copied().collect())
            .unwrap()
            .encode()
            .unwrap()
    }

    /// `pixels` brought down to `to` by averaging what each output pixel
    /// covers: how a downscale spreads a thin change over its neighbours.
    fn shrink(pixels: &[[u8; 4]], from: (u32, u32), to: (u32, u32)) -> Vec<[u8; 4]> {
        let (sx, sy) = (
            f64::from(from.0) / f64::from(to.0),
            f64::from(from.1) / f64::from(to.1),
        );
        let mut out = Vec::with_capacity(to.0 as usize * to.1 as usize);
        for oy in 0..to.1 {
            let (y0, y1) = (f64::from(oy) * sy, f64::from(oy + 1) * sy);
            for ox in 0..to.0 {
                let (x0, x1) = (f64::from(ox) * sx, f64::from(ox + 1) * sx);
                let (mut sum, mut area) = ([0.0_f64; 4], 0.0);
                for y in (y0.floor() as u32)..(y1.ceil() as u32).min(from.1) {
                    let dy = f64::from(y + 1).min(y1) - f64::from(y).max(y0);
                    for x in (x0.floor() as u32)..(x1.ceil() as u32).min(from.0) {
                        let dx = f64::from(x + 1).min(x1) - f64::from(x).max(x0);
                        let pixel = pixels[(y * from.0 + x) as usize];
                        for (total, channel) in sum.iter_mut().zip(pixel) {
                            *total += f64::from(channel) * dx * dy;
                        }
                        area += dx * dy;
                    }
                }
                out.push(sum.map(|total| (total / area).round() as u8));
            }
        }
        out
    }

    /// The part of a drawn display a region asks for, at its own resolution.
    fn cut(
        pixels: &[[u8; 4]],
        size: (u32, u32),
        [x, y, width, height]: [f64; 4],
    ) -> (Vec<[u8; 4]>, (u32, u32)) {
        let at = |points: f64, most: u32| ((points * FULL_SCALE).round() as u32).min(most);
        let (left, top) = (at(x, size.0), at(y, size.1));
        let (right, bottom) = (at(x + width, size.0), at(y + height, size.1));
        let kept = (top..bottom)
            .flat_map(|row| {
                (left..right).map(move |column| pixels[(row * size.0 + column) as usize])
            })
            .collect();
        (kept, (right - left, bottom - top))
    }

    /// A picture as the helper answers one: at `origin`, `scale` pixels a point.
    fn answered(png: &[u8], (width, height): (u32, u32), origin: (f64, f64), scale: f64) -> Value {
        use base64::Engine as _;
        serde_json::json!({
            "screenshot": {
                "data": base64::engine::general_purpose::STANDARD.encode(png),
                "width": width,
                "height": height,
                "scale": scale,
            },
            "origin": { "x": origin.0, "y": origin.1 },
        })
    }

    /// A helper over `drawn`: the eye's newest frame (`eyeFrame` — still the
    /// one before the act while `eye_behind`, its repaint not delivered yet),
    /// a capture at the look's size (`screenshotDesktop`) or at the display's
    /// own resolution (`fullRes`, or a region), and the eye's account
    /// (`eyeChanges`: the act's mark, and where the act repainted once the
    /// eye has it).
    fn desk<'d>(
        drawn: &'d Drawn,
        acted: &'d std::cell::Cell<bool>,
        eye_behind: bool,
    ) -> impl FnMut(&str, Value) -> Result<Value, ComputerUseError> + 'd {
        move |method: &str, params: Value| {
            let now = usize::from(acted.get());
            let seen = if eye_behind { 0 } else { now };
            match method {
                "eyeFrame" => {
                    let mut frame = answered(
                        &drawn.look[seen],
                        drawn.look_size,
                        (0.0, 0.0),
                        drawn.look_scale,
                    );
                    frame["seq"] = (seen + 1).into();
                    Ok(frame)
                }
                "screenshotDesktop" => {
                    let region = params.get("region").and_then(|region| {
                        Some([
                            region.get("x")?.as_f64()?,
                            region.get("y")?.as_f64()?,
                            region.get("width")?.as_f64()?,
                            region.get("height")?.as_f64()?,
                        ])
                    });
                    Ok(if let Some(asked) = region {
                        let (piece, size) = cut(&drawn.full[now], drawn.full_size, asked);
                        answered(
                            &encoded(&piece, size),
                            size,
                            (asked[0], asked[1]),
                            FULL_SCALE,
                        )
                    } else if params.get("fullRes").and_then(Value::as_bool) == Some(true) {
                        let png = encoded(&drawn.full[now], drawn.full_size);
                        answered(&png, drawn.full_size, (0.0, 0.0), FULL_SCALE)
                    } else {
                        answered(
                            &drawn.look[now],
                            drawn.look_size,
                            (0.0, 0.0),
                            drawn.look_scale,
                        )
                    })
                }
                "eyeChanges" => {
                    let act = acted
                        .get()
                        .then(|| serde_json::json!({ "seq": 1, "atMs": 1_000 }));
                    let changes = if seen == 1 {
                        serde_json::json!([{ "seq": 2, "atMs": 1_050, "rects": [drawn.repainted] }])
                    } else {
                        serde_json::json!([])
                    };
                    Ok(serde_json::json!({
                        "streaming": true,
                        "streamId": "drawn",
                        "whole": true,
                        "seq": seen + 1,
                        "nowMs": 2_000,
                        "act": act,
                        "changes": changes,
                    }))
                }
                "listAllWindows" => Ok(serde_json::json!({ "windows": [] })),
                other => Err(ComputerUseError::new(
                    error_code::UNSUPPORTED_CAPABILITY,
                    format!("the drawn desk does not answer {other}"),
                )),
            }
        }
    }

    /// An act answered, noted where the window notes one, and the drawn
    /// screen turned to its after.
    fn act(acted: &std::cell::Cell<bool>) {
        let now = crate::now_epoch_ms();
        super::super::guard::note_action("mouse-click", now);
        super::super::state::note_action(
            "mouse-click",
            &["mouse-click".to_string()],
            true,
            None,
            now,
            None,
        );
        acted.set(true);
    }

    fn diff_look(viewer: &str) -> Map<String, Value> {
        let mut params = Map::new();
        params.insert("viewer".into(), viewer.into());
        params.insert("diff".into(), true.into());
        params
    }

    /// A 12 x 12-point square at (20, 12) that turns from dark to white.
    fn square(after: bool, x: u32, y: u32) -> [u8; 4] {
        if after && (40..64).contains(&x) && (24..48).contains(&y) {
            [255, 255, 255, 255]
        } else {
            [40, 40, 40, 255]
        }
    }

    fn says_changed(answer: &Value) -> bool {
        answer["changed"]
            .as_array()
            .is_some_and(|changed| !changed.is_empty())
    }

    /// A: the eye's newest frame is still the one before the act — its
    /// repaint not delivered yet — and a desktop look answers from it, so the
    /// look after the act compares two frames from before it.
    #[test]
    fn a_look_after_an_act_does_not_answer_from_a_frame_taken_before_it() {
        let drawn = drawn((64, 40), square, [20.0, 12.0, 12.0, 12.0]);
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, true);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-a");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert!(
            says_changed(&answer),
            "the look after the act answered from the eye's frame before it: changed {}",
            answer["changed"]
        );
    }

    /// B: a 12 x 12-point box whose one-pixel border darkens by 11 — past
    /// the diff's drift at the display's own resolution, under it once the
    /// look averages about 2.4 display pixels into one of its own.
    #[test]
    fn a_small_change_a_retina_display_shows_is_not_nothing_at_the_looks_size() {
        let border = |after: bool, x: u32, y: u32| {
            let on_box = (40..64).contains(&x) && (24..48).contains(&y);
            let edge = x == 40 || x == 63 || y == 24 || y == 47;
            if after && on_box && edge {
                [189, 189, 189, 255]
            } else {
                [200, 200, 200, 255]
            }
        };
        let drawn = drawn((64, 40), border, [20.0, 12.0, 12.0, 12.0]);
        let full = |at: usize| encoded(&drawn.full[at], drawn.full_size);
        let seen = changes(
            Some(&full(0)),
            &full(1),
            ShotFrame::new((0.0, 0.0), FULL_SCALE).unwrap(),
        );
        assert!(
            seen.is_some_and(|(regions, _)| !regions.is_empty()),
            "the drawn change is past the drift at the display's own resolution"
        );
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, false);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-b");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        let changed = answer["changed"].as_array().cloned().unwrap_or_default();
        assert!(
            changed.iter().any(|rect| {
                let at = |key: &str| rect[key].as_f64().unwrap_or(f64::NAN);
                (at("x")..=at("x") + at("width")).contains(&26.0)
                    && (at("y")..=at("y") + at("height")).contains(&18.0)
            }),
            "a 12 x 12-point change at {:.2} px a point was answered as {}",
            drawn.look_scale,
            answer["changed"]
        );
    }

    /// C: a desktop screenshot between the act and the diff becomes the
    /// viewer's last frame (`remember`), so the diff compares after with
    /// after. The act's own baseline is the look before it.
    #[test]
    fn a_screenshot_between_the_act_and_the_diff_does_not_hide_what_the_act_changed() {
        let drawn = drawn((64, 40), square, [20.0, 12.0, 12.0, 12.0]);
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, false);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-c");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        // The agent's own desktop screenshot of the same place, after the act.
        let mut shot = Map::new();
        shot.insert("viewer".into(), "t-15517-c".into());
        let screenshot = call("eyeFrame", Value::Object(Map::new())).unwrap();
        remember(&shot, &screenshot, drawn.look[1].clone());
        let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert!(
            says_changed(&answer),
            "a screenshot after the act made the diff compare after with after: changed {}",
            answer["changed"]
        );
    }

    /// D: a look that cannot be compared — another scale, another window —
    /// answers `changed: null`, unknown; it is not counted as a look that saw
    /// nothing change.
    #[test]
    fn a_look_that_cannot_be_compared_answers_unknown_not_nothing() {
        let pictures = [
            (png(8, 8, |_, _| [9, 9, 9, 255]), (8, 8), 1.0),
            (png(16, 16, |_, _| [9, 9, 9, 255]), (16, 16), 2.0),
        ];
        let mut looks = 0;
        let mut call = |method: &str, _: Value| -> Result<Value, ComputerUseError> {
            match method {
                "eyeFrame" => {
                    let (png, size, scale) = &pictures[looks.min(1)];
                    looks += 1;
                    Ok(answered(png, *size, (0.0, 0.0), *scale))
                }
                other => Err(ComputerUseError::new(
                    error_code::UNSUPPORTED_CAPABILITY,
                    format!("not asked here: {other}"),
                )),
            }
        };
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-d");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert!(answer["changed"].is_null(), "{}", answer["changed"]);
        assert!(answer["changedShare"].is_null());
        assert!(
            answer.get("stuck").is_none(),
            "an unknown look is not counted as one that saw nothing change"
        );
    }

    /// The measurement: every look leaves one line of numbers — how many
    /// rectangles changed and what share, the frame's and the last act's
    /// stamps, the picture's size and scale — and nothing it saw: no
    /// picture, no text, no window's name.
    #[test]
    fn a_look_leaves_its_numbers_and_nothing_it_saw() {
        let drawn = drawn((64, 40), square, [20.0, 12.0, 12.0, 12.0]);
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, false);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-m");
        drain_looks();
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        let lines = drain_looks();
        assert_eq!(lines.len(), 2, "a look leaves one line: {lines:?}");
        let read = |line: &String| serde_json::from_str::<Map<String, Value>>(line).unwrap();
        let (first, after) = (read(&lines[0]), read(&lines[1]));
        for (line, fields) in lines.iter().zip([&first, &after]) {
            assert!(line.len() <= LOOK_LINE_MAX_BYTES, "{} bytes", line.len());
            for (key, value) in fields {
                assert!(
                    ![
                        "screenshot",
                        "data",
                        "text",
                        "tree",
                        "title",
                        "app",
                        "window"
                    ]
                    .contains(&key.as_str()),
                    "a look's line keeps no {key}"
                );
                assert!(
                    value
                        .as_str()
                        .is_none_or(|word| word.len() <= LOOK_LINE_WORD_MAX),
                    "a look's line keeps numbers and short words only: {key} = {value}"
                );
            }
        }
        assert!(first["changed"].is_null(), "no earlier look: unknown");
        assert!(
            after["n"].as_u64() > first["n"].as_u64(),
            "looks are numbered"
        );
        assert!(after["changed"].as_u64().is_some_and(|rects| rects > 0));
        assert!(
            after["changedShare"]
                .as_f64()
                .is_some_and(|share| share > 0.0)
        );
        assert_eq!(after["width"], drawn.look_size.0);
        assert_eq!(after["height"], drawn.look_size.1);
        assert_eq!(after["scale"], drawn.look_scale);
        assert_eq!(after["frameSeq"], 2, "the eye's frame after the act");
        assert_eq!(after["actSeq"], 1, "the helper's mark of the act");
        assert!(
            after["lastActEpochMs"].as_i64().is_some_and(|at| at > 0),
            "the last act's time, to join the step log"
        );
    }
}
