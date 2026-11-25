//! The shield over a watched Chromium pane (t-66513).
//!
//! While the person watches an agent's work browser live, the pane itself is
//! shown — the page's own pixels at the page's own frame rate, no snapshot —
//! and this view stands over the pane's frame. It takes every click, scroll,
//! drag and gesture there and does nothing with them, never takes the
//! keyboard, and draws what the person needs to read the pane: a border in the
//! agent's working colour, a tag that says the agent is working and the pane
//! is for looking only, and the agent's pointer where it last pressed.
//!
//! Why a view of our own, not a switch on the pane: a windowed CEF browser
//! takes the window's events by AppKit's own hit test and has no setting that
//! turns its input off while it keeps drawing. A view above it wins that hit
//! test, so no press reaches it — and Chromium's own view drops a mouse event,
//! a move included, that the window's hit test does not give to it
//! (`shouldIgnoreMouseEvent:` in its NSView, present in the framework this app
//! ships), so a mouse moving over the shield is no hover in the page either.
//! The keyboard never goes there because the shield refuses to take it and the
//! placement hands it back to the window's own webview.
//!
//! Every function here runs on the main thread, inside the pane's own native
//! operations; the drawing happens only when something it shows changed.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, class, define_class, msg_send};
use objc2_app_kit::{NSColor, NSEvent, NSView, NSWindowOrderingMode};
use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};
use serde::Deserialize;

/// The border's width, in points.
const BORDER: f64 = 2.0;
/// The tag's distance from the pane's corner, and its padding across.
const TAG_INSET: f64 = 8.0;
/// The tag's padding up and down.
const TAG_PAD: f64 = 4.0;
/// The tag's words, in points — the type scale's smallest step.
const TAG_TYPE: f64 = 11.0;
/// The agent's pointer where it last pressed (2026-11-25; it was a ring and a
/// dot in the working colour, which the person read as a green circle): a
/// dart with no tail and round corners inside a white rim, over a soft edge —
/// the person's own pointer has a tail, so the two are told apart by shape as
/// well as by colour, and the white has an outline on a white page.
///
/// Its two edges leave the tip at these angles below the horizontal and run
/// this long, in points; the notch in its back is cut this far along the line
/// from the tip to the middle of the back (1 is no notch at all).
const WING_DEGREES: (f64, f64) = (26.0, 72.0);
const WING: f64 = 22.0;
const NOTCH: f64 = 0.8;
/// The body is filled and then stroked in its own colour with a round join, so
/// every corner is a circle of half this width; the rim is this much white all
/// round it.
const BODY: f64 = 3.0;
const RIM: f64 = 1.5;
/// How far the rim's outermost point stands off the body's corner.
const REACH: f64 = BODY / 2.0 + RIM;
/// The soft edge under the rim: its blur, how far it falls down the screen,
/// and how dark it is.
const SHADOW_BLUR: f64 = 3.5;
const SHADOW_DROP: f64 = 1.5;
const SHADOW_ALPHA: f64 = 0.3;
/// NSLineJoinStyle's round join.
const ROUND_JOIN: usize = 1;
/// The most characters of the tag the window may send.
const SAYS_CAP: usize = 80;

/// What the window dresses the shield in (t-66513): the words its tag says,
/// in the person's language, and four colours read off the window's design
/// tokens, so none is written down twice — the agent's working colour
/// (`--signal-ready`) for the border and the tag, the ground's
/// (`--surface-abyss`) for the tag's words, and for the dart where the agent
/// last pressed the agent's pointer (`--agent-pointer`, a colour no state
/// wears) inside the white of its rim (`--ink-on-solid`).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Dress {
    says: String,
    tint: [u8; 3],
    ink: [u8; 3],
    pointer: [u8; 3],
    rim: [u8; 3],
}

impl Dress {
    pub(crate) fn checked(self) -> Result<Self, String> {
        if self.says.chars().count() > SAYS_CAP || self.says.chars().any(char::is_control) {
            return Err("workspace_watch_invalid: 관전 표시 문구가 올바르지 않습니다".to_string());
        }
        Ok(self)
    }
}

/// The agent's last press: the pressed element's rectangle in the page's CSS
/// pixels, with the page's device ratio — what a click's answer says.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Press {
    rect: [f64; 4],
    dpr: f64,
}

