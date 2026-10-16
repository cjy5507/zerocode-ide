//! Where a label's verdict came from, and the bars a seat's rise is read
//! against (t-26584, ADR-150 / ADR-171 / ADR-176).
//!
//! The judge used to take four things on trust. Each is read here instead.
//!
//! 1. The source of a label ([`LabelSource`]). An executed outcome beats a
//!    judgment, and a judgment beats a structural proxy — a pane a person
//!    moved later, a file read later. A proxy never raises a seat, and a label
//!    that names no source counts as no label at all. A mark on a request row
//!    is the seat's own judgment set against the reader it replaces, so it
//!    needs no source of its own.
//! 2. The rise bar ([`PROMOTION_BAR`]): at least thirty pairs of the current
//!    judgment and the candidate, a quality gain of more than two percent, a
//!    cost per decision that rises by less than one percent, and a p95
//!    latency that rises by less than five percent. A pair written exactly on
//!    a line is not inside it.
//! 3. The calibration an act line rests on ([`CALIBRATION_CHECK`]): the line
//!    must have been drawn (`Line::Calibration` `NoRecord`), its acted
//!    answers must hold a lower bound, the answers it leaves out must stay
//!    under the abstain ceiling, and the answers it acts on must beat the
//!    cheapest reader by a lift and a margin.
//! 4. A fixed human bundle ([`bundle_of`]): the rows a person judged, pinned
//!    by a hash the ledger carries, with a canary slice the rise must clear
//!    too. A line fitted to the seat's own answers cannot pass on its own
//!    bench, because the bundle it is checked against is not the seat's.
//!
//! Every bar here holds a rise and none takes an acting seat back, the same
//! as the sample floor (`Line::only_holds`): a missing record is a gap in
//! the evidence, not a mark against the seat.

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::jev::promote::permille;
use crate::jev::summary::{
    AGREED, BASELINE_AGREED, CURRENT_ELAPSED_MS, CURRENT_INPUT_TOKENS, ELAPSED_MS, INPUT_TOKENS,
    JUDGED_EVERY_ROWS, LABEL, LABEL_AUTHOR, LABEL_PART, LABEL_SOURCE, WILSON_Z_95,
    grades_a_request, wilson_lower,
};
use crate::jev::threshold::Graded;
use crate::jev::{CurrentSide, JevUse};

/// A label's source, best first (ADR-171). A label names one
/// ([`LABEL_SOURCE`]); a label that names none is no label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    /// Something the product ran and watched finish: a receipt, a measured
    /// hand, a tried move.
    Executed,
    /// A model's judgment, held to the answer it graded.
    Judged,
    /// A structural stand-in for the answer: a pane a person moved later, a
    /// file read later, a notice answered later.
    Proxy,
    /// The person's own judgment of a request: the rows the human bundle is
    /// made of. Never an agreement mark — that is the bundle's alone.
    Person,
}

impl LabelSource {
    /// Every source, in the order a reader lists them.
    pub const ALL: [Self; 4] = [Self::Executed, Self::Judged, Self::Proxy, Self::Person];

    /// The word a row carries for this source.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Executed => "executed",
            Self::Judged => "judged",
            Self::Proxy => "proxy",
            Self::Person => "person",
        }
    }

    /// The source a word names, if it names one.
    #[must_use]
    pub fn from_token(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.token() == word)
    }

    /// Whether a label of this source may raise a seat: an execution or a
    /// judgment, never a proxy (ADR-171).
    #[must_use]
    pub const fn may_raise(self) -> bool {
        matches!(self, Self::Executed | Self::Judged)
    }
}

/// The source `row`'s label names, when it names a known one.
#[must_use]
pub fn source_of(row: &Value) -> Option<LabelSource> {
    LABEL_SOURCE
        .read(row)
        .and_then(Value::as_str)
        .and_then(LabelSource::from_token)
}

/// Whether `row`'s mark may count toward agreement (t-26584, ①). A row that
/// names a source counts when that source may raise a seat. A row that names
/// none counts when it is a request's own mark — the seat's judgment set
/// against the reader it replaces — and never when it is a label, because a
/// label that names no source is no label.
#[must_use]
pub fn counts_toward_agreement(row: &Value) -> bool {
    match source_of(row) {
        Some(source) => source.may_raise(),
        None => !grades_a_request(row),
    }
}

