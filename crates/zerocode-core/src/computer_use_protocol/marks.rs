//! The marks (docs/design/computer-use-full-operator.md §7.1): which controls
//! of one window get a number on the picture the model reads, where each
//! badge sits, and the pin a click by number carries, so a control that moved
//! or changed since the look is refused rather than pressed.
//!
//! The provider only exports what its walk saw (`snapshot.elements`, one
//! [`ElementFace`] per indexed element with a frame); choosing, numbering and
//! placing happen here once, and the window draws. Every platform gets the
//! same marks from the same faces. The one rule mirrored into the macOS
//! helper is [`Pin::holds`] (`MarkPin.swift`, case for case), and its
//! tolerance travels in the request: the helper keeps no numbers of its own.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::frame::{SPACE_KEY, ShotFrame};
use super::render::{self, Rect, RenderNode, RenderedRecord, fully_covered};
use super::{ProviderError, error_code};
use crate::computer_use::{
    MACOS_PRESS_ACTIONS, MARK_ANCHORS_INSIDE_FIRST, MARK_ANCHORS_OUTSIDE_FIRST,
    MARK_BADGE_BORDER_PX, MARK_BADGE_PAD_PX, MARK_CAP, MARK_GLYPH_COLUMNS, MARK_GLYPH_GAP_PX,
    MARK_GLYPH_ROWS, MARK_GLYPH_SCALE, MARK_INSIDE_MAX_SHARE, MARK_LABEL_MAX_CHARS,
    MARK_MAX_WINDOW_SHARE, MARK_MIN_SIDE_POINTS, MARK_NEVER_ROLES, MARK_ROLES,
    MARK_SAME_TARGET_IOU, MARK_SKIP_TRAITS, TEXT_ENTRY_ROLES, WINDOWS_PRESS_PATTERNS,
};

/// The words the window, the helper and the Windows provider share.
pub const ELEMENT_FRAMES_KEY: &str = "elementFrames";
pub const ELEMENTS_KEY: &str = "elements";
pub const PIN_SIGNATURE_KEY: &str = "elementSignature";
pub const PIN_NAME_KEY: &str = "elementName";
pub const PIN_CONTEXT_KEY: &str = "elementContext";
pub const PIN_FRAME_KEY: &str = "elementFrame";
pub const PIN_TOLERANCE_KEY: &str = "markTolerance";
/// The space a marked look's items are answered in before the window
/// speaks them in the picture's pixels (`ShotFrame::pixelize`).
pub const SCREEN_SPACE: &str = "screen";
/// The answer's own words.
pub const LOOK_ID_KEY: &str = "lookId";
pub const ITEMS_KEY: &str = "items";
pub const LEGEND_KEY: &str = "legend";
/// Words in a marked look that are the tool's to read, not a model's: a zo
/// look drops them once the legend is written.
pub const TOOL_ONLY_KEYS: &[&str] = &[SPACE_KEY, "pid", "windowId", ITEMS_KEY];

/// One indexed element as the walk saw it: what a mark is chosen and drawn
/// from, and what its pin must find again. The frame is window-local, in the
/// provider's units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementFace {
    pub index: usize,
    pub role: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub traits: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub signature: String,
    /// The frame cut by the clipping containers above it (none: all of it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<FaceFrame>,
    /// The nearest named element above it — the row a star sits in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// A rectangle on the wire, window-local.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FaceFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<Rect> for FaceFrame {
    fn from(rect: Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        }
    }
}

impl ElementFace {
    #[must_use]
    pub fn local(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }

    /// What of it can be seen: its clipped frame, or all of it.
    #[must_use]
    pub fn seen(&self) -> Rect {
        self.visible.map_or_else(
            || self.local(),
            |seen| Rect::new(seen.x, seen.y, seen.width, seen.height),
        )
    }

