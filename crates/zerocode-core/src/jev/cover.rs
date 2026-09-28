//! The cover seat (t-12979): a press whose place another window hides, and
//! what a person would do about it — asked of the scene's facts alone, in one
//! request, and carried out only as moves of the target's own window.
//!
//! One request asks four things none of which sees another's answer, so each
//! carries the premise whole ([`crate::jev::questions`]): whether a press
//! would land on something else (a Noul), what stands in front as a person
//! would call it (a closed choice), which move should come first (a closed
//! choice), and whether moves of the target's own window can do it at all (a
//! Noul). What is read of an answer and what is done with it are here; the
//! window lists the windows, moves them and asks the person
//! (`computer_use::cover` in the shell).
//!
//! The line the code holds, whatever the answer: a move changes the target's
//! own window and can be undone; what stands in front is never read, answered,
//! moved or closed; a kind that is the system's, an app waiting on an answer,
//! or not known — and an answer that did not come, or came unsure — holds the
//! hand and asks the person. A move that left the place covered hands on to
//! the next the same answer ranked, while it ranked it at all
//! ([`crate::jev::COVER_RUNNER_UP_FLOOR_PERMILLE`]), with no second question.
//! Its marks are later facts: whether the move the answer put first is the one
//! that left the place clear ([`marks`]).

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use super::choice::{self, Choice};
use super::noul;
use super::questions::{
    COVER_COVERED, COVER_COVERED_ASKS, COVER_COVERED_NO, COVER_COVERED_YES, COVER_KIND,
    COVER_KIND_ASKS, COVER_KINDS, COVER_MOVE, COVER_MOVE_ASKS, COVER_MOVES, COVER_REVERSIBLE,
    COVER_REVERSIBLE_ASKS, COVER_REVERSIBLE_NO, COVER_REVERSIBLE_YES, COVER_RUBRIC_VERSION,
    COVER_STATE_KEYS, cover_rubric_fingerprint,
};
use super::summary::{AGREED, BASELINE_AGREED, LABEL, NOT_COMPARED, REQUEST_AT};
use super::{COVER, COVER_OVER_CAP, COVER_RUNNER_UP_FLOOR_PERMILLE, NOUL_UNCERTAIN_TO_PERMILLE};
use crate::computer_use_protocol::cover::{Cover, Owner};
use crate::screen_action::ActionRefusal;

/// A move, by the rubric's word ([`COVER_MOVES`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    /// Bring the target's window to the front.
    RaiseTarget,
    /// Move the target's window to a clear place.
    MoveTarget,
    /// Wait and list the windows again.
    LookAgain,
    /// Leave everything and ask the person.
    AskPerson,
}

impl Move {
    /// In the rubric's order.
    pub const ALL: [Self; 4] = [
        Self::RaiseTarget,
        Self::MoveTarget,
        Self::LookAgain,
        Self::AskPerson,
    ];

    /// The rubric's word for it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        COVER_MOVES[self as usize].0
    }

    /// The move a word names.
    #[must_use]
    pub fn of(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|each| each.word() == word)
    }
}

/// Why the hand stays still and the person is asked — the word a row carries
/// and the window's sentence is chosen by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// The question came back with nothing usable inside its wall.
    Unanswered,
    /// What stands in front may be the system's, or an app's that waits on
    /// an answer, or is not known: it is the person's.
    Theirs,
    /// The answer was not sure enough of what stands in front, or of the
    /// move, to act on alone.
    Unsure,
    /// The answer doubts moves of the target's window can uncover it.
    NotReversible,
    /// The answer put asking the person first.
    Chosen,
    /// Every move allowed was made and the place is still covered.
    NothingCleared,
    /// The target's window is not on the screen to move.
    Gone,
}

impl Held {
    pub const ALL: [Self; 7] = [
        Self::Unanswered,
        Self::Theirs,
        Self::Unsure,
        Self::NotReversible,
        Self::Chosen,
        Self::NothingCleared,
        Self::Gone,
    ];

    /// The word a row and the window's sentence name it by.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Unanswered => "unanswered",
            Self::Theirs => "theirs",
            Self::Unsure => "unsure",
            Self::NotReversible => "not_reversible",
            Self::Chosen => "chosen",
            Self::NothingCleared => "nothing_cleared",
            Self::Gone => "gone",
        }
    }
}

/// The moves a hand makes, first to last, or why it makes none and the
/// person is asked.
pub type Ladder = Result<Vec<Move>, Held>;

/// The kinds a hand never works around: the system's, an app's waiting on
/// an answer, and what the facts do not tell.
fn theirs(kind: &str) -> bool {
    COVER_KINDS[2..].iter().any(|(word, _)| *word == kind)
}

/// The request's `state` and `questions` for one scene.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverAsk {
    pub state: Value,
    pub questions: Value,
}

/// A point value as the state carries it: whole points, so the same scene a
/// fraction of a point away asks the same bytes.
fn points(value: f64) -> i64 {
    // Screen points are well inside i64.
    #[allow(clippy::cast_possible_truncation)]
    let whole = value.round() as i64;
    whole
}

