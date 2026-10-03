//! Typed API adapter for the shared Jev score reader.
//! All scale arithmetic and validation live in `zerocode_core::jev::score`.

use api::{SystemOneQuestionKind, SystemOneScoreAnswer};
pub use zerocode_core::jev::score::{BOTTOM_LEVEL_CUT, Scale, ScoreReading, ScoreRule};

/// Check an API score without copying its spread or legend.
///
/// # Errors
/// Returns the shared reader's first broken answer rule.
pub fn read_score(answer: &SystemOneScoreAnswer, scale: &Scale<'_>) -> Result<ScoreReading, ScoreRule> {
    zerocode_core::jev::score::read_score(&zerocode_core::jev::score::ScoreAnswer {
        is_score: answer.kind == SystemOneQuestionKind::Score,
        score: answer.score,
        confidence: answer.confidence,
        probabilities: &answer.probabilities,
        legend: &answer.legend,
    }, scale)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const FOUR: Scale<'static> = Scale::new(&["a", "b", "c", "d"]);
    const THREE: Scale<'static> = Scale::new(&["a", "b", "c"]);

    fn answer(scale: &Scale<'_>, spread: &[f64], score: f64, confidence: f64) -> SystemOneScoreAnswer {
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