    /// The words it is known by: its name, else its placeholder (two
    /// unlabelled fields are told apart by what they ask for).
    #[must_use]
    pub fn words(&self) -> &str {
        self.name
            .as_deref()
            .or(self.placeholder.as_deref())
            .unwrap_or_default()
    }

    /// What a pin finds again: the identity, the words and the place in the
    /// tree. A frame alone cannot tell two look-alikes apart.
    fn identity(&self) -> (&str, &str, &str) {
        (
            self.signature.as_str(),
            self.words(),
            self.context.as_deref().unwrap_or_default(),
        )
    }
}

/// The faces of a walk's records, by index: every record with a frame of
/// positive area.
#[must_use]
pub fn element_faces<H>(records: &BTreeMap<usize, RenderedRecord<H>>) -> Vec<ElementFace> {
    records
        .values()
        .filter_map(|record| {
            let frame = record.local_frame?;
            (frame.width > 0.0 && frame.height > 0.0).then(|| ElementFace {
                index: record.index,
                role: record.role.clone(),
                name: record.name.clone(),
                placeholder: record.placeholder.clone(),
                traits: record.traits.clone(),
                actions: record.actions.clone(),
                x: frame.x,
                y: frame.y,
                width: frame.width,
                height: frame.height,
                signature: record.signature.clone(),
                visible: record.visible.map(FaceFrame::from),
                context: record.context.clone(),
            })
        })
        .collect()
}

/// What a click by number proves before it presses: the element at the index
/// is still the one the mark was drawn on — the same identity, the same
/// words (a row's text changes when a new row pushes it down with its frame
/// unchanged), the same place in the tree (a star is known by its row), the
/// same frame within the tolerance.
#[derive(Debug, Clone, PartialEq)]
pub struct Pin {
    pub signature: String,
    /// The element's words: its name, else its placeholder.
    pub name: String,
    pub context: String,
    pub frame: Rect,
    pub tolerance: f64,
}

impl Pin {
    /// A request's pin: all five keys or none.
    pub fn from_params(params: &Map<String, Value>) -> Result<Option<Self>, ProviderError> {
        let keys = [
            PIN_SIGNATURE_KEY,
            PIN_NAME_KEY,
            PIN_CONTEXT_KEY,
            PIN_FRAME_KEY,
            PIN_TOLERANCE_KEY,
        ];
        let present = keys.iter().filter(|key| params.contains_key(**key)).count();
        if present == 0 {
            return Ok(None);
        }
        let incomplete = || {
            ProviderError::invalid_argument(format!(
                "a mark's pin needs {} together",
                keys.join(", ")
            ))
        };
        if present != keys.len() {
            return Err(incomplete());
        }
        let text = |key: &str| {
            params
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(incomplete)
        };
        let frame = params.get(PIN_FRAME_KEY).ok_or_else(incomplete)?;
        let number = |key: &str| {
            frame
                .get(key)
                .and_then(Value::as_f64)
                .filter(|value| value.is_finite())
                .ok_or_else(|| {
                    ProviderError::invalid_argument(format!(
                        "{PIN_FRAME_KEY}.{key} must be a number"
                    ))
                })
        };
        let frame = Rect::new(
            number("x")?,
            number("y")?,
            number("width")?,
            number("height")?,
        );
        if frame.width < 0.0 || frame.height < 0.0 {
            return Err(ProviderError::invalid_argument(format!(
                "{PIN_FRAME_KEY} needs a non-negative size"
            )));
        }
        let tolerance = params
            .get(PIN_TOLERANCE_KEY)
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| {
                ProviderError::invalid_argument(format!(
                    "{PIN_TOLERANCE_KEY} must be a non-negative number"
                ))
            })?;
        Ok(Some(Self {
            signature: text(PIN_SIGNATURE_KEY)?,
            name: text(PIN_NAME_KEY)?,
            context: text(PIN_CONTEXT_KEY)?,
            frame,
            tolerance,
        }))
    }

    /// Put the pin into a request.
    pub fn write(&self, params: &mut Map<String, Value>) {
        params.insert(PIN_SIGNATURE_KEY.into(), self.signature.clone().into());
        params.insert(PIN_NAME_KEY.into(), self.name.clone().into());
        params.insert(PIN_CONTEXT_KEY.into(), self.context.clone().into());
        params.insert(
            PIN_FRAME_KEY.into(),
            json!({ "x": self.frame.x, "y": self.frame.y,
                    "width": self.frame.width, "height": self.frame.height }),
        );
        params.insert(PIN_TOLERANCE_KEY.into(), self.tolerance.into());
    }

    /// Whether the element found now is the one the mark was drawn on. Words
    /// or a context the element does not have are the empty word.
    #[must_use]
    pub fn holds(
        &self,
        signature: Option<&str>,
        name: Option<&str>,
        context: Option<&str>,
        frame: Option<Rect>,
    ) -> bool {
        signature == Some(self.signature.as_str())
            && name.unwrap_or_default() == self.name
            && context.unwrap_or_default() == self.context
            && frame.is_some_and(|frame| frame.matches_within(&self.frame, self.tolerance))
    }
}