/// The rise bar every seat is read on (ADR-150): the paired records a rise
/// needs, and how far the candidate may gain or cost or slow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromotionBar {
    /// Pairs the bar reads on, at least.
    pub min_pairs: usize,
    /// The quality gain, per thousand of pairs, the candidate must exceed.
    pub quality_over_permille: u16,
    /// The rise in cost per decision, per thousand, the candidate must stay
    /// under where its current side is a model ([`CurrentSide`]).
    pub cost_rise_under_permille: u16,
    /// The rise in p95 latency, per thousand, the candidate must stay under.
    pub latency_rise_under_permille: u16,
}

/// The bar ADR-150 names. A policy line from that decision, not a fitted one.
pub const PROMOTION_BAR: PromotionBar = PromotionBar {
    min_pairs: 30,
    quality_over_permille: 20,
    cost_rise_under_permille: 10,
    latency_rise_under_permille: 50,
};

/// The paired records a rise is read on (ADR-150): the rows that carry the
/// candidate's mark and the current judgment's mark on the same fact, and —
/// among those — the rows that also carry both costs and both latencies.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PairTally {
    /// Rows carrying both marks.
    pub pairs: usize,
    /// Of those, the candidate was right ([`AGREED`]).
    pub candidate_right: usize,
    /// Of those, the current judgment was right ([`BASELINE_AGREED`]).
    pub current_right: usize,
    /// Pairs that also carry both costs and both latencies.
    pub priced: usize,
    /// Input tokens over the priced pairs, candidate and current.
    pub candidate_tokens: u64,
    /// See [`Self::candidate_tokens`].
    pub current_tokens: u64,
    /// The p95 of the priced pairs' candidate latencies, in milliseconds.
    pub candidate_p95_ms: u64,
    /// The p95 of the priced pairs' current latencies, in milliseconds.
    pub current_p95_ms: u64,
}

/// The paired records `rows` carry ([`PairTally`]). A row that names a source
/// which may not raise a seat is no pair (ADR-171).
#[must_use]
pub fn pair_tally<'rows>(rows: impl IntoIterator<Item = &'rows Value>) -> PairTally {
    let mut tally = PairTally::default();
    let (mut candidate_ms, mut current_ms) = (Vec::new(), Vec::new());
    for row in rows {
        if !counts_toward_agreement(row) {
            continue;
        }
        let (Some(candidate), Some(current)) = (
            AGREED.read(row).and_then(Value::as_bool),
            BASELINE_AGREED.read(row).and_then(Value::as_bool),
        ) else {
            continue;
        };
        tally.pairs += 1;
        tally.candidate_right += usize::from(candidate);
        tally.current_right += usize::from(current);
        if let (Some(tokens), Some(current_tokens), Some(ms), Some(current_ms_of_row)) = (
            INPUT_TOKENS.read(row).and_then(Value::as_u64),
            CURRENT_INPUT_TOKENS.read(row).and_then(Value::as_u64),
            ELAPSED_MS.read(row).and_then(Value::as_u64),
            CURRENT_ELAPSED_MS.read(row).and_then(Value::as_u64),
        ) {
            tally.priced += 1;
            tally.candidate_tokens += tokens;
            tally.current_tokens += current_tokens;
            candidate_ms.push(ms);
            current_ms.push(current_ms_of_row);
        }
    }
    tally.candidate_p95_ms = p95(&candidate_ms);
    tally.current_p95_ms = p95(&current_ms);
    tally
}

/// Why the paired records do not clear the bar (ADR-150).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarMiss {
    /// Fewer pairs than the bar reads on.
    Pairs { pairs: usize, wanted: usize },
    /// Enough pairs, but too few carry the costs and latencies the bar reads.
    Unpriced { priced: usize, wanted: usize },
    /// The candidate is not right over the current judgment by the margin.
    Quality {
        gain_permille: i64,
        over_permille: u16,
    },
    /// Cost per decision rose by the bar or more over a model's current side.
    Cost {
        candidate: u64,
        current: u64,
        under_permille: u16,
    },
    /// p95 latency rose by the bar or more.
    Latency {
        candidate_ms: u64,
        current_ms: u64,
        under_permille: u16,
    },
}

