//! One reading of a score answer, for every seat that asks one.
//!
//! The wire type ([`SystemOneScoreAnswer`]) says an answer is score-shaped.
//! What it cannot say is that the score is the one THESE levels were asked
//! for: that the spread names the level numbers that were offered and no
//! others, that the probabilities are probabilities and sum to one, that the
//! score sits on the scale, and that it is the probability-weighted mean of
//! the level numbers the contract says it is.
//!
//! Those checks are arithmetic about the wire's rounding and the length of a
//! scale, and they are the same arithmetic whether the question was about a
//! recalled note or an installed skill. They live here, once, because the
//! alternative is each seat carrying its own copy of a tolerance — and a
//! tolerance that drifts in one copy is a seat that quietly refuses answers
//! its neighbour accepts.
//!
//! What does NOT live here is what a seat does with a reading: which levels
//! it offers, which readings it acts on, and how a whole batch is accepted or
//! discarded all belong to the seat.

use api::{SystemOneQuestionKind, SystemOneScoreAnswer};
// The grid the wire answers on, from the one crate both programs read.
use zerocode_core::jev::{ANSWER_STEP, WIRE_ROUNDING};

/// An ordered scale a score question offers, lowest level first.
///
/// Borrowed rather than owned: every scale in this workspace is a table
/// written down in full, and a scale built at runtime would be a rubric
/// nothing can be pinned against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scale {
    levels: &'static [&'static str],
}

impl Scale {
    /// The scale these level descriptions make.
    #[must_use]
    pub const fn new(levels: &'static [&'static str]) -> Self {
        Self { levels }
    }

    /// The level descriptions, as the request sends them.
    #[must_use]
    pub const fn levels(&self) -> &'static [&'static str] {
        self.levels
    }

    /// The top level's number — the upper end of the scale, and the divisor
    /// that puts a reading on 0 to 1.
    #[must_use]
    pub fn top(&self) -> f64 {
        self.ends().0
    }

    /// The top level's number and the level numbers added up.
    ///
    /// Counted up rather than cast from a length: a scale is a handful of
    /// levels, and a length converted to a float is the only lossy step this
    /// module would otherwise have.
    fn ends(&self) -> (f64, f64) {
        let (mut top, mut sum, mut number) = (0.0, 0.0, 0.0);
        for _ in self.levels {
            sum += number;
            top = number;
            number += 1.0;
        }
        (top, sum)
    }

    /// How many levels there are, as a float — counted up, for [`Self::ends`]'
    /// reason.
    fn count(&self) -> f64 {
        let mut count = 0.0;
        for _ in self.levels {
            count += 1.0;
        }
        count
    }

    /// How far the level probabilities may sum from one before the answer is
    /// refused.
    ///
    /// The contract says they sum to one, and they do — before the wire
    /// rounds them. Each arrives rounded, half an [`ANSWER_STEP`] at the
    /// most, so their sum can stand that many half-steps from one. The bound
    /// sits one more half-step out, and not on the last admissible distance
    /// itself, for [`Self::score_mean_tolerance`]'s reason.
    #[must_use]
    pub fn spread_sum_tolerance(&self) -> f64 {
        self.count() * WIRE_ROUNDING + WIRE_ROUNDING
    }

    /// How far a score may sit from the probability-weighted mean of its
    /// levels before the answer is refused.
    ///
    /// The contract says the two are the same number, and they are — before
    /// the wire rounds them. The rounding reaches the comparison twice:
    /// through the probabilities, because the mean is rebuilt from numbers
    /// each up to half a step out and a level's own number multiplies its
    /// error; and through the score, which was rounded too.
    ///
    /// The first term is the widest gap the rounding alone can open, and it
    /// lands ON the grid — both sides are whole numbers of steps, so the
    /// distance between them is one too. The second carries the bound half a
    /// step past the last distance it admits, where no double's last bit
    /// decides a gap the contract allows.
    #[must_use]
    pub fn score_mean_tolerance(&self) -> f64 {
        self.ends().1 * WIRE_ROUNDING + WIRE_ROUNDING
    }

    /// The reading a level's number is, on 0 to 1 — the cut a per-thousand
    /// floor in the use table is compared against.
    #[must_use]
    pub fn normalise(&self, score: f64) -> f64 {
        let top = self.top();
        if top > 0.0 { score / top } else { 0.0 }
    }

    /// Whether `score` reaches a line written in parts per thousand of the
    /// top level.
    ///
    /// Compared in the line's own units after flooring, as the promotion
    /// judge compares its bounds, so the answer does not turn on a float's
    /// last bit. Half a wire step is allowed back, because a score that was
    /// rounded down onto the grid must not be read as under a line it was
    /// exactly on.
    #[must_use]
    pub fn reaches(&self, score: f64, floor_permille: u16) -> bool {
        let reading = self.normalise(score) + WIRE_ROUNDING / self.top().max(1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let permille = (reading * 1_000.0).floor().clamp(0.0, 1_000.0) as u64;
        permille >= u64::from(floor_permille)
    }
}

