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
}