impl BarMiss {
    /// The word a screen names this miss by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Pairs { .. } => "pairs",
            Self::Unpriced { .. } => "unpriced",
            Self::Quality { .. } => "quality",
            Self::Cost { .. } => "cost",
            Self::Latency { .. } => "latency",
        }
    }
}

/// The first bar `pairs` does not clear, in the order ADR-150 lists them, or
/// `None` when it clears them all. The cost line reads a seat whose current
/// side is a model (t-26584, decision 1a): a rule spends no tokens, so beside
/// it any answer would be an infinite rise. The comparisons are exact: a rise
/// written exactly on a line is not under it.
#[must_use]
pub fn bar_miss(pairs: &PairTally, bar: &PromotionBar, side: CurrentSide) -> Option<BarMiss> {
    if pairs.pairs < bar.min_pairs {
        return Some(BarMiss::Pairs {
            pairs: pairs.pairs,
            wanted: bar.min_pairs,
        });
    }
    if pairs.priced < bar.min_pairs {
        return Some(BarMiss::Unpriced {
            priced: pairs.priced,
            wanted: bar.min_pairs,
        });
    }
    let count = wide(pairs.pairs);
    let gain = (wide(pairs.candidate_right) - wide(pairs.current_right)) * 1_000;
    if gain <= i128::from(bar.quality_over_permille) * count {
        return Some(BarMiss::Quality {
            gain_permille: i64::try_from(gain / count).unwrap_or(i64::MIN),
            over_permille: bar.quality_over_permille,
        });
    }
    if side == CurrentSide::Model
        && !rises_under(
            pairs.candidate_tokens,
            pairs.current_tokens,
            bar.cost_rise_under_permille,
        )
    {
        return Some(BarMiss::Cost {
            candidate: pairs.candidate_tokens,
            current: pairs.current_tokens,
            under_permille: bar.cost_rise_under_permille,
        });
    }
    if !rises_under(
        pairs.candidate_p95_ms,
        pairs.current_p95_ms,
        bar.latency_rise_under_permille,
    ) {
        return Some(BarMiss::Latency {
            candidate_ms: pairs.candidate_p95_ms,
            current_ms: pairs.current_p95_ms,
            under_permille: bar.latency_rise_under_permille,
        });
    }
    None
}

/// Whether `candidate` is at or under `current`, or rises over it by less
/// than `limit_permille` per thousand of it.
fn rises_under(candidate: u64, current: u64, limit_permille: u16) -> bool {
    candidate <= current
        || (wide64(candidate) - wide64(current)) * 1_000
            < i128::from(limit_permille) * wide64(current)
}

/// The nearest-rank p95 of `samples` (zero for none).
fn p95(samples: &[u64]) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = (sorted.len() * 95).div_ceil(100).max(1);
    sorted.get(rank - 1).copied().unwrap_or(0)
}

fn wide(count: usize) -> i128 {
    i128::try_from(count).unwrap_or(i128::MAX)
}

fn wide64(value: u64) -> i128 {
    i128::from(value)
}

/// The checks an act line must pass before a rise is written (t-26584, ③):
/// the typesafe-router's abstain, lift and margin lines and the record count
/// and lower bound the router-calibrator reads the line with. Policy lines,
/// not fitted ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationCheck {
    /// Marks the line acts on, at least.
    pub min_records: usize,
    /// The lower bound of the acted answers' agreement, per thousand, at least.
    pub lower_bound_floor_permille: u16,
    /// The share of answered requests the line leaves out, per thousand, at
    /// most: a router that abstains more than this is not deciding.
    pub abstain_ceiling_permille: u16,
    /// The acted answers' agreement over the cheapest reader's, as a ratio in
    /// thousandths (1.2 is 1200), at least.
    pub lift_floor_milli: u32,
    /// The acted answers' agreement minus the cheapest reader's, per thousand,
    /// at least.
    pub margin_floor_permille: u16,
}