/// Where a press lands on the pane, in the pane's points from its top left:
/// the pressed element's centre, its CSS pixels carried by the page's zoom —
/// the device ratio over the screen's backing scale. None for a press the
/// page measured nothing for.
fn press_point(rect: [f64; 4], dpr: f64, backing: f64) -> Option<(f64, f64)> {
    let [x, y, width, height] = rect;
    let measured = rect.iter().all(|side| side.is_finite())
        && [dpr, backing]
            .iter()
            .all(|ratio| ratio.is_finite() && *ratio > 0.0);
    if !measured {
        return None;
    }
    let zoom = dpr / backing;
    Some(((x + width / 2.0) * zoom, (y + height / 2.0) * zoom))
}

/// The way a dart lies from its tip: 1 towards the right or down the pane, -1
/// the other way.
type Lies = (f64, f64);

fn unit(degrees: f64) -> (f64, f64) {
    let radians = degrees.to_radians();
    (radians.cos(), radians.sin())
}

/// From the dart's tip towards the middle of its back.
fn dart_axis() -> (f64, f64) {
    let (right, down) = (unit(WING_DEGREES.0), unit(WING_DEGREES.1));
    let (x, y) = (right.0 + down.0, right.1 + down.1);
    let length = x.hypot(y);
    (x / length, y / length)
}

/// The body's four corners with the tip of its rim on `tip`: the corner at the
/// tip, the right wing's end, the notch and the lower wing's end. The corner
/// at the tip stands `REACH` behind the tip along the axis, so the rim's round
/// corner ends exactly on the point pressed. Where `lies` is -1 the dart is
/// turned back across its tip.
fn dart_corners(tip: (f64, f64), lies: Lies) -> [(f64, f64); 4] {
    let axis = dart_axis();
    let (right, down) = (unit(WING_DEGREES.0), unit(WING_DEGREES.1));
    let corner = (axis.0 * REACH, axis.1 * REACH);
    let right_end = (corner.0 + right.0 * WING, corner.1 + right.1 * WING);
    let down_end = (corner.0 + down.0 * WING, corner.1 + down.1 * WING);
    let middle = (
        (right_end.0 + down_end.0) / 2.0,
        (right_end.1 + down_end.1) / 2.0,
    );
    let cut = (
        corner.0 + (middle.0 - corner.0) * NOTCH,
        corner.1 + (middle.1 - corner.1) * NOTCH,
    );
    [corner, right_end, cut, down_end].map(|(x, y)| (tip.0 + x * lies.0, tip.1 + y * lies.1))
}

/// How far the dart reaches from its tip, across and down, its rim and its
/// soft edge included.
fn dart_room() -> (f64, f64) {
    let corners = dart_corners((0.0, 0.0), (1.0, 1.0));
    let across = corners.iter().map(|corner| corner.0).fold(0.0, f64::max);
    let down = corners.iter().map(|corner| corner.1).fold(0.0, f64::max);
    (
        across + REACH + SHADOW_BLUR,
        down + REACH + SHADOW_BLUR + SHADOW_DROP,
    )
}

/// Which way the dart lies for a press at `point` on a pane of `size`: to the
/// right of its tip and below it, and turned back across the tip on a side
/// where the pane has no room for it — a press by the right or the lower edge
/// would otherwise show nothing of its pointer. A pane with room on neither
/// side keeps the usual way.
fn dart_lies(point: (f64, f64), size: (f64, f64)) -> Lies {
    let room = dart_room();
    let side = |at: f64, room: f64, size: f64| {
        if at + room > size && at - room >= 0.0 {
            -1.0
        } else {
            1.0
        }
    };
    (side(point.0, room.0, size.0), side(point.1, room.1, size.1))
}

#[derive(Default)]
struct Look {
    dress: Option<Dress>,
    press: Option<Press>,
}

struct Ivars {
    look: RefCell<Look>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond its designated
    //   initializer, which `ShieldView::new` calls.
    // - `ShieldView` does not implement `Drop`.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZerocodeBrowserShield"]
    #[ivars = Ivars]
    struct ShieldView;