/// The refusal a pin that no longer holds answers with.
#[must_use]
pub fn pin_broken(index: usize) -> ProviderError {
    ProviderError::new(
        error_code::ELEMENT_NOT_FOUND,
        format!(
            "element {index} is no longer the control its mark was drawn on (it moved or changed since that look); look again with --marks"
        ),
    )
}

/// The window-list parameter that asks for every on-screen window at every
/// layer — menus, panels, the Dock — with its layer and alpha: what covers a
/// window is not only other windows.
pub const EVERY_LAYER_KEY: &str = "everyLayer";
/// The row word for an overlay: a window above the document layer that
/// spans a whole display. The Dock's is one on macOS 26 — its window covers
/// the screen at the Dock's layer and is clear everywhere but the Dock, so a
/// click falls through it and the windows under it show. An overlay hides
/// nothing; the helper decides which windows are ones
/// (`DesktopOverlay.isOverlay`), and its own-window refusal skips them too.
pub const OVERLAY_KEY: &str = "overlay";

/// One `listAllWindows` row: where a window sits, whose it is, whether it is
/// ZeroCode's own (the helper says so: `own`), and — asked `everyLayer` — its
/// layer (0 is a document window), how opaque it is, and whether it is an
/// overlay ([`OVERLAY_KEY`]).
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopWindow {
    pub id: u64,
    pub pid: i64,
    pub app: String,
    pub rect: Rect,
    pub own: bool,
    pub layer: i64,
    pub alpha: f64,
    pub overlay: bool,
}

