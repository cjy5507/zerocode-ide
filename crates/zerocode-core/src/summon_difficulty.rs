//! A summons' difficulty and editable launch profiles.
//! Pins describe the executed baseline; only finished work grades a choice.
use std::collections::BTreeSet;

use crate::jev::{Cap, choice};
use serde_json::{Value, json};

pub const RUBRIC_VERSION: u32 = 2;
pub mod lineup;
pub mod outcomes;
pub const PROFILES_SETTING: &str = "summonProfiles";
/// The shipped launch table: the model each agent's difficulty falls back to
/// when no lineup names one. Its rows name models only — no effort is ours
/// to ship (t-74181): the model seat chooses one with the model, the person
/// may write one, and otherwise the agent's CLI runs at its own default.
pub const DEFAULT_PROFILES: &str = include_str!("summon-profiles.json");
/// The difficulty a summons nobody judged runs at (t-45961): the ladder's
/// top, whose row is the person's own where they wrote one and otherwise the
/// lineup's hard rung — the provider's top implementation model. Never the
/// middle: an unjudged summons cannot tell easy work from hard, and a
/// misread lineup put its weakest model on the middle row (10-10~11: six
/// unjudged summonses out of six on the cheapest model).
pub const UNJUDGED_DIFFICULTY: &str = LADDER[2].0;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub model: String,
    /// `None`: no effort goes on the command line, and the agent's CLI runs
    /// at its own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// One difficulty's row as a person wrote it. Either field may be left to
/// the lineup, which fills it at every read: a model written alone runs at
/// the difficulty's effort brought inside what it accepts, and an effort
/// written alone rides the lineup's model, brought inside what that accepts.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Written {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Written {
    /// Neither field written: the row follows the lineup whole.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.model.is_none() && self.effort.is_none()
    }

    /// The row as the launch profile it names, when it writes a model.
    #[must_use]
    pub fn whole(&self) -> Option<Profile> {
        Some(Profile {
            model: self.model.clone()?,
            effort: self.effort.clone(),
        })
    }
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
            if !LADDER.iter().any(|(key, _)| key == difficulty) {
                return Err(format!("{agent}: unknown difficulty {difficulty}"));
            }
            let written: Written =
                serde_json::from_value(row.clone()).map_err(|e| format!("{agent}: {e}"))?;
            written_launchable(agent, &written)?;
        }
    }
    Ok(())
}

/// Whether `profile` can launch `agent` at all — the refusal a person's row
/// gets before it runs, whatever the lineup says.
pub fn launchable(agent: &str, profile: &Profile) -> Result<(), String> {
    written_launchable(
        agent,
        &Written {
            model: Some(profile.model.clone()),
            effort: profile.effort.clone(),
        },
    )
}

