//! Where one answer of a seat may act alone, read off the seat's own graded
//! answers (t-9468).
//!
//! A seat's [`crate::jev::ConfidenceBands`] were drawn before any label
//! existed: the confidence-routing pattern's 0.6 and 0.85 and the seats'
//! press floors — policy lines the vendor's own pages say are the reader's
//! to move "by plotting confidence against accuracy on your data". Read
//! against a grid of lines, this machine's ledgers (t-9087 r1 §3) held no
//! seat whose confident part beat its current rule, each for a reason of
//! its own: the notify seat's most confident answers were its worst, every
//! one of the stall seat's answers sat at 0.7 or above, the summons'
//! baseline was right by construction, and placement's confident part held
//! seventeen marks.
//!
//! So a seat's act line is a reading and not a constant ([`calibrate`]):
//! its graded answers — each the confidence it was given with, the mark
//! that graded it and the baseline's mark on the same fact — counted at or
//! above every line of one grid ([`CALIBRATION`]), and the lowest line whose
//! answers pass what the judge would ask of them there
//! ([`crate::jev::promote::first_broken_line`]'s own lines, read on the
//! line's answers alone). Before any line is read, the premise: a seat whose
//! every answer already passes wants no line; some line must split the
//! answers; and no line's answers may be, beyond their own interval, less
//! often right than a lower line's — a confidence that runs against being
//! right is no line to act from.
//!
//! The line is kept as data beside the seat's ledger ([`THRESHOLDS_FILE`]):
//! the replay tool writes it on an explicit flag, and the product only reads
//! it ([`Thresholds::line_of`]) — a row only while it was read under the
//! words its seat asks now, and only for a seat whose stage reads a line at
//! all ([`crate::jev::JevUse::reads_act_line`]). A seat with no row acts as
//! it always has.

use std::path::Path;

use serde_json::{Value, json};

use crate::jev::JevUse;
use crate::jev::promote::{Line, marks_that_can_clear, permille};
use crate::jev::recent::CONFIDENCE;
use crate::jev::summary::{RUBRIC_VERSION, WILSON_Z_95, wilson_lower};

/// The lines a seat's graded answers are read at, and what a line must show
/// beyond the judge's own lines before it is the seat's — one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Calibration {
    /// The lines, per thousand of an answer's own confidence, lowest first;
    /// the first is every answer. The nine t-9087 r1 read this machine's
    /// ledgers at: the Noul cookbook's uncertain middle (0.3, 0.7), the
    /// screen seats' press floor (0.5), the confidence-routing pattern's two
    /// (0.6, 0.85), and the steps between and above them.
    pub grid: &'static [u16],
    /// How far above every answer's lower bound, per thousand, a line's must
    /// stand before the line separates anything. Shedding its least
    /// confident answers lifted the notify seat's bound 31‰ (688 → 719, from
    /// every answer to 0.6) while its most confident were its worst (t-9087
    /// r1 §3): a lift that size is a tail shed, not a line, and this asks
    /// for more.
    pub lift_permille: u16,
    /// The least share of a seat's answered requests, per thousand, a line
    /// must act on — under it `auto` does not rise on the line
    /// ([`Line::ApplyShare`]). A policy line, not a calibrated claim: a seat
    /// acting on fewer than one answer in five pays five requests to act
    /// once, and reads as acting while it mostly records.
    pub apply_share_floor_permille: u16,
}

/// The one calibration every seat is read with.
pub const CALIBRATION: Calibration = Calibration {
    grid: &[0, 300, 500, 600, 700, 800, 850, 900, 950],
    lift_permille: 50,
    apply_share_floor_permille: 200,
};

/// The confidence a row says its answer was given with
/// ([`crate::jev::recent::CONFIDENCE`]) — `None` for a row that says none,
/// or a reading outside `0..=1`, which no line can be read against.
#[must_use]
pub fn answer_confidence(row: &Value) -> Option<f64> {
    CONFIDENCE
        .read(row)
        .and_then(Value::as_f64)
        .filter(|confidence| (0.0..=1.0).contains(confidence))
}

/// One graded answer: the confidence it was given with, where its rows say
/// one, whether the mark that graded it agreed, and whether the seat's
/// baseline was right on the same fact, where the writer marked it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Graded {
    pub confidence: Option<f64>,
    pub agreed: bool,
    pub baseline: Option<bool>,
}

