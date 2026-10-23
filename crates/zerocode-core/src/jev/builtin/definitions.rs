//! Editor definitions come from the question owners, never rubric fingerprints.

use super::QuestionDefinition;
use crate::jev::{self, questions as q};

#[must_use]
pub fn default_questions(id: &str) -> Option<Vec<QuestionDefinition>> {
    type Definition = (&'static str, fn() -> Vec<QuestionDefinition>);
    let definitions: &[Definition] = &[
        (jev::BROWSER.id, crate::screen_action::question_definitions),
        (jev::DESKTOP.id, crate::screen_action::question_definitions),
        (jev::EMULATOR.id, crate::screen_action::question_definitions),
        (jev::STALL.id, crate::stall_cause::question_definitions),
        (
            jev::PLACEMENT.id,
            crate::worker_placement::question_definitions,
        ),
        (jev::SUMMON.id, crate::summon_choice::question_definitions),
        (
            jev::SUMMON_DIFFICULTY.id,
            crate::summon_difficulty::question_definitions,
        ),
        (
            jev::SUMMON_MODEL.id,
            crate::summon_model::question_definitions,
        ),
        (
            jev::STEP_EFFORT.id,
            crate::step_effort::question_definitions,
        ),
        (
            jev::BROWSER_READ.id,
            crate::browser_read::question_definitions,
        ),
        (jev::NOTIFY.id, crate::notify_call::question_definitions),
        (jev::BRANCHING.id, crate::branching::question_definitions),
        (
            jev::MAIL_TRIAGE.id,
            crate::mail_triage::question_definitions,
        ),
        (jev::CHALLENGER.id, jev::challenger::question_definitions),
        (jev::CLAIM.id, jev::claim::question_definitions),
        (jev::FILE_PICK.id, jev::file_pick::question_definitions),
        (
            jev::REFLEX_DECIDE.id,
            jev::reflex_decide::question_definitions,
        ),
    ];
    if let Some((_, make)) = definitions.iter().find(|(seat, _)| *seat == id) {
        return Some(make());
    }
    if id == jev::COMMAND_GUARD.id {
        return Some(
            q::COMMAND_GUARD_QUESTIONS
                .iter()
                .map(|[id, asks, yes, no]| QuestionDefinition::noul(id, asks, yes, no))
                .collect(),
        );
    }
    if id == jev::TOOL_TEXT_GUARD.id {
        return Some(vec![QuestionDefinition::noul(
            q::INSTRUCTED,
            q::TOOL_TEXT_INSTRUCTED_ASKS,
            q::TOOL_TEXT_INSTRUCTED_YES,
            q::TOOL_TEXT_INSTRUCTED_NO,
        )]);
    }
    if id == jev::COVER.id {
        return Some(vec![
            QuestionDefinition::noul(
                q::COVER_COVERED,
                q::COVER_COVERED_ASKS,
                q::COVER_COVERED_YES,
                q::COVER_COVERED_NO,
            ),
            QuestionDefinition::choice(q::COVER_KIND, q::COVER_KIND_ASKS, &q::COVER_KINDS),
            QuestionDefinition::choice(q::COVER_MOVE, q::COVER_MOVE_ASKS, &q::COVER_MOVES),
            QuestionDefinition::noul(
                q::COVER_REVERSIBLE,
                q::COVER_REVERSIBLE_ASKS,
                q::COVER_REVERSIBLE_YES,
                q::COVER_REVERSIBLE_NO,
            ),
        ]);
    }
    if id == jev::VAULT_PAIRS.id {
        return Some(vec![
            QuestionDefinition::score(
                "link_state",
                q::VAULT_PAIR_LINK_QUESTION,
                &q::VAULT_PAIR_LINK_LEVELS,
            ),
            QuestionDefinition::noul(
                "same_claim",
                q::VAULT_PAIR_SAME_CLAIM,
                q::VAULT_PAIR_YES,
                q::VAULT_PAIR_NO,
            ),
            QuestionDefinition::noul(
                "opposite_claim",
                q::VAULT_PAIR_OPPOSITE_CLAIM,
                q::VAULT_PAIR_YES,
                q::VAULT_PAIR_NO,
            ),
            QuestionDefinition::noul(
                "replaces",
                q::VAULT_PAIR_REPLACES,
                q::VAULT_PAIR_YES,
                q::VAULT_PAIR_NO,
            ),
        ]);
    }
    [
        jev::AGENT_TOOL.id,
        jev::PROJECT_RULES.id,
        jev::JUDGMENT_CACHE.id,
    ]
    .contains(&id)
    .then(Vec::new)
}
