//! The eyes (docs/design/computer-use-full-operator.md §7.1): one look that
//! merges what the helper knows — the accessibility tree when an app is
//! named, the frame, the text the pixels carry with `--ocr` — and, with
//! `--diff`, what changed since the last look, as rectangles in screen
//! points. The last frames are kept here, one per viewer and place looked at
//! (an app's window, or a desktop display and region), a bounded table of
//! them, so a look at another app never counts as "what changed" in this one
//! — and an agent that names itself (`--viewer`) is never told "nothing
//! changed" against a frame another agent looked at.
//!
//! The look after an act is held to what the act did (t-15517): its picture
//! is taken after the act, its diff counts from the last look before the act
//! whatever was looked at in between, and a change the look's shrunk picture
//! averages under the drift is found where the eye saw the display repaint.
//! Every look leaves its numbers — never its picture or text — in the
//! session's `looks.jsonl`, so a wrong "nothing changed" can be counted.

use std::cell::RefCell;
use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Map, Value};
use zerocode_core::computer_use::{
    OBSERVE_DIFF_CELL, OBSERVE_DIFF_PIXEL_DELTA, OBSERVE_DIFF_RECTS, OBSERVE_FRAMES_MAX,
};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::eye::{Changes, SEQ_KEY, STREAM_ID_KEY};
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::ELEMENT_FRAMES_KEY;
use zerocode_core::computer_use_protocol::render::Rect;

use super::ComputerUseError;
use super::compare::{Diff, changed_regions, decode_png, diff};
use super::screenshot_png::RgbaImage;

/// The file beside the session's steps that holds one line of numbers per
/// look.
pub const LOOKS_FILE: &str = "looks.jsonl";
/// The most a look's line in the session's `looks.jsonl` may weigh: its
/// numbers come to about 250 bytes (t-15517); a line past this is not
/// written.
pub(crate) const LOOK_LINE_MAX_BYTES: usize = 512;
/// The longest word a look's line keeps — which road the look took; every
/// other field is a number.
pub(crate) const LOOK_LINE_WORD_MAX: usize = 16;
/// How far a pixel of the look's picture must move, inside a place the eye
/// saw the display repaint after the act, to count: at all. The eye notes a
/// repaint only when its frame — the look's own size — is not the same to
/// the last bit, and places it where the window server says it drew, so
/// there a moved pixel is the change averaged down, not drift: a
/// 12 × 12-point box whose one-pixel border darkens by 11 moves the look's
/// pixels by at most 6 at 0.85 pixels a point (the built-in display's
/// look), under `OBSERVE_DIFF_PIXEL_DELTA` (t-15517).
pub(crate) const OBSERVE_REPAINTED_PIXEL_DELTA: u8 = 0;

/// The previous look's frame: its PNG and where it sat on the screen, so a
/// change can be placed there; how many acts the window had answered when it
/// was taken; and the last look before an act no diff has answered yet —
/// what the diff after that act counts from, whatever was looked at between.
struct LastFrame {
    png: Vec<u8>,
    frame: ShotFrame,
    acts: u64,
    before_act: Option<Box<LastFrame>>,
}

impl LastFrame {
    /// What a diff of this place counts from: the last look before an act no
    /// diff has answered, else the last look.
    fn baseline(&self) -> &Self {
        self.before_act.as_deref().unwrap_or(self)
    }