    impl ShieldView {
        /// The page's own way up: y grows downwards, as a press's does.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Never the keyboard's: a key stays with the window's own webview.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            false
        }

        // Every press, drag, scroll and gesture over the pane ends here.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {}

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, _event: &NSEvent) {}

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, _event: &NSEvent) {}

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, _event: &NSEvent) {}

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, _event: &NSEvent) {}

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, _event: &NSEvent) {}

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, _event: &NSEvent) {}

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, _event: &NSEvent) {}

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, _event: &NSEvent) {}

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, _event: &NSEvent) {}

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, _event: &NSEvent) {}

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify(&self, _event: &NSEvent) {}

        #[unsafe(method(smartMagnifyWithEvent:))]
        fn smart_magnify(&self, _event: &NSEvent) {}

        #[unsafe(method(rotateWithEvent:))]
        fn rotate(&self, _event: &NSEvent) {}

        #[unsafe(method(swipeWithEvent:))]
        fn swipe(&self, _event: &NSEvent) {}

        #[unsafe(method(pressureChangeWithEvent:))]
        fn pressure_change(&self, _event: &NSEvent) {}

        #[unsafe(method(quickLookWithEvent:))]
        fn quick_look(&self, _event: &NSEvent) {}

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            self.draw();
        }
    }
);

impl ShieldView {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            look: RefCell::new(Look::default()),
        });
        // SAFETY: `initWithFrame:` is NSView's designated initializer.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn draw(&self) {
        let look = self.ivars().look.borrow();
        let Some(dress) = &look.dress else {
            return;
        };
        let bounds = self.bounds();
        let tint = color(dress.tint);
        let ink = color(dress.ink);
        let edge = inset(bounds, BORDER / 2.0);
        stroke(&tint, &rect_path(edge), BORDER);
        draw_tag(&dress.says, &tint, &ink);
        let backing = self
            .window()
            .map_or(1.0, |window| window.backingScaleFactor());
        if let Some((x, y)) = look
            .press
            .and_then(|press| press_point(press.rect, press.dpr, backing))
        {
            let x = x.clamp(0.0, bounds.size.width);
            let y = y.clamp(0.0, bounds.size.height);
            let lies = dart_lies((x, y), (bounds.size.width, bounds.size.height));
            draw_dart((x, y), lies, &color(dress.pointer), &color(dress.rim));
        }
    }
}

fn color([red, green, blue]: [u8; 3]) -> Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(red) / 255.0,
        f64::from(green) / 255.0,
        f64::from(blue) / 255.0,
        1.0,
    )
}

fn inset(rect: NSRect, by: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.origin.x + by, rect.origin.y + by),
        NSSize::new(
            (rect.size.width - by * 2.0).max(0.0),
            (rect.size.height - by * 2.0).max(0.0),
        ),
    )
}

/// An NSBezierPath round the rectangle.
fn rect_path(rect: NSRect) -> Retained<AnyObject> {
    // SAFETY: NSBezierPath's own constructor, taking an NSRect.
    unsafe { msg_send![class!(NSBezierPath), bezierPathWithRect: rect] }
}

fn stroke(color: &NSColor, path: &AnyObject, width: f64) {
    color.setStroke();
    // SAFETY: an NSBezierPath answers `setLineWidth:` and `stroke`.
    unsafe {
        let _: () = msg_send![path, setLineWidth: width];
        let _: () = msg_send![path, stroke];
    }
}

fn fill(color: &NSColor, path: &AnyObject) {
    color.setFill();
    // SAFETY: an NSBezierPath answers `fill`.
    let _: () = unsafe { msg_send![path, fill] };
}