/// The four questions about `cover`, and the scene's facts as their state:
/// whose each window over the place is, its app, its layer and its bounds
/// from the target's corner — never a title, never what a window shows.
#[must_use]
pub fn ask(cover: &Cover) -> CoverAsk {
    let [target_key, place_key, over_key] = COVER_STATE_KEYS;
    let over: Vec<Value> = cover
        .coverers
        .iter()
        .take(COVER_OVER_CAP)
        .map(|over| {
            json!({
                "owner": over.owner.word(),
                "app": over.app,
                "layer": over.layer,
                "x": points(over.bounds.x),
                "y": points(over.bounds.y),
                "width": points(over.bounds.width),
                "height": points(over.bounds.height),
                "hides_permille": over.hides_permille,
            })
        })
        .collect();
    let state = json!({
        (target_key): {
            "app": cover.app,
            "layer": cover.layer,
            "width": points(cover.window.width),
            "height": points(cover.window.height),
        },
        (place_key): {
            "x": points(cover.spot.x - cover.window.x),
            "y": points(cover.spot.y - cover.window.y),
            "width": points(cover.spot.width),
            "height": points(cover.spot.height),
            "hidden_permille": cover.hidden_permille,
            "centre_hidden": cover.centre_hidden,
        },
        (over_key): over,
    });
    let mut questions = Map::new();
    questions.insert(
        COVER_COVERED.to_string(),
        noul::question(COVER_COVERED_ASKS, COVER_COVERED_YES, COVER_COVERED_NO),
    );
    questions.insert(
        COVER_KIND.to_string(),
        choice::question(COVER_KIND_ASKS, &COVER_KINDS),
    );
    questions.insert(
        COVER_MOVE.to_string(),
        choice::question(COVER_MOVE_ASKS, &COVER_MOVES),
    );
    questions.insert(
        COVER_REVERSIBLE.to_string(),
        noul::question(
            COVER_REVERSIBLE_ASKS,
            COVER_REVERSIBLE_YES,
            COVER_REVERSIBLE_NO,
        ),
    );
    CoverAsk {
        state,
        questions: Value::Object(questions),
    }
}

/// A validated answer to all four questions.
#[derive(Debug, Clone, PartialEq)]
pub struct CoverRead {
    /// The probability of yes that a press would land on something else.
    pub covered: f64,
    pub kind: Choice,
    pub moves: Choice,
    /// The probability of yes that moves of the target's window can do it.
    pub reversible: f64,
}

fn offered(options: &[(&str, &str)]) -> BTreeSet<String> {
    options
        .iter()
        .map(|(word, _)| (*word).to_string())
        .collect()
}

/// What the endpoint's `answers` say about the four questions. One broken
/// rule discards the answer whole.
///
/// # Errors
///
/// [`ActionRefusal`] names the first rule the answer broke.
pub fn read(answers: &Value) -> Result<CoverRead, ActionRefusal> {
    Ok(CoverRead {
        covered: noul::read(answers, COVER_COVERED)?,
        kind: choice::read(answers, COVER_KIND, &offered(&COVER_KINDS))?,
        moves: choice::read(answers, COVER_MOVE, &offered(&COVER_MOVES))?,
        reversible: noul::read(answers, COVER_REVERSIBLE)?,
    })
}

/// What the product does with no seat — the reader the seat is held against
/// ([`crate::jev::Baseline::TodaysRule`]): bring the target to the front, then
/// move it clear; but when another app's window stands above ordinary windows
/// over the place, it may be the system's, and the facts alone cannot tell,
/// so the person is asked.
///
/// # Errors
///
/// [`Held::Theirs`] when the person is asked.
pub fn todays_rule(cover: &Cover) -> Ladder {
    let above = cover
        .coverers
        .iter()
        .any(|over| over.owner == Owner::OtherApp && over.layer > cover.layer);
    if above {
        Err(Held::Theirs)
    } else {
        Ok(vec![Move::RaiseTarget, Move::MoveTarget])
    }
}

