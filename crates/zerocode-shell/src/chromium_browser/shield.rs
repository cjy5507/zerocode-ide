//! The shield over a watched Chromium pane (t-66513).
//!
//! While the person watches an agent's work browser live, the pane itself is
//! shown — the page's own pixels at the page's own frame rate, no snapshot —
//! and this view stands over the pane's frame. It takes every click, scroll,
//! drag and gesture there and does nothing with them, never takes the
//! keyboard, and draws what the person needs to read the pane: a border in the
//! agent's working colour, a tag that says the agent is working and the pane
//! is for looking only, and the agent's last press.
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
/// The press mark's ring and dot radii, in points.
const RING: f64 = 12.0;
const DOT: f64 = 4.0;
/// The most characters of the tag the window may send.
const SAYS_CAP: usize = 80;

/// What the window dresses the shield in (t-66513): the words its tag says,
/// in the person's language, and two colours read off the window's design
/// tokens — the agent's working colour (`--signal-ready`) for the border, the
/// tag and the press mark, and the ground's (`--surface-abyss`) for the tag's
/// words and the mark's rim — so no colour is written down twice.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Dress {
    says: String,
    tint: [u8; 3],
    ink: [u8; 3],
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
        stroke(&tint, oval_or_rect(edge, false), BORDER);
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
            let ring = square(x, y, RING);
            stroke(&ink, oval_or_rect(ring, true), BORDER * 2.0);
            stroke(&tint, oval_or_rect(ring, true), BORDER);
            let dot = oval_or_rect(square(x, y, DOT), true);
            tint.setFill();
            // SAFETY: an NSBezierPath answers `fill`.
            let _: () = unsafe { msg_send![&*dot, fill] };
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

fn square(x: f64, y: f64, radius: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(x - radius, y - radius),
        NSSize::new(radius * 2.0, radius * 2.0),
    )
}

/// An NSBezierPath round the rectangle, or the oval inside it.
fn oval_or_rect(rect: NSRect, oval: bool) -> Retained<AnyObject> {
    // SAFETY: both are NSBezierPath's own constructors, taking an NSRect.
    unsafe {
        if oval {
            msg_send![class!(NSBezierPath), bezierPathWithOvalInRect: rect]
        } else {
            msg_send![class!(NSBezierPath), bezierPathWithRect: rect]
        }
    }
}

fn stroke(color: &NSColor, path: Retained<AnyObject>, width: f64) {
    color.setStroke();
    // SAFETY: an NSBezierPath answers `setLineWidth:` and `stroke`.
    unsafe {
        let _: () = msg_send![&*path, setLineWidth: width];
        let _: () = msg_send![&*path, stroke];
    }
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
        };
        assert!(dress("에이전트 조작 중 · 보기만").checked().is_ok());
        assert!(dress("").checked().is_ok());
        assert!(dress(&"가".repeat(SAYS_CAP + 1)).checked().is_err());
        assert!(dress("줄\n바꿈").checked().is_err());
    }
}
