//! The editable part of a System One question, separate from its answer key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{CRITERIA_MAX, QUESTION_MAX};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionKind {
    Noul,
    Choice,
    Score,
}

impl QuestionKind {
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Noul => "noul",
            Self::Choice => "choice",
            Self::Score => "score",
        }
    }
}

/// Values retain JSON structure, including an explicit null description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionEdit {
    #[serde(rename = "type")]
    pub kind: QuestionKind,
    pub instructions: Value,
    pub criteria: Value,
    /// A repeated question still needs the runtime item's original scope.
    #[serde(default, skip_serializing_if = "is_false")]
    pub contextual: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuestionDefinition {
    /// An exact wire id, or one bounded prefix/suffix pattern for a batch.
    pub id: String,
    #[serde(flatten)]
    pub question: QuestionEdit,
    /// Other choices come from the current candidates, not a saved list.
    #[serde(default)]
    pub dynamic_options: bool,
}

impl QuestionDefinition {
    /// Read the actual request builder's output without flattening its values.
    pub fn from_questions(questions: Value) -> Result<Vec<Self>, &'static str> {
        let questions = questions
            .as_object()
            .ok_or("builtin_decision_question_schema")?;
        questions
            .iter()
            .map(|(id, value)| {
                let question = serde_json::from_value(value.clone())
                    .map_err(|_| "builtin_decision_question_schema")?;
                Ok(Self {
                    id: id.clone(),
                    question,
                    dynamic_options: false,
                })
            })
            .collect()
    }
    #[must_use]
    pub fn noul(id: &str, instructions: &str, yes: &str, no: &str) -> Self {
        Self::new(
            id,
            QuestionKind::Noul,
            instructions,
            json!({"true": yes, "false": no}),
        )
    }

    #[must_use]
    pub fn choice(id: &str, instructions: &str, options: &[(&str, &str)]) -> Self {
        Self::new(
            id,
            QuestionKind::Choice,
            instructions,
            Value::Object(
                options
                    .iter()
                    .map(|(key, words)| ((*key).into(), Value::from(*words)))
                    .collect(),
            ),
        )
    }

    #[must_use]
    pub fn score(id: &str, instructions: &str, levels: &[&str]) -> Self {
        Self::new(id, QuestionKind::Score, instructions, json!(levels))
    }

    #[must_use]
    pub fn new(id: &str, kind: QuestionKind, instructions: &str, criteria: Value) -> Self {
        Self {
            id: id.to_string(),
            question: QuestionEdit {
                kind,
                instructions: Value::from(instructions),
                criteria,
                contextual: false,
            },
            dynamic_options: false,
        }
    }

    #[must_use]
    pub fn contextual(mut self) -> Self {
        self.question.contextual = true;
        self
    }

    #[must_use]
    pub fn dynamic_options(mut self) -> Self {
        self.dynamic_options = true;
        self
    }
}

pub fn validate_edits(edits: &BTreeMap<String, QuestionEdit>) -> Result<(), &'static str> {
    if edits.len() > 64 {
        return Err("builtin_decision_questions");
    }
    for (id, edit) in edits {
        if id.is_empty() || id.len() > 128 || id.matches('*').count() > 1 {
            return Err("builtin_decision_questions");
        }
        validate_content(&edit.instructions, QUESTION_MAX)?;
        validate_content(&edit.criteria, CRITERIA_MAX)?;
        match edit.kind {
            QuestionKind::Noul if edit.criteria.is_null() => {}
            QuestionKind::Noul => {
                let options = edit
                    .criteria
                    .as_object()
                    .ok_or("builtin_decision_options")?;
                if options.keys().any(|key| key != "true" && key != "false") {
                    return Err("builtin_decision_options");
                }
                for value in options.values() {
                    validate_content(value, CRITERIA_MAX)?;
                }
            }
            QuestionKind::Choice if edit.criteria.is_object() => {
                for value in edit.criteria.as_object().into_iter().flat_map(Map::values) {
                    validate_content(value, CRITERIA_MAX)?;
                }
            }
            QuestionKind::Score => {
                let levels = edit.criteria.as_array().ok_or("builtin_decision_options")?;
                if !(2..=10).contains(&levels.len()) {
                    return Err("builtin_decision_options");
                }
                for level in levels {
                    validate_content(level, CRITERIA_MAX)?;
                }
            }
            _ => return Err("builtin_decision_options"),
        }
    }
    Ok(())
}

fn validate_content(value: &Value, cap: usize) -> Result<(), &'static str> {
    let encoded = value.to_string();
    if !matches!(
        value,
        Value::Null | Value::String(_) | Value::Array(_) | Value::Object(_)
    ) || encoded.chars().count() > cap
    {
        return Err("builtin_decision_question_content");
    }
    // Authored descriptions can discuss passwords or tokens. The raw-tool
    // filter rejects even that prose and the fixed choice key `ask_person`.
    // Read values with the shared value-aware mask, and inspect structured
    // keys separately so a password value cannot hide behind a JSON key.
    let mut left = vec![value];
    while let Some(value) = left.pop() {
        match value {
            Value::String(text) if crate::private_data::mask(text).found.credential > 0 => {
                return Err("builtin_decision_credential");
            }
            Value::Array(values) => left.extend(values),
            Value::Object(values) => {
                if values.keys().any(|key| {
                    crate::credential::names_a_credential(key)
                        || crate::private_data::mask(key).found.credential > 0
                }) {
                    return Err("builtin_decision_credential");
                }
                left.extend(values.values());
            }
            _ => {}
        }
    }
    Ok(())
}