/// [`launchable`] for a row that may leave a field to the lineup: what is
/// written must be one word, and a row writes at least one field.
pub fn written_launchable(agent: &str, written: &Written) -> Result<(), String> {
    let word = |value: &str| !value.trim().is_empty() && !value.chars().any(char::is_whitespace);
    if written.is_empty()
        || !written.model.as_deref().is_none_or(word)
        || !written.effort.as_deref().is_none_or(word)
    {
        return Err(format!(
            "{PROFILES_SETTING}: model and effort must be nonempty"
        ));
    }
    // An agent that runs any provider's model (zo) may name another CLI's
    // model; one tied to its own provider may not.
    if let Some(model) = written.model.as_deref()
        && crate::orchestration::native_agent(model).is_some_and(|native| native != agent)
        && !crate::orchestration::runs_model(agent, model)
    {
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

/// One ladder: option and meaning. A difficulty picks a model from the
/// lineup; it sets no effort (t-74181).
pub const LADDER: [(&str, &str); 3] = [
    (
        "low",
        "Documentation, translation, tests only, or a small fix.",
    ),
    ("mid", "A UI feature, tooling, benchmarking, or a TUI port."),
    (
        "high",
        "Core rules, protocols, concurrency, or ledger contracts.",
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
            .map(|(key, means)| ((*key).to_string(), json!(means)))
            .collect(),
    )
}

pub fn read(answers: &Value) -> Result<choice::Choice, choice::ChoiceRefusal> {
    let offered: BTreeSet<String> = LADDER.iter().map(|(key, _)| (*key).to_string()).collect();
    choice::read(answers, QUESTION, &offered)
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
        .chain(LADDER.iter().flat_map(|(key, means)| [*key, *means]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Structured editor fields, using the same words and answer keys as requests.
#[must_use]
pub fn question_definitions() -> Vec<crate::jev::builtin::QuestionDefinition> {
    use crate::jev::builtin::QuestionDefinition as D;
    vec![D::choice(QUESTION, INSTRUCTIONS, &LADDER)]
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
        changed["codex"][LADDER[0].0]["model"] = json!("gpt-test-profile");
        assert!(validate_profiles(&changed).is_err());
    }
    /// t-74181 (source contract): no difficulty-to-effort table, in code or
    /// in data. The ladder carries an option and its meaning only, the
    /// shipped table names models only, and the code that builds a default
    /// row names no effort word — what a summons nobody chose an effort for
    /// runs at is its CLI's own default.
    #[test]
    fn no_difficulty_to_effort_table_is_written_in_code_or_data() {
        let quoted = |body: &str| -> Vec<&str> {
            crate::capabilities::EFFORT_ORDER
                .iter()
                .copied()
                .filter(|word| body.contains(&format!("\"{word}\"")))
                .collect()
        };
        let own = include_str!("summon_difficulty.rs");
        let code = own.split("#[cfg(test)]").next().expect("code");
        let (before, ladder) = code.split_once("pub const LADDER").expect("LADDER");
        let (ladder, after) = ladder.split_once("];").expect("LADDER ends");
        assert!(
            ladder.starts_with(": [(&str, &str); "),
            "the ladder has a column beside option and meaning:{ladder}"
        );
        assert_eq!(quoted(&format!("{before}{after}")), Vec::<&str>::new());
        let lineup = include_str!("summon_difficulty/lineup.rs");
        let lineup = lineup.split("#[cfg(test)]").next().expect("code");
        assert_eq!(quoted(lineup), Vec::<&str>::new(), "lineup.rs");
        let shipped: Value = serde_json::from_str(DEFAULT_PROFILES).unwrap();
        for (agent, rows) in shipped.as_object().unwrap() {
            for (difficulty, row) in rows.as_object().unwrap() {
                assert!(
                    row.get("effort").is_none(),
                    "summon-profiles.json {agent} {difficulty} ships an effort: {row}"
                );
            }
        }
        let orchestration = include_str!("orchestration.rs");
        assert!(
            !orchestration.contains("fn difficulty_effort("),
            "a difficulty is turned into an effort in code"
        );
    }

    /// t-74181 (review P3-2): a table of efforts — by difficulty, by agent,
    /// in a match or in a list — sets effort words side by side. No line of
    /// the code that builds, asks about or launches a summons' row holds two:
    /// the efforts a CLI takes are named once where they were measured
    /// (`capabilities`) and read from there, so a table moved into another of
    /// these files, or behind another function's name, is still caught.
    #[test]
    fn no_line_of_the_summons_code_sets_efforts_side_by_side() {
        let efforts_on = |line: &str| {
            crate::capabilities::EFFORT_ORDER
                .iter()
                .filter(|word| line.contains(&format!("\"{word}\"")))
                .count()
        };
        for (file, source) in [
            ("summon_difficulty.rs", include_str!("summon_difficulty.rs")),
            (
                "summon_difficulty/lineup.rs",
                include_str!("summon_difficulty/lineup.rs"),
            ),
            ("summon_model.rs", include_str!("summon_model.rs")),
            ("summon_assign.rs", include_str!("summon_assign.rs")),
            // Its tests live in files of their own.
            ("orchestration.rs", include_str!("orchestration.rs")),
        ] {
            let code = source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .expect("code");
            let side_by_side: Vec<(usize, &str)> = code
                .lines()
                .enumerate()
                .filter(|(_, line)| efforts_on(line) > 1)
                .map(|(at, line)| (at + 1, line.trim()))
                .collect();
            assert_eq!(side_by_side, Vec::<(usize, &str)>::new(), "{file}");
        }
    }

    #[test]
    fn a_written_row_may_leave_a_field_to_the_lineup_but_not_both() {
        let mut table = json!({"codex": {
            LADDER[0].0: {"effort": "high"},
            LADDER[2].0: {"model": "gpt-test-profile"},
        }});
        validate_profiles(&table).unwrap();
        table["codex"][LADDER[1].0] = json!({});
        assert!(validate_profiles(&table).is_err(), "a row writes something");
        table["codex"][LADDER[1].0] = json!({"model": "gpt-test-profile", "note": "x"});
        assert!(validate_profiles(&table).is_err(), "and nothing else");
    }
    #[test]
    fn a_pin_is_context_and_cannot_award_success() {
        let mut row = json!({"chosen": LADDER[2].0});
        compare(&mut row, Some("max"));
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
