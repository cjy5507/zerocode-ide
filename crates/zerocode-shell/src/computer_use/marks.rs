//! The window's half of the marks (docs/design/computer-use-full-operator.md
//! §7.1): a look asked `--marks` gets the controls of one window numbered on
//! its picture, and `click --mark N --look L` presses the control numbered N
//! on that look through the element path, with the pin that refuses it when
//! the control moved or changed since.
//!
//! The core plans (`computer_use_protocol::marks`); this file reads the
//! helper's faces, draws the badges into the picture the agent is shown,
//! keeps each look's numbers under its own id, and turns a mark back into
//! the helper's element click. A look never fails because of its marks: a
//! mark that cannot be made says why in `marks.unavailable`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use base64::Engine as _;
use serde_json::{Map, Value, json};
use zerocode_core::computer_use::{
    MARK_BADGE_BORDER_PX, MARK_BADGE_PAD_PX, MARK_DIGIT_GLYPHS, MARK_FILL_RGBA, MARK_GLYPH_COLUMNS,
    MARK_GLYPH_GAP_PX, MARK_GLYPH_ROWS, MARK_GLYPH_SCALE, MARK_INK_RGBA, MARK_LOOKS_KEPT,
    MARK_OUTLINE_PX, MARK_PIN_TOLERANCE_POINTS, MARK_SNAPSHOT_SESSION,
};
use zerocode_core::computer_use_protocol::frame::ShotFrame;
use zerocode_core::computer_use_protocol::marks::{
    self as plan, DesktopWindow, ELEMENT_FRAMES_KEY, ELEMENTS_KEY, EVERY_LAYER_KEY, ElementFace,
    MarkInput, MarkedWindow, PlacedMark,
};
use zerocode_core::computer_use_protocol::render::Rect;
use zerocode_core::computer_use_protocol::{cache, error_code};

use super::ComputerUseError;
use super::observe::Kept;
use super::screenshot_png::RgbaImage;

/// One marked look's numbers, kept under its id.
#[derive(Debug, Clone)]
pub(super) struct MarkTable {
    /// Which place it was a look at (observe's frame key), and the clean
    /// picture's fingerprint: a later look there with the very same pixels
    /// keeps these numbers.
    place: String,
    picture: u64,
    window: MarkedWindow,
    marks: Vec<PlacedMark>,
    candidates: usize,
    omitted: usize,
    frame: ShotFrame,
    made: Instant,
}

static TABLES: Mutex<Kept<MarkTable>> = Mutex::new(Kept::new(MARK_LOOKS_KEPT));

/// A clean picture's fingerprint: the same pixels, the same PNG bytes, the
/// same number.
pub(super) fn fingerprint(png: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    png.hash(&mut hasher);
    hasher.finish()
}
static LOOKS: AtomicU64 = AtomicU64::new(0);

/// A new look's id: this window process and a count, so an id read before a
/// restart never names a look made after it.
fn next_look_id() -> String {
    format!(
        "{}.{}",
        std::process::id(),
        LOOKS.fetch_add(1, Ordering::Relaxed) + 1
    )
}

fn tables() -> std::sync::MutexGuard<'static, Kept<MarkTable>> {
    TABLES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// What a look with `--marks` adds: the numbers, and the picture with them
/// drawn in (`None` when nothing could be drawn and the clean picture stays).
pub(super) struct Marked {
    pub answer: Value,
    pub png: Option<Vec<u8>>,
}

fn unavailable(why: &ComputerUseError) -> Marked {
    Marked {
        answer: json!({ "unavailable": format!("{}: {}", why.code, why.message) }),
        png: None,
    }
}

/// What a desktop look knew before its picture: every window on the screen,
/// front to back — the target is chosen from it and must stand still.
pub(super) fn desktop_windows(
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
) -> Result<Vec<DesktopWindow>, ComputerUseError> {
    let listed = call("listAllWindows", json!({ EVERY_LAYER_KEY: true }))?;
    Ok(listed
        .get("windows")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(DesktopWindow::from_row).collect())
        .unwrap_or_default())
}

/// The picture a look was taken of, and where.
pub(super) struct Picture<'a> {
    pub clean: &'a RgbaImage,
    pub fingerprint: u64,
    pub placed: ShotFrame,
    pub place: &'a str,
}

