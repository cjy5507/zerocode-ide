//! The person's edits to built-in decisions, kept in personal settings.
//! A deleted decision stays deleted when the global switch is enabled again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{JEV_USES, JevMode, JevUse};

pub const SETTING: &str = "builtinDecisions";
pub const NAME_MAX: usize = 80;
pub const LEGACY_QUESTION_MAX: usize = 2_000;
// Built-in instructions can be longer than a user-created decision's prompt.
pub const QUESTION_MAX: usize = 4_000;
pub const CRITERIA_MAX: usize = 8_000;
pub const RUBRIC_KEY: &str = "decisionRubric";
pub const EDITED_RUBRIC_FLAG: u32 = 1 << 31;
const DECISION_STATE_KEY: &str = "__builtinDecision";
const QUESTION_SCHEMA: u64 = 2;

mod definitions;
mod schema;
#[cfg(test)]
mod schema_tests;
pub use definitions::default_questions;
pub use schema::{QuestionDefinition, QuestionEdit, QuestionKind, validate_against};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Decision {
    pub name: Option<String>,
    pub question: Option<String>,
    pub criteria: Option<String>,
    pub mode: Option<String>,
    pub deleted: bool,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub questions: BTreeMap<String, QuestionEdit>,
}

impl Decision {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    #[must_use]
    pub fn edited(&self) -> bool {
        self.question.is_some() || self.criteria.is_some() || !self.questions.is_empty()
    }

    /// Custom wording has its own rubric. It must never promote the shipped
    /// question using evidence gathered under a person's changed criteria.
    #[must_use]
    pub fn rubric(&self, base_version: u32) -> Option<u32> {
        if !self.edited() {
            return None;
        }
        let words = if self.questions.is_empty() {
            serde_json::to_vec(&(base_version, &self.question, &self.criteria))
        } else {
            let value = serde_json::to_value((
                base_version,
                &self.question,
                &self.criteria,
                &self.questions,
            ))
            .ok()?;
            serde_json::to_vec(&schema::canonical(&value))
        }
        .ok()?;
        let digest = Sha256::digest(words);
        Some(u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) | EDITED_RUBRIC_FLAG)
    }

    /// Older clients cannot apply edited wording. Their existing mode key
    /// holds those requests off; current clients read this definition first.
    #[must_use]
    pub fn legacy_mode(&self, seat: &JevUse) -> Option<JevMode> {
        if self.deleted || self.edited() {
            Some(JevMode::Off)
        } else {
            self.mode(seat)
        }
    }

    #[must_use]
    pub fn mode(&self, seat: &JevUse) -> Option<JevMode> {
        if self.deleted {
            return Some(JevMode::Off);
        }
        let mode = self
            .mode
            .as_ref()
            .map(|word| seat.mode_of(Some(&Value::String(word.clone()))));
        if self.edited() && mode.is_none_or(JevMode::automatic) {
            Some(if seat.modes.contains(&JevMode::Shadow) {
                JevMode::Shadow
            } else {
                JevMode::Off
            })
        } else {
            mode
        }
    }
}

pub type Decisions = BTreeMap<String, Decision>;

#[must_use]
pub fn seat(id: &str) -> Option<&'static JevUse> {
    JEV_USES
        .iter()
        .find(|row| row.id == id && row.id != super::CUSTOM_DECISIONS.id)
}

#[must_use]
pub fn in_root(root: &Value, id: &str) -> Option<Decision> {
    seat(id)?;
    let saved = root.get(super::SMART_SETTINGS_KEY)?.get(SETTING)?;
    let Some(decisions) = saved.as_object() else {
        return Some(Decision {
            deleted: true,
            ..Decision::default()
        });
    };
    let value = decisions.get(id)?;
    // A malformed saved override must not silently resurrect its decision.
    let mut decision: Decision =
        serde_json::from_value(value.clone()).unwrap_or_else(|_| Decision {
            deleted: true,
            ..Decision::default()
        });
    if let Some(schema) = value.get("questionSchema") {
        // A still-running older zo sees the legacy tombstone and asks nothing.
        // Only a client understanding the typed definition reads its real state.
        decision.deleted = schema.as_u64() != Some(QUESTION_SCHEMA)
            || decision.questions.is_empty()
            || value.get("questionDeleted").and_then(Value::as_bool) != Some(false);
    }
    Some(decision)
}

