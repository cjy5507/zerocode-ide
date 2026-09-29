//! A summons' difficulty and editable launch profiles.
//! Pins describe the executed baseline; only finished work grades a choice.
use std::collections::BTreeSet;

use crate::jev::{Cap, choice};
use serde_json::{Value, json};

pub const RUBRIC_VERSION: u32 = 2;
pub mod lineup;
pub mod outcomes;
pub const PROFILES_SETTING: &str = "summonProfiles";
pub const DEFAULT_PROFILES: &str = include_str!("summon-profiles.json");
pub const FALLBACK_DIFFICULTY: &str = LADDER[1].0;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub model: String,
    pub effort: String,
}

#[must_use]
pub fn profiles(root: &Value) -> Value {
    root.get(crate::jev::SMART_SETTINGS_KEY)
        .and_then(|s| s.get(PROFILES_SETTING))
        .cloned()
        .unwrap_or_else(|| serde_json::from_str(DEFAULT_PROFILES).unwrap_or_default())
}

/// Only the rows a person wrote (`{}` when none) — what a settings screen
/// fills its fields with, leaving every other row to follow the lineup.
#[must_use]
pub fn written_profiles(root: &Value) -> Value {
    root.get(crate::jev::SMART_SETTINGS_KEY)
        .and_then(|s| s.get(PROFILES_SETTING))
        .filter(|table| table.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

/// A settings table holds only the rows a person wrote: any difficulty may be
/// left out (it follows the lineup), and every row that is there must launch.
pub fn validate_profiles(table: &Value) -> Result<(), String> {
    let agents = table
        .as_object()
        .ok_or_else(|| format!("{PROFILES_SETTING}: expected an object"))?;
    for (agent, rows) in agents {
        if !crate::agent::AGENT_SPECS.iter().any(|s| s.id == agent) {
            return Err(format!("unknown agent: {agent}"));
        }
        let rows = rows
            .as_object()
            .ok_or_else(|| format!("{agent}: expected an object of difficulties"))?;
        for (difficulty, row) in rows {
            if effort(difficulty).is_none() {
                return Err(format!("{agent}: unknown difficulty {difficulty}"));
            }
            let profile: Profile =
                serde_json::from_value(row.clone()).map_err(|e| format!("{agent}: {e}"))?;
            launchable(agent, &profile)?;
        }
    }
    Ok(())
}

/// Whether `profile` can launch `agent` at all — the refusal a person's row
/// gets before it runs, whatever the lineup says.
pub fn launchable(agent: &str, profile: &Profile) -> Result<(), String> {
    if profile.model.trim().is_empty()
        || profile.effort.trim().is_empty()
        || profile.model.chars().any(char::is_whitespace)
        || profile.effort.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "{PROFILES_SETTING}: model and effort must be nonempty"
        ));
    }
    if crate::orchestration::native_agent(&profile.model).is_some_and(|native| native != agent) {
        return Err(format!("{agent}: model belongs to another agent"));
    }
    Ok(())
}

/// An explicit settings entry replaces its default row, including invalid
/// entries: a typo must not silently launch an unintended model.
pub fn profile(root: &Value, agent: &str, difficulty: &str) -> Result<Option<Profile>, String> {
    profile_in(root, agent, difficulty, None)
}

/// [`profile`] read against today's lineup ([`lineup::rows`]): a default row
/// is the lineup's model for the difficulty, and the shipped table is the
/// road back when no lineup was read.
pub fn profile_in(
    root: &Value,
    agent: &str,
    difficulty: &str,
    lineup: Option<&lineup::Lineup>,
) -> Result<Option<Profile>, String> {
    Ok(lineup::row_at(root, agent, difficulty, lineup, None, 0)?.map(|row| row.profile()))
}
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
    /// Whether the executed model/effort matches the configured high row.
    pub baseline_high: bool,
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

/// Legacy effort-to-difficulty context for old replay files; never a label.
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

/// Keep the old pin as context, never as a success label.
pub fn compare(row: &mut Value, teacher_effort: Option<&str>) {
    row["pinnedEffort"] = json!(teacher_effort);
    if let Some(object) = row.as_object_mut() {
        object.remove(crate::jev::summary::AGREED.canonical);
        object.remove(crate::jev::summary::BASELINE_AGREED.canonical);
        object.remove("teacher");
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
    fn profiles_use_the_editable_table_and_refuse_invalid_overrides() {
        let defaults = profiles(&Value::Null);
        validate_profiles(&defaults).unwrap();
        for (agent, levels) in defaults.as_object().unwrap() {
            for (difficulty, configured) in levels.as_object().unwrap() {
                assert_eq!(
                    serde_json::to_value(profile(&Value::Null, agent, difficulty).unwrap())
                        .unwrap(),
                    *configured
                );
            }
        }
        let mut changed = defaults.clone();
        changed["codex"][LADDER[0].0]["model"] = json!("gpt-test-profile");
        let root = json!({crate::jev::SMART_SETTINGS_KEY:{PROFILES_SETTING:changed}});
        assert_eq!(
            profile(&root, "codex", LADDER[0].0).unwrap().unwrap().model,
            "gpt-test-profile"
        );
        changed["codex"][LADDER[0].0]["effort"] = json!("");
        assert!(validate_profiles(&changed).is_err());
    }
    #[test]
    fn a_pin_is_context_and_cannot_award_success() {
        let mut row = json!({"chosen": LADDER[2].0});
        compare(&mut row, Some(LADDER[2].2));
        assert!(row.get("agreed").is_none());
        assert!(row.get("baselineAgreed").is_none());
    }
    #[test]
    fn rubric_version_is_pinned_to_question_option_meanings_and_evidence_fields() {
        assert_eq!(RUBRIC_VERSION, 2);
        assert_eq!(
            crate::jev::rubric_fingerprint(rubric_words),
            "cfd9c076bf3f6257"
        );
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