/// Mark a look: the app's own window when the look named one, else the
/// frontmost document window on the picture that is not ZeroCode's own
/// (`before`: the windows listed just before the picture). A look at the
/// same place whose pixels are the very ones a kept table was drawn on keeps
/// that table's numbers and draws them again, with no walk.
pub(super) fn mark_look(
    params: &Map<String, Value>,
    frame_answer: &Value,
    picture: &Picture<'_>,
    before: Option<Result<Vec<DesktopWindow>, ComputerUseError>>,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
) -> Marked {
    // A caller that does not want the picture is not drawn one: the numbers
    // are the answer, and the badges are for a person's eyes.
    let draws = !zerocode_core::computer_use_protocol::params::flag(params, "noScreenshot");
    if let Some(kept) = latest_at(picture) {
        return draw_answer(kept, picture.clean, true, draws);
    }
    let made = if params.contains_key("app") {
        app_marks(frame_answer, picture)
    } else {
        match before {
            Some(Ok(before)) => desktop_marks(&before, picture, call),
            Some(Err(why)) => Err(why),
            None => Err(ComputerUseError::new(
                error_code::WINDOW_NOT_FOUND,
                "a desktop look is marked only with the windows listed before its picture",
            )),
        }
    };
    match made {
        Ok(Some(table)) => {
            tables().keep(table.window.look_id.clone(), table.clone());
            draw_answer(table, picture.clean, false, draws)
        }
        Ok(None) => unavailable(&ComputerUseError::new(
            error_code::WINDOW_NOT_FOUND,
            "no window on this picture is one to mark",
        )),
        Err(why) => unavailable(&why),
    }
}

/// The newest fresh table made at this place from these very pixels; its
/// clock starts again, since this look saw the same screen.
fn latest_at(picture: &Picture<'_>) -> Option<MarkTable> {
    let mut held = tables();
    let key = held
        .iter()
        .rev()
        .find(|(_, table)| {
            table.place == picture.place
                && table.picture == picture.fingerprint
                && table.frame == picture.placed
                && !cache::is_expired(table.made, Instant::now())
        })
        .map(|(key, _)| key.clone())?;
    let mut table = held.take(&key)?;
    table.made = Instant::now();
    held.keep(key, table.clone());
    Some(table)
}

fn faces_of(snapshot: &Value) -> Result<Vec<ElementFace>, ComputerUseError> {
    let faces = snapshot.get(ELEMENTS_KEY).ok_or_else(|| {
        ComputerUseError::new(
            error_code::PROVIDER_INCOMPATIBLE,
            "the provider answered no element faces (an older helper?)",
        )
    })?;
    serde_json::from_value(faces.clone()).map_err(|error| {
        ComputerUseError::new(error_code::PROVIDER_INCOMPATIBLE, error.to_string())
    })
}

fn window_rect(snapshot: &Value) -> Option<Rect> {
    let number = |key: &str| {
        snapshot
            .pointer(&format!("/window/{key}"))
            .and_then(Value::as_f64)
    };
    Some(Rect::new(
        number("x")?,
        number("y")?,
        number("width")?,
        number("height")?,
    ))
}