/// Which of the answer rules a reply broke.
///
/// A closed set, and one word each, so a ledger row can say which check
/// refused a reply rather than saying `schema` for all of them. None of these
/// words comes from the reply or from anything a person wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreRule {
    /// The answer is not shaped as a score answer.
    NotAScore,
    /// `probabilities` or `legend` does not name exactly the levels offered.
    LevelKeys,
    /// A probability outside `[0, 1]`, or not a number at all.
    ProbabilityRange,
    /// The probabilities do not sum to one within the scale's tolerance.
    ProbabilitySum,
    /// `score` is missing, not finite, or outside the scale.
    ScoreRange,
    /// `score` is not the probability-weighted mean of the level numbers.
    ScoreMismatch,
    /// `confidence` outside `[0, 1]`.
    ConfidenceRange,
}

impl ScoreRule {
    /// The rule's own word, as a ledger row spells it.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::NotAScore => "not_a_score",
            Self::LevelKeys => "level_keys",
            Self::ProbabilityRange => "probability_range",
            Self::ProbabilitySum => "probability_sum",
            Self::ScoreRange => "score_range",
            Self::ScoreMismatch => "score_mismatch",
            Self::ConfidenceRange => "confidence_range",
        }
    }

    /// Every rule, so a contract can walk them.
    pub const ALL: [Self; 7] = [
        Self::NotAScore,
        Self::LevelKeys,
        Self::ProbabilityRange,
        Self::ProbabilitySum,
        Self::ScoreRange,
        Self::ScoreMismatch,
        Self::ConfidenceRange,
    ];
}

/// One checked answer: where it sits on the scale, and how sure of it the
/// judgment was.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreReading {
    /// The position along the levels, as answered.
    pub score: f64,
    /// The same reading on 0 to 1, so scales of different lengths compare.
    pub normalised: f64,
    pub confidence: f64,
}

/// Check one score answer against the scale it was asked on.
///
/// # Errors
/// The first rule the answer breaks.
pub fn read_score(
    answer: &SystemOneScoreAnswer,
    scale: &Scale,
) -> Result<ScoreReading, ScoreRule> {
    if answer.kind != SystemOneQuestionKind::Score {
        return Err(ScoreRule::NotAScore);
    }
    if !names_the_levels(answer, scale) {
        return Err(ScoreRule::LevelKeys);
    }
    let mut weighted = 0.0;
    let mut total = 0.0;
    // The level's own number, carried alongside rather than converted from the
    // index: the scale is short and a cast would be the only lossy step here.
    let mut number = 0.0;
    for level in 0..scale.levels.len() {
        let probability = answer
            .probabilities
            .get(&level.to_string())
            .copied()
            .ok_or(ScoreRule::LevelKeys)?;
        if !probability.is_finite() || !(0.0..=1.0).contains(&probability) {
            return Err(ScoreRule::ProbabilityRange);
        }
        weighted += probability * number;
        total += probability;
        number += 1.0;
    }
    if (total - 1.0).abs() > scale.spread_sum_tolerance() {
        return Err(ScoreRule::ProbabilitySum);
    }
    let top = scale.top();
    if !answer.score.is_finite() || !(0.0..=top).contains(&answer.score) {
        return Err(ScoreRule::ScoreRange);
    }
    // The contract says the score IS the weighted mean. Checking it is how a
    // reply built for some other scale is caught before it orders anything —
    // within what the wire's rounding can account for, and nothing more.
    if (answer.score - weighted).abs() > scale.score_mean_tolerance() {
        return Err(ScoreRule::ScoreMismatch);
    }
    if !answer.confidence.is_finite() || !(0.0..=1.0).contains(&answer.confidence) {
        return Err(ScoreRule::ConfidenceRange);
    }
    Ok(ScoreReading {
        score: answer.score,
        normalised: scale.normalise(answer.score),
        confidence: answer.confidence,
    })
}

/// Whether an answer is about the levels that were offered: the same level
/// numbers in its spread and in the legend it echoes back.
fn names_the_levels(answer: &SystemOneScoreAnswer, scale: &Scale) -> bool {
    answer.probabilities.len() == scale.levels.len()
        && answer.legend.len() == scale.levels.len()
        && (0..scale.levels.len()).all(|level| {
            let level = level.to_string();
            answer.probabilities.contains_key(&level) && answer.legend.contains_key(&level)
        })
}

