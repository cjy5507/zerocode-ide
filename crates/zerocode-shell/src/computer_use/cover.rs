//! Uncovering a press's place (t-12979): a place another window hides is
//! found, the hand stops, the cover seat is asked once, and the target's own
//! window is brought to the front or moved clear — checked on a fresh window
//! list after every move — or the person is asked in one line.
//!
//! What stands over the place, and where the window could stand, are the
//! core's (`zerocode_core::computer_use_protocol::cover`); what the seat asks
//! and what its answer allows are the seat's (`zerocode_core::jev::cover`).
//! This module is the hand between them: the helper's window list, its two
//! window moves (`windowAction` focus and move), the pause before a second
//! look, and the person's card. It never presses: the caller presses once the
//! place shows, down its own road.
//!
//! What never moves: a move changes the target's own window and nothing in
//! front is read, answered, moved or closed; under a seat that acts, an answer
//! that does not come inside its wall holds the hand for the person — the
//! press never goes on as though nothing stood there; under `shadow` or `off`
//! the hand makes today's rule's moves and a `shadow` answer is only kept.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use zerocode_core::computer_use::{COMPUTER_CONFIRM_TIMEOUT_MS, COVER_LOOK_AGAIN_MS};
use zerocode_core::computer_use_protocol::cover::{Cover, clear_place, cover_of};
use zerocode_core::computer_use_protocol::error_code;
use zerocode_core::computer_use_protocol::render::Rect;
use zerocode_core::jev::cover::{self as seat, CoverAsk, CoverRead, Held, Ladder, Move, Outcome};
use zerocode_core::jev::reflex_decide::Wired;
use zerocode_core::jev::{COVER, JevMode, Run};

use super::ComputerUseError;
use super::confirm::{Asking, Decision};
use super::marks::{PinnedClick, desktop_windows};
use crate::systemone::{self, Wire};

/// What a place must be to press: its centre showing — a press lands on one
/// point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needs {
    Centre,
}

impl Needs {
    fn met(self, cover: &Cover) -> bool {
        match self {
            Self::Centre => !cover.blocks_a_press(),
        }
    }
}

/// The place to uncover: the target's window, and where in it — in points
/// from its top-left corner — or all of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Place {
    pub window: u64,
    pub local: Option<Rect>,
    pub needs: Needs,
}

/// The cover seat as the hand meets it: where it stands, one question on
/// the wire inside its wall, and its rows.
pub(crate) trait Judge {
    /// The seat's word, whether it acts now, and the act line its graded
    /// answers drew.
    fn standing(&mut self) -> (JevMode, bool, Option<u16>);
    /// One question, bounded by the seat's wall.
    fn ask(&mut self, asked: &CoverAsk) -> Wired;
    /// The rows one scene left, into the seat's ledger.
    fn record(&mut self, rows: Vec<Value>);
}

/// The hand: the helper, the pause before a second look, the person's card,
/// and the wall clock rows are stamped with.
pub(crate) struct Hand<'a> {
    pub call: &'a mut dyn FnMut(&str, Value) -> Result<Value, ComputerUseError>,
    pub pause: &'a mut dyn FnMut(Duration),
    pub person: &'a mut dyn FnMut(&Said) -> Decision,
    pub wall_ms: &'a dyn Fn() -> i64,
}

/// What the person's card says: its words, the page's key and what fills it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Said {
    pub held: Held,
    pub key: &'static str,
    pub args: Value,
    pub reason: String,
}

/// What uncovering came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Uncovered {
    /// The place shows: the moves made (none when nothing stood there), and
    /// whether it was the person who cleared it.
    Clear { moves: Vec<Move>, by_person: bool },
    /// The hand stayed still and the person did not clear it: why, and the
    /// app whose window stands over the place ("" when none is known).
    Held { held: Held, over: String },
}