/// The check every act line passes (typesafe-router's and router-calibrator's
/// lines, t-26584, ③).
pub const CALIBRATION_CHECK: CalibrationCheck = CalibrationCheck {
    min_records: 30,
    lower_bound_floor_permille: 600,
    abstain_ceiling_permille: 300,
    lift_floor_milli: 1_200,
    margin_floor_permille: 5,
};

/// What an act line's answers say, read off a window (t-26584, ③).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Acted {
    /// Answered requests in the window, acted on or not.
    pub answered: usize,
    /// Of those, the ones the line acts on.
    pub acted_answers: usize,
    /// Marks on acted answers.
    pub marks: usize,
    /// Of those, the ones the judgment got right.
    pub agreed: usize,
    /// Acted marks that also carry the cheapest reader's mark.
    pub paired_marks: usize,
    /// Of those, the judgment got right.
    pub paired_agreed: usize,
    /// Of those, the cheapest reader got right.
    pub paired_baseline_agreed: usize,
}

impl Acted {
    /// Read `answered` (each answer's confidence) and `graded` (each mark's
    /// graded answer) against `line`.
    #[must_use]
    pub fn of(line: u16, answered: &[Option<f64>], graded: &[Graded]) -> Self {
        let mut acted = Self {
            answered: answered.len(),
            acted_answers: answered
                .iter()
                .flatten()
                .filter(|confidence| crate::jev::reaches(**confidence, line))
                .count(),
            ..Self::default()
        };
        for mark in graded {
            if !mark
                .confidence
                .is_some_and(|c| crate::jev::reaches(c, line))
            {
                continue;
            }
            acted.marks += 1;
            acted.agreed += usize::from(mark.agreed);
            if let Some(baseline) = mark.baseline {
                acted.paired_marks += 1;
                acted.paired_agreed += usize::from(mark.agreed);
                acted.paired_baseline_agreed += usize::from(baseline);
            }
        }
        acted
    }
}

/// Why an act line is not calibrated, or its record does not clear the check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationMiss {
    /// The seat has no line drawn for its words.
    NoRecord,
    /// Fewer acted marks than the check asks.
    Records { records: usize, wanted: usize },
    /// The acted answers' lower bound is under the floor.
    LowerBound {
        bound_permille: u16,
        floor_permille: u16,
    },
    /// The line leaves out more of the answers than the ceiling allows.
    Abstain {
        share_permille: u16,
        ceiling_permille: u16,
    },
    /// The acted answers do not beat the cheapest reader by the lift.
    Lift { lift_milli: u32, floor_milli: u32 },
    /// The acted answers beat the cheapest reader by less than the margin.
    Margin {
        margin_permille: i32,
        floor_permille: u16,
    },
}

impl CalibrationMiss {
    /// The word a screen names this miss by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::NoRecord => "no_record",
            Self::Records { .. } => "records",
            Self::LowerBound { .. } => "lower_bound",
            Self::Abstain { .. } => "abstain",
            Self::Lift { .. } => "lift",
            Self::Margin { .. } => "margin",
        }
    }
}