impl DesktopWindow {
    #[must_use]
    pub fn from_row(row: &Value) -> Option<Self> {
        let number = |key: &str| row.get(key).and_then(Value::as_f64);
        let app = row.get("app")?;
        Some(Self {
            id: row.get("id")?.as_u64()?,
            pid: app.get("pid")?.as_i64()?,
            app: app
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            rect: Rect::new(
                number("x")?,
                number("y")?,
                number("width")?,
                number("height")?,
            ),
            own: row.get("own").and_then(Value::as_bool).unwrap_or(false),
            layer: row.get("layer").and_then(Value::as_i64).unwrap_or(0),
            alpha: number("alpha").unwrap_or(1.0),
            overlay: row
                .get(OVERLAY_KEY)
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Whether the window hides what is under it: seen at all, and not an
    /// overlay.
    #[must_use]
    pub fn covers(&self) -> bool {
        self.alpha > 0.0 && !self.overlay
    }
}

/// The window a desktop look marks: the first, front to back, that is a
/// document window (layer 0), not ZeroCode's own, and seen on the picture —
/// some of it on the picture and not under the windows in front of it. A
/// window hidden whole (ZeroCode's own filling the screen) is not walked.
#[must_use]
pub fn desktop_target(windows: &[DesktopWindow], picture: Rect) -> Option<usize> {
    windows.iter().enumerate().position(|(at, window)| {
        window.layer == 0
            && !window.own
            && window.rect.intersection(&picture).is_some_and(|shown| {
                shown.area() > 0.0 && !fully_covered(shown, &occluders(windows, at))
            })
    })
}

/// What covers the target: every window in front of it that hides what is
/// under it — a menu, a panel, ZeroCode's own; not an overlay.
#[must_use]
pub fn occluders(windows: &[DesktopWindow], target: usize) -> Vec<Rect> {
    windows[..target.min(windows.len())]
        .iter()
        .filter(|window| window.covers())
        .map(|window| window.rect)
        .collect()
}

/// Whether the windows from the front down to the target stood still
/// between two lists — one before the picture, one after the walk: the same
/// windows, in the same order, each where it was within the tolerance.
#[must_use]
pub fn stood_still(
    before: &[DesktopWindow],
    after: &[DesktopWindow],
    target: u64,
    tolerance: f64,
) -> bool {
    let front = |list: &[DesktopWindow]| {
        list.iter()
            .position(|window| window.id == target)
            .map(|at| list[..=at].to_vec())
    };
    match (front(before), front(after)) {
        (Some(before), Some(after)) => {
            before.len() == after.len()
                && before.iter().zip(&after).all(|(then, now)| {
                    then.id == now.id && then.rect.matches_within(&now.rect, tolerance)
                })
        }
        _ => false,
    }
}

/// Where a badge sits against its control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkAnchor {
    InsideTopLeft,
    InsideTopRight,
    InsideTopCenter,
    OutsideAboveLeft,
    OutsideLeft,
    OutsideRightTop,
    OutsideBelowLeft,
}

impl MarkAnchor {
    /// The badge's rectangle, in picture pixels, beside `element`.
    #[must_use]
    pub fn place(self, element: &Rect, (width, height): (f64, f64)) -> Rect {
        let (x, y) = match self {
            Self::InsideTopLeft => (element.x, element.y),
            Self::InsideTopRight => (element.max_x() - width, element.y),
            Self::InsideTopCenter => (element.mid_x() - width / 2.0, element.y),
            Self::OutsideAboveLeft => (element.x, element.y - height),
            Self::OutsideLeft => (element.x - width, element.y),
            Self::OutsideRightTop => (element.max_x(), element.y),
            Self::OutsideBelowLeft => (element.x, element.max_y()),
        };
        Rect::new(x, y, width, height)
    }
}

/// How many digits a mark's number has.
#[must_use]
pub fn digits(mark: usize) -> Vec<usize> {
    mark.to_string()
        .chars()
        .filter_map(|digit| digit.to_digit(10).map(|value| value as usize))
        .collect()
}

/// A badge's size in picture pixels for `mark`: its digits at the glyph
/// scale, the gap between them, the pad and the border — nothing else.
#[must_use]
pub fn badge_size(mark: usize) -> (u32, u32) {
    let count = u32::try_from(digits(mark).len()).unwrap_or(1).max(1);
    let edge = 2 * (MARK_BADGE_PAD_PX + MARK_BADGE_BORDER_PX);
    let glyph_width = MARK_GLYPH_COLUMNS * MARK_GLYPH_SCALE;
    let width = edge + count * glyph_width + (count - 1) * MARK_GLYPH_GAP_PX;
    let height = edge + MARK_GLYPH_ROWS * MARK_GLYPH_SCALE;
    (width, height)
}

/// What a plan is made from: one window's faces and where that window and
/// its picture are.
#[derive(Debug, Clone)]
pub struct MarkInput<'a> {
    pub faces: &'a [ElementFace],
    /// The bounds the faces' frames are relative to, in screen points.
    pub window: Rect,
    /// Where the picture sits on the screen.
    pub frame: ShotFrame,
    /// The picture's size in pixels.
    pub picture: (u32, u32),
    /// Screen rectangles in front of the window: what lies under them is not
    /// seen, and so not marked.
    pub occluders: Vec<Rect>,
}

/// One mark as drawn: its number, the element it names, and where both are.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedMark {
    pub mark: usize,
    pub element_index: usize,
    /// The role word the legend shows ("button", "text field").
    pub role: String,
    pub label: Option<String>,
    pub screen: Rect,
    pub local: Rect,
    pub signature: String,
    /// The words the pin holds to: the name, else the placeholder ("" when
    /// the element has neither).
    pub name: String,
    /// The nearest named element above it ("" when none).
    pub context: String,
    pub element_px: Rect,
    pub badge_px: Rect,
}