    /// The look before the act to keep beside a newer look that answers no
    /// diff: the one already kept, or this look when an act came after it.
    fn before(self, acts: u64) -> Option<Box<Self>> {
        match self.before_act {
            Some(before) => Some(before),
            None if self.acts < acts => Some(Box::new(self)),
            None => None,
        }
    }
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

/// How many acts the window had answered when `key`'s last look was taken.
fn held_acts(key: &str) -> Option<u64> {
    LAST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .map(|held| held.acts)
}

fn keep(key: String, frame: LastFrame) {
    LAST.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .keep(key, frame);
}

/// How many acts the window has answered, and when the last one was — what
/// "an act since this look" is judged by (`guard::note_action`).
fn last_act() -> (u64, i64) {
    let report = super::guard::activity_report(None);
    (
        report["actions"].as_u64().unwrap_or(0),
        report["at"].as_i64().unwrap_or(0),
    )
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
    let after = decode_png(current)?;
    let before = decode_png(previous?)?;
    Some(answered_diff(
        &diff(&before, &after, OBSERVE_DIFF_PIXEL_DELTA),
        frame,
    ))
}

/// A diff as a look answers it: the changed areas in screen points, and the
/// share of pixels that differ.
fn answered_diff(report: &Diff, frame: ShotFrame) -> (Vec<Value>, f64) {
    let total = (report.width as usize * report.height as usize).max(1);
    #[allow(clippy::cast_precision_loss)]
    let share = report.different as f64 / total as f64;
    let regions = changed_regions(report, OBSERVE_DIFF_CELL, OBSERVE_DIFF_RECTS)
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
    (regions, share)
}

/// What changed where the eye saw the display repaint (`repainted`, screen
/// points): the look's pixels there that moved at all
/// (`OBSERVE_REPAINTED_PIXEL_DELTA`), answered like any diff — only those
/// pixels are compared. A repaint that came and went leaves the picture as
/// it was, and is not a change. `None` when the two pictures differ in size.
fn changes_where_repainted(
    before: &RgbaImage,
    after: &RgbaImage,
    frame: ShotFrame,
    repainted: &[Rect],
) -> Option<(Vec<Value>, f64)> {
    if (before.width, before.height) != (after.width, after.height) {
        return None;
    }
    let (width, height) = (after.width as usize, after.height as usize);
    let mut mask = vec![false; width * height];
    let mut different = 0;
    for rect in repainted {
        let [left, top] = frame.to_pixel([rect.x, rect.y]);
        let [right, bottom] = frame.to_pixel([rect.max_x(), rect.max_y()]);
        let within = |edge: f64, most: usize| (edge.max(0.0) as usize).min(most);
        let columns = within(left.floor(), width)..within(right.ceil(), width);
        for row in within(top.floor(), height)..within(bottom.ceil(), height) {
            let first = row * width + columns.start;
            for (at, moved) in (first..).zip(&mut mask[first..row * width + columns.end]) {
                let pixel = at * 4..at * 4 + 4;
                if !*moved
                    && before.pixels[pixel.clone()]
                        .iter()
                        .zip(&after.pixels[pixel])
                        .any(|(&was, &is)| was.abs_diff(is) > OBSERVE_REPAINTED_PIXEL_DELTA)
                {
                    *moved = true;
                    different += 1;
                }
            }
        }
    }
    let report = Diff {
        width: after.width,
        height: after.height,
        different,
        mask,
    };
    Some(answered_diff(&report, frame))
}

/// A desktop `screenshot` is a look too: its frame becomes the viewer's last
/// one at its place, so a later `--diff` says what changed since the caller
/// last saw the screen, whichever verb showed it — except that the first
/// diff after an act still counts from the last look before it. A
/// full-resolution shot is not the picture `observe` takes there, so it is
/// not the one to diff.
pub fn remember(params: &Map<String, Value>, answer: &Value, png: Vec<u8>) {
    if params.contains_key("app") || params.get("fullRes").and_then(Value::as_bool) == Some(true) {
        return;
    }
    let key = frame_key(params);
    let (acts, _) = last_act();
    let before_act = take_last(&key).and_then(|held| held.before(acts));
    keep(
        key,
        LastFrame {
            png,
            frame: placement(answer),
            acts,
            before_act,
        },
    );
}

thread_local! {
    /// The lines the looks on this thread left, until `observe` writes them.
    static LOOK_LINES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// How many looks this window has taken: a look's number in its line, so
/// two looks in one millisecond are two lines.
static LOOKS: AtomicU64 = AtomicU64::new(0);

/// The lines the looks on this thread left, taken out.
fn drain_looks() -> Vec<String> {
    LOOK_LINES.with(|lines| std::mem::take(&mut *lines.borrow_mut()))
}

/// Append the looks' lines to the session's `looks.jsonl` — once the window
/// has answered an act, so its session folder stands (the act's step made
/// it). Best effort: the lines are an observation, never a gate on the look.
fn write_looks(lines: &[String]) {
    if lines.is_empty() || last_act().0 == 0 {
        return;
    }
    let Some(dir) = super::evidence::session_dir(crate::now_epoch_ms()) else {
        return;
    };
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOOKS_FILE))
    else {
        return;
    };
    let _ = file.write_all(format!("{}\n", lines.join("\n")).as_bytes());
}