/// The moves an answer asks for, in its order — the move it put first, then
/// each it ranked from the runner-up floor, down to asking the person — or
/// why the person is asked instead: what stands in front is theirs, the
/// answer is unsure of it or of the move (the seat's own act line, `line`,
/// when its graded answers drew one), or it doubts the moves can do it. An
/// answer sure that nothing covers the place looks again first: the list
/// the question was asked of may be a moment old.
///
/// # Errors
///
/// [`Held`] says why the person is asked.
pub fn ladder(read: &CoverRead, line: Option<u16>) -> Ladder {
    if theirs(&read.kind.chosen) {
        return Err(Held::Theirs);
    }
    if !COVER.acts_on(read.kind.confidence, line) || !COVER.acts_on(read.moves.confidence, line) {
        return Err(Held::Unsure);
    }
    if !super::reaches(read.reversible, NOUL_UNCERTAIN_TO_PERMILLE) {
        return Err(Held::NotReversible);
    }
    let mut ranked: Vec<(&String, f64)> = read
        .moves
        .probabilities
        .iter()
        .map(|(word, share)| (word, *share))
        .collect();
    // Highest first; the rubric's order on a tie, so the same answer always
    // ranks the same way.
    ranked.sort_by(|(a, pa), (b, pb)| {
        pb.total_cmp(pa).then_with(|| {
            Move::of(a)
                .map(|m| m as usize)
                .cmp(&Move::of(b).map(|m| m as usize))
        })
    });
    let first = Move::of(&read.moves.chosen).ok_or(Held::Unsure)?;
    let mut moves = Vec::new();
    if super::reaches(1.0 - read.covered, NOUL_UNCERTAIN_TO_PERMILLE) && first != Move::LookAgain {
        moves.push(Move::LookAgain);
    }
    moves.push(first);
    for (word, share) in ranked {
        let Some(next) = Move::of(word) else {
            continue;
        };
        if next == first || moves.contains(&next) {
            continue;
        }
        if !super::reaches(share, COVER_RUNNER_UP_FLOOR_PERMILLE) {
            break;
        }
        moves.push(next);
    }
    if let Some(ask) = moves.iter().position(|each| *each == Move::AskPerson) {
        moves.truncate(ask);
    }
    if moves.is_empty() {
        return Err(Held::Chosen);
    }
    Ok(moves)
}

/// What followed one question: the moves made, in order, and the one after
/// which the place showed — `None` when none did or none was made.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub tried: Vec<Move>,
    pub cleared_by: Option<Move>,
}

/// The mark one move put first earns from what followed: a move that was
/// made is right when the place showed after it and wrong when it did not;
/// asking the person is right when no move that was made cleared the place,
/// and wrong when one did. A move nobody made, or a scene where nothing was
/// made at all, is not compared.
fn mark_of(first: Move, outcome: &Outcome) -> Result<bool, &'static str> {
    if first == Move::AskPerson {
        return if outcome.tried.iter().any(|each| *each != Move::LookAgain) {
            Ok(outcome.cleared_by.is_none())
        } else {
            Err(NOT_TRIED)
        };
    }
    if outcome.tried.contains(&first) {
        Ok(outcome.cleared_by == Some(first))
    } else {
        Err(NOT_TRIED)
    }
}

/// The word a label writes for a move that was never made.
pub const NOT_TRIED: &str = "not_tried";

/// The answer's mark and today's rule's, for one scene — the answer's first
/// move and the rule's, each graded by [`mark_of`].
///
/// # Errors
///
/// The word the label writes when the answer's first move was not compared.
pub fn marks(
    read: &CoverRead,
    rule: &Ladder,
    outcome: &Outcome,
) -> Result<(bool, Option<bool>), &'static str> {
    let first = Move::of(&read.moves.chosen).ok_or(NOT_TRIED)?;
    let agreed = mark_of(first, outcome)?;
    let rule_first = match rule {
        Ok(moves) => moves.first().copied().unwrap_or(Move::AskPerson),
        Err(_) => Move::AskPerson,
    };
    Ok((agreed, mark_of(rule_first, outcome).ok()))
}

/// The request's row: what was asked — the scene's facts, never a title —
/// and what came back, read or refused by its rule's word.
#[must_use]
pub fn request_row(asked: &str, cover_ask: &CoverAsk, answer: Result<&CoverRead, &str>) -> Value {
    let mut row = json!({
        "asked": asked,
        "rubricVersion": COVER_RUBRIC_VERSION,
        "rubric": cover_rubric_fingerprint(),
        "state": cover_ask.state,
    });
    match answer {
        Ok(read) => {
            row["outcome"] = json!(super::door::ANSWERED_OUTCOME);
            row["chosen"] = json!(read.moves.chosen);
            row["probabilities"] = json!(read.moves.probabilities);
            row["confidence"] = json!(read.moves.confidence);
            row[COVER_KIND] = json!(read.kind.chosen);
            row["kindConfidence"] = json!(read.kind.confidence);
            row[COVER_COVERED] = json!(read.covered);
            row[COVER_REVERSIBLE] = json!(read.reversible);
        }
        Err(word) => row["outcome"] = json!(word),
    }
    row
}

/// The label a scene's end writes for the request `asked` at `request_at`:
/// the moves made and what cleared, with the marks — or why none.
#[must_use]
pub fn label_row(
    asked: &str,
    request_at: i64,
    outcome: &Outcome,
    marks: Result<(bool, Option<bool>), &'static str>,
) -> Value {
    let mut row = json!({
        (LABEL.canonical): asked,
        (REQUEST_AT.canonical): request_at,
        "tried": outcome.tried.iter().map(|each| each.word()).collect::<Vec<_>>(),
        "clearedBy": outcome.cleared_by.map(Move::word),
    });
    match marks {
        Ok((agreed, baseline)) => {
            row[AGREED.canonical] = json!(agreed);
            if let Some(baseline) = baseline {
                row[BASELINE_AGREED.canonical] = json!(baseline);
            }
        }
        Err(word) => row[NOT_COMPARED.canonical] = json!(word),
    }
    row
}

#[cfg(test)]
mod tests;