impl PlacedMark {
    /// The identity and geometry this mark must prove at the press boundary.
    #[must_use]
    pub fn pin(&self, tolerance: f64) -> Pin {
        Pin {
            signature: self.signature.clone(),
            name: self.name.clone(),
            context: self.context.clone(),
            frame: self.local,
            tolerance,
        }
    }
}

/// A look's marks: the ones placed, how many controls qualified, and how many
/// of those got no badge (no free place, or past the cap).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MarkPlan {
    pub marks: Vec<PlacedMark>,
    pub candidates: usize,
    pub omitted: usize,
}

fn presses(face: &ElementFace) -> bool {
    face.actions.iter().any(|action| {
        let bare = render::bare_action(action);
        MACOS_PRESS_ACTIONS
            .iter()
            .any(|press| render::bare_action(press) == bare)
            || WINDOWS_PRESS_PATTERNS.contains(&bare)
    })
}

fn role_rank(face: &ElementFace) -> usize {
    MARK_ROLES
        .iter()
        .position(|role| *role == face.role)
        .unwrap_or(MARK_ROLES.len())
}

/// The words a mark is known by in the legend: the element's name, or its
/// placeholder; a link's markdown name gives back its words.
#[must_use]
pub fn label_of(face: &ElementFace) -> Option<String> {
    let name = face.name.as_deref().map(|name| {
        if face.role == "AXLink"
            && let Some(rest) = name.strip_prefix('[')
            && let Some((text, _url)) = rest.rsplit_once("](")
        {
            return text
                .replace("\\[", "[")
                .replace("\\]", "]")
                .replace("\\\\", "\\");
        }
        name.to_string()
    });
    name.or_else(|| face.placeholder.clone())
        .filter(|label| !label.trim().is_empty())
        .map(|label| render::preview(&label, MARK_LABEL_MAX_CHARS))
}

struct Candidate<'a> {
    face: &'a ElementFace,
    screen: Rect,
    px: Rect,
}

/// Whether role, state and size qualify a face as a possible mark. Visibility,
/// clipping and the provider's additional proof are checked by the plan.
#[must_use]
pub fn is_mark_candidate(face: &ElementFace, window: Rect) -> bool {
    face.width >= MARK_MIN_SIDE_POINTS
        && face.height >= MARK_MIN_SIDE_POINTS
        && !face
            .traits
            .iter()
            .any(|word| MARK_SKIP_TRAITS.contains(&word.as_str()))
        && !MARK_NEVER_ROLES.contains(&face.role.as_str())
        && (MARK_ROLES.contains(&face.role.as_str()) || presses(face))
        && (TEXT_ENTRY_ROLES.contains(&face.role.as_str())
            || face.local().area() <= MARK_MAX_WINDOW_SHARE * window.area())
}

/// Choose, number and place a window's marks. Pure and deterministic: the
/// same faces and frame always give the same numbers in the same places.
#[must_use]
pub fn plan(input: &MarkInput<'_>) -> MarkPlan {
    plan_where(input, |_| true)
}