/// Persist typed edits with an off marker older clients already understand.
/// The editor and current runtime still exchange the logical `Decision`.
pub fn storage_value(decision: &Decision) -> Result<Value, serde_json::Error> {
    let mut value = serde_json::to_value(decision)?;
    if !decision.questions.is_empty() {
        value["deleted"] = Value::Bool(true);
        value["questionSchema"] = Value::from(QUESTION_SCHEMA);
        value["questionDeleted"] = Value::Bool(decision.deleted);
    }
    Ok(value)
}

#[must_use]
pub fn read(root: &Value) -> Decisions {
    JEV_USES
        .iter()
        .filter_map(|row| in_root(root, row.id).map(|value| (row.id.to_string(), value)))
        .collect()
}

/// Validate edits at the settings write boundary. Refusals contain no input.
pub fn validate(id: &str, draft: &Decision) -> Result<(), &'static str> {
    let row = seat(id).ok_or("builtin_decision_missing")?;
    schema::validate_edits(&draft.questions)?;
    if !draft.questions.is_empty()
        && let Some(definitions) = default_questions(id)
    {
        schema::validate_against(&draft.questions, &definitions)?;
    }
    for (value, cap, token) in [
        (&draft.name, NAME_MAX, "builtin_decision_name"),
        (
            &draft.question,
            LEGACY_QUESTION_MAX,
            "builtin_decision_question",
        ),
        (&draft.criteria, CRITERIA_MAX, "builtin_decision_criteria"),
    ] {
        if let Some(value) = value {
            if value.trim().is_empty() || value.chars().count() > cap {
                return Err(token);
            }
            if value.lines().any(crate::credential::may_carry_a_credential) {
                return Err("builtin_decision_credential");
            }
        }
    }
    if let Some(mode) = &draft.mode {
        let chosen = row.modes.iter().find(|choice| choice.key() == mode);
        if chosen.is_none() || (draft.edited() && chosen.is_some_and(|choice| choice.automatic())) {
            return Err("builtin_decision_mode");
        }
    }
    if row.id == super::JUDGMENT_CACHE.id && draft.edited() {
        return Err("builtin_decision_no_question");
    }
    Ok(())
}

/// The request's existing subject and output schema stay intact. The user's
/// question and criteria replace its evaluation wording, after their own
/// redaction and bounds; caller context was already cleared by the door.
pub fn apply(draft: &Decision, body: &mut Value) -> Result<usize, &'static str> {
    let withheld = apply_legacy(draft, body);
    schema::apply(&draft.questions, body)?;
    Ok(withheld)
}

fn apply_legacy(draft: &Decision, body: &mut Value) -> usize {
    if !draft.edited() {
        return 0;
    }
    let (question, question_withheld) = super::door::clear_text(
        draft.question.as_deref().unwrap_or_default(),
        super::Cap::Chars(LEGACY_QUESTION_MAX),
    );
    let (criteria, criteria_withheld) = super::door::clear_text(
        draft.criteria.as_deref().unwrap_or_default(),
        super::Cap::Chars(CRITERIA_MAX),
    );
    if question.is_empty() && criteria.is_empty() {
        return question_withheld + criteria_withheld;
    }
    // A batch carries the edited wording once, not once per note or control.
    let shared = body
        .get_mut("state")
        .and_then(Value::as_object_mut)
        .is_some_and(|state| {
            state.insert(
                DECISION_STATE_KEY.to_string(),
                serde_json::json!({"question": question, "criteria": criteria}),
            );
            true
        });
    let Some(questions) = body.get_mut("questions").and_then(Value::as_object_mut) else {
        return question_withheld + criteria_withheld;
    };
    for (id, field) in questions.iter_mut() {
        if draft
            .questions
            .keys()
            .any(|pattern| schema::matches_id(pattern, id))
        {
            continue;
        }
        let Some(field) = field.as_object_mut() else {
            continue;
        };
        let original = field.get("instructions").map_or(String::new(), |words| {
            words
                .as_str()
                .map_or_else(|| words.to_string(), str::to_string)
        });
        let mut edited = if shared {
            String::from(
                "Apply nonempty saved question and criteria from state.__builtinDecision instead of the default evaluation below. Keep this item's scope, output type and choices.\n",
            )
        } else {
            String::from(
                "Apply the following saved question and criteria instead of the default evaluation below. Keep this item's scope, output type and choices.\n",
            )
        };
        if !shared && !question.is_empty() {
            edited.push_str("Question:\n");
            edited.push_str(&question);
            edited.push('\n');
        }
        if !shared && !criteria.is_empty() {
            edited.push_str("Criteria:\n");
            edited.push_str(&criteria);
            edited.push('\n');
        }
        edited.push_str("Item context:\n");
        edited.push_str(&original);
        field.insert("instructions".to_string(), Value::String(edited));
    }
    question_withheld + criteria_withheld
}