pub fn validate_against(
    edits: &BTreeMap<String, QuestionEdit>,
    definitions: &[QuestionDefinition],
) -> Result<(), &'static str> {
    validate_edits(edits)?;
    for (id, edit) in edits {
        let original = definitions
            .iter()
            .find(|row| &row.id == id)
            .ok_or("builtin_decision_question_schema")?;
        if edit.kind != original.question.kind || edit.contextual != original.question.contextual {
            return Err("builtin_decision_question_schema");
        }
        if edit.kind == QuestionKind::Choice {
            let known = original
                .question
                .criteria
                .as_object()
                .ok_or("builtin_decision_question_schema")?;
            if edit
                .criteria
                .as_object()
                .is_some_and(|options| options.keys().any(|key| !known.contains_key(key)))
            {
                return Err("builtin_decision_question_schema");
            }
        }
        let _ = changed_criteria(edit, &original.question.criteria)?;
    }
    Ok(())
}

pub(super) fn matches_id(pattern: &str, id: &str) -> bool {
    match pattern.split_once('*') {
        Some((start, end)) => {
            id.len() >= start.len() + end.len()
                && id.starts_with(start)
                && id.ends_with(end)
                && !id[start.len()..id.len() - end.len()].contains('/')
        }
        None => pattern == id,
    }
}

/// Build all replacements first. A stale shape changes no part of a request.
pub(super) fn apply(
    edits: &BTreeMap<String, QuestionEdit>,
    body: &mut Value,
) -> Result<(), &'static str> {
    if edits.is_empty() {
        return Ok(());
    }
    validate_edits(edits)?;
    let questions = body
        .get_mut("questions")
        .and_then(Value::as_object_mut)
        .ok_or("builtin_decision_question_schema")?;
    let mut replacements = Vec::new();
    for (id, original) in questions.iter() {
        let mut matching = edits.iter().filter(|(pattern, _)| matches_id(pattern, id));
        let Some((_, edit)) = matching.next() else {
            continue;
        };
        if matching.next().is_some()
            || original.get("type").and_then(Value::as_str) != Some(edit.kind.key())
        {
            return Err("builtin_decision_question_schema");
        }
        let criteria = changed_criteria(edit, original.get("criteria").unwrap_or(&Value::Null))?;
        let instructions = if edit.contextual {
            json!({
                "instructions": edit.instructions,
                "item_context": original.get("instructions"),
                "scope": "Use item_context to identify the subject of this question. Evaluate that subject using instructions, which replace its default evaluation wording."
            })
        } else {
            edit.instructions.clone()
        };
        let mut changed = original
            .as_object()
            .cloned()
            .ok_or("builtin_decision_question_schema")?;
        changed.insert("instructions".into(), instructions);
        changed.insert("criteria".into(), criteria);
        replacements.push((id.clone(), Value::Object(changed)));
    }
    questions.extend(replacements);
    Ok(())
}

fn changed_criteria(edit: &QuestionEdit, original: &Value) -> Result<Value, &'static str> {
    match edit.kind {
        QuestionKind::Noul if edit.criteria.is_null() => Ok(Value::Null),
        QuestionKind::Noul => {
            let options = edit
                .criteria
                .as_object()
                .ok_or("builtin_decision_options")?;
            if options.is_empty() {
                return Ok(original.clone());
            }
            let mut kept = original.as_object().cloned().unwrap_or_default();
            kept.extend(options.clone());
            Ok(Value::Object(kept))
        }
        QuestionKind::Choice => {
            let options = edit
                .criteria
                .as_object()
                .ok_or("builtin_decision_options")?;
            let mut kept: Map<String, Value> = original
                .as_object()
                .cloned()
                .ok_or("builtin_decision_question_schema")?;
            // The runtime can offer a subset of the definition (for example,
            // no split in a narrow window). Never add an unavailable choice.
            for (key, value) in options {
                if let Some(target) = kept.get_mut(key) {
                    *target = value.clone();
                }
            }
            Ok(Value::Object(kept))
        }
        QuestionKind::Score => {
            if edit.criteria.as_array().map(Vec::len) != original.as_array().map(Vec::len) {
                return Err("builtin_decision_question_schema");
            }
            Ok(edit.criteria.clone())
        }
    }
}

/// Both workspaces hash the same content with or without preserve_order.
pub(super) fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(values) => Value::Object(
            values
                .iter()
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .map(|(key, value)| (key.clone(), canonical(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(canonical).collect()),
        _ => value.clone(),
    }
}