/// The shared plan with an additional provider eligibility check. All faces
/// remain in the identity census, including ones this check refuses: hiding
/// an uncertain face must not make its look-alike appear unique.
#[must_use]
pub fn plan_where(input: &MarkInput<'_>, may_mark: impl Fn(&ElementFace) -> bool) -> MarkPlan {
    let frame = input.frame;
    let picture_px = Rect::new(
        0.0,
        0.0,
        f64::from(input.picture.0),
        f64::from(input.picture.1),
    );
    let picture_screen = {
        let [x, y] = frame.to_point([0.0, 0.0]);
        Rect::new(
            x,
            y,
            frame.length_to_point(picture_px.width),
            frame.length_to_point(picture_px.height),
        )
    };
    // 1. Select what a person could press on this picture.
    let mut chosen: Vec<Candidate<'_>> = input
        .faces
        .iter()
        .filter(|face| is_mark_candidate(face, input.window) && may_mark(face))
        .filter_map(|face| {
            let local = face.local();
            let screen = Rect::new(
                input.window.x + local.x,
                input.window.y + local.y,
                local.width,
                local.height,
            );
            let (cx, cy) = (screen.mid_x(), screen.mid_y());
            // The centre is where a press lands: it must be on the element's
            // visible part, in the window, on the picture, under nothing.
            let seen = face.seen().area() > 0.0
                && face.seen().contains_point(local.mid_x(), local.mid_y())
                && input.window.contains_point(cx, cy)
                && picture_screen.contains_point(cx, cy)
                && !input
                    .occluders
                    .iter()
                    .any(|cover| cover.contains_point(cx, cy));
            seen.then(|| {
                let [px, py] = frame.to_pixel([screen.x, screen.y]);
                Candidate {
                    face,
                    screen,
                    px: Rect::new(
                        px,
                        py,
                        frame.length_to_pixel(screen.width),
                        frame.length_to_pixel(screen.height),
                    ),
                }
            })
        })
        .collect();
    // A control its pin could not tell from another anywhere in the window —
    // the same identity, words and place in the tree, told apart by its frame
    // alone, even one scrolled out of sight — is left out: one slot's shift
    // would put the other under its number.
    let mut identities: BTreeMap<(&str, &str, &str), usize> = BTreeMap::new();
    for face in input.faces {
        *identities.entry(face.identity()).or_default() += 1;
    }
    chosen.retain(|candidate| identities[&candidate.face.identity()] == 1);
    // 2. One target, one mark: the better role keeps it.
    chosen.sort_by_key(|candidate| (role_rank(candidate.face), candidate.face.index));
    let mut kept: Vec<Candidate<'_>> = Vec::new();
    for candidate in chosen {
        if kept
            .iter()
            .all(|held| held.screen.iou(&candidate.screen) < MARK_SAME_TARGET_IOU)
        {
            kept.push(candidate);
        }
    }
    // 3. Reading order, in the picture: lines top to bottom, each left to right.
    let line_height = f64::from(badge_size(1).1);
    kept.sort_by(|a, b| {
        a.px.y
            .total_cmp(&b.px.y)
            .then(a.face.index.cmp(&b.face.index))
    });
    let mut ordered: Vec<(usize, Candidate<'_>)> = Vec::with_capacity(kept.len());
    let mut line = 0;
    let mut line_top: Option<f64> = None;
    for candidate in kept {
        if line_top.is_some_and(|top| candidate.px.y - top > line_height) {
            line += 1;
            line_top = None;
        }
        line_top.get_or_insert(candidate.px.y);
        ordered.push((line, candidate));
    }
    ordered.sort_by(|(line_a, a), (line_b, b)| {
        line_a
            .cmp(line_b)
            .then(a.px.x.total_cmp(&b.px.x))
            .then(a.face.index.cmp(&b.face.index))
    });
    // 4. Place greedily: the first free anchor, or no badge.
    let candidates = ordered.len();
    let mut marks: Vec<PlacedMark> = Vec::new();
    let elements: Vec<Rect> = ordered.iter().map(|(_, candidate)| candidate.px).collect();
    for (at, (_, candidate)) in ordered.iter().enumerate() {
        if marks.len() == MARK_CAP {
            break;
        }
        let number = marks.len() + 1;
        let (width, height) = badge_size(number);
        let size = (f64::from(width), f64::from(height));
        let own = candidate.px;
        let fits_inside = size.0 <= own.width
            && size.1 <= own.height
            && size.0 * size.1 <= MARK_INSIDE_MAX_SHARE * own.area();
        let anchors = if fits_inside {
            MARK_ANCHORS_INSIDE_FIRST
        } else {
            MARK_ANCHORS_OUTSIDE_FIRST
        };
        let placed = anchors
            .iter()
            .map(|anchor| anchor.place(&own, size))
            .find(|badge| {
                picture_px.contains_rect(badge)
                    && marks.iter().all(|mark| !mark.badge_px.intersects(badge))
                    && elements.iter().enumerate().all(|(other, rect)| {
                        other == at || !rect.intersects(badge) || rect.contains_rect(&own)
                    })
            });
        let Some(badge_px) = placed else {
            continue;
        };
        let face = candidate.face;
        marks.push(PlacedMark {
            mark: number,
            element_index: face.index,
            role: render::role_text(&RenderNode::with_role(&face.role)),
            label: label_of(face),
            screen: candidate.screen,
            local: face.local(),
            signature: face.signature.clone(),
            name: face.words().to_string(),
            context: face.context.clone().unwrap_or_default(),
            element_px: own,
            badge_px,
        });
    }
    MarkPlan {
        omitted: candidates - marks.len(),
        marks,
        candidates,
    }
}

/// Which window a marked look's numbers belong to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkedWindow {
    pub look_id: String,
    pub app: String,
    pub pid: i64,
    pub window_id: u64,
}