/// What a reading below half the first step reads as: the bottom level.
///
/// `half` is where conventional rounding puts the boundary between the bottom
/// level and the one above it, and the cut sits half a wire step below it so
/// no number the wire can spell lands ON it: answers arrive on a grid of whole
/// [`ANSWER_STEP`]s and a half is one of them, so a cut at the midpoint would
/// have a double's last bit deciding which side of it a reading falls.
pub const BOTTOM_LEVEL_CUT: f64 = 0.5 - WIRE_ROUNDING;
const _: () = assert!(
    BOTTOM_LEVEL_CUT < 0.5 && 0.5 - BOTTOM_LEVEL_CUT < ANSWER_STEP,
    "the cut sits inside the last wire step below the midpoint, which is what \
     keeps every grid value on one side of it or the other"
);

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const FOUR: Scale = Scale::new(&["a", "b", "c", "d"]);
    const THREE: Scale = Scale::new(&["a", "b", "c"]);

    fn answer(scale: &Scale, spread: &[f64], score: f64, confidence: f64) -> SystemOneScoreAnswer {
        let probabilities: BTreeMap<String, f64> = spread
            .iter()
            .enumerate()
            .map(|(level, probability)| (level.to_string(), *probability))
            .collect();
        let legend: BTreeMap<String, serde_json::Value> = scale
            .levels()
            .iter()
            .enumerate()
            .map(|(level, words)| (level.to_string(), serde_json::json!(words)))
            .collect();
        SystemOneScoreAnswer {
            kind: SystemOneQuestionKind::Score,
            score,
            legend,
            probabilities,
            confidence,
        }
    }

    /// The tolerances the four-level scale derives are the numbers the recall
    /// rubric has carried since they were measured — the extraction changed no
    /// bound, which is the whole of what makes it safe.
    #[test]
    fn the_four_level_scale_derives_the_bounds_recall_was_measured_at() {
        assert!((FOUR.top() - 3.0).abs() < f64::EPSILON);
        assert!((FOUR.spread_sum_tolerance() - 0.025).abs() < 1e-12);
        assert!((FOUR.score_mean_tolerance() - 0.035).abs() < 1e-12);
    }

    /// A shorter scale sums fewer rounded numbers and weights them by smaller
    /// level numbers, so both bounds tighten — which is the point of deriving
    /// them from the scale rather than writing one pair down.
    #[test]
    fn a_shorter_scale_is_held_to_tighter_bounds() {
        assert!((THREE.top() - 2.0).abs() < f64::EPSILON);
        assert!((THREE.spread_sum_tolerance() - 0.02).abs() < 1e-12);
        assert!((THREE.score_mean_tolerance() - 0.02).abs() < 1e-12);
        assert!(THREE.spread_sum_tolerance() < FOUR.spread_sum_tolerance());
        assert!(THREE.score_mean_tolerance() < FOUR.score_mean_tolerance());
    }

    #[test]
    fn a_whole_answer_reads_back_on_the_scale_it_was_asked_on() {
        let reading = read_score(&answer(&THREE, &[0.0, 0.0, 1.0], 2.0, 0.9), &THREE)
            .expect("a well-formed answer");
        assert!((reading.score - 2.0).abs() < f64::EPSILON);
        assert!((reading.normalised - 1.0).abs() < f64::EPSILON);
        assert!((reading.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn each_rule_refuses_its_own_answer_and_says_which() {
        let broken = [
            (answer(&THREE, &[0.0, 0.0], 1.0, 0.5), ScoreRule::LevelKeys),
            (
                answer(&THREE, &[0.0, 2.0, -1.0], 1.0, 0.5),
                ScoreRule::ProbabilityRange,
            ),
            (
                answer(&THREE, &[0.1, 0.1, 0.1], 1.0, 0.5),
                ScoreRule::ProbabilitySum,
            ),
            (
                answer(&THREE, &[0.0, 0.0, 1.0], 9.0, 0.5),
                ScoreRule::ScoreRange,
            ),
            (
                answer(&THREE, &[0.0, 0.0, 1.0], 0.0, 0.5),
                ScoreRule::ScoreMismatch,
            ),
            (
                answer(&THREE, &[0.0, 0.0, 1.0], 2.0, 1.5),
                ScoreRule::ConfidenceRange,
            ),
        ];
        for (answer, rule) in broken {
            assert_eq!(read_score(&answer, &THREE), Err(rule));
        }
        let mut wrong_kind = answer(&THREE, &[0.0, 0.0, 1.0], 2.0, 0.5);
        wrong_kind.kind = SystemOneQuestionKind::Choice;
        assert_eq!(read_score(&wrong_kind, &THREE), Err(ScoreRule::NotAScore));
        let words: std::collections::BTreeSet<&str> =
            ScoreRule::ALL.iter().map(|rule| rule.word()).collect();
        assert_eq!(words.len(), ScoreRule::ALL.len(), "one word per rule");
    }

    /// The floor is read in the line's own units, and a score the wire rounded
    /// down onto the grid still reaches a line it was exactly on.
    #[test]
    fn a_line_per_thousand_is_read_in_its_own_units() {
        // 1.4 of a top level of 2 is 700 per thousand.
        assert!(THREE.reaches(1.4, 700));
        assert!(!THREE.reaches(1.39, 700));
        assert!(THREE.reaches(2.0, 700));
        assert!(!THREE.reaches(0.0, 700));
        // A floor of nothing is reached by everything, including a scale's
        // own floor value.
        assert!(THREE.reaches(0.0, 0));
    }
}