fn marked_window(snapshot: &Value) -> MarkedWindow {
    MarkedWindow {
        look_id: next_look_id(),
        app: snapshot
            .pointer("/app/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        pid: snapshot
            .pointer("/app/pid")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        window_id: snapshot
            .pointer("/window/id")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    }
}

fn table(
    snapshot: &Value,
    window: Rect,
    picture: &Picture<'_>,
    occluders: Vec<Rect>,
) -> Result<MarkTable, ComputerUseError> {
    let faces = faces_of(snapshot)?;
    let planned = plan::plan(&MarkInput {
        faces: &faces,
        window,
        frame: picture.placed,
        picture: (picture.clean.width, picture.clean.height),
        occluders,
    });
    Ok(MarkTable {
        place: picture.place.to_string(),
        picture: picture.fingerprint,
        window: marked_window(snapshot),
        candidates: planned.candidates,
        omitted: planned.omitted,
        marks: planned.marks,
        frame: picture.placed,
        made: Instant::now(),
    })
}

/// An app look: the helper walked the named window already and answered its
/// faces beside the tree.
fn app_marks(
    frame_answer: &Value,
    picture: &Picture<'_>,
) -> Result<Option<MarkTable>, ComputerUseError> {
    let snapshot = frame_answer.get("snapshot").ok_or_else(|| {
        ComputerUseError::new(
            error_code::PROVIDER_INCOMPATIBLE,
            "an app look without its snapshot",
        )
    })?;
    let Some(window) = window_rect(snapshot) else {
        return Ok(None);
    };
    table(snapshot, window, picture, Vec::new()).map(Some)
}

/// A desktop look: the frontmost document window on the picture that is not
/// ZeroCode's own, walked in the marks' own helper namespace; every window
/// in front of it — menus, panels, the Dock — hides what it covers. The
/// windows down to it must stand still from before the picture to after the
/// walk, and the walk must find the window where the list put it: otherwise
/// its badges would sit on pixels of another moment.
fn desktop_marks(
    before: &[DesktopWindow],
    picture: &Picture<'_>,
    call: &mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
) -> Result<Option<MarkTable>, ComputerUseError> {
    let placed = picture.placed;
    let [x, y] = placed.to_point([0.0, 0.0]);
    let shown = Rect::new(
        x,
        y,
        placed.length_to_point(f64::from(picture.clean.width)),
        placed.length_to_point(f64::from(picture.clean.height)),
    );
    let Some(target) = plan::desktop_target(before, shown) else {
        return Ok(None);
    };
    let row = &before[target];
    let mut ask = Map::new();
    ask.insert("app".into(), format!("pid:{}", row.pid).into());
    ask.insert("windowId".into(), row.id.into());
    ask.insert("noScreenshot".into(), true.into());
    ask.insert(ELEMENT_FRAMES_KEY.into(), true.into());
    ask.insert("session".into(), MARK_SNAPSHOT_SESSION.into());
    let walked = call("getAppState", Value::Object(ask))?;
    let snapshot = walked.get("snapshot").ok_or_else(|| {
        ComputerUseError::new(
            error_code::PROVIDER_INCOMPATIBLE,
            "an app look without its snapshot",
        )
    })?;
    let Some(window) = window_rect(snapshot) else {
        return Ok(None);
    };
    let after = desktop_windows(call)?;
    if !window.matches_within(&row.rect, MARK_PIN_TOLERANCE_POINTS)
        || !plan::stood_still(before, &after, row.id, MARK_PIN_TOLERANCE_POINTS)
    {
        return Err(ComputerUseError::new(
            error_code::WINDOW_STALE,
            format!(
                "{} or a window in front of it moved while it was looked at; look again",
                row.app
            ),
        ));
    }
    table(snapshot, window, picture, plan::occluders(before, target)).map(Some)
}

/// Draw a table's marks into a copy of the clean picture and answer them in
/// screen points (the look speaks them in the picture's pixels later). A
/// picture that cannot be encoded carries no numbers: a legend with no
/// badges would name what the agent cannot see.
/// The table as an answer, and — unless the caller said it did not want the
/// picture — the same pixels with the badges drawn on them.
///
/// Drawing is what a PERSON reads; the numbers are what a caller spends. A
/// walk that presses by number never opens the picture, and copying the frame,
/// painting ninety-nine badges on it and encoding a PNG cost 262 ms of every
/// look on this machine (measured 2026-09-18, Finder's 291-element tree: a
/// marked look 645 ms, the same frame and tree without the drawing 383 ms).
/// Over a thirty-step walk that is 7.8 seconds spent on pictures nobody opens,
/// so `--no-screenshot` skips it and the answer carries the numbers alone.
fn draw_answer(table: MarkTable, clean: &RgbaImage, same: bool, draws: bool) -> Marked {
    let png = if draws {
        let mut picture = clean.clone();
        draw(&mut picture, &table.marks);
        let Some(png) = picture.encode() else {
            return unavailable(&ComputerUseError::new(
                error_code::SCREENSHOT_FAILED,
                "the marked picture could not be encoded",
            ));
        };
        Some(png)
    } else {
        None
    };
    let mut answer = plan::answer(
        &plan::MarkPlan {
            marks: table.marks.clone(),
            candidates: table.candidates,
            omitted: table.omitted,
        },
        &table.window,
    );
    if same {
        answer["sameAsLastLook"] = Value::Bool(true);
    }
    Marked { answer, png }
}

fn paint(picture: &mut RgbaImage, x0: i64, y0: i64, x1: i64, y1: i64, rgba: [u8; 4]) {
    let (width, height) = (i64::from(picture.width), i64::from(picture.height));
    for y in y0.max(0)..y1.min(height) {
        for x in x0.max(0)..x1.min(width) {
            let at = usize::try_from((y * width + x) * 4).unwrap_or_default();
            picture.pixels[at..at + 4].copy_from_slice(&rgba);
        }
    }
}

fn edges(rect: &Rect) -> (i64, i64, i64, i64) {
    #[allow(clippy::cast_possible_truncation)]
    let round = |value: f64| value.round() as i64;
    (
        round(rect.x),
        round(rect.y),
        round(rect.max_x()),
        round(rect.max_y()),
    )
}

/// Draw a desktop look's marks: each `PlacedMark`'s outline and badge, down
/// the one badge renderer ([`draw_badges`]).
pub(super) fn draw(picture: &mut RgbaImage, marks: &[PlacedMark]) {
    let placed: Vec<(Rect, Rect, usize)> = marks
        .iter()
        .map(|mark| (mark.element_px, mark.badge_px, mark.mark))
        .collect();
    draw_badges(picture, &placed);
}

/// The one badge renderer the desktop look and the browser marks share: each
/// control's outline first (so no outline crosses a badge), then each badge —
/// fill, an ink border, the digits — from a triple of `(element_px, badge_px,
/// number)` in the picture's own pixels. Every pixel is the table's fill or
/// ink.
pub(crate) fn draw_badges(picture: &mut RgbaImage, placed: &[(Rect, Rect, usize)]) {
    let line = i64::from(MARK_OUTLINE_PX);
    for (element_px, _, _) in placed.iter().filter(|_| line > 0) {
        let (x0, y0, x1, y1) = edges(element_px);
        paint(picture, x0, y0, x1, y0 + line, MARK_FILL_RGBA);
        paint(picture, x0, y1 - line, x1, y1, MARK_FILL_RGBA);
        paint(picture, x0, y0, x0 + line, y1, MARK_FILL_RGBA);
        paint(picture, x1 - line, y0, x1, y1, MARK_FILL_RGBA);
    }
    let border = i64::from(MARK_BADGE_BORDER_PX);
    let inset = border + i64::from(MARK_BADGE_PAD_PX);
    let scale = i64::from(MARK_GLYPH_SCALE);
    let step = i64::from(MARK_GLYPH_COLUMNS * MARK_GLYPH_SCALE + MARK_GLYPH_GAP_PX);
    for (_, badge_px, number) in placed {
        let (x0, y0, x1, y1) = edges(badge_px);
        paint(picture, x0, y0, x1, y1, MARK_INK_RGBA);
        paint(
            picture,
            x0 + border,
            y0 + border,
            x1 - border,
            y1 - border,
            MARK_FILL_RGBA,
        );
        for (at, digit) in plan::digits(*number).into_iter().enumerate() {
            let left = x0 + inset + i64::try_from(at).unwrap_or_default() * step;
            for (row, bits) in MARK_DIGIT_GLYPHS[digit].iter().enumerate() {
                for column in 0..MARK_GLYPH_COLUMNS {
                    if bits & (1 << (MARK_GLYPH_COLUMNS - 1 - column)) != 0 {
                        let x = left + i64::from(column) * scale;
                        let y = y0 + inset + i64::try_from(row).unwrap_or_default() * scale;
                        paint(picture, x, y, x + scale, y + scale, MARK_INK_RGBA);
                    }
                }
            }
        }
    }
    debug_assert!(MARK_GLYPH_ROWS as usize == MARK_DIGIT_GLYPHS[0].len());
}

/// A marked picture as the look answers it: the base64 PNG in the frame.
pub(super) fn put_picture(answer: &mut Value, png: &[u8]) {
    if let Some(screenshot) = answer.get_mut("screenshot").and_then(Value::as_object_mut) {
        screenshot.insert(
            "data".into(),
            base64::engine::general_purpose::STANDARD.encode(png).into(),
        );
    }
}

/// A click by mark, resolved: the helper's element click and what it names.
#[derive(Debug, Clone)]
pub struct PinnedClick {
    pub params: Map<String, Value>,
    pub mark: usize,
    pub look: String,
    pub role: String,
    pub label: Option<String>,
    pub element_index: usize,
    pub app: String,
    /// The mark's rectangle in screen points, as the look placed it — what
    /// a walk rings on the frame after the press (plan D11), read from the
    /// table, never from the tree again.
    pub frame: zerocode_core::computer_use_protocol::render::Rect,
}

/// Turn `click --mark N --look L` into the element click on the look's
/// window, pinned by what the look saw.
pub fn pinned_click(params: &Value) -> Result<PinnedClick, ComputerUseError> {
    let look = params
        .get("look")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mark = params
        .get("mark")
        .and_then(Value::as_u64)
        .and_then(|mark| usize::try_from(mark).ok())
        .unwrap_or_default();
    let table = tables().get(look).cloned().ok_or_else(|| {
        ComputerUseError::new(
            error_code::ELEMENT_NOT_FOUND,
            format!("no marked look {look} (a newer look replaced it, or the window restarted); look again with --marks"),
        )
    })?;
    if cache::is_expired(table.made, Instant::now()) {
        return Err(ComputerUseError::new(
            error_code::ELEMENT_NOT_FOUND,
            format!(
                "marked look {look} is older than {} s; look again with --marks",
                cache::MAX_AGE.as_secs()
            ),
        ));
    }
    let placed = mark
        .checked_sub(1)
        .and_then(|at| table.marks.get(at))
        .ok_or_else(|| {
            ComputerUseError::invalid_argument(format!(
                "mark {mark} is not on look {look} (1–{})",
                table.marks.len()
            ))
        })?;
    let mut helper = Map::new();
    helper.insert("app".into(), format!("pid:{}", table.window.pid).into());
    helper.insert("windowId".into(), table.window.window_id.into());
    helper.insert("elementIndex".into(), placed.element_index.into());
    helper.insert("session".into(), MARK_SNAPSHOT_SESSION.into());
    helper.insert("noScreenshot".into(), true.into());
    placed.pin(MARK_PIN_TOLERANCE_POINTS).write(&mut helper);
    for key in ["mouseButton", "clickCount", "modifiers", "confirming"] {
        if let Some(value) = params.get(key) {
            helper.insert(key.into(), value.clone());
        }
    }
    Ok(PinnedClick {
        params: helper,
        mark,
        look: look.to_string(),
        role: placed.role.clone(),
        label: placed.label.clone(),
        element_index: placed.element_index,
        app: table.window.app,
        frame: placed.screen,
    })
}

/// What a mark click answers: what it pressed, and the window its evidence
/// frame is taken by — not the tree or a picture (the caller looks again).
#[must_use]
pub fn click_answer(mut answer: Value, mark: &PinnedClick) -> Value {
    if let Some(snapshot) = answer.get_mut("snapshot").and_then(Value::as_object_mut) {
        snapshot.remove("treeText");
    }
    if let Some(object) = answer.as_object_mut() {
        object.remove("screenshot");
        object.insert(
            "mark".into(),
            json!({ "mark": mark.mark, "look": mark.look, "role": mark.role, "label": mark.label,
                    "elementIndex": mark.element_index, "app": mark.app,
                    "frame": { "x": mark.frame.x, "y": mark.frame.y,
                               "width": mark.frame.width, "height": mark.frame.height } }),
        );
    }
    answer
}

/// A mark click the pin refused says which mark, in the model's words.
#[must_use]
pub fn click_refusal(error: ComputerUseError, mark: &PinnedClick) -> ComputerUseError {
    if error.code != error_code::ELEMENT_NOT_FOUND {
        return error;
    }
    let named = [Some(mark.role.as_str()), mark.label.as_deref()]
        .into_iter()
        .flatten()
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    ComputerUseError::new(
        error_code::ELEMENT_NOT_FOUND,
        format!(
            "mark {} ({named}) moved or changed since look {}; look again with --marks",
            mark.mark, mark.look
        ),
    )
}

#[cfg(test)]
mod tests;