/// A seat's graded answers and answered requests counted at one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AtLine {
    /// The line, per thousand of an answer's confidence.
    pub from_permille: u16,
    /// Graded answers at or above the line, and how many of their marks
    /// agreed.
    pub marks: usize,
    pub agreed: usize,
    /// Of those, the ones the baseline marked, and how many it got right.
    pub baseline_marks: usize,
    pub baseline_agreed: usize,
    /// Graded answers under the line — the ones it leaves to today's path —
    /// and how many of their marks agreed.
    pub under_marks: usize,
    pub under_agreed: usize,
    /// Answered requests, and how many of them the line acts on. A request
    /// whose row says no confidence is one no line acts on.
    pub answered: usize,
    pub acted: usize,
}

impl AtLine {
    /// `graded` and `answered` counted at `from_permille`. A graded answer
    /// that says no confidence stands at no line, above or under.
    #[must_use]
    pub fn of(graded: &[Graded], answered: &[Option<f64>], from_permille: u16) -> Self {
        let mut at = Self {
            from_permille,
            answered: answered.len(),
            ..Self::default()
        };
        for one in graded {
            let Some(confidence) = one.confidence else {
                continue;
            };
            if crate::jev::reaches(confidence, from_permille) {
                at.marks += 1;
                at.agreed += usize::from(one.agreed);
                if let Some(baseline) = one.baseline {
                    at.baseline_marks += 1;
                    at.baseline_agreed += usize::from(baseline);
                }
            } else {
                at.under_marks += 1;
                at.under_agreed += usize::from(one.agreed);
            }
        }
        at.acted = answered
            .iter()
            .flatten()
            .filter(|confidence| crate::jev::reaches(**confidence, from_permille))
            .count();
        at
    }

    /// The 95% Wilson lower bound on the agreed share, per thousand —
    /// `None` for a line no graded answer reaches.
    #[must_use]
    pub fn lower_bound_permille(&self) -> Option<u16> {
        (self.marks > 0).then(|| permille(wilson_lower(self.agreed, self.marks, WILSON_Z_95)))
    }

    /// The upper end of the same interval: the lower end of the disagreeing
    /// share's, turned over — one formula for both ends.
    fn upper_bound(&self) -> Option<f64> {
        (self.marks > 0)
            .then(|| 1.0 - wilson_lower(self.marks - self.agreed, self.marks, WILSON_Z_95))
    }

    /// The agreed share itself.
    fn share(&self) -> Option<f64> {
        share_of(self.agreed, self.marks)
    }

    /// The baseline's share over the line's marks, per thousand, floored as
    /// the judge floors it ([`crate::jev::promote::Agreement::baseline_share`]).
    #[must_use]
    pub fn baseline_permille(&self) -> Option<u16> {
        share_of(self.baseline_agreed, self.baseline_marks).map(permille)
    }

    /// The share of the answered requests the line acts on.
    #[must_use]
    pub fn apply_share(&self) -> Option<f64> {
        share_of(self.acted, self.answered)
    }

    /// How often, per thousand, the marks of the answers the line acts on
    /// said the answer was wrong.
    #[must_use]
    pub fn error_permille(&self) -> Option<u16> {
        error_of(self.marks, self.agreed)
    }

    /// How often the baseline was wrong on the same marks.
    #[must_use]
    pub fn baseline_error_permille(&self) -> Option<u16> {
        error_of(self.baseline_marks, self.baseline_agreed)
    }

    /// How often the marks of the answers the line leaves alone said wrong.
    #[must_use]
    pub fn under_error_permille(&self) -> Option<u16> {
        error_of(self.under_marks, self.under_agreed)
    }

    /// The line as a ledger-style row: camelCase, counts and per-thousands.
    #[must_use]
    pub fn json(&self) -> Value {
        json!({
            "fromPermille": self.from_permille,
            "marks": self.marks,
            "agreed": self.agreed,
            "lowerBoundPermille": self.lower_bound_permille(),
            "baselineMarks": self.baseline_marks,
            "baselineAgreed": self.baseline_agreed,
            "underMarks": self.under_marks,
            "underAgreed": self.under_agreed,
            "answered": self.answered,
            "acted": self.acted,
            "applyShare": self.apply_share(),
            "errorPermille": self.error_permille(),
            "baselineErrorPermille": self.baseline_error_permille(),
            "underErrorPermille": self.under_error_permille(),
        })
    }
}

/// `part` of `whole`, or `None` of nothing.
fn share_of(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| {
        #[allow(clippy::cast_precision_loss)]
        {
            part as f64 / whole as f64
        }
    })
}