/// The agent's pointer with the tip of its rim on `tip`: the soft edge, the
/// rim, then the body. The body's outline is filled and stroked with a round
/// join, and the rim is the same outline stroked wider.
fn draw_dart(tip: (f64, f64), lies: Lies, body: &NSColor, rim: &NSColor) {
    let corners = dart_corners(tip, lies);
    // SAFETY: NSBezierPath's own constructor and its path verbs, each taking an
    // NSPoint; `setLineJoinStyle:` takes an NSLineJoinStyle, an NSUInteger.
    let outline: Retained<AnyObject> = unsafe {
        let outline: Retained<AnyObject> = msg_send![class!(NSBezierPath), bezierPath];
        let _: () = msg_send![&*outline, moveToPoint: NSPoint::new(corners[0].0, corners[0].1)];
        for corner in &corners[1..] {
            let _: () = msg_send![&*outline, lineToPoint: NSPoint::new(corner.0, corner.1)];
        }
        let _: () = msg_send![&*outline, closePath];
        let _: () = msg_send![&*outline, setLineJoinStyle: ROUND_JOIN];
        outline
    };
    let rimmed = BODY + RIM * 2.0;
    // The soft edge is cast once, by the rim's band alone: the fill inside it
    // would cast a second one over the first. A shadow's offset is in the
    // window's own space, y up, whichever way the view is flipped — down the
    // screen is negative.
    let soft = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, SHADOW_ALPHA);
    // SAFETY: NSGraphicsContext's class methods save and restore the state of
    // the context this view is drawing in; an NSShadow answers its three
    // setters (a colour, a CGFloat, an NSSize) and `set`.
    unsafe {
        let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
        let shadow: Retained<AnyObject> = msg_send![class!(NSShadow), new];
        let _: () = msg_send![&*shadow, setShadowColor: &*soft];
        let _: () = msg_send![&*shadow, setShadowBlurRadius: SHADOW_BLUR];
        let _: () = msg_send![&*shadow, setShadowOffset: NSSize::new(0.0, -SHADOW_DROP)];
        let _: () = msg_send![&*shadow, set];
    }
    stroke(rim, &outline, rimmed);
    // SAFETY: as above; it ends the state the soft edge was set in.
    let _: () = unsafe { msg_send![class!(NSGraphicsContext), restoreGraphicsState] };
    fill(rim, &outline);
    stroke(rim, &outline, rimmed);
    fill(body, &outline);
    stroke(body, &outline, BODY);
}

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    #[link_name = "NSFontAttributeName"]
    static FONT_ATTRIBUTE: &'static NSString;
    #[link_name = "NSForegroundColorAttributeName"]
    static COLOR_ATTRIBUTE: &'static NSString;
}

/// The tag in the pane's top left corner: the window's words on the agent's
/// working colour.
fn draw_tag(says: &str, tint: &NSColor, ink: &NSColor) {
    if says.is_empty() {
        return;
    }
    let words = NSString::from_str(says);
    // SAFETY: NSFont's own constructor; the attribute names are AppKit's
    // constants; NSString answers `sizeWithAttributes:` and
    // `drawAtPoint:withAttributes:` (AppKit's string drawing) on the main
    // thread, inside the view's drawing.
    unsafe {
        let font: Retained<AnyObject> = msg_send![class!(NSFont), boldSystemFontOfSize: TAG_TYPE];
        let ink: &AnyObject = ink;
        let attributes = NSDictionary::<NSString, AnyObject>::from_slices(
            &[FONT_ATTRIBUTE, COLOR_ATTRIBUTE],
            &[&*font, ink],
        );
        let size: NSSize = msg_send![&*words, sizeWithAttributes: &*attributes];
        let tag = NSRect::new(
            NSPoint::new(TAG_INSET, TAG_INSET),
            NSSize::new(size.width + TAG_INSET * 2.0, size.height + TAG_PAD * 2.0),
        );
        tint.setFill();
        let ground: Retained<AnyObject> = msg_send![
            class!(NSBezierPath),
            bezierPathWithRoundedRect: tag,
            xRadius: TAG_PAD,
            yRadius: TAG_PAD
        ];
        let _: () = msg_send![&*ground, fill];
        let _: () = msg_send![
            &*words,
            drawAtPoint: NSPoint::new(TAG_INSET * 2.0, TAG_INSET + TAG_PAD),
            withAttributes: &*attributes
        ];
    }
}

/// The shield a Chromium pane keeps (t-66513): none until the person first
/// watches the pane, then one view, raised and lowered with each placement,
/// and gone with the pane.
#[derive(Default)]
pub(super) struct Shield {
    view: Option<Retained<ShieldView>>,
    /// The agent's last press, kept while the shield is down too: watching
    /// starts with the mark where the agent last pressed.
    press: Option<Press>,
}