/// The first check `acted` does not pass for an act line drawn at `line`, or
/// `None` when it passes them all. The lift and the margin read the paired
/// marks: the judgment's and the cheapest reader's over the same facts, so
/// the lift is the ratio of their right counts and the margin their
/// difference over the pairs.
#[must_use]
pub fn calibration_miss(
    line: Option<u16>,
    acted: &Acted,
    baseline_binds: bool,
) -> Option<CalibrationMiss> {
    let check = CALIBRATION_CHECK;
    if line.is_none() {
        return Some(CalibrationMiss::NoRecord);
    }
    if acted.marks < check.min_records {
        return Some(CalibrationMiss::Records {
            records: acted.marks,
            wanted: check.min_records,
        });
    }
    let bound = permille(wilson_lower(acted.agreed, acted.marks, WILSON_Z_95));
    if bound < check.lower_bound_floor_permille {
        return Some(CalibrationMiss::LowerBound {
            bound_permille: bound,
            floor_permille: check.lower_bound_floor_permille,
        });
    }
    let abstained = acted.answered.saturating_sub(acted.acted_answers);
    if acted.answered > 0
        && wide(abstained) * 1_000
            > i128::from(check.abstain_ceiling_permille) * wide(acted.answered)
    {
        return Some(CalibrationMiss::Abstain {
            share_permille: u16::try_from(abstained * 1_000 / acted.answered).unwrap_or(u16::MAX),
            ceiling_permille: check.abstain_ceiling_permille,
        });
    }
    // A seat with no baseline has no reader to beat: the lift and the margin
    // are not read against it (the judge holds such a seat to its floor).
    if !baseline_binds {
        return None;
    }
    if acted.paired_marks == 0 {
        return Some(CalibrationMiss::Lift {
            lift_milli: 0,
            floor_milli: check.lift_floor_milli,
        });
    }
    let (judged, baseline, paired) = (
        wide(acted.paired_agreed),
        wide(acted.paired_baseline_agreed),
        wide(acted.paired_marks),
    );
    let lift_clears = if baseline == 0 {
        judged > 0
    } else {
        judged * 1_000 >= i128::from(check.lift_floor_milli) * baseline
    };
    if !lift_clears {
        return Some(CalibrationMiss::Lift {
            lift_milli: if baseline == 0 {
                0
            } else {
                u32::try_from(judged * 1_000 / baseline).unwrap_or(u32::MAX)
            },
            floor_milli: check.lift_floor_milli,
        });
    }
    let difference = (judged - baseline) * 1_000;
    if difference < i128::from(check.margin_floor_permille) * paired {
        return Some(CalibrationMiss::Margin {
            margin_permille: i32::try_from(difference / paired).unwrap_or(i32::MIN),
            floor_permille: check.margin_floor_permille,
        });
    }
    None
}

/// The rules a fixed human bundle is read by (t-26584, ④, ADR-176).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundlePolicy {
    /// Person-judged rows the bundle holds, at least.
    pub min_rows: usize,
    /// One row in this many is canary: the rows held out of the fit and
    /// checked on their own.
    pub canary_every: u32,
    /// Canary rows the check reads on, at least.
    pub canary_min_rows: usize,
    /// The canary rows' agreement, per thousand, at least.
    pub canary_floor_permille: u16,
}

/// The policy every bundle is read by. The minimum is a window's worth of
/// judgments, the same one a judge waits for before it reads a seat at all.
pub const HUMAN_BUNDLE: BundlePolicy = BundlePolicy {
    min_rows: JUDGED_EVERY_ROWS,
    canary_every: 5,
    canary_min_rows: 5,
    canary_floor_permille: 600,
};

/// The kind a pin row carries: a row a calibrator writes beside a seat's
/// rows, naming the bundle its line was drawn against.
pub const PIN_KIND: &str = "bundle_pin";
/// The key a pin row carries its bundle's hash under.
pub const PIN_HASH_KEY: &str = "bundleSha256";

/// The person-judged rows of a ledger, read for one seat (t-26584, ④): how
/// many there are, whether the newest pin for the seat names their hash, and
/// the canary slice's agreement. Only the label and the mark are read — never
/// the words of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bundle {
    /// Person-judged rows.
    pub rows: usize,
    /// Whether the seat's newest pin row names this bundle's hash.
    pub pinned: bool,
    /// Canary rows the judgment got right.
    pub canary_agreed: usize,
    /// Canary rows the judgment has a mark on.
    pub canary_compared: usize,
}

/// The bundle `rows` carry for the seat `seat_id`.
#[must_use]
pub fn bundle_of(rows: &[Value], seat_id: &str, policy: &BundlePolicy) -> Bundle {
    let judged = judged_of(rows);
    let hash = bundle_hash(&judged);
    let pinned = rows
        .iter()
        .rev()
        .find(|row| is_pin_of(row, seat_id))
        .and_then(|row| row.get(PIN_HASH_KEY))
        .and_then(Value::as_str)
        == Some(hash.as_str());
    let canary = judged
        .iter()
        .filter(|(label, _)| is_canary(label, policy.canary_every));
    Bundle {
        rows: judged.len(),
        pinned,
        canary_agreed: canary.clone().filter(|(_, agreed)| *agreed).count(),
        canary_compared: canary.count(),
    }
}