/// The page's words for why the hand stopped, by the reason: an app's
/// window it may not touch, a seat that did not answer, moves that did not
/// clear it, or a window that is not on the screen.
fn said(held: Held, cover: Option<&Cover>) -> Said {
    let app = cover
        .and_then(|cover| cover.coverers.first())
        .map_or_else(String::new, |over| over.app.clone());
    let (key, reason) = match held {
        Held::Unanswered => (
            "computer.cover.unanswered",
            "the place to press is covered and the cover judgment did not answer in time; the hand stopped",
        ),
        Held::NothingCleared => (
            "computer.cover.stuck",
            "the target's window was brought to the front and moved, and the place is still covered",
        ),
        Held::Gone => (
            "computer.cover.gone",
            "the target's window is not on the screen (minimized, or on another desktop)",
        ),
        Held::Theirs | Held::Unsure | Held::NotReversible | Held::Chosen => (
            "computer.cover.ask",
            "another window covers the place to press; it was not read or closed",
        ),
    };
    Said {
        held,
        key,
        args: json!({ "app": app }),
        reason: if app.is_empty() {
            reason.to_string()
        } else {
            format!("{reason} (over it: {app})")
        },
    }
}

/// Uncover `place`: list the windows, and if what stands over it keeps the
/// press from landing, stop, ask the seat, make the moves its answer — or
/// today's rule — allows, looking again after each, and hand what is left to
/// the person.
///
/// # Errors
///
/// The helper's refusal to list the windows.
pub(crate) fn uncover(
    place: Place,
    hand: &mut Hand<'_>,
    judge: &mut dyn Judge,
) -> Result<Uncovered, ComputerUseError> {
    // Today (the red before t-12979): the hand stops in front of what covers
    // its place and moves nothing.
    let _ = judge;
    let listed = desktop_windows(hand.call)?;
    Ok(match cover_of(&listed, place.window, place.local) {
        Some(cover) if place.needs.met(&cover) => Uncovered::Clear {
            moves: Vec::new(),
            by_person: false,
        },
        Some(_) => Uncovered::Held {
            held: Held::NothingCleared,
            over: String::new(),
        },
        None => Uncovered::Held {
            held: Held::Gone,
            over: String::new(),
        },
    })
}

/// One move of the target's own window.
fn make(next: Move, cover: &Cover, hand: &mut Hand<'_>) -> Result<(), ComputerUseError> {
    match next {
        Move::RaiseTarget => (hand.call)(
            "windowAction",
            json!({ "action": "focus", "windowId": cover.target }),
        )
        .map(drop),
        Move::MoveTarget => {
            let displays = (hand.call)("displays", json!({}))?;
            let screens = screens_of(&displays);
            match clear_place(cover, &screens) {
                Some([x, y]) => (hand.call)(
                    "windowAction",
                    json!({ "action": "move", "windowId": cover.target, "x": x, "y": y }),
                )
                .map(drop),
                // No place on any screen clears it: the move is made of
                // nothing, and the look after it says so.
                None => Ok(()),
            }
        }
        Move::LookAgain => {
            (hand.pause)(Duration::from_millis(COVER_LOOK_AGAIN_MS));
            Ok(())
        }
        Move::AskPerson => Ok(()),
    }
}

/// The screens' bounds from the helper's `displays`.
fn screens_of(displays: &Value) -> Vec<Rect> {
    displays
        .get("displays")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|shown| {
            let bounds = shown.get("bounds")?;
            let number = |key: &str| bounds.get(key).and_then(Value::as_f64);
            Some(Rect::new(
                number("x")?,
                number("y")?,
                number("width")?,
                number("height")?,
            ))
        })
        .collect()
}

/// Hand the place to the person with one line, and look again when they say
/// they are done: clear then, or held.
fn to_person(held: Held, cover: Option<&Cover>, place: Place, hand: &mut Hand<'_>) -> Uncovered {
    let line = said(held, cover);
    let over = line.args["app"].as_str().unwrap_or_default().to_string();
    if (hand.person)(&line) != Decision::Allowed {
        return Uncovered::Held { held, over };
    }
    let cleared = desktop_windows(hand.call)
        .ok()
        .and_then(|listed| cover_of(&listed, place.window, place.local))
        .is_some_and(|now| place.needs.met(&now));
    if cleared {
        Uncovered::Clear {
            moves: Vec::new(),
            by_person: true,
        }
    } else {
        Uncovered::Held { held, over }
    }
}

/// The scene's rows: its request, when the seat was asked, and the label
/// the moves made earn it.
fn record(
    judge: &mut dyn Judge,
    mut rows: Vec<Value>,
    read: Option<&CoverRead>,
    rule: &Ladder,
    outcome: &Outcome,
    asked: &str,
    asked_at: i64,
) {
    if let Some(read) = read {
        rows.push(seat::label_row(
            asked,
            asked_at,
            outcome,
            seat::marks(read, rule, outcome),
        ));
    }
    if !rows.is_empty() {
        judge.record(rows);
    }
}

