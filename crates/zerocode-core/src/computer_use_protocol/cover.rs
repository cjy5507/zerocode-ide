//! What covers the place a hand is about to press, told as a person would
//! see it — whose window it is, at what layer, where — and where the
//! target's own window could stand for that place to show (t-12979).
//!
//! A press whose place another window hides does not press: the helper
//! refuses a mark covered at its centre, and a coordinate press whose
//! recipient is another window stops before its first event. Until now that
//! refusal was the end, and a look taken again left the hidden control out of
//! its numbers, so a control a person could uncover in one move was, to the
//! agent, not there. The moves a person makes first are the ones that change
//! nothing but the target's own window — bring it to the front, move it clear
//! — and both can be undone; what covers it is never read, answered, moved or
//! closed by them.
//!
//! What a window is and what stands in front of it is [`super::marks`]'s
//! ([`in_front`]): the marks and this module read one answer. Nothing here
//! reads a title or what a window shows — only whose it is, its layer and its
//! bounds — and nothing touches an operating system: the window lists the
//! windows and moves them.

use super::marks::{DesktopWindow, in_front};
use super::render::Rect;

/// Whose a window in front of the target is, beside the target's own: the
/// relation, which is what a person reads off a window before its words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The target's own app — a panel, a sheet, a second window of it.
    SameApp,
    /// Another app's.
    OtherApp,
    /// ZeroCode's own, which a hand never moves.
    ZeroCode,
}

impl Owner {
    /// The word a question's state and a row carry for it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::SameApp => "same_app",
            Self::OtherApp => "other_app",
            Self::ZeroCode => "zerocode",
        }
    }
}

/// One window over the place: whose, at what layer, where — in points from
/// the target window's top-left corner, so the same scene read at another
/// place on the screen reads the same — and how much of the place it hides
/// by itself, per thousand.
#[derive(Debug, Clone, PartialEq)]
pub struct Coverer {
    pub id: u64,
    /// The app's name as the window list gives it — an identifier, never a
    /// window's title.
    pub app: String,
    pub owner: Owner,
    pub layer: i64,
    pub bounds: Rect,
    pub hides_permille: u16,
}

/// The place a hand is to press and what stands over it now.
#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    /// The target's window: its id, app and layer, and where it is on the
    /// screen.
    pub target: u64,
    pub app: String,
    pub layer: i64,
    pub window: Rect,
    /// The place on the screen, where the target's window stands now.
    pub spot: Rect,
    /// The windows over the place, front to back.
    pub coverers: Vec<Coverer>,
    /// Every window in front of the target that hides what is under it,
    /// crossing the place or not: what a move must keep clear of.
    pub in_front: Vec<Rect>,
    /// How much of the place the windows over it hide together, per
    /// thousand.
    pub hidden_permille: u16,
    /// Whether the place's centre — where a press lands — is hidden.
    pub centre_hidden: bool,
}

impl Cover {
    /// Whether a press on the place would land on something else: its
    /// centre is under another window.
    #[must_use]
    pub const fn blocks_a_press(&self) -> bool {
        self.centre_hidden
    }

    /// Whether any of the place is hidden.
    #[must_use]
    pub const fn hides_any(&self) -> bool {
        self.hidden_permille > 0
    }
}

/// What stands over `local` — a place in points from the target window's
/// top-left corner, or the whole window when `None` — in the window list
/// `windows` (front to back, every layer). `None` when the target is not on
/// the list: minimized, on another space, or gone.
#[must_use]
pub fn cover_of(windows: &[DesktopWindow], target: u64, local: Option<Rect>) -> Option<Cover> {
    let at = windows.iter().position(|window| window.id == target)?;
    let own = &windows[at];
    let spot = local.map_or(own.rect, |local| {
        Rect::new(
            own.rect.x + local.x,
            own.rect.y + local.y,
            local.width,
            local.height,
        )
    });
    let front: Vec<&DesktopWindow> = in_front(windows, at).collect();
    let coverers = front
        .iter()
        .filter(|window| window.rect.intersects(&spot))
        .map(|window| Coverer {
            id: window.id,
            app: window.app.clone(),
            owner: if window.own {
                Owner::ZeroCode
            } else if window.pid == own.pid {
                Owner::SameApp
            } else {
                Owner::OtherApp
            },
            layer: window.layer,
            bounds: Rect::new(
                window.rect.x - own.rect.x,
                window.rect.y - own.rect.y,
                window.rect.width,
                window.rect.height,
            ),
            hides_permille: hidden_permille(spot, &[window.rect]),
        })
        .collect();
    let ahead: Vec<Rect> = front.iter().map(|window| window.rect).collect();
    Some(Cover {
        target,
        app: own.app.clone(),
        layer: own.layer,
        window: own.rect,
        spot,
        coverers,
        hidden_permille: hidden_permille(spot, &ahead),
        centre_hidden: ahead
            .iter()
            .any(|cover| cover.contains_point(spot.mid_x(), spot.mid_y())),
        in_front: ahead,
    })
}

/// How much of `spot` the rectangles in `covers` hide together, per
/// thousand, to the nearest — and never 0 when any of it is hidden: a sliver
/// hidden is not nothing hidden.
#[must_use]
pub fn hidden_permille(spot: Rect, covers: &[Rect]) -> u16 {
    let whole = spot.area();
    if whole <= 0.0 {
        return 0;
    }
    let shown: f64 = covers
        .iter()
        .fold(vec![spot], |pieces, cover| {
            pieces
                .into_iter()
                .flat_map(|piece| piece.minus(cover))
                .collect()
        })
        .iter()
        .map(Rect::area)
        .sum();
    let hidden = whole - shown;
    if hidden <= 0.0 {
        return 0;
    }
    // A share is in [0, 1000] by construction; the cast is of that.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let permille = (hidden * 1_000.0 / whole).round().clamp(1.0, 1_000.0) as u16;
    permille
}

/// Where the target's window could stand — its new top-left corner — for
/// its place to show: every window in front kept clear of the place, the
/// whole window on one screen, and the shortest move that does both. The
/// candidates are the moves that put the place against an edge of a window
/// in front, and the window against an edge of a screen, alone and paired;
/// among those that hold, the shortest wins, the first found on a tie.
/// `None` when no such place is on any screen.
#[must_use]
pub fn clear_place(cover: &Cover, screens: &[Rect]) -> Option<[f64; 2]> {
    let (window, spot) = (cover.window, cover.spot);
    let mut dxs = vec![0.0];
    let mut dys = vec![0.0];
    for over in &cover.in_front {
        dxs.extend([over.x - spot.max_x(), over.max_x() - spot.x]);
        dys.extend([over.y - spot.max_y(), over.max_y() - spot.y]);
    }
    for screen in screens {
        dxs.extend([screen.x - window.x, screen.max_x() - window.max_x()]);
        dys.extend([screen.y - window.y, screen.max_y() - window.max_y()]);
    }
    let mut best: Option<(f64, [f64; 2])> = None;
    for dx in &dxs {
        for dy in &dys {
            let moved = |rect: Rect| Rect::new(rect.x + dx, rect.y + dy, rect.width, rect.height);
            let (to, place) = (moved(window), moved(spot));
            let clear = !cover.in_front.iter().any(|over| over.intersects(&place));
            let on_a_screen = screens.iter().any(|screen| screen.contains_rect(&to));
            let length = dx.hypot(*dy);
            if clear && on_a_screen && best.is_none_or(|(shortest, _)| length < shortest) {
                best = Some((length, [to.x, to.y]));
            }
        }
    }
    best.map(|(_, corner)| corner)
}

#[cfg(test)]
mod tests;
