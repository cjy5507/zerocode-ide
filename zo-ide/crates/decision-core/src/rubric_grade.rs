//! Pure parser for the goal **rubric grader's** verdict.
//!
//! A `/goal --check "<rubric>"` attaches a `ModelRubric` success criterion
//! that — unlike the objective cargo/git/grep validators — has no deterministic
//! check. It is graded by an *independent* model evaluator running in a fresh
//! context (the live goal loop spawns a one-shot grader sub-agent). The grader
//! returns a per-criterion judgement; this module folds it into the single
//! `Option<bool>` that flows into the goal-completion gate's **semantic**
//! channel ([`decide_goal_completion`](crate::goal_gate::decide_goal_completion)):
//! `Some(true)` accept, `Some(false)` reject, `None` no usable signal.
//!
//! `ModelRubric`: this is the validator variant in the live goal controller.
//!
//! The fold is deliberately conservative — [`fold_lens_verdicts`] under
//! [`ConsensusPolicy::AnyReject`]: every listed criterion must be met for an
//! accept, a single unmet criterion rejects, and an empty/unparseable grade is
//! `None` (never a silent accept), matching the goal gate's anti-optimistic
//! stance. Pure and total — unit tested in isolation.

use serde::Deserialize;

use crate::loop_fanout::{fold_lens_verdicts, ConsensusPolicy, LensVerdict};

/// The grader's response contract. `criteria` is the per-criterion judgement;
/// `pass`/`accepted` are an overall fallback for a grader that did not enumerate
/// criteria. All fields are optional so a partial/garbled reply degrades to
/// "no usable signal" rather than an error.
#[derive(Debug, Default, Deserialize)]
struct RubricGradeJson {
    #[serde(default)]
    criteria: Vec<RubricCriterion>,
    #[serde(default)]
    pass: Option<bool>,
    #[serde(default)]
    accepted: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct RubricCriterion {
    /// Whether this criterion is satisfied. Absent ⇒ the grader abstained on it.
    #[serde(default)]
    met: Option<bool>,
}

/// Fold a rubric grader's raw response into a goal-facing accept/reject signal.
///
/// Accepts the model text (optionally fenced or prose-wrapped); extracts the
/// JSON object and folds it. Per-criterion `met` flags fold under `AnyReject`
/// (all met ⇒ accept, any unmet ⇒ reject, all-abstain ⇒ no signal). When no
/// criteria are listed, an overall `pass`/`accepted` bool is used. Anything
/// unparseable is `None`.
#[must_use]
pub fn parse_rubric_grade(raw: &str) -> Option<bool> {
    let json = extract_json(raw)?;
    if !json.criteria.is_empty() {
        let verdicts: Vec<LensVerdict> = json
            .criteria
            .iter()
            .map(|criterion| match criterion.met {
                Some(true) => LensVerdict::Accept,
                Some(false) => LensVerdict::Reject,
                None => LensVerdict::Abstain,
            })
            .collect();
        return fold_lens_verdicts(&verdicts, ConsensusPolicy::AnyReject);
    }
    json.pass.or(json.accepted)
}

/// Deserialize the grader's JSON. Tries the trimmed text directly, then falls
/// back to the first `{` … last `}` span so a fenced/prose-wrapped reply still
/// parses (the grader is instructed to emit only the object, so the outer span
/// is safe). `None` if nothing parses.
fn extract_json(raw: &str) -> Option<RubricGradeJson> {
    let trimmed = raw.trim();
    if let Ok(parsed) = serde_json::from_str::<RubricGradeJson>(trimmed) {
        return Some(parsed);
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end < start {
        return None;
    }
    serde_json::from_str::<RubricGradeJson>(&trimmed[start..=end]).ok()
}

#[cfg(test)]
mod tests {
    use super::parse_rubric_grade;

    #[test]
    fn all_criteria_met_accepts() {
        let raw = r#"{"criteria":[{"name":"has summary","met":true},{"name":"cites sources","met":true}]}"#;
        assert_eq!(parse_rubric_grade(raw), Some(true));
    }

    #[test]
    fn a_single_unmet_criterion_rejects() {
        let raw = r#"{"criteria":[{"name":"has summary","met":true},{"name":"cites sources","met":false}]}"#;
        assert_eq!(parse_rubric_grade(raw), Some(false));
    }

    #[test]
    fn an_abstained_criterion_does_not_block_acceptance() {
        // met absent ⇒ Abstain; no Reject present ⇒ accept under AnyReject.
        let raw = r#"{"criteria":[{"name":"a","met":true},{"name":"b"}]}"#;
        assert_eq!(parse_rubric_grade(raw), Some(true));
    }

    #[test]
    fn fenced_or_prose_wrapped_json_still_parses() {
        let raw = "Here is my grade:\n```json\n{\"criteria\":[{\"name\":\"x\",\"met\":false}]}\n```\nDone.";
        assert_eq!(parse_rubric_grade(raw), Some(false));
    }

    #[test]
    fn overall_pass_is_used_when_no_criteria_listed() {
        assert_eq!(parse_rubric_grade(r#"{"pass":true}"#), Some(true));
        assert_eq!(parse_rubric_grade(r#"{"accepted":false}"#), Some(false));
    }

    #[test]
    fn empty_or_garbage_is_no_signal() {
        assert_eq!(parse_rubric_grade("not json at all"), None);
        assert_eq!(parse_rubric_grade(r#"{"criteria":[]}"#), None);
        assert_eq!(parse_rubric_grade(""), None);
    }
}