/// The page asks the person the way every handed-over desk is asked
/// (`confirm::handoff`), bounded by the confirmation table.
pub(crate) fn ask_the_person(line: &Said) -> Decision {
    super::confirm::handoff_said(
        &line.reason,
        line.key,
        line.args.clone(),
        COMPUTER_CONFIRM_TIMEOUT_MS,
    )
}

/// A press by a look's mark, whose place may be hidden: pressed as ever,
/// and — when the helper refused it as not found — the window list read for
/// what stands over its centre. Something does: it is uncovered and the mark
/// pressed once more, or the press is refused as covered, in words of whose
/// window it is and never of what the helper's refusal said (its words may
/// name what the covering window shows). Nothing does: the refusal was the
/// pin's, and it stands.
///
/// # Errors
///
/// The press's own refusal, the helper's refusal to list the windows, or
/// [`error_code::COVERED`].
pub(crate) fn press_mark(
    mark: &PinnedClick,
    press: &mut dyn FnMut() -> Result<Value, ComputerUseError>,
    hand: &mut Hand<'_>,
    judge: &mut dyn Judge,
) -> Result<Value, ComputerUseError> {
    // Today (the red before t-12979): the press's own answer, refused or not.
    let _ = (mark, hand, judge);
    press()
}

/// The cover seat on this machine: the person's settings and its own
/// ledger, the Jev door for the folder the press was asked from.
pub(crate) struct LiveJudge {
    wire: Wire,
    workspace: Option<PathBuf>,
}

impl LiveJudge {
    /// The seat on this machine's settings and ledger.
    pub(crate) fn here(workspace: Option<&Path>) -> Self {
        Self::on(Wire::of_this_machine(), workspace)
    }

    /// The seat on `wire`'s settings and ledger — a bench's own zo home.
    pub(crate) fn on(wire: Wire, workspace: Option<&Path>) -> Self {
        Self {
            wire,
            workspace: workspace.map(Path::to_path_buf),
        }
    }
}

impl Judge for LiveJudge {
    fn standing(&mut self) -> (JevMode, bool, Option<u16>) {
        let (mode, applies) = systemone::standing_in(&self.wire, &COVER, Run::Fresh);
        (mode, applies, systemone::act_line(&self.wire, &COVER))
    }

    fn ask(&mut self, asked: &CoverAsk) -> Wired {
        let began = Instant::now();
        // The seat's own wall; a row with none waits for nothing and holds.
        let wall = COVER
            .apply_deadline_ms
            .map_or(Duration::ZERO, Duration::from_millis);
        let sent = self.wire.ask(
            &COVER,
            self.workspace.as_deref(),
            systemone::request_body(&asked.state, &asked.questions),
            wall,
        );
        Wired {
            answer: sent.answer,
            attempts: sent.spent.requests,
            request_bytes: sent.request_bytes,
            rtt_ms: u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    }

    fn record(&mut self, rows: Vec<Value>) {
        if let Some(ledger) = systemone::ledger_of(&self.wire, &COVER) {
            systemone::record_rows(
                &COVER,
                &ledger,
                &rows,
                crate::project_runtime::now_epoch_ms(),
            );
        }
    }
}

/// [`press_mark`] on this machine: the helper, the person's card when a
/// person is there to ask (a recipe's walk hands the step back instead), and
/// the cover seat for `workspace`.
///
/// # Errors
///
/// As [`press_mark`].
pub(crate) fn press_mark_here(
    mark: &PinnedClick,
    press: &mut dyn FnMut() -> Result<Value, ComputerUseError>,
    asking: Asking,
    workspace: Option<&Path>,
) -> Result<Value, ComputerUseError> {
    let mut call = super::call;
    let mut pause = std::thread::sleep;
    let mut person = |line: &Said| match asking {
        Asking::Person => ask_the_person(line),
        Asking::HandBack => Decision::Refused,
    };
    let wall = crate::project_runtime::now_epoch_ms;
    press_mark(
        mark,
        press,
        &mut Hand {
            call: &mut call,
            pause: &mut pause,
            person: &mut person,
            wall_ms: &wall,
        },
        &mut LiveJudge::here(workspace),
    )
}

#[cfg(test)]
mod tests;