impl Shield {
    /// Stand the shield over `pane`'s frame, dressed as the window says, and
    /// directly above it.
    pub(super) fn raise(&mut self, pane: &NSView, dress: &Dress) -> Result<(), String> {
        let parent = unsafe { pane.superview() }
            .ok_or_else(|| "Chromium parent view is unavailable".to_string())?;
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "the shield is raised on the main thread only".to_string())?;
        let frame = pane.frame();
        let view = self
            .view
            .get_or_insert_with(|| ShieldView::new(mtm, frame))
            .clone();
        let mut repaint = view.frame().size != frame.size;
        view.setFrame(frame);
        {
            let mut look = view.ivars().look.borrow_mut();
            if look.dress.as_ref() != Some(dress) {
                look.dress = Some(dress.clone());
                repaint = true;
            }
            if look.press != self.press {
                look.press = self.press;
                repaint = true;
            }
        }
        let placed = unsafe { view.superview() }.is_some_and(|held| std::ptr::eq(&*held, &*parent));
        if !placed || view.isHidden() {
            parent.addSubview_positioned_relativeTo(&view, NSWindowOrderingMode::Above, Some(pane));
        }
        view.setHidden(false);
        if repaint {
            view.setNeedsDisplay(true);
        }
        Ok(())
    }

    /// Take the shield down; it stays out of every hit test until raised.
    pub(super) fn lower(&mut self) {
        if let Some(view) = &self.view {
            view.setHidden(true);
        }
    }

    /// The agent pressed: the mark moves there, drawn now if the person is
    /// watching.
    pub(super) fn pressed(&mut self, rect: [f64; 4], dpr: f64) {
        self.press = Some(Press { rect, dpr });
        if let Some(view) = &self.view {
            view.ivars().look.borrow_mut().press = self.press;
            if !view.isHidden() {
                view.setNeedsDisplay(true);
            }
        }
    }
}