/// Shipped words, read from the same functions whose versions the tests pin.
/// Some zo seats own their words in its runtime; the companion supplies those.
#[must_use]
pub fn default_words(id: &str) -> Option<String> {
    type Words = (&'static str, fn() -> String);
    let words: &[Words] = &[
        (super::ROUTING.id, super::questions::routing_rubric_words),
        (
            super::SKILL_SUGGESTION.id,
            super::questions::skill_suggestion_rubric_words,
        ),
        (super::BROWSER.id, crate::screen_action::rubric_words),
        (super::DESKTOP.id, crate::screen_action::rubric_words),
        (super::EMULATOR.id, crate::screen_action::rubric_words),
        (super::COVER.id, super::questions::cover_rubric_words),
        (super::STALL.id, crate::stall_cause::rubric_words),
        (super::PLACEMENT.id, crate::worker_placement::rubric_words),
        (super::SUMMON.id, crate::summon_choice::rubric_words),
        (
            super::SUMMON_DIFFICULTY.id,
            crate::summon_difficulty::rubric_words,
        ),
        (super::SUMMON_MODEL.id, crate::summon_model::rubric_words),
        (super::STEP_EFFORT.id, crate::step_effort::rubric_words),
        (super::BROWSER_READ.id, crate::browser_read::rubric_words),
        (super::NOTIFY.id, crate::notify_call::rubric_words),
        (super::BRANCHING.id, crate::branching::rubric_words),
        (super::MAIL_TRIAGE.id, crate::mail_triage::rubric_words),
        (super::CHALLENGER.id, super::challenger::rubric_words),
        (super::CLAIM.id, super::claim::rubric_words),
        (
            super::VAULT_PAIRS.id,
            super::questions::vault_pair_rubric_words,
        ),
        (super::FILE_PICK.id, super::file_pick::rubric_words),
        (
            super::COMMAND_GUARD.id,
            super::questions::command_guard_rubric_words,
        ),
        (
            super::TOOL_TEXT_GUARD.id,
            super::questions::tool_text_guard_rubric_words,
        ),
        (
            super::REFLEX_DECIDE.id,
            super::questions::reflex_decide_rubric_words,
        ),
    ];
    if let Some((_, words)) = words.iter().find(|(seat, _)| *seat == id) {
        return Some(words());
    }
    [
        super::AGENT_TOOL.id,
        super::PROJECT_RULES.id,
        super::JUDGMENT_CACHE.id,
    ]
    .contains(&id)
    .then(String::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jev::{PLACEMENT, door, promote};
    use serde_json::json;

    #[test]
    fn an_individual_noul_edit_reaches_its_question_and_criteria_only() {
        let draft: Decision = serde_json::from_value(json!({
            "questions": {"irreversible": {
                "type": "noul",
                "instructions": "Does this operation discard the original data?",
                "criteria": {"true": "The original data is discarded.", "false": "The original data remains."}
            }}
        })).unwrap();
        let mut body = json!({"state": {"command": "read"}, "questions": {
            "irreversible": super::super::noul::question("The original question.", "Original yes.", "Original no."),
            "outside": super::super::noul::question("Does it write outside the project?", "Outside.", "Inside.")
        }});
        let outside = body["questions"]["outside"].clone();
        let _ = apply(&draft, &mut body);
        assert_eq!(
            body["questions"]["irreversible"]["instructions"],
            "Does this operation discard the original data?"
        );
        assert_eq!(
            body["questions"]["irreversible"]["criteria"]["true"],
            "The original data is discarded."
        );
        assert_eq!(body["questions"]["irreversible"]["type"], "noul");
        assert_eq!(body["questions"]["outside"], outside);
        assert!(draft.edited());
    }

    #[test]
    fn a_deleted_builtin_stays_off_when_the_global_switch_is_turned_back_on() {
        let root =
            json!({"smart":{"jev":{"enabled":true}, (SETTING):{(PLACEMENT.id):{"deleted":true}}}});
        assert_eq!(PLACEMENT.mode_in(&root), JevMode::Off);
        let mut settings = door::JevSettings::from_root(&root);
        settings.workspaces = vec!["/work/acme".into()];
        let asked = door::Asking {
            key: true,
            settings: &settings,
            workspace: Some("/work/acme"),
            sent_today: 0,
        };
        assert_eq!(
            door::may_send(&PLACEMENT, &asked, json!({"questions":{}})),
            Err(door::Refused::Off)
        );
        let malformed = json!({"smart":{"jev":{"enabled":true}, (SETTING):{(PLACEMENT.id):{"deleted":true,"question":7}}}});
        assert_eq!(PLACEMENT.mode_in(&malformed), JevMode::Off);
    }

    #[test]
    fn edited_questions_reach_the_common_door_without_changing_output_fields() {
        let folder = tempfile::tempdir().unwrap();
        let workspace = folder.path().to_string_lossy().into_owned();
        let edit = Decision {
            question: Some("Which result helps the current task?".into()),
            criteria: Some("Prefer direct evidence over guesses.".into()),
            mode: Some(JevMode::Shadow.key().into()),
            ..Decision::default()
        };
        let settings = door::JevSettings {
            decisions: BTreeMap::from([(PLACEMENT.id.into(), edit.clone())]),
            enabled: true,
            workspaces: vec![workspace.clone()],
            daily_requests: None,
            model: "fixture-model".into(),
        }
        .resolved();
        let asking = door::Asking {
            key: true,
            settings: &settings,
            workspace: Some(&workspace),
            sent_today: 0,
        };
        let body = json!({"state":{},"questions":{
            "first":{"type":"choice","instructions":"Evaluate notes[0].","criteria":{"keep":"Keep this note.","skip":"Skip this note."}},
            "second":{"type":"choice","instructions":"Evaluate notes[1].","criteria":{"keep":"Keep this note.","skip":"Skip this note."}}
        }});
        let cleared = door::may_send(&PLACEMENT, &asking, body.clone()).unwrap();
        let sent: Value = serde_json::from_slice(cleared.bytes()).unwrap();
        for (key, target) in [("first", "notes[0]"), ("second", "notes[1]")] {
            let text = sent["questions"][key]["instructions"].as_str().unwrap();
            assert!(text.contains(DECISION_STATE_KEY));
            assert_eq!(
                sent["state"][DECISION_STATE_KEY]["question"],
                edit.question.as_ref().unwrap().as_str()
            );
            assert_eq!(
                sent["state"][DECISION_STATE_KEY]["criteria"],
                edit.criteria.as_ref().unwrap().as_str()
            );
            assert!(text.contains(target));
            assert_eq!(
                sent["questions"][key]["type"],
                body["questions"][key]["type"]
            );
            assert_eq!(
                sent["questions"][key]["criteria"],
                body["questions"][key]["criteria"]
            );
        }
        assert_eq!(
            cleared.decision_rubric(),
            edit.rubric(PLACEMENT.rubric_version)
        );
    }

    #[test]
    fn edited_wording_has_its_own_rubric_and_never_promotes_the_shipped_question() {
        let edit = Decision {
            question: Some("A different question".into()),
            ..Decision::default()
        };
        let row = json!({"outcome":"answered","requests":1,"model":"fixture-model","rubricVersion":PLACEMENT.rubric_version, (RUBRIC_KEY):edit.rubric(PLACEMENT.rubric_version).unwrap()});
        let mut original = row.clone();
        original.as_object_mut().unwrap().remove(RUBRIC_KEY);
        assert_eq!(
            promote::on_the_newest_version(&PLACEMENT, &[original])
                .requests
                .len(),
            1
        );
        let rows = [row];
        assert!(
            promote::on_the_newest_version(&PLACEMENT, &rows)
                .requests
                .is_empty()
        );
        assert_eq!(edit.mode(&PLACEMENT), Some(JevMode::Shadow));
        let mut changed = edit.clone();
        changed.criteria = Some("A different criterion".into());
        assert_ne!(
            edit.rubric(PLACEMENT.rubric_version),
            changed.rubric(PLACEMENT.rubric_version)
        );
        changed.mode = Some(JevMode::Auto.key().into());
        assert_eq!(
            validate(PLACEMENT.id, &changed),
            Err("builtin_decision_mode")
        );
    }

    #[test]
    fn default_wording_is_the_same_text_as_the_versioned_placement_rubric() {
        assert_eq!(
            default_words(PLACEMENT.id),
            Some(crate::worker_placement::rubric_words())
        );
        assert!(seat(crate::jev::CUSTOM_DECISIONS.id).is_none());
        assert_eq!(
            validate("unknown-feature", &Decision::default()),
            Err("builtin_decision_missing")
        );
    }

    #[test]
    fn older_settings_and_unknown_metadata_keep_their_existing_meaning() {
        let old = json!({"smart":{"jev":{"enabled":true}}});
        assert!(door::JevSettings::from_root(&old).decisions.is_empty());
        assert_eq!(PLACEMENT.mode_in(&old), PLACEMENT.recommended);
        let future = json!({"smart":{"jev":{"enabled":true}, (SETTING):{
            (PLACEMENT.id):{"deleted":true,"futureMetadata":{"version":2}},
            "future-feature":{"deleted":true}
        }}});
        let settings = door::JevSettings::from_root(&future);
        assert_eq!(settings.decisions.len(), 1);
        assert!(settings.decisions[PLACEMENT.id].deleted);
        assert_eq!(PLACEMENT.mode_in(&future), JevMode::Off);
    }

    #[test]
    fn shipped_builtin_ids_remain_valid_keys_for_saved_deletions() {
        // These are persisted keys, not the current table generated again.
        // Adding a seat is fine; renaming one requires a settings migration.
        let ids = [
            "routing",
            "recall",
            "skills",
            "skill_suggestion",
            "browser",
            "desktop",
            "emulator",
            "cover",
            "stall",
            "placement",
            "summon",
            "summon_difficulty",
            "summon_model",
            "model_tiers",
            "effort",
            "step_effort",
            "compaction",
            "agent_tool",
            "project_rules",
            "browser_read",
            "notify",
            "mention_rerank",
            "branching",
            "judgment_cache",
            "mail_triage",
            "challenger",
            "patch_review",
            "claim",
            "vault_pairs",
            "file_pick",
            "command_guard",
            "tool_text_guard",
            "reflex_decide",
        ];
        let deleted: serde_json::Map<String, Value> = ids
            .iter()
            .map(|id| ((*id).to_string(), json!({"deleted": true})))
            .collect();
        let mut root = json!({"smart":{(SETTING):deleted}});
        door::switch_on(root["smart"].as_object_mut().unwrap()).unwrap();
        for id in ids {
            let row =
                seat(id).unwrap_or_else(|| panic!("saved decision id {id} needs a migration"));
            assert!(in_root(&root, id).unwrap().deleted);
            assert_eq!(row.mode_in(&root), JevMode::Off, "{id}");
        }
    }

    #[test]
    fn user_wording_is_redacted_before_it_enters_the_shared_request_context() {
        let secret = ["unit", "fixture", "placeholder"].join("-");
        let edit = Decision {
            criteria: Some(format!(
                "Authorization: Bearer {secret}\nUse direct evidence."
            )),
            ..Decision::default()
        };
        assert_eq!(
            validate(PLACEMENT.id, &edit),
            Err("builtin_decision_credential")
        );
        let mut body = json!({"state":{},"questions":{"result":{"type":"noul","instructions":"Assess the current item."}}});
        assert_eq!(apply(&edit, &mut body), Ok(1));
        assert!(!body.to_string().contains(&secret));
        assert!(
            body["state"][DECISION_STATE_KEY]["criteria"]
                .as_str()
                .unwrap()
                .contains("Use direct evidence.")
        );
    }
}