/// The (label, mark) pairs of the person-judged rows, in label order. Of two
/// rows naming one request and one part of it ([`LABEL_PART`]), the newest
/// counts, as in the labels line ([`crate::jev::promote::person_labels`]): a
/// label a person changed is read once, as it reads now, and one taken back —
/// a newest row with neither mark — not at all.
fn judged_of(rows: &[Value]) -> Vec<(String, bool)> {
    let mut newest = HashMap::new();
    for row in rows {
        if source_of(row) != Some(LabelSource::Person) {
            continue;
        }
        let Some(label) = LABEL.read(row).and_then(Value::as_str) else {
            continue;
        };
        let part = LABEL_PART.read(row).and_then(Value::as_str);
        let agreed = match (
            AGREED.read(row).and_then(Value::as_bool),
            BASELINE_AGREED.read(row).and_then(Value::as_bool),
        ) {
            (Some(agreed), _) => Some(agreed),
            (None, None) => None,
            (None, Some(_)) => continue,
        };
        newest.insert((label, part), agreed);
    }
    let mut judged: Vec<(String, bool)> = newest
        .into_iter()
        .filter_map(|((label, _), agreed)| Some((label.to_string(), agreed?)))
        .collect();
    judged.sort();
    judged
}

/// The pin row a calibrator writes beside a seat's rows: it names the hash of
/// the person-judged rows it was drawn against (ADR-176, t-26584, ④).
#[must_use]
pub fn bundle_pin_for(rows: &[Value], seat_id: &str) -> Value {
    let judged = judged_of(rows);
    serde_json::json!({
        "kind": PIN_KIND,
        "seat": seat_id,
        PIN_HASH_KEY: bundle_hash(&judged),
        "rows": judged.len(),
    })
}

/// Why the human bundle does not hold a rise (ADR-176).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleMiss {
    /// Fewer person-judged rows than the bundle holds.
    Rows { rows: usize, wanted: usize },
    /// No pin row for the seat names the bundle's hash.
    Unpinned,
    /// The canary slice is too thin or does not agree enough.
    Canary {
        agreed: usize,
        compared: usize,
        floor_permille: u16,
    },
}

impl BundleMiss {
    /// The word a screen names this miss by.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Rows { .. } => "rows",
            Self::Unpinned => "unpinned",
            Self::Canary { .. } => "canary",
        }
    }
}

/// The first rule `bundle` does not clear under `policy`, or `None`.
#[must_use]
pub fn bundle_miss(bundle: &Bundle, policy: &BundlePolicy) -> Option<BundleMiss> {
    if bundle.rows < policy.min_rows {
        return Some(BundleMiss::Rows {
            rows: bundle.rows,
            wanted: policy.min_rows,
        });
    }
    if !bundle.pinned {
        return Some(BundleMiss::Unpinned);
    }
    let clears = bundle.canary_compared >= policy.canary_min_rows
        && wide(bundle.canary_agreed) * 1_000
            >= i128::from(policy.canary_floor_permille) * wide(bundle.canary_compared);
    (!clears).then_some(BundleMiss::Canary {
        agreed: bundle.canary_agreed,
        compared: bundle.canary_compared,
        floor_permille: policy.canary_floor_permille,
    })
}

