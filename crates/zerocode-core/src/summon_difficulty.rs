//! A summons' difficulty, learned from the coordinator's effort pin.
//! Comparison marks measure agreement with that teacher, not the outcome of
//! an effort the worker never ran. Execution outcomes belong to a later note.
use std::collections::BTreeSet;

use crate::jev::{Cap, choice};
use serde_json::{Value, json};

pub const RUBRIC_VERSION: u32 = 1;
pub const SPEC_CHAR_CAP: usize = 400;
pub const TITLE_CHAR_CAP: usize = crate::summon_choice::SUMMON_RECENT_BRIEF_CHAR_CAP;
pub const APPLY_DEADLINE_MS: u64 = 2_000;
pub const QUESTION: &str = "summon_difficulty";

/// One ladder: option, meaning, and launch effort outside the question.
pub const LADDER: [(&str, &str, &str); 3] = [
    (
        "low",
        "Documentation, translation, tests only, or a small fix.",
        "medium",
    ),
    (
        "mid",
        "A UI feature, tooling, benchmarking, or a TUI port.",
        "high",
    ),
    (
        "high",
        "Core rules, protocols, concurrency, or ledger contracts.",
        "max",
    ),
];
const INSTRUCTIONS: &str = "Choose the difficulty of the work in `title` and `spec` (the first 400 characters). Judge its scope and reasoning requirements using the option meanings. `attempt` counts earlier summonses of this task, `failures` counts consecutive failures, and `retryOf` says whether this summons replaces an ended attempt. These are context, not automatic reasons to choose the highest level. Choose the lowest level adequate for the work.";
const STATE_KEYS: [&str; 5] = ["title", "spec", "attempt", "failures", "retryOf"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub title: String,
    pub spec: String,
    pub attempt: usize,
    pub failures: u32,
    pub retry_of: bool,
}
impl Look {
    #[must_use]
    pub fn state(&self) -> Value {
        json!({
            STATE_KEYS[0]: crate::jev::brief_shape(&self.title, Cap::Chars(TITLE_CHAR_CAP)).0,
            STATE_KEYS[1]: crate::jev::brief_shape(&self.spec, Cap::Chars(SPEC_CHAR_CAP)).0,
            STATE_KEYS[2]: self.attempt,
            STATE_KEYS[3]: self.failures,
            STATE_KEYS[4]: self.retry_of,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shadow {
    pub look: Look,
    /// Only the coordinator's explicit effort; never an applied answer.
    pub teacher_effort: Option<String>,
    /// The acting request's receipt. Its presence prevents a second request.
    pub receipt: Option<Value>,
}

#[must_use]
pub fn questions() -> Value {
    choice::asked(
        QUESTION,
        INSTRUCTIONS,
        LADDER
            .iter()
            .map(|(key, means, _)| ((*key).to_string(), json!(means)))
            .collect(),
    )
}

pub fn read(answers: &Value) -> Result<choice::Choice, choice::ChoiceRefusal> {
    let offered: BTreeSet<String> = LADDER
        .iter()
        .map(|(key, _, _)| (*key).to_string())
        .collect();
    choice::read(answers, QUESTION, &offered)
}

/// Reverse the ladder for the teacher. Unsupported pins remain ungraded.
#[must_use]
pub fn teacher(effort: &str) -> Option<&'static str> {
    let effort = if effort == "xhigh" {
        LADDER[2].2
    } else {
        effort
    };
    LADDER
        .iter()
        .find_map(|(key, _, rung)| (*rung == effort).then_some(*key))
}

#[must_use]
pub fn effort(difficulty: &str) -> Option<&'static str> {
    LADDER
        .iter()
        .find_map(|(key, _, effort)| (*key == difficulty).then_some(*effort))
}

/// Grade the independent answer against the pin, with the same-task baseline.
pub fn compare(row: &mut Value, teacher_effort: Option<&str>) {
    let teacher = teacher_effort.and_then(teacher);
    row["teacher"] = json!(teacher);
    if let (Some(teacher), Some(chosen)) = (teacher, row["chosen"].as_str()) {
        row[crate::jev::summary::AGREED.canonical] = json!(chosen == teacher);
        row[crate::jev::summary::BASELINE_AGREED.canonical] = json!(teacher == LADDER[2].0);
    } else {
        row[crate::jev::summary::NOT_COMPARED.canonical] = json!("no_teacher");
    }
}

#[must_use]
pub fn rubric_words() -> String {
    std::iter::once(INSTRUCTIONS)
        .chain(STATE_KEYS)
        .chain(LADDER.iter().flat_map(|(key, means, _)| [*key, *means]))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rubric_version_is_pinned_to_question_option_meanings_and_evidence_fields() {
        assert_eq!(RUBRIC_VERSION, 1);
        assert_eq!(
            crate::jev::rubric_fingerprint(rubric_words),
            "cfd9c076bf3f6257"
        );
    }
    #[test]
    fn comparison_uses_the_pin_and_can_beat_always_high_without_counterfactuals() {
        let mut row = json!({"chosen": LADDER[0].0});
        compare(&mut row, Some(LADDER[0].2));
        assert_eq!(
            (row["agreed"].as_bool(), row["baselineAgreed"].as_bool()),
            (Some(true), Some(false))
        );
        compare(&mut row, Some(LADDER[2].2));
        assert_eq!(
            (row["agreed"].as_bool(), row["baselineAgreed"].as_bool()),
            (Some(false), Some(true))
        );
        assert_eq!(teacher("xhigh"), Some(LADDER[2].0));
        let mut row = json!({"chosen": LADDER[0].0});
        compare(&mut row, None);
        assert_eq!(row["notCompared"], "no_teacher");
        assert!(row.get("agreed").is_none());
    }
    #[test]
    fn evidence_is_bounded_and_the_pin_never_enters_the_question() {
        let look = Look {
            title: "task".into(),
            spec: "가".repeat(SPEC_CHAR_CAP + 1),
            attempt: 2,
            failures: 1,
            retry_of: true,
        };
        let state = look.state();
        assert_eq!(
            state["spec"].as_str().unwrap().chars().count(),
            SPEC_CHAR_CAP
        );
        assert_eq!(state.as_object().unwrap().len(), STATE_KEYS.len());
        assert_eq!(
            questions()[QUESTION]["criteria"].as_object().unwrap().len(),
            LADDER.len()
        );
    }
}