/// The share of `marks` that did not agree, per thousand, floored.
fn error_of(marks: usize, agreed: usize) -> Option<u16> {
    (marks > 0)
        .then(|| u16::try_from(marks.saturating_sub(agreed) * 1_000 / marks).unwrap_or(1_000))
}

/// Why a seat has no act line of its own ([`calibrate`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoLine {
    /// Its graded answers carry no confidence they were given with — there
    /// is nothing to read a line off. A seat with no graded answer at all has
    /// too few marks ([`Line::TooFewCompared`]).
    NoConfidence,
    /// Every answer, taken whole, already passes what the judge asks of it:
    /// no line is wanted, and the seat stands or falls whole.
    Whole,
    /// No line splits the graded answers: each holds all of them or none.
    OneColour,
    /// A line's answers are, beyond their own interval, less often right
    /// than a lower line's: the seat's confidence runs against being right.
    NonMonotone,
    /// The line whose answers got furthest breaks this one of the judge's
    /// own lines there — too few marks, a label that never said no, a bound
    /// under the floor or under the baseline, or too small a share to act
    /// on ([`Line::ApplyShare`]).
    Line(Line),
    /// The line whose answers got furthest passes every one of the judge's
    /// lines but lifts the bound too little over every answer's
    /// ([`Calibration::lift_permille`]).
    NoLift {
        lift_permille: u16,
        wanted_permille: u16,
    },
}

impl NoLine {
    /// The word a row and a screen name the reason by — a judge's line by
    /// its own word.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::NoConfidence => "no_confidence",
            Self::Whole => "whole",
            Self::OneColour => "one_colour",
            Self::NonMonotone => "non_monotone",
            Self::Line(line) => line.token(),
            Self::NoLift { .. } => "no_lift",
        }
    }
}

/// A seat's graded answers read at every line of the grid, and the line
/// they say it acts from.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrated {
    /// The answers at each line of [`CALIBRATION`]'s grid, lowest first.
    pub grid: Vec<AtLine>,
    /// How many marks a line must hold before it can be the seat's: as many
    /// as its agreement line can be cleared on with the negatives inside
    /// ([`marks_that_can_clear`]).
    pub marks_wanted: usize,
    /// The act line, per thousand — or why there is none.
    pub line: Result<u16, NoLine>,
}

impl Calibrated {
    /// The grid's reading at `from_permille`, when it is a line of the grid.
    #[must_use]
    pub fn at(&self, from_permille: u16) -> Option<&AtLine> {
        self.grid
            .iter()
            .find(|at| at.from_permille == from_permille)
    }
}

/// What the judge asks of a set of marks, as `seat`'s row names it.
struct Asks {
    floor_permille: u16,
    negatives: usize,
    sample: usize,
    marks: usize,
    baseline: bool,
}

impl Asks {
    /// The first of the judge's own lines `at`'s answers do not clear, in
    /// the judge's order ([`crate::jev::promote::first_broken_line`]), with
    /// how far down that order it lies.
    fn broken(&self, at: &AtLine) -> Option<(usize, Line)> {
        if at.marks < self.marks {
            return Some((
                0,
                Line::TooFewCompared {
                    compared: at.marks,
                    wanted: self.marks,
                },
            ));
        }
        let disagreed = at.marks - at.agreed;
        if disagreed < self.negatives {
            return Some((
                1,
                Line::OneSided {
                    disagreed,
                    wanted: self.negatives,
                },
            ));
        }
        let bound = at.lower_bound_permille().unwrap_or(0);
        if bound < self.floor_permille {
            return Some((
                2,
                Line::Agreement {
                    bound_permille: bound,
                    floor_permille: self.floor_permille,
                },
            ));
        }
        if !self.baseline {
            return None;
        }
        if at.baseline_marks < self.sample {
            return Some((
                3,
                Line::TooFewBaseline {
                    compared: at.baseline_marks,
                    wanted: self.sample,
                },
            ));
        }
        let baseline = at.baseline_permille().unwrap_or(0);
        (bound <= baseline).then_some((
            4,
            Line::Baseline {
                bound_permille: bound,
                baseline_permille: baseline,
            },
        ))
    }
}