/// The SHA-256 of the bundle's (label, mark) lines, in label order — the hash
/// a pin row names.
fn bundle_hash(judged: &[(String, bool)]) -> String {
    let mut hasher = Sha256::new();
    for (label, agreed) in judged {
        hasher.update(label.as_bytes());
        hasher.update([b'\t', if *agreed { b'1' } else { b'0' }, b'\n']);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Whether `label` is one of the canary rows: a stable slice of the bundle
/// by the hash of its label, one row in `every`.
fn is_canary(label: &str, every: u32) -> bool {
    every > 0 && u32::from(Sha256::digest(label.as_bytes())[0]) % every == 0
}

/// Whether `row` is the pin row a calibrator wrote for `seat_id`.
fn is_pin_of(row: &Value, seat_id: &str) -> bool {
    row.get("kind").and_then(Value::as_str) == Some(PIN_KIND)
        && row.get("seat").and_then(Value::as_str) == Some(seat_id)
}

/// Stamp a label row with where its verdict came from and which writer put it
/// in the ledger (t-26584, ①). Every writer of a label calls this as its row
/// leaves it.
pub fn stamp_label(row: &mut Value, source: LabelSource, author: &str) {
    row[LABEL_SOURCE.canonical] = Value::from(source.token());
    row[LABEL_AUTHOR.canonical] = Value::from(author);
}

/// Where each seat's labels come from, by the seat's id (t-26584, ①). A label
/// that names no source of its own takes its seat's entry when it is written
/// to the seat's ledger ([`stamp_for_ledger`]). A seat is a proxy until a
/// writer shows its label is the execution's own result: only the entries
/// marked [`LabelSource::Executed`] were read as an outcome the product ran
/// and watched finish, and a writer that mixes both writes the source of each
/// row beside it.
pub const SEAT_LABEL_SOURCE: &[(&str, LabelSource)] = &[
    ("branching", LabelSource::Proxy),
    ("browser", LabelSource::Proxy),
    ("browser_read", LabelSource::Proxy),
    ("challenger", LabelSource::Executed),
    ("claim", LabelSource::Executed),
    ("command_guard", LabelSource::Executed),
    ("compaction", LabelSource::Proxy),
    ("cover", LabelSource::Executed),
    ("desktop", LabelSource::Proxy),
    ("effort", LabelSource::Proxy),
    ("emulator", LabelSource::Proxy),
    ("file_pick", LabelSource::Proxy),
    ("judgment_cache", LabelSource::Proxy),
    ("mention_rerank", LabelSource::Proxy),
    // Records only and never rises (t-26584, decision 6): nothing writes its
    // labels yet, and one would grade a tier against the work later seen on
    // that model — an outcome watched afterwards, not the placement's own run.
    ("model_tiers", LabelSource::Proxy),
    ("notify", LabelSource::Proxy),
    ("patch_review", LabelSource::Proxy),
    ("placement", LabelSource::Proxy),
    ("recall", LabelSource::Proxy),
    ("reflex_decide", LabelSource::Executed),
    ("routing", LabelSource::Proxy),
    ("skill_suggestion", LabelSource::Proxy),
    ("skills", LabelSource::Proxy),
    ("stall", LabelSource::Proxy),
    ("step_effort", LabelSource::Proxy),
    ("summon", LabelSource::Proxy),
    ("summon_difficulty", LabelSource::Executed),
    ("summon_model", LabelSource::Executed),
    ("tool_text_guard", LabelSource::Proxy),
];

/// The seat's source for its labels, by the seat's id ([`SEAT_LABEL_SOURCE`]).
#[must_use]
pub fn seat_label_source(seat_id: &str) -> Option<LabelSource> {
    SEAT_LABEL_SOURCE
        .iter()
        .find(|(id, _)| *id == seat_id)
        .map(|(_, source)| *source)
}

/// The one seat whose ledger is `ledger`, by its file name. A file two seats
/// name names no author, so it names none.
#[must_use]
pub fn seat_of_ledger(ledger: &Path) -> Option<&'static JevUse> {
    let name = ledger.file_name()?.to_str()?;
    let mut seats = crate::jev::JEV_USES
        .iter()
        .filter(|seat| seat.ledger == name);
    let seat = seats.next()?;
    seats.next().is_none().then_some(seat)
}

/// Whether `row` is a label that names no source of its own: the only rows
/// [`stamp_for_ledger`] changes.
#[must_use]
pub fn needs_source(row: &Value) -> bool {
    grades_a_request(row) && LABEL_SOURCE.read(row).is_none()
}

/// Stamp a label written to `ledger` with its seat's source and author, when
/// the writer named no source of its own (t-26584, ①). A row that is not a
/// label, or that names a source, is left as it is.
pub fn stamp_for_ledger(ledger: &Path, row: &mut Value) {
    if !needs_source(row) {
        return;
    }
    let Some(seat) = seat_of_ledger(ledger) else {
        return;
    };
    let Some(source) = seat_label_source(seat.id) else {
        return;
    };
    stamp_label(row, source, seat.id);
}

#[cfg(test)]
mod tests;
