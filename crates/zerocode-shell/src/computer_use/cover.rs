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
//! that does not come usable inside its wall leaves the hand to today's rule —
//! the rule a hand with Jev switched off keeps, which asks the person itself
//! for what it cannot tell from the system's — and never to the plan the hand
//! had before it looked; under `shadow` or `off` the hand makes today's
//! rule's moves and a `shadow` answer is only kept.

use std::collections::VecDeque;
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

/// What a place must be to press: its centre showing (a press lands on one
/// point), or less than a share of it hidden, per thousand (a hand that
/// watches a region and refuses what is hidden in it cell by cell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needs {
    Centre,
    Under(u16),
}

impl Needs {
    pub(crate) fn met(self, cover: &Cover) -> bool {
        match self {
            Self::Centre => !cover.blocks_a_press(),
            Self::Under(share) => cover.hidden_permille < share,
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
/// window it may not touch, moves that did not clear it, or a window that is
/// not on the screen.
fn said(held: Held, cover: Option<&Cover>) -> Said {
    let app = cover
        .and_then(|cover| cover.coverers.first())
        .map_or_else(String::new, |over| over.app.clone());
    let (key, reason) = match held {
        Held::NothingCleared => (
            "computer.cover.stuck",
            "the target's window was brought to the front and moved, and the place is still covered",
        ),
        Held::Gone => (
            "computer.cover.gone",
            "the target's window is not on the screen (minimized, or on another desktop)",
        ),
        Held::Theirs | Held::Unsure | Held::NotReversible | Held::Chosen | Held::Unanswered => (
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
    let listed = desktop_windows(hand.call)?;
    let Some(cover) = cover_of(&listed, place.window, place.local) else {
        return Ok(to_person(Held::Gone, None, place, hand));
    };
    if place.needs.met(&cover) {
        return Ok(Uncovered::Clear {
            moves: Vec::new(),
            by_person: false,
        });
    }
    let rule = seat::todays_rule(&cover);
    let (mode, applies, line) = judge.standing();
    let asked_at = (hand.wall_ms)();
    let asked = format!("cv-{asked_at}-{}", place.window);
    let mut rows = Vec::new();
    let mut read: Option<CoverRead> = None;
    let ladder: Ladder = if mode == JevMode::Off {
        rule.clone()
    } else {
        let question = seat::ask(&cover);
        let wired = judge.ask(&question);
        let answer = wired.answer.clone().and_then(|body| {
            serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|parsed| parsed.get("answers").cloned())
                .ok_or_else(|| {
                    zerocode_core::jev::choice::ChoiceRefusal::NoAnswer
                        .token()
                        .to_string()
                })
                .and_then(|answers| {
                    seat::read(&answers).map_err(|refusal| refusal.token().to_string())
                })
        });
        let mut row = seat::request_row(&asked, &question, answer.as_ref().map_err(String::as_str));
        row["at"] = json!(asked_at);
        row["mode"] = json!(mode.key());
        row["attempts"] = json!(wired.attempts);
        row["requestBytes"] = json!(wired.request_bytes);
        row["rttMs"] = json!(wired.rtt_ms);
        // An answer carried out, or the hand as it goes with no seat: the door
        // refusing the question (the switch, the folder's consent, the day's
        // count) and a question that brought nothing usable back inside its
        // wall both leave it to today's rule, and the row says which.
        let ladder = match (&answer, applies) {
            (Ok(read), true) => seat::ladder(read, line),
            (Err(_), true) => {
                row["why"] = json!(Held::Unanswered.word());
                rule.clone()
            }
            (_, false) => rule.clone(),
        };
        row["applied"] = json!(applies && answer.is_ok());
        if let Err(held) = &ladder {
            row["held"] = json!(held.word());
        }
        rows.push(row);
        read = answer.ok();
        ladder
    };
    let mut outcome = Outcome::default();
    let ended = match ladder {
        Err(held) => to_person(held, Some(&cover), place, hand),
        Ok(moves) => {
            let mut now = Some(cover);
            let mut next_moves: VecDeque<Move> = moves.into();
            let mut cleared = false;
            // The rule's moves go on once when the answer's made nothing.
            let mut rule_left = rule.as_ref().ok().cloned();
            while let Some(next) = next_moves.pop_front().or_else(|| {
                let untried: Vec<Move> = rule_left
                    .take()
                    .filter(|_| {
                        outcome
                            .tried
                            .iter()
                            .all(|each| outcome.not_made.contains(each))
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|each| !outcome.tried.contains(each))
                    .collect();
                next_moves.extend(untried);
                next_moves.pop_front()
            }) {
                let Some(standing) = now.clone() else {
                    break;
                };
                outcome.tried.push(next);
                match step(next, &standing, place, hand) {
                    Ok((made, after)) => {
                        if !made {
                            outcome.not_made.push(next);
                        }
                        now = after;
                    }
                    Err(refusal) => {
                        record(
                            judge,
                            rows,
                            read.as_ref(),
                            &rule,
                            &outcome,
                            &asked,
                            asked_at,
                        );
                        return Err(refusal);
                    }
                }
                if now.as_ref().is_some_and(|after| place.needs.met(after)) {
                    cleared = true;
                    // A move the helper would not make cleared nothing: what
                    // shows now is the last one made, landing late.
                    outcome.cleared_by = outcome
                        .tried
                        .iter()
                        .rev()
                        .find(|each| !outcome.not_made.contains(each))
                        .copied();
                    break;
                }
            }
            match (&now, cleared) {
                (_, true) => Uncovered::Clear {
                    moves: outcome.tried.clone(),
                    by_person: false,
                },
                (None, false) => to_person(Held::Gone, None, place, hand),
                (Some(standing), false) => {
                    to_person(Held::NothingCleared, Some(standing), place, hand)
                }
            }
        }
    };
    record(
        judge,
        rows,
        read.as_ref(),
        &rule,
        &outcome,
        &asked,
        asked_at,
    );
    Ok(ended)
}

/// One move and the look after it: whether it was made, and what stands
/// now. The window server's order can land after the helper's answer: a
/// raise after which nothing in front of the target changed is looked at
/// once more after a pause, and a raise its app's activation carried —
/// which answers alike whether the order changed or not — that still changed
/// nothing was not made.
fn step(
    next: Move,
    standing: &Cover,
    place: Place,
    hand: &mut Hand<'_>,
) -> Result<(bool, Option<Cover>), ComputerUseError> {
    let made = make(next, standing, hand)?;
    let look = |hand: &mut Hand<'_>| {
        desktop_windows(hand.call).map(|listed| cover_of(&listed, place.window, place.local))
    };
    let mut after = look(hand)?;
    let unchanged = |after: &Option<Cover>| {
        after.as_ref().is_some_and(|after| {
            !place.needs.met(after)
                && after.window == standing.window
                && after.in_front == standing.in_front
        })
    };
    if next == Move::RaiseTarget && made != Made::Refused && unchanged(&after) {
        (hand.pause)(Duration::from_millis(COVER_LOOK_AGAIN_MS));
        after = look(hand)?;
        if made == Made::ByItsApp && unchanged(&after) {
            return Ok((false, after));
        }
    }
    Ok((made != Made::Refused, after))
}

/// What the helper said to one move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Made {
    /// It made it — or there was nothing to make.
    Done,
    /// It brought the window's whole app forward instead of the window: an
    /// answer that says nothing of the order.
    ByItsApp,
    /// It would not make it.
    Refused,
}

/// One move of the target's own window. A window its app does not publish
/// to accessibility (a non-activating panel) cannot be raised or moved by
/// the helper: it is brought forward with its whole app instead, and a move
/// the helper refuses is a move not made — the look after it says what
/// stands, and the next move, or the person, follows. Anything else the
/// helper says is an error.
fn make(next: Move, cover: &Cover, hand: &mut Hand<'_>) -> Result<Made, ComputerUseError> {
    match next {
        Move::RaiseTarget => match (hand.call)(
            "windowAction",
            json!({ "action": "focus", "windowId": cover.target }),
        ) {
            Err(refusal) if not_its_to_move(&refusal) => not_made((hand.call)(
                "activateApp",
                json!({ "app": format!("pid:{}", cover.pid) }),
            ))
            .map(|made| {
                if made == Made::Done {
                    Made::ByItsApp
                } else {
                    made
                }
            }),
            raised => raised.map(|_| Made::Done),
        },
        Move::MoveTarget => {
            let displays = (hand.call)("displays", json!({}))?;
            let screens = screens_of(&displays);
            match clear_place(cover, &screens) {
                Some([x, y]) => not_made((hand.call)(
                    "windowAction",
                    json!({ "action": "move", "windowId": cover.target, "x": x, "y": y }),
                )),
                // No place on any screen clears it: the move is made of
                // nothing, and the look after it says so.
                None => Ok(Made::Done),
            }
        }
        Move::LookAgain => {
            (hand.pause)(Duration::from_millis(COVER_LOOK_AGAIN_MS));
            Ok(Made::Done)
        }
        Move::AskPerson => Ok(Made::Done),
    }
}

/// Whether the helper refused a window move because the window is not one it
/// can move: gone from its app's accessibility windows, or refused by them.
fn not_its_to_move(refusal: &ComputerUseError) -> bool {
    refusal.code == error_code::WINDOW_NOT_FOUND || refusal.code == error_code::ACCESSIBILITY_ERROR
}

/// A move's answer: made, or — the helper refusing to make it — not made.
fn not_made(answer: Result<Value, ComputerUseError>) -> Result<Made, ComputerUseError> {
    match answer {
        Err(refusal) if not_its_to_move(&refusal) => Ok(Made::Refused),
        answer => answer.map(|_| Made::Done),
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
/// (`confirm::handoff_said`), bounded by the confirmation table.
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
    if zerocode_core::computer_use_protocol::execution::requested(&mark.params) {
        return press();
    }
    let refused = match press() {
        Err(error) if error.code == error_code::ELEMENT_NOT_FOUND => error,
        pressed => return pressed,
    };
    let place = Place {
        window: mark.window_id,
        local: Some(mark.local),
        needs: Needs::Centre,
    };
    match uncover(place, hand, judge)? {
        Uncovered::Clear { moves, by_person } if by_person || !moves.is_empty() => {
            let mut answer = press()?;
            if let Some(object) = answer.as_object_mut() {
                object.insert(
                    "uncovered".into(),
                    json!({
                        "moves": moves.iter().map(|each| each.word()).collect::<Vec<_>>(),
                        "byPerson": by_person,
                    }),
                );
            }
            Ok(answer)
        }
        Uncovered::Clear { .. } => Err(refused),
        Uncovered::Held { held, over } => {
            let named = [Some(mark.role.as_str()), mark.label.as_deref()]
                .into_iter()
                .flatten()
                .filter(|word| !word.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let whose = if over.is_empty() {
                "another window".to_string()
            } else {
                format!("a window of {over}")
            };
            Err(ComputerUseError::new(
                error_code::COVERED,
                format!(
                    "mark {} ({named}) is covered at its centre by {whose} ({}); nothing was pressed and nothing in front was moved or closed — the person clears it, then press again",
                    mark.mark,
                    held.word()
                ),
            ))
        }
    }
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

/// A cover seat that is never asked — `off` — for a hand that makes today's
/// rule's moves alone: a test's.
#[cfg(test)]
pub(crate) struct Unasked;

#[cfg(test)]
impl Judge for Unasked {
    fn standing(&mut self) -> (JevMode, bool, Option<u16>) {
        (JevMode::Off, false, None)
    }

    fn ask(&mut self, _asked: &CoverAsk) -> Wired {
        Wired {
            answer: Err(JevMode::Off.key().to_string()),
            attempts: 0,
            request_bytes: 0,
            rtt_ms: 0,
        }
    }

    fn record(&mut self, _rows: Vec<Value>) {}
}

#[cfg(test)]
mod tests;