/// `seat`'s graded answers read at every line of [`CALIBRATION`]'s grid,
/// and the line they say it acts from (t-9468) — `None` for a seat that
/// never rises or names no bands, which has no line to move.
///
/// `graded` is the seat's record of graded answers
/// ([`crate::jev::promote::OnVersion::graded`]) and `answered` the
/// confidences of its answered requests
/// ([`crate::jev::promote::OnVersion::answered`]) — the same series the
/// judge reads.
#[must_use]
pub fn calibrate(seat: &JevUse, graded: &[Graded], answered: &[Option<f64>]) -> Option<Calibrated> {
    seat.confidence_bands?;
    let asks = Asks {
        floor_permille: seat.agreement_floor_permille?,
        negatives: seat.negatives_wanted?,
        sample: seat.agreement_rows_wanted?,
        marks: marks_that_can_clear(seat)?,
        baseline: seat.baseline.binds(),
    };
    let grid: Vec<AtLine> = CALIBRATION
        .grid
        .iter()
        .map(|from| AtLine::of(graded, answered, *from))
        .collect();
    let line = if grid.first().is_some_and(|whole| whole.marks == 0) && !graded.is_empty() {
        Err(NoLine::NoConfidence)
    } else {
        choose(&grid, &asks)
    };
    Some(Calibrated {
        grid,
        marks_wanted: asks.marks,
        line,
    })
}

/// The lowest line of `grid` whose answers pass, or why none does: the
/// premise first, then the line that got furthest down the judge's order —
/// the lowest of them where two got as far.
fn choose(grid: &[AtLine], asks: &Asks) -> Result<u16, NoLine> {
    let Some((whole, lines)) = grid.split_first() else {
        return Err(NoLine::NoConfidence);
    };
    if whole.marks == 0 {
        return Err(NoLine::Line(Line::TooFewCompared {
            compared: 0,
            wanted: asks.marks,
        }));
    }
    if asks.broken(whole).is_none() {
        return Err(NoLine::Whole);
    }
    if !lines
        .iter()
        .any(|at| at.marks > 0 && at.marks < whole.marks)
    {
        return Err(NoLine::OneColour);
    }
    if runs_against(grid, asks.negatives) {
        return Err(NoLine::NonMonotone);
    }
    let whole_bound = whole.lower_bound_permille().unwrap_or(0);
    let mut furthest: Option<(usize, NoLine)> = None;
    for at in lines {
        let (reached, why) = match asks.broken(at) {
            Some((reached, line)) => (reached, NoLine::Line(line)),
            None => {
                let lift = at
                    .lower_bound_permille()
                    .unwrap_or(0)
                    .saturating_sub(whole_bound);
                let share = at.apply_share().map_or(0, permille);
                if lift < CALIBRATION.lift_permille {
                    (
                        5,
                        NoLine::NoLift {
                            lift_permille: lift,
                            wanted_permille: CALIBRATION.lift_permille,
                        },
                    )
                } else if share < CALIBRATION.apply_share_floor_permille {
                    (
                        6,
                        NoLine::Line(Line::ApplyShare {
                            share_permille: share,
                            floor_permille: CALIBRATION.apply_share_floor_permille,
                        }),
                    )
                } else {
                    return Ok(at.from_permille);
                }
            }
        };
        if furthest.is_none_or(|(held, _)| reached > held) {
            furthest = Some((reached, why));
        }
    }
    Err(furthest.map_or(NoLine::OneColour, |(_, why)| why))
}

/// Whether some line's answers are, beyond their own interval, less often
/// right than a lower line's: the upper end of a higher line's 95% Wilson
/// interval under a lower line's agreed share. Read on the lines holding at
/// least `speaks` marks — as many as the label must have said no on — so a
/// line of one mark is no evidence either way.
fn runs_against(grid: &[AtLine], speaks: usize) -> bool {
    let speaking: Vec<&AtLine> = grid.iter().filter(|at| at.marks >= speaks.max(1)).collect();
    speaking.iter().enumerate().any(|(at, lower)| {
        lower.share().is_some_and(|share| {
            speaking[at + 1..]
                .iter()
                .any(|higher| higher.upper_bound().is_some_and(|upper| upper < share))
        })
    })
}

/// The file a seat's act line is kept in: one beside each ledger folder,
/// a row for each seat whose ledger is kept there (t-9468).
pub const THRESHOLDS_FILE: &str = "thresholds.json";