/// A marked look's answer, in screen points (`ShotFrame::pixelize` speaks it
/// in the picture's pixels with no code of its own).
#[must_use]
pub fn answer(plan: &MarkPlan, window: &MarkedWindow) -> Value {
    json!({
        SPACE_KEY: SCREEN_SPACE,
        LOOK_ID_KEY: window.look_id,
        "app": window.app,
        "pid": window.pid,
        "windowId": window.window_id,
        ITEMS_KEY: items(plan),
        "candidates": plan.candidates,
        "omitted": plan.omitted,
    })
}

/// The tool's geometry for each number, shared by desktop and mobile looks.
#[must_use]
pub fn items(plan: &MarkPlan) -> Vec<Value> {
    plan.marks
        .iter()
        .map(|mark| {
            json!({
                "mark": mark.mark,
                "elementIndex": mark.element_index,
                "role": mark.role,
                "label": mark.label,
                "x": mark.screen.x,
                "y": mark.screen.y,
                "width": mark.screen.width,
                "height": mark.screen.height,
                "centerX": mark.screen.mid_x(),
                "centerY": mark.screen.mid_y(),
            })
        })
        .collect()
}

/// One legend line: `7 button Save @412,88` — the number, the role, the
/// words when it has any, and its centre in whatever space the item speaks.
#[must_use]
pub fn legend_line(item: &Value) -> Option<String> {
    let mark = item.get("mark")?.as_u64()?;
    let role = item.get("role").and_then(Value::as_str).unwrap_or_default();
    let x = item.get("centerX")?.as_f64()?.round();
    let y = item.get("centerY")?.as_f64()?.round();
    let label = item
        .get("label")
        .and_then(Value::as_str)
        .filter(|label| !label.is_empty())
        .map(|label| format!(" {label}"))
        .unwrap_or_default();
    let role = if role.is_empty() {
        String::new()
    } else {
        format!(" {role}")
    };
    Some(format!("{mark}{role}{label} @{x},{y}"))
}

#[cfg(test)]
mod tests;