impl Drop for Shield {
    fn drop(&mut self) {
        if let Some(view) = self.view.take() {
            view.removeFromSuperview();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A press lands at the pressed element's centre, carried by the page's
    /// zoom: CSS pixels times the device ratio over the backing scale.
    #[test]
    fn a_press_lands_at_the_elements_centre_in_the_panes_points() {
        assert_eq!(
            press_point([100.0, 40.0, 80.0, 20.0], 2.0, 2.0),
            Some((140.0, 50.0))
        );
        assert_eq!(
            press_point([100.0, 40.0, 80.0, 20.0], 3.0, 2.0),
            Some((210.0, 75.0)),
            "a page zoomed to 150% carries its CSS pixels half again as far"
        );
        assert_eq!(
            press_point([10.0, 10.0, 0.0, 0.0], 1.0, 1.0),
            Some((10.0, 10.0))
        );
        assert_eq!(press_point([f64::NAN, 0.0, 1.0, 1.0], 2.0, 2.0), None);
        assert_eq!(press_point([0.0, 0.0, 1.0, 1.0], 0.0, 2.0), None);
        assert_eq!(press_point([0.0, 0.0, 1.0, 1.0], 2.0, 0.0), None);
    }

    /// The window's words are bounded: a tag of a sentence, no control
    /// characters.
    #[test]
    fn the_tag_takes_a_bounded_line_of_words() {
        let dress = |says: &str| Dress {
            says: says.to_string(),
            tint: [131, 230, 183],
            ink: [6, 9, 11],
            pointer: [157, 75, 216],
            rim: [255, 255, 255],
        };
        assert!(dress("에이전트 조작 중 · 보기만").checked().is_ok());
        assert!(dress("").checked().is_ok());
        assert!(dress(&"가".repeat(SAYS_CAP + 1)).checked().is_err());
        assert!(dress("줄\n바꿈").checked().is_err());
    }

    /// The window hands the shield the pointer's colour and its rim's with the
    /// rest of its dress: a dress without either is not one.
    #[test]
    fn the_dress_carries_the_pointers_colour_and_its_rims() {
        let whole = serde_json::json!({
            "says": "에이전트 조작 중 · 보기만", "tint": [98, 192, 115], "ink": [0, 0, 0],
            "pointer": [157, 75, 216], "rim": [255, 255, 255],
        });
        let dress: Dress = serde_json::from_value(whole.clone()).expect("a whole dress");
        assert_eq!(
            (dress.pointer, dress.rim),
            ([157, 75, 216], [255, 255, 255])
        );
        for missing in ["pointer", "rim"] {
            let mut short = whole.clone();
            short.as_object_mut().expect("an object").remove(missing);
            assert!(
                serde_json::from_value::<Dress>(short).is_err(),
                "a dress without `{missing}` was taken"
            );
        }
    }

    /// The rim's round corner ends on the point the agent pressed: the body's
    /// corner there stands the rim's reach behind the tip along the dart's own
    /// axis, and every other corner lies further along it, to the right of the
    /// tip and below it.
    #[test]
    fn the_darts_rim_ends_on_the_point_the_agent_pressed() {
        let tip = (140.0, 50.0);
        let axis = dart_axis();
        assert!((axis.0.hypot(axis.1) - 1.0).abs() < 1e-9);
        let corners = dart_corners(tip, (1.0, 1.0));
        let along = |corner: (f64, f64)| (corner.0 - tip.0) * axis.0 + (corner.1 - tip.1) * axis.1;
        let aside = |corner: (f64, f64)| (corner.0 - tip.0) * axis.1 - (corner.1 - tip.1) * axis.0;
        assert!(
            (along(corners[0]) - REACH).abs() < 1e-9 && aside(corners[0]).abs() < 1e-9,
            "the corner at the tip stands the rim's reach behind it, on the axis: {:?}",
            corners[0]
        );
        for corner in &corners[1..] {
            assert!(
                along(*corner) > REACH,
                "{corner:?} lies behind the tip's corner"
            );
            assert!(
                corner.0 > tip.0 && corner.1 > tip.1,
                "{corner:?} lies to the right of the tip and below it"
            );
        }
        // The two wings stand on either side of the axis, and the notch is cut
        // into the back between them, nearer the tip than either wing's end.
        assert!(aside(corners[1]) > 0.0 && aside(corners[3]) < 0.0);
        assert!(along(corners[2]) < along(corners[1]).min(along(corners[3])));
        assert!(aside(corners[2]).abs() < 1e-9, "the notch is on the axis");
    }

    /// A press by the right or the lower edge turns the dart back across its
    /// tip, so the pointer is drawn inside the pane wherever the press was.
    #[test]
    fn a_press_by_an_edge_turns_the_dart_into_the_pane() {
        let size = (640.0, 480.0);
        let room = dart_room();
        assert!(room.0 > WING && room.1 > WING, "{room:?}");
        assert_eq!(dart_lies((100.0, 100.0), size), (1.0, 1.0));
        assert_eq!(dart_lies((size.0 - 2.0, 100.0), size), (-1.0, 1.0));
        assert_eq!(dart_lies((100.0, size.1 - 2.0), size), (1.0, -1.0));
        assert_eq!(dart_lies(size, size), (-1.0, -1.0));
        assert_eq!(
            dart_lies((size.0 - room.0 - 0.5, size.1 - room.1 - 0.5), size),
            (1.0, 1.0),
            "with room enough it lies the usual way"
        );
        for point in [
            (0.0, 0.0),
            (size.0, 0.0),
            (0.0, size.1),
            size,
            (320.0, 240.0),
            (size.0 - 10.0, size.1 - 10.0),
        ] {
            for corner in dart_corners(point, dart_lies(point, size)) {
                assert!(
                    (0.0..=size.0).contains(&corner.0) && (0.0..=size.1).contains(&corner.1),
                    "a press at {point:?} draws a corner at {corner:?}, outside the pane"
                );
            }
        }
        assert_eq!(
            dart_lies((10.0, 10.0), (20.0, 20.0)),
            (1.0, 1.0),
            "a pane with room on neither side keeps the usual way"
        );
        // Turned, the dart is its own mirror image across the tip.
        let usual = dart_corners((0.0, 0.0), (1.0, 1.0));
        let turned = dart_corners((0.0, 0.0), (-1.0, -1.0));
        for (one, other) in usual.iter().zip(turned.iter()) {
            assert!((one.0 + other.0).abs() < 1e-9 && (one.1 + other.1).abs() < 1e-9);
        }
    }
}