/// The keys a row of that file carries — camelCase, as a ledger row's are;
/// the rubric's is [`RUBRIC_VERSION`]'s.
pub const SEAT_KEY: &str = "seat";
pub const COMPUTED_AT_KEY: &str = "computedAtMs";
pub const MARKS_KEY: &str = "n";
pub const LOWER_BOUND_KEY: &str = "lowerBoundPermille";
pub const ACT_FROM_KEY: &str = "actFromPermille";
pub const APPLY_SHARE_KEY: &str = "applyShare";
pub const REASON_KEY: &str = "reason";
pub const GRID_KEY: &str = "grid";

/// The row `calibrated` is kept as for `seat`, read at `now_ms`: the line
/// and what it stands on — its marks, their lower bound, the share of the
/// answered requests it acts on — or no line and why, with every line of
/// the grid behind either.
#[must_use]
pub fn row(seat: &JevUse, calibrated: &Calibrated, now_ms: i64) -> Value {
    let line = calibrated.line.ok();
    let at = line
        .and_then(|from| calibrated.at(from))
        .or_else(|| calibrated.grid.first());
    json!({
        (SEAT_KEY): seat.id,
        (RUBRIC_VERSION.canonical): seat.rubric_version,
        (COMPUTED_AT_KEY): now_ms,
        (MARKS_KEY): at.map(|at| at.marks),
        (LOWER_BOUND_KEY): at.and_then(AtLine::lower_bound_permille),
        (ACT_FROM_KEY): line,
        (APPLY_SHARE_KEY): line.and(at).and_then(AtLine::apply_share),
        (REASON_KEY): calibrated.line.err().map(NoLine::token),
        (GRID_KEY): calibrated.grid.iter().map(AtLine::json).collect::<Vec<Value>>(),
    })
}

/// What the file beside a seat's ledger says of the seats' act lines, as
/// the product reads it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Thresholds {
    rows: Vec<Kept>,
}

/// One row as the product reads it: whose line, read under which words.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Kept {
    seat: String,
    rubric_version: u32,
    act_from_permille: Option<u16>,
}

impl Kept {
    /// A row of the file, or `None` for one that is not a row: no seat, no
    /// rubric that is a version, or a line that is not a per-thousand.
    fn of(row: &Value) -> Option<Self> {
        let act_from_permille = match row.get(ACT_FROM_KEY) {
            None | Some(Value::Null) => None,
            Some(line) => Some(
                line.as_u64()
                    .and_then(|line| u16::try_from(line).ok())
                    .filter(|line| *line <= 1_000)?,
            ),
        };
        Some(Self {
            seat: row.get(SEAT_KEY)?.as_str()?.to_string(),
            rubric_version: RUBRIC_VERSION
                .read(row)?
                .as_u64()
                .and_then(|version| u32::try_from(version).ok())?,
            act_from_permille,
        })
    }
}

impl Thresholds {
    /// The file's text read as the table: an array of rows. Anything else —
    /// no file, a torn write, another shape — is no table, and a row that is
    /// not one is no row; every seat they leave out acts as it always has.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let rows = serde_json::from_str::<Value>(text)
            .ok()
            .and_then(|table| {
                table
                    .as_array()
                    .map(|rows| rows.iter().filter_map(Kept::of).collect())
            })
            .unwrap_or_default();
        Self { rows }
    }

    /// The table kept in `dir` — the folder a seat's ledger is kept in.
    #[must_use]
    pub fn read_in(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join(THRESHOLDS_FILE))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// The act line the product reads for `seat`: its row's, while the row
    /// was read under the words the seat asks now and the seat's stage reads
    /// a line at all ([`JevUse::reads_act_line`]). `None` — the seat acts as
    /// it always has — for a seat with no such row, a row that drew no line,
    /// and a table holding two rows for one seat's words: a table that says
    /// two things says nothing.
    #[must_use]
    pub fn line_of(&self, seat: &JevUse) -> Option<u16> {
        if !seat.reads_act_line {
            return None;
        }
        let mut kept = self
            .rows
            .iter()
            .filter(|row| row.seat == seat.id && row.rubric_version == seat.rubric_version);
        let one = kept.next()?;
        kept.next()
            .is_none()
            .then_some(one.act_from_permille)
            .flatten()
    }
}

/// The act line kept beside `ledger` for `seat` ([`Thresholds::line_of`])
/// — what a stage about to act on an answer and a judge about to judge the
/// seat both read, from the one file beside the seat's own rows.
#[must_use]
pub fn line_beside(seat: &JevUse, ledger: &Path) -> Option<u16> {
    Thresholds::read_in(ledger.parent()?).line_of(seat)
}

#[cfg(test)]
mod tests;