/// One look, through the helper.
pub fn observe(params: &Map<String, Value>) -> Result<Value, ComputerUseError> {
    let answer = observe_with(
        params,
        &super::eye::MEMORY,
        &mut super::call,
        &mut std::thread::sleep,
    );
    write_looks(&drain_looks());
    answer
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
        return look(params, memory, call, Some(pause));
    }
    let display = params.get("display").and_then(Value::as_u64);
    if let Some(settled) = super::eye::settle(memory, display, call, pause) {
        // The eye's settle waited for the act's repaint: its newest frame
        // is the screen after it, and the look does not wait again.
        let mut answer = look(params, memory, call, None)?;
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
            look(&diffed, memory, call, None)
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

/// Which road a look's picture came by — the word its line keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Road {
    /// An app's window, through its tree.
    App,
    /// The eye's newest frame.
    Eye,
    /// The eye's newest frame was from before the act: captured instead.
    EyeBehind,
    /// A capture: a region, a marked look, or no eye.
    Capture,
}

impl Road {
    const fn word(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Eye => "eye",
            Self::EyeBehind => "eye-behind",
            Self::Capture => "capture",
        }
    }
}

/// Whether the eye's frame `seen` shows the screen after the act the eye's
/// `standing` marks: a repaint of the same stream numbered after the act's
/// mark. With no act the eye saw, its frames all follow the last act.
fn after_the_act(seen: &Value, standing: &Changes) -> bool {
    let Some(act) = standing.act else {
        return true;
    };
    let same_stream = seen
        .get(STREAM_ID_KEY)
        .and_then(Value::as_str)
        .is_none_or(|stream| standing.stream_id.as_deref() == Some(stream));
    same_stream
        && seen
            .get(SEQ_KEY)
            .and_then(Value::as_u64)
            .is_some_and(|seq| seq > act.seq)
}

/// One look: the app's window through its tree, or the desktop — the eye's
/// newest frame when the eye is open, a capture otherwise: always for a
/// region, which is looked at closer than the eye sees, for a marked look,
/// whose badges must sit on pixels taken after the windows were listed (the
/// eye's newest frame can be older than that list), and for the look after
/// an act whose repaint the eye has not delivered in `EYE_QUIET_MS`. `wait`
/// paces that wait; a look after the eye's settle has waited already.
#[allow(clippy::too_many_lines)]
fn look(
    params: &Map<String, Value>,
    memory: &super::eye::Memory,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
    wait: Option<&mut dyn FnMut(std::time::Duration)>,
) -> Result<Value, ComputerUseError> {
    let began = std::time::Instant::now();
    let app = params.contains_key("app");
    let marks = params.get("marks").and_then(Value::as_bool) == Some(true);
    let diffing = params.get("diff").and_then(Value::as_bool) == Some(true);
    let key = frame_key(params);
    let mut ask = Map::new();
    for key in ["app", "windowId", "windowIndex", "display", "region"] {
        if let Some(value) = params.get(key) {
            ask.insert(key.to_string(), value.clone());
        }
    }
    let display = ask.get("display").and_then(Value::as_u64);
    let whole_display = !app && !ask.contains_key("region") && !marks;
    let (acts, act_at_ms) = last_act();
    // An act since this place's last look: the look after it.
    let after_act = held_acts(&key).is_some_and(|held| held < acts);
    // Where the eye stands on the act, asked once, when a look needs it.
    let mut standing: Option<Changes> = None;
    // A marked desktop look lists the windows before its picture: the window
    // it marks is chosen from what the picture shows, and must stand still.
    let before = (marks && !app).then(|| super::marks::desktop_windows(call));
    let (frame, road) = if app {
        let mut look = ask.clone();
        if marks {
            look.insert(ELEMENT_FRAMES_KEY.into(), Value::Bool(true));
        }
        (call("getAppState", Value::Object(look))?, Road::App)
    } else if let Some(seen) = whole_display
        .then(|| super::eye::frame(memory, display, call))
        .flatten()
    {
        if after_act && wait.is_some() {
            standing = super::eye::standing(memory, display, call);
        }
        match (standing.as_ref(), wait) {
            (Some(standing), Some(wait)) if !after_the_act(&seen, standing) => {
                // The act's repaint is not in the eye's frame yet: wait for
                // it, and read the frame that has it — or capture.
                let fresh = standing
                    .act
                    .is_some_and(|act| {
                        super::eye::repainted_after(
                            memory,
                            display,
                            (act, standing.stream_id.as_deref()),
                            call,
                            wait,
                        )
                    })
                    .then(|| super::eye::frame(memory, display, call))
                    .flatten()
                    .filter(|frame| after_the_act(frame, standing));
                match fresh {
                    Some(frame) => (frame, Road::Eye),
                    None => (
                        call("screenshotDesktop", Value::Object(ask.clone()))?,
                        Road::EyeBehind,
                    ),
                }
            }
            _ => (seen, Road::Eye),
        }
    } else {
        (
            call("screenshotDesktop", Value::Object(ask.clone()))?,
            Road::Capture,
        )
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
    // Taken out once the picture and its text are had: a look the helper
    // refused leaves the place's last look where it was.
    let held = take_last(&key);
    let frame_seq = (road == Road::Eye)
        .then(|| frame.get(SEQ_KEY).and_then(Value::as_u64))
        .flatten();
    let mut by = None;
    let (changed, share) = if diffing {
        // A diff is only honest against a look at the same place: a frame of
        // another window or another scale is not "what changed".
        let baseline = held
            .as_ref()
            .map(LastFrame::baseline)
            .filter(|held| held.frame == placed);
        let counted = clean.as_ref().and_then(|after| {
            let before = decode_png(&baseline?.png)?;
            let (regions, share) =
                answered_diff(&diff(&before, after, OBSERVE_DIFF_PIXEL_DELTA), placed);
            by = Some("pixels");
            // Nothing at the look's size after an act, on a whole display:
            // where the eye saw the display repaint, any pixel that moved.
            let since_act = baseline.is_some_and(|held| held.acts < acts);
            if !regions.is_empty() || !since_act || !whole_display {
                return Some((regions, share));
            }
            if standing.is_none() {
                standing = super::eye::standing(memory, display, call);
            }
            let repainted = standing.as_ref().and_then(|standing| {
                super::eye::repainted_since(
                    memory,
                    display,
                    (standing.act?, standing.stream_id.as_deref()),
                    frame_seq,
                    call,
                )
            });
            match repainted
                .filter(|rects| !rects.is_empty())
                .and_then(|rects| changes_where_repainted(&before, after, placed, &rects))
            {
                Some(found) if !found.0.is_empty() => {
                    by = Some("repainted");
                    Some(found)
                }
                _ => Some((regions, share)),
            }
        });
        match counted {
            Some((regions, share)) => (Value::Array(regions), Some(share)),
            None => (Value::Null, None),
        }
    } else {
        (Value::Null, None)
    };
    let from_before_act = diffing && held.as_ref().is_some_and(|held| held.before_act.is_some());
    let fingerprint = marks.then(|| super::marks::fingerprint(&png));
    // The diff's baseline is always the clean frame, never the marked one; a
    // look that answers no diff keeps the last look before an act for the
    // diff after it, and a diff answers that act.
    let before_act = if diffing {
        None
    } else {
        held.and_then(|held| held.before(acts))
    };
    keep(
        key.clone(),
        LastFrame {
            png,
            frame: placed,
            acts,
            before_act,
        },
    );
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
    note_line(&serde_json::json!({
        "n": LOOKS.fetch_add(1, Ordering::Relaxed) + 1,
        "atEpochMs": crate::now_epoch_ms(),
        "ms": u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        "road": road.word(),
        "diff": diffing,
        "changed": answer["changed"].as_array().map(Vec::len),
        "changedShare": share,
        "by": by,
        "fromBeforeAct": from_before_act,
        "width": answer.pointer("/screenshot/width"),
        "height": answer.pointer("/screenshot/height"),
        "scale": placed.scale(),
        "frameSeq": frame_seq,
        "actSeq": standing.as_ref().and_then(|standing| standing.act).map(|act| act.seq),
        "actAtMs": standing.as_ref().and_then(|standing| standing.act).map(|act| act.at_ms),
        "eyeNowMs": standing.as_ref().map(|standing| standing.now_ms),
        "acts": acts,
        "lastActEpochMs": act_at_ms,
    }));
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

/// Leave a look's line for `observe` to write: flat, its fields numbers,
/// booleans and words no longer than `LOOK_LINE_WORD_MAX` — never a picture,
/// a text or a name — and never past `LOOK_LINE_MAX_BYTES`.
fn note_line(line: &Value) {
    let flat = line.as_object().is_some_and(|fields| {
        fields.values().all(|value| {
            !value.is_object()
                && !value.is_array()
                && value
                    .as_str()
                    .is_none_or(|word| word.len() <= LOOK_LINE_WORD_MAX)
        })
    });
    let line = line.to_string();
    if flat && line.len() <= LOOK_LINE_MAX_BYTES {
        LOOK_LINES.with(|lines| lines.borrow_mut().push(line));
    }
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
            acts: 0,
            before_act: None,
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
        full_scale: f64,
        look: [Vec<u8>; 2],
        look_size: (u32, u32),
        look_scale: f64,
        /// Where the act repainted, in points: the eye's repaint rect.
        repainted: [f64; 4],
        /// Whether the eye notes that repaint: as the helper's eye does, when
        /// its frame — the look's picture — is not the same to the last bit.
        eye_notes: bool,
    }

    /// A display of `points` on the built-in Retina display's scales.
    fn drawn(
        points: (u32, u32),
        paint: impl Fn(bool, u32, u32) -> [u8; 4],
        repainted: [f64; 4],
    ) -> Drawn {
        drawn_on((FULL_SCALE, BUILT_IN_POINTS_WIDE), points, paint, repainted)
    }

    /// A display of `points`, drawn at `full_scale` pixels a point, whose
    /// look is the ladder's first rung over a display `points_wide` wide.
    fn drawn_on(
        (full_scale, points_wide): (f64, f64),
        points: (u32, u32),
        paint: impl Fn(bool, u32, u32) -> [u8; 4],
        repainted: [f64; 4],
    ) -> Drawn {
        let look_scale =
            zerocode_core::computer_use_protocol::SCREENSHOT_RESIZE_START_PX / points_wide;
        let sized = |scale: f64| {
            (
                (f64::from(points.0) * scale).round() as u32,
                (f64::from(points.1) * scale).round() as u32,
            )
        };
        let (full_size, look_size) = (sized(full_scale), sized(look_scale));
        let full = [false, true].map(|after| {
            (0..full_size.1)
                .flat_map(|y| (0..full_size.0).map(move |x| (x, y)))
                .map(|(x, y)| paint(after, x, y))
                .collect::<Vec<_>>()
        });
        let look =
            [0_usize, 1].map(|at| encoded(&shrink(&full[at], full_size, look_size), look_size));
        let eye_notes = look[0] != look[1];
        Drawn {
            full,
            full_size,
            full_scale,
            look,
            look_size,
            look_scale,
            repainted,
            eye_notes,
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
        (size, scale): ((u32, u32), f64),
        [x, y, width, height]: [f64; 4],
    ) -> (Vec<[u8; 4]>, (u32, u32)) {
        let at = |points: f64, most: u32| ((points * scale).round() as u32).min(most);
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

    /// The eye never delivers the act's repaint.
    const EYE_NEVER_CATCHES_UP: u32 = u32::MAX;

    /// A helper over `drawn`: the eye's newest frame (`eyeFrame` — still the
    /// one before the act until the eye was asked about it `behind` times,
    /// its repaint not delivered yet),
    /// a capture at the look's size (`screenshotDesktop`) or at the display's
    /// own resolution (`fullRes`, or a region), and the eye's account
    /// (`eyeChanges`: the act's mark, and where the act repainted once the
    /// eye has it).
    fn desk<'d>(
        drawn: &'d Drawn,
        acted: &'d std::cell::Cell<bool>,
        behind: u32,
    ) -> impl FnMut(&str, Value) -> Result<Value, ComputerUseError> + 'd {
        let mut asked = 0_u32;
        move |method: &str, params: Value| {
            let now = usize::from(acted.get());
            // The eye delivers the act's repaint once it was asked about
            // the act `behind` times.
            let seen = if acted.get() && asked < behind {
                0
            } else {
                now
            };
            if method == "eyeChanges" && acted.get() {
                asked = asked.saturating_add(1);
            }
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
                        let (piece, size) =
                            cut(&drawn.full[now], (drawn.full_size, drawn.full_scale), asked);
                        answered(
                            &encoded(&piece, size),
                            size,
                            (asked[0], asked[1]),
                            drawn.full_scale,
                        )
                    } else if params.get("fullRes").and_then(Value::as_bool) == Some(true) {
                        let png = encoded(&drawn.full[now], drawn.full_size);
                        answered(&png, drawn.full_size, (0.0, 0.0), drawn.full_scale)
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
                    let changes = if seen == 1 && drawn.eye_notes {
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
        let mut call = desk(&drawn, &acted, EYE_NEVER_CATCHES_UP);
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
        let mut call = desk(&drawn, &acted, 0);
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
        let mut call = desk(&drawn, &acted, 0);
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

    /// t-37883: one window asked for by its English name, its own-language
    /// name and its bundle id is one place — the next look compares with the
    /// first — and the look says who it saw.
    #[test]
    fn an_apps_window_named_three_ways_keeps_one_last_frame() {
        let picture = png(8, 8, |_, _| [9, 9, 9, 255]);
        let mut call = |method: &str, _: Value| -> Result<Value, ComputerUseError> {
            match method {
                "getAppState" => {
                    let mut frame = answered(&picture, (8, 8), (742.0, 61.0), 1.0);
                    frame["snapshot"] = serde_json::json!({
                        "app": { "name": "iPhone Mirroring", "bundleId": "com.apple.ScreenContinuity", "pid": 4243 },
                        "window": { "id": 77, "title": "iPhone Mirroring" },
                        "treeText": "",
                        "elementCount": 1,
                    });
                    Ok(frame)
                }
                other => Err(ComputerUseError::new(
                    error_code::UNSUPPORTED_CAPABILITY,
                    format!("not asked here: {other}"),
                )),
            }
        };
        let memory = super::super::eye::Memory::new();
        let look = |name: &str| {
            let mut params = diff_look("t-37883-names");
            params.insert("app".into(), name.into());
            params
        };
        let first =
            observe_with(&look("iPhone Mirroring"), &memory, &mut call, &mut |_| {}).unwrap();
        assert_eq!(
            first["app"]["bundleId"], "com.apple.ScreenContinuity",
            "the look says who it saw"
        );
        for name in ["iPhone 미러링", "com.apple.ScreenContinuity"] {
            let again = observe_with(&look(name), &memory, &mut call, &mut |_| {}).unwrap();
            assert_eq!(
                again["changed"],
                serde_json::json!([]),
                "{name}: the same window, and nothing changed"
            );
        }
    }

    /// The measurement: every look leaves one line of numbers — how many
    /// rectangles changed and what share, the frame's and the last act's
    /// stamps, the picture's size and scale — and nothing it saw: no
    /// picture, no text, no window's name.
    #[test]
    fn a_look_leaves_its_numbers_and_nothing_it_saw() {
        let drawn = drawn((64, 40), square, [20.0, 12.0, 12.0, 12.0]);
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, 0);
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

    /// The other side of B: where the eye saw the display repaint after the
    /// act but the picture is as it was — a highlight that came and went —
    /// nothing changed, and the look says so.
    #[test]
    fn a_repaint_that_came_and_went_is_not_a_change() {
        let mut drawn = drawn(
            (64, 40),
            |_, _, _| [200, 200, 200, 255],
            [20.0, 12.0, 12.0, 12.0],
        );
        // The eye noted the highlight's repaints; the picture is as it was.
        drawn.eye_notes = true;
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, 0);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-e");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert_eq!(answer["changed"], serde_json::json!([]));
        assert_eq!(answer["changedShare"], 0.0);
    }

    /// C, once: the diff after an act answers the act, and the next diff
    /// counts from that one — the act's baseline is not kept past it.
    #[test]
    fn the_diff_after_an_act_answers_it_once() {
        let drawn = drawn((64, 40), square, [20.0, 12.0, 12.0, 12.0]);
        let acted = std::cell::Cell::new(false);
        let mut call = desk(&drawn, &acted, 0);
        let memory = super::super::eye::Memory::new();
        let params = diff_look("t-15517-f");
        observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        act(&acted);
        let mut shot = Map::new();
        shot.insert("viewer".into(), "t-15517-f".into());
        let screenshot = call("eyeFrame", Value::Object(Map::new())).unwrap();
        remember(&shot, &screenshot, drawn.look[1].clone());
        let first = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert!(says_changed(&first), "{}", first["changed"]);
        let again = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
        assert_eq!(
            again["changed"],
            serde_json::json!([]),
            "the second diff counts from the first"
        );
    }

    /// The displays the scenes are drawn on: a name, pixels a point at the
    /// display's own resolution, and its width in points (its look is the
    /// ladder's first rung over it) — an external 1920 × 1080 at 1×, the
    /// built-in 1512 × 982 at 2×, and an external 3008 × 1692 at 2×, where
    /// the look takes 0.43 pixels a point.
    const SCENE_DISPLAYS: [(&str, f64, f64); 3] = [
        ("1920@1", 1.0, 1920.0),
        ("1512@2", 2.0, 1512.0),
        ("3008@2", 2.0, 3008.0),
    ];
    /// The part of the display a scene draws, in points.
    const SCENE_POINTS: (u32, u32) = (96, 64);
    /// The side of what an act changes in a scene, in points: a control.
    const SCENE_BOX: f64 = 12.0;
    /// Where the box sits: off the look's pixel grid in every way.
    const SCENE_AT: [(f64, f64); 6] = [
        (8.3, 6.1),
        (22.0, 14.0),
        (35.7, 21.9),
        (49.4, 29.8),
        (63.1, 37.7),
        (76.8, 45.6),
    ];

    /// What an act does to a 12 × 12-point control at `at`, painted at the
    /// display's own pixels (`x`, `y` at `scale` pixels a point): before
    /// the act and after it. Every kind but `none` changes the screen.
    fn scene_paint(
        kind: &str,
        (scale, at): (f64, (f64, f64)),
        after: bool,
        x: u32,
        y: u32,
    ) -> [u8; 4] {
        let grey = |level: u8| [level, level, level, 255];
        let (px, py) = (f64::from(x) / scale, f64::from(y) / scale);
        let inside = |left: f64, top: f64, side: f64| {
            (at.0 + left..at.0 + left + side).contains(&px)
                && (at.1 + top..at.1 + top + side).contains(&py)
        };
        let on_box = inside(0.0, 0.0, SCENE_BOX);
        // The box's edge, one of the display's own pixels wide.
        let edge = on_box && !inside(1.0 / scale, 1.0 / scale, SCENE_BOX - 2.0 / scale);
        match kind {
            // A control that turns from dark to white.
            "square" if after && on_box => grey(255),
            "square" => grey(40),
            // A focus ring: the box's one-pixel border darkens by 11.
            "border" if after && edge => grey(189),
            "border" => grey(200),
            // A checkbox checked: its inner 6 points fill.
            "check" if after && inside(3.0, 3.0, 6.0) => grey(60),
            "check" => grey(245),
            // A caret: one point wide, the box's height.
            "caret" if after && on_box && px < at.0 + 1.0 => grey(0),
            "caret" => grey(255),
            // A label's glyphs: one-pixel strokes every third pixel.
            "strokes" if after && on_box && x.is_multiple_of(3) => grey(60),
            "strokes" => grey(250),
            // A hover tint under the drift: the box shifts by 6.
            "tint" if after && on_box => grey(234),
            "tint" => grey(240),
            // Nothing: the act changed nothing.
            _ => grey(128),
        }
    }

    /// Acceptance (spec item 4), in synthetic frames: on every display, for
    /// every kind of change, place and road — the eye's frame after the act,
    /// the eye still behind it, a screenshot between the act and the diff —
    /// the look after the act never says "nothing changed" when it did, and
    /// never "changed" when nothing did.
    #[test]
    fn no_wrong_word_in_the_synthetic_scenes() {
        const KINDS: [&str; 7] = [
            "square", "border", "check", "caret", "strokes", "tint", "none",
        ];
        const ROADS: [&str; 3] = ["eye", "eye-behind", "shot-between"];
        let mut wrong = Vec::new();
        let mut counted = Vec::new();
        for (name, scale, wide) in SCENE_DISPLAYS {
            let mut acts = 0;
            for (kind, road, at) in KINDS.iter().flat_map(|kind| {
                ROADS
                    .iter()
                    .flat_map(move |road| SCENE_AT.iter().map(move |at| (*kind, *road, *at)))
            }) {
                let drawn = drawn_on(
                    (scale, wide),
                    SCENE_POINTS,
                    |after, x, y| scene_paint(kind, (scale, at), after, x, y),
                    [at.0, at.1, SCENE_BOX, SCENE_BOX],
                );
                let acted = std::cell::Cell::new(false);
                let mut call = desk(&drawn, &acted, u32::from(road == "eye-behind"));
                let memory = super::super::eye::Memory::new();
                let viewer = format!("t-15517-scene-{name}-{kind}-{road}-{}", at.0);
                let params = diff_look(&viewer);
                observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
                act(&acted);
                if road == "shot-between" {
                    let mut shot = Map::new();
                    shot.insert("viewer".into(), viewer.clone().into());
                    let screenshot = call("eyeFrame", Value::Object(Map::new())).unwrap();
                    remember(&shot, &screenshot, drawn.look[1].clone());
                }
                let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
                drain_looks();
                acts += 1;
                let truly = drawn.full[0] != drawn.full[1];
                let said = answer["changed"].as_array().map(|rects| !rects.is_empty());
                if said != Some(truly) {
                    wrong.push(format!(
                        "{name} {kind} {road} at {at:?}: {}",
                        answer["changed"]
                    ));
                }
            }
            counted.push(format!("{name}: {acts} acts"));
        }
        eprintln!("t-15517 scenes: {counted:?}, wrong {}", wrong.len());
        assert!(wrong.is_empty(), "{} wrong words: {wrong:#?}", wrong.len());
    }

    /// What the fix adds to a look, at the look's real size (the built-in
    /// display's 1280 × 831): the look after an act that changed a control
    /// (the frame's stamp is checked), and after one whose change is under
    /// the drift at the look's size (checked where the eye saw the repaint),
    /// against a look with no act before it. The helper here answers at
    /// once, so what is timed is the window's own work; the helper's round
    /// trips are measured on a real display. Not part of the gate: run it
    /// with `--ignored --nocapture`.
    #[test]
    #[ignore = "times the look's own work on frames of a real display's size"]
    fn what_the_fix_adds_to_a_look() {
        const RUNS: usize = 60;
        let (wide, high) = (1280_u32, 831_u32);
        let look_scale =
            zerocode_core::computer_use_protocol::SCREENSHOT_RESIZE_START_PX / BUILT_IN_POINTS_WIDE;
        let paint = |shift: i16| {
            let pixels: Vec<[u8; 4]> = (0..high)
                .flat_map(|y| (0..wide).map(move |x| (x, y)))
                .map(|(x, y)| {
                    // A desktop's texture: bands and edges, not a flat fill.
                    let base = 180 + ((x / 7 + y / 5) % 40) as i16;
                    let on_box = (200..210).contains(&x) && (100..110).contains(&y);
                    let level = if on_box { base + shift } else { base };
                    let level = u8::try_from(level.clamp(0, 255)).unwrap();
                    [level, level, level, 255]
                })
                .collect();
            encoded(&pixels, (wide, high))
        };
        let before = paint(0);
        let cases = [
            ("no act", 0, false),
            ("act, a control", 60, true),
            ("act, under the drift", -5, true),
        ];
        for (name, shift, acting) in cases {
            let after = paint(shift);
            let drawn = Drawn {
                full: [Vec::new(), Vec::new()],
                full_size: (wide, high),
                full_scale: look_scale,
                eye_notes: before != after,
                look: [before.clone(), after],
                look_size: (wide, high),
                look_scale,
                repainted: [
                    200.0 / look_scale,
                    100.0 / look_scale,
                    10.0 / look_scale,
                    10.0 / look_scale,
                ],
            };
            let mut times = Vec::with_capacity(RUNS);
            for run in 0..RUNS {
                let acted = std::cell::Cell::new(false);
                let mut call = desk(&drawn, &acted, 0);
                let memory = super::super::eye::Memory::new();
                let params = diff_look(&format!("t-15517-bench-{name}-{run}"));
                observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
                if acting {
                    act(&acted);
                } else {
                    // The same picture after, with no act: the old look.
                    acted.set(true);
                }
                let began = std::time::Instant::now();
                let answer = observe_with(&params, &memory, &mut call, &mut |_| {}).unwrap();
                times.push(began.elapsed().as_micros());
                drain_looks();
                assert_eq!(
                    answer["changed"]
                        .as_array()
                        .is_some_and(|rects| !rects.is_empty()),
                    shift != 0,
                    "{name}"
                );
            }
            times.sort_unstable();
            eprintln!(
                "t-15517 bench {name}: p50 {} us, p95 {} us ({RUNS} looks)",
                times[RUNS / 2],
                times[RUNS * 95 / 100]
            );
        }
    }
}
